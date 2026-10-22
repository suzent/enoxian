//! Agent cards in a Circle's shared folder (see `agent::cards`).
//!
//! Reading lists every advertised agent with its card, so the settings panel
//! can show which of this device's agents still have none. Creating writes the
//! template for one of *this device's* agents only: a card is a file in the
//! shared folder like any other, but nobody should be minting cards for agents
//! on someone else's machine.

use axum::{
    extract::{Path, State},
    http::StatusCode,
    response::IntoResponse,
    Json,
};
use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::agent::cards;
use crate::daemon::DaemonState;

#[derive(Serialize)]
struct CardView {
    handle: String,
    owner: String,
    device: String,
    agent: String,
    path: String,
    /// True for an agent configured on this device.
    local: bool,
    exists: bool,
    summary: String,
}

pub async fn list(
    State(daemon): State<DaemonState>,
    Path(circle): Path<String>,
) -> axum::response::Response {
    let Some(state) = daemon.get(&circle) else {
        return not_found();
    };
    let cards: Vec<CardView> = cards::roster_cards(&state)
        .into_iter()
        .map(|(member, agent, card)| CardView {
            handle: format!("@{}/{}/{}", member.owner, member.device_label, agent),
            path: cards::relative_path(&member.owner, &member.device_label, &agent),
            local: member.peer_id == state.peer_id,
            exists: card.is_some(),
            summary: card.map(|c| c.summary).unwrap_or_default(),
            owner: member.owner,
            device: member.device_label,
            agent,
        })
        .collect();
    Json(json!({ "cards": cards })).into_response()
}

#[derive(Deserialize)]
pub struct CreateRequest {
    pub agent: String,
}

pub async fn create(
    State(daemon): State<DaemonState>,
    Path(circle): Path<String>,
    Json(req): Json<CreateRequest>,
) -> axum::response::Response {
    let Some(state) = daemon.get(&circle) else {
        return not_found();
    };
    let Some(me) = state.self_member().filter(|m| !m.device_label.is_empty()) else {
        return bad_request(
            "this device has no label in the Circle yet, so its agents have no handle".into(),
        );
    };
    // The same set the list marks as local: what this device advertises, which
    // includes agents named in `identity.toml` without an `agents.toml` entry.
    let local = me.agents.iter().any(|a| a.eq_ignore_ascii_case(&req.agent))
        || crate::agent::config::AgentConfig::load()
            .resolve(&req.agent)
            .is_some();
    if !local {
        return bad_request(format!(
            "no agent named '{}' is advertised by this device",
            req.agent
        ));
    }
    // Through the file API, which seeds the shared document as it writes the
    // file. Written to disk alone, a card opened in the editor at once would
    // load as the empty document the watcher had not filled yet — and the
    // editor would write that back over the template.
    let path = cards::relative_path(&me.owner, &me.device_label, &req.agent);
    let content = cards::template(&me.owner, &me.device_label, &req.agent);
    let resp = super::files::create_file(
        State(daemon),
        Path(circle),
        Json(super::files::CreateFileRequest {
            path: path.clone(),
            content: Some(content),
        }),
    )
    .await
    .into_response();
    match resp.status() {
        StatusCode::OK => Json(json!({ "path": path, "created": true })).into_response(),
        // Already there, perhaps synced in from another device: never replaced.
        StatusCode::CONFLICT => Json(json!({ "path": path, "created": false })).into_response(),
        _ => resp,
    }
}

fn not_found() -> axum::response::Response {
    (
        StatusCode::NOT_FOUND,
        Json(json!({"error": "circle not found"})),
    )
        .into_response()
}

fn bad_request(msg: String) -> axum::response::Response {
    (StatusCode::BAD_REQUEST, Json(json!({ "error": msg }))).into_response()
}

#[cfg(test)]
mod tests {
    use super::*;
    use yrs::{Any, GetString, Map, ReadTxn, Transact, WriteTxn};

    fn circle(agents: &[&str]) -> (DaemonState, crate::state::AppState, tempfile::TempDir) {
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
            "suzy".into(),
            crate::mls::new_mls_state(crate::mls::MlsIdentity::generate("peer").unwrap(), None),
        );
        {
            let mut txn = state.control.transact_mut();
            let map = txn.get_or_insert_map(crate::control::MEMBER_LIST_KEY);
            let entry = json!({
                "peer_id": "peer", "owner": "suzy", "agent_id": "suzy",
                "device_label": "mac", "agents": agents, "role": "member",
                "added_at": "2026-10-01T00:00:00Z", "signature": "",
            });
            map.insert(&mut txn, "peer", Any::String(entry.to_string().into()));
        }
        let daemon = DaemonState::new();
        daemon.insert_circle(
            "c1".into(),
            state.clone(),
            tokio_util::sync::CancellationToken::new(),
        );
        (daemon, state, dir)
    }

    async fn post(daemon: &DaemonState, agent: &str) -> (StatusCode, serde_json::Value) {
        let resp = create(
            State(daemon.clone()),
            Path("c1".into()),
            Json(CreateRequest {
                agent: agent.into(),
            }),
        )
        .await;
        let status = resp.status();
        let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        (status, serde_json::from_slice(&body).unwrap())
    }

    /// An agent this device advertises gets a card even with no `agents.toml`
    /// entry, and the template lands in the shared document exactly once —
    /// including when the document already holds it, as it does when the
    /// watcher reads the new file first.
    #[tokio::test]
    async fn an_advertised_agent_gets_its_template_once() {
        let (daemon, state, dir) = circle(&["identity-only-agent"]);
        let path = cards::relative_path("suzy", "mac", "identity-only-agent");
        let template = cards::template("suzy", "mac", "identity-only-agent");
        {
            let doc = state.get_or_create_doc(&path);
            let mut txn = doc.transact_mut();
            let text = txn.get_or_insert_text(path.as_str());
            yrs::Text::insert(&text, &mut txn, 0, &template);
        }
        let (status, body) = post(&daemon, "identity-only-agent").await;
        assert_eq!(status, StatusCode::OK, "{body}");
        assert_eq!(body["created"], true);
        let doc = state.get_or_create_doc(&path);
        let txn = doc.transact();
        let text = txn.get_text(path.as_str()).unwrap().get_string(&txn);
        assert_eq!(text, template, "not doubled");
        assert_eq!(
            std::fs::read_to_string(dir.path().join(&path)).unwrap(),
            template
        );

        let (status, _) = post(&daemon, "someone-elses-agent").await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
    }
}
