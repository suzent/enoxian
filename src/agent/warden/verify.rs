//! A second look at a single answer.
//!
//! When the warden sends a message to one listener, nobody checks the answer.
//! The vote catches a wrong answer by asking two; this catches it after the
//! fact, at the price of one decider question per single answer: once the
//! answer is posted, the decider is asked whether it is right (`answer_ok`).
//! Below the spec's threshold, the next listener in the warden's ranking is
//! asked to check it — told an answer is already there, to pass if it holds
//! and to correct it if not.
//!
//! Only with a decider that answers questions: the rules have no opinion on an
//! answer. Only once per message, and never for a turn that was itself a
//! check, so a disputed answer cannot start a chain.

use super::log::{Phase, Record, Verification};
use super::spec::ModeKind;
use crate::agent::inbox::{Inbox, Request};
use crate::control::Author;
use crate::state::AppState;

/// The instruction for a turn asked to check an answer already posted.
pub fn instruction() -> String {
    "Another agent has already answered this message; its reply is in the history above. The \
     room's warden judged that answer may be wrong or incomplete and asked you to check it. Work \
     the question through yourself. If the answer holds, reply with exactly PASS. If it does not, \
     reply with the correction: say what is wrong and give the right answer, briefly. Do not \
     repeat what it got right. This is a conversational turn, not a work order: do not change \
     files."
        .to_string()
}

/// How much of the question and the answer the decider is shown.
const LIMIT: usize = 4_000;

fn clip(text: &str) -> String {
    let mut chars = text.chars();
    let head: String = chars.by_ref().take(LIMIT).collect();
    if chars.next().is_some() {
        format!("{head}…")
    } else {
        head
    }
}

/// Check the answer `request`'s turn just posted, if the spec asks for it.
pub async fn after_answer(
    state: &AppState,
    inbox: &Inbox,
    request: &Request,
    cfg: &crate::agent::config::AgentConfig,
) {
    let Some(assignment) = request.warden.as_ref() else {
        return;
    };
    if assignment.mode != ModeKind::Single || assignment.check {
        return;
    }
    let (spec, _) = super::spec::WardenSpec::load();
    if !spec.verify.enabled {
        return;
    }
    let Ok(decider) = super::decider::resolve(&cfg.warden) else {
        return;
    };
    if decider.is_rules() {
        return;
    }
    let message_id = request.message.id.as_str();
    let Some(answer) = state.transcript().into_iter().rev().find(|m| {
        m.reply_to.as_deref() == Some(message_id)
            && m.author == Author::Agent
            && m.agent_id == request.agent
            && m.peer_id == state.peer_id
    }) else {
        // It passed, or its post did not land: nothing to check.
        return;
    };
    let question = super::decider::Question {
        name: "answer_ok".into(),
        instructions: spec.questions.answer_ok.clone(),
        ask: super::decider::Ask::YesNo,
    };
    let input = format!(
        "Question:\n{}\n\nAnswer:\n{}",
        clip(&request.message.text),
        clip(&answer.text)
    );
    let reply = match super::decider::ask(&decider, &input, &[question]).await {
        Ok(reply) => reply,
        Err(error) => {
            tracing::debug!("[warden] could not check the answer to {message_id}: {error:#}");
            return;
        }
    };
    let Some(raw) = reply.yes("answer_ok") else {
        return;
    };
    let p_ok = super::spec::calibrate(spec.calibration.get("answer_ok"), raw);
    let checker = if p_ok < spec.verify.threshold {
        add_checker(state, inbox, request, assignment, cfg)
    } else {
        None
    };
    super::log::append(
        &state.circle_dir,
        &Record {
            ts: chrono::Utc::now().timestamp(),
            message_id: message_id.to_string(),
            phase: Phase::Verify,
            features: None,
            decision: None,
            executed: None,
            vote: None,
            verify: Some(Verification {
                answered_by: request.agent.clone(),
                p_ok,
                raw,
                threshold: spec.verify.threshold,
                decider: decider.manifest.id.clone(),
                checker,
                cost_usd: reply.cost_usd,
                latency_ms: Some(reply.latency_ms),
            }),
        },
    );
}

/// Queue the next ranked listener to check the answer. Returns who, if anyone.
fn add_checker(
    state: &AppState,
    inbox: &Inbox,
    request: &Request,
    assignment: &super::Assignment,
    cfg: &crate::agent::config::AgentConfig,
) -> Option<String> {
    let settings = cfg.resolved(&state.circle_id);
    let checker = assignment
        .pool
        .iter()
        .find(|c| !c.eq_ignore_ascii_case(&request.agent) && settings.is_ambient(c))?
        .clone();
    let check = Request {
        agent: checker.clone(),
        mention_key: format!("~ambient:{checker}"),
        co_listeners: vec![request.agent.clone(), checker.clone()],
        warden: Some(super::Assignment {
            mode: ModeKind::Single,
            max: 1,
            pool: assignment.pool.clone(),
            check: true,
        }),
        ..request.clone()
    };
    match inbox.admit(
        check,
        settings.max_relay_turns,
        chrono::Utc::now().timestamp(),
    ) {
        Ok(crate::agent::inbox::Admission::Accepted { .. }) => Some(checker),
        _ => None,
    }
}
