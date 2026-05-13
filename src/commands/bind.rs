use anyhow::Result;
use serde_json::{json, Value};

pub async fn run(client: &reqwest::Client, base: &str, path: String, json_out: bool) -> Result<()> {
    let agent_id = std::env::var("ENOCHIAN_AGENT_ID").unwrap_or_else(|_| "cli".to_string());
    let resp = client
        .post(format!("{base}/bind"))
        .json(&json!({ "path": path, "agent_id": agent_id }))
        .send().await?;
    let status = resp.status();
    let val: Value = resp.json().await?;
    if json_out {
        println!("{}", serde_json::to_string_pretty(&val)?);
    } else if status.is_success() {
        println!("✦ bound: {path}");
    } else {
        println!("✗ bind failed: {}", val["error"].as_str().unwrap_or("unknown"));
        if let Some(holder) = val["held_by"].as_str() {
            println!("  held by: {holder}");
        }
    }
    Ok(())
}
