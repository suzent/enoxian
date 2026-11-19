//! What a running turn is doing, and stopping one.
//!
//! ACP has no heartbeat and no way to ask an agent whether it is working. What
//! it does have is the stream the agent reports progress on: every tool call
//! is announced, moves to `in_progress` while it runs, and ends `completed` or
//! `failed`. A long build is a tool call in progress with nothing else said for
//! minutes, which is work, not a hang. So nothing here stops a turn for taking
//! long. It records what the agent last did, which tool calls are still open,
//! and when it last said anything, and leaves the judgement to a person — who
//! can stop the turn — or to an addressed request for the same agent, which
//! takes over from an ambient one (see `LiveRuns::preempt_ambient`).
//!
//! Stopping goes through `session/cancel`, which obliges the agent to wind down
//! and end the turn as `cancelled`; see `AcpSession::prompt_until`.

use crate::control::{RunProgress, ToolProgress};
use serde::Serialize;
use serde_json::Value;
use std::collections::{BTreeMap, VecDeque};
use std::path::Path;
use std::sync::{Arc, Mutex};
use tokio_util::sync::CancellationToken;

const MAX_RECENT_TOOLS: usize = 8;

/// A tool call the agent announced and has not finished.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct OpenTool {
    pub title: String,
    /// ACP tool kind (`read`, `edit`, `execute`, …) when the agent gave one.
    pub kind: Option<String>,
    /// `pending` or `in_progress`.
    pub status: String,
    /// When the agent announced it (Unix seconds).
    pub since: i64,
    /// The first of the call's ACP `locations`, as the agent gave it (an
    /// absolute path, per the spec). Kept for the room's summary, which
    /// reduces it to a workspace-relative path; not shown as is.
    #[serde(skip)]
    pub location: Option<String>,
    /// For an `execute` call, the program it runs, read from `rawInput`.
    #[serde(skip)]
    pub program: Option<String>,
}

/// A tool call the agent finished during this turn.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct CompletedTool {
    pub title: String,
    pub kind: Option<String>,
    /// `completed`, `failed`, or `cancelled`.
    pub status: String,
    pub duration_secs: i64,
    pub finished_at: i64,
}

/// One step from an ACP `plan` update.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct PlanStep {
    pub content: String,
    /// `pending`, `in_progress`, or `completed`.
    pub status: String,
}

/// What the agent in one turn has been doing.
#[derive(Debug, Clone, Default, Serialize)]
pub struct Activity {
    /// When the agent last sent anything (Unix seconds).
    pub last_activity_at: Option<i64>,
    /// What that was: a `session/update` kind, or the method it called.
    pub last_kind: Option<String>,
    /// Path from the most recent `fs/read_text_file` or `fs/write_text_file`.
    pub last_file: Option<String>,
    /// Tool calls announced and not yet finished, by tool call id.
    pub open_tools: BTreeMap<String, OpenTool>,
    /// Most recent finished tool calls, oldest first (bounded by `MAX_RECENT_TOOLS`).
    pub recent_tools: VecDeque<CompletedTool>,
    /// Current step-by-step plan, when the agent reported one via `session/update`.
    pub plan: Vec<PlanStep>,
}

impl Activity {
    /// Record one message from the agent, at `now`.
    pub fn observe(&mut self, msg: &Value, now: i64) {
        self.last_activity_at = Some(now);
        let method = msg.get("method").and_then(Value::as_str);
        if matches!(method, Some("fs/read_text_file" | "fs/write_text_file")) {
            if let Some(path) = msg
                .get("params")
                .and_then(|p| p.get("path"))
                .and_then(Value::as_str)
            {
                self.last_file = Some(path.to_string());
            }
        }
        let update = (method == Some("session/update"))
            .then(|| msg.get("params").and_then(|p| p.get("update")))
            .flatten();
        let Some(update) = update else {
            self.last_kind = method.map(str::to_string);
            return;
        };
        let kind = super::acp::update_kind(update).map(str::to_string);
        match kind.as_deref() {
            Some("tool_call") | Some("tool_call_update") => {
                self.observe_tool(update, kind.as_deref() == Some("tool_call"), now);
            }
            Some("plan") => {
                self.observe_plan(update);
            }
            _ => {}
        }
        self.last_kind = kind;
    }

