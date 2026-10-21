//! Local delivery status, read from the atomically persisted execution inbox.
//! This endpoint never opens the inbox for ownership or performs recovery.
use crate::{agent::inbox::Inbox, daemon::DaemonState};
use axum::{
    extract::{Path, Query, State},
    http::StatusCode,
    response::IntoResponse,
    Json,
};
use serde::Deserialize;
use serde_json::json;

#[derive(Default, Deserialize)]
pub struct QueryArgs {
    pub message_id: Option<String>,
    pub before: Option<String>,
    pub limit: Option<usize>,
}

pub async fn list(
    State(daemon): State<DaemonState>,
    Path(circle): Path<String>,
    Query(query): Query<QueryArgs>,
) -> axum::response::Response {
    let Some(state) = daemon.get(&circle) else {
        return (
            StatusCode::NOT_FOUND,
            Json(json!({"error": "circle not found"})),
        )
            .into_response();
    };
    if state
        .execution_inbox
        .read()
        .unwrap()
        .as_ref()
        .is_some_and(|owner| owner.upgrade().is_none_or(|inbox| !inbox.is_healthy()))
    {
        return (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(json!({"error": "execution storage unavailable; owner recovery in progress"})),
        )
            .into_response();
    }
    let snapshot = match Inbox::read(&state.circle_dir) {
        Ok(snapshot) => snapshot,
        Err(error) => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({"error": format!("execution inbox unavailable: {error}")})),
            )
                .into_response()
        }
    };
    let Some(snapshot) = snapshot else {
        return Json(json!({"peer_id": state.peer_id, "activated_at": null, "runs": [], "next_cursor": null})).into_response();
    };
    let end = match query.before.as_deref() {
        Some(id) => match snapshot.entries.iter().position(|e| e.run_id == id) {
            Some(index) => index,
            None => {
                return (
                    StatusCode::BAD_REQUEST,
                    Json(json!({"error": "unknown run cursor"})),
                )
                    .into_response()
            }
        },
        None => snapshot.entries.len(),
    };
    let limit = query.limit.unwrap_or(50).clamp(1, 200);
    let mut matching = snapshot.entries[..end].iter().rev().filter(|e| {
        query
            .message_id
            .as_ref()
            .is_none_or(|id| *id == e.request.message.id)
    });
    let page: Vec<_> = matching.by_ref().take(limit).collect();
    let next_cursor = matching
        .next()
        .and_then(|_| page.last().map(|e| e.run_id.clone()));
    let now = chrono::Utc::now().timestamp();
    // What became of each run: its proposals as they stand, and whether a pass
    // was followed by someone calling an agent. Derived now rather than stored,
    // so a decision synced from another device is already reflected. Local, like
    // usage. Not computed for the activity panel's frequent poll.
    let proposals = crate::proposal::store::ProposalStore::open(&state.workspace)
        .map(|store| store.list_proposals())
        .unwrap_or_default();
    let mut outcomes = crate::agent::outcome::derive(&page, &proposals, &state.transcript());
    let runs: Vec<_> = page
        .into_iter()
        .map(|e| {
            json!({
                "run_id": e.run_id, "message_id": e.request.message.id,
                "agent_id": e.request.agent, "status": e.status, "detail": e.detail,
                "admitted_at": e.admitted_at, "updated_at": e.updated_at,
                "relay_root": e.request.relay.as_ref().map(|r| &r.root),
                "reply_to": e.request.message.reply_to,
                "ambient": e.request.ambient,
                "activity": state.live_runs.get(&e.run_id).map(|run| run.snapshot(now)),
                "usage": e.usage,
                "outcome": outcomes.remove(&e.run_id),
            })
        })
        .collect();
    Json(
        json!({"peer_id": state.peer_id, "activated_at": snapshot.activated_at,
        "runs": runs, "next_cursor": next_cursor}),
    )
    .into_response()
}

#[derive(Deserialize)]
pub struct Action {
    pub action: String,
}

