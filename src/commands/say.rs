use anyhow::Result;
use serde_json::json;

pub async fn run(
    client: &reqwest::Client,
    base: &str,
    text: String,
    agent_id: Option<&str>,
) -> Result<()> {
    let body = json!({
        "text": text,
        "agent_id": agent_id.unwrap_or("unknown"),
    });
    let resp = client.post(format!("{base}/chat")).json(&body).send().await?;
    if resp.status().is_success() {
        let val: serde_json::Value = resp.json().await?;
        println!("✓ sent (id: {})", val["id"].as_str().unwrap_or("?"));
    } else {
        anyhow::bail!("post failed: {}", resp.status());
    }
    Ok(())
}
