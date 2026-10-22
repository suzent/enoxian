//! Blind vote: listeners answer without seeing one another, and one answer is
//! posted.
//!
//! Scry with several listeners lets each one see the others' replies and veil
//! its own. That is the right shape for a conversation and the wrong one for a
//! question with an answer: in the BBEH runs an agent that had it right went
//! along with a wrong answer it was shown 41% of the time. A vote keeps the
//! voters apart until they are done:
//!
//! 1. The first round's voters run as ordinary unaddressed turns, but their
//!    replies are held here instead of posted.
//! 2. When the last of the round finishes, the answers are compared. Agreement
//!    is judged by each answer's closing `ANSWER:` line, or by the decider
//!    when one is configured and the spec says to ask it.
//! 3. All agree: one answer is posted. They differ and the vote may grow: the
//!    next candidate is added as one more voter, and the round settles again
//!    when it finishes. Otherwise the answer most voters agree on is posted.
//!
//! Ballots are held in memory. A restart mid-vote loses them, and the turns
//! that cast them count as having produced nothing — the message is not
//! answered twice, which is the failure that matters in a room.

use super::log::{Ballot, Phase, Record, VoteRound};
use crate::agent::inbox::{Entry, Inbox, Request, Status};
use crate::state::AppState;
use std::collections::HashMap;
use std::sync::Mutex;

/// What one voter produced.
#[derive(Debug, Clone, PartialEq)]
pub enum Cast {
    Answer(String),
    Pass,
}

#[derive(Default)]
struct MessageVote {
    ballots: HashMap<String, Cast>,
    /// Voters already counted by a settled round. A later settle proceeds only
    /// once a voter it has not counted has finished.
    settled: usize,
    /// A settle is under way; another must not post the same answer.
    busy: bool,
    done: bool,
}

/// Ballots for every vote in progress on this device and Circle.
#[derive(Default)]
pub struct Board {
    votes: Mutex<HashMap<String, MessageVote>>,
}

impl Board {
    pub fn cast(&self, message_id: &str, agent: &str, cast: Cast) {
        self.votes
            .lock()
            .unwrap()
            .entry(message_id.to_string())
            .or_default()
            .ballots
            .insert(agent.to_string(), cast);
    }

    /// Take the right to settle this message's current round, if it is ready
    /// and nobody else has it. Returns the ballots cast so far.
    fn begin(&self, message_id: &str, voters: usize) -> Option<HashMap<String, Cast>> {
        let mut votes = self.votes.lock().unwrap();
        let vote = votes.entry(message_id.to_string()).or_default();
        if vote.done || vote.busy || voters <= vote.settled {
            return None;
        }
        vote.busy = true;
        Some(vote.ballots.clone())
    }

    fn finish(&self, message_id: &str, voters: usize, done: bool) {
        let mut votes = self.votes.lock().unwrap();
        let vote = votes.entry(message_id.to_string()).or_default();
        vote.busy = false;
        vote.settled = voters;
        vote.done = done;
        // A finished vote keeps only its flag, so the map stays small.
        if done {
            vote.ballots.clear();
        }
    }
}

/// The instruction a voter gets in place of the scry one.
pub fn instruction(voters: usize) -> String {
    format!(
        "You are one of {voters} agents answering this message independently. None of you can \
         see the others' answers, and only one answer will be posted: the one most of you agree \
         on. So answer it fully yourself rather than leaving part of it to someone else, and do \
         not hedge toward what others might say. Reply with exactly PASS only if the message \
         plainly needs no answer at all. End your reply with a final line of the form \
         `ANSWER: <your conclusion in a few words>` so the answers can be compared. This is a \
         conversational turn, not a work order: do not change files."
    )
}

