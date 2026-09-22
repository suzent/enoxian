//! Which messages this device has already decided about, and what it decided.
//!
//! See `docs/concepts/internals.md`, "Agent Runtime". Ambient admission used to
//! ask "was this message authored in the last thirty seconds", comparing the
//! *author's* wall clock against this device's. A peer whose clock ran a minute
//! slow could therefore never trigger an ambient turn here — not "after a
//! delay", but never, because its messages were born stale. A peer that synced
//! after a short disconnection had correct timestamps discarded for the same
//! reason.
//!
//! Addressed mentions never had this problem, because they are deduplicated by
//! a durable set — `handled_mentions.log` plus the inbox's `Duplicate` — which
//! involves no clock and is why an `@mention` an hour late still runs. Ambient
//! reached for a clock instead only because its dedup key does not exist before
//! the decision is made: the decision *is* which agents get offered the message.
//!
//! This is that missing set. The invariant it buys:
//!
//! > An ambient turn is offered for a human message **the first time this
//! > device decides about it**, whenever that happens, and at most once.
//!
//! "First time this device decides" is local, observable and monotonic, so
//! arrival latency stops mattering by construction.
//!
//! Not synced, like every other execution decision. What this machine spends is
//! its own business, and a peer replicating its ledger would be able to
//! suppress another device's turns.

use std::collections::HashMap;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

/// What this device decided about a message.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Decision {
    /// Offered to at least one listener.
    Offered,
    /// Eligible, but collapsed into a backlog whose tail carried the turn.
    Backlog,
    /// Already in the transcript when this device's agent loop first ran.
    ///
    /// The only decision that also suppresses an **addressed** mention, which
    /// is what lets it replace the `activated_at` timestamp comparison (§2.4):
    /// enabling agents in a Circle with history must not fire every `@mention`
    /// ever written in it, and "was this here when I started" is a set
    /// membership question, not a question about two machines' clocks.
    PreActivation,
    /// No agent reads this room, so there was nothing to offer it to.
    ///
    /// Distinct from [`Self::PreActivation`] precisely because it must *not*
    /// suppress an addressed mention: a Circle with no ambient listener still
    /// answers `@claude`.
    NoListener,
    /// Declined by a cheap gate, carrying the reason for diagnosis.
    Skipped(String),
}

impl Decision {
    fn encode(&self) -> String {
        match self {
            Self::Offered => "offered".into(),
            Self::Backlog => "backlog".into(),
            Self::PreActivation => "pre-activation".into(),
            Self::NoListener => "no-listener".into(),
            Self::Skipped(reason) => format!("skipped:{reason}"),
        }
    }

    fn decode(raw: &str) -> Option<Self> {
        match raw {
            "offered" => Some(Self::Offered),
            "backlog" => Some(Self::Backlog),
            "pre-activation" => Some(Self::PreActivation),
            "no-listener" => Some(Self::NoListener),
            other => other
                .strip_prefix("skipped:")
                .map(|reason| Self::Skipped(reason.to_string())),
        }
    }
}

fn path(circle_dir: &Path) -> PathBuf {
    circle_dir.join("ambient_decisions.log")
}

/// One line per message decision: `<message_id> <unix_ts> <decision>`.
///
/// A message may appear more than once — [`AmbientLedger::settle`] appends
/// rather than rewrites — and the **last** line for an id wins. Compaction on
/// load collapses the duplicates back to one.
///
/// The decision comes last because a skip carries its reason, and reasons have
/// spaces in them. Putting it at the end makes it the rest of the line, so
/// nothing needs escaping and a reason can be read back verbatim.
///
/// Append-only, and best-effort on write for the same reason as
/// [`super::handled`]: a lost line risks re-deciding one message after a
/// restart, which is a duplicate turn at worst and never a lost one.
pub struct AmbientLedger {
    file: PathBuf,
    decided: Mutex<HashMap<String, Decision>>,
}

