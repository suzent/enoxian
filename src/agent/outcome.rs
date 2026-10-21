//! What became of a run, read back from what happened afterwards.
//!
//! The execution inbox knows a run finished; it does not know whether what the
//! agent did was any good. Two things this device can already see say
//! something about that, and both are derived here when runs are listed rather
//! than recorded when they happen:
//!
//! - **Proposals.** A run's file changes are captured as proposals carrying the
//!   run's id as their `session_id`. Their status — accepted, rejected,
//!   reverted — is whatever this device's proposal store says now, which
//!   includes decisions made on other devices once they have synced. Reading it
//!   at list time means there is no second copy to fall out of date, and no
//!   hook on each of the paths a status can change by.
//! - **Called after a pass.** Every agent scrying the room passed on a message,
//!   nobody answered it, and soon after the same person called an agent by
//!   name in the same thread. That is often a miss — the message wanted an
//!   answer — but not always, so it is reported as what it is, not as a verdict.
//!
//! Like usage, these are this device's own observations about its own runs and
//! are never published to the Circle. Proposals garbage-collected from the store
//! drop out of a run's outcome with them.

use std::collections::HashMap;

use serde::Serialize;

use super::inbox::{Entry, Status};
use crate::control::{Author, ChatMessage};
use crate::proposal::model::{Proposal, ProposalStatus};

/// How soon after the passed message a call still counts as following it.
pub const CALLED_AFTER_PASS_WINDOW_SECS: i64 = 10 * 60;

/// A proposal a run produced, as it stands now.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ProposalOutcome {
    pub id: String,
    pub status: ProposalStatus,
    pub changed_paths: usize,
}

/// The person called an agent by name after every listener passed.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct CalledAfterPass {
    /// The message that called an agent.
    pub message_id: String,
    pub agents: Vec<String>,
    /// Seconds between the passed message and the call, by the author's clock.
    pub after_secs: i64,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub struct RunOutcome {
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub proposals: Vec<ProposalOutcome>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub called_after_pass: Option<CalledAfterPass>,
}

impl RunOutcome {
    pub fn is_empty(&self) -> bool {
        self.proposals.is_empty() && self.called_after_pass.is_none()
    }
}

/// Outcomes for `entries`, keyed by run id; runs with nothing to report are
/// left out.
pub fn derive(
    entries: &[&Entry],
    proposals: &[Proposal],
    transcript: &[ChatMessage],
) -> HashMap<String, RunOutcome> {
    let mut by_run: HashMap<&str, Vec<ProposalOutcome>> = HashMap::new();
    for p in proposals {
        if let Some(run) = p.session_id.as_deref() {
            by_run.entry(run).or_default().push(ProposalOutcome {
                id: p.id.clone(),
                status: p.status,
                changed_paths: p.changed_paths.len(),
            });
        }
    }
    entries
        .iter()
        .filter_map(|entry| {
            let outcome = RunOutcome {
                proposals: by_run.remove(entry.run_id.as_str()).unwrap_or_default(),
                called_after_pass: called_after_pass(entry, transcript),
            };
            (!outcome.is_empty()).then(|| (entry.run_id.clone(), outcome))
        })
        .collect()
}