/// The conclusion an answer states on its `ANSWER:` line, normalized for
/// comparison: case, markup, spacing and trailing punctuation do not count.
pub fn conclusion(text: &str) -> Option<String> {
    let line = text.lines().rev().find_map(|line| {
        let line = line
            .trim()
            .trim_matches(|c| c == '*' || c == '_' || c == '`');
        let upper = line.to_uppercase();
        let rest = upper
            .strip_prefix("ANSWER")
            .map(|_| &line["ANSWER".len()..])?;
        let rest = rest.trim_start_matches(['*', '_', ' ']);
        let rest = rest.strip_prefix(':').or_else(|| rest.strip_prefix('：'))?;
        Some(rest.to_string())
    })?;
    let normalized: String = line
        .to_lowercase()
        .chars()
        .filter(|c| !matches!(c, '*' | '_' | '`' | '"' | '\'' | '“' | '”'))
        .collect::<String>()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .trim_end_matches(['.', '。', '!', '！'])
        .trim()
        .to_string();
    (!normalized.is_empty()).then_some(normalized)
}

/// Group answers that agree, in order of first appearance.
fn group_by_conclusion(answers: &[(String, String)]) -> Vec<Vec<usize>> {
    let mut groups: Vec<(Option<String>, Vec<usize>)> = Vec::new();
    for (i, (_, text)) in answers.iter().enumerate() {
        let key = conclusion(text);
        match key
            .as_ref()
            .and_then(|k| groups.iter_mut().find(|(g, _)| g.as_ref() == Some(k)))
        {
            Some((_, members)) => members.push(i),
            None => groups.push((key, vec![i])),
        }
    }
    groups.into_iter().map(|(_, m)| m).collect()
}

/// Group answers by asking the decider whether each one agrees with the first
/// member of an existing group. `None` if any question could not be answered,
/// so the caller falls back to `ANSWER:` lines rather than half of each.
async fn group_by_decider(
    decider: &super::decider::Resolved,
    question: &str,
    answers: &[(String, String)],
) -> Option<Vec<Vec<usize>>> {
    use super::decider::{Ask, Question};
    let mut groups: Vec<Vec<usize>> = Vec::new();
    for (i, (_, text)) in answers.iter().enumerate() {
        let mut placed = false;
        for group in groups.iter_mut() {
            let (_, first) = &answers[group[0]];
            let input = format!("Answer A:\n{first}\n\nAnswer B:\n{text}");
            let q = [Question {
                name: "agree".into(),
                instructions: question.to_string(),
                ask: Ask::YesNo,
            }];
            let reply = super::decider::ask(decider, &input, &q).await.ok()?;
            if reply.yes("agree")? >= 0.5 {
                group.push(i);
                placed = true;
                break;
            }
        }
        if !placed {
            groups.push(vec![i]);
        }
    }
    Some(groups)
}

fn votes_for(entries: &[Entry], message_id: &str) -> Vec<Entry> {
    entries
        .iter()
        .filter(|e| {
            e.request.message.id == message_id
                && e.request
                    .warden
                    .as_ref()
                    .is_some_and(|a| a.mode == super::spec::ModeKind::Vote)
        })
        .cloned()
        .collect()
}

/// Settle the round `request` was part of, if it is complete. Called after
/// every voter's turn ends, however it ended.
pub async fn settle(
    state: &AppState,
    inbox: &Inbox,
    request: &Request,
    cfg: &crate::agent::config::AgentConfig,
) {
    let Some(assignment) = request.warden.as_ref() else {
        return;
    };
    let message_id = request.message.id.as_str();
    let voters = votes_for(&inbox.entries(), message_id);
    if voters
        .iter()
        .any(|e| matches!(e.status, Status::Pending | Status::Running))
    {
        return;
    }
    let board = &state.warden.votes;
    let Some(cast) = board.begin(message_id, voters.len()) else {
        return;
    };
    let (done, round) = resolve(state, inbox, cfg, assignment, request, &voters, &cast).await;
    board.finish(message_id, voters.len(), done);
    super::log::append(
        &state.circle_dir,
        &Record {
            ts: chrono::Utc::now().timestamp(),
            message_id: message_id.to_string(),
            phase: Phase::Vote,
            features: None,
            decision: None,
            executed: None,
            vote: Some(round),
        },
    );
}

