//! Durable record of which chat mentions have already triggered an agent.
//!
//! The reaction loop must act on a given mention **at most once, ever** — not
//! just once per daemon run. On reconnect, P2P sync replays the entire chat
//! history as fresh CRDT updates, so without a durable guard every past mention
//! re-launches its agent on every restart. An in-memory set resets on restart
//! and cannot prevent that; this persists across restarts.
//!
//! Keyed by `(message_id, mention)`, stored one-per-line under the circle dir.
//! Message ids are UUIDs, so the set grows by the number of *distinct* mentions
//! ever acted on — small and bounded in practice.

use std::collections::HashSet;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

fn path(circle_dir: &Path) -> PathBuf {
    circle_dir.join("handled_mentions.log")
}

/// A persistent set of handled `(message_id, mention)` keys for one circle.
/// Load once at reaction-loop start; `mark_new` returns whether a key is newly
/// seen (and records it, appending to disk).
pub struct HandledMentions {
    file: PathBuf,
    seen: Mutex<HashSet<String>>,
}

impl HandledMentions {
    /// Load the persisted set (empty if none yet).
    pub fn load(circle_dir: &Path) -> Self {
        let file = path(circle_dir);
        let seen = std::fs::read_to_string(&file)
            .map(|text| text.lines().map(str::to_string).collect())
            .unwrap_or_default();
        Self {
            file,
            seen: Mutex::new(seen),
        }
    }

    fn key(message_id: &str, mention: &str) -> String {
        format!("{message_id}::{mention}")
    }

    /// Record `(message_id, mention)` as handled. Returns `true` if it was newly
    /// added (caller should act), `false` if already handled (caller must skip).
    /// Appends to disk on first sight so the record survives a restart.
    pub fn mark_new(&self, message_id: &str, mention: &str) -> bool {
        let key = Self::key(message_id, mention);
        let mut seen = self.seen.lock().unwrap();
        if !seen.insert(key.clone()) {
            return false;
        }
        // Append durably. A failed write only risks a possible re-trigger of
        // this one mention on a future restart — non-fatal, so we log and go on.
        if let Some(parent) = self.file.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        match std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.file)
        {
            Ok(mut f) => {
                let _ = writeln!(f, "{key}");
            }
            Err(e) => tracing::warn!("[agent] could not persist handled mention: {e}"),
        }
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dedup_survives_reload() {
        let tmp = std::env::temp_dir().join(format!("enox-handled-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&tmp).unwrap();

        let h = HandledMentions::load(&tmp);
        assert!(h.mark_new("msg-1", "claude"), "first sight acts");
        assert!(!h.mark_new("msg-1", "claude"), "second sight skips");
        // A different mention in the same message is distinct.
        assert!(h.mark_new("msg-1", "codex"));

        // Reload from disk (simulating a restart) — still remembers.
        let h2 = HandledMentions::load(&tmp);
        assert!(!h2.mark_new("msg-1", "claude"), "persisted across reload");
        assert!(!h2.mark_new("msg-1", "codex"));
        assert!(h2.mark_new("msg-2", "claude"), "a new message is new");

        std::fs::remove_dir_all(&tmp).ok();
    }
}
