//! `enox channels` — the Circle's topic channels. See
//! `docs/development/designs/channels.md`.
use crate::cli::ChannelsAction;
use anyhow::Result;
use serde_json::{json, Value};

pub async fn run(
    client: &reqwest::Client,
    base: &str,
    action: Option<ChannelsAction>,
    actor_token: Option<&str>,
    json_out: bool,
) -> Result<()> {
    match action {
        None => list(client, base, json_out).await,
        Some(ChannelsAction::Create { name, purpose }) => {
            let agent_id = super::say::fetch_agent_id(client, base).await;
            let mut body = json!({ "name": name, "purpose": purpose, "agent_id": agent_id });
            if let Some(token) = actor_token {
                body["actor_token"] = Value::String(token.to_string());
            }
            let resp = client
                .post(format!("{base}/channels"))
                .json(&body)
                .send()
                .await?;
            let info = checked(resp).await?;
            if json_out {
                println!("{}", serde_json::to_string_pretty(&info)?);
            } else {
                println!("✓ created #{}", info["name"].as_str().unwrap_or("?"));
            }
            Ok(())
        }
        Some(ChannelsAction::Rename { channel, name }) => {
            let reference = channel.trim().trim_start_matches('#');
            let resp = client
                .patch(format!("{base}/channels/{reference}"))
                .json(&json!({ "name": name }))
                .send()
                .await?;
            let info = checked(resp).await?;
            if json_out {
                println!("{}", serde_json::to_string_pretty(&info)?);
            } else {
                println!("✓ renamed to #{}", info["name"].as_str().unwrap_or("?"));
            }
            Ok(())
        }
        Some(ChannelsAction::Archive { channel }) => {
            set_archived(client, base, &channel, true, json_out).await
        }
        Some(ChannelsAction::Unarchive { channel }) => {
            set_archived(client, base, &channel, false, json_out).await
        }
    }
}

async fn list(client: &reqwest::Client, base: &str, json_out: bool) -> Result<()> {
    let channels = fetch(client, base).await?;
    if json_out {
        println!("{}", serde_json::to_string_pretty(&channels)?);
        return Ok(());
    }
    let (archived, live): (Vec<&Value>, Vec<&Value>) =
        channels.iter().partition(|c| c["archived"] == true);
    for c in &live {
        let purpose = c["purpose"].as_str().unwrap_or("");
        let count = c["message_count"].as_u64().unwrap_or(0);
        let line = format!(
            "#{:<20} {count:>5} messages",
            c["name"].as_str().unwrap_or("?")
        );
        if purpose.is_empty() {
            println!("{line}");
        } else {
            println!("{line}  — {purpose}");
        }
    }
    if !archived.is_empty() {
        let names: Vec<String> = archived
            .iter()
            .map(|c| format!("#{}", c["name"].as_str().unwrap_or("?")))
            .collect();
        println!("\nArchived: {}", names.join(", "));
    }
    println!("\nRead one with: enox chat --channel <name>");
    Ok(())
}

/// Every channel, `#general` first.
pub async fn fetch(client: &reqwest::Client, base: &str) -> Result<Vec<Value>> {
    let resp = client.get(format!("{base}/channels")).send().await?;
    let val = checked(resp).await?;
    Ok(val.as_array().cloned().unwrap_or_default())
}

/// The id of the channel `reference` names (`None` for `#general`), resolved
/// the way the daemon does for the common cases: an id, or a unique name.
pub async fn resolve(
    client: &reqwest::Client,
    base: &str,
    reference: Option<&str>,
) -> Result<Option<String>> {
    let Some(raw) = reference.map(str::trim).filter(|r| !r.is_empty()) else {
        return Ok(None);
    };
    let name = raw.trim_start_matches('#').to_ascii_lowercase();
    if name == crate::channel::GENERAL {
        return Ok(None);
    }
    let channels = fetch(client, base).await?;
    if let Some(c) = channels.iter().find(|c| c["id"].as_str() == Some(raw)) {
        return Ok(c["id"].as_str().map(str::to_string));
    }
    let named: Vec<&Value> = channels
        .iter()
        .filter(|c| c["name"] == name.as_str())
        .collect();
    let pick = match named.as_slice() {
        [one] => Some(*one),
        many => {
            let live: Vec<&&Value> = many.iter().filter(|c| c["archived"] != true).collect();
            match live.as_slice() {
                [one] => Some(**one),
                [] if many.is_empty() => anyhow::bail!("unknown channel #{name}"),
                _ => anyhow::bail!("several channels are named #{name}; use an id"),
            }
        }
    };
    Ok(pick.and_then(|c| c["id"].as_str().map(str::to_string)))
}

async fn set_archived(
    client: &reqwest::Client,
    base: &str,
    channel: &str,
    archived: bool,
    json_out: bool,
) -> Result<()> {
    let reference = channel.trim().trim_start_matches('#');
    let resp = client
        .patch(format!("{base}/channels/{reference}"))
        .json(&json!({ "archived": archived }))
        .send()
        .await?;
    let info = checked(resp).await?;
    if json_out {
        println!("{}", serde_json::to_string_pretty(&info)?);
    } else {
        let verb = if archived { "archived" } else { "unarchived" };
        println!("✓ {verb} #{}", info["name"].as_str().unwrap_or("?"));
    }
    Ok(())
}

async fn checked(resp: reqwest::Response) -> Result<Value> {
    let status = resp.status();
    let val: Value = resp.json().await.unwrap_or_default();
    if !status.is_success() {
        anyhow::bail!(
            "{} ({status})",
            val["error"].as_str().unwrap_or("request failed")
        );
    }
    Ok(val)
}
