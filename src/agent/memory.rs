//! Per-agent ACP session memory.
//!
//! To give a mentioned agent conversation continuity, we persist the ACP
//! `sessionId` it was assigned, keyed by (circle, agent). On the next mention
//! we hand that id back so the driver can `session/load` and resume — the agent
//! remembers what it said and did before.
//!
//! Alongside the session id we remember the last chat message the agent has
//! already seen. Session resume restores what the *agent* said and did, but not
//! what the room said while it was away, so [`super::context`] uses this mark
//! to send only the messages posted since the agent's last turn. See that
//! module for how the delta is framed.
//!
//! Persistence is best-effort: the ACP spec does not guarantee an agent retains
//! session state across its own process restarts, so a stored id may fail to
//! load. The driver falls back to a fresh session in that case (see
//! `AcpSession::start`). We store records under the circle dir so they survive
//! daemon restarts and are naturally scoped per circle.
//!
//! One session per (circle, agent): all mentions of `@claude` in a circle share
//! one evolving conversation, matching "claude is a participant in this room".

use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// What we remember about an agent's conversation in one circle.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Record {
    /// ACP session id to resume. Empty if we have none.
    pub session_id: String,
    /// Id of the last contiguous chat message delivered in its input context.
    /// Posting an output never advances this cursor.
    /// Empty for a record written before this field existed.
    #[serde(default)]
    pub last_seen_message: String,
    /// Ids delivered *ahead* of `last_seen_message` — the lines the last prompt
    /// showed out of contiguous order. [`super::context`] subtracts them from
    /// the next prompt so nothing is shown twice, and prunes them as the cursor
    /// catches up. Empty for a record written before this field existed.
    #[serde(default)]
    pub delivered: Vec<String>,
}

/// Directory holding per-agent records under a circle dir.
fn dir(circle_dir: &Path) -> PathBuf {
    circle_dir.join("agent_sessions")
}

/// Path of the record for one agent. The agent name is sanitized so a scoped or
/// odd name can't escape the directory.
fn path(circle_dir: &Path, agent: &str) -> PathBuf {
    let safe: String = agent
        .chars()
        .map(|c| {
            if c.is_alphanumeric() || c == '-' || c == '_' {
                c
            } else {
                '_'
            }
        })
        .collect();
    dir(circle_dir).join(format!("{safe}.session"))
}

/// The stored record for `agent` in this circle, if any.
///
/// Records written before this file held JSON are a bare session id; they are
/// read as a record with no seen-mark, which costs that agent one turn without
/// the chat delta and then self-heals on the next save.
pub fn load(circle_dir: &Path, agent: &str) -> Option<Record> {
    let raw = std::fs::read_to_string(path(circle_dir, agent)).ok()?;
    let raw = raw.trim();
    if raw.is_empty() {
        return None;
    }
    let record = serde_json::from_str::<Record>(raw).unwrap_or_else(|_| Record {
        session_id: raw.to_string(),
        ..Record::default()
    });
    // Useful when *either* field is set. Requiring a session id discarded the
    // record of an agent that has only ever passed (§2.3) — it has a seen-mark
    // and no session, and dropping it made the mark unreadable.
    (!record.session_id.is_empty() || !record.last_seen_message.is_empty()).then_some(record)
}

/// Persist the ACP session id, preserving any seen-mark already stored.
pub fn save_session(circle_dir: &Path, agent: &str, session_id: &str) -> std::io::Result<()> {
    let _guard = mutation_lock(circle_dir, agent)?;
    let mut record = load(circle_dir, agent).unwrap_or_default();
    record.session_id = session_id.to_string();
    write(circle_dir, agent, &record)
}

/// Advance an agent's seen-mark, preserving any session id.
///
/// Creates the record when there is none. It used to return early instead,
/// which was invisible while every turn came from a mention — an agent that
/// replies gets a session id, and the record exists from then on. An agent that
/// *passes* on its very first turn (engagement spec §2.3) never writes one, so
/// the mark never advanced and it re-read the same history on every later turn.
pub fn save_seen(
    circle_dir: &Path,
    agent: &str,
    message_id: &str,
    delivered: Vec<String>,
) -> std::io::Result<()> {
    let _guard = mutation_lock(circle_dir, agent)?;
    let mut record = load(circle_dir, agent).unwrap_or_default();
    record.last_seen_message = message_id.to_string();
    record.delivered = delivered;
    write(circle_dir, agent, &record)
}