/// Did this unaddressed turn pass, go unanswered, and get followed by a call?
fn called_after_pass(entry: &Entry, transcript: &[ChatMessage]) -> Option<CalledAfterPass> {
    let asked = &entry.request.message;
    let passed = entry.request.ambient
        && entry.status == Status::Completed
        && entry.detail.as_deref() == Some(super::reaction::PASSED_DETAIL);
    if !passed || asked.author != Author::Human {
        return None;
    }
    // Some agent answered after all (on this device or another): not a miss.
    let answered = transcript
        .iter()
        .any(|m| m.author == Author::Agent && m.reply_to.as_deref() == Some(asked.id.as_str()));
    if answered {
        return None;
    }
    transcript
        .iter()
        .filter(|m| {
            m.id != asked.id
                && m.author == Author::Human
                && m.peer_id == asked.peer_id
                && m.agent_id == asked.agent_id
                && !m.mentions.is_empty()
                && m.ts > asked.ts
                && m.ts - asked.ts <= CALLED_AFTER_PASS_WINDOW_SECS
                && (m.reply_to.as_deref() == Some(asked.id.as_str())
                    || m.thread_root == asked.thread_root)
        })
        .min_by_key(|m| m.ts)
        .map(|m| CalledAfterPass {
            message_id: m.id.clone(),
            agents: m.mentions.clone(),
            after_secs: m.ts - asked.ts,
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::inbox::tests::request;

    fn message(id: &str, author: Author, ts: i64, mentions: &[&str]) -> ChatMessage {
        ChatMessage {
            thread_root: None,
            id: id.into(),
            agent_id: if author == Author::Human {
                "human"
            } else {
                "claude"
            }
            .into(),
            text: "text".into(),
            mentions: mentions.iter().map(|m| m.to_string()).collect(),
            ts,
            peer_id: "sender".into(),
            attachments: vec![],
            relay: None,
            author,
            reply_to: None,
        }
    }

    /// An unaddressed turn on the message `m1` (posted at 100) that passed.
    fn passed() -> Entry {
        let mut req = request("m1", "claude");
        req.ambient = true;
        req.message.mentions.clear();
        Entry {
            run_id: "run-1".into(),
            request: req,
            status: Status::Completed,
            detail: Some(super::super::reaction::PASSED_DETAIL.into()),
            admitted_at: 100,
            updated_at: 160,
            usage: None,
        }
    }

    fn proposal(id: &str, run: &str, status: ProposalStatus) -> Proposal {
        let mut p = Proposal::ambient("c1".into(), "s0".into(), "s1".into(), vec!["a.md".into()]);
        p.id = id.into();
        p.session_id = Some(run.into());
        p.status = status;
        p
    }

    #[test]
    fn a_runs_proposals_report_their_current_status() {
        let mut entry = passed();
        entry.request.ambient = false;
        entry.detail = None;
        let proposals = [
            proposal("p1", "run-1", ProposalStatus::Reverted),
            proposal("p2", "other-run", ProposalStatus::Accepted),
        ];
        let outcomes = derive(&[&entry], &proposals, &[]);
        let outcome = &outcomes["run-1"];
        assert_eq!(outcome.proposals.len(), 1);
        assert_eq!(outcome.proposals[0].status, ProposalStatus::Reverted);
        assert_eq!(outcome.called_after_pass, None);
    }

    #[test]
    fn a_call_soon_after_an_unanswered_pass_is_reported() {
        let entry = passed();
        let transcript = [
            entry.request.message.clone(),
            message("unrelated", Author::Human, 120, &[]),
            message("call", Author::Human, 190, &["claude"]),
        ];
        let outcome = &derive(&[&entry], &[], &transcript)["run-1"];
        let call = outcome.called_after_pass.as_ref().unwrap();
        assert_eq!(call.message_id, "call");
        assert_eq!(call.agents, ["claude"]);
        assert_eq!(call.after_secs, 90);
    }

    #[test]
    fn a_pass_is_not_a_miss_when_it_was_answered_or_nobody_followed_up() {
        let entry = passed();
        let asked = entry.request.message.clone();
        let call = message("call", Author::Human, 190, &["claude"]);

        // Another agent answered the message.
        let mut answer = message("answer", Author::Agent, 150, &[]);
        answer.reply_to = Some(asked.id.clone());
        let answered = [asked.clone(), answer, call.clone()];
        assert!(derive(&[&entry], &[], &answered).is_empty());

        // The call came too late to be about it.
        let mut late = call.clone();
        late.ts = asked.ts + CALLED_AFTER_PASS_WINDOW_SECS + 1;
        assert!(derive(&[&entry], &[], &[asked.clone(), late]).is_empty());

        // Someone else called, or it was in another thread.
        let mut other_person = call.clone();
        other_person.peer_id = "someone-else".into();
        let mut other_thread = call.clone();
        other_thread.thread_root = Some("elsewhere".into());
        assert!(derive(&[&entry], &[], &[asked.clone(), other_person, other_thread]).is_empty());

        // The turn answered rather than passed.
        let mut replied = entry.clone();
        replied.detail = None;
        assert!(derive(&[&replied], &[], &[asked, call]).is_empty());
    }

    #[test]
    fn a_reply_to_the_passed_message_counts_whatever_its_thread() {
        let entry = passed();
        let mut call = message("call", Author::Human, 130, &["codex"]);
        call.reply_to = Some("m1".into());
        call.thread_root = Some("m1".into());
        let outcome = &derive(&[&entry], &[], &[entry.request.message.clone(), call])["run-1"];
        assert_eq!(
            outcome.called_after_pass.as_ref().unwrap().message_id,
            "call"
        );
    }
}
