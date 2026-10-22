//! From a message to a choice: which questions to ask, and what to do with the
//! answers.
//!
//! The decider only estimates probabilities. Everything that turns them into
//! an action is here, in code, under the spec's numbers: whether to answer at
//! all, which way, and who goes first. The rules decider skips the questions
//! and applies `spec.rules` directly, which is also what happens whenever a
//! configured decider cannot be reached in time.

use super::decider::{Ask, Question, Reply};
use super::spec::{ModeKind, WardenSpec};
use serde::Serialize;
use std::collections::BTreeMap;

/// A listener the warden may choose.
#[derive(Debug, Clone, PartialEq, Serialize, serde::Deserialize)]
pub struct Candidate {
    pub name: String,
    /// From the agent's card, or its qualified name when it has none.
    pub summary: String,
}

/// What the warden knows about a message when it decides.
#[derive(Debug, Clone, PartialEq, Serialize, serde::Deserialize)]
pub struct Features {
    pub message_id: String,
    pub text: String,
    /// In the scry length floor's weighted units, so a Chinese message is not
    /// judged short for being written in fewer characters.
    pub weighted_len: usize,
    pub attachments: usize,
    /// The last few lines before the message, oldest first.
    pub context: Vec<String>,
    /// Least recently offered first: the order rotation would use.
    pub candidates: Vec<Candidate>,
    /// The most listeners this Circle allows on one message.
    pub ceiling: usize,
}

/// What the warden decided to do.
#[derive(Debug, Clone, PartialEq, Serialize, serde::Deserialize)]
pub struct Choice {
    pub admit: bool,
    /// `None` when not admitted.
    pub mode: Option<ModeKind>,
    /// Listeners offered at first: 1 for single, the room size, or the first
    /// round of voters.
    pub listeners: usize,
    /// Voters at most, for a vote; equal to `listeners` otherwise.
    pub max_listeners: usize,
    /// Every candidate, best first. The first `listeners` are offered.
    pub agents: Vec<String>,
}

/// A decision, with everything needed to judge it later.
#[derive(Debug, Clone, PartialEq, Serialize, serde::Deserialize)]
pub struct Decision {
    pub choice: Choice,
    /// The decider that produced it: `builtin:rules` when the rules decided,
    /// including as a fallback.
    pub decider: String,
    pub spec_version: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub p_admit: Option<f64>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub p_success: BTreeMap<String, f64>,
    /// `p(success) − λ × cost`, per mode considered.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub scores: BTreeMap<String, f64>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub p_agent: BTreeMap<String, f64>,
    /// What the decider said, per question, before the spec's calibration.
    /// The probabilities above are calibrated; these are what a new
    /// calibration is fitted from.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub raw: BTreeMap<String, f64>,
    /// One line on why, for the activity panel.
    pub reason: String,
    /// Why the configured decider was not used, when it was not.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub fallback: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cost_usd: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub latency_ms: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub input_tokens: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cached_tokens: Option<u64>,
}

/// How many listeners each mode would use here, `(first, at most)`, or `None`
/// when the mode cannot run with these candidates under this ceiling.
pub fn shape(spec: &WardenSpec, features: &Features, mode: ModeKind) -> Option<(usize, usize)> {
    let available = features.candidates.len().min(features.ceiling.max(1));
    match mode {
        ModeKind::Single => (available >= 1).then_some((1, 1)),
        ModeKind::Room => {
            let k = spec.room_size.min(available);
            (k >= 2).then_some((k, k))
        }
        ModeKind::Vote => {
            let start = spec.vote.start.min(available);
            let max = spec.vote.max.min(available).max(start);
            (start >= 2).then_some((start, max))
        }
    }
}

/// The modes the spec allows that can run here.
pub fn feasible(spec: &WardenSpec, features: &Features) -> Vec<ModeKind> {
    ModeKind::ALL
        .into_iter()
        .filter(|m| spec.allows(*m) && shape(spec, features, *m).is_some())
        .collect()
}

/// The decider's input: the conversation, then the message. Stable parts first,
/// so a proxy that caches prefixes can reuse them across messages.
pub fn input(features: &Features) -> String {
    let mut out = String::new();
    if !features.context.is_empty() {
        out.push_str("Recent conversation in the room:\n");
        for line in &features.context {
            out.push_str("  ");
            out.push_str(line);
            out.push('\n');
        }
        out.push('\n');
    }
    out.push_str("New message, naming no one:\n");
    out.push_str(&features.text);
    if features.attachments > 0 {
        out.push_str(&format!("\n[{} attachment(s)]", features.attachments));
    }
    out
}

