use anyhow::Result;
use chrono::{DateTime, Utc};
use serde_json::Value;

pub async fn run(client: &reqwest::Client, base: &str, json: bool) -> Result<()> {
    let resp = client.get(format!("{base}/who")).send().await?;
    let val: Value = resp.json().await?;
    if json {
        println!("{}", serde_json::to_string_pretty(&val)?);
        return Ok(());
    }

    let agents = val.as_array().map(|a| a.as_slice()).unwrap_or(&[]);
    if agents.is_empty() {
        println!("(no agents present)");
        return Ok(());
    }

    let now = Utc::now();
    for a in agents {
        let id = a["agent_id"].as_str().unwrap_or("?");
        let file = a["current_file"].as_str().unwrap_or("");
        let last_seen_str = a["last_seen"].as_str().unwrap_or("");
        let age = last_seen_str
            .parse::<DateTime<Utc>>()
            .ok()
            .map(|t| (now - t).num_seconds());

        let status = match age {
            Some(s) if s <= 90 => "online",
            Some(_) => "stale",
            None => a["status"].as_str().unwrap_or("?"),
        };

        let age_label = match age {
            Some(s) if s < 60 => format!("{s}s ago"),
            Some(s) if s < 3600 => format!("{}m ago", s / 60),
            Some(s) => format!("{}h ago", s / 3600),
            None => String::new(),
        };

        let file_part = if file.is_empty() { String::new() } else { format!("  {file}") };
        let age_part = if age_label.is_empty() { String::new() } else { format!("  {age_label}") };

        println!("  {id}  [{status}]{age_part}{file_part}");
    }
    Ok(())
}