impl AmbientLedger {
    /// Load the ledger, compacting it against the transcript.
    ///
    /// `transcript_ids` is every message id currently in the room, in order.
    /// Entries for messages that have left the transcript are dropped, which
    /// bounds the file by transcript size rather than by uptime.
    ///
    /// When no ledger exists yet, every id in `transcript_ids` is recorded as
    /// [`Decision::PreActivation`]. That is what makes the upgrade a no-op: an
    /// existing Circle's whole history is already decided, so the first drain
    /// sees only genuinely new messages instead of collapsing years of chat
    /// into a turn nobody asked for.
    pub fn load(circle_dir: &Path, transcript_ids: &[String], now: i64) -> Self {
        let file = path(circle_dir);
        let known: std::collections::HashSet<&str> =
            transcript_ids.iter().map(String::as_str).collect();
        let raw = std::fs::read_to_string(&file);
        let seeding = raw.is_err();
        let decided: HashMap<String, Decision> = raw
            .map(|text| {
                text.lines()
                    .filter_map(|line| {
                        let mut parts = line.splitn(3, ' ');
                        let id = parts.next()?;
                        let _at = parts.next()?;
                        let decision = Decision::decode(parts.next()?)?;
                        known.contains(id).then(|| (id.to_string(), decision))
                    })
                    .collect()
            })
            .unwrap_or_default();
        let ledger = Self {
            file,
            decided: Mutex::new(decided),
        };
        if seeding {
            for id in transcript_ids {
                ledger.record(id, Decision::PreActivation, now);
            }
        } else {
            ledger.rewrite(now);
        }
        ledger
    }

    /// Was this message already in the room when this device started listening?
    ///
    /// The replacement for `message.ts < inbox.activated_at()`. Unlike that
    /// comparison it cannot be wrong about a peer whose clock disagrees, and it
    /// cannot silently drop that peer's mentions for the lifetime of the
    /// Circle.
    pub fn predates_activation(&self, message_id: &str) -> bool {
        self.decision(message_id) == Some(Decision::PreActivation)
    }

    pub fn decided(&self, message_id: &str) -> bool {
        self.decided.lock().unwrap().contains_key(message_id)
    }

    pub fn decision(&self, message_id: &str) -> Option<Decision> {
        self.decided.lock().unwrap().get(message_id).cloned()
    }

    pub fn len(&self) -> usize {
        self.decided.lock().unwrap().len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Record a decision, appending it durably.
    ///
    /// A message already decided keeps its first decision: re-deciding would
    /// defeat the point of the ledger, and the caller filtering on
    /// [`Self::decided`] means this only fires on a genuine race or a bug.
    pub fn record(&self, message_id: &str, decision: Decision, now: i64) -> bool {
        let mut decided = self.decided.lock().unwrap();
        if decided.contains_key(message_id) {
            return false;
        }
        decided.insert(message_id.to_string(), decision);
        drop(decided);
        self.append(message_id, now);
        true
    }

    /// Append the current decision for `message_id` to the log.
    fn append(&self, message_id: &str, now: i64) {
        let Some(decision) = self.decision(message_id) else {
            return;
        };
        if let Some(parent) = self.file.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        match std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.file)
        {
            Ok(mut f) => {
                let _ = writeln!(f, "{message_id} {now} {}", decision.encode());
            }
            Err(e) => tracing::warn!("[agent] could not persist an ambient decision: {e}"),
        }
    }

    /// Overwrite an existing decision.
    ///
    /// Used when a message that was offered turns out not to have been
    /// answered — every attempt failed, and the ledger should say so rather
    /// than keep claiming the room was served (§3.2).
    pub fn settle(&self, message_id: &str, decision: Decision, now: i64) {
        self.decided
            .lock()
            .unwrap()
            .insert(message_id.to_string(), decision);
        // The file is append-only, and `load` keeps the *last* line for an id,
        // so appending the new decision is the overwrite.
        self.append(message_id, now);
    }

