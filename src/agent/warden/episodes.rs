//! Episodes: each decision joined with what became of it.
//!
//! The warden's log says what it saw and chose; the execution inbox says what
//! ran and what it cost; the run outcomes say whether its proposals were kept
//! and whether a pass was followed by someone calling an agent by name. One
//! episode per message puts the three together, which is what an experiment
//! scores a policy against — offline replay, or an RSI loop proposing a new
//! spec. Built on request and never stored: the inputs are the record.
//!
//! Local to this device, like every input. Nothing here leaves the machine
//! unless someone exports it.

use super::log::{self, Executed, Phase, VoteRound};
use super::policy::{Decision, Features};
use crate::agent::inbox::{Entry, Inbox};
use crate::agent::outcome::RunOutcome;
use crate::state::AppState;
use serde::Serialize;
use std::collections::BTreeMap;

#[derive(Debug, Clone, Serialize)]
pub struct Judgement {
    pub ts: i64,
    pub phase: Phase,
    pub decision: Decision,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub executed: Option<Executed>,
}

#[derive(Debug, Clone, Serialize)]
pub struct Run {
    pub run_id: String,
    pub agent: String,
    pub status: crate::agent::inbox::Status,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
    pub ambient: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub assignment: Option<super::Assignment>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub usage: Option<crate::agent::usage::TurnUsage>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub outcome: Option<RunOutcome>,
}

#[derive(Debug, Clone, Serialize)]
pub struct Episode {
    pub message_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub features: Option<Features>,
    pub judgements: Vec<Judgement>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub votes: Vec<VoteRound>,
    pub runs: Vec<Run>,
    /// Did any agent reply to the message, on any device?
    pub answered: bool,
    /// What every run of this message reported spending, summed per currency.
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub spend: BTreeMap<String, f64>,
}

/// Every message the warden has a record of, oldest first. `since` keeps only
/// messages first recorded at or after that time.
pub fn build(state: &AppState, since: Option<i64>) -> Vec<Episode> {
    let records = log::read_all(&state.circle_dir);
    let entries: Vec<Entry> = Inbox::read(&state.circle_dir)
        .ok()
        .flatten()
        .map(|s| s.entries)
        .unwrap_or_default();
    let transcript = state.transcript();
    let proposals = crate::proposal::store::ProposalStore::open(&state.workspace)
        .map(|store| store.list_proposals())
        .unwrap_or_default();
    let roster = crate::agent::label::Roster::load(state);

    let mut order: Vec<String> = Vec::new();
    let mut by_message: BTreeMap<String, Episode> = BTreeMap::new();
    // Messages first recorded before `since`. A later record of one — a vote
    // settling, a check — must not bring it back as a partial episode.
    let mut before: std::collections::HashSet<String> = std::collections::HashSet::new();
    for record in records {
        if before.contains(&record.message_id) {
            continue;
        }
        if since.is_some_and(|s| record.ts < s) && !by_message.contains_key(&record.message_id) {
            before.insert(record.message_id.clone());
            continue;
        }
        let episode = by_message
            .entry(record.message_id.clone())
            .or_insert_with(|| {
                order.push(record.message_id.clone());
                Episode {
                    message_id: record.message_id.clone(),
                    features: None,
                    judgements: Vec::new(),
                    votes: Vec::new(),
                    runs: Vec::new(),
                    answered: false,
                    spend: BTreeMap::new(),
                }
            });
        if episode.features.is_none() {
            episode.features = record.features;
        }
        if let Some(decision) = record.decision {
            episode.judgements.push(Judgement {
                ts: record.ts,
                phase: record.phase,
                decision,
                executed: record.executed,
            });
        }
        if let Some(vote) = record.vote {
            episode.votes.push(vote);
        }
    }

    let runs_of: Vec<&Entry> = entries
        .iter()
        .filter(|e| by_message.contains_key(&e.request.message.id))
        .collect();
    let mut outcomes = crate::agent::outcome::derive(&runs_of, &proposals, &transcript, &roster);
    for entry in runs_of {
        let Some(episode) = by_message.get_mut(&entry.request.message.id) else {
            continue;
        };
        if let Some(cost) = entry.usage.as_ref().and_then(|u| u.cost.as_ref()) {
            *episode.spend.entry(cost.currency.clone()).or_default() += cost.amount;
        }
        episode.runs.push(Run {
            run_id: entry.run_id.clone(),
            agent: entry.request.agent.clone(),
            status: entry.status,
            detail: entry.detail.clone(),
            ambient: entry.request.ambient,
            assignment: entry.request.warden.clone(),
            usage: entry.usage.clone(),
            outcome: outcomes.remove(&entry.run_id),
        });
    }
    for message in &transcript {
        if message.author == crate::control::Author::Agent {
            if let Some(parent) = message.reply_to.as_deref() {
                if let Some(episode) = by_message.get_mut(parent) {
                    episode.answered = true;
                }
            }
        }
    }
    order
        .into_iter()
        .filter_map(|id| by_message.remove(&id))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::super::log::{append, Record};
    use super::*;

    fn record(ts: i64, id: &str, phase: Phase) -> Record {
        Record {
            ts,
            message_id: id.into(),
            phase,
            features: None,
            decision: None,
            executed: None,
            vote: None,
        }
    }

    #[test]
    fn since_leaves_out_a_message_first_seen_before_it() {
        let dir = tempfile::tempdir().unwrap();
        let state = AppState::new(
            "c1".into(),
            "circle".into(),
            dir.path().to_path_buf(),
            dir.path().to_path_buf(),
            String::new(),
            "label".into(),
            1,
            "peer".into(),
            crate::config::JoinPolicy::Manual,
            "owner".into(),
            crate::mls::new_mls_state(crate::mls::MlsIdentity::generate("peer").unwrap(), None),
        );
        append(dir.path(), &record(10, "old", Phase::Act));
        append(dir.path(), &record(20, "new", Phase::Act));
        // A vote on the old message settles after the boundary.
        append(dir.path(), &record(30, "old", Phase::Vote));
        let ids: Vec<String> = build(&state, Some(15))
            .into_iter()
            .map(|e| e.message_id)
            .collect();
        assert_eq!(ids, vec!["new"]);
        assert_eq!(build(&state, None).len(), 2);
    }
}