/// What to ask about this message.
pub fn questions(spec: &WardenSpec, features: &Features) -> Vec<Question> {
    let mut out = vec![Question {
        name: "admit".into(),
        instructions: spec.questions.admit.clone(),
        ask: Ask::YesNo,
    }];
    if features.candidates.len() > 1 {
        out.push(Question {
            name: "agent".into(),
            instructions: spec.questions.agent.clone(),
            ask: Ask::Choice(
                features
                    .candidates
                    .iter()
                    .map(|c| (c.name.clone(), c.summary.clone()))
                    .collect(),
            ),
        });
    }
    let modes = feasible(spec, features);
    // A mode with no alternative needs no estimate.
    if modes.len() > 1 {
        for mode in modes {
            out.push(Question {
                name: format!("success:{}", mode.as_str()),
                instructions: match mode {
                    ModeKind::Single => spec.questions.single.clone(),
                    ModeKind::Room => spec.questions.room.clone(),
                    ModeKind::Vote => spec.questions.vote.clone(),
                },
                ask: Ask::YesNo,
            });
        }
    }
    out
}

fn choice(spec: &WardenSpec, features: &Features, mode: ModeKind, agents: Vec<String>) -> Choice {
    let (listeners, max_listeners) = shape(spec, features, mode).unwrap_or((1, 1));
    Choice {
        admit: true,
        mode: Some(mode),
        listeners,
        max_listeners,
        agents,
    }
}

fn declined(agents: Vec<String>) -> Choice {
    Choice {
        admit: false,
        mode: None,
        listeners: 0,
        max_listeners: 0,
        agents,
    }
}

fn names(features: &Features) -> Vec<String> {
    features.candidates.iter().map(|c| c.name.clone()).collect()
}

/// The mode the rules give, among those that can run.
fn rules_mode(spec: &WardenSpec, features: &Features) -> Option<ModeKind> {
    let modes = feasible(spec, features);
    let wanted = if features.weighted_len >= spec.rules.long_chars {
        spec.rules.long_mode
    } else {
        spec.rules.short_mode
    };
    if modes.contains(&wanted) {
        Some(wanted)
    } else if modes.contains(&ModeKind::Single) {
        Some(ModeKind::Single)
    } else {
        modes.first().copied()
    }
}

/// Decide by the spec's rules alone.
pub fn by_rules(spec: &WardenSpec, features: &Features, fallback: Option<String>) -> Decision {
    let agents = names(features);
    let (choice, reason) = match rules_mode(spec, features) {
        Some(mode) => (
            choice(spec, features, mode, agents),
            format!(
                "{} message → {}",
                if features.weighted_len >= spec.rules.long_chars {
                    "long"
                } else {
                    "short"
                },
                mode.as_str()
            ),
        ),
        None => (declined(agents), "no listener can take it".to_string()),
    };
    Decision {
        choice,
        decider: super::decider::RULES.into(),
        spec_version: spec.version.clone(),
        p_admit: None,
        p_success: BTreeMap::new(),
        scores: BTreeMap::new(),
        p_agent: BTreeMap::new(),
        raw: BTreeMap::new(),
        reason,
        fallback,
        cost_usd: None,
        latency_ms: None,
        input_tokens: None,
        cached_tokens: None,
    }
}

