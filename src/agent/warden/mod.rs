//! The warden: decides, for a message nobody addressed, whether an agent should
//! answer, which way, and who.
//!
//! It sits on the scry path, after the gates that are invariants rather than
//! guesses — only a person's message, never one already addressed to an agent,
//! never one already answered or claimed — and before any listener is offered
//! the message. A call by name never reaches it.
//!
//! Three modes per Circle, set on this device (`warden_mode`):
//!
//! - **off** — scry as before: rotation picks the listeners.
//! - **shadow** — rotation still picks, and the warden's choice is recorded
//!   beside it for comparison. Nothing it decides is carried out.
//! - **act** — the warden's choice is carried out: nobody, one listener, a
//!   room of several, or a blind vote ([`vote`]).
//!
//! Its parts: [`spec`] is the policy (data, replaceable whole), [`decider`]
//! asks a decision model for probabilities, [`policy`] turns those into a
//! choice, [`log`] records every decision for later scoring, and [`episodes`]
//! joins the log with what became of each run.

pub mod decider;
pub mod episodes;
pub mod log;
pub mod policy;
pub mod spec;
pub mod verify;
pub mod vote;

use crate::control::ChatMessage;
use crate::state::AppState;
use serde::{Deserialize, Serialize};
use spec::{ModeKind, WardenSpec};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WardenMode {
    #[default]
    Off,
    Shadow,
    Act,
}

/// What the warden asked of one listener's turn. Carried on the queued request
/// so it survives a restart and a config change while the turn waits.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Assignment {
    pub mode: ModeKind,
    /// Voters at most, for a vote.
    #[serde(default)]
    pub max: usize,
    /// Every candidate, best first; a vote that needs one more voter takes the
    /// next unused one.
    #[serde(default)]
    pub pool: Vec<String>,
    /// This turn checks an answer already posted, rather than answering.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub check: bool,
}

/// The instruction for a turn the warden assigned, in place of the scry one.
///
/// A message the warden admitted has already been judged to need an answer,
/// so the turn is told it is being asked. The scry instruction's "passing is
/// the common case" is the right prior for overheard chat and the reason a
/// lone listener passed on direct questions (#299).
pub fn instruction(
    assignment: &Assignment,
    co_listeners: &[String],
    self_id: &str,
    name: impl Fn(&str) -> String,
) -> String {
    if assignment.mode == ModeKind::Vote {
        return vote::instruction(co_listeners.len().max(2));
    }
    if assignment.check {
        return verify::instruction();
    }
    let others: Vec<String> = co_listeners
        .iter()
        .map(String::as_str)
        .filter(|listener| !listener.eq_ignore_ascii_case(self_id))
        .map(name)
        .collect();
    let shared = if others.is_empty() {
        String::new()
    } else {
        format!(
            " {} {} answering it too, and you will see their replies if they post first; \
             cover what you are best placed to cover.",
            others.join(" and "),
            if others.len() == 1 { "is" } else { "are" },
        )
    };
    format!(
        "Nobody named you, but this room's warden judged that this message needs an answer and \
         chose you to give one, so answer it.{shared} Reply with exactly PASS only if it plainly \
         needs nothing from you — for example, someone has already answered it in the recent \
         history above. This is a conversational turn, not a work order: do not change files. If \
         something needs doing, say so and let someone ask for it."
    )
}

/// The decision for one message, while it is being made.
enum Slot {
    Pending,
    Ready(Box<policy::Decision>),
}

/// How many undecided messages may be waiting on a decider at once. Past this
/// the oldest are forgotten; their messages are asked about again on the next
/// drain.
const MAX_SLOTS: usize = 512;

/// Per-Circle warden state on this device.
#[derive(Default)]
pub struct Runtime {
    slots: Mutex<HashMap<String, Slot>>,
    /// Woken when a decision a drain is waiting for is ready.
    pub ready: Arc<tokio::sync::Notify>,
    pub votes: vote::Board,
}

/// Decide, asking the configured decider. Never fails: a decider that cannot
/// be asked, or does not answer in time, leaves the decision to the rules, and
/// the decision says why.
pub async fn decide(
    cfg: &decider::WardenConfig,
    spec: &WardenSpec,
    features: &policy::Features,
) -> policy::Decision {
    match decider::resolve(cfg) {
        Err(why) => policy::by_rules(spec, features, Some(why)),
        Ok(resolved) if resolved.is_rules() => policy::by_rules(spec, features, None),
        Ok(resolved) => {
            let questions = policy::questions(spec, features);
            match decider::ask(&resolved, &policy::input(features), &questions).await {
                Ok(reply) => policy::from_reply(spec, features, &resolved.manifest.id, &reply),
                Err(error) => policy::by_rules(
                    spec,
                    features,
                    Some(format!(
                        "{} could not be asked: {error:#}",
                        resolved.manifest.id
                    )),
                ),
            }
        }
    }
}

