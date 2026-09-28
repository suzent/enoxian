//! `enox runs` — the agent turns this device is running, and stopping one.
use anyhow::Result;
use serde_json::{json, Value};

/// Turns that are running or waiting, newest first.
async fn active(client: &reqwest::Client, base: &str) -> Result<(Vec<Value>, Value)> {
    let resp = client
        .get(format!("{base}/chat/executions"))
        .query(&[("limit", "200")])
        .send()
        .await?;
    let status = resp.status();
    let val: Value = resp.json().await?;
    if !status.is_success() {
        anyhow::bail!("{}", val["error"].as_str().unwrap_or("runs unavailable"));
    }
    let runs = val["runs"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .into_iter()
        .filter(|r| matches!(r["status"].as_str(), Some("running" | "pending")))
        .collect();
    Ok((runs, val))
}

pub async fn list(client: &reqwest::Client, base: &str, json_out: bool) -> Result<()> {
    let (runs, _) = active(client, base).await?;
    if json_out {
        println!("{}", serde_json::to_string_pretty(&runs)?);
        return Ok(());
    }
    if runs.is_empty() {
        println!("No agent turns running or waiting on this device.");
        return Ok(());
    }
    let now = chrono::Utc::now().timestamp();
    for run in &runs {
        let kind = if run["ambient"] == true {
            "read the room"
        } else {
            "addressed"
        };
        println!(
            "{}  {:<8} {} ({kind})",
            short(&run["run_id"]),
            run["status"].as_str().unwrap_or(""),
            run["agent_id"].as_str().unwrap_or("?"),
        );
        if let Some(line) = activity_line(&run["activity"], now) {
            println!("          {line}");
        }
    }
    println!("\nStop one with: enox runs stop <id>");
    Ok(())
}

pub async fn stop(client: &reqwest::Client, base: &str, run: String, json_out: bool) -> Result<()> {
    let (runs, _) = active(client, base).await?;
    let matches: Vec<&str> = runs
        .iter()
        .filter_map(|r| r["run_id"].as_str())
        .filter(|id| id.starts_with(run.as_str()))
        .collect();
    let run_id = match matches.as_slice() {
        [one] => one.to_string(),
        [] => anyhow::bail!("no running or waiting turn starts with `{run}` (see `enox runs`)"),
        _ => anyhow::bail!(
            "`{run}` matches {} turns; give more of the id",
            matches.len()
        ),
    };
    let resp = client
        .post(format!("{base}/chat/executions/{run_id}"))
        .json(&json!({"action": "cancel"}))
        .send()
        .await?;
    let status = resp.status();
    let val: Value = resp.json().await?;
    if json_out {
        println!("{}", serde_json::to_string_pretty(&val)?);
    } else if !status.is_success() {
        anyhow::bail!(
            "{}",
            val["error"].as_str().unwrap_or("could not stop the turn")
        );
    } else if val["status"] == "stopping" {
        println!(
            "✓ asked {} to stop; it has 30s to wind down before it is ended",
            short(&Value::String(run_id))
        );
    } else {
        println!(
            "✓ cancelled {} before it started",
            short(&Value::String(run_id))
        );
    }
    Ok(())
}

/// One line on what a running turn is doing: its open tool calls, or how long
/// it has been quiet. Quiet is only reported, never acted on.
fn activity_line(activity: &Value, now: i64) -> Option<String> {
    if activity.is_null() {
        return None;
    }
    if activity["stopping"] == true {
        return Some("stopping…".into());
    }
    let running = activity["started_at"]
        .as_i64()
        .map(|t| format!("running {}", ago(now - t)))
        .unwrap_or_default();
    let tools: Vec<String> = activity["open_tools"]
        .as_array()
        .into_iter()
        .flatten()
        .map(|t| {
            let since = t["since"]
                .as_i64()
                .map(|s| ago(now - s))
                .unwrap_or_default();
            format!("{} ({since})", t["title"].as_str().unwrap_or("tool call"))
        })
        .collect();
    if !tools.is_empty() {
        return Some(format!("{running} · using {}", tools.join(", ")));
    }
    let idle = activity["idle_secs"].as_i64().unwrap_or(0);
    Some(format!("{running} · last activity {} ago", ago(idle)))
}

fn ago(secs: i64) -> String {
    match secs.max(0) {
        s if s < 60 => format!("{s}s"),
        s if s < 3600 => format!("{}m", s / 60),
        s => format!("{}h{:02}m", s / 3600, s % 3600 / 60),
    }
}

fn short(v: &Value) -> String {
    v.as_str().unwrap_or("?").chars().take(8).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_open_tool_call_is_named_rather_than_counted_as_quiet() {
        let activity = json!({
            "started_at": 1000, "idle_secs": 400, "stopping": false,
            "open_tools": [{"title": "cargo test", "since": 1100, "status": "in_progress"}],
        });
        assert_eq!(
            activity_line(&activity, 1500).unwrap(),
            "running 8m · using cargo test (6m)"
        );
    }

    #[test]
    fn a_quiet_turn_says_how_long_it_has_been_quiet() {
        let activity =
            json!({"started_at": 0, "idle_secs": 750, "open_tools": [], "stopping": false});
        assert_eq!(
            activity_line(&activity, 4000).unwrap(),
            "running 1h06m · last activity 12m ago"
        );
    }

    #[test]
    fn a_stopping_turn_says_so() {
        let activity = json!({"started_at": 0, "stopping": true});
        assert_eq!(activity_line(&activity, 10).unwrap(), "stopping…");
        assert_eq!(activity_line(&Value::Null, 10), None);
    }
}
