//! `enox watch`: this device's live Circle events.
//!
//! In JSON mode each event becomes one line of a run log: the event as the
//! daemon raised it, under `payload`, with the time it was seen, the device
//! and peer it was seen on, and the agent it concerns lifted out to the top.
//! Every device writes its own log. They are never synced through the Circle
//! (agents' token usage stays on the device that spent it); a harness that
//! wants one log for a run merges the files by `ts`.

use crate::cli::WatchArgs;
use anyhow::{Context, Result};
use chrono::{DateTime, SecondsFormat, Utc};
use serde_json::{json, Map, Value};
use std::io::Write;

/// What is stamped on every line of a run log.
#[derive(Debug, Clone, Default)]
pub struct Stamp {
    pub device: String,
    pub peer_id: String,
    pub circle_id: String,
    pub run_id: Option<String>,
}

pub async fn run(client: &reqwest::Client, base: &str, args: WatchArgs, json: bool) -> Result<()> {
    let stamp = if json || args.log.is_some() {
        Some(stamp(client, base, &args).await?)
    } else {
        None
    };
    let mut log = match &args.log {
        Some(path) => Some(
            std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(path)
                .with_context(|| format!("opening {}", path.display()))?,
        ),
        None => None,
    };

    // Kept off stdout so a `--json` stream is nothing but events.
    eprintln!("◆ Watching circle events (Ctrl+C to stop)...");
    let mut resp = client
        .get(format!("{base}/events"))
        .header("Accept", "text/event-stream")
        .send()
        .await?
        .error_for_status()?;

    // An SSE line, or a character in it, can be split across chunks; only
    // whole lines are decoded.
    let mut pending: Vec<u8> = Vec::new();
    while let Some(chunk) = resp.chunk().await? {
        pending.extend_from_slice(&chunk);
        while let Some(end) = pending.iter().position(|&b| b == b'\n') {
            let bytes: Vec<u8> = pending.drain(..=end).collect();
            let line = String::from_utf8_lossy(&bytes);
            let Some(data) = line.trim_end().strip_prefix("data: ") else {
                continue;
            };
            let Ok(event) = serde_json::from_str::<Value>(data) else {
                continue;
            };
            let line = stamp
                .as_ref()
                .map(|stamp| envelope(event.clone(), stamp, Utc::now()).to_string());
            if let (Some(file), Some(line)) = (log.as_mut(), &line) {
                writeln!(file, "{line}")?;
                file.flush()?;
            }
            match (json, &line) {
                (true, Some(line)) => println!("{line}"),
                _ => {
                    let kind = event["type"].as_str().unwrap_or("unknown");
                    println!("  [{kind}] {event}");
                }
            }
        }
    }
    Ok(())
}

/// Who this device is, from the daemon, with the caller's overrides.
async fn stamp(client: &reqwest::Client, base: &str, args: &WatchArgs) -> Result<Stamp> {
    let status: Value = client
        .get(format!("{base}/status"))
        .send()
        .await?
        .error_for_status()?
        .json()
        .await
        .context("reading circle status")?;
    let device = args
        .device
        .clone()
        .or_else(|| status["device_label"].as_str().map(str::to_string))
        .unwrap_or_default();
    Ok(Stamp {
        device,
        peer_id: status["p2p"]["peer_id"].as_str().unwrap_or_default().into(),
        circle_id: status["circle_id"].as_str().unwrap_or_default().into(),
        run_id: args.run_id.clone(),
    })
}

/// One run-log line for `event`, a `CircleEvent` as the daemon serialized it.
///
/// `kind` is the event's `type`; `payload` is the rest of it. `agent` is the
/// agent the event names, when it names one. A `turn_end` also carries its
/// usage at the top, as `usage`, so a reader can sum spend without knowing
/// each kind's payload.
pub fn envelope(event: Value, stamp: &Stamp, ts: DateTime<Utc>) -> Value {
    let mut payload = match event {
        Value::Object(map) => map,
        other => {
            let mut map = Map::new();
            map.insert("value".into(), other);
            map
        }
    };
    let kind = payload
        .remove("type")
        .and_then(|t| t.as_str().map(str::to_string))
        .unwrap_or_else(|| "unknown".into());
    let agent = payload
        .get("agent_id")
        .or_else(|| payload.get("message").and_then(|m| m.get("agent_id")))
        .and_then(Value::as_str)
        .map(str::to_string);

    let mut line = json!({
        "ts": ts.to_rfc3339_opts(SecondsFormat::Millis, true),
        "device": stamp.device,
        "peer_id": stamp.peer_id,
        "circle_id": stamp.circle_id,
        "kind": kind,
    });
    let fields = line.as_object_mut().expect("object literal");
    if let Some(run_id) = &stamp.run_id {
        fields.insert("run_id".into(), json!(run_id));
    }
    if let Some(agent) = agent {
        fields.insert("agent".into(), json!(agent));
    }
    if kind == "turn_end" {
        if let Some(usage) = payload.get("usage").and_then(flat_usage) {
            fields.insert("usage".into(), usage);
        }
    }
    fields.insert("payload".into(), Value::Object(payload));
    line
}