/// Can a decision be made without waiting, because no decider would be asked?
fn immediate(cfg: &decider::WardenConfig) -> Option<Option<String>> {
    match decider::resolve(cfg) {
        Err(why) => Some(Some(why)),
        Ok(resolved) if resolved.is_rules() => Some(None),
        Ok(_) => None,
    }
}

impl Runtime {
    /// The decision for `features`, if it is ready.
    ///
    /// With the rules, or a decider that cannot be used, it is made here and
    /// returned at once. Otherwise the first call starts asking and returns
    /// `None`; the drain leaves the message undecided, and [`Self::ready`] wakes
    /// it again when the answer is in.
    pub fn decision_for(
        self: &Arc<Self>,
        cfg: &decider::WardenConfig,
        spec: &WardenSpec,
        features: &policy::Features,
    ) -> Option<policy::Decision> {
        if let Some(fallback) = immediate(cfg) {
            return Some(policy::by_rules(spec, features, fallback));
        }
        let id = features.message_id.clone();
        {
            let mut slots = self.slots.lock().unwrap();
            match slots.remove(&id) {
                Some(Slot::Ready(decision)) => return Some(*decision),
                Some(Slot::Pending) => {
                    slots.insert(id, Slot::Pending);
                    return None;
                }
                None => {}
            }
            if slots.len() >= MAX_SLOTS {
                slots.clear();
            }
            slots.insert(id.clone(), Slot::Pending);
        }
        let this = self.clone();
        let (cfg, spec, features) = (cfg.clone(), spec.clone(), features.clone());
        tokio::spawn(async move {
            let decision = decide(&cfg, &spec, &features).await;
            let mut slots = this.slots.lock().unwrap();
            // Forgotten meanwhile (the map was cleared): the next drain asks
            // again rather than finding a stale answer.
            if let Some(slot) = slots.get_mut(&id) {
                *slot = Slot::Ready(Box::new(decision));
            }
            drop(slots);
            this.ready.notify_one();
        });
        None
    }

    /// Decide in the background and record the decision without acting on it.
    pub fn observe(
        &self,
        circle_dir: std::path::PathBuf,
        cfg: &decider::WardenConfig,
        spec: &WardenSpec,
        features: policy::Features,
        phase: log::Phase,
        executed: Option<log::Executed>,
    ) {
        let record = move |decision: policy::Decision, features: policy::Features| {
            log::append(
                &circle_dir,
                &log::Record {
                    ts: chrono::Utc::now().timestamp(),
                    message_id: features.message_id.clone(),
                    phase,
                    features: Some(features),
                    decision: Some(decision),
                    executed,
                    vote: None,
                    verify: None,
                },
            )
        };
        if let Some(fallback) = immediate(cfg) {
            record(policy::by_rules(spec, &features, fallback), features);
            return;
        }
        let (cfg, spec) = (cfg.clone(), spec.clone());
        tokio::spawn(async move {
            let decision = decide(&cfg, &spec, &features).await;
            record(decision, features);
        });
    }
}

/// How many lines of the room the decider is shown before the message.
const CONTEXT_LINES: usize = 6;

/// How much of each line, and of the message, the decider is shown.
const LINE_LIMIT: usize = 400;
const TEXT_LIMIT: usize = 4_000;

fn clip(text: &str, limit: usize) -> String {
    let flat = text.replace(['\r', '\n'], " ");
    let mut chars = flat.chars();
    let head: String = chars.by_ref().take(limit).collect();
    if chars.next().is_some() {
        format!("{head}…")
    } else {
        head
    }
}

/// What the warden is told about `message`.
///
/// `candidates` is in rotation order (least recently offered first) and already
/// excludes anyone the quiet gate holds back.
pub fn features(
    state: &AppState,
    history: &[ChatMessage],
    message: &ChatMessage,
    candidates: &[String],
    ceiling: usize,
) -> policy::Features {
    let roster = crate::agent::label::Roster::load(state);
    let at = history
        .iter()
        .position(|m| m.id == message.id)
        .unwrap_or(history.len());
    let context = history[at.saturating_sub(CONTEXT_LINES)..at]
        .iter()
        .map(|m| format!("{}: {}", roster.speaker(m), clip(&m.text, LINE_LIMIT)))
        .collect();
    policy::Features {
        message_id: message.id.clone(),
        text: clip(&message.text, TEXT_LIMIT),
        weighted_len: crate::agent::ambient::weighted_len(message.text.trim()),
        attachments: message.attachments.len(),
        context,
        candidates: candidates
            .iter()
            .map(|name| policy::Candidate {
                name: name.clone(),
                summary: crate::agent::cards::local_summary(state, name),
            })
            .collect(),
        ceiling,
    }
}