pub async fn update(
    State(daemon): State<DaemonState>,
    Path((circle, run)): Path<(String, String)>,
    Json(action): Json<Action>,
) -> axum::response::Response {
    let Some(state) = daemon.get(&circle) else {
        return StatusCode::NOT_FOUND.into_response();
    };
    let Some(inbox) = state
        .execution_inbox
        .read()
        .unwrap()
        .as_ref()
        .and_then(std::sync::Weak::upgrade)
    else {
        return (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(json!({"error": "execution owner unavailable"})),
        )
            .into_response();
    };
    let now = chrono::Utc::now().timestamp();
    let result = match action.action.as_str() {
        // A running turn is asked to stop; the run settles as cancelled once
        // the agent winds down (or is killed for not doing so).
        "cancel"
            if inbox
                .entries()
                .iter()
                .any(|e| e.run_id == run && e.status == crate::agent::inbox::Status::Running) =>
        {
            match state.live_runs.get(&run) {
                Some(live) => {
                    live.request_stop(crate::agent::liveness::StopCause::Requested);
                    Ok(json!({"status": "stopping"}))
                }
                None => Err(anyhow::anyhow!(
                    "the run has not started its agent yet; try again in a moment"
                )),
            }
        }
        "cancel" => inbox
            .transition(
                &run,
                crate::agent::inbox::Status::Pending,
                crate::agent::inbox::Status::Cancelled,
                Some("cancelled by user".into()),
                now,
            )
            .and_then(|changed| {
                anyhow::ensure!(changed, "run is no longer pending");
                Ok(json!({"status": "cancelled"}))
            }),
        "retry" => inbox
            .retry(
                &run,
                crate::agent::config::AgentConfig::load()
                    .resolved(&circle)
                    .max_relay_turns,
                now,
            )
            .map(|entry| json!({"run_id": entry.run_id, "status": entry.status})),
        _ => {
            return (
                StatusCode::BAD_REQUEST,
                Json(json!({"error": "expected retry or cancel"})),
            )
                .into_response()
        }
    };
    match result {
        Ok(body) => {
            let _ = publish(&state, &inbox);
            Json(body).into_response()
        }
        Err(error) => (
            StatusCode::CONFLICT,
            Json(json!({"error": error.to_string()})),
        )
            .into_response(),
    }
}

/// Sanitized, device-vouched delivery receipts. Never sync tasks or commands.
pub const RECEIPTS_KEY: &str = "execution_receipts";
const TERMINAL_RECEIPTS_PER_DEVICE: usize = 100;
const RECEIPT_RETENTION_SECS: i64 = 30 * 24 * 60 * 60;
/// How far back the activity panel's spend list reaches. Two days, so "today"
/// is covered in any time zone the browser is in.
const SPEND_WINDOW_SECS: i64 = 2 * 24 * 60 * 60;

fn recent_receipts(
    entries: Vec<crate::agent::inbox::Entry>,
    now: i64,
) -> Vec<crate::agent::inbox::Entry> {
    use crate::agent::inbox::Status;
    let (mut active, mut terminal): (Vec<_>, Vec<_>) = entries
        .into_iter()
        .partition(|e| matches!(e.status, Status::Pending | Status::Running));
    terminal.retain(|e| e.updated_at >= now - RECEIPT_RETENTION_SECS);
    terminal.sort_by(|a, b| {
        b.updated_at
            .cmp(&a.updated_at)
            .then(b.run_id.cmp(&a.run_id))
    });
    terminal.truncate(TERMINAL_RECEIPTS_PER_DEVICE);
    active.extend(terminal);
    active
}

