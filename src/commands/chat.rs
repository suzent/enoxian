use anyhow::Result;
use std::collections::HashSet;
use crate::control::ChatMessage;

pub async fn run(
    client: &reqwest::Client,
    base: &str,
    follow: bool,
    since: Option<i64>,
) -> Result<()> {
    let mut url = format!("{base}/chat");
    if let Some(s) = since {
        url.push_str(&format!("?since={s}"));
    }
    let messages: Vec<ChatMessage> = client.get(&url).send().await?.json().await?;
    for msg in &messages {
        print_message(msg);
    }

    if !follow {
        return Ok(());
    }

    // Track IDs already shown so we don't duplicate messages that arrive
    // between the history fetch and the SSE subscription.
    let seen: HashSet<String> = messages.into_iter().map(|m| m.id).collect();

    println!("◆ Following chat (Ctrl+C to stop)...");

    // Build a dedicated client with no timeout — SSE connections stay open indefinitely.
    let stream_client = reqwest::Client::builder().build()?;

    let mut resp = stream_client
        .get(format!("{base}/chat/stream"))
        .header("Accept", "text/event-stream")
        .send()
        .await?;

    while let Some(chunk) = resp.chunk().await? {
        let text = String::from_utf8_lossy(&chunk);
        for line in text.lines() {
            if let Some(data) = line.strip_prefix("data: ") {
                if let Ok(val) = serde_json::from_str::<serde_json::Value>(data) {
                    if val["type"].as_str() == Some("message_posted") {
                        if let Ok(msg) = serde_json::from_value::<ChatMessage>(val["message"].clone()) {
                            if !seen.contains(&msg.id) {
                                print_message(&msg);
                            }
                        }
                    }
                }
            }
        }
    }
    Ok(())
}

fn print_message(msg: &ChatMessage) {
    let dt = chrono::DateTime::from_timestamp(msg.ts, 0)
        .map(|t| t.format("%Y-%m-%d %H:%M").to_string())
        .unwrap_or_else(|| msg.ts.to_string());
    println!("[{dt}] {}: {}", msg.agent_id, msg.text);
}
