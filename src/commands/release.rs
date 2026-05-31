use anyhow::Result;
use serde_json::{json, Value};

pub async fn run(client: &reqwest::Client, base: &str, path: String, json_out: bool) -> Result<()> {
    let agent_id = std::env::var("enoxian_AGENT_ID").unwrap_or_else(|_| "cli".to_string());
    let resp = client
        .post(format!("{base}/release"))
        .json(&json!({ "path": path, "agent_id": agent_id }))
        .send().await?;
    let val: Value = resp.json().await?;
    if json_out {
        println!("{}", serde_json::to_string_pretty(&val)?);
    } else {
        println!("✦ released: {path}");
    }
    Ok(())
}
