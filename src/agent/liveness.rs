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

use serde::Serialize;
use serde_json::Value;
use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};
use tokio_util::sync::CancellationToken;

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
}

/// What the agent in one turn has been doing.
#[derive(Debug, Clone, Default, Serialize)]
pub struct Activity {
    /// When the agent last sent anything (Unix seconds).
    pub last_activity_at: Option<i64>,
    /// What that was: a `session/update` kind, or the method it called.
    pub last_kind: Option<String>,
    /// Tool calls announced and not yet finished, by tool call id.
    pub open_tools: BTreeMap<String, OpenTool>,
}

impl Activity {
    /// Record one message from the agent, at `now`.
    pub fn observe(&mut self, msg: &Value, now: i64) {
        self.last_activity_at = Some(now);
        let method = msg.get("method").and_then(Value::as_str);
        let update = (method == Some("session/update"))
            .then(|| msg.get("params").and_then(|p| p.get("update")))
            .flatten();
        let Some(update) = update else {
            self.last_kind = method.map(str::to_string);
            return;
        };
        let kind = super::acp::update_kind(update).map(str::to_string);
        if matches!(
            kind.as_deref(),
            Some("tool_call") | Some("tool_call_update")
        ) {
            self.observe_tool(update, kind.as_deref() == Some("tool_call"), now);
        }
        self.last_kind = kind;
    }

    fn observe_tool(&mut self, update: &Value, announced: bool, now: i64) {
        let Some(id) = update.get("toolCallId").and_then(Value::as_str) else {
            return;
        };
        let status = update.get("status").and_then(Value::as_str);
        if matches!(
            status,
            Some("completed") | Some("failed") | Some("cancelled")
        ) {
            self.open_tools.remove(id);
            return;
        }
        let text = |key: &str| update.get(key).and_then(Value::as_str).map(str::to_string);
        match self.open_tools.get_mut(id) {
            Some(open) => {
                if let Some(status) = status {
                    open.status = status.to_string();
                }
                if let Some(title) = text("title") {
                    open.title = title;
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
                    },
                );
            }
            None => {}
        }
    }
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
            "open_tools": activity.open_tools.values().collect::<Vec<_>>(),
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
}