/// A turn's `TurnUsage` as flat numbers: tokens by kind, and the cost when
/// the agent reported one. Missing figures are left out, never zeroed.
fn flat_usage(usage: &Value) -> Option<Value> {
    let mut flat = Map::new();
    if let Some(tokens) = usage.get("tokens").and_then(Value::as_object) {
        for key in ["input", "output", "cached_read", "cached_write", "total"] {
            if let Some(n) = tokens.get(key) {
                flat.insert(key.into(), n.clone());
            }
        }
    }
    if let Some(cost) = usage.get("cost").filter(|c| c.is_object()) {
        flat.insert("cost".into(), cost["amount"].clone());
        flat.insert("currency".into(), cost["currency"].clone());
    }
    for key in ["context_used", "context_size"] {
        if let Some(n) = usage.get(key) {
            flat.insert(key.into(), n.clone());
        }
    }
    (!flat.is_empty()).then_some(Value::Object(flat))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn stamp() -> Stamp {
        Stamp {
            device: "B".into(),
            peer_id: "12D3KooWpeer".into(),
            circle_id: "c1".into(),
            run_id: Some("pilot/qwen/M2C2/syn-x-B/0".into()),
        }
    }

    fn at() -> DateTime<Utc> {
        "2026-10-27T18:00:00.123456Z".parse().unwrap()
    }

    #[test]
    fn every_line_carries_time_device_and_peer() {
        let event = json!({ "type": "task_claimed", "task_id": "t1", "agent_id": "codex" });
        let line = envelope(event, &stamp(), at());
        assert_eq!(line["ts"], "2026-10-27T18:00:00.123Z");
        assert_eq!(line["device"], "B");
        assert_eq!(line["peer_id"], "12D3KooWpeer");
        assert_eq!(line["run_id"], "pilot/qwen/M2C2/syn-x-B/0");
        assert_eq!(line["kind"], "task_claimed");
        assert_eq!(line["agent"], "codex");
        assert_eq!(
            line["payload"],
            json!({ "task_id": "t1", "agent_id": "codex" })
        );
        assert!(line.get("usage").is_none());
    }

    #[test]
    fn a_message_names_its_author_as_the_agent() {
        let event =
            json!({ "type": "message_posted", "message": { "id": "m1", "agent_id": "claude" } });
        let line = envelope(event, &Stamp::default(), at());
        assert_eq!(line["agent"], "claude");
        assert!(line.get("run_id").is_none());
    }

    #[test]
    fn a_turn_end_lifts_its_usage_to_the_top() {
        let event = serde_json::to_value(crate::control::CircleEvent::TurnEnd {
            agent_id: "claude".into(),
            run_id: "run-1".into(),
            stop_reason: "end_turn".into(),
            usage: Some(crate::agent::usage::TurnUsage {
                context_used: 33_477,
                context_size: 200_000,
                tokens: Some(crate::agent::usage::Tokens {
                    input: 2,
                    output: 4,
                    cached_read: 0,
                    cached_write: 31_518,
                    total: 31_524,
                }),
                cost: Some(crate::agent::usage::Cost {
                    amount: 0.315,
                    currency: "USD".into(),
                }),
            }),
        })
        .unwrap();
        let line = envelope(event, &stamp(), at());
        assert_eq!(line["kind"], "turn_end");
        assert_eq!(line["agent"], "claude");
        assert_eq!(
            line["usage"],
            json!({
                "input": 2, "output": 4, "cached_read": 0, "cached_write": 31_518,
                "total": 31_524, "cost": 0.315, "currency": "USD",
                "context_used": 33_477, "context_size": 200_000,
            })
        );
        assert_eq!(line["payload"]["run_id"], "run-1");
        assert_eq!(line["payload"]["stop_reason"], "end_turn");
    }

    #[test]
    fn a_silent_agent_has_no_usage() {
        let event = serde_json::to_value(crate::control::CircleEvent::TurnEnd {
            agent_id: "gemini".into(),
            run_id: "run-2".into(),
            stop_reason: "end_turn".into(),
            usage: None,
        })
        .unwrap();
        let line = envelope(event, &stamp(), at());
        assert!(line.get("usage").is_none());
        assert!(line["payload"].get("usage").is_none());
    }
}
