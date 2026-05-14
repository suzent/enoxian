use anyhow::{Context, Result};

use crate::{
    cli::MemberAction,
    config::{self, circle_dir},
    crypto::keypair_from_hex,
    resolve,
};

pub async fn run(
    client: &reqwest::Client,
    daemon_base: &str,
    circle_hint: Option<&str>,
    action: MemberAction,
    json: bool,
) -> Result<()> {
    let configs = config::load_all()?;
    let cfg = match circle_hint {
        Some(h) => resolve::resolve(h, &configs)?,
        None => resolve::resolve_default(&configs)?,
    }
    .clone();

    let base = format!("{}/circles/{}/members", daemon_base, cfg.circle_id);

    match action {
        MemberAction::List => {
            let resp = client.get(&base).send().await
                .context("failed to reach daemon — is enochd running?")?;
            let val: serde_json::Value = resp.json().await?;
            if json {
                println!("{}", serde_json::to_string_pretty(&val)?);
            } else {
                let members = val.as_array().map(|a| a.as_slice()).unwrap_or(&[]);
                if members.is_empty() {
                    println!("(no members)");
                } else {
                    for m in members {
                        let peer = m["peer_id"].as_str().unwrap_or("?");
                        let role = m["role"].as_str().unwrap_or("member");
                        let agent = m["agent_id"].as_str().unwrap_or("");
                        if agent.is_empty() {
                            println!("  [{role}] {peer}");
                        } else {
                            println!("  [{role}] {peer}  ({agent})");
                        }
                    }
                }
            }
        }

        MemberAction::Add { peer_id, role } => {
            let sig = sign_admin(&cfg.circle_id, format!("add:{peer_id}:{role}").as_bytes())?;
            let body = serde_json::json!({
                "peer_id": peer_id,
                "role": role,
                "admin_signature": sig,
            });
            let resp = client.post(&base).json(&body).send().await
                .context("failed to reach daemon")?;
            let val: serde_json::Value = resp.json().await?;
            if json {
                println!("{}", serde_json::to_string_pretty(&val)?);
            } else {
                println!("✦ {}", val["status"].as_str().unwrap_or("done"));
            }
        }

        MemberAction::Remove { peer_id } => {
            let sig = sign_admin(&cfg.circle_id, format!("remove:{peer_id}").as_bytes())?;
            let body = serde_json::json!({
                "peer_id": peer_id,
                "admin_signature": sig,
            });
            let url = format!("{base}/remove");
            let resp = client.post(&url).json(&body).send().await
                .context("failed to reach daemon")?;
            let val: serde_json::Value = resp.json().await?;
            if json {
                println!("{}", serde_json::to_string_pretty(&val)?);
            } else {
                println!("✦ {}", val["status"].as_str().unwrap_or("done"));
            }
        }

        MemberAction::Promote { peer_id } => {
            let sig = sign_admin(&cfg.circle_id, format!("add:{peer_id}:admin").as_bytes())?;
            let body = serde_json::json!({
                "peer_id": peer_id,
                "admin_signature": sig,
            });
            let url = format!("{base}/promote");
            let resp = client.post(&url).json(&body).send().await
                .context("failed to reach daemon")?;
            let val: serde_json::Value = resp.json().await?;
            if json {
                println!("{}", serde_json::to_string_pretty(&val)?);
            } else {
                println!("✦ {}", val["status"].as_str().unwrap_or("done"));
            }
        }
    }

    Ok(())
}

fn sign_admin(circle_id: &str, msg: &[u8]) -> Result<String> {
    let key_path = circle_dir(circle_id)?.join("admin.key");
    let hex = std::fs::read_to_string(&key_path)
        .with_context(|| format!("admin.key not found for this circle — only the circle creator can perform member operations\n  Expected: {}", key_path.display()))?;
    let keypair = keypair_from_hex(hex.trim())
        .context("failed to load admin.key")?;
    let sig = keypair.sign(msg)
        .map_err(|e| anyhow::anyhow!("signing failed: {e}"))?;
    Ok(hex::encode(sig))
}
