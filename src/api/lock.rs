use crate::control::{
    arbitration::append_lock_entry, fs_lock::set_readonly, CircleEvent, LockAction, LockEntry,
    Task, TaskStatus, LOCK_LOG_KEY, TASKS_KEY,
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
    pub run_id: Option<String>,
    pub path: String,
    pub agent_id: Option<String>,
    pub actor_token: Option<String>,
}

pub async fn bind_path(
    State(daemon): State<DaemonState>,
    Path(circle_id): Path<String>,
    Json(mut req): Json<PathRequest>,
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
    req.path = match crate::proposal::canonical_workspace_path(
        &state.workspace,
        std::path::Path::new(&req.path),
    ) {
        Ok(path) => match state.workspace.canonicalize().ok().and_then(|root| {
            path.strip_prefix(root)
                .ok()
                .map(|p| p.to_string_lossy().replace('\\', "/"))
        }) {
            Some(path) => path,
            None => return StatusCode::BAD_REQUEST.into_response(),
        },
        Err(error) => {
            return (
                StatusCode::BAD_REQUEST,
                Json(json!({"error": error.to_string()})),
            )
                .into_response()
        }
    };
    let mut actor = match super::actor::resolve_actor(
        &state,
        req.actor_token.as_deref(),
        req.agent_id.clone(),
        "anonymous",
    ) {
        Ok(actor) => actor,
        Err(error) => return error.into_response(),
    };
    if let Err(error) = attach_run(&state, &mut actor, req.run_id.as_deref(), false) {
        return (
            StatusCode::CONFLICT,
            Json(json!({"error": error.to_string()})),
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
            run_id: actor.run_id.clone(),
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
        if crate::control::arbitration::is_locked_by_other_run(
            &lock_log,
            &txn,
            &req.path,
            &agent_id,
            &entry.peer_id,
            entry.run_id.as_deref(),
        ) {
            return (
                StatusCode::CONFLICT,
                Json(json!({"error": "path belongs to another agent or run"})),
            )
                .into_response();
        }
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
    Json(mut req): Json<PathRequest>,
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
    req.path = match crate::proposal::canonical_workspace_path(
        &state.workspace,
        std::path::Path::new(&req.path),
    ) {
        Ok(path) => match state.workspace.canonicalize().ok().and_then(|root| {
            path.strip_prefix(root)
                .ok()
                .map(|p| p.to_string_lossy().replace('\\', "/"))
        }) {
            Some(path) => path,
            None => return StatusCode::BAD_REQUEST.into_response(),
        },
        Err(error) => {
            return (
                StatusCode::BAD_REQUEST,
                Json(json!({"error": error.to_string()})),
            )
                .into_response()
        }
    };
    let mut actor = match super::actor::resolve_actor(
        &state,
        req.actor_token.as_deref(),
        req.agent_id,
        "anonymous",
    ) {
        Ok(actor) => actor,
        Err(error) => return error.into_response(),
    };
    if let Err(error) = attach_run(&state, &mut actor, req.run_id.as_deref(), true) {
        return (
            StatusCode::CONFLICT,
            Json(json!({"error": error.to_string()})),
        )
            .into_response();
    }
    let agent_id = actor.agent_id.clone();

    {
        let entry = LockEntry {
            run_id: actor.run_id.clone(),
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
        if crate::control::arbitration::is_locked_by_other_run(
            &lock_log,
            &txn,
            &req.path,
            &agent_id,
            &entry.peer_id,
            entry.run_id.as_deref(),
        ) {
            return (
                StatusCode::CONFLICT,
                Json(json!({"error": "path belongs to another agent or run"})),
            )
                .into_response();
        }
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

fn attach_run(
    state: &AppState,
    actor: &mut crate::actor_token::ActorIdentity,
    id: Option<&str>,
    releasing: bool,
) -> anyhow::Result<()> {
    let Some(id) = id else {
        return Ok(());
    };
    crate::proposal::validate_storage_id("run", id)?;
    anyhow::ensure!(
        actor.run_id.as_deref().is_none_or(|bound| bound == id),
        "token belongs to another run"
    );
    let record: crate::proposal::runs::RunRecord = serde_json::from_slice(&std::fs::read(
        state
            .circle_dir
            .join("managed_runs")
            .join(format!("{id}.json")),
    )?)?;
    anyhow::ensure!(
        record.session.circle_id == state.circle_id
            && record.session.actor_id.as_deref() == Some(actor.agent_id.as_str()),
        "run belongs to another actor"
    );
    anyhow::ensure!(releasing || record.session.is_open(), "run has finished");
    actor.run_id = Some(id.to_string());
    Ok(())
}

// ── Task claiming / completion ─────────────────────────────────────────────

#[derive(Deserialize)]
pub struct TaskRequest {
    pub task_id: String,
    pub agent_id: Option<String>,
    pub actor_token: Option<String>,
    /// Claim the task even though someone else holds it.
    #[serde(default)]
    pub takeover: bool,
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
    let transition = if req.takeover {
        Transition::Takeover
    } else {
        Transition::Claim
    };
    match update_task_status(&state, &req.task_id, transition, &actor).await {
        Ok(taken_over_from) => {
            let _ = state.events.send(CircleEvent::TaskClaimed {
                task_id: req.task_id.clone(),
                agent_id,
                taken_over_from: taken_over_from.clone(),
            });
            let mut body = json!({ "status": "claimed", "task_id": req.task_id });
            if let Some(previous) = taken_over_from {
                body["taken_over_from"] = json!(previous);
            }
            (StatusCode::OK, Json(body)).into_response()
        }
        Err(error) => {
            let status = error
                .downcast_ref::<TaskTransitionError>()
                .map(TaskTransitionError::status_code)
                .unwrap_or(StatusCode::NOT_FOUND);
            (status, Json(json!({ "error": error.to_string() }))).into_response()
        }
    }
}

pub async fn unclaim_task(
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
    match update_task_status(&state, &req.task_id, Transition::Unclaim, &actor).await {
        Ok(_) => {
            let _ = state.events.send(CircleEvent::TaskUnclaimed {
                task_id: req.task_id.clone(),
                agent_id,
            });
            (
                StatusCode::OK,
                Json(json!({ "status": "open", "task_id": req.task_id })),
            )
                .into_response()
        }
        Err(error) => {
            let status = error
                .downcast_ref::<TaskTransitionError>()
                .map(TaskTransitionError::status_code)
                .unwrap_or(StatusCode::NOT_FOUND);
            (status, Json(json!({ "error": error.to_string() }))).into_response()
        }
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
    match update_task_status(&state, &req.task_id, Transition::Done, &actor).await {
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
        Err(error) => {
            let status = error
                .downcast_ref::<TaskTransitionError>()
                .map(TaskTransitionError::status_code)
                .unwrap_or(StatusCode::NOT_FOUND);
            (status, Json(json!({ "error": error.to_string() }))).into_response()
        }
    }
}

async fn update_task_status(
    state: &AppState,
    task_id: &str,
    transition: Transition,
    actor: &crate::actor_token::ActorIdentity,
) -> anyhow::Result<Option<String>> {
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
    let taken_over_from = apply_task_status(&mut task, transition, actor)?;

    let updated_json = serde_json::to_string(&task)?;
    let mut txn = state
        .control
        .try_transact_mut()
        .map_err(|_| anyhow::anyhow!("circle state busy; retry shortly"))?;
    let tasks_map = txn.get_or_insert_map(TASKS_KEY);
    tasks_map.insert(&mut txn, task_id, Any::String(updated_json.as_str().into()));
    Ok(taken_over_from)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Transition {
    /// `open → claimed`; refused while someone else holds the task.
    Claim,
    /// `claimed → claimed` for a new holder, recording who was displaced, so
    /// a task held by an agent that went away does not stay stuck.
    Takeover,
    /// `claimed → open`, by the claimant only.
    Unclaim,
    /// `claimed → done`, by the claimant only. Final: a done task cannot be
    /// claimed again.
    Done,
}

#[derive(Debug, thiserror::Error)]
enum TaskTransitionError {
    #[error("task is not currently claimed")]
    NotClaimed,
    #[error("only the agent that claimed this task can do that")]
    NotClaimant,
    #[error("task is already claimed by {0}; use --takeover to take it over")]
    AlreadyClaimed(String),
    #[error("task is already done")]
    AlreadyDone,
}

impl TaskTransitionError {
    fn status_code(&self) -> StatusCode {
        match self {
            Self::NotClaimed | Self::AlreadyClaimed(_) | Self::AlreadyDone => StatusCode::CONFLICT,
            Self::NotClaimant => StatusCode::FORBIDDEN,
        }
    }
}

/// Apply `transition` to `task`. Returns the claimant a takeover displaced.
fn apply_task_status(
    task: &mut Task,
    transition: Transition,
    actor: &crate::actor_token::ActorIdentity,
) -> Result<Option<String>, TaskTransitionError> {
    let same_agent = task.claimed_by.as_deref() == Some(actor.agent_id.as_str());
    let same_device = task
        .claimed_by_peer_id
        .as_deref()
        .is_none_or(|peer_id| peer_id == actor.peer_id);
    let is_claimant = same_agent && same_device;
    let mut taken_over_from = None;
    match transition {
        Transition::Unclaim | Transition::Done => {
            if task.status != TaskStatus::Claimed {
                return Err(TaskTransitionError::NotClaimed);
            }
            if !is_claimant {
                return Err(TaskTransitionError::NotClaimant);
            }
            if transition == Transition::Unclaim {
                task.claimed_by = None;
                task.claimed_by_peer_id = None;
                task.unclaimed_by = Some(actor.agent_id.clone());
                task.unclaimed_by_peer_id = Some(actor.peer_id.clone());
                task.status = TaskStatus::Open;
            } else {
                task.completed_by = Some(actor.agent_id.clone());
                task.completed_by_peer_id = Some(actor.peer_id.clone());
                task.status = TaskStatus::Done;
            }
        }
        Transition::Claim | Transition::Takeover => {
            if task.status == TaskStatus::Done {
                return Err(TaskTransitionError::AlreadyDone);
            }
            let held_by_other = task.status == TaskStatus::Claimed && !is_claimant;
            if held_by_other {
                // Re-claiming your own task stays idempotent; taking someone
                // else's has to be asked for, so nobody is replaced silently.
                if transition == Transition::Claim {
                    let holder = task.claimed_by.clone().unwrap_or_default();
                    return Err(TaskTransitionError::AlreadyClaimed(holder));
                }
                taken_over_from = task.claimed_by.clone();
                task.taken_over_from = task.claimed_by.take();
                task.taken_over_from_peer_id = task.claimed_by_peer_id.take();
            } else if task.status == TaskStatus::Open {
                task.taken_over_from = None;
                task.taken_over_from_peer_id = None;
            }
            task.claimed_by = Some(actor.agent_id.clone());
            task.claimed_by_peer_id = Some(actor.peer_id.clone());
            task.status = TaskStatus::Claimed;
        }
    }
    task.updated_at = chrono::Utc::now();
    Ok(taken_over_from)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn task(claimed_by: Option<&str>, claimed_by_peer_id: Option<&str>) -> Task {
        Task {
            task_id: "task-1".into(),
            title: "Test".into(),
            description: None,
            status: TaskStatus::Claimed,
            created_by: "creator".into(),
            created_by_peer_id: "creator-peer".into(),
            claimed_by: claimed_by.map(str::to_owned),
            claimed_by_peer_id: claimed_by_peer_id.map(str::to_owned),
            unclaimed_by: None,
            unclaimed_by_peer_id: None,
            completed_by: None,
            completed_by_peer_id: None,
            taken_over_from: None,
            taken_over_from_peer_id: None,
            created_at: chrono::Utc::now(),
            updated_at: chrono::Utc::now(),
        }
    }

    fn actor(agent_id: &str, peer_id: &str) -> crate::actor_token::ActorIdentity {
        let now = chrono::Utc::now();
        crate::actor_token::ActorIdentity {
            run_id: None,
            registration_id: "registration".into(),
            agent_id: agent_id.into(),
            circle_id: "circle".into(),
            peer_id: peer_id.into(),
            issued_at: now,
            expires_at: now + chrono::Duration::hours(1),
        }
    }

    #[test]
    fn claimant_can_return_task_to_open_pool() {
        let mut task = task(Some("codex"), Some("device-a"));

        apply_task_status(&mut task, Transition::Unclaim, &actor("codex", "device-a")).unwrap();

        assert_eq!(task.status, TaskStatus::Open);
        assert_eq!(task.claimed_by, None);
        assert_eq!(task.claimed_by_peer_id, None);
        assert_eq!(task.unclaimed_by.as_deref(), Some("codex"));
        assert_eq!(task.unclaimed_by_peer_id.as_deref(), Some("device-a"));
    }

    #[test]
    fn another_actor_cannot_unclaim_task() {
        let mut task = task(Some("codex"), Some("device-a"));

        let error = apply_task_status(&mut task, Transition::Unclaim, &actor("hermes", "device-a"))
            .unwrap_err();

        assert!(matches!(error, TaskTransitionError::NotClaimant));
        assert_eq!(task.status, TaskStatus::Claimed);
    }

    #[test]
    fn same_label_on_another_device_cannot_unclaim_task() {
        let mut task = task(Some("codex"), Some("device-a"));

        let error = apply_task_status(&mut task, Transition::Unclaim, &actor("codex", "device-b"))
            .unwrap_err();

        assert!(matches!(error, TaskTransitionError::NotClaimant));
    }

    #[test]
    fn matching_legacy_claim_without_peer_id_can_be_unclaimed() {
        let mut task = task(Some("codex"), None);

        apply_task_status(&mut task, Transition::Unclaim, &actor("codex", "device-a")).unwrap();

        assert_eq!(task.status, TaskStatus::Open);
    }

    #[test]
    fn open_task_cannot_be_unclaimed_again() {
        let mut task = task(None, None);
        task.status = TaskStatus::Open;

        let error = apply_task_status(&mut task, Transition::Unclaim, &actor("codex", "device-a"))
            .unwrap_err();

        assert!(matches!(error, TaskTransitionError::NotClaimed));
    }

    #[test]
    fn another_actor_cannot_claim_a_claimed_task() {
        let mut task = task(Some("codex"), Some("device-a"));

        let error = apply_task_status(&mut task, Transition::Claim, &actor("hermes", "device-b"))
            .unwrap_err();

        assert!(matches!(error, TaskTransitionError::AlreadyClaimed(ref by) if by == "codex"));
        assert_eq!(task.claimed_by.as_deref(), Some("codex"));
        assert_eq!(task.claimed_by_peer_id.as_deref(), Some("device-a"));
    }

    #[test]
    fn same_label_on_another_device_cannot_claim_a_claimed_task() {
        let mut task = task(Some("codex"), Some("device-a"));

        let error = apply_task_status(&mut task, Transition::Claim, &actor("codex", "device-b"))
            .unwrap_err();

        assert!(matches!(error, TaskTransitionError::AlreadyClaimed(_)));
        assert_eq!(task.claimed_by_peer_id.as_deref(), Some("device-a"));
    }

    #[test]
    fn claimant_can_reclaim_its_own_task() {
        let mut task = task(Some("codex"), Some("device-a"));

        apply_task_status(&mut task, Transition::Claim, &actor("codex", "device-a")).unwrap();

        assert_eq!(task.status, TaskStatus::Claimed);
        assert_eq!(task.claimed_by.as_deref(), Some("codex"));
    }

    #[test]
    fn open_task_can_be_claimed() {
        let mut task = task(None, None);
        task.status = TaskStatus::Open;

        apply_task_status(&mut task, Transition::Claim, &actor("hermes", "device-b")).unwrap();

        assert_eq!(task.status, TaskStatus::Claimed);
        assert_eq!(task.claimed_by.as_deref(), Some("hermes"));
        assert_eq!(task.claimed_by_peer_id.as_deref(), Some("device-b"));
    }

    #[test]
    fn takeover_replaces_claimant_and_records_who_was_displaced() {
        let mut task = task(Some("codex"), Some("device-a"));

        let displaced = apply_task_status(
            &mut task,
            Transition::Takeover,
            &actor("hermes", "device-b"),
        )
        .unwrap();

        assert_eq!(displaced.as_deref(), Some("codex"));
        assert_eq!(task.claimed_by.as_deref(), Some("hermes"));
        assert_eq!(task.claimed_by_peer_id.as_deref(), Some("device-b"));
        assert_eq!(task.taken_over_from.as_deref(), Some("codex"));
        assert_eq!(task.taken_over_from_peer_id.as_deref(), Some("device-a"));
    }

    #[test]
    fn takeover_of_own_or_open_task_displaces_nobody() {
        let mut held = task(Some("codex"), Some("device-a"));
        let displaced =
            apply_task_status(&mut held, Transition::Takeover, &actor("codex", "device-a"))
                .unwrap();
        assert_eq!(displaced, None);

        let mut open = task(None, None);
        open.status = TaskStatus::Open;
        let displaced = apply_task_status(
            &mut open,
            Transition::Takeover,
            &actor("hermes", "device-b"),
        )
        .unwrap();
        assert_eq!(displaced, None);
        assert_eq!(open.claimed_by.as_deref(), Some("hermes"));
    }

    #[test]
    fn claimant_can_mark_task_done() {
        let mut task = task(Some("codex"), Some("device-a"));

        apply_task_status(&mut task, Transition::Done, &actor("codex", "device-a")).unwrap();

        assert_eq!(task.status, TaskStatus::Done);
        assert_eq!(task.completed_by.as_deref(), Some("codex"));
    }

    #[test]
    fn another_actor_cannot_mark_task_done() {
        let mut task = task(Some("codex"), Some("device-a"));

        let error = apply_task_status(&mut task, Transition::Done, &actor("hermes", "device-b"))
            .unwrap_err();

        assert!(matches!(error, TaskTransitionError::NotClaimant));
        assert_eq!(task.status, TaskStatus::Claimed);
    }

    #[test]
    fn unclaimed_task_cannot_be_marked_done() {
        let mut task = task(None, None);
        task.status = TaskStatus::Open;

        let error = apply_task_status(&mut task, Transition::Done, &actor("codex", "device-a"))
            .unwrap_err();

        assert!(matches!(error, TaskTransitionError::NotClaimed));
    }

    #[test]
    fn done_task_cannot_be_claimed_or_taken_over() {
        for transition in [Transition::Claim, Transition::Takeover] {
            let mut task = task(Some("codex"), Some("device-a"));
            task.status = TaskStatus::Done;

            let error =
                apply_task_status(&mut task, transition, &actor("hermes", "device-b")).unwrap_err();

            assert!(matches!(error, TaskTransitionError::AlreadyDone));
            assert_eq!(task.status, TaskStatus::Done);
        }
    }
}
