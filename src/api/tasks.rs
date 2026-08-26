use crate::control::{CircleEvent, Task, TaskStatus, TASKS_KEY};
use crate::daemon::DaemonState;
use axum::{
    extract::{Path, Query, State},
    http::StatusCode,
    response::IntoResponse,
    Json,
};
use serde::Deserialize;
use serde_json::json;
use yrs::{Any, Map, Out, ReadTxn, Transact, WriteTxn};

#[derive(Deserialize)]
pub struct TasksQuery {
    pub status: Option<String>,
}

pub async fn get_tasks(
    State(daemon): State<DaemonState>,
    Path(circle_id): Path<String>,
    Query(q): Query<TasksQuery>,
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
    let txn = match state.control.try_transact() {
        Ok(txn) => txn,
        Err(_) => return super::circle_busy(),
    };
    let Some(tasks_map) = txn.get_map(TASKS_KEY) else {
        return Json(Vec::<Task>::new()).into_response();
    };

    let mut result: Vec<Task> = Vec::new();
    for (_key, val) in tasks_map.iter(&txn) {
        if let Out::Any(Any::String(s)) = val {
            if let Ok(t) = serde_json::from_str::<Task>(&s) {
                if let Some(ref filter) = q.status {
                    if t.status.to_string() != *filter {
                        continue;
                    }
                }
                result.push(t);
            }
        }
    }
    result.sort_by_key(|a| a.created_at);
    Json(result).into_response()
}

#[derive(Deserialize)]
pub struct CreateTaskRequest {
    pub title: String,
    pub description: Option<String>,
    pub created_by: Option<String>,
    pub actor_token: Option<String>,
}

pub async fn create_task(
    State(daemon): State<DaemonState>,
    Path(circle_id): Path<String>,
    Json(req): Json<CreateTaskRequest>,
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
        req.created_by.clone(),
        "unknown",
    ) {
        Ok(actor) => actor,
        Err(response) => return response,
    };
    let task = Task {
        task_id: uuid::Uuid::new_v4().to_string(),
        title: req.title,
        description: req.description,
        status: TaskStatus::Open,
        created_by: actor.agent_id,
        created_by_peer_id: actor.peer_id,
        claimed_by: None,
        claimed_by_peer_id: None,
        completed_by: None,
        completed_by_peer_id: None,
        created_at: chrono::Utc::now(),
        updated_at: chrono::Utc::now(),
    };

    let json_str = match serde_json::to_string(&task) {
        Ok(s) => s,
        Err(_) => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({"error": "serialize failed"})),
            )
                .into_response()
        }
    };
    let task_id = task.task_id.clone();

    {
        let mut txn = match state.control.try_transact_mut() {
            Ok(txn) => txn,
            Err(_) => return super::circle_busy(),
        };
        let tasks_map = txn.get_or_insert_map(TASKS_KEY);
        tasks_map.insert(
            &mut txn,
            task.task_id.as_str(),
            Any::String(json_str.as_str().into()),
        );
    }

    let _ = state.events.send(CircleEvent::TaskCreated {
        task_id: task_id.clone(),
    });
    (
        StatusCode::CREATED,
        Json(json!({ "task_id": task_id, "status": "created" })),
    )
        .into_response()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{config::JoinPolicy, mls, state::AppState};
    use std::path::PathBuf;
    use yrs::Transact;

    fn test_state() -> AppState {
        AppState::new(
            "circle".into(),
            "Circle".into(),
            PathBuf::new(),
            PathBuf::new(),
            String::new(),
            "agent".into(),
            1,
            "peer".into(),
            JoinPolicy::Manual,
            "owner".into(),
            mls::new_mls_state(mls::MlsIdentity::generate("peer").unwrap(), None),
        )
    }

    #[tokio::test]
    async fn busy_control_document_returns_retryable_503() {
        let daemon = DaemonState::new();
        let state = test_state();
        daemon.insert("circle".into(), state.clone());
        let _write_guard = state.control.transact_mut();

        let response = get_tasks(
            State(daemon),
            Path("circle".into()),
            Query(TasksQuery { status: None }),
        )
        .await
        .into_response();

        assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
        assert_eq!(response.headers()[axum::http::header::RETRY_AFTER], "1");
    }
}