    fn observe_plan(&mut self, update: &Value) {
        let Some(entries) = update.get("entries").and_then(Value::as_array) else {
            return;
        };
        self.plan = entries
            .iter()
            .filter_map(|e| {
                let content = e.get("content").and_then(Value::as_str)?.trim();
                if content.is_empty() {
                    return None;
                }
                let status = e
                    .get("status")
                    .and_then(Value::as_str)
                    .unwrap_or("pending")
                    .to_string();
                Some(PlanStep {
                    content: content.to_string(),
                    status,
                })
            })
            .collect();
    }

    fn observe_tool(&mut self, update: &Value, announced: bool, now: i64) {
        let Some(id) = update.get("toolCallId").and_then(Value::as_str) else {
            return;
        };
        let text = |key: &str| update.get(key).and_then(Value::as_str).map(str::to_string);
        let status = update.get("status").and_then(Value::as_str);
        if let Some(done @ ("completed" | "failed" | "cancelled")) = status {
            if let Some(open) = self.open_tools.remove(id) {
                if self.recent_tools.len() >= MAX_RECENT_TOOLS {
                    self.recent_tools.pop_front();
                }
                self.recent_tools.push_back(CompletedTool {
                    title: text("title").unwrap_or(open.title),
                    kind: text("kind").or(open.kind),
                    status: done.to_string(),
                    duration_secs: (now - open.since).max(0),
                    finished_at: now,
                });
            }
            return;
        }
        // ACP: a `tool_call_update` carries only the fields that changed, and
        // an omitted one keeps its earlier value.
        let location = first_location(update);
        let program = update.get("rawInput").and_then(program_of);
        match self.open_tools.get_mut(id) {
            Some(open) => {
                if let Some(status) = status {
                    open.status = status.to_string();
                }
                if let Some(title) = text("title") {
                    open.title = title;
                }
                if let Some(kind) = text("kind") {
                    open.kind = Some(kind);
                }
                if location.is_some() {
                    open.location = location;
                }
                if program.is_some() {
                    open.program = program;
                }
            }
            // An update for a call we never saw announced still means it is
            // running; record it rather than lose it.
            None if announced || status.is_some() => {
                self.open_tools.insert(
                    id.to_string(),
                    OpenTool {
                        title: text("title").unwrap_or_else(|| "tool call".into()),
                        kind: text("kind"),
                        status: status.unwrap_or("pending").to_string(),
                        since: now,
                        location,
                        program,
                    },
                );
            }
            None => {}
        }
    }
}

impl Activity {
    /// What the room is told about this turn: see [`RunProgress`].
    ///
    /// Built from the ACP updates already recorded, reduced to what may leave
    /// the device: no tool titles, no arguments, no paths outside `workspace`.
    pub fn progress(&self, started_at: i64, workspace: &Path) -> RunProgress {
        // The call it is in: a running one over one still waiting for input or
        // approval, then the most recently announced.
        let tool = self
            .open_tools
            .values()
            .max_by_key(|t| (t.status == "in_progress", t.since))
            .map(|t| {
                let kind = tool_kind(t.kind.as_deref());
                ToolProgress {
                    program: (kind == "execute").then(|| t.program.clone()).flatten(),
                    path: t
                        .location
                        .as_deref()
                        .and_then(|p| workspace_relative(p, workspace)),
                    kind: kind.to_string(),
                    since: t.since,
                }
            });
        let count = |n: usize| u16::try_from(n).unwrap_or(u16::MAX);
        RunProgress {
            started_at,
            last_activity_at: self.last_activity_at,
            tool,
            open_tools: count(self.open_tools.len()),
            plan_done: count(
                self.plan
                    .iter()
                    .filter(|step| step.status == "completed")
                    .count(),
            ),
            plan_total: count(self.plan.len()),
            plan_step: self
                .plan
                .iter()
                .find(|step| step.status == "in_progress")
                .map(|step| clip(&step.content, PLAN_STEP_LIMIT)),
        }
    }
}

