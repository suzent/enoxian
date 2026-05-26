use anyhow::{bail, Context, Result};

use crate::{
    cli::MemberAction,
    config::{self, circle_dir},
    crypto::keypair_from_hex,
    resolve,
};

/// Fetch the member list and resolve a peer ID hint (prefix or suffix) to a full peer ID.
async fn resolve_peer_id(client: &reqwest::Client, base: &str, hint: &str) -> Result<String> {
    // Fast path: looks like a full peer ID already
    if hint.len() > 20 {
        return Ok(hint.to_string());
    }
    let resp = client.get(base).send().await
        .context("failed to reach daemon — is enochd running?")?;
    let members: serde_json::Value = resp.json().await?;
    let members = members.as_array().context("unexpected response")?;

    let matches: Vec<String> = members.iter()
        .filter_map(|m| m["peer_id"].as_str())
        .filter(|pid| pid.starts_with(hint) || pid.ends_with(hint))
        .map(|s| s.to_string())
        .collect();

    match matches.len() {
        0 => bail!("no member matching '{hint}'"),
        1 => Ok(matches.into_iter().next().unwrap()),
        _ => bail!("ambiguous prefix '{hint}' matches {} members: {}", matches.len(), matches.join(", ")),
    }
}

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
                        let owner = m["owner"].as_str().unwrap_or("");
                        let agent = m["agent_id"].as_str().unwrap_or("");
                        let label = match (owner, agent) {
                            ("", "") => String::new(),
                            (o, "") => format!("  owner={o}"),
                            ("", a) => format!("  agent={a}"),
                            (o, a) if o == a => format!("  {o}"),
                            (o, a) => format!("  {o} / {a}"),
                        };
                        println!("  [{role}] {peer}{label}");
                    }
                }
            }
        }

        MemberAction::Add { peer_id, role, owner, agent_id } => {
            let sig = sign_admin(&cfg.circle_id, format!("add:{peer_id}:{role}").as_bytes())?;
            let body = serde_json::json!({
                "peer_id": peer_id,
                "role": role,
                "owner": owner,
                "agent_id": agent_id,
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
            let peer_id = resolve_peer_id(client, &base, &peer_id).await?;
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
            let peer_id = resolve_peer_id(client, &base, &peer_id).await?;
            let sig = sign_admin(&cfg.circle_id, format!("add:{peer_id}:admin:owner:").as_bytes())?;
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

        MemberAction::Pending => {
            let url = format!("{base}/pending");
            let resp = client.get(&url).send().await
                .context("failed to reach daemon — is enochd running?")?;
            let val: serde_json::Value = resp.json().await?;
            if json {
                println!("{}", serde_json::to_string_pretty(&val)?);
            } else {
                let members = val.as_array().map(|a| a.as_slice()).unwrap_or(&[]);
                if members.is_empty() {
                    println!("(no pending requests)");
                } else {
                    for m in members {
                        let peer = m["peer_id"].as_str().unwrap_or("?");
                        let owner = m["owner"].as_str().unwrap_or("");
                        let agent = m["agent_id"].as_str().unwrap_or("");
                        println!("  [pending] {peer}  owner={owner}  agent={agent}");
                    }
                }
            }
        }

        MemberAction::Approve { peer_id, role, owner } => {
            let effective_owner = if let Some(o) = owner {
                o
            } else {
                let url = format!("{base}/pending");
                let resp = client.get(&url).send().await
                    .context("failed to reach daemon")?;
                let val: serde_json::Value = resp.json().await?;
                val.as_array()
                    .and_then(|arr| arr.iter().find(|m| m["peer_id"].as_str() == Some(&peer_id)))
                    .and_then(|m| m["owner"].as_str())
                    .unwrap_or("")
                    .to_string()
            };
            let sig = sign_admin(&cfg.circle_id, format!("add:{peer_id}:{role}:owner:{effective_owner}").as_bytes())?;
            let body = serde_json::json!({
                "peer_id": peer_id,
                "role": role,
                "owner": effective_owner,
                "admin_signature": sig,
            });
            let url = format!("{base}/approve");
            let resp = client.post(&url).json(&body).send().await
                .context("failed to reach daemon")?;
            let val: serde_json::Value = resp.json().await?;
            if json {
                println!("{}", serde_json::to_string_pretty(&val)?);
            } else {
                println!("✦ {}", val["status"].as_str().unwrap_or("done"));
            }
        }

        MemberAction::Reject { peer_id } => {
            let sig = sign_admin(&cfg.circle_id, format!("reject:{peer_id}").as_bytes())?;
            let body = serde_json::json!({
                "peer_id": peer_id,
                "admin_signature": sig,
            });
            let url = format!("{base}/reject");
            let resp = client.post(&url).json(&body).send().await
                .context("failed to reach daemon")?;
            let val: serde_json::Value = resp.json().await?;
            if json {
                println!("{}", serde_json::to_string_pretty(&val)?);
            } else {
                println!("✦ {}", val["status"].as_str().unwrap_or("done"));
            }
        }

        MemberAction::RemoveByOwner { owner } => {
            let resp = client.get(&base).send().await
                .context("failed to reach daemon — is enochd running?")?;
            let val: serde_json::Value = resp.json().await?;
            let peer_ids: Vec<String> = val.as_array()
                .map(|arr| arr.iter()
                    .filter(|m| m["owner"].as_str() == Some(&owner))
                    .filter_map(|m| m["peer_id"].as_str().map(|s| s.to_string()))
                    .collect())
                .unwrap_or_default();
            for pid in peer_ids {
                let sig = sign_admin(&cfg.circle_id, format!("remove:{pid}").as_bytes())?;
                let body = serde_json::json!({
                    "peer_id": pid,
                    "admin_signature": sig,
                });
                let url = format!("{base}/remove");
                let resp = client.post(&url).json(&body).send().await
                    .context("failed to reach daemon")?;
                let val: serde_json::Value = resp.json().await?;
                if json {
                    println!("{}", serde_json::to_string_pretty(&val)?);
                } else {
                    println!("✦ removed {pid}: {}", val["status"].as_str().unwrap_or("done"));
                }
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
