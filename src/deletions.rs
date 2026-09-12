//! Durable path deletions.
//!
//! A deletion used to exist only as a `\0delete/` frame on the live sync
//! stream. That made it unrecoverable: a peer disconnected at that moment never
//! learned, the `all_deletes` broadcast silently dropped the overflow of a bulk
//! delete, and — worst — the forgetful peer still advertised the file in the
//! next handshake, so it was re-created on the device that deleted it. Absence
//! from a doc set is indistinguishable from "hasn't synced yet".
//!
//! Deletions therefore live in the control doc as a CRDT map, replicating and
//! reconciling like any other state. The live frame is kept as a latency
//! optimisation, not as the source of truth.
//!
//! Tombstones here are **supersedable**, unlike the member-removal tombstone:
//! re-creating a path clears its entry, because files are routinely deleted and
//! re-created under the same name.

use crate::control::{Deletion, DELETIONS_KEY};
use crate::state::AppState;
use yrs::{Any, Map, MapRef, Out, ReadTxn, Transact, WriteTxn};

/// How long a tombstone is retained.
///
/// It only has to outlive the longest realistic peer absence: once every peer
/// has applied it, the entry is dead weight in a fully-replicated document. A
/// peer offline for longer than this re-uploads the file, which is a visible
/// and recoverable outcome — unlike an unbounded map that every device carries
/// forever.
pub const RETENTION_MS: i64 = 90 * 24 * 60 * 60 * 1000;

pub fn now_ms() -> i64 {
    chrono::Utc::now().timestamp_millis()
}

fn map(txn: &impl ReadTxn) -> Option<MapRef> {
    txn.get_map(DELETIONS_KEY)
}

/// Record `path` as deleted. Returns false if the control doc was busy — the
/// caller keeps the live broadcast, so a miss here degrades to the old
/// behaviour rather than failing the delete.
pub fn record(state: &AppState, path: &str) -> bool {
    let entry = Deletion {
        path: path.to_string(),
        ts: now_ms(),
        peer_id: state.peer_id.clone(),
    };
    let Ok(json) = serde_json::to_string(&entry) else {
        return false;
    };
    let Ok(mut txn) = state.control.try_transact_mut() else {
        return false;
    };
    let m = txn.get_or_insert_map(DELETIONS_KEY);
    m.insert(&mut txn, path.to_string(), Any::String(json.into()));
    true
}

/// Drop the tombstone for `path`, because the path exists again.
///
/// Called when the watcher sees a create or write. Without this a re-created
/// file would be deleted again on the next reconcile.
pub fn clear(state: &AppState, path: &str) -> bool {
    let Ok(mut txn) = state.control.try_transact_mut() else {
        return false;
    };
    let m = txn.get_or_insert_map(DELETIONS_KEY);
    m.remove(&mut txn, path);
    true
}

/// The tombstone for `path`, if any.
pub fn get(state: &AppState, path: &str) -> Option<Deletion> {
    let txn = state.control.try_transact().ok()?;
    let m = map(&txn)?;
    match m.get(&txn, path) {
        Some(Out::Any(Any::String(s))) => serde_json::from_str(&s).ok(),
        _ => None,
    }
}

/// Whether `path` is currently tombstoned.
pub fn is_deleted(state: &AppState, path: &str) -> bool {
    get(state, path).is_some()
}

/// Every live tombstone.
pub fn all(state: &AppState) -> Vec<Deletion> {
    let Ok(txn) = state.control.try_transact() else {
        return Vec::new();
    };
    let Some(m) = map(&txn) else {
        return Vec::new();
    };
    m.iter(&txn)
        .filter_map(|(_, v)| match v {
            Out::Any(Any::String(s)) => serde_json::from_str::<Deletion>(&s).ok(),
            _ => None,
        })
        .collect()
}

/// Drop tombstones past [`RETENTION_MS`], so the fully-replicated control doc
/// does not grow without bound.
pub fn prune(state: &AppState) -> usize {
    let cutoff = now_ms() - RETENTION_MS;
    let stale: Vec<String> = all(state)
        .into_iter()
        .filter(|d| d.ts < cutoff)
        .map(|d| d.path)
        .collect();
    if stale.is_empty() {
        return 0;
    }
    let Ok(mut txn) = state.control.try_transact_mut() else {
        return 0;
    };
    let m = txn.get_or_insert_map(DELETIONS_KEY);
    for path in &stale {
        m.remove(&mut txn, path.as_str());
    }
    stale.len()
}

/// Apply every tombstone to local state: forget the doc and remove the file.
///
/// Runs at startup and whenever a sync stream is established, which is what
/// closes the hole the live-frame-only design left. Idempotent.
pub async fn reconcile(state: &AppState) -> usize {
    let mut applied = 0;
    for deletion in all(state) {
        if apply(state, &deletion).await {
            applied += 1;
        }
    }
    applied
}