/// How much of the plan entry in progress the room sees.
const PLAN_STEP_LIMIT: usize = 80;

/// How much of a workspace path the room sees.
const PATH_LIMIT: usize = 120;

/// The ACP `ToolKind`s. Anything else, or none, is `other`, as the spec's
/// default says.
const TOOL_KINDS: [&str; 10] = [
    "read",
    "edit",
    "delete",
    "move",
    "search",
    "execute",
    "think",
    "fetch",
    "switch_mode",
    "other",
];

fn tool_kind(kind: Option<&str>) -> &'static str {
    kind.and_then(|k| TOOL_KINDS.iter().find(|known| **known == k))
        .copied()
        .unwrap_or("other")
}

/// The path of a tool call's first ACP location, if it gave one.
fn first_location(update: &Value) -> Option<String> {
    update
        .get("locations")?
        .as_array()?
        .first()?
        .get("path")?
        .as_str()
        .filter(|p| !p.is_empty())
        .map(str::to_string)
}

/// `path` relative to `workspace`, or `None` when it is outside it. ACP sends
/// absolute paths; a relative one is taken as already relative to the
/// workspace. Nothing that climbs out with `..` is kept.
fn workspace_relative(path: &str, workspace: &Path) -> Option<String> {
    let path = Path::new(path);
    let relative = if path.is_absolute() {
        path.strip_prefix(workspace).ok()?
    } else {
        path
    };
    if relative.as_os_str().is_empty()
        || relative
            .components()
            .any(|c| !matches!(c, std::path::Component::Normal(_)))
    {
        return None;
    }
    Some(clip(
        &relative.to_string_lossy().replace('\\', "/"),
        PATH_LIMIT,
    ))
}

/// The program an `execute` call runs, from its `rawInput`, or `None`.
///
/// `rawInput` is whatever the agent's tool takes, so this reads the shapes
/// agents actually send — `{"command": "cargo test"}` and
/// `{"command": ["bash", "-lc", "cargo test"]}` — and gives up on anything
/// else. Only the program's file name is kept, and only when it looks like
/// one: arguments are where secrets live.
fn program_of(raw: &Value) -> Option<String> {
    let line = match raw.get("command")? {
        Value::String(line) => line.clone(),
        Value::Array(argv) => {
            let argv: Vec<&str> = argv.iter().filter_map(Value::as_str).collect();
            match argv.as_slice() {
                // A shell running a script: the script is the command.
                [shell, flag, script, ..]
                    if matches!(
                        program_name(shell).as_deref(),
                        Some("sh" | "bash" | "zsh" | "dash" | "fish")
                    ) && flag.starts_with('-')
                        && flag.contains('c') =>
                {
                    script.to_string()
                }
                [program, ..] => return program_name(program),
                [] => return None,
            }
        }
        _ => return None,
    };
    first_program(&line)
}

/// Characters past which a command line is not read: quoting, escapes and
/// expansions. Splitting on whitespace is only sound before the first of
/// them — `PASSWORD='a sk-secret b' curl …` otherwise yields `sk-secret` as
/// the "program" — so the line is cut there, and a program word the cut runs
/// into is not trusted either.
const SHELL_SPECIAL: [char; 8] = ['\'', '"', '\\', '$', '`', '(', ')', '#'];

