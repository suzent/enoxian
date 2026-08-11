//! Durable persistence for the coordination state in the `__control__` CRDT.
//!
//! The control doc (chat, tasks, members, presence, MLS scratch) is otherwise
//! in-memory only: an all-offline restart loses it, since nothing wrote it to
//! disk and no peer remains to re-sync from. This persists the **durable**
//! subset so a cold-started circle keeps its history. See
//! `docs/plan/control-persistence.md`.
//!
//! Selective durability (Tier A):
//!
//! - **tasks**, **member_list** — persisted in full (bounded, system-of-record).
//! - **chat** — persisted, time-windowed to the last [`CHAT_RETENTION_DAYS`]
//!   days so it never grows without bound. A member offline longer than the
//!   window misses those messages (documented trade-off — there is no per-member
//!   read cursor yet).
//! - **presence** — never persisted (stale-on-restore is wrong; it must be
//!   rebuilt by live heartbeats).
//! - **MLS scratch** — never persisted here (key material follows its own
//!   lifecycle via `group.save`).
//!
//! Mechanism (option 4a): serialize the durable key-sets to JSON and restore by
//! writing them back into the live doc through normal transactions. This sidesteps
//! CRDT-state-encoding subtleties and gives a natural point to window chat. The
//! Map entries are restored by key. Chat uses an append array, so snapshots are
//! deduplicated by stable message ID before restore and save; without that,
//! independently restored peers create distinct Yjs items for the same message
//! and multiply duplicates whenever their control docs merge.
//!
//! Restore runs at startup **before** the swarm connects. Restored chat carries
//! its original (old) timestamps, so the agent reaction loop's `ts` cutoff skips
//! it — a restored mention never re-triggers an agent.
//!
//! At-rest note: chat is written **plaintext** (like workspace files). Content
//! encryption is M17; until then this is a known at-rest exposure, documented in
//! `docs/concepts/security.md`.

use crate::control::{CHAT_KEY, MEMBER_LIST_KEY, TASKS_KEY};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use yrs::{Any, Array, Map, Out, Transact};

/// Chat older than this many days is dropped at save time.
const CHAT_RETENTION_DAYS: i64 = 30;
/// A busy or automated circle can produce far more messages than a time-only
/// window can safely restore during daemon startup.
const CHAT_RETENTION_MESSAGES: usize = 10_000;

fn path(circle_dir: &Path) -> PathBuf {
    circle_dir.join("control.json")
}

/// The persisted subset of the control doc.
#[derive(Debug, Default, Serialize, Deserialize)]
struct ControlSnapshot {
    /// Chat messages (JSON strings, as stored in the CRDT array), time-windowed.
    #[serde(default)]
    chat: Vec<String>,
    /// task_id → task JSON string.
    #[serde(default)]
    tasks: std::collections::BTreeMap<String, String>,
    /// peer_id → member entry JSON string.
    #[serde(default)]
    members: std::collections::BTreeMap<String, String>,
}

/// A `ChatMessage` is stored as a JSON string in the array; we only need its
/// timestamp to window, so parse minimally.
#[derive(Deserialize)]
struct TsOnly {
    ts: i64,
}

#[derive(Deserialize)]
struct IdOnly {
    id: String,
}

/// Read the durable subset from the live control doc, windowing chat, and write
/// it to `<circle_dir>/control.json`. Called on a debounced timer and at clean
/// shutdown.
pub fn save(circle_dir: &Path, control: &yrs::Doc) -> anyhow::Result<()> {
    let cutoff = chrono::Utc::now().timestamp() - CHAT_RETENTION_DAYS * 86_400;
    // Resolve the shared refs BEFORE opening a read transaction. `get_or_insert_*`
    // may need to create the type (a write), which would deadlock if a read
    // transaction is already held.
    let chat_arr = control.get_or_insert_array(CHAT_KEY);
    let tasks_map = control.get_or_insert_map(TASKS_KEY);
    let members_map = control.get_or_insert_map(MEMBER_LIST_KEY);
    let snap = {
        let txn = control.transact();

        let mut chat: Vec<String> = chat_arr
            .iter(&txn)
            .filter_map(|v| match v {
                Out::Any(Any::String(s)) => Some(s.to_string()),
                _ => None,
            })
            // Keep only messages within the retention window.
            .filter(|s| {
                serde_json::from_str::<TsOnly>(s)
                    .map(|m| m.ts >= cutoff)
                    .unwrap_or(true) // keep unparseable rather than lose data
            })
            .collect();
        deduplicate_messages(&mut chat);
        retain_latest_messages(&mut chat);

        let tasks = map_strings(&tasks_map, &txn);
        let members = map_strings(&members_map, &txn);

        ControlSnapshot { chat, tasks, members }
    };

    let json = serde_json::to_string(&snap)?;
    std::fs::create_dir_all(circle_dir)?;
    // Write atomically: temp file + rename, so a crash mid-write can't truncate
    // the last good snapshot.
    let tmp = path(circle_dir).with_extension("json.tmp");
    std::fs::write(&tmp, &json)?;
    std::fs::rename(&tmp, path(circle_dir))?;
    Ok(())
}