/// Apply one tombstone. Returns true if it changed anything locally.
///
/// A file modified after the tombstone was written is left alone: that is a
/// re-creation the deleting peer has not seen yet, and destroying it would lose
/// work. The watcher clears the tombstone when it observes that write.
pub async fn apply(state: &AppState, deletion: &Deletion) -> bool {
    let full = state
        .workspace
        .join(deletion.path.replace('/', std::path::MAIN_SEPARATOR_STR));

    if let Ok(meta) = tokio::fs::metadata(&full).await {
        if let Ok(modified) = meta.modified() {
            let modified_ms = modified
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_millis() as i64)
                .unwrap_or(0);
            if modified_ms > deletion.ts {
                // Re-created after the delete — keep it and retire the tombstone.
                clear(state, &deletion.path);
                return false;
            }
        }
    }

    let had_doc = state.docs.contains_key(&deletion.path);
    state.remove_doc(&deletion.path);
    crate::store::crdt::delete(&state.workspace, &deletion.path).await;
    let existed = tokio::fs::remove_file(&full).await.is_ok();

    if existed {
        prune_empty_parents(&state.workspace, &full).await;
    }
    if had_doc || existed {
        let _ = state.events.send(crate::control::CircleEvent::FileDeleted {
            path: deletion.path.clone(),
        });
        return true;
    }
    false
}

/// Remove directories left empty by a deletion, stopping at the workspace root.
///
/// Deleting a folder arrives as one event per contained file, so without this
/// the emptied directory tree survives on every peer but the one that deleted
/// it. `remove_dir` only succeeds on an empty directory, so this can never
/// remove a directory that still holds anything.
pub(crate) async fn prune_empty_parents(workspace: &std::path::Path, from: &std::path::Path) {
    let mut dir = match from.parent() {
        Some(d) => d.to_path_buf(),
        None => return,
    };
    while dir.starts_with(workspace) && dir != workspace {
        if tokio::fs::remove_dir(&dir).await.is_err() {
            return; // not empty, or gone already
        }
        match dir.parent() {
            Some(parent) => dir = parent.to_path_buf(),
            None => return,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{config::JoinPolicy, mls};
    use std::path::PathBuf;

    fn test_state(workspace: PathBuf) -> AppState {
        AppState::new(
            "circle".into(),
            "Circle".into(),
            workspace,
            PathBuf::new(),
            String::new(),
            "agent".into(),
            1,
            "peer-local".into(),
            JoinPolicy::Manual,
            "owner".into(),
            mls::new_mls_state(mls::MlsIdentity::generate("peer-local").unwrap(), None),
        )
    }

    #[test]
    fn record_and_clear_round_trip() {
        let state = test_state(PathBuf::new());
        assert!(!is_deleted(&state, "a.txt"));
        assert!(record(&state, "a.txt"));
        assert!(is_deleted(&state, "a.txt"));

        // Re-creating the path must retire the tombstone, or the file would be
        // deleted again on the next reconcile.
        assert!(clear(&state, "a.txt"));
        assert!(!is_deleted(&state, "a.txt"));
    }

    #[tokio::test]
    async fn apply_removes_the_file_and_its_empty_parents() {
        let dir = tempfile::tempdir().unwrap();
        let state = test_state(dir.path().to_path_buf());
        let nested = dir.path().join("repo").join("src");
        std::fs::create_dir_all(&nested).unwrap();
        std::fs::write(nested.join("main.rs"), "fn main() {}").unwrap();

        record(&state, "repo/src/main.rs");
        let deletion = get(&state, "repo/src/main.rs").unwrap();
        assert!(apply(&state, &deletion).await);

        assert!(!nested.join("main.rs").exists());
        // Deleting a folder arrives per-file, so the emptied tree must go too.
        assert!(!nested.exists(), "empty src/ should be pruned");
        assert!(
            !dir.path().join("repo").exists(),
            "empty repo/ should be pruned"
        );
        assert!(dir.path().exists(), "workspace root must never be removed");
    }

    #[tokio::test]
    async fn a_file_recreated_after_the_delete_survives() {
        let dir = tempfile::tempdir().unwrap();
        let state = test_state(dir.path().to_path_buf());
        std::fs::write(dir.path().join("a.txt"), "original").unwrap();

        // Tombstone dated before the file was written.
        let stale = Deletion {
            path: "a.txt".into(),
            ts: now_ms() - 60_000,
            peer_id: "other".into(),
        };
        assert!(!apply(&state, &stale).await);
        assert!(
            dir.path().join("a.txt").exists(),
            "a newer file must not be destroyed by an older tombstone"
        );
    }

    #[tokio::test]
    async fn apply_is_idempotent_and_safe_when_nothing_exists() {
        let dir = tempfile::tempdir().unwrap();
        let state = test_state(dir.path().to_path_buf());
        let deletion = Deletion {
            path: "never/existed.txt".into(),
            ts: now_ms(),
            peer_id: "other".into(),
        };
        assert!(!apply(&state, &deletion).await);
        assert!(!apply(&state, &deletion).await);
    }

    #[test]
    fn prune_drops_only_expired_tombstones() {
        let state = test_state(PathBuf::new());
        record(&state, "fresh.txt");
        {
            // Backdate one past the retention window.
            let old = Deletion {
                path: "old.txt".into(),
                ts: now_ms() - RETENTION_MS - 1,
                peer_id: "p".into(),
            };
            let json = serde_json::to_string(&old).unwrap();
            let mut txn = state.control.try_transact_mut().unwrap();
            let m = txn.get_or_insert_map(DELETIONS_KEY);
            m.insert(&mut txn, "old.txt".to_string(), Any::String(json.into()));
        }
        assert_eq!(prune(&state), 1);
        assert!(is_deleted(&state, "fresh.txt"));
        assert!(!is_deleted(&state, "old.txt"));
    }
}