/// The first program a plain shell line runs: the first step that is not
/// `cd`, past any `VAR=value` prefixes. `None` whenever that cannot be read
/// without parsing shell quoting.
fn first_program(line: &str) -> Option<String> {
    let cut = line.find(SHELL_SPECIAL).unwrap_or(line.len());
    let plain = &line[..cut];
    let truncated = cut < line.len();
    let (word, end) = plain
        .split(['&', ';', '|', '\n'])
        .scan(0usize, |offset, step| {
            let start = *offset;
            *offset += step.len() + 1;
            Some((start, step))
        })
        .filter_map(|(start, step)| {
            let mut at = 0;
            step.split_whitespace()
                .map(|word| {
                    at = step[at..].find(word).map_or(at, |i| at + i) + word.len();
                    (word, start + at)
                })
                .find(|(word, _)| !word.contains('='))
        })
        .find(|(word, _)| *word != "cd")?;
    // `cargo"x"` cut to `cargo`: the word went on past what was read.
    if truncated && end == cut {
        return None;
    }
    program_name(word)
}

/// The file name of `word` if it reads as a program name.
fn program_name(word: &str) -> Option<String> {
    let name = word.rsplit(['/', '\\']).next()?;
    let ok = !name.is_empty()
        && name.len() <= 32
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-' | '+'));
    ok.then(|| name.to_string())
}

/// `text` cut to `limit` characters, marked when cut.
fn clip(text: &str, limit: usize) -> String {
    let text = text.trim();
    if text.chars().count() <= limit {
        return text.to_string();
    }
    let kept: String = text.chars().take(limit.saturating_sub(1)).collect();
    format!("{}…", kept.trim_end())
}

/// Why a turn was stopped.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StopCause {
    /// Someone stopped it by hand.
    Requested,
    /// An addressed request for the same agent arrived while this unaddressed
    /// turn held its conversation.
    Preempted,
}

impl StopCause {
    /// What the run's record says.
    pub fn detail(&self) -> &'static str {
        match self {
            StopCause::Requested => "stopped by user",
            StopCause::Preempted => "stopped: an addressed request for this agent took over",
        }
    }
}

/// A turn that ended because it was stopped, not because it finished.
#[derive(Debug)]
pub struct TurnStopped(pub StopCause);

impl std::fmt::Display for TurnStopped {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.0.detail())
    }
}

impl std::error::Error for TurnStopped {}

/// A turn in progress on this device.
#[derive(Debug)]
pub struct LiveRun {
    pub agent: String,
    pub ambient: bool,
    pub started_at: i64,
    pub activity: Arc<Mutex<Activity>>,
    pub usage: Arc<Mutex<super::usage::Meter>>,
    stop: CancellationToken,
    cause: Mutex<Option<StopCause>>,
}

impl LiveRun {
    /// Ask the turn to stop. The first cause asked for is the one recorded.
    pub fn request_stop(&self, cause: StopCause) {
        {
            let mut current = self.cause.lock().unwrap();
            if current.is_none() {
                *current = Some(cause);
            }
        }
        self.stop.cancel();
    }

    pub fn stop_token(&self) -> &CancellationToken {
        &self.stop
    }

    pub fn stop_cause(&self) -> Option<StopCause> {
        self.cause.lock().unwrap().clone()
    }

    /// What the activity panel and `enox runs` show.
    pub fn snapshot(&self, now: i64) -> Value {
        let activity = self.activity.lock().unwrap().clone();
        serde_json::json!({
            "started_at": self.started_at,
            "last_activity_at": activity.last_activity_at,
            "idle_secs": now - activity.last_activity_at.unwrap_or(self.started_at),
            "last_kind": activity.last_kind,
            "last_file": activity.last_file,
            "open_tools": activity.open_tools.values().collect::<Vec<_>>(),
            "recent_tools": activity.recent_tools,
            "plan": activity.plan,
            "stopping": self.stop.is_cancelled(),
        })
    }
}

/// The turns running in one Circle on this device, by run id.
#[derive(Debug, Clone, Default)]
pub struct LiveRuns {
    runs: Arc<dashmap::DashMap<String, Arc<LiveRun>>>,
    /// Agents an addressed request is waiting on, asked to give way before
    /// their unaddressed turn was listed. A turn is listed only once it has a
    /// device slot, and an agent is busy from the moment the worker takes its
    /// turn — so a request can arrive in between, find nothing to stop, and
    /// nothing would ask again. The request is remembered and served when the
    /// turn is listed. Keys are lowercase.
    waiting: Arc<dashmap::DashSet<String>>,
}

