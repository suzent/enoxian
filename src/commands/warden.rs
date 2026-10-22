//! `enox warden` — the warden's mode, its records, and the spec it decides by.
//!
//! `episodes` is the one an experiment uses: it prints every decision in the
//! Circle joined with what became of it, one JSON object per line, ready to be
//! replayed against another spec.
use crate::cli::{WardenAction, WardenSpecAction};
use anyhow::Result;
use serde_json::{json, Value};

async fn read(resp: reqwest::Response) -> Result<Value> {
    let status = resp.status();
    let val: Value = resp.json().await?;
    if !status.is_success() {
        anyhow::bail!("{}", val["error"].as_str().unwrap_or("request failed"));
    }
    Ok(val)
}

/// The circle id inside `<root>/circles/<id>/api`.
fn circle_id(base: &str) -> Option<&str> {
    base.split("/circles/").nth(1)?.split('/').next()
}

pub async fn run(
    client: &reqwest::Client,
    root: &str,
    base: &str,
    action: Option<WardenAction>,
    json_out: bool,
) -> Result<()> {
    match action {
        None => status(client, root, base, json_out).await,
        Some(WardenAction::Mode { mode, all }) => {
            if !matches!(mode.as_str(), "off" | "shadow" | "act") {
                anyhow::bail!("mode must be off, shadow or act");
            }
            let mut body = json!({ "warden_mode": mode });
            if !all {
                body["circle_id"] = json!(circle_id(base).unwrap_or_default());
            }
            read(
                client
                    .post(format!("{root}/api/agent-config/engagement"))
                    .json(&body)
                    .send()
                    .await?,
            )
            .await?;
            if !json_out {
                println!(
                    "Warden {} {}.",
                    if all { "default is now" } else { "here is now" },
                    mode
                );
            }
            Ok(())
        }
        Some(WardenAction::Decisions { limit }) => {
            let val = read(
                client
                    .get(format!("{base}/warden/decisions"))
                    .query(&[("limit", limit.to_string())])
                    .send()
                    .await?,
            )
            .await?;
            if json_out {
                println!("{}", serde_json::to_string_pretty(&val)?);
                return Ok(());
            }
            println!("Mode here: {}", val["mode"].as_str().unwrap_or("off"));
            for record in val["records"].as_array().into_iter().flatten() {
                println!("{}", describe(record));
            }
            Ok(())
        }
        Some(WardenAction::Episodes { since }) => {
            let mut query = vec![("format", "jsonl".to_string())];
            if let Some(since) = since {
                query.push(("since", since.to_string()));
            }
            let resp = client
                .get(format!("{base}/warden/episodes"))
                .query(&query)
                .send()
                .await?;
            if !resp.status().is_success() {
                read(resp).await?;
                return Ok(());
            }
            print!("{}", resp.text().await?);
            Ok(())
        }
        Some(WardenAction::Spec { action }) => match action.unwrap_or(WardenSpecAction::Show) {
            WardenSpecAction::Show => {
                let val = read(client.get(format!("{root}/api/warden")).send().await?).await?;
                let spec: crate::agent::warden::spec::WardenSpec =
                    serde_json::from_value(val["spec"].clone())?;
                if json_out {
                    println!("{}", serde_json::to_string_pretty(&val["spec"])?);
                } else {
                    print!("{}", toml::to_string_pretty(&spec)?);
                }
                Ok(())
            }
            WardenSpecAction::Apply { file } => {
                let text = std::fs::read_to_string(&file)?;
                let val = read(
                    client
                        .post(format!("{root}/api/warden/spec"))
                        .json(&json!({ "toml": text }))
                        .send()
                        .await?,
                )
                .await?;
                if !json_out {
                    println!(
                        "Spec {} is now in force.",
                        val["spec"]["version"].as_str().unwrap_or("?")
                    );
                }
                Ok(())
            }
            WardenSpecAction::Reset => {
                read(
                    client
                        .post(format!("{root}/api/warden/spec/reset"))
                        .send()
                        .await?,
                )
                .await?;
                if !json_out {
                    println!("The built-in spec is in force again.");
                }
                Ok(())
            }
        },
        Some(WardenAction::Test { text }) => {
            let val = read(
                client
                    .post(format!("{root}/api/warden/test"))
                    .json(&json!({ "text": text, "circle_id": circle_id(base) }))
                    .send()
                    .await?,
            )
            .await?;
            if json_out {
                println!("{}", serde_json::to_string_pretty(&val)?);
                return Ok(());
            }
            println!("{}", describe_decision(&val["decision"]));
            Ok(())
        }
    }
}