/// Whether a message the gates turned away is one of the sampled fraction.
/// By message id, so every device and every restart agrees.
pub fn sampled(message_id: &str, fraction: f64) -> bool {
    if fraction <= 0.0 {
        return false;
    }
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for byte in message_id.as_bytes() {
        hash ^= *byte as u64;
        hash = hash.wrapping_mul(0x1000_0000_01b3);
    }
    (hash % 10_000) < (fraction * 10_000.0) as u64
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_admitted_turn_is_told_it_is_being_asked() {
        let single = Assignment {
            mode: ModeKind::Single,
            max: 1,
            pool: vec![],
            check: false,
        };
        let text = instruction(&single, &["claude".into()], "claude", |n| n.to_string());
        assert!(text.contains("needs an answer"));
        assert!(!text.contains("Passing is the common case"));
        let room = Assignment {
            mode: ModeKind::Room,
            max: 2,
            pool: vec![],
            check: false,
        };
        let text = instruction(&room, &["claude".into(), "codex".into()], "claude", |n| {
            format!("{n} (on x)")
        });
        assert!(text.contains("codex (on x) is answering it too"));
        let vote = Assignment {
            mode: ModeKind::Vote,
            max: 3,
            pool: vec![],
            check: false,
        };
        assert!(
            instruction(&vote, &["a".into(), "b".into()], "a", |n| n.into()).contains("ANSWER:")
        );
    }

    #[test]
    fn sampling_is_by_id_and_respects_the_fraction() {
        assert!(!sampled("anything", 0.0));
        assert!(sampled("anything", 1.0));
        let hits = (0..2_000)
            .filter(|i| sampled(&format!("m{i}"), 0.25))
            .count();
        assert!((350..650).contains(&hits), "{hits}");
        assert_eq!(sampled("m1", 0.5), sampled("m1", 0.5));
    }

    #[tokio::test]
    async fn the_rules_decide_at_once_and_an_unusable_decider_falls_back() {
        let runtime = Arc::new(Runtime::default());
        let features = policy::Features {
            message_id: "m".into(),
            text: "x".into(),
            weighted_len: 30,
            attachments: 0,
            context: vec![],
            candidates: vec![policy::Candidate {
                name: "a".into(),
                summary: "a".into(),
            }],
            ceiling: 1,
        };
        let spec = WardenSpec::default();
        let d = runtime
            .decision_for(&decider::WardenConfig::default(), &spec, &features)
            .unwrap();
        assert_eq!(d.decider, decider::RULES);
        assert!(d.fallback.is_none());
        let missing = decider::WardenConfig {
            decider: "no-such-decider".into(),
            ..Default::default()
        };
        let d = runtime.decision_for(&missing, &spec, &features).unwrap();
        assert!(d.fallback.unwrap().contains("no decider named"));
    }

    #[tokio::test]
    async fn a_slow_decider_is_waited_for_then_handed_over_once() {
        // Nothing listens on this port: the request fails fast, and the rules
        // decide — after the first call has returned `None`.
        let runtime = Arc::new(Runtime::default());
        let features = policy::Features {
            message_id: "m".into(),
            text: "x".into(),
            weighted_len: 30,
            attachments: 0,
            context: vec![],
            candidates: vec![],
            ceiling: 1,
        };
        let cfg = decider::WardenConfig {
            decider: "litellm-decisions".into(),
            endpoint: Some("http://127.0.0.1:9/v1/decisions".into()),
            key_env: Some("PATH".into()),
            timeout_secs: 2,
            ..Default::default()
        };
        let spec = WardenSpec::default();
        let notified = runtime.ready.clone();
        let wait = notified.notified();
        assert!(runtime.decision_for(&cfg, &spec, &features).is_none());
        assert!(
            runtime.decision_for(&cfg, &spec, &features).is_none(),
            "still pending"
        );
        tokio::time::timeout(std::time::Duration::from_secs(10), wait)
            .await
            .unwrap();
        let d = runtime.decision_for(&cfg, &spec, &features).unwrap();
        assert!(d.fallback.unwrap().contains("could not be asked"));
        assert!(!d.choice.admit, "no candidates: nobody can take it");
    }
}
