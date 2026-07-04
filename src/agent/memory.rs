//! Per-agent ACP session memory.
//!
//! To give a mentioned agent conversation continuity, we persist the ACP
//! `sessionId` it was assigned, keyed by (circle, agent). On the next mention
//! we hand that id back so the driver can `session/load` and resume — the agent
//! remembers what it said and did before.
//!
//! Persistence is best-effort: the ACP spec does not guarantee an agent retains
//! session state across its own process restarts, so a stored id may fail to
//! load. The driver falls back to a fresh session in that case (see
//! `AcpSession::start`). We store ids under the circle dir so they survive
//! daemon restarts and are naturally scoped per circle.
//!
//! One session per (circle, agent): all mentions of `@claude` in a circle share
//! one evolving conversation, matching "claude is a participant in this room".

use std::path::{Path, PathBuf};

/// Directory holding per-agent session-id files under a circle dir.
fn dir(circle_dir: &Path) -> PathBuf {
    circle_dir.join("agent_sessions")
}

/// Path of the session-id record for one agent. The agent name is sanitized so
/// a scoped or odd name can't escape the directory.
fn path(circle_dir: &Path, agent: &str) -> PathBuf {
    let safe: String = agent
        .chars()
        .map(|c| if c.is_alphanumeric() || c == '-' || c == '_' { c } else { '_' })
        .collect();
    dir(circle_dir).join(format!("{safe}.session"))
}

/// The stored ACP session id for `agent` in this circle, if any.
pub fn load(circle_dir: &Path, agent: &str) -> Option<String> {
    let id = std::fs::read_to_string(path(circle_dir, agent)).ok()?;
    let id = id.trim().to_string();
    if id.is_empty() { None } else { Some(id) }
}

/// Persist the ACP session id so the next mention can resume it.
pub fn save(circle_dir: &Path, agent: &str, session_id: &str) -> std::io::Result<()> {
    let d = dir(circle_dir);
    std::fs::create_dir_all(&d)?;
    std::fs::write(path(circle_dir, agent), session_id)
}

/// Forget an agent's session (e.g. a user "reset conversation" action). Not
/// wired to a command yet, but kept so the surface is complete.
#[allow(dead_code)]
pub fn clear(circle_dir: &Path, agent: &str) -> std::io::Result<()> {
    let p = path(circle_dir, agent);
    if p.exists() {
        std::fs::remove_file(p)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip_and_sanitize() {
        let tmp = std::env::temp_dir().join(format!("enox-mem-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&tmp).unwrap();

        assert_eq!(load(&tmp, "claude"), None);
        save(&tmp, "claude", "sess-123").unwrap();
        assert_eq!(load(&tmp, "claude").as_deref(), Some("sess-123"));

        // A scoped/odd agent name is sanitized to a safe filename.
        save(&tmp, "alice/laptop/claude", "sess-xyz").unwrap();
        assert_eq!(load(&tmp, "alice/laptop/claude").as_deref(), Some("sess-xyz"));

        clear(&tmp, "claude").unwrap();
        assert_eq!(load(&tmp, "claude"), None);

        std::fs::remove_dir_all(&tmp).ok();
    }
}
