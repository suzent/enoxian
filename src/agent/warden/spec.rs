//! `warden_spec`: the policy the warden decides by.
//!
//! Data, not code. Everything here is a knob the warden reads and an experiment
//! can change — which ways of answering it may choose, how much it weighs cost,
//! where its gates sit, how its questions are worded — and nothing here can
//! widen what the warden is allowed to do. The invariants live in code: only a
//! person's message reaches it, a call by name bypasses it, the relay ceiling
//! and the device's concurrency hold whatever this says.
//!
//! One file per device, `~/.enoxian/warden/spec.toml`; absent, the built-in
//! default applies. A spec is replaced whole, after validation, so a proposed
//! change (from a person or from an RSI run) is reviewed as one diff and can be
//! put back the same way.

use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// A way of answering a message, as the warden chooses it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ModeKind {
    /// One listener answers.
    Single,
    /// Several listeners are offered the message together, as scry does today:
    /// each sees the others' replies and may veil its draft.
    Room,
    /// Blind vote: listeners answer without seeing one another; one answer is
    /// posted when they agree, another listener is added when they do not.
    Vote,
}

impl ModeKind {
    pub const ALL: [ModeKind; 3] = [ModeKind::Single, ModeKind::Room, ModeKind::Vote];

    pub fn as_str(self) -> &'static str {
        match self {
            ModeKind::Single => "single",
            ModeKind::Room => "room",
            ModeKind::Vote => "vote",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Gates {
    /// Messages shorter than this (in the weighted units of the scry length
    /// floor) are not offered at all. `0` hands even "ok" to the decider.
    pub min_chars: usize,
    /// An agent that spoke here within this many seconds is not chosen.
    pub quiet_secs: i64,
    /// Fraction of messages the gates turned away that the decider is still
    /// asked about, in shadow, so the gates themselves can be measured. Costs a
    /// decider call each; nothing is run.
    pub sample_skipped: f64,
}

impl Default for Gates {
    fn default() -> Self {
        Self {
            min_chars: 24,
            quiet_secs: 30,
            sample_skipped: 0.0,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Vote {
    /// Voters asked in the first round.
    pub start: usize,
    /// Voters at most, counting the first round.
    pub max: usize,
    /// Ask the decider whether two answers agree, when one is configured.
    /// Otherwise answers agree when their `ANSWER:` lines match.
    pub ask_decider: bool,
}

impl Default for Vote {
    fn default() -> Self {
        Self {
            start: 2,
            max: 3,
            ask_decider: true,
        }
    }
}

/// What each way of answering costs, relative to one listener answering once.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Costs {
    pub single: f64,
    /// Per listener offered.
    pub room_each: f64,
    /// Expected, with early stopping: two voters, a third on disagreement.
    pub vote: f64,
}

impl Default for Costs {
    fn default() -> Self {
        // `vote` from the T0 replay: vote_adaptive used 46.8k tokens per
        // question against 19.4k for single.
        Self {
            single: 1.0,
            room_each: 1.0,
            vote: 2.4,
        }
    }
}

/// The built-in rules, used by `builtin:rules` and as the fallback whenever a
/// decider cannot be asked.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Rules {
    /// At or above this weighted length a message is treated as hard: it gets
    /// `long_mode`, shorter ones get `short_mode`.
    pub long_chars: usize,
    pub short_mode: ModeKind,
    pub long_mode: ModeKind,
}

impl Default for Rules {
    fn default() -> Self {
        // T0: "short → single, long → vote_adaptive" matched sc3's accuracy at
        // 64% of its cost, using only a feature real rooms have.
        Self {
            long_chars: 400,
            short_mode: ModeKind::Single,
            long_mode: ModeKind::Vote,
        }
    }
}

/// How the questions put to a decider are worded. RSI may rewrite these; the
/// set of questions and what the daemon does with the answers stay in code.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Questions {
    pub admit: String,
    pub agent: String,
    pub single: String,
    pub room: String,
    pub vote: String,
    pub agree: String,
    pub answer_ok: String,
}

impl Default for Questions {
    fn default() -> Self {
        Self {
            admit: "This message was posted to a group chat shared by people and AI agents, and \
                    names no one. Does it need an answer from an AI agent — a question, a request, \
                    or a claim worth checking — rather than being chat between people?"
                .into(),
            agent: "Which of these agents is best placed to answer this message?".into(),
            single: "If one capable AI agent answered this message on its own, would its answer \
                     be correct and useful?"
                .into(),
            room: "If several AI agents answered this message together, each seeing the others' \
                   replies, would the result be correct and useful?"
                .into(),
            vote: "If two or three AI agents answered this message independently and the answer \
                   most of them agreed on were posted, would it be correct and useful?"
                .into(),
            agree: "Do these answers reach the same conclusion? Ignore wording, length and \
                    reasoning; compare only what each one concludes."
                .into(),
            answer_ok: "Does this answer get the question right and answer what was asked? Judge \
                        the conclusion, not the style."
                .into(),
        }
    }
}

/// A second look at a single answer, after it is posted.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Verify {
    /// Ask the decider whether a single listener's posted answer is right.
    /// Needs a decider that answers questions; the rules never verify.
    pub enabled: bool,
    /// Below this probability that the answer is right, the next listener is
    /// asked to check it.
    pub threshold: f64,
}

impl Default for Verify {
    fn default() -> Self {
        Self {
            enabled: true,
            threshold: 0.4,
        }
    }
}

/// A decider's raw probability for one question, mapped onto how often it
/// turned out right. Fitted outside enoxian, from episodes with outcomes, and
/// written here as points `[raw, calibrated]`, raw ascending; between points
/// the map is linear, beyond them it is flat.
pub type Calibration = std::collections::BTreeMap<String, Vec<[f64; 2]>>;

/// Apply `points` to `p`. No points, or a question with none, is the identity.
pub fn calibrate(points: Option<&Vec<[f64; 2]>>, p: f64) -> f64 {
    let Some(points) = points.filter(|v| !v.is_empty()) else {
        return p;
    };
    let first = points[0];
    let last = points[points.len() - 1];
    if p <= first[0] {
        return first[1];
    }
    if p >= last[0] {
        return last[1];
    }
    for pair in points.windows(2) {
        let ([x0, y0], [x1, y1]) = (pair[0], pair[1]);
        if p <= x1 {
            if x1 <= x0 {
                return y1;
            }
            return y0 + (y1 - y0) * (p - x0) / (x1 - x0);
        }
    }
    last[1]
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct WardenSpec {
    /// Recorded with every decision, so an outcome can be traced to the policy
    /// that produced it.
    pub version: String,
    /// The ways of answering the warden may choose. Not answering is always
    /// available and is not listed.
    pub modes: Vec<ModeKind>,
    /// Below this probability that the message needs an agent, nobody answers.
    pub admit_threshold: f64,
    /// Weight of cost against success: the warden picks the mode with the best
    /// `p(success) − lambda × cost`.
    pub lambda: f64,
    /// Listeners offered together in `room` mode.
    pub room_size: usize,
    pub vote: Vote,
    pub costs: Costs,
    pub gates: Gates,
    pub rules: Rules,
    pub questions: Questions,
    pub verify: Verify,
    /// Per question (`admit`, `agent:<name>`, `success:<mode>`, `answer_ok`).
    pub calibration: Calibration,
}

impl Default for WardenSpec {
    fn default() -> Self {
        Self {
            version: "builtin-1".into(),
            modes: vec![ModeKind::Single, ModeKind::Vote],
            admit_threshold: 0.5,
            lambda: 0.05,
            room_size: 2,
            vote: Vote::default(),
            costs: Costs::default(),
            gates: Gates::default(),
            rules: Rules::default(),
            questions: Questions::default(),
            verify: Verify::default(),
            calibration: Calibration::new(),
        }
    }
}

impl WardenSpec {
    /// Reject a spec that would make the warden do something nonsensical. The
    /// bounds are deliberately loose: they stop typos, not experiments.
    pub fn validate(&self) -> Result<(), String> {
        if self.version.trim().is_empty() {
            return Err("version is required".into());
        }
        if self.modes.is_empty() {
            return Err("modes must name at least one way of answering".into());
        }
        let unit = |name: &str, v: f64| {
            if (0.0..=1.0).contains(&v) {
                Ok(())
            } else {
                Err(format!("{name} must be between 0 and 1"))
            }
        };
        unit("admit_threshold", self.admit_threshold)?;
        unit("gates.sample_skipped", self.gates.sample_skipped)?;
        if !(0.0..=10.0).contains(&self.lambda) {
            return Err("lambda must be between 0 and 10".into());
        }
        if !(1..=8).contains(&self.room_size) {
            return Err("room_size must be 1–8".into());
        }
        if self.vote.start < 2 || self.vote.max < self.vote.start || self.vote.max > 5 {
            return Err("vote needs 2 ≤ start ≤ max ≤ 5".into());
        }
        for (name, cost) in [
            ("costs.single", self.costs.single),
            ("costs.room_each", self.costs.room_each),
            ("costs.vote", self.costs.vote),
        ] {
            if !(cost.is_finite() && cost >= 0.0) {
                return Err(format!("{name} must be a non-negative number"));
            }
        }
        if self.gates.min_chars > 2_000 {
            return Err("gates.min_chars must be at most 2000".into());
        }
        if !(0..=3_600).contains(&self.gates.quiet_secs) {
            return Err("gates.quiet_secs must be 0–3600".into());
        }
        if self.rules.long_chars == 0 {
            return Err("rules.long_chars must be at least 1".into());
        }
        unit("verify.threshold", self.verify.threshold)?;
        for (question, points) in &self.calibration {
            let mut previous: Option<[f64; 2]> = None;
            for point in points {
                if !point.iter().all(|v| (0.0..=1.0).contains(v)) {
                    return Err(format!("calibration.{question}: points must be within 0–1"));
                }
                if let Some(prev) = previous {
                    if point[0] < prev[0] || point[1] < prev[1] {
                        return Err(format!(
                            "calibration.{question}: points must rise with raw probability"
                        ));
                    }
                }
                previous = Some(*point);
            }
        }
        Ok(())
    }

    pub fn allows(&self, mode: ModeKind) -> bool {
        self.modes.contains(&mode)
    }

    /// The cost of answering this way, in units of one listener answering once.
    pub fn cost(&self, mode: ModeKind) -> f64 {
        match mode {
            ModeKind::Single => self.costs.single,
            ModeKind::Room => self.costs.room_each * self.room_size as f64,
            ModeKind::Vote => self.costs.vote,
        }
    }

    pub fn from_toml(text: &str) -> Result<Self, String> {
        let spec: Self = toml::from_str(text).map_err(|e| e.to_string())?;
        spec.validate()?;
        Ok(spec)
    }

    pub fn path() -> anyhow::Result<PathBuf> {
        Ok(crate::config::enoxian_dir()?
            .join("warden")
            .join("spec.toml"))
    }

    /// The spec in force, and where it came from. An invalid file is reported
    /// and ignored rather than half-applied: the default is a known policy.
    pub fn load() -> (Self, Source) {
        let Ok(path) = Self::path() else {
            return (Self::default(), Source::Default);
        };
        match std::fs::read_to_string(&path) {
            Ok(text) => match Self::from_toml(&text) {
                Ok(spec) => (spec, Source::File),
                Err(error) => {
                    tracing::warn!(
                        "[warden] {} is invalid ({error}); using the built-in spec",
                        path.display()
                    );
                    (Self::default(), Source::Invalid(error))
                }
            },
            Err(_) => (Self::default(), Source::Default),
        }
    }

    /// Replace the spec on disk. Validated first, so a bad spec never lands.
    pub fn save(&self) -> anyhow::Result<()> {
        self.validate().map_err(anyhow::Error::msg)?;
        let path = Self::path()?;
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let tmp = path.with_extension("toml.tmp");
        std::fs::write(&tmp, toml::to_string_pretty(self)?)?;
        std::fs::rename(&tmp, &path)?;
        Ok(())
    }

    /// Go back to the built-in spec.
    pub fn reset() -> anyhow::Result<()> {
        match std::fs::remove_file(Self::path()?) {
            Err(error) if error.kind() != std::io::ErrorKind::NotFound => Err(error.into()),
            _ => Ok(()),
        }
    }
}

/// Where the spec in force came from.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case", tag = "kind", content = "error")]
pub enum Source {
    Default,
    File,
    /// The file exists and is invalid; the default is in force.
    Invalid(String),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_default_is_valid_and_round_trips() {
        let spec = WardenSpec::default();
        spec.validate().unwrap();
        let text = toml::to_string_pretty(&spec).unwrap();
        assert_eq!(WardenSpec::from_toml(&text).unwrap(), spec);
    }

    #[test]
    fn a_partial_file_fills_in_the_rest() {
        let spec = WardenSpec::from_toml("version = \"t1\"\nlambda = 0.2\n").unwrap();
        assert_eq!(spec.version, "t1");
        assert_eq!(spec.lambda, 0.2);
        assert_eq!(spec.vote, Vote::default());
    }

    #[test]
    fn nonsense_is_refused() {
        for bad in [
            "admit_threshold = 1.5",
            "modes = []",
            "[vote]\nstart = 1",
            "[vote]\nstart = 3\nmax = 2",
            "room_size = 0",
            "[gates]\nsample_skipped = -0.1",
            "unknown_knob = 1",
            "[costs]\nvote = -1.0",
        ] {
            assert!(WardenSpec::from_toml(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn calibration_interpolates_and_is_flat_beyond_its_points() {
        let points = vec![[0.2, 0.1], [0.6, 0.3], [0.9, 0.9]];
        assert_eq!(calibrate(Some(&points), 0.0), 0.1);
        assert!((calibrate(Some(&points), 0.4) - 0.2).abs() < 1e-9);
        assert!((calibrate(Some(&points), 0.75) - 0.6).abs() < 1e-9);
        assert_eq!(calibrate(Some(&points), 1.0), 0.9);
        assert_eq!(calibrate(None, 0.42), 0.42);
        let spec =
            WardenSpec::from_toml("[calibration]\nadmit = [[0.1, 0.0], [0.9, 0.7]]\n").unwrap();
        assert_eq!(spec.calibration["admit"].len(), 2);
        assert!(
            WardenSpec::from_toml("[calibration]\nadmit = [[0.5, 0.6], [0.9, 0.2]]\n").is_err()
        );
        assert!(WardenSpec::from_toml("[calibration]\nadmit = [[0.5, 1.6]]\n").is_err());
    }

    #[test]
    fn room_cost_scales_with_its_size() {
        let spec = WardenSpec {
            room_size: 3,
            ..WardenSpec::default()
        };
        assert_eq!(spec.cost(ModeKind::Room), 3.0);
        assert_eq!(spec.cost(ModeKind::Single), 1.0);
    }
}
