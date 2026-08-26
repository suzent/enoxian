use crate::control::{
    arbitration::{append_lock_entry, compute_lock_state, is_locked_by_other},
    fs_lock::set_readonly,
    CircleEvent, LockAction, LockEntry, Task, TaskStatus, LOCK_LOG_KEY, TASKS_KEY,
};
use crate::daemon::DaemonState;
use crate::state::AppState;
use axum::{
    extract::{Path, State},
    http::StatusCode,
    response::IntoResponse,
    Json,
};
use serde::Deserialize;
use serde_json::json;
use yrs::{Any, Map, Out, ReadTxn, Transact, WriteTxn};

// ── File locking ──────────────────────────────────────────────────────────

#[derive(Deserialize)]
pub struct PathRequest {
    pub path: String,
    pub agent_id: Option<String>,
    pub actor_token: Option<String>,
}

pub async fn bind_path(
    State(daemon): State<DaemonState>,
    Path(circle_id): Path<String>,
    Json(req): Json<PathRequest>,
) -> impl IntoResponse {
    let state = match daemon.get(&circle_id) {
        Some(s) => s,
        None => {
            return (
                StatusCode::NOT_FOUND,
                Json(json!({"error": "circle not found"})),
            )
                .into_response()
        }
    };
    let actor = match super::actor::resolve_actor(
        &state,
        req.actor_token.as_deref(),
        req.agent_id.clone(),
        "anonymous",
    ) {
        Ok(actor) => actor,
        Err(error) => return error.into_response(),
    };
    let agent_id = actor.agent_id.clone();

    let conflict = {
        let txn = match state.control.try_transact() {
            Ok(txn) => txn,
            Err(_) => return super::circle_busy(),
        };
        txn.get_array(LOCK_LOG_KEY).and_then(|lock_log| {
            if is_locked_by_other(&lock_log, &txn, &req.path, &agent_id, &actor.peer_id) {
                let holders = compute_lock_state(&lock_log, &txn);
                Some(holders.get(&req.path).cloned().unwrap_or_default())
            } else {
                None
            }
        })
    };

    if let Some(holder) = conflict {
        return (
            StatusCode::CONFLICT,
            Json(json!({ "error": "already locked", "held_by": holder })),
        )
            .into_response();
    }

    write_bind(&state, req, actor).await
}

async fn write_bind(
    state: &AppState,
    req: PathRequest,
    actor: crate::actor_token::ActorIdentity,
) -> axum::response::Response {
    let agent_id = actor.agent_id.clone();
    {
        let entry = LockEntry {
            entry_id: uuid::Uuid::new_v4().to_string(),
            agent_id: agent_id.clone(),
            peer_id: actor.peer_id,
            path: req.path.clone(),
            action: LockAction::Acquire,
            ts: chrono::Utc::now(),
        };
        let mut txn = match state.control.try_transact_mut() {
            Ok(txn) => txn,
            Err(_) => return super::circle_busy(),
        };
        let lock_log = txn.get_or_insert_array(LOCK_LOG_KEY);
        let _ = append_lock_entry(&lock_log, &mut txn, &entry);
    }

    let full = state
        .workspace
        .join(req.path.replace('/', std::path::MAIN_SEPARATOR_STR));
    let _ = set_readonly(&full, true).await;
    let _ = state.events.send(CircleEvent::LockAcquired {
        path: req.path.clone(),
        agent_id: agent_id.clone(),
    });

    (
        StatusCode::OK,
        Json(json!({ "status": "bound", "path": req.path, "agent_id": agent_id })),
    )
        .into_response()
}

pub async fn release_path(
    State(daemon): State<DaemonState>,
    Path(circle_id): Path<String>,
    Json(req): Json<PathRequest>,
) -> impl IntoResponse {
    let state = match daemon.get(&circle_id) {
        Some(s) => s,
        None => {
            return (
                StatusCode::NOT_FOUND,
                Json(json!({"error": "circle not found"})),
            )
                .into_response()
        }
    };
    let actor = match super::actor::resolve_actor(
        &state,
        req.actor_token.as_deref(),
        req.agent_id,
        "anonymous",
    ) {
        Ok(actor) => actor,
        Err(error) => return error.into_response(),
    };
    let agent_id = actor.agent_id.clone();

    {
        let entry = LockEntry {
            entry_id: uuid::Uuid::new_v4().to_string(),
            agent_id: agent_id.clone(),
            peer_id: actor.peer_id,
            path: req.path.clone(),
            action: LockAction::Release,
            ts: chrono::Utc::now(),
        };
        let mut txn = match state.control.try_transact_mut() {
            Ok(txn) => txn,
            Err(_) => return super::circle_busy(),
        };
        let lock_log = txn.get_or_insert_array(LOCK_LOG_KEY);
        let _ = append_lock_entry(&lock_log, &mut txn, &entry);
    }

    let full = state
        .workspace
        .join(req.path.replace('/', std::path::MAIN_SEPARATOR_STR));
    let _ = set_readonly(&full, false).await;
    let _ = state.events.send(CircleEvent::LockReleased {
        path: req.path.clone(),
        agent_id: agent_id.clone(),
    });

    (
        StatusCode::OK,
        Json(json!({ "status": "released", "path": req.path })),
    )
        .into_response()
}

