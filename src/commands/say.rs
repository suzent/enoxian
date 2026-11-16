use anyhow::{Context, Result};
use serde_json::{json, Value};
use std::path::PathBuf;

pub async fn run(
    client: &reqwest::Client,
    base: &str,
    text: String,
    attachments: Vec<PathBuf>,
    reply_to: Option<String>,
    actor_token: Option<&str>,
) -> Result<()> {
    if text.trim().is_empty() && attachments.is_empty() {
        anyhow::bail!("message must have text or at least one --attach <path>");
    }

    let mut uploaded = Vec::with_capacity(attachments.len());
    for path in &attachments {
        let bytes = std::fs::read(path)
            .with_context(|| format!("failed to read attachment {}", path.display()))?;
        if bytes.is_empty() {
            anyhow::bail!("attachment is empty: {}", path.display());
        }
        if bytes.len() > crate::api::attachments::MAX_ATTACHMENT_BYTES {
            anyhow::bail!(
                "attachment {} exceeds maximum size ({} MB)",
                path.display(),
                crate::api::attachments::MAX_ATTACHMENT_BYTES / (1024 * 1024)
            );
        }
        let name = path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("attachment.bin");
        let resp = client
            .post(format!("{base}/chat/attachments"))
            .query(&[("name", name)])
            .header("Content-Type", "application/octet-stream")
            .body(bytes)
            .send()
            .await
            .with_context(|| format!("failed to upload {}", path.display()))?;
        if !resp.status().is_success() {
            let status = resp.status();
            let err: Value = resp.json().await.unwrap_or_default();
            let detail = err["error"].as_str().unwrap_or("upload failed");
            anyhow::bail!("uploading {} failed ({status}): {detail}", path.display());
        }
        let val: Value = resp.json().await?;
        let hash = val["hash"]
            .as_str()
            .context("upload response missing hash")?
            .to_string();
        let clean_name = val["name"].as_str().unwrap_or(name).to_string();
        uploaded.push(json!({
            "hash": hash,
            "name": clean_name,
        }));
    }

    let agent_id = fetch_agent_id(client, base).await;
    let mut body = json!({
        "text": text,
        "agent_id": agent_id,
        "attachments": uploaded,
        "reply_to": reply_to,
    });
    if let Some(token) = actor_token {
        body["actor_token"] = Value::String(token.to_string());
    }
    let resp = client
        .post(format!("{base}/chat"))
        .json(&body)
        .send()
        .await?;
    if resp.status().is_success() {
        let val: Value = resp.json().await?;
        println!("✓ sent (id: {})", val["id"].as_str().unwrap_or("?"));
    } else {
        let status = resp.status();
        let err: Value = resp.json().await.unwrap_or_default();
        let detail = err["error"].as_str().unwrap_or("");
        if detail.is_empty() {
            anyhow::bail!("post failed: {status}");
        } else {
            anyhow::bail!("post failed ({status}): {detail}");
        }
    }
    Ok(())
}

async fn fetch_agent_id(client: &reqwest::Client, base: &str) -> String {
    async {
        let val: Value = client
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
