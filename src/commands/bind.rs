use anyhow::Result;
use serde_json::{json, Value};

pub async fn run(
    client: &reqwest::Client,
    base: &str,
    path: String,
    actor_token: Option<&str>,
    json_out: bool,
) -> Result<()> {
    let agent_id = std::env::var("ENOXIAN_AGENT_ID")
        .or_else(|_| std::env::var("enoxian_AGENT_ID"))
        .unwrap_or_else(|_| "cli".to_string());
    let mut body = json!({ "path": path, "agent_id": agent_id });
    if let Some(token) = actor_token {
        body["actor_token"] = Value::String(token.to_string());
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
        println!("✦ bound: {path}");
    } else {
        println!(
            "✗ bind failed: {}",
            val["error"].as_str().unwrap_or("unknown")
        );
        if let Some(holder) = val["held_by"].as_str() {
            println!("  held by: {holder}");
        }
    }
    Ok(())
}