/// Decide from a decider's answers.
///
/// A question the decider refused or skipped falls back to the rules for that
/// part only: no admit answer means admitted, no success estimates means the
/// rules' mode, no agent estimate means rotation order.
pub fn from_reply(
    spec: &WardenSpec,
    features: &Features,
    decider: &str,
    reply: &Reply,
) -> Decision {
    let mut raw = BTreeMap::new();
    // Calibrated by the spec, with the decider's own figure kept beside it.
    let mut yes = |name: &str| {
        let p = reply.yes(name)?;
        raw.insert(name.to_string(), p);
        Some(super::spec::calibrate(spec.calibration.get(name), p))
    };
    let p_admit = yes("admit");
    let mut success = BTreeMap::new();
    for mode in feasible(spec, features) {
        let name = format!("success:{}", mode.as_str());
        if let Some(p) = yes(&name) {
            success.insert(mode, p);
        }
    }
    let mut p_agent = reply.choice("agent").cloned().unwrap_or_default();
    for (agent, p) in p_agent.iter_mut() {
        let name = format!("agent:{agent}");
        raw.insert(name.clone(), *p);
        *p = super::spec::calibrate(spec.calibration.get(&name), *p);
    }
    let mut agents = names(features);
    // Stable: candidates the decider did not score keep rotation order.
    agents.sort_by(|a, b| {
        let pa = p_agent.get(a).copied().unwrap_or(-1.0);
        let pb = p_agent.get(b).copied().unwrap_or(-1.0);
        pb.partial_cmp(&pa).unwrap_or(std::cmp::Ordering::Equal)
    });
    let mut p_success = BTreeMap::new();
    let mut scores = BTreeMap::new();
    for (mode, p) in success {
        p_success.insert(mode.as_str().to_string(), p);
        scores.insert(mode.as_str().to_string(), p - spec.lambda * spec.cost(mode));
    }
    let admitted = p_admit.is_none_or(|p| p >= spec.admit_threshold);
    let best = scores
        .iter()
        .max_by(|a, b| a.1.partial_cmp(b.1).unwrap_or(std::cmp::Ordering::Equal))
        .and_then(|(name, _)| ModeKind::ALL.into_iter().find(|m| m.as_str() == name));
    let mode = best.or_else(|| rules_mode(spec, features));
    let (choice, reason) = match (admitted, mode) {
        (false, _) => (
            declined(agents),
            format!(
                "no reply needed (p={:.2} < {:.2})",
                p_admit.unwrap_or(0.0),
                spec.admit_threshold
            ),
        ),
        (true, None) => (declined(agents), "no listener can take it".to_string()),
        (true, Some(mode)) => {
            let why = match (best, p_success.get(mode.as_str())) {
                (Some(_), Some(p)) => format!("{} p={p:.2}", mode.as_str()),
                _ => format!("{} by the rules", mode.as_str()),
            };
            (choice(spec, features, mode, agents), why)
        }
    };
    Decision {
        choice,
        decider: decider.to_string(),
        spec_version: spec.version.clone(),
        p_admit,
        p_success,
        scores,
        p_agent,
        raw,
        reason,
        fallback: None,
        cost_usd: reply.cost_usd,
        latency_ms: Some(reply.latency_ms),
        input_tokens: reply.input_tokens,
        cached_tokens: reply.cached_tokens,
    }
}

#[cfg(test)]
mod tests {
    use super::super::decider::Answer;
    use super::*;

    fn features(len: usize, candidates: &[&str], ceiling: usize) -> Features {
        Features {
            message_id: "m".into(),
            text: "x".repeat(len),
            weighted_len: len,
            attachments: 0,
            context: vec![],
            candidates: candidates
                .iter()
                .map(|n| Candidate {
                    name: n.to_string(),
                    summary: format!("{n} card"),
                })
                .collect(),
            ceiling,
        }
    }

    fn reply(pairs: &[(&str, Answer)]) -> Reply {
        Reply {
            answers: pairs
                .iter()
                .map(|(k, v)| (k.to_string(), v.clone()))
                .collect(),
            latency_ms: 12,
            ..Reply::default()
        }
    }

    #[test]
    fn rules_send_short_messages_to_one_and_long_ones_to_a_vote() {
        let spec = WardenSpec::default();
        let short = by_rules(&spec, &features(40, &["a", "b"], 4), None);
        assert_eq!(short.choice.mode, Some(ModeKind::Single));
        assert_eq!(short.choice.listeners, 1);
        let long = by_rules(&spec, &features(800, &["a", "b", "c"], 4), None);
        assert_eq!(long.choice.mode, Some(ModeKind::Vote));
        assert_eq!((long.choice.listeners, long.choice.max_listeners), (2, 3));
    }

    #[test]
    fn a_vote_needs_two_listeners_and_a_ceiling_that_allows_them() {
        let spec = WardenSpec::default();
        // One listener configured, or a Circle that allows one per message:
        // the long message gets a single answer instead.
        for f in [features(800, &["a"], 4), features(800, &["a", "b"], 1)] {
            assert_eq!(
                by_rules(&spec, &f, None).choice.mode,
                Some(ModeKind::Single)
            );
        }
        // Two listeners: the vote is capped at two.
        let two = by_rules(&spec, &features(800, &["a", "b"], 4), None);
        assert_eq!((two.choice.listeners, two.choice.max_listeners), (2, 2));
    }

