use anyhow::Result;
use serde_json::json;

pub async fn run(
    client: &reqwest::Client,
    base: &str,
    text: String,
) -> Result<()> {
    let agent_id = fetch_agent_id(client, base).await;
    let body = json!({
        "text": text,
        "agent_id": agent_id,
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

async fn fetch_agent_id(client: &reqwest::Client, base: &str) -> String {
    async {
        let val: serde_json::Value = client
            .get(format!("{base}/status"))
            .send()
            .await?
            .json()
            .await?;
        anyhow::Ok(val["agent_id"].as_str().unwrap_or("unknown").to_string())
    }
    .await
    .unwrap_or_else(|_| "unknown".to_string())
}
