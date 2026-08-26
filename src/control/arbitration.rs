use crate::control::{LockAction, LockEntry};
use std::collections::HashMap;
use yrs::{Any, Array, ArrayRef, Out, ReadTxn};

/// Replay the lock_log and return the current holder per path.
/// Deterministic: first unmatched acquire = holder.
pub fn compute_lock_state<T: ReadTxn>(lock_log: &ArrayRef, txn: &T) -> HashMap<String, String> {
    compute_lock_holders(lock_log, txn)
        .into_iter()
        .map(|(path, holder)| (path, holder.agent_id))
        .collect()
}

#[derive(Clone)]
struct LockHolder {
    agent_id: String,
    peer_id: String,
}

fn compute_lock_holders<T: ReadTxn>(lock_log: &ArrayRef, txn: &T) -> HashMap<String, LockHolder> {
    let mut holders: HashMap<String, LockHolder> = HashMap::new();

    for item in lock_log.iter(txn) {
        let json_str = match item {
            Out::Any(Any::String(s)) => s,
            _ => continue,
        };
        let entry: LockEntry = match serde_json::from_str(&json_str) {
            Ok(e) => e,
            Err(_) => continue,
        };
        match entry.action {
            LockAction::Acquire => {
                // First acquire without a release = lock holder
                holders.entry(entry.path).or_insert(LockHolder {
                    agent_id: entry.agent_id,
                    peer_id: entry.peer_id,
                });
            }
            LockAction::Release => {
                if holders
                    .get(&entry.path)
                    .map(|holder| same_actor(holder, &entry))
                    .unwrap_or(false)
                {
                    holders.remove(&entry.path);
                }
            }
        }
    }
    holders
}

/// True if `path` is locked by someone other than `agent_id`.
pub fn is_locked_by_other<T: ReadTxn>(
    lock_log: &ArrayRef,
    txn: &T,
    path: &str,
    agent_id: &str,
    peer_id: &str,
) -> bool {
    compute_lock_holders(lock_log, txn)
        .get(path)
        .map(|holder| {
            holder.agent_id != agent_id || (!holder.peer_id.is_empty() && holder.peer_id != peer_id)
        })
        .unwrap_or(false)
}

fn same_actor(holder: &LockHolder, entry: &LockEntry) -> bool {
    holder.agent_id == entry.agent_id
        && (holder.peer_id.is_empty()
            || entry.peer_id.is_empty()
            || holder.peer_id == entry.peer_id)
}

/// Append an acquire or release entry.
pub fn append_lock_entry(
    lock_log: &ArrayRef,
    txn: &mut yrs::TransactionMut,
    entry: &LockEntry,
) -> anyhow::Result<()> {
    let json = serde_json::to_string(entry)?;
    lock_log.push_back(txn, Any::String(json.as_str().into()));
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::control::{LockAction, LockEntry};
    use chrono::Utc;
    use yrs::{Doc, Transact};

    fn entry(agent_id: &str, peer_id: &str, action: LockAction) -> LockEntry {
        LockEntry {
            entry_id: uuid::Uuid::new_v4().to_string(),
            agent_id: agent_id.to_string(),
            peer_id: peer_id.to_string(),
            path: "src/shared.rs".to_string(),
            action,
            ts: Utc::now(),
        }
    }

    #[test]
    fn same_label_on_another_device_cannot_release_lock() {
        let doc = Doc::new();
        let lock_log = doc.get_or_insert_array("locks");
        let mut txn = doc.transact_mut();
        append_lock_entry(
            &lock_log,
            &mut txn,
            &entry("codex", "peer-a", LockAction::Acquire),
        )
        .unwrap();
        append_lock_entry(
            &lock_log,
            &mut txn,
            &entry("codex", "peer-b", LockAction::Release),
        )
        .unwrap();
        drop(txn);

        let txn = doc.transact();
        assert_eq!(
            compute_lock_state(&lock_log, &txn).get("src/shared.rs"),
            Some(&"codex".to_string())
        );
        assert!(is_locked_by_other(
            &lock_log,
            &txn,
            "src/shared.rs",
            "codex",
            "peer-b"
        ));
        assert!(!is_locked_by_other(
            &lock_log,
            &txn,
            "src/shared.rs",
            "codex",
            "peer-a"
        ));
    }
}