    /// Flush the compacted set back to disk, so entries dropped by [`Self::load`]
    /// do not accumulate in the file forever.
    fn rewrite(&self, now: i64) {
        let decided = self.decided.lock().unwrap();
        let body: String = decided
            .iter()
            .map(|(id, decision)| format!("{id} {now} {}\n", decision.encode()))
            .collect();
        drop(decided);
        // A failed compaction leaves the longer file in place, which is
        // correct, just larger. Never worth failing the reaction loop over.
        let temporary = self.file.with_extension("tmp");
        if std::fs::write(&temporary, body).is_ok() {
            let _ = std::fs::rename(&temporary, &self.file);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ids(names: &[&str]) -> Vec<String> {
        names.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn a_new_ledger_treats_existing_history_as_already_decided() {
        // Otherwise upgrading an existing Circle collapses its whole
        // transcript into one ambient turn nobody asked for.
        let dir = tempfile::tempdir().unwrap();
        let ledger = AmbientLedger::load(dir.path(), &ids(&["m1", "m2", "m3"]), 100);
        assert_eq!(ledger.len(), 3);
        assert_eq!(ledger.decision("m2"), Some(Decision::PreActivation));
        assert!(!ledger.decided("m4"), "a message it has not seen is new");
    }

    #[test]
    fn a_decision_survives_a_restart() {
        let dir = tempfile::tempdir().unwrap();
        let all = ids(&["m1", "m2"]);
        let ledger = AmbientLedger::load(dir.path(), &all, 100);
        assert!(ledger.record("m3", Decision::Offered, 101));
        assert!(
            !ledger.record("m3", Decision::Backlog, 102),
            "the first decision stands"
        );
        drop(ledger);

        let reloaded = AmbientLedger::load(dir.path(), &ids(&["m1", "m2", "m3"]), 200);
        assert_eq!(reloaded.decision("m3"), Some(Decision::Offered));
    }

    #[test]
    fn a_skip_keeps_its_reason_across_a_restart() {
        let dir = tempfile::tempdir().unwrap();
        let ledger = AmbientLedger::load(dir.path(), &[], 100);
        ledger.record(
            "m1",
            Decision::Skipped("too short to be worth a turn".into()),
            100,
        );
        drop(ledger);
        let reloaded = AmbientLedger::load(dir.path(), &ids(&["m1"]), 200);
        assert_eq!(
            reloaded.decision("m1"),
            Some(Decision::Skipped("too short to be worth a turn".into()))
        );
    }

    #[test]
    fn entries_for_messages_that_left_the_transcript_are_compacted_away() {
        let dir = tempfile::tempdir().unwrap();
        let ledger = AmbientLedger::load(dir.path(), &[], 100);
        for id in ["m1", "m2", "m3"] {
            ledger.record(id, Decision::Offered, 100);
        }
        drop(ledger);

        // m2 is gone from the room, so its decision is no longer worth keeping.
        let reloaded = AmbientLedger::load(dir.path(), &ids(&["m1", "m3"]), 200);
        assert_eq!(reloaded.len(), 2);
        assert!(!reloaded.decided("m2"));
        // And the file itself shrank, rather than only the in-memory view.
        let text = std::fs::read_to_string(path(dir.path())).unwrap();
        assert_eq!(text.lines().count(), 2);
        assert!(!text.contains("m2"));
    }

    #[test]
    fn settling_overwrites_the_earlier_decision_and_survives_a_restart() {
        // A message that was offered and then failed every attempt must stop
        // claiming the room was served.
        let dir = tempfile::tempdir().unwrap();
        let ledger = AmbientLedger::load(dir.path(), &[], 100);
        ledger.record("m1", Decision::Offered, 100);
        ledger.settle("m1", Decision::Skipped("every listener tried".into()), 101);
        assert_eq!(
            ledger.decision("m1"),
            Some(Decision::Skipped("every listener tried".into()))
        );
        drop(ledger);

        let reloaded = AmbientLedger::load(dir.path(), &ids(&["m1"]), 200);
        assert_eq!(
            reloaded.decision("m1"),
            Some(Decision::Skipped("every listener tried".into())),
            "the last line for an id wins"
        );
        // And compaction collapsed the duplicate away.
        assert_eq!(
            std::fs::read_to_string(path(dir.path()))
                .unwrap()
                .lines()
                .count(),
            1
        );
    }

    #[test]
    fn a_corrupt_line_is_skipped_rather_than_fatal() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            path(dir.path()),
            "m1 100 offered\ngarbage\nm2 100 whatever\nm3 100 backlog\n",
        )
        .unwrap();
        let ledger = AmbientLedger::load(dir.path(), &ids(&["m1", "m2", "m3"]), 200);
        assert_eq!(ledger.decision("m1"), Some(Decision::Offered));
        assert_eq!(ledger.decision("m3"), Some(Decision::Backlog));
        assert!(!ledger.decided("m2"), "an unknown tag is not a decision");
    }
}
