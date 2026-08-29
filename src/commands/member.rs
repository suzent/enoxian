use anyhow::{bail, Context, Result};

use crate::{
    cli::MemberAction,
    config::{self, circle_dir},
    crypto::keypair_from_hex,
    resolve,
};

/// Send a member-management request and return its JSON body, failing loudly.
///
/// Every call site used to do `resp.json()` and print `val["status"]`, which on
/// an error response has no `status` field — so `unwrap_or("done")` reported a
/// rejected request as `✦ done`. A broken signature format went unnoticed that
/// way for as long as it had been broken.
async fn post_member_action(
    client: &reqwest::Client,
    url: &str,
    body: &serde_json::Value,
) -> Result<serde_json::Value> {
    let resp = client
        .post(url)
        .json(body)
        .send()
        .await
        .context("failed to reach daemon")?;
    let status = resp.status();
    let val: serde_json::Value = resp.json().await.unwrap_or_else(|_| serde_json::json!({}));
    if !status.is_success() {
        let detail = val["error"]
            .as_str()
            .map(str::to_string)
            .unwrap_or_else(|| status.to_string());
        bail!("{detail}");
    }
    Ok(val)
}

/// Fetch member-management JSON, failing loudly.
///
/// A rejected GET decoded to an empty array and rendered as "no members" or an
/// empty owner — wrong rather than absent, and indistinguishable from a Circle
/// that genuinely has none.
async fn get_member_json(client: &reqwest::Client, url: &str) -> Result<serde_json::Value> {
    let resp = client
        .get(url)
        .send()
        .await
        .context("failed to reach daemon — run `enox start`")?;
    let status = resp.status();
    let val: serde_json::Value = resp.json().await.unwrap_or_else(|_| serde_json::json!({}));
    if !status.is_success() {
        let detail = val["error"]
            .as_str()
            .map(str::to_string)
            .unwrap_or_else(|| status.to_string());
        bail!("{detail}");
    }
    Ok(val)
}

/// Signature left to the daemon.
///
/// Some admin messages cover values only the daemon can determine — the owner
/// it derives when the request omits one, or the owner it reads back from the
/// member list on promote. A client cannot construct those, so it must not try:
/// the daemon signs with the same `admin.key` after computing them. Sending an
/// empty signature is how `resolve_admin_sig` is asked to do that.
const DAEMON_SIGNS: &str = "";

/// Fetch the member list and resolve a peer ID hint (prefix or suffix) to a full peer ID.
async fn resolve_peer_id(client: &reqwest::Client, base: &str, hint: &str) -> Result<String> {
    // Fast path: looks like a full peer ID already
    if hint.len() > 20 {
        return Ok(hint.to_string());
    }
    let members = get_member_json(client, base).await?;
    let members = members.as_array().context("unexpected response")?;

    let matches: Vec<String> = members
        .iter()
        .filter_map(|m| m["peer_id"].as_str())
        .filter(|pid| pid.starts_with(hint) || pid.ends_with(hint))
        .map(|s| s.to_string())
        .collect();

    match matches.len() {
        0 => bail!("no member matching '{hint}'"),
        1 => Ok(matches.into_iter().next().unwrap()),
        _ => bail!(
            "ambiguous prefix '{hint}' matches {} members: {}",
            matches.len(),
            matches.join(", ")
        ),
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
            let val = get_member_json(client, &base).await?;
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

        MemberAction::Add {
            peer_id,
            role,
            owner,
            agent_id,
        } => {
            // The signed message covers the owner the daemon derives when the
            // request omits one, so the daemon signs it. Signing here produced
            // `add:{peer}:{role}` against a daemon verifying
            // `add:{peer}:{role}:owner:{owner}` — every CLI add was rejected.
            let body = serde_json::json!({
                "peer_id": peer_id,
                "role": role,
                "owner": owner,
                "agent_id": agent_id,
                "admin_signature": DAEMON_SIGNS,
            });
            let val = post_member_action(client, &base, &body).await?;
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
            let val = post_member_action(client, &url, &body).await?;
            if json {
                println!("{}", serde_json::to_string_pretty(&val)?);
            } else {
                println!("✦ {}", val["status"].as_str().unwrap_or("done"));
            }
        }

        MemberAction::Promote { peer_id } => {
            let peer_id = resolve_peer_id(client, &base, &peer_id).await?;
            // The signed message covers the owner read back from the member
            // list, which only the daemon has. Signing here used an empty owner
            // and was rejected whenever the member had one.
            let body = serde_json::json!({
                "peer_id": peer_id,
                "admin_signature": DAEMON_SIGNS,
            });
            let url = format!("{base}/promote");
            let val = post_member_action(client, &url, &body).await?;
            if json {
                println!("{}", serde_json::to_string_pretty(&val)?);
            } else {
                println!("✦ {}", val["status"].as_str().unwrap_or("done"));
            }
        }

        MemberAction::Pending => {
            let url = format!("{base}/pending");
            let val = get_member_json(client, &url).await?;
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

        MemberAction::Approve {
            peer_id,
            role,
            owner,
        } => {
            let effective_owner = if let Some(o) = owner {
                o
            } else {
                let url = format!("{base}/pending");
                let val = get_member_json(client, &url).await?;
                val.as_array()
                    .and_then(|arr| arr.iter().find(|m| m["peer_id"].as_str() == Some(&peer_id)))
                    .and_then(|m| m["owner"].as_str())
                    .unwrap_or("")
                    .to_string()
            };
            let sig = sign_admin(
                &cfg.circle_id,
                format!("add:{peer_id}:{role}:owner:{effective_owner}").as_bytes(),
            )?;
            let body = serde_json::json!({
                "peer_id": peer_id,
                "role": role,
                "owner": effective_owner,
                "admin_signature": sig,
            });
            let url = format!("{base}/approve");
            let val = post_member_action(client, &url, &body).await?;
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
            let val = post_member_action(client, &url, &body).await?;
            if json {
                println!("{}", serde_json::to_string_pretty(&val)?);
            } else {
                println!("✦ {}", val["status"].as_str().unwrap_or("done"));
            }
        }

        MemberAction::RemoveByOwner { owner } => {
            let val = get_member_json(client, &base).await?;
            let peer_ids: Vec<String> = val
                .as_array()
                .map(|arr| {
                    arr.iter()
                        .filter(|m| m["owner"].as_str() == Some(&owner))
                        .filter_map(|m| m["peer_id"].as_str().map(|s| s.to_string()))
                        .collect()
                })
                .unwrap_or_default();
            for pid in peer_ids {
                let sig = sign_admin(&cfg.circle_id, format!("remove:{pid}").as_bytes())?;
                let body = serde_json::json!({
                    "peer_id": pid,
                    "admin_signature": sig,
                });
                let url = format!("{base}/remove");
                let val = post_member_action(client, &url, &body).await?;
                if json {
                    println!("{}", serde_json::to_string_pretty(&val)?);
                } else {
                    println!(
                        "✦ removed {pid}: {}",
                        val["status"].as_str().unwrap_or("done")
                    );
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
    let keypair = keypair_from_hex(hex.trim()).context("failed to load admin.key")?;
    let sig = keypair
        .sign(msg)
        .map_err(|e| anyhow::anyhow!("signing failed: {e}"))?;
    Ok(hex::encode(sig))
}
