use crate::control::{LockAction, LockEntry};
use chrono::{DateTime, Utc};
use std::collections::HashMap;
use yrs::{Any, Array, ArrayRef, Out, ReadTxn};

/// Compact the lock log once it grows past this many entries.
///
/// Every lock operation replays the whole log (see [`compute_lock_holders`]),
/// and the log is persisted verbatim, so an unbounded log is quadratic work and
/// an unbounded file. A run of agents cycling locks can append hundreds of
/// thousands of entries in hours.
const COMPACT_THRESHOLD: u32 = 2_000;

/// How long a lock lasts when the binder does not say.
pub const DEFAULT_LOCK_SECS: i64 = 600;

/// The longest a lock may be held before it has to be renewed. A lock is
/// advisory, so its only failure mode is outliving the work it announced; a
/// bound on the lease is what stops a crashed binder holding a path forever.
pub const MAX_LOCK_SECS: i64 = 3600;

/// Replay the lock_log and return the current holder per path.
pub fn compute_lock_state<T: ReadTxn>(lock_log: &ArrayRef, txn: &T) -> HashMap<String, String> {
    compute_lock_holders(lock_log, txn)
        .into_iter()
        .map(|(path, holder)| (path, holder.agent_id))
        .collect()
}

#[derive(Clone, Debug)]
pub(crate) struct LockHolder {
    pub(crate) run_id: Option<String>,
    pub(crate) agent_id: String,
    pub(crate) peer_id: String,
    pub(crate) expires_at: DateTime<Utc>,
    pub(crate) taken_over_from: Option<String>,
    pub(crate) taken_over_from_peer_id: Option<String>,
}

impl LockHolder {
    fn from_entry(entry: &LockEntry) -> Self {
        Self {
            run_id: entry.run_id.clone(),
            agent_id: entry.agent_id.clone(),
            peer_id: entry.peer_id.clone(),
            expires_at: lease_end(entry),
            taken_over_from: entry.taken_over_from.clone(),
            taken_over_from_peer_id: entry.taken_over_from_peer_id.clone(),
        }
    }
}

fn lease_end(entry: &LockEntry) -> DateTime<Utc> {
    entry
        .expires_at
        .unwrap_or(entry.ts + chrono::Duration::seconds(DEFAULT_LOCK_SECS))
}

/// Holders whose lease is still running at `now`.
pub(crate) fn compute_lock_holders<T: ReadTxn>(
    lock_log: &ArrayRef,
    txn: &T,
) -> HashMap<String, LockHolder> {
    compute_lock_holders_at(lock_log, txn, Utc::now())
}

pub(crate) fn compute_lock_holders_at<T: ReadTxn>(
    lock_log: &ArrayRef,
    txn: &T,
    now: DateTime<Utc>,
) -> HashMap<String, LockHolder> {
    let entries = parse_all(lock_log.iter(txn).filter_map(|item| match item {
        Out::Any(Any::String(s)) => Some(s.to_string()),
        _ => None,
    }));
    replay(&entries)
        .into_iter()
        .filter(|(_, (_, holder))| holder.expires_at > now)
        .map(|(path, (_, holder))| (path, holder))
        .collect()
}

/// Fold the log into the holder of each path, with the index of the entry
/// that put the holder there. The one source of truth for both lookups and
/// compaction, so the two can never disagree.
///
/// Expiry is judged against each entry's own timestamp, never the replaying
/// device's clock, so every peer folds the same log to the same holders:
/// - an acquire on a free path, or on one whose lease ended before it, takes it;
/// - an acquire by the holder renews its lease;
/// - a takeover replaces a live holder; any other acquire yields to one;
/// - a release by the holder frees the path.
fn replay(entries: &[Option<LockEntry>]) -> HashMap<String, (usize, LockHolder)> {
    let mut holders: HashMap<String, (usize, LockHolder)> = HashMap::new();
    for (index, entry) in entries.iter().enumerate() {
        let Some(entry) = entry else { continue };
        match entry.action {
            LockAction::Acquire => {
                let takes = match holders.get(&entry.path) {
                    None => true,
                    Some((_, holder)) => {
                        entry.ts >= holder.expires_at || entry.takeover || same_actor(holder, entry)
                    }
                };
                if takes {
                    holders.insert(entry.path.clone(), (index, LockHolder::from_entry(entry)));
                }
            }
            LockAction::Release => {
                if holders
                    .get(&entry.path)
                    .map(|(_, holder)| same_actor(holder, entry))
                    .unwrap_or(false)
                {
                    holders.remove(&entry.path);
                }
            }
        }
    }
    holders
}