async fn status(client: &reqwest::Client, root: &str, base: &str, json_out: bool) -> Result<()> {
    let device = read(client.get(format!("{root}/api/warden")).send().await?).await?;
    let here = read(
        client
            .get(format!("{base}/warden/decisions"))
            .query(&[("limit", "5")])
            .send()
            .await?,
    )
    .await?;
    if json_out {
        println!(
            "{}",
            serde_json::to_string_pretty(&json!({ "device": device, "circle": here }))?
        );
        return Ok(());
    }
    println!("Mode here:  {}", here["mode"].as_str().unwrap_or("off"));
    println!(
        "Decider:    {}",
        device["config"]["decider"].as_str().unwrap_or("?")
    );
    if let Some(problem) = device["problem"].as_str() {
        println!("            not usable ({problem}); the built-in rules decide meanwhile");
    }
    println!(
        "Spec:       {} ({})",
        device["spec"]["version"].as_str().unwrap_or("?"),
        device["spec_source"]["kind"].as_str().unwrap_or("?")
    );
    let records = here["records"].as_array().cloned().unwrap_or_default();
    if !records.is_empty() {
        println!("\nRecent decisions here:");
        for record in &records {
            println!("  {}", describe(record));
        }
    }
    Ok(())
}

fn describe(record: &Value) -> String {
    let id: String = record["message_id"]
        .as_str()
        .unwrap_or("")
        .chars()
        .take(8)
        .collect();
    let phase = record["phase"].as_str().unwrap_or("");
    if let Some(vote) = record.get("vote").filter(|v| !v.is_null()) {
        return format!(
            "{id}  vote     {} — groups {}",
            vote["action"].as_str().unwrap_or(""),
            vote["groups"]
        );
    }
    let ran = match record.get("executed").filter(|v| !v.is_null()) {
        Some(e) => format!(
            " · ran: {} {}",
            e["by"].as_str().unwrap_or(""),
            e["agents"]
                .as_array()
                .map(|a| a
                    .iter()
                    .filter_map(Value::as_str)
                    .collect::<Vec<_>>()
                    .join(", "))
                .unwrap_or_default()
        ),
        None => String::new(),
    };
    format!(
        "{id}  {phase:<8} {}{ran}",
        describe_decision(&record["decision"])
    )
}

fn describe_decision(decision: &Value) -> String {
    let choice = &decision["choice"];
    let what = if choice["admit"] == true {
        format!(
            "{} × {}",
            choice["mode"].as_str().unwrap_or("?"),
            choice["listeners"]
        )
    } else {
        "nobody".to_string()
    };
    let mut line = format!(
        "{what} — {} [{}]",
        decision["reason"].as_str().unwrap_or(""),
        decision["decider"].as_str().unwrap_or("")
    );
    if let Some(fallback) = decision["fallback"].as_str() {
        line.push_str(&format!(" (fallback: {fallback})"));
    }
    line
}

#[cfg(test)]
mod tests {
    #[test]
    fn the_circle_id_is_read_from_the_base() {
        assert_eq!(
            super::circle_id("http://127.0.0.1:36521/circles/abc-123/api"),
            Some("abc-123")
        );
    }
}