/// Restore the persisted subset into the live control doc. Existing keys are
/// overwritten with the persisted value. Chat is deduplicated by message ID and
/// only appended if the array is currently empty. Call once at startup, before
/// the swarm connects.
pub fn restore(circle_dir: &Path, control: &yrs::Doc) -> anyhow::Result<()> {
    let Ok(text) = std::fs::read_to_string(path(circle_dir)) else {
        return Ok(()); // nothing persisted yet
    };
    let mut snap: ControlSnapshot = serde_json::from_str(&text)?;
    let persisted_chat_count = snap.chat.len();
    deduplicate_messages(&mut snap.chat);
    retain_latest_messages(&mut snap.chat);
    if persisted_chat_count > snap.chat.len() {
        tracing::warn!(
            "[control] loading the latest {} of {} persisted chat messages",
            snap.chat.len(),
            persisted_chat_count
        );
    }

    // Resolve all refs before opening the transaction (get_or_insert_* may write
    // to create the type, which deadlocks under a held transaction).
    let tasks = control.get_or_insert_map(TASKS_KEY);
    let members = control.get_or_insert_map(MEMBER_LIST_KEY);
    let chat = control.get_or_insert_array(CHAT_KEY);

    // One write transaction for everything, avoiding any read/write interleave.
    {
        let mut txn = control.transact_mut();
        for (k, v) in &snap.tasks {
            tasks.insert(&mut txn, k.as_str(), v.as_str());
        }
        for (k, v) in &snap.members {
            members.insert(&mut txn, k.as_str(), v.as_str());
        }
        // Chat: only seed if the live array is empty. If a peer already re-synced
        // the history, don't append a second copy on top of it.
        if chat.len(&txn) == 0 && !snap.chat.is_empty() {
            for msg in &snap.chat {
                chat.push_back(&mut txn, Any::String(msg.as_str().into()));
            }
        }
    }

    tracing::info!(
        "[control] restored {} chat, {} tasks, {} members from disk",
        snap.chat.len(), snap.tasks.len(), snap.members.len()
    );
    Ok(())
}

fn retain_latest_messages(chat: &mut Vec<String>) {
    if chat.len() > CHAT_RETENTION_MESSAGES {
        chat.drain(..chat.len() - CHAT_RETENTION_MESSAGES);
    }
}

fn deduplicate_messages(chat: &mut Vec<String>) {
    let mut seen = std::collections::HashSet::new();
    chat.retain(|raw| {
        let key = serde_json::from_str::<IdOnly>(raw)
            .map(|message| format!("id:{}", message.id))
            .unwrap_or_else(|_| format!("raw:{raw}"));
        seen.insert(key)
    });
}