/// The live holder of `path` when it is bound from a device other than
/// `editor_peer` — an edit from `editor_peer` then went against the lock.
///
/// Only devices can be told apart: several agents on one device all edit as
/// that device, so an edit on the holder's own device is never reported.
/// Legacy holders without a device are skipped for the same reason. A busy
/// control doc skips the check rather than stalling the edit it describes.
pub(crate) fn holder_elsewhere(
    control: &yrs::Doc,
    path: &str,
    editor_peer: &str,
) -> Option<LockHolder> {
    use yrs::Transact;
    let txn = control.try_transact().ok()?;
    let log = txn.get_array(crate::control::LOCK_LOG_KEY)?;
    compute_lock_holders(&log, &txn)
        .remove(path)
        .filter(|holder| !holder.peer_id.is_empty() && holder.peer_id != editor_peer)
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

pub fn is_locked_by_other_run<T: ReadTxn>(
    log: &ArrayRef,
    txn: &T,
    path: &str,
    agent: &str,
    peer: &str,
    run: Option<&str>,
) -> bool {
    compute_lock_holders(log, txn).get(path).is_some_and(|h| {
        h.agent_id != agent
            || (!h.peer_id.is_empty() && h.peer_id != peer)
            || h.run_id.as_deref() != run
    })
}

fn same_actor(holder: &LockHolder, entry: &LockEntry) -> bool {
    holder.run_id == entry.run_id
        && holder.agent_id == entry.agent_id
        && (holder.peer_id.is_empty()
            || entry.peer_id.is_empty()
            || holder.peer_id == entry.peer_id)
}

/// Indices of the entries that put each current holder in place.
///
/// Every acquire that takes a path carries the full holder in itself, so
/// replaying just these entries rebuilds the same holders — the property that
/// lets compaction drop everything else. Expired holders are kept: whether a
/// later entry sees them as expired depends on that entry's timestamp, and
/// dropping them would change the answer for an entry from a skewed clock.
fn held_indices(entries: &[Option<LockEntry>]) -> Vec<usize> {
    replay(entries)
        .into_values()
        .map(|(index, _)| index)
        .collect()
}

/// Which entries a compaction keeps: the unmatched acquires, plus anything that
/// failed to parse — unreadable entries are kept rather than lost, the same way
/// the chat snapshot keeps unparseable messages, and replay skips them anyway.
///
/// Deliberately no "recent history" tail. An entry's meaning depends on the
/// entries before it, so keeping a raw suffix can change what the log replays
/// to. Given `A acquire p`, `B acquire p`, `A release p`, the full log leaves
/// `p` free: B's acquire is ignored while A holds it, and A's release then
/// clears it. Keep only the last two and replay makes B the holder — a lock
/// nobody took and nobody can release. Retaining just the unmatched acquires
/// has no such dependency: one entry per path, no releases to interact.
fn retained(entries: &[Option<LockEntry>]) -> Vec<bool> {
    let mut keep: Vec<bool> = entries.iter().map(Option::is_none).collect();
    for index in held_indices(entries) {
        keep[index] = true;
    }
    keep
}

fn parse_all(raw: impl Iterator<Item = String>) -> Vec<Option<LockEntry>> {
    raw.map(|json| serde_json::from_str(&json).ok()).collect()
}

/// Drop lock entries that no longer affect the derived state.
///
/// A matched acquire/release pair leaves `compute_lock_holders` in exactly the
/// place it started, so removing the pair changes nothing a caller can observe.
/// Runs inside the caller's transaction, so the log is never briefly wrong.
/// Returns the number of entries removed.
pub fn compact_lock_log(lock_log: &ArrayRef, txn: &mut yrs::TransactionMut) -> u32 {
    if lock_log.len(txn) <= COMPACT_THRESHOLD {
        return 0;
    }
    let entries = parse_all(lock_log.iter(&*txn).filter_map(|item| match item {
        Out::Any(Any::String(s)) => Some(s.to_string()),
        _ => None,
    }));
    if entries.len() != lock_log.len(txn) as usize {
        // A non-string item means this array is not shaped the way we think.
        return 0;
    }
    let keep = retained(&entries);

    // Remove from the end so earlier indices stay valid, in contiguous runs so
    // one CRDT delete covers each stretch of dead entries.
    let mut removed = 0;
    let mut end = keep.len();
    while end > 0 {
        if keep[end - 1] {
            end -= 1;
            continue;
        }
        let mut start = end;
        while start > 0 && !keep[start - 1] {
            start -= 1;
        }
        lock_log.remove_range(txn, start as u32, (end - start) as u32);
        removed += (end - start) as u32;
        end = start;
    }
    removed
}

/// The persisted-snapshot form of [`compact_lock_log`], for a log loaded from
/// disk that was written before compaction existed.
pub fn compact_persisted_lock_log(entries: &mut Vec<String>) {
    if entries.len() <= COMPACT_THRESHOLD as usize {
        return;
    }
    let keep = retained(&parse_all(entries.iter().cloned()));
    let mut index = 0;
    entries.retain(|_| {
        index += 1;
        keep[index - 1]
    });
}

/// Append an acquire or release entry.
pub fn append_lock_entry(
    lock_log: &ArrayRef,
    txn: &mut yrs::TransactionMut,
    entry: &LockEntry,
) -> anyhow::Result<()> {
    let json = serde_json::to_string(entry)?;
    lock_log.push_back(txn, Any::String(json.as_str().into()));
    // Bound the log at its only two append sites (bind and release), so neither
    // the replay cost nor the snapshot can run away.
    let removed = compact_lock_log(lock_log, txn);
    if removed > 0 {
        tracing::debug!("[locks] compacted lock log, dropped {removed} settled entries");
    }
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
            run_id: None,
            entry_id: uuid::Uuid::new_v4().to_string(),
            agent_id: agent_id.to_string(),
            peer_id: peer_id.to_string(),
            path: "src/shared.rs".to_string(),
            action,
            ts: Utc::now(),
            expires_at: None,
            takeover: false,
            taken_over_from: None,
            taken_over_from_peer_id: None,
        }
    }

    fn entry_for(path: &str, agent: &str, action: LockAction) -> LockEntry {
        // A counter rather than a uuid: these helpers build thousands of
        // entries, and the id only has to be distinct.
        use std::sync::atomic::{AtomicU64, Ordering};
        static NEXT: AtomicU64 = AtomicU64::new(0);
        LockEntry {
            run_id: None,
            entry_id: NEXT.fetch_add(1, Ordering::Relaxed).to_string(),
            agent_id: agent.to_string(),
            peer_id: "peer".to_string(),
            path: path.to_string(),
            action,
            ts: Utc::now(),
            expires_at: None,
            takeover: false,
            taken_over_from: None,
            taken_over_from_peer_id: None,
        }
    }

    /// Churn well past the compaction threshold, holding one lock throughout.
    fn churned_log() -> (Doc, ArrayRef) {
        let doc = Doc::new();
        let log = doc.get_or_insert_array("locks");
        let mut txn = doc.transact_mut();
        append_lock_entry(
            &log,
            &mut txn,
            &entry_for("held.rs", "a", LockAction::Acquire),
        )
        .unwrap();
        // Just past the threshold: enough to trigger compaction once, and no
        // more work than that, so the suite stays cheap.
        for i in 0..COMPACT_THRESHOLD / 2 + 50 {
            let path = format!("churn-{}.rs", i % 40);
            append_lock_entry(&log, &mut txn, &entry_for(&path, "b", LockAction::Acquire)).unwrap();
            append_lock_entry(&log, &mut txn, &entry_for(&path, "b", LockAction::Release)).unwrap();
        }
        drop(txn);
        (doc, log)
    }

    #[test]
    fn compaction_cannot_invent_a_holder_from_a_contended_path() {
        // Two peers' acquires can merge into one log. Replayed whole, B's
        // acquire is ignored while A holds the path and A's release frees it,
        // so the path ends free.
        //
        // The entries are laid out to straddle where a "keep the recent tail"
        // rule would cut: A's acquire early, B's acquire and A's release at the
        // end. Such a rule drops A's acquire, keeps the other two, and replay
        // then reports B as holding a path nobody took and nobody can release.
        let json = |e: &LockEntry| serde_json::to_string(e).unwrap();
        let mut log = vec![json(&entry_for("contended.rs", "a", LockAction::Acquire))];
        for i in 0..COMPACT_THRESHOLD + 500 {
            let path = format!("churn-{}.rs", i % 40);
            log.push(json(&entry_for(&path, "c", LockAction::Acquire)));
            log.push(json(&entry_for(&path, "c", LockAction::Release)));
        }
        log.push(json(&entry_for("contended.rs", "b", LockAction::Acquire)));
        log.push(json(&entry_for("contended.rs", "a", LockAction::Release)));

        let holders_of = |entries: &[String]| {
            let doc = Doc::new();
            let array = doc.get_or_insert_array("locks");
            let mut txn = doc.transact_mut();
            for raw in entries {
                array.push_back(&mut txn, Any::String(raw.as_str().into()));
            }
            compute_lock_state(&array, &txn)
        };
        assert!(
            holders_of(&log).is_empty(),
            "precondition: the full log leaves the path free"
        );

        compact_persisted_lock_log(&mut log);
        assert_eq!(
            holders_of(&log).get("contended.rs"),
            None,
            "compaction invented a holder for a path that was free"
        );
    }

    #[test]
    fn compaction_bounds_a_log_that_only_ever_grew() {
        let (doc, log) = churned_log();
        let txn = doc.transact();
        assert!(
            log.len(&txn) <= COMPACT_THRESHOLD,
            "log stayed at {} entries",
            log.len(&txn)
        );
    }

    #[test]
    fn compaction_never_drops_a_lock_someone_still_holds() {
        // The property that makes discarding settled entries safe: the derived
        // holder set is what callers see, and it must survive untouched.
        let (doc, log) = churned_log();
        let txn = doc.transact();
        let holders = compute_lock_state(&log, &txn);
        assert_eq!(holders.get("held.rs"), Some(&"a".to_string()));
        assert_eq!(holders.len(), 1, "settled churn left a phantom holder");
    }

    #[test]
    fn a_release_after_compaction_still_frees_the_path() {
        let (doc, log) = churned_log();
        let mut txn = doc.transact_mut();
        append_lock_entry(
            &log,
            &mut txn,
            &entry_for("held.rs", "a", LockAction::Release),
        )
        .unwrap();
        assert!(compute_lock_state(&log, &txn).is_empty());
    }

    #[test]
    fn a_persisted_log_compacts_to_the_same_holders() {
        let (doc, log) = churned_log();
        let txn = doc.transact();
        let mut persisted: Vec<String> = log
            .iter(&txn)
            .filter_map(|v| match v {
                Out::Any(Any::String(s)) => Some(s.to_string()),
                _ => None,
            })
            .collect();
        // Pad past the threshold so the snapshot path actually engages.
        let settled = entry_for("gone.rs", "c", LockAction::Acquire);
        let mut released = settled.clone();
        released.action = LockAction::Release;
        for _ in 0..COMPACT_THRESHOLD / 2 + 50 {
            persisted.push(serde_json::to_string(&settled).unwrap());
            persisted.push(serde_json::to_string(&released).unwrap());
        }
        let before = persisted.len();
        compact_persisted_lock_log(&mut persisted);
        assert!(persisted.len() < before);

        let replay = Doc::new();
        let replayed = replay.get_or_insert_array("locks");
        let mut txn = replay.transact_mut();
        for raw in &persisted {
            replayed.push_back(&mut txn, Any::String(raw.as_str().into()));
        }
        assert_eq!(
            compute_lock_state(&replayed, &txn).get("held.rs"),
            Some(&"a".to_string())
        );
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
    #[test]
    fn an_old_run_cannot_release_the_same_agents_new_lock() {
        let doc = Doc::new();
        let log = doc.get_or_insert_array("locks");
        let mut txn = doc.transact_mut();
        let mut acquire = entry("a", "peer", LockAction::Acquire);
        acquire.run_id = Some("new".into());
        append_lock_entry(&log, &mut txn, &acquire).unwrap();
        let mut release = entry("a", "peer", LockAction::Release);
        release.run_id = Some("old".into());
        append_lock_entry(&log, &mut txn, &release).unwrap();
        assert!(is_locked_by_other_run(
            &log,
            &txn,
            "src/shared.rs",
            "a",
            "peer",
            Some("old")
        ));
        assert!(!is_locked_by_other_run(
            &log,
            &txn,
            "src/shared.rs",
            "a",
            "peer",
            Some("new")
        ));
    }

    use chrono::Duration;

    /// An acquire of `src/shared.rs` stamped `at`, leased for `secs`.
    fn lease(agent: &str, peer: &str, at: DateTime<Utc>, secs: i64) -> LockEntry {
        let mut e = entry(agent, peer, LockAction::Acquire);
        e.ts = at;
        e.expires_at = Some(at + Duration::seconds(secs));
        e
    }

    fn log_of(entries: &[LockEntry]) -> (Doc, ArrayRef) {
        let doc = Doc::new();
        let log = doc.get_or_insert_array("locks");
        {
            let mut txn = doc.transact_mut();
            for e in entries {
                append_lock_entry(&log, &mut txn, e).unwrap();
            }
        }
        (doc, log)
    }

    fn holder_at(entries: &[LockEntry], now: DateTime<Utc>) -> Option<LockHolder> {
        let (doc, log) = log_of(entries);
        let txn = doc.transact();
        compute_lock_holders_at(&log, &txn, now).remove("src/shared.rs")
    }

    #[test]
    fn a_lock_frees_itself_when_its_lease_ends() {
        let t = Utc::now();
        let entries = [lease("a", "peer-a", t, 60)];
        assert!(holder_at(&entries, t + Duration::seconds(59)).is_some());
        assert!(holder_at(&entries, t + Duration::seconds(61)).is_none());
    }

    #[test]
    fn a_legacy_acquire_lasts_the_default_lease() {
        let t = Utc::now();
        let mut legacy = entry("a", "peer-a", LockAction::Acquire);
        legacy.ts = t;
        let before_end = t + Duration::seconds(DEFAULT_LOCK_SECS - 1);
        assert!(holder_at(std::slice::from_ref(&legacy), before_end).is_some());
        let after_end = t + Duration::seconds(DEFAULT_LOCK_SECS + 1);
        assert!(holder_at(&[legacy], after_end).is_none());
    }

    #[test]
    fn binding_again_renews_the_holders_lease() {
        let t = Utc::now();
        let entries = [
            lease("a", "peer-a", t, 60),
            lease("a", "peer-a", t + Duration::seconds(30), 600),
        ];
        let holder = holder_at(&entries, t + Duration::seconds(120)).unwrap();
        assert_eq!(holder.agent_id, "a");
        assert_eq!(holder.expires_at, t + Duration::seconds(630));
    }

    #[test]
    fn an_acquire_yields_to_a_live_holder() {
        let t = Utc::now();
        let entries = [
            lease("a", "peer-a", t, 600),
            lease("b", "peer-b", t + Duration::seconds(10), 600),
        ];
        let holder = holder_at(&entries, t + Duration::seconds(20)).unwrap();
        assert_eq!(holder.agent_id, "a");
    }

    #[test]
    fn an_acquire_after_the_lease_ended_takes_the_path() {
        let t = Utc::now();
        let entries = [
            lease("a", "peer-a", t, 60),
            lease("b", "peer-b", t + Duration::seconds(90), 600),
        ];
        let holder = holder_at(&entries, t + Duration::seconds(100)).unwrap();
        assert_eq!(holder.agent_id, "b");
    }

    #[test]
    fn expiry_is_judged_by_each_entrys_time_not_the_replayers_clock() {
        // B's acquire landed while A was live, so it yielded — replaying the
        // log after A's lease ended must not retroactively hand B the lock.
        let t = Utc::now();
        let entries = [
            lease("a", "peer-a", t, 60),
            lease("b", "peer-b", t + Duration::seconds(10), 600),
        ];
        assert!(holder_at(&entries, t + Duration::seconds(120)).is_none());
    }

    #[test]
    fn a_takeover_replaces_a_live_holder_and_says_whom_it_replaced() {
        let t = Utc::now();
        let mut takeover = lease("b", "peer-b", t + Duration::seconds(10), 600);
        takeover.takeover = true;
        takeover.taken_over_from = Some("a".into());
        takeover.taken_over_from_peer_id = Some("peer-a".into());
        let entries = [lease("a", "peer-a", t, 600), takeover];

        let holder = holder_at(&entries, t + Duration::seconds(20)).unwrap();
        assert_eq!(holder.agent_id, "b");
        assert_eq!(holder.taken_over_from.as_deref(), Some("a"));
        assert_eq!(holder.taken_over_from_peer_id.as_deref(), Some("peer-a"));

        // The displaced holder's release no longer frees the path.
        let mut release = entry("a", "peer-a", LockAction::Release);
        release.ts = t + Duration::seconds(30);
        let mut with_release = entries.to_vec();
        with_release.push(release);
        let holder = holder_at(&with_release, t + Duration::seconds(40)).unwrap();
        assert_eq!(holder.agent_id, "b");
    }

    #[test]
    fn compaction_keeps_a_renewed_and_taken_over_lease_as_it_was() {
        let t = Utc::now();
        let json = |e: &LockEntry| serde_json::to_string(e).unwrap();
        let mut takeover = lease("b", "peer-b", t + Duration::seconds(10), 600);
        takeover.takeover = true;
        takeover.taken_over_from = Some("a".into());
        let mut renewal = lease("b", "peer-b", t + Duration::seconds(20), 1200);
        renewal.taken_over_from = Some("a".into());
        let mut log = vec![json(&lease("a", "peer-a", t, 600)), json(&takeover)];
        for i in 0..COMPACT_THRESHOLD + 50 {
            let path = format!("churn-{}.rs", i % 40);
            log.push(json(&entry_for(&path, "c", LockAction::Acquire)));
            log.push(json(&entry_for(&path, "c", LockAction::Release)));
        }
        log.push(json(&renewal));

        let holder_of = |entries: &[String]| {
            let doc = Doc::new();
            let array = doc.get_or_insert_array("locks");
            let mut txn = doc.transact_mut();
            for raw in entries {
                array.push_back(&mut txn, Any::String(raw.as_str().into()));
            }
            compute_lock_holders_at(&array, &txn, t + Duration::seconds(700))
                .remove("src/shared.rs")
        };
        let before = holder_of(&log).expect("precondition: b holds the renewed lease");
        compact_persisted_lock_log(&mut log);
        let after = holder_of(&log).expect("compaction dropped a live lease");
        assert_eq!(after.agent_id, before.agent_id);
        assert_eq!(after.expires_at, before.expires_at);
        assert_eq!(after.taken_over_from, before.taken_over_from);
    }

    #[test]
    fn only_an_edit_from_another_device_goes_against_a_lock() {
        let t = Utc::now();
        let doc = Doc::new();
        {
            let log = doc.get_or_insert_array(crate::control::LOCK_LOG_KEY);
            let mut txn = doc.transact_mut();
            append_lock_entry(&log, &mut txn, &lease("a", "peer-a", t, 600)).unwrap();
            let mut legacy = entry("b", "", LockAction::Acquire);
            legacy.path = "legacy.rs".into();
            append_lock_entry(&log, &mut txn, &legacy).unwrap();
        }
        assert!(holder_elsewhere(&doc, "src/shared.rs", "peer-a").is_none());
        assert_eq!(
            holder_elsewhere(&doc, "src/shared.rs", "peer-b").map(|h| h.agent_id),
            Some("a".into())
        );
        assert!(holder_elsewhere(&doc, "legacy.rs", "peer-b").is_none());
        assert!(holder_elsewhere(&doc, "unbound.rs", "peer-b").is_none());
    }
}