// ── Task claiming / completion ─────────────────────────────────────────────

#[derive(Deserialize)]
pub struct TaskRequest {
    pub task_id: String,
    pub agent_id: Option<String>,
    pub actor_token: Option<String>,
}

pub async fn claim_task(
    State(daemon): State<DaemonState>,
    Path(circle_id): Path<String>,
    Json(req): Json<TaskRequest>,
) -> impl IntoResponse {
    let state = match daemon.get(&circle_id) {
        Some(s) => s,
        None => {
            return (
                StatusCode::NOT_FOUND,
                Json(json!({"error": "circle not found"})),
            )
                .into_response()
        }
    };
    let actor = match super::actor::resolve_actor(
        &state,
        req.actor_token.as_deref(),
        req.agent_id,
        "anonymous",
    ) {
        Ok(actor) => actor,
        Err(error) => return error.into_response(),
    };
    let agent_id = actor.agent_id.clone();
    match update_task_status(&state, &req.task_id, TaskStatus::Claimed, &actor).await {
        Ok(_) => {
            let _ = state.events.send(CircleEvent::TaskClaimed {
                task_id: req.task_id.clone(),
                agent_id,
            });
            (
                StatusCode::OK,
                Json(json!({ "status": "claimed", "task_id": req.task_id })),
            )
                .into_response()
        }
        Err(e) => (
            StatusCode::NOT_FOUND,
            Json(json!({ "error": e.to_string() })),
        )
            .into_response(),
    }
}

pub async fn done_task(
    State(daemon): State<DaemonState>,
    Path(circle_id): Path<String>,
    Json(req): Json<TaskRequest>,
) -> impl IntoResponse {
    let state = match daemon.get(&circle_id) {
        Some(s) => s,
        None => {
            return (
                StatusCode::NOT_FOUND,
                Json(json!({"error": "circle not found"})),
            )
                .into_response()
        }
    };
    let actor = match super::actor::resolve_actor(
        &state,
        req.actor_token.as_deref(),
        req.agent_id,
        "anonymous",
    ) {
        Ok(actor) => actor,
        Err(error) => return error.into_response(),
    };
    match update_task_status(&state, &req.task_id, TaskStatus::Done, &actor).await {
        Ok(_) => {
            let _ = state.events.send(CircleEvent::TaskDone {
                task_id: req.task_id.clone(),
            });
            (
                StatusCode::OK,
                Json(json!({ "status": "done", "task_id": req.task_id })),
            )
                .into_response()
        }
        Err(e) => (
            StatusCode::NOT_FOUND,
            Json(json!({ "error": e.to_string() })),
        )
            .into_response(),
    }
}

async fn update_task_status(
    state: &AppState,
    task_id: &str,
    new_status: TaskStatus,
    actor: &crate::actor_token::ActorIdentity,
) -> anyhow::Result<()> {
    let json_str = {
        let txn = state
            .control
            .try_transact()
            .map_err(|_| anyhow::anyhow!("circle state busy; retry shortly"))?;
        let tasks_map = txn
            .get_map(TASKS_KEY)
            .ok_or_else(|| anyhow::anyhow!("task not found"))?;
        match tasks_map.get(&txn, task_id) {
            Some(Out::Any(Any::String(s))) => s.to_string(),
            _ => return Err(anyhow::anyhow!("task not found")),
        }
    };

    let mut task: Task = serde_json::from_str(&json_str)?;
    task.status = new_status.clone();
    task.updated_at = chrono::Utc::now();
    if new_status == TaskStatus::Claimed {
        task.claimed_by = Some(actor.agent_id.clone());
        task.claimed_by_peer_id = Some(actor.peer_id.clone());
    } else if new_status == TaskStatus::Done {
        task.completed_by = Some(actor.agent_id.clone());
        task.completed_by_peer_id = Some(actor.peer_id.clone());
    }

    let updated_json = serde_json::to_string(&task)?;
    let mut txn = state
        .control
        .try_transact_mut()
        .map_err(|_| anyhow::anyhow!("circle state busy; retry shortly"))?;
    let tasks_map = txn.get_or_insert_map(TASKS_KEY);
    tasks_map.insert(&mut txn, task_id, Any::String(updated_json.as_str().into()));
    Ok(())
}
