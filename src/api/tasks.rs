use axum::{
    extract::{Path, Query, State},
    http::StatusCode,
    response::IntoResponse,
    Json,
};
use serde::Deserialize;
use serde_json::json;
use yrs::{Any, Map, MapRef, Out, Transact};
use crate::control::{CircleEvent, Task, TaskStatus, TASKS_KEY};
use crate::daemon::DaemonState;

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
        None => return (StatusCode::NOT_FOUND, Json(json!({"error": "circle not found"}))).into_response(),
    };
    let doc = &state.control;
    let tasks_map: MapRef = doc.get_or_insert_map(TASKS_KEY);
    let txn = doc.transact();

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
}

pub async fn create_task(
    State(daemon): State<DaemonState>,
    Path(circle_id): Path<String>,
    Json(req): Json<CreateTaskRequest>,
) -> impl IntoResponse {
    let state = match daemon.get(&circle_id) {
        Some(s) => s,
        None => return (StatusCode::NOT_FOUND, Json(json!({"error": "circle not found"}))).into_response(),
    };
    let task = Task {
        task_id: uuid::Uuid::new_v4().to_string(),
        title: req.title,
        description: req.description,
        status: TaskStatus::Open,
        created_by: req.created_by.unwrap_or_else(|| "unknown".to_string()),
        claimed_by: None,
        created_at: chrono::Utc::now(),
        updated_at: chrono::Utc::now(),
    };

    let json_str = match serde_json::to_string(&task) {
        Ok(s) => s,
        Err(_) => return (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({"error": "serialize failed"}))).into_response(),
    };
    let task_id = task.task_id.clone();

    {
        let doc = &state.control;
        let tasks_map: MapRef = doc.get_or_insert_map(TASKS_KEY);
        let mut txn = doc.transact_mut();
        tasks_map.insert(&mut txn, task.task_id.as_str(), Any::String(json_str.as_str().into()));
    }

    let _ = state.events.send(CircleEvent::TaskCreated { task_id: task_id.clone() });
    (StatusCode::CREATED, Json(json!({ "task_id": task_id, "status": "created" }))).into_response()
}

