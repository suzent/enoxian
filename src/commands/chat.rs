use crate::control::ChatMessage;
use anyhow::Result;
use std::collections::HashSet;

pub async fn run(
    client: &reqwest::Client,
    base: &str,
    channel: Option<String>,
    follow: bool,
    since: Option<i64>,
) -> Result<()> {
    // Resolved here so `--follow` can tell this channel's messages apart.
    let channel = super::channels::resolve(client, base, channel.as_deref()).await?;
    let mut query: Vec<(&str, String)> = Vec::new();
    if let Some(s) = since {
        query.push(("since", s.to_string()));
    }
    if let Some(id) = &channel {
        query.push(("channel", id.clone()));
    }
    let resp = client
        .get(format!("{base}/chat"))
        .query(&query)
        .send()
        .await?;
    if !resp.status().is_success() {
        let status = resp.status();
        let err: serde_json::Value = resp.json().await.unwrap_or_default();
        anyhow::bail!(
            "{} ({status})",
            err["error"].as_str().unwrap_or("chat unavailable")
        );
    }
    let messages: Vec<ChatMessage> = resp.json().await?;
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
                        if let Ok(msg) =
                            serde_json::from_value::<ChatMessage>(val["message"].clone())
                        {
                            if !seen.contains(&msg.id) && msg.channel == channel {
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
    if msg.attachments.is_empty() {
        println!("[{dt}] {}: {}", msg.agent_id, msg.text);
    } else {
        let files: Vec<String> = msg
            .attachments
            .iter()
            .map(|a| {
                format!(
                    "{} ({}, {} KB)",
                    a.name,
                    a.mime,
                    a.size.div_ceil(1024).max(1)
                )
            })
            .collect();
        if msg.text.trim().is_empty() {
            println!("[{dt}] {}: [attached: {}]", msg.agent_id, files.join(", "));
        } else {
            println!(
                "[{dt}] {}: {} [attached: {}]",
                msg.agent_id,
                msg.text,
                files.join(", ")
            );
        }
    }
}
