//! Why nothing ran: a bounded, local record of admission decisions.
//!
//! See `docs/guide/agents.md`, "Finding out why nothing happened". The reaction loop drops
//! messages for a dozen good reasons and, until this existed, said so at
//! `trace!` or not at all — [`reaction::dispatch`] returns silently when the
//! device is in pull mode or the agent is not configured, which are the two
//! most common causes of "the agent wasn't triggered". A user cannot debug what
//! the daemon does not report, and a `trace!` build is not a support channel.
//!
//! Deliberately *not* synced and deliberately not durable. These are this
//! device's own reasons for not acting; a peer neither needs them nor should be
//! told what this machine declined to spend. Phase C's observation ledger
//! (§2.1) persists the same decisions for a different purpose — dedup — and
//! will subsume this buffer's contents when it lands.
//!
//! Records are collapsed by `(message_id, agent, reason)` rather than appended,
//! because the five-second reconcile tick re-derives the same decision about
//! the same message forever. Thirty identical lines is not thirty facts.

use std::collections::VecDeque;
use std::sync::Mutex;

/// How many distinct decisions to keep. Sized so a quiet Circle retains
/// yesterday's reason and a busy one still shows the last few minutes.
const CAPACITY: usize = 200;

#[derive(Clone, Debug, serde::Serialize)]
pub struct Decision {
    pub message_id: String,
    /// The agent this concerns, or `None` when the decision is about the
    /// message as a whole (nobody was eligible, it never reached selection).
    pub agent: Option<String>,
    pub reason: String,
    /// How many times this exact decision has been re-derived. Almost always
    /// driven by the reconcile tick, so a high count means "still true", not
    /// "happened repeatedly".
    pub count: u32,
    pub first_at: i64,
    pub last_at: i64,
}

impl Decision {
    fn matches(&self, message_id: &str, agent: Option<&str>, reason: &str) -> bool {
        self.message_id == message_id && self.agent.as_deref() == agent && self.reason == reason
    }
}

/// A fixed-size ring of the most recent distinct admission decisions.
#[derive(Debug, Default)]
pub struct AdmissionLog {
    entries: Mutex<VecDeque<Decision>>,
}

impl AdmissionLog {
    pub fn new() -> Self {
        Self::default()
    }

    /// Record that `message_id` was not acted on, and why.
    ///
    /// An identical decision already in the buffer is refreshed in place and
    /// moved to the front rather than duplicated.
    pub fn record(&self, message_id: &str, agent: Option<&str>, reason: &str, now: i64) {
        let mut entries = self.entries.lock().unwrap();
        if let Some(index) = entries
            .iter()
            .position(|d| d.matches(message_id, agent, reason))
        {
            let mut existing = entries.remove(index).expect("index came from iter");
            existing.count = existing.count.saturating_add(1);
            existing.last_at = now;
            entries.push_front(existing);
            return;
        }
        entries.push_front(Decision {
            message_id: message_id.to_string(),
            agent: agent.map(str::to_string),
            reason: reason.to_string(),
            count: 1,
            first_at: now,
            last_at: now,
        });
        while entries.len() > CAPACITY {
            entries.pop_back();
        }
    }

    /// Forget every decision about `message_id`.
    ///
    /// Called when the message does get a turn after all, so a skip recorded by
    /// an earlier pass — "every listener spoke recently", say — does not sit in
    /// the panel next to the run it was superseded by.
    pub fn clear_message(&self, message_id: &str) {
        self.entries
            .lock()
            .unwrap()
            .retain(|d| d.message_id != message_id);
    }

    /// Most recently decided first.
    pub fn recent(&self, limit: usize) -> Vec<Decision> {
        self.entries
            .lock()
            .unwrap()
            .iter()
            .take(limit)
            .cloned()
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_unchanged_decision_collapses_instead_of_repeating() {
        // The reconcile tick re-derives the same skip every five seconds. That
        // is one fact that is still true, not a stream of new ones.
        let log = AdmissionLog::new();
        for tick in 0..30 {
            log.record("m1", Some("claude"), "pull mode", 100 + tick * 5);
        }
        let recent = log.recent(10);
        assert_eq!(recent.len(), 1);
        assert_eq!(recent[0].count, 30);
        assert_eq!(recent[0].first_at, 100, "when it started being true");
        assert_eq!(recent[0].last_at, 245, "and when it last was");
    }

    #[test]
    fn distinct_reasons_and_agents_stay_distinct() {
        let log = AdmissionLog::new();
        log.record("m1", Some("claude"), "pull mode", 1);
        log.record("m1", Some("codex"), "pull mode", 1);
        log.record("m1", Some("claude"), "not configured", 1);
        log.record("m1", None, "every listener spoke recently", 1);
        assert_eq!(log.recent(10).len(), 4);
    }

    #[test]
    fn the_newest_decision_is_first_even_when_refreshed() {
        let log = AdmissionLog::new();
        log.record("m1", None, "a", 1);
        log.record("m2", None, "b", 2);
        log.record("m1", None, "a", 3);
        let recent = log.recent(10);
        assert_eq!(recent[0].message_id, "m1", "refreshed moves to the front");
        assert_eq!(recent[1].message_id, "m2");
    }

    #[test]
    fn the_buffer_is_bounded_and_drops_the_oldest() {
        let log = AdmissionLog::new();
        for i in 0..(CAPACITY + 50) {
            log.record(&format!("m{i}"), None, "too short to be worth a turn", 1);
        }
        let recent = log.recent(CAPACITY * 2);
        assert_eq!(recent.len(), CAPACITY);
        assert_eq!(recent[0].message_id, format!("m{}", CAPACITY + 49));
        assert!(
            !recent.iter().any(|d| d.message_id == "m0"),
            "the oldest decision is evicted"
        );
    }

    #[test]
    fn a_message_that_runs_after_all_drops_its_skips() {
        let log = AdmissionLog::new();
        log.record("m1", None, "every listener spoke recently", 1);
        log.record("m2", None, "too short to be worth a turn", 1);
        log.clear_message("m1");
        let recent = log.recent(10);
        assert_eq!(recent.len(), 1);
        assert_eq!(recent[0].message_id, "m2");
    }
}