/// Collect a control-doc map's `key → String` entries.
fn map_strings(
    map: &yrs::MapRef,
    txn: &yrs::Transaction,
) -> std::collections::BTreeMap<String, String> {
    map.iter(txn)
        .filter_map(|(k, v)| match v {
            Out::Any(Any::String(s)) => Some((k.to_string(), s.to_string())),
            _ => None,
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use yrs::Doc;

    fn msg(id: &str, ts: i64) -> String {
        format!(r#"{{"id":"{id}","agent_id":"a","text":"hi","mentions":[],"ts":{ts}}}"#)
    }

    #[test]
    fn save_windows_chat_and_restore_roundtrips() {
        let tmp = std::env::temp_dir().join(format!("enox-ctrl-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&tmp).unwrap();
        let now = chrono::Utc::now().timestamp();

        // Source doc with fresh + stale chat, a task, a member.
        let src = Doc::new();
        {
            let chat = src.get_or_insert_array(CHAT_KEY);
            let tasks = src.get_or_insert_map(TASKS_KEY);
            let members = src.get_or_insert_map(MEMBER_LIST_KEY);
            let mut txn = src.transact_mut();
            chat.push_back(&mut txn, Any::String(msg("fresh", now).as_str().into()));
            chat.push_back(&mut txn, Any::String(msg("stale", now - 40 * 86_400).as_str().into()));
            tasks.insert(&mut txn, "t1", r#"{"task_id":"t1"}"#);
            members.insert(&mut txn, "p1", r#"{"peer_id":"p1"}"#);
        }
        save(&tmp, &src).unwrap();

        // Restore into a fresh doc.
        let dst = Doc::new();
        restore(&tmp, &dst).unwrap();
        // Resolve refs BEFORE opening the read txn (get_or_insert under a held
        // txn deadlocks).
        let chat = dst.get_or_insert_array(CHAT_KEY);
        let tasks = dst.get_or_insert_map(TASKS_KEY);
        let members = dst.get_or_insert_map(MEMBER_LIST_KEY);
        let txn = dst.transact();
        // Only the fresh message survived the 30-day window.
        assert_eq!(chat.len(&txn), 1);
        if let Some(Out::Any(Any::String(s))) = chat.get(&txn, 0) {
            assert!(s.contains("fresh"));
        } else {
            panic!("expected fresh message");
        }
        assert_eq!(tasks.len(&txn), 1);
        assert_eq!(members.len(&txn), 1);

        std::fs::remove_dir_all(&tmp).ok();
    }

    #[test]
    fn restore_does_not_double_seed_nonempty_chat() {
        let tmp = std::env::temp_dir().join(format!("enox-ctrl-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&tmp).unwrap();
        let now = chrono::Utc::now().timestamp();

        let src = Doc::new();
        {
            let chat = src.get_or_insert_array(CHAT_KEY);
            let mut txn = src.transact_mut();
            chat.push_back(&mut txn, Any::String(msg("m1", now).as_str().into()));
        }
        save(&tmp, &src).unwrap();

        // Destination already has chat (as if a peer synced it) — restore skips.
        let dst = Doc::new();
        {
            let chat = dst.get_or_insert_array(CHAT_KEY);
            let mut txn = dst.transact_mut();
            chat.push_back(&mut txn, Any::String(msg("existing", now).as_str().into()));
        }
        restore(&tmp, &dst).unwrap();
        let chat = dst.get_or_insert_array(CHAT_KEY);
        let txn = dst.transact();
        assert_eq!(chat.len(&txn), 1, "no double-seed");

        std::fs::remove_dir_all(&tmp).ok();
    }

    #[test]
    fn restore_missing_file_is_ok() {
        let tmp = std::env::temp_dir().join(format!("enox-ctrl-{}", uuid::Uuid::new_v4()));
        let dst = Doc::new();
        assert!(restore(&tmp, &dst).is_ok());
    }

    #[test]
    fn save_alone_writes_file() {
        let tmp = std::env::temp_dir().join(format!("enox-ctrl-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&tmp).unwrap();
        let now = chrono::Utc::now().timestamp();
        let src = Doc::new();
        {
            let chat = src.get_or_insert_array(CHAT_KEY);
            let mut txn = src.transact_mut();
            chat.push_back(&mut txn, Any::String(msg("a", now).as_str().into()));
        }
        save(&tmp, &src).unwrap();
        assert!(super::path(&tmp).exists());
        std::fs::remove_dir_all(&tmp).ok();
    }

    #[test]
    fn message_retention_keeps_the_latest_entries() {
        let mut chat = (0..CHAT_RETENTION_MESSAGES + 3)
            .map(|i| i.to_string())
            .collect::<Vec<_>>();

        retain_latest_messages(&mut chat);

        assert_eq!(chat.len(), CHAT_RETENTION_MESSAGES);
        assert_eq!(chat.first().map(String::as_str), Some("3"));
        let expected_last = (CHAT_RETENTION_MESSAGES + 2).to_string();
        assert_eq!(
            chat.last().map(String::as_str),
            Some(expected_last.as_str())
        );
    }

    #[test]
    fn message_deduplication_uses_stable_message_ids() {
        let mut chat = vec![
            r#"{"id":"one","text":"first"}"#.to_string(),
            r#"{"id":"two","text":"second"}"#.to_string(),
            r#"{"id":"one","text":"duplicate CRDT item"}"#.to_string(),
        ];

        deduplicate_messages(&mut chat);

        assert_eq!(chat.len(), 2);
        assert!(chat[0].contains("first"));
        assert!(chat[1].contains("second"));
    }
}