pub fn publish(state: &crate::state::AppState, inbox: &Inbox) -> anyhow::Result<()> {
    use yrs::{Any, Map, Transact, WriteTxn};
    let entries = recent_receipts(inbox.entries(), chrono::Utc::now().timestamp());
    let mut txn = state
        .control
        .try_transact_mut()
        .map_err(|_| anyhow::anyhow!("Circle busy"))?;
    let map = txn.get_or_insert_map(RECEIPTS_KEY);
    let retained: std::collections::HashSet<_> =
        entries.iter().map(|e| e.run_id.as_str()).collect();
    let stale: Vec<_> = map
        .iter(&txn)
        .filter_map(|(key, value)| {
            let yrs::Out::Any(Any::String(raw)) = value else {
                return None;
            };
            let receipt: serde_json::Value = serde_json::from_str(&raw).ok()?;
            (receipt["peer_id"].as_str() == Some(state.peer_id.as_str()) && !retained.contains(key))
                .then(|| key.to_string())
        })
        .collect();
    for key in stale {
        map.remove(&mut txn, &key);
    }
    // Publish only retained records, otherwise the next reconcile resurrects
    // every terminal receipt just pruned from the replicated map.
    for e in entries {
        let summary = json!({"run_id": e.run_id, "message_id": e.request.message.id,
            "agent_id": e.request.agent, "peer_id": state.peer_id, "status": e.status,
            "detail": e.detail, "ambient": e.request.ambient, "updated_at": e.updated_at,
            "admitted_at": e.admitted_at });
        let value = serde_json::to_string(&summary)?;
        let unchanged = map
            .get(&txn, &e.run_id)
            .is_some_and(|old| matches!(old, yrs::Out::Any(Any::String(s)) if s.as_ref() == value));
        if !unchanged {
            map.insert(&mut txn, e.run_id, value);
        }
    }
    Ok(())
}

pub async fn deliveries(
    State(daemon): State<DaemonState>,
    Path(circle): Path<String>,
) -> axum::response::Response {
    use yrs::{Map, ReadTxn, Transact};
    let Some(state) = daemon.get(&circle) else {
        return StatusCode::NOT_FOUND.into_response();
    };
    if state
        .execution_inbox
        .read()
        .unwrap()
        .as_ref()
        .is_some_and(|owner| owner.upgrade().is_none_or(|inbox| !inbox.is_healthy()))
    {
        return (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(json!({"error": "execution storage unavailable; owner recovery in progress"})),
        )
            .into_response();
    }
    let Ok(txn) = state.control.try_transact() else {
        return super::circle_busy();
    };
    let mut runs: Vec<serde_json::Value> = txn
        .get_map(RECEIPTS_KEY)
        .map(|map| {
            map.iter(&txn)
                .filter_map(|(_, v)| {
                    if let yrs::Out::Any(yrs::Any::String(raw)) = v {
                        serde_json::from_str(&raw).ok()
                    } else {
                        None
                    }
                })
                .collect()
        })
        .unwrap_or_default();
    runs.sort_by(|a, b| {
        b["admitted_at"]
            .as_i64()
            .cmp(&a["admitted_at"].as_i64())
            .then(b["run_id"].as_str().cmp(&a["run_id"].as_str()))
    });
    runs.truncate(100);
    // `runs` is replicated and device-vouched; the two fields below are neither.
    // They are this device's own account of what it declined to do and why, and
    // they are here rather than on their own endpoint only so the activity
    // panel — which already polls this one — can show a run and the reason a
    // run is missing side by side. No peer sees them.
    // What this device's running turns are doing, by run id. Local, like the
    // two fields below it: it changes constantly and no peer needs it.
    let now = chrono::Utc::now().timestamp();
    let activity: serde_json::Map<String, serde_json::Value> = runs
        .iter()
        .filter_map(|run| run["run_id"].as_str())
        .filter_map(|id| Some((id.to_string(), state.live_runs.get(id)?.snapshot(now))))
        .collect();
    // What this device's own runs used, as their agents reported it. Local for
    // the same reason: a peer has no business knowing what this machine spent.
    // `usage` covers the runs listed above; `spend` is every run of the last
    // two days, so a day's total is not cut short by the 100 receipts listed.
    // The inbox keeps its terminal entries for good, so neither is its whole
    // history.
    let listed: std::collections::HashSet<&str> = runs
        .iter()
        .filter_map(|run| run["run_id"].as_str())
        .collect();
    let entries = state
        .execution_inbox
        .read()
        .unwrap()
        .as_ref()
        .and_then(std::sync::Weak::upgrade)
        .map(|inbox| inbox.entries())
        .unwrap_or_default();
    let mut usage = serde_json::Map::new();
    let mut spend = Vec::new();
    for e in entries {
        let Some(used) = e.usage else { continue };
        if e.updated_at >= now - SPEND_WINDOW_SECS {
            spend.push(json!({"run_id": e.run_id, "ambient": e.request.ambient,
                "updated_at": e.updated_at, "usage": used}));
        }
        if listed.contains(e.run_id.as_str()) {
            usage.insert(e.run_id, json!(used));
        }
    }
    Json(json!({
        "peer_id": state.peer_id,
        "runs": runs,
        "activity": activity,
        "usage": usage,
        "spend": spend,
        "readiness": readiness(&state),
        "skips": state.admission_log.recent(30),
    }))
    .into_response()
}