impl LiveRuns {
    /// Record a turn as running until the returned guard drops. An
    /// unaddressed turn an addressed request is already waiting on is stopped
    /// at once; an addressed one serves that request.
    pub fn register(&self, run_id: &str, agent: &str, ambient: bool, now: i64) -> LiveRunGuard {
        let run = Arc::new(LiveRun {
            agent: agent.to_string(),
            ambient,
            started_at: now,
            activity: Arc::default(),
            usage: Arc::default(),
            stop: CancellationToken::new(),
            cause: Mutex::new(None),
        });
        if self.waiting.remove(&agent.to_lowercase()).is_some() && ambient {
            tracing::info!("[agent] stopping {agent}'s unaddressed turn {run_id}: an addressed request is waiting");
            run.request_stop(StopCause::Preempted);
        }
        self.runs.insert(run_id.to_string(), run.clone());
        LiveRunGuard {
            runs: self.clone(),
            run_id: run_id.to_string(),
            run,
        }
    }

    /// Forget a waiting request for `agent`: its turn ended without being
    /// listed, so there is nothing left for the request to stop.
    pub fn clear_waiting(&self, agent: &str) {
        self.waiting.remove(&agent.to_lowercase());
    }

    pub fn get(&self, run_id: &str) -> Option<Arc<LiveRun>> {
        self.runs.get(run_id).map(|entry| entry.value().clone())
    }

    /// Any turn `agent` is running. At most one per Circle: a conversation
    /// takes one turn at a time.
    pub fn get_by_agent(&self, agent: &str) -> Option<Arc<LiveRun>> {
        self.runs
            .iter()
            .find(|entry| entry.value().agent.eq_ignore_ascii_case(agent))
            .map(|entry| entry.value().clone())
    }

    /// Stop `agent`'s unaddressed turn so an addressed request can have its
    /// conversation. Returns whether it asked a listed turn to stop. With no
    /// turn listed for the agent, the request is remembered for when one is
    /// (see `waiting`).
    pub fn preempt_ambient(&self, agent: &str) -> bool {
        let mut stopped = false;
        let mut busy = false;
        for entry in self.runs.iter() {
            let run = entry.value();
            if !run.agent.eq_ignore_ascii_case(agent) {
                continue;
            }
            busy = true;
            if run.ambient && !run.stop.is_cancelled() {
                tracing::info!(
                    "[agent] stopping {agent}'s unaddressed turn {}: an addressed request is waiting",
                    entry.key()
                );
                run.request_stop(StopCause::Preempted);
                stopped = true;
            }
        }
        if !busy {
            self.waiting.insert(agent.to_lowercase());
        }
        stopped
    }
}

/// Keeps a turn listed while it runs.
pub struct LiveRunGuard {
    runs: LiveRuns,
    run_id: String,
    run: Arc<LiveRun>,
}

impl LiveRunGuard {
    pub fn run(&self) -> &Arc<LiveRun> {
        &self.run
    }
}

