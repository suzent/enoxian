use std::collections::HashMap;
use yrs::{Array, ArrayRef, ReadTxn, Any, Out};
use crate::control::{LockAction, LockEntry};

/// Replay the lock_log and return the current holder per path.
/// Deterministic: first unmatched acquire = holder.
pub fn compute_lock_state<T: ReadTxn>(
    lock_log: &ArrayRef,
    txn: &T,
) -> HashMap<String, String> {
    let mut holders: HashMap<String, String> = HashMap::new();

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
                holders.entry(entry.path).or_insert(entry.agent_id);
            }
            LockAction::Release => {
                if holders
                    .get(&entry.path)
                    .map(|h| h == &entry.agent_id)
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
) -> bool {
    compute_lock_state(lock_log, txn)
        .get(path)
        .map(|h| h.as_str() != agent_id)
        .unwrap_or(false)
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
