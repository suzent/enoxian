//! ACP login controls inherit the daemon's token-protected, loopback API boundary.
use crate::{agent::authentication::Action, daemon::DaemonState};
use axum::{
    extract::{Path, State},
    http::{header, StatusCode},
    response::{IntoResponse, Response},
    Json,
};
use serde::Deserialize;
use serde_json::json;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Request {
    action: Action,
    method_id: Option<String>,
    circle_id: Option<String>,
}

pub async fn states(State(daemon): State<DaemonState>) -> impl IntoResponse {
    (
        [(header::CACHE_CONTROL, "no-store")],
        Json(daemon.acp_auth.states()),
    )
}
pub async fn start(
    State(daemon): State<DaemonState>,
    Path(id): Path<String>,
    Json(request): Json<Request>,
) -> Response {
    if request
        .method_id
        .as_ref()
        .is_some_and(|id| id.is_empty() || id.len() > 256)
    {
        return StatusCode::BAD_REQUEST.into_response();
    }
    let workspace = match request.circle_id {
        Some(id) => match daemon.circles.get(&id) {
            Some(circle) => Some(circle.workspace.clone()),
            None => {
                return (
                    StatusCode::BAD_REQUEST,
                    Json(json!({"error":"Circle is not active"})),
                )
                    .into_response()
            }
        },
        None => None,
    };
    match daemon.acp_auth.start(&id, request.action, request.method_id, daemon.shutdown_token.clone(), workspace.as_deref()) {
        Ok(state) => (StatusCode::ACCEPTED, [(header::CACHE_CONTROL, "no-store")], Json(state)).into_response(),
        Err(_) => (StatusCode::BAD_REQUEST, Json(json!({"error": "Cannot start authentication; check the agent configuration and active jobs"}))).into_response(),
    }
}
pub async fn cancel(
    State(daemon): State<DaemonState>,
    Path(id): Path<String>,
) -> impl IntoResponse {
    daemon.acp_auth.cancel(&id);
    (StatusCode::ACCEPTED, Json(json!({"ok": true})))
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::{body::Body, http::Request as HttpRequest};
    use tower::ServiceExt;

    #[tokio::test]
    async fn login_controls_require_the_local_api_token() {
        let router = crate::api::router(DaemonState::new(), Some("local-secret".into()));
        for (method, path) in [
            ("GET", "/api/agent-auth"),
            ("POST", "/api/agent-auth/custom"),
            ("DELETE", "/api/agent-auth/custom"),
        ] {
            let response = router
                .clone()
                .oneshot(
                    HttpRequest::builder()
                        .method(method)
                        .uri(path)
                        .body(Body::empty())
                        .unwrap(),
                )
                .await
                .unwrap();
            assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
        }
        let response = router
            .oneshot(
                HttpRequest::builder()
                    .uri("/api/agent-auth")
                    .header("Authorization", "Bearer local-secret")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(response.headers()[header::CACHE_CONTROL], "no-store");
    }

    #[test]
    fn login_requests_cannot_supply_executable_arguments() {
        for field in ["command", "args", "env"] {
            let value = json!({"action":"login", field: ["untrusted"]});
            assert!(serde_json::from_value::<Request>(value).is_err());
        }
    }
}
