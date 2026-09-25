//! `enox inbox` — what is waiting for an agent, and taking a message.
use anyhow::Result;
use serde_json::{json, Value};

/// The name claims are made as, matching `enox bind` and `enox claim`.
fn agent_name(explicit: Option<String>) -> String {
    explicit
        .or_else(|| std::env::var("ENOXIAN_AGENT_ID").ok())
        .unwrap_or_else(|| "cli".to_string())
}

pub async fn list(
    client: &reqwest::Client,
    base: &str,
    agent: Option<String>,
    json_out: bool,
) -> Result<()> {
    let agent = agent_name(agent);
    let resp = client
        .get(format!("{base}/inbox"))
        .query(&[("agent", agent.as_str())])
        .send()
        .await?;
    let status = resp.status();
    let val: Value = resp.json().await?;
    if json_out {
        println!("{}", serde_json::to_string_pretty(&val)?);
        return Ok(());
    }
    if !status.is_success() {
        anyhow::bail!("{}", val["error"].as_str().unwrap_or("inbox unavailable"));
    }

    let waiting = val["waiting"].as_array().cloned().unwrap_or_default();
    println!("Waiting for {agent} on this device ({})", waiting.len());
    for w in &waiting {
        println!(
            "  {}  {:<8} {}",
            short(&w["message_id"]),
            w["status"].as_str().unwrap_or(""),
            line(&w["text"])
        );
    }

    let open = val["open"].as_array().cloned().unwrap_or_default();
    println!("\nOpen — nobody has answered ({})", open.len());
    for m in &open {
        // `label` names the machine too; an older daemon only sends `agent`.
        let held = m["claimed_by"]["label"]
            .as_str()
            .or_else(|| m["claimed_by"]["agent"].as_str())
            .map(|who| format!("  [{who} has it]"))
            .unwrap_or_default();
        println!(
            "  {}  {}: {}{held}",
            short(&m["message_id"]),
            m["from"].as_str().unwrap_or("?"),
            line(&m["text"])
        );
    }
    if !open.is_empty() {
        println!("\nTake one with: enox inbox claim <id>");
    }
    Ok(())
}

pub async fn claim(
    client: &reqwest::Client,
    base: &str,
    agent: Option<String>,
    message_id: String,
    ttl: Option<i64>,
    actor_token: Option<&str>,
    json_out: bool,
) -> Result<()> {
    let mut body = json!({"message_id": message_id, "agent_id": agent_name(agent)});
    if let Some(ttl) = ttl {
        body["ttl_secs"] = json!(ttl);
    }
    send(client, base, "claim", body, actor_token, json_out).await
}

pub async fn release(
    client: &reqwest::Client,
    base: &str,
    agent: Option<String>,
    message_id: String,
    actor_token: Option<&str>,
    json_out: bool,
) -> Result<()> {
    let body = json!({"message_id": message_id, "agent_id": agent_name(agent)});
    send(client, base, "release", body, actor_token, json_out).await
}

async fn send(
    client: &reqwest::Client,
    base: &str,
    action: &str,
    mut body: Value,
    actor_token: Option<&str>,
    json_out: bool,
) -> Result<()> {
    if let Some(token) = actor_token {
        body["actor_token"] = Value::String(token.to_string());
    }
    let resp = client
        .post(format!("{base}/inbox/{action}"))
        .json(&body)
        .send()
        .await?;
    let status = resp.status();
    let val: Value = resp.json().await?;
    if json_out {
        println!("{}", serde_json::to_string_pretty(&val)?);
    } else if !status.is_success() {
        anyhow::bail!("{}", val["error"].as_str().unwrap_or("request failed"));
    } else if val["claimed"] == true {
        let secs = val["expires_at"].as_i64().unwrap_or(0) - chrono::Utc::now().timestamp();
        println!(
            "✦ {} has {} for {}m — ambient agents will leave it alone",
            val["agent"].as_str().unwrap_or("?"),
            short(&val["message_id"]),
            (secs.max(0) + 59) / 60
        );
    } else {
        println!("✓ released {}", short(&val["message_id"]));
    }
    Ok(())
}

fn short(v: &Value) -> String {
    v.as_str().unwrap_or("?").chars().take(8).collect()
}

fn line(v: &Value) -> String {
    let text = v.as_str().unwrap_or("").replace('\n', " ");
    let clipped: String = text.chars().take(72).collect();
    if text.chars().count() > 72 {
        format!("{clipped}…")
    } else {
        clipped
    }
}