impl Drop for LiveRunGuard {
    fn drop(&mut self) {
        self.runs.runs.remove(&self.run_id);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// A workspace root that is absolute on every platform CI runs on.
    fn ws() -> std::path::PathBuf {
        if cfg!(windows) {
            std::path::PathBuf::from(r"C:\Users\suzy\circle")
        } else {
            std::path::PathBuf::from("/home/suzy/circle")
        }
    }

    /// `rel` under [`ws`], as the agent would send it in a location.
    fn abs(rel: &str) -> String {
        ws().join(rel).to_string_lossy().into_owned()
    }

    fn update(update: Value) -> Value {
        json!({"method": "session/update", "params": {"sessionId": "s", "update": update}})
    }

    #[test]
    fn a_tool_call_stays_open_until_it_finishes() {
        let mut a = Activity::default();
        a.observe(
            &update(json!({"sessionUpdate": "tool_call", "toolCallId": "t1",
                "title": "cargo test", "kind": "execute", "status": "pending"})),
            100,
        );
        a.observe(
            &update(
                json!({"sessionUpdate": "tool_call_update", "toolCallId": "t1",
                "status": "in_progress"}),
            ),
            101,
        );
        let open = &a.open_tools["t1"];
        assert_eq!(open.title, "cargo test");
        assert_eq!(open.status, "in_progress");
        assert_eq!(open.since, 100);
        assert_eq!(a.last_activity_at, Some(101));

        a.observe(
            &update(
                json!({"sessionUpdate": "tool_call_update", "toolCallId": "t1",
                "status": "completed"}),
            ),
            500,
        );
        assert!(a.open_tools.is_empty());
        assert_eq!(a.last_kind.as_deref(), Some("tool_call_update"));
        assert_eq!(a.recent_tools.len(), 1);
        assert_eq!(a.recent_tools[0].title, "cargo test");
        assert_eq!(a.recent_tools[0].kind.as_deref(), Some("execute"));
        assert_eq!(a.recent_tools[0].status, "completed");
        assert_eq!(a.recent_tools[0].duration_secs, 400);
    }

    #[test]
    fn records_plan_updates_and_file_access() {
        let mut a = Activity::default();
        a.observe(
            &json!({"method": "fs/read_text_file", "id": 3, "params": {"path": "src/main.rs"}}),
            42,
        );
        assert_eq!(a.last_file.as_deref(), Some("src/main.rs"));

        a.observe(
            &update(json!({
                "sessionUpdate": "plan",
                "entries": [
                    {"content": "Inspect liveness", "status": "completed"},
                    {"content": "Update frontend", "status": "in_progress"}
                ]
            })),
            50,
        );
        assert_eq!(a.plan.len(), 2);
        assert_eq!(a.plan[0].content, "Inspect liveness");
        assert_eq!(a.plan[0].status, "completed");
        assert_eq!(a.plan[1].status, "in_progress");
    }

    #[test]
    fn any_message_from_the_agent_counts_as_activity() {
        let mut a = Activity::default();
        a.observe(
            &json!({"method": "fs/read_text_file", "id": 3, "params": {}}),
            42,
        );
        assert_eq!(a.last_activity_at, Some(42));
        assert_eq!(a.last_kind.as_deref(), Some("fs/read_text_file"));
    }

    #[test]
    fn preemption_stops_only_that_agents_unaddressed_turn() {
        let runs = LiveRuns::default();
        let ambient = runs.register("r1", "claude", true, 0);
        let addressed = runs.register("r2", "Claude", false, 0);
        let other = runs.register("r3", "codex", true, 0);

        assert!(runs.preempt_ambient("claude"));

        assert_eq!(ambient.run().stop_cause(), Some(StopCause::Preempted));
        assert!(ambient.run().stop_token().is_cancelled());
        assert!(!addressed.run().stop_token().is_cancelled());
        assert!(!other.run().stop_token().is_cancelled());
        assert!(!runs.preempt_ambient("claude"), "already stopping");
    }

    /// The race the worker has: its turn is taken before it is listed, so a
    /// request can arrive with nothing to stop yet. It must not be lost.
    #[test]
    fn a_request_that_arrives_before_the_turn_is_listed_stops_it_once_it_is() {
        let runs = LiveRuns::default();
        assert!(
            !runs.preempt_ambient("claude"),
            "nothing listed to stop yet"
        );
        let aside = runs.register("r1", "Claude", true, 0);
        assert_eq!(aside.run().stop_cause(), Some(StopCause::Preempted));

        // Served once: the next aside is not stopped for the same request.
        drop(aside);
        let next = runs.register("r2", "claude", true, 0);
        assert!(!next.run().stop_token().is_cancelled());
    }

    #[test]
    fn a_remembered_request_is_served_by_the_addressed_turn_itself() {
        let runs = LiveRuns::default();
        runs.preempt_ambient("claude");
        let addressed = runs.register("r1", "claude", false, 0);
        assert!(!addressed.run().stop_token().is_cancelled());
        drop(addressed);
        let aside = runs.register("r2", "claude", true, 0);
        assert!(
            !aside.run().stop_token().is_cancelled(),
            "nothing left waiting"
        );
    }

    #[test]
    fn a_request_is_not_remembered_while_an_addressed_turn_runs_or_once_cleared() {
        let runs = LiveRuns::default();
        let addressed = runs.register("r1", "claude", false, 0);
        runs.preempt_ambient("claude");
        drop(addressed);
        assert!(!runs
            .register("r2", "claude", true, 0)
            .run()
            .stop_token()
            .is_cancelled());

        runs.preempt_ambient("codex");
        runs.clear_waiting("codex");
        assert!(!runs
            .register("r3", "codex", true, 0)
            .run()
            .stop_token()
            .is_cancelled());
    }

    #[test]
    fn the_first_stop_cause_is_the_one_recorded() {
        let runs = LiveRuns::default();
        let guard = runs.register("r1", "claude", true, 0);
        guard.run().request_stop(StopCause::Requested);
        guard.run().request_stop(StopCause::Preempted);
        assert_eq!(guard.run().stop_cause(), Some(StopCause::Requested));
    }

    #[test]
    fn a_finished_turn_is_no_longer_listed() {
        let runs = LiveRuns::default();
        drop(runs.register("r1", "claude", true, 0));
        assert!(runs.get("r1").is_none());
    }

    /// What the room sees of a turn is a summary built from the ACP updates,
    /// never the titles: those carry whole command lines.
    #[test]
    fn progress_names_the_kind_program_and_workspace_path_only() {
        let workspace = ws();
        let workspace = workspace.as_path();
        let mut a = Activity::default();
        a.observe(
            &update(json!({"sessionUpdate": "tool_call", "toolCallId": "t1",
                "title": "curl -H 'Authorization: Bearer sk-secret' https://x", "kind": "execute",
                "status": "pending",
                "rawInput": {"command": "TOKEN=sk-secret curl -H 'Authorization: Bearer sk-secret' https://x"}})),
            100,
        );
        // ACP: the update carries only what changed; kind and rawInput stay.
        a.observe(
            &update(
                json!({"sessionUpdate": "tool_call_update", "toolCallId": "t1",
                "status": "in_progress"}),
            ),
            101,
        );
        let p = a.progress(90, workspace);
        let tool = p.tool.clone().unwrap();
        assert_eq!(tool.kind, "execute");
        assert_eq!(tool.program.as_deref(), Some("curl"));
        assert_eq!(tool.since, 100);
        let wire = serde_json::to_string(&p).unwrap();
        assert!(!wire.contains("sk-secret"), "{wire}");
        assert!(!wire.contains("Authorization"), "{wire}");

        a.observe(
            &update(json!({"sessionUpdate": "tool_call", "toolCallId": "t2",
                "title": format!("Read {}", abs("notes/plan.md")), "kind": "read",
                "status": "in_progress",
                "locations": [{"path": abs("notes/plan.md"), "line": 3}]})),
            102,
        );
        let p = a.progress(90, workspace);
        assert_eq!(p.open_tools, 2);
        let tool = p.tool.unwrap();
        assert_eq!(tool.kind, "read");
        assert_eq!(tool.path.as_deref(), Some("notes/plan.md"));
        assert_eq!(tool.program, None, "only execute calls name a program");
    }

    #[test]
    fn paths_outside_the_workspace_and_unknown_kinds_are_not_named() {
        let workspace = ws();
        let outside = workspace
            .parent()
            .unwrap()
            .join(".ssh")
            .join("id_ed25519")
            .to_string_lossy()
            .into_owned();
        for (path, kept) in [
            (outside.as_str(), None),
            (&abs("../secret"), None),
            ("../secret", None),
            ("src/lib.rs", Some("src/lib.rs")),
            (&abs("src/lib.rs"), Some("src/lib.rs")),
            (&workspace.to_string_lossy(), None),
        ] {
            assert_eq!(
                workspace_relative(path, &workspace).as_deref(),
                kept,
                "{path}"
            );
        }
        assert_eq!(tool_kind(Some("teleport")), "other");
        assert_eq!(tool_kind(None), "other");
        assert_eq!(tool_kind(Some("switch_mode")), "switch_mode");
    }

    #[test]
    fn the_program_is_read_from_the_shapes_agents_send() {
        let program = |raw: Value| program_of(&raw);
        assert_eq!(
            program(json!({"command": "cargo test --locked"})).as_deref(),
            Some("cargo")
        );
        assert_eq!(
            program(json!({"command": "cd crates/x && cargo build"})).as_deref(),
            Some("cargo")
        );
        assert_eq!(
            program(json!({"command": ["bash", "-lc", "npm run build"]})).as_deref(),
            Some("npm")
        );
        assert_eq!(
            program(json!({"command": ["/usr/bin/git", "status"]})).as_deref(),
            Some("git")
        );
        assert_eq!(
            program(json!({"command": "FOO=1 ./scripts/pre-push.sh"})).as_deref(),
            Some("pre-push.sh")
        );
        // Quoted values are not split into words (a secret can look like a
        // program name), and a word the quoting runs into is not trusted.
        assert_eq!(
            program(json!({"command": "PASSWORD='prefix sk-secret suffix' curl https://x"})),
            None
        );
        assert_eq!(
            program(json!({"command": "TOKEN=\"a sk-secret\" curl"})),
            None
        );
        assert_eq!(
            program(json!({"command": "cd \"a;sk-secret b\" && cargo test"})),
            None
        );
        assert_eq!(program(json!({"command": "cargo\"x\" test"})), None);
        assert_eq!(
            program(json!({"command": ["bash", "-c", "K='x y' make"]})),
            None
        );
        // Quoting after the program is fine: the program came first.
        assert_eq!(
            program(json!({"command": "curl -H 'Authorization: Bearer sk-secret' https://x"}))
                .as_deref(),
            Some("curl")
        );
        assert_eq!(
            program(json!({"command": "git commit -m \"a b\""})).as_deref(),
            Some("git")
        );
        // Not a program name: dropped rather than guessed at.
        assert_eq!(program(json!({"command": "$(echo sk-secret)"})), None);
        assert_eq!(program(json!({"cmd": "cargo test"})), None);
        assert_eq!(program(json!("cargo test")), None);
    }

    #[test]
    fn progress_counts_the_plan_and_names_the_step_in_progress() {
        let mut a = Activity::default();
        a.observe(
            &update(json!({"sessionUpdate": "plan", "entries": [
                {"content": "Read the spec", "priority": "high", "status": "completed"},
                {"content": "Write the tests", "priority": "high", "status": "in_progress"},
                {"content": "Ship it", "priority": "low", "status": "pending"}]})),
            100,
        );
        let p = a.progress(90, Path::new("/w"));
        assert_eq!((p.plan_done, p.plan_total), (1, 3));
        assert_eq!(p.plan_step.as_deref(), Some("Write the tests"));
        assert_eq!(p.tool, None);
        assert_eq!(p.last_activity_at, Some(100));

        // Each plan update replaces the whole plan (ACP).
        a.observe(
            &update(json!({"sessionUpdate": "plan", "entries": [
                {"content": "x".repeat(200), "priority": "high", "status": "in_progress"}]})),
            101,
        );
        let p = a.progress(90, Path::new("/w"));
        assert_eq!((p.plan_done, p.plan_total), (0, 1));
        assert_eq!(p.plan_step.unwrap().chars().count(), PLAN_STEP_LIMIT);
    }
}