fn mutation_lock(circle_dir: &Path, agent: &str) -> std::io::Result<std::fs::File> {
    std::fs::create_dir_all(dir(circle_dir))?;
    let file = std::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(path(circle_dir, agent).with_extension("lock"))?;
    file.lock()?;
    Ok(file)
}

fn write(circle_dir: &Path, agent: &str, record: &Record) -> std::io::Result<()> {
    let d = dir(circle_dir);
    std::fs::create_dir_all(&d)?;
    crate::proposal::runs::atomic_json(&path(circle_dir, agent), record)
        .map_err(std::io::Error::other)
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

    fn tmpdir() -> PathBuf {
        let tmp = std::env::temp_dir().join(format!("enox-mem-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&tmp).unwrap();
        tmp
    }

    #[test]
    fn roundtrip_and_sanitize() {
        let tmp = tmpdir();

        assert!(load(&tmp, "claude").is_none());
        save_session(&tmp, "claude", "sess-123").unwrap();
        assert_eq!(load(&tmp, "claude").unwrap().session_id, "sess-123");

        // A scoped/odd agent name is sanitized to a safe filename.
        save_session(&tmp, "alice/laptop/claude", "sess-xyz").unwrap();
        assert_eq!(
            load(&tmp, "alice/laptop/claude").unwrap().session_id,
            "sess-xyz"
        );

        clear(&tmp, "claude").unwrap();
        assert!(load(&tmp, "claude").is_none());

        std::fs::remove_dir_all(&tmp).ok();
    }

    #[test]
    fn session_and_seen_mark_do_not_clobber_each_other() {
        let tmp = tmpdir();

        save_session(&tmp, "claude", "sess-1").unwrap();
        save_seen(&tmp, "claude", "msg-7", Vec::new()).unwrap();
        let record = load(&tmp, "claude").unwrap();
        assert_eq!(record.session_id, "sess-1");
        assert_eq!(record.last_seen_message, "msg-7");

        // A later session id (e.g. the agent lost its session and started a
        // fresh one) keeps the seen-mark.
        save_session(&tmp, "claude", "sess-2").unwrap();
        let record = load(&tmp, "claude").unwrap();
        assert_eq!(record.session_id, "sess-2");
        assert_eq!(record.last_seen_message, "msg-7");

        std::fs::remove_dir_all(&tmp).ok();
    }

    #[test]
    fn legacy_bare_session_id_still_loads() {
        let tmp = tmpdir();
        std::fs::create_dir_all(dir(&tmp)).unwrap();
        std::fs::write(path(&tmp, "claude"), "sess-legacy\n").unwrap();

        let record = load(&tmp, "claude").unwrap();
        assert_eq!(record.session_id, "sess-legacy");
        assert!(record.last_seen_message.is_empty());

        // Writing a seen-mark upgrades the file in place.
        save_seen(&tmp, "claude", "msg-1", Vec::new()).unwrap();
        let record = load(&tmp, "claude").unwrap();
        assert_eq!(record.session_id, "sess-legacy");
        assert_eq!(record.last_seen_message, "msg-1");

        std::fs::remove_dir_all(&tmp).ok();
    }

    #[test]
    fn a_seen_mark_is_kept_even_with_no_session() {
        // An agent that passes on its very first turn (§2.3) has a mark and no
        // session. Dropping the record made the mark unreadable, so it re-read
        // the same history on every later turn.
        let tmp = tmpdir();
        save_seen(&tmp, "claude", "msg-1", Vec::new()).unwrap();
        let record = load(&tmp, "claude").expect("a mark alone is worth keeping");
        assert_eq!(record.last_seen_message, "msg-1");
        assert!(record.session_id.is_empty());

        // And a session id arriving later joins it rather than replacing it.
        save_session(&tmp, "claude", "sess-1").unwrap();
        let record = load(&tmp, "claude").unwrap();
        assert_eq!(record.session_id, "sess-1");
        assert_eq!(record.last_seen_message, "msg-1");
        std::fs::remove_dir_all(&tmp).ok();
    }
}
