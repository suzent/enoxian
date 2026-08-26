use crate::actor_token::ActorIdentity;
use crate::daemon::DaemonState;
use crate::state::AppState;
use axum::{
    extract::{Path, State},
    http::StatusCode,
    response::{IntoResponse, Response},
    Json,
};
use serde::Deserialize;
use serde_json::json;

#[derive(Deserialize)]
pub struct RegisterActorRequest {
    pub agent_id: String,
}

pub async fn register_actor(
    State(daemon): State<DaemonState>,
    Path(circle_id): Path<String>,
    Json(req): Json<RegisterActorRequest>,
) -> Response {
    let Some(state) = daemon.get(&circle_id) else {
        return (
            StatusCode::NOT_FOUND,
            Json(json!({"error": "circle not found"})),
        )
            .into_response();
    };
    let agent_id = req.agent_id.trim();
    if !valid_agent_id(agent_id) {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({
                "error": "agent_id must be 1-64 characters and contain no control characters"
            })),
        )
            .into_response();
    }

    let (token, actor) = state
        .actor_tokens
        .issue(&state.circle_id, &state.peer_id, agent_id);
    (
        StatusCode::CREATED,
        Json(json!({
            "token": token,
            "registration_id": actor.registration_id,
            "agent_id": actor.agent_id,
            "circle_id": actor.circle_id,
            "peer_id": actor.peer_id,
            "issued_at": actor.issued_at,
            "expires_at": actor.expires_at,
        })),
    )
        .into_response()
}

pub(crate) fn resolve_actor(
    state: &AppState,
    token: Option<&str>,
    legacy_agent_id: Option<String>,
    fallback: &str,
) -> Result<ActorIdentity, ActorAuthError> {
    if let Some(token) = token {
        return state
            .actor_tokens
            .validate(token, &state.circle_id, &state.peer_id)
            .map_err(|_| ActorAuthError::InvalidToken);
    }

    // Backward compatibility for the UI and older CLI clients. This is local
    // API attribution, not a separately authenticated agent identity.
    Ok(ActorIdentity {
        registration_id: String::new(),
        agent_id: legacy_agent_id.unwrap_or_else(|| fallback.to_string()),
        circle_id: state.circle_id.clone(),
        peer_id: state.peer_id.clone(),
        issued_at: chrono::Utc::now(),
        expires_at: chrono::Utc::now(),
    })
}

pub(crate) enum ActorAuthError {
    InvalidToken,
}

impl IntoResponse for ActorAuthError {
    fn into_response(self) -> Response {
        match self {
            Self::InvalidToken => (
                StatusCode::UNAUTHORIZED,
                Json(json!({
                    "error": "invalid or expired actor token",
                    "code": "invalid_actor_token"
                })),
            )
                .into_response(),
        }
    }
}

fn valid_agent_id(value: &str) -> bool {
    !value.is_empty() && value.chars().count() <= 64 && !value.chars().any(char::is_control)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validates_agent_labels() {
        assert!(valid_agent_id("codex-worker-2"));
        assert!(!valid_agent_id(""));
        assert!(!valid_agent_id("bad\nname"));
        assert!(!valid_agent_id(&"x".repeat(65)));
    }
}