async fn resolve(
    state: &AppState,
    inbox: &Inbox,
    cfg: &crate::agent::config::AgentConfig,
    assignment: &super::Assignment,
    request: &Request,
    voters: &[Entry],
    cast: &HashMap<String, Cast>,
) -> (bool, VoteRound) {
    let mut ballots = Vec::new();
    let mut answers: Vec<(String, String)> = Vec::new();
    for entry in voters {
        let agent = entry.request.agent.clone();
        let result = match (entry.status, cast.get(&agent)) {
            (Status::Completed, Some(Cast::Answer(text))) => {
                answers.push((agent.clone(), text.clone()));
                "answered"
            }
            (Status::Completed, Some(Cast::Pass)) => "passed",
            _ => "failed",
        };
        ballots.push(Ballot {
            conclusion: answers
                .last()
                .filter(|(a, _)| *a == agent && result == "answered")
                .and_then(|(_, t)| conclusion(t)),
            agent,
            result: result.into(),
        });
    }
    let mut round = VoteRound {
        ballots,
        groups: Vec::new(),
        judged_by: "answer_line".into(),
        action: "nothing".into(),
        posted_by: None,
        added: None,
    };
    if answers.is_empty() {
        // Everyone passed, or every turn failed. A failure reopens the message
        // through the ordinary scry retry; a unanimous pass is an answer.
        return (true, round);
    }
    let (spec, _) = super::spec::WardenSpec::load();
    let decider = super::decider::resolve(&cfg.warden)
        .ok()
        .filter(|d| !d.is_rules());
    let mut groups = None;
    if answers.len() > 1 && spec.vote.ask_decider {
        if let Some(decider) = &decider {
            groups = group_by_decider(decider, &spec.questions.agree, &answers).await;
            if groups.is_some() {
                round.judged_by = decider.manifest.id.clone();
            }
        }
    }
    let groups = groups.unwrap_or_else(|| group_by_conclusion(&answers));
    round.groups = groups
        .iter()
        .map(|g| g.iter().map(|&i| answers[i].0.clone()).collect())
        .collect();
    // Largest group; ties go to the one that answered first.
    let top = groups
        .iter()
        .enumerate()
        .max_by(|(ia, a), (ib, b)| a.len().cmp(&b.len()).then(ib.cmp(ia)))
        .map(|(_, g)| g.clone())
        .unwrap_or_default();
    // Agreement needs two answers that agree. One answer beside passes or
    // failures is not a confirmed result, so the vote still grows if it can.
    let unanimous = groups.len() == 1 && answers.len() > 1;
    let used: Vec<String> = voters.iter().map(|e| e.request.agent.clone()).collect();
    let next = (voters.len() < assignment.max)
        .then(|| {
            assignment
                .pool
                .iter()
                .find(|c| !used.iter().any(|u| u.eq_ignore_ascii_case(c)))
                .cloned()
        })
        .flatten();
    if let (false, Some(next)) = (unanimous, next) {
        let mut co_listeners = used.clone();
        co_listeners.push(next.clone());
        let added = Request {
            agent: next.clone(),
            mention_key: format!("~ambient:{next}"),
            co_listeners,
            ..request.clone()
        };
        let settings = cfg.resolved(&state.circle_id);
        if let Ok(crate::agent::inbox::Admission::Accepted { .. }) = inbox.admit(
            added,
            settings.max_relay_turns,
            chrono::Utc::now().timestamp(),
        ) {
            round.action = "added_voter".into();
            round.added = Some(next);
            return (false, round);
        }
    }
    // Agreement, or no one left to ask: post the answer most voters agree on.
    let (agent, text) = &answers[top[0]];
    let note = match (unanimous, answers.len()) {
        (_, 1) => format!(
            "\n\n_Blind vote: the only answer; {} other voter{} gave none._",
            voters.len() - 1,
            if voters.len() == 2 { "" } else { "s" }
        ),
        (true, n) => format!("\n\n_Blind vote: {n} of {n} answers agreed._"),
        (false, n) if top.len() * 2 > n => format!(
            "\n\n_Blind vote: the answers differed; this is the one {} of {n} agreed on._",
            top.len()
        ),
        (false, n) => format!(
            "\n\n_Blind vote: the {n} answers did not agree, and nobody was left to break the \
             tie; this is the first one given._"
        ),
    };
    if !post(state, agent, &format!("{text}{note}"), request).await {
        // Settled, but not posted. Keep trying in the background rather than
        // drop the answer the vote paid for; the voters' turns are complete,
        // so nothing else would ever reopen the message.
        retry_post(
            state.clone(),
            agent.clone(),
            format!("{text}{note}"),
            request.clone(),
        );
        round.action = "post_retrying".into();
        round.posted_by = Some(agent.clone());
        return (true, round);
    }
    for (other, _) in &answers {
        if other != agent {
            crate::agent::reaction::publish_voted(state, other, &request.message.id);
        }
    }
    round.action = "posted".into();
    round.posted_by = Some(agent.clone());
    (true, round)
}

