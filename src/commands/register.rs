use anyhow::{bail, Result};
use serde_json::{json, Value};

pub async fn run(
    client: &reqwest::Client,
    base: &str,
    agent_id: String,
    json_out: bool,
) -> Result<()> {
    let resp = client
        .post(format!("{base}/actors/register"))
        .json(&json!({ "agent_id": agent_id }))
        .send()
        .await?;
    let status = resp.status();
    let val: Value = resp.json().await?;
    if !status.is_success() {
        bail!(
            "registration failed: {}",
            val["error"].as_str().unwrap_or("unknown error")
        );
    }
    if json_out {
        println!("{}", serde_json::to_string_pretty(&val)?);
    } else {
        println!("{}", val["token"].as_str().unwrap_or_default());
        eprintln!(
            "Registered {} on device {} until {}. Pass this value with --token; treat it as a short-lived secret.",
            val["agent_id"].as_str().unwrap_or("agent"),
            val["peer_id"].as_str().unwrap_or("unknown"),
            val["expires_at"].as_str().unwrap_or("unknown")
        );
    }
    Ok(())
}