    #[test]
    fn only_feasible_modes_are_asked_about() {
        let spec = WardenSpec::default();
        let asked: Vec<String> = questions(&spec, &features(10, &["a"], 4))
            .into_iter()
            .map(|q| q.name)
            .collect();
        // One candidate: no agent question, and single is the only mode.
        assert_eq!(asked, vec!["admit"]);
        let asked: Vec<String> = questions(&spec, &features(10, &["a", "b"], 4))
            .into_iter()
            .map(|q| q.name)
            .collect();
        assert_eq!(
            asked,
            vec!["admit", "agent", "success:single", "success:vote"]
        );
    }

    #[test]
    fn a_low_admit_probability_declines() {
        let spec = WardenSpec::default();
        let d = from_reply(
            &spec,
            &features(100, &["a", "b"], 4),
            "jev",
            &reply(&[("admit", Answer::YesNo { p: 0.2 })]),
        );
        assert!(!d.choice.admit);
        assert_eq!(d.choice.listeners, 0);
        assert!(d.reason.contains("no reply needed"));
    }

    #[test]
    fn the_best_score_wins_after_paying_for_cost() {
        let spec = WardenSpec {
            lambda: 0.1,
            ..WardenSpec::default()
        };
        // single 0.70 − 0.10 = 0.60; vote 0.78 − 0.24 = 0.54: single wins.
        let d = from_reply(
            &spec,
            &features(100, &["a", "b"], 4),
            "jev",
            &reply(&[
                ("admit", Answer::YesNo { p: 0.9 }),
                ("success:single", Answer::YesNo { p: 0.70 }),
                ("success:vote", Answer::YesNo { p: 0.78 }),
            ]),
        );
        assert_eq!(d.choice.mode, Some(ModeKind::Single));
        // A big enough gain pays for the vote.
        let d = from_reply(
            &spec,
            &features(100, &["a", "b"], 4),
            "jev",
            &reply(&[
                ("success:single", Answer::YesNo { p: 0.40 }),
                ("success:vote", Answer::YesNo { p: 0.80 }),
            ]),
        );
        assert_eq!(d.choice.mode, Some(ModeKind::Vote));
        assert!(d.choice.admit, "no admit answer means admitted");
    }

    #[test]
    fn agents_are_ranked_by_the_decider_and_otherwise_keep_rotation_order() {
        let spec = WardenSpec::default();
        let mut p = BTreeMap::new();
        p.insert("c".to_string(), 0.6);
        p.insert("a".to_string(), 0.3);
        let d = from_reply(
            &spec,
            &features(10, &["a", "b", "c"], 4),
            "jev",
            &reply(&[("agent", Answer::Choice { p })]),
        );
        assert_eq!(d.choice.agents, vec!["c", "a", "b"]);
    }

    #[test]
    fn calibration_moves_the_decision_and_the_raw_figure_is_kept() {
        let mut spec = WardenSpec::default();
        // This decider says 0.6 for messages that turned out not to need anyone.
        spec.calibration
            .insert("admit".into(), vec![[0.0, 0.0], [0.7, 0.3], [1.0, 1.0]]);
        let d = from_reply(
            &spec,
            &features(100, &["a"], 4),
            "jev",
            &reply(&[("admit", Answer::YesNo { p: 0.6 })]),
        );
        assert!(!d.choice.admit, "0.6 calibrates below the threshold");
        assert_eq!(d.raw["admit"], 0.6);
        assert!(d.p_admit.unwrap() < 0.5);
    }

    #[test]
    fn refusals_fall_back_to_the_rules_part_by_part() {
        let spec = WardenSpec::default();
        let d = from_reply(
            &spec,
            &features(800, &["a", "b"], 4),
            "jev",
            &reply(&[
                ("admit", Answer::Refused),
                ("success:single", Answer::Refused),
            ]),
        );
        assert!(d.choice.admit);
        assert_eq!(d.choice.mode, Some(ModeKind::Vote), "the rules' long mode");
        assert!(d.reason.contains("by the rules"));
    }
}