/// Post the settled answer. `false` if it could not be posted.
async fn post(state: &AppState, agent: &str, text: &str, request: &Request) -> bool {
    match crate::agent::reaction::post_vote(
        state,
        agent,
        text,
        request.relay.clone(),
        &request.message.id,
    )
    .await
    {
        Ok(_) => true,
        Err(error) => {
            tracing::warn!(
                "[warden] could not post the vote's answer to {}: {error}",
                request.message.id
            );
            false
        }
    }
}

/// How long a settled answer that would not post keeps being retried.
const POST_RETRY: [u64; 5] = [5, 15, 30, 60, 120];

/// Retry posting a settled answer; record whether it got through, and if it
/// never does, tell the room rather than leave the message looking ignored.
fn retry_post(state: AppState, agent: String, text: String, request: Request) {
    tokio::spawn(async move {
        let mut posted = false;
        for wait in POST_RETRY {
            tokio::time::sleep(std::time::Duration::from_secs(wait)).await;
            if post(&state, &agent, &text, &request).await {
                posted = true;
                break;
            }
        }
        super::log::append(
            &state.circle_dir,
            &Record {
                ts: chrono::Utc::now().timestamp(),
                message_id: request.message.id.clone(),
                phase: Phase::Vote,
                features: None,
                decision: None,
                executed: None,
                vote: Some(VoteRound {
                    ballots: Vec::new(),
                    groups: Vec::new(),
                    judged_by: String::new(),
                    action: if posted { "posted" } else { "post_failed" }.into(),
                    posted_by: Some(agent.clone()),
                    added: None,
                }),
            },
        );
        if !posted {
            crate::agent::reaction::announce_unanswered(
                &state,
                &request.message.id,
                "the agents agreed on an answer, but it could not be posted",
            );
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_answer_line_is_normalized() {
        assert_eq!(
            conclusion("Reasoning…\n\n**ANSWER:** The retry is on the reconcile tick."),
            Some("the retry is on the reconcile tick".into())
        );
        assert_eq!(conclusion("answer：  42 "), Some("42".into()));
        assert_eq!(conclusion("ANSWER: (B)\nthanks"), Some("(b)".into()));
        assert_eq!(conclusion("No conclusion line here"), None);
        assert_eq!(conclusion("ANSWER:   "), None);
    }

    #[test]
    fn answers_group_by_conclusion_and_unparsed_ones_stand_alone() {
        let answers = vec![
            ("a".to_string(), "x\nANSWER: True".to_string()),
            ("b".to_string(), "y\nanswer: true.".to_string()),
            ("c".to_string(), "no line".to_string()),
            ("d".to_string(), "also none".to_string()),
            ("e".to_string(), "ANSWER: False".to_string()),
        ];
        assert_eq!(
            group_by_conclusion(&answers),
            vec![vec![0, 1], vec![2], vec![3], vec![4]]
        );
    }

    #[test]
    fn a_round_settles_once() {
        let board = Board::default();
        board.cast("m", "a", Cast::Pass);
        assert!(board.begin("m", 2).is_some());
        assert!(board.begin("m", 2).is_none(), "busy");
        board.finish("m", 2, false);
        assert!(board.begin("m", 2).is_none(), "nothing new since");
        assert!(board.begin("m", 3).is_some(), "a third voter finished");
        board.finish("m", 3, true);
        assert!(board.begin("m", 4).is_none(), "done");
    }
}
