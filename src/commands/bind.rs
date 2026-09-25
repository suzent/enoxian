use anyhow::Result;
use serde_json::{json, Value};

pub async fn run(
    client: &reqwest::Client,
    base: &str,
    path: String,
    ttl: Option<i64>,
    takeover: bool,
    actor_token: Option<&str>,
    json_out: bool,
) -> Result<()> {
    let agent_id = std::env::var("ENOXIAN_AGENT_ID")
        .or_else(|_| std::env::var("enoxian_AGENT_ID"))
        .unwrap_or_else(|_| "cli".to_string());
    let mut body = json!({ "path": path, "agent_id": agent_id, "takeover": takeover });
    if let Some(ttl) = ttl {
        body["ttl"] = json!(ttl);
    }
    if let Some(token) = actor_token {
        body["actor_token"] = Value::String(token.to_string());
    }
    if let Ok(run) = std::env::var("ENOXIAN_RUN_ID") {
        body["run_id"] = Value::String(run);
    }
    let resp = client
        .post(format!("{base}/bind"))
        .json(&body)
        .send()
        .await?;
    let status = resp.status();
    let val: Value = resp.json().await?;
    if json_out {
        println!("{}", serde_json::to_string_pretty(&val)?);
    } else if status.is_success() {
        let until = until(&val["expires_at"]);
        match val["taken_over_from"].as_str() {
            Some(previous) => {
                println!("✦ bound: {path} until {until} (taken over from {previous})")
            }
            None => println!("✦ bound: {path} until {until}"),
        }
    } else {
        println!(
            "✗ bind failed: {}",
            val["error"].as_str().unwrap_or("unknown")
        );
        if let Some(holder) = val["held_by"].as_str() {
            println!("  held by: {holder} until {}", until(&val["expires_at"]));
        }
    }
    anyhow::ensure!(
        status.is_success(),
        "bind rejected: {}",
        val["error"].as_str().unwrap_or("conflict")
    );
    Ok(())
}

/// A lease end as local wall-clock time, for a reader deciding when to renew.
pub(crate) fn until(value: &Value) -> String {
    value
        .as_str()
        .and_then(|s| chrono::DateTime::parse_from_rfc3339(s).ok())
        .map(|t| {
            t.with_timezone(&chrono::Local)
                .format("%H:%M:%S")
                .to_string()
        })
        .unwrap_or_else(|| "?".into())
}