/// What this device would do with an unaddressed message right now.
///
/// A standing configuration fact rather than an event, which is why it is
/// reported as state instead of being written into the skip log once per
/// message. Empty `ambient` is the single most common reason nothing happens,
/// and it is invisible from the transcript.
fn readiness(state: &crate::state::AppState) -> serde_json::Value {
    let cfg = crate::agent::config::AgentConfig::load();
    let settings = cfg.resolved(&state.circle_id);
    let missing: Vec<&String> = settings
        .ambient
        .iter()
        .filter(|name| {
            !cfg.agents
                .keys()
                .any(|configured| configured.eq_ignore_ascii_case(name))
        })
        .collect();
    json!({
        "reaction": settings.reaction,
        "ambient": settings.ambient,
        "ambient_responders": settings.ambient_responders,
        "engagement_window_secs": settings.engagement_window_secs,
        "ambient_backlog_tail": settings.ambient_backlog_tail,
        "ambient_max_attempts": settings.ambient_max_attempts,
        // Named in `ambient` but absent from `[agents.*]` under any casing, so
        // there is no agent for the drain to offer anything to.
        "ambient_unconfigured": missing,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::inbox::{tests::request, Status};

    fn circle() -> (
        DaemonState,
        crate::state::AppState,
        std::sync::Arc<Inbox>,
        tempfile::TempDir,
    ) {
        let dir = tempfile::tempdir().unwrap();
        let state = crate::state::AppState::new(
            "c1".into(),
            "circle".into(),
            dir.path().to_path_buf(),
            dir.path().to_path_buf(),
            String::new(),
            "label".into(),
            1,
            "peer".into(),
            crate::config::JoinPolicy::Manual,
            "owner".into(),
            crate::mls::new_mls_state(crate::mls::MlsIdentity::generate("peer").unwrap(), None),
        );
        let inbox = std::sync::Arc::new(Inbox::open(dir.path(), 100).unwrap());
        *state.execution_inbox.write().unwrap() = Some(std::sync::Arc::downgrade(&inbox));
        let daemon = DaemonState::new();
        daemon.insert_circle(
            "c1".into(),
            state.clone(),
            tokio_util::sync::CancellationToken::new(),
        );
        (daemon, state, inbox, dir)
    }

    async fn cancel(daemon: &DaemonState, run: &str) -> (StatusCode, serde_json::Value) {
        let resp = update(
            State(daemon.clone()),
            Path(("c1".into(), run.into())),
            Json(Action {
                action: "cancel".into(),
            }),
        )
        .await;
        let status = resp.status();
        let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        (status, serde_json::from_slice(&body).unwrap())
    }

    /// A listed run says what became of it: a pass that someone followed by
    /// calling an agent, and the status its proposals have now.
    #[tokio::test]
    async fn listed_runs_carry_what_became_of_them() {
        use crate::proposal::model::{Proposal, ProposalStatus};
        use yrs::{Any, Array, Transact, WriteTxn};
        let (daemon, state, inbox, _dir) = circle();
        let mut asked = request("m", "claude");
        asked.ambient = true;
        asked.message.mentions.clear();
        let message = asked.message.clone();
        inbox.admit(asked, 20, 100).unwrap();
        inbox.admit(request("m2", "codex"), 20, 100).unwrap();
        let entries = inbox.entries();
        let (passed, wrote) = (entries[0].run_id.clone(), entries[1].run_id.clone());
        for (run, detail) in [
            (&passed, Some(crate::agent::reaction::PASSED_DETAIL.into())),
            (&wrote, None),
        ] {
            inbox
                .transition(run, Status::Pending, Status::Running, None, 101)
                .unwrap();
            inbox
                .transition(run, Status::Running, Status::Completed, detail, 102)
                .unwrap();
        }
        let mut call = message.clone();
        call.id = "call".into();
        call.ts = message.ts + 60;
        call.mentions = vec!["codex".into()];
        {
            let mut txn = state.control.transact_mut();
            let chat = txn.get_or_insert_array(crate::control::CHAT_KEY);
            for m in [&message, &call] {
                chat.push_back(
                    &mut txn,
                    Any::String(serde_json::to_string(m).unwrap().into()),
                );
            }
        }
        let store = crate::proposal::store::ProposalStore::open(&state.workspace).unwrap();
        let mut proposal =
            Proposal::ambient("c1".into(), "s0".into(), "s1".into(), vec!["a.md".into()]);
        proposal.session_id = Some(wrote.clone());
        proposal.status = ProposalStatus::Rejected;
        store.save_proposal(&proposal).unwrap();

        let resp = list(
            State(daemon.clone()),
            Path("c1".into()),
            Query(QueryArgs {
                before: None,
                limit: None,
                message_id: None,
            }),
        )
        .await;
        let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        let page: serde_json::Value = serde_json::from_slice(&body).unwrap();
        let run = |id: &str| {
            page["runs"]
                .as_array()
                .unwrap()
                .iter()
                .find(|r| r["run_id"] == id)
                .unwrap()
                .clone()
        };
        assert_eq!(
            run(&passed)["outcome"]["called_after_pass"]["message_id"],
            "call"
        );
        assert_eq!(
            run(&passed)["outcome"]["called_after_pass"]["after_secs"],
            60
        );
        assert_eq!(run(&wrote)["outcome"]["proposals"][0]["status"], "rejected");
    }

    /// What a run cost is this device's business: shown in its own panel,
    /// never in the receipt every member replicates.
    #[tokio::test]
    async fn usage_is_reported_locally_and_never_published() {
        let (daemon, state, inbox, _dir) = circle();
        inbox.admit(request("m", "claude"), 20, 100).unwrap();
        let entry = inbox.entries().remove(0);
        inbox
            .transition(
                &entry.run_id,
                Status::Pending,
                Status::Running,
                None,
                chrono::Utc::now().timestamp(),
            )
            .unwrap();
        let spent = crate::agent::usage::TurnUsage {
            context_used: 41_000,
            context_size: 200_000,
            tokens: None,
            cost: Some(crate::agent::usage::Cost {
                amount: 0.03,
                currency: "USD".into(),
            }),
        };
        inbox.record_usage(&entry.run_id, spent).unwrap();
        publish(&state, &inbox).unwrap();

        let resp = deliveries(State(daemon.clone()), Path("c1".into())).await;
        let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        let page: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(page["usage"][&entry.run_id]["cost"]["amount"], 0.03);
        assert_eq!(page["spend"][0]["run_id"], entry.run_id.as_str());
        assert_eq!(page["spend"][0]["usage"]["cost"]["amount"], 0.03);
        let receipt = &page["runs"][0];
        assert_eq!(receipt["run_id"], entry.run_id.as_str());
        assert!(receipt.get("usage").is_none(), "{receipt}");
        assert!(!receipt.to_string().contains("41000"), "{receipt}");
    }

    #[tokio::test]
    async fn cancelling_a_running_turn_asks_it_to_stop() {
        let (daemon, state, inbox, _dir) = circle();
        inbox.admit(request("m", "claude"), 20, 100).unwrap();
        let entry = inbox.entries().remove(0);
        inbox
            .transition(&entry.run_id, Status::Pending, Status::Running, None, 101)
            .unwrap();

        // Running, but its agent not started yet: nothing to stop so far.
        let (status, body) = cancel(&daemon, &entry.run_id).await;
        assert_eq!(status, StatusCode::CONFLICT, "{body}");

        let live = state
            .live_runs
            .register(&entry.run_id, "claude", false, 101);
        let (status, body) = cancel(&daemon, &entry.run_id).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body["status"], "stopping");
        assert_eq!(
            live.run().stop_cause(),
            Some(crate::agent::liveness::StopCause::Requested)
        );
    }
}
