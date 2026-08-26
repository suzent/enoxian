use anyhow::{bail, Result};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

#[derive(Deserialize, Serialize)]
pub struct RegisteredActor {
    pub token: String,
    pub registration_id: String,
    pub agent_id: String,
    pub circle_id: String,
    pub peer_id: String,
    pub issued_at: String,
    pub expires_at: String,
}

pub async fn issue(
    client: &reqwest::Client,
    base: &str,
    agent_id: &str,
) -> Result<RegisteredActor> {
    let resp = client
        .post(format!("{base}/actors/register"))
        .json(&json!({ "agent_id": agent_id }))
        .send()
        .await?;
    let status = resp.status();
    let val: Value = resp.json().await?;
    if !status.is_success() {
        bail!(
            "registration failed: {}",
            val["error"].as_str().unwrap_or("unknown error")
        );
    }
    Ok(serde_json::from_value(val)?)
}

pub async fn run(
    client: &reqwest::Client,
    base: &str,
    agent_id: String,
    json_out: bool,
) -> Result<()> {
    let registered = issue(client, base, &agent_id).await?;
    if json_out {
        println!("{}", serde_json::to_string_pretty(&registered)?);
    } else {
        println!("{}", registered.token);
        eprintln!(
            "Registered {} on device {} until {}. Pass this value with --token; treat it as a short-lived secret.",
            registered.agent_id,
            registered.peer_id,
            registered.expires_at,
        );
    }
    Ok(())
}
