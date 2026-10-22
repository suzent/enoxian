//! The warden's ledger: what it decided about each message, what actually ran,
//! and how a vote went. One JSON object per line in `<circle>/warden.jsonl`.
//!
//! Local, like the execution inbox: these are this device's decisions about
//! what to spend, and nothing here is synced. It is the raw material for
//! judging the warden — joined with what became of each run, it is an episode
//! (see `episodes`) — so it records the inputs the warden saw as well as what it
//! chose, and it is append-only: a decision is never rewritten after the fact.

use super::policy::{Decision, Features};
use serde::{Deserialize, Serialize};
use std::io::Write;
use std::path::{Path, PathBuf};

/// Past this size the file is moved aside to `warden.1.jsonl`, replacing any
/// older one. Two generations bound the disk use without losing the recent
/// past mid-experiment.
const ROTATE_BYTES: u64 = 8 * 1024 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Phase {
    /// Decided and recorded only; rotation chose who ran.
    Shadow,
    /// Decided and carried out.
    Act,
    /// A message the gates turned away, asked about anyway to measure them.
    Sampled,
    /// A round of a blind vote was settled.
    Vote,
    /// A single answer was checked after it was posted.
    Verify,
}

/// What actually ran for a message.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Executed {
    /// `rotation`, `warden`, or `nobody`.
    pub by: String,
    pub agents: Vec<String>,
}

/// One voter's answer in a settled round.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Ballot {
    pub agent: String,
    /// The conclusion compared for agreement; `None` for a pass or a failure.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub conclusion: Option<String>,
    /// `answered`, `passed`, or `failed`.
    pub result: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct VoteRound {
    pub ballots: Vec<Ballot>,
    /// Groups of answers that agree with one another.
    pub groups: Vec<Vec<String>>,
    /// How agreement was judged: `answer_line` or the decider id.
    pub judged_by: String,
    /// `posted`, `added_voter`, or `nothing`.
    pub action: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub posted_by: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub added: Option<String>,
}

/// The second look at a single answer.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Verification {
    pub answered_by: String,
    /// Probability the answer is right, calibrated by the spec.
    pub p_ok: f64,
    /// What the decider said before calibration.
    pub raw: f64,
    pub threshold: f64,
    pub decider: String,
    /// The listener asked to check it, when the answer fell below threshold.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub checker: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cost_usd: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub latency_ms: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Record {
    pub ts: i64,
    pub message_id: String,
    pub phase: Phase,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub features: Option<Features>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub decision: Option<Decision>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub executed: Option<Executed>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub vote: Option<VoteRound>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub verify: Option<Verification>,
}

pub fn path(circle_dir: &Path) -> PathBuf {
    circle_dir.join("warden.jsonl")
}

fn previous(circle_dir: &Path) -> PathBuf {
    circle_dir.join("warden.1.jsonl")
}

/// Serializes appends within this process; lines are short enough that one
/// `write_all` per record keeps them whole across processes too.
static WRITE: std::sync::Mutex<()> = std::sync::Mutex::new(());

pub fn append(circle_dir: &Path, record: &Record) {
    let _guard = WRITE.lock().unwrap_or_else(|e| e.into_inner());
    let path = path(circle_dir);
    if std::fs::metadata(&path).is_ok_and(|m| m.len() > ROTATE_BYTES) {
        let _ = std::fs::rename(&path, previous(circle_dir));
    }
    let result = (|| -> std::io::Result<()> {
        let mut line = serde_json::to_vec(record)?;
        line.push(b'\n');
        std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&path)?
            .write_all(&line)
    })();
    if let Err(error) = result {
        tracing::warn!("[warden] could not record a decision: {error}");
    }
}

/// Every record still on disk, oldest first. A line that does not parse — a
/// torn write, a newer version's record — is skipped, not fatal.
pub fn read_all(circle_dir: &Path) -> Vec<Record> {
    [previous(circle_dir), path(circle_dir)]
        .iter()
        .filter_map(|p| std::fs::read_to_string(p).ok())
        .flat_map(|text| {
            text.lines()
                .filter_map(|l| serde_json::from_str::<Record>(l).ok())
                .collect::<Vec<_>>()
        })
        .collect()
}

/// The most recent `limit` records, newest first.
pub fn recent(circle_dir: &Path, limit: usize) -> Vec<Record> {
    let mut all = read_all(circle_dir);
    all.reverse();
    all.truncate(limit);
    all
}

#[cfg(test)]
mod tests {
    use super::*;

    fn record(id: &str, phase: Phase) -> Record {
        Record {
            ts: 1,
            message_id: id.into(),
            phase,
            features: None,
            decision: None,
            executed: Some(Executed {
                by: "rotation".into(),
                agents: vec!["claude".into()],
            }),
            vote: None,
            verify: None,
        }
    }

    #[test]
    fn records_come_back_in_order_and_torn_lines_are_skipped() {
        let dir = tempfile::tempdir().unwrap();
        append(dir.path(), &record("a", Phase::Shadow));
        std::fs::OpenOptions::new()
            .append(true)
            .open(path(dir.path()))
            .unwrap()
            .write_all(b"{\"ts\":2,\"mess\n")
            .unwrap();
        append(dir.path(), &record("b", Phase::Act));
        let all = read_all(dir.path());
        assert_eq!(
            all.iter()
                .map(|r| r.message_id.as_str())
                .collect::<Vec<_>>(),
            ["a", "b"]
        );
        assert_eq!(recent(dir.path(), 1)[0].message_id, "b");
    }
}
