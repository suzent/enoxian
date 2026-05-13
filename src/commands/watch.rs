use anyhow::Result;

pub async fn run(client: &reqwest::Client, base: &str) -> Result<()> {
    println!("◆ Watching circle events (Ctrl+C to stop)...");
    let mut resp = client
        .get(format!("{base}/events"))
        .header("Accept", "text/event-stream")
        .send()
        .await?;

    while let Some(chunk) = resp.chunk().await? {
        let text = String::from_utf8_lossy(&chunk);
        for line in text.lines() {
            if let Some(data) = line.strip_prefix("data: ") {
                if let Ok(val) = serde_json::from_str::<serde_json::Value>(data) {
                    let event_type = val["type"].as_str().unwrap_or("unknown");
                    println!("  [{event_type}] {val}");
                }
            }
        }
    }
    Ok(())
}
