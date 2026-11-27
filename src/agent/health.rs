//! Whether one of this device's agents can take a turn at all.
//!
//! Some failures say nothing about the message and everything about the
//! agent: its sign-in expired, or its program is gone. Every message offered to
//! it fails the same way until someone fixes it, and an unaddressed message
//! handed to it costs the room a failed turn and a retry. So after one such
//! failure the agent is left out of the listeners on this device — rotation,
//! the warden, and the retries of failed messages all read that one list — and
//! a message nobody named goes to the others instead.
//!
//! It is let back in by any of: a turn that succeeds (an @mention still runs
//! it, and is how someone who fixed it outside enoxian finds out); a sign-in
//! from Settings that verifies; or, every [`PROBE_EVERY`], one message offered
//! as a probe. Only this device's agents, and only in memory: a restart lets
//! every agent try again, which is what someone restarting expects.
//!
//! Nothing here is synced. Other devices learn about it the way they learn
//! about every turn, from the failed run's receipt.

use std::collections::HashMap;
use std::sync::Mutex;

/// How long a broken agent is left out before one message probes it again.
pub const PROBE_EVERY: i64 = 10 * 60;

#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Problem {
    /// The agent or its provider wants someone to sign in again.
    SignIn,
    /// The agent's program could not be started.
    Missing,
    /// The agent's account has used up its allowance until some later time.
    Limited,
}

impl Problem {
    /// What the room is told, as the reason a listener was left out.
    pub fn reason(self) -> &'static str {
        match self {
            Problem::SignIn => "needs to sign in again on this device",
            Problem::Missing => "could not be started on this device",
            Problem::Limited => "has hit its usage limit on this device",
        }
    }
}

#[derive(Clone, Debug)]
struct Broken {
    problem: Problem,
    /// When a message may next be offered to it as a probe.
    probe_at: i64,
}

static BROKEN: Mutex<Option<HashMap<String, Broken>>> = Mutex::new(None);

fn with<T>(f: impl FnOnce(&mut HashMap<String, Broken>) -> T) -> T {
    let mut guard = BROKEN.lock().unwrap_or_else(|e| e.into_inner());
    f(guard.get_or_insert_with(HashMap::new))
}

fn key(agent: &str) -> String {
    agent.to_ascii_lowercase()
}

/// Whether a turn's failure is the agent's rather than the message's.
///
/// Deliberately narrow: a timeout, a refused prompt or a model error is one
/// message going wrong, and leaving an agent out for it would turn one bad turn
/// into ten minutes of silence.
pub fn classify(error: &anyhow::Error) -> Option<Problem> {
    for cause in error.chain() {
        if let Some(rpc) = cause.downcast_ref::<super::acp::RpcError>() {
            // -32000 is ACP's "auth required" — unless it is about a session
            // this authorization can no longer reach, which a fresh session
            // fixes without anyone signing in.
            if rpc.code == -32000 && !super::acp::session_gone(error) {
                return Some(Problem::SignIn);
            }
        }
        if let Some(io) = cause.downcast_ref::<std::io::Error>() {
            if matches!(
                io.kind(),
                std::io::ErrorKind::NotFound | std::io::ErrorKind::PermissionDenied
            ) && format!("{error:#}").contains("failed to spawn")
            {
                return Some(Problem::Missing);
            }
        }
    }
    classify_text(&format!("{error:#}"))
}

/// The same judgement from an error's text, for adapters that wrap a provider's
/// sign-in failure in a generic error code (Claude's expired OAuth arrives as
/// -32603 "Internal error: Failed to authenticate").
pub fn classify_text(text: &str) -> Option<Problem> {
    let text = text.to_ascii_lowercase();
    const SIGN_IN: &[&str] = &[
        "failed to authenticate",
        "authentication required",
        "authorization required",
        "not logged in",
        "please log in",
        "please run /login",
        "invalid api key",
        "oauth token has expired",
        "oauth session expired",
    ];
    // An allowance used up, not a request refused for being too fast: a
    // plain rate limit clears in seconds and is one message's bad luck.
    const LIMITED: &[&str] = &[
        "hit your session limit",
        "hit your usage limit",
        "usage limit reached",
        "quota exceeded",
        "exceeded your current quota",
    ];
    if SIGN_IN.iter().any(|needle| text.contains(needle)) {
        Some(Problem::SignIn)
    } else if LIMITED.iter().any(|needle| text.contains(needle)) {
        Some(Problem::Limited)
    } else {
        None
    }
}

/// Note that `agent`'s turn failed with `problem`.
pub fn failed(agent: &str, problem: Problem, now: i64) {
    with(|all| {
        all.insert(
            key(agent),
            Broken {
                problem,
                probe_at: now + PROBE_EVERY,
            },
        );
    })
}

/// Note that `agent` is working again: a turn of its succeeded, or a sign-in
/// verified.
pub fn recovered(agent: &str) {
    with(|all| {
        all.remove(&key(agent));
    })
}

/// A turn is starting for `agent`. If it was left out, this turn is its probe:
/// no other message is offered to it until this one has had its answer, or
/// another [`PROBE_EVERY`] has passed.
pub fn starting(agent: &str, now: i64) {
    with(|all| {
        if let Some(broken) = all.get_mut(&key(agent)) {
            broken.probe_at = broken.probe_at.max(now + PROBE_EVERY);
        }
    })
}

/// Why `agent` should not be offered a message now, if it should not.
pub fn left_out(agent: &str, now: i64) -> Option<Problem> {
    with(|all| {
        all.get(&key(agent))
            .filter(|broken| now < broken.probe_at)
            .map(|broken| broken.problem)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_sign_in_failures_from_their_text() {
        let expired = "ACP prompt turn failed: ACP request failed (code -32603): Internal error: \
                       Failed to authenticate: OAuth session expired and could not be refreshed";
        assert_eq!(classify_text(expired), Some(Problem::SignIn));
        assert_eq!(
            classify_text("Suzent authorization required. Run terminal login"),
            Some(Problem::SignIn)
        );
        assert_eq!(classify_text("turn timed out after 600s"), None);
        assert_eq!(
            classify_text(
                "Internal error: You've hit your session limit · resets 3:30am (Europe/London)"
            ),
            Some(Problem::Limited)
        );
        assert_eq!(classify_text("429 rate limited, retry in 2s"), None);
        assert_eq!(classify_text("model returned 529 overloaded"), None);
    }

    #[test]
    fn auth_required_rpc_is_sign_in_but_a_lost_session_is_not() {
        let auth = anyhow::Error::new(crate::agent::acp::RpcError {
            code: -32000,
            message: "Authentication required".into(),
        })
        .context("ACP handshake failed");
        assert_eq!(classify(&auth), Some(Problem::SignIn));
        let gone = anyhow::Error::new(crate::agent::acp::RpcError {
            code: -32000,
            message: "this authorization cannot access the requested session".into(),
        });
        assert_eq!(classify(&gone), None);
        let busy = anyhow::Error::new(crate::agent::acp::RpcError {
            code: -32603,
            message: "Internal error: rate limited".into(),
        });
        assert_eq!(classify(&busy), None);
    }

    #[test]
    fn a_missing_program_is_its_own_problem() {
        let error = anyhow::Error::new(std::io::Error::from(std::io::ErrorKind::NotFound))
            .context("failed to spawn ACP agent `nope`");
        assert_eq!(classify(&error), Some(Problem::Missing));
    }

    #[test]
    fn left_out_until_a_probe_is_due_or_it_recovers() {
        let agent = "health-test-probe";
        let now = 1_000;
        failed(agent, Problem::SignIn, now);
        assert_eq!(left_out(agent, now + 1), Some(Problem::SignIn));
        assert_eq!(
            left_out("HEALTH-TEST-PROBE", now + 1),
            Some(Problem::SignIn),
            "names match without case"
        );
        // Due for a probe: offered once…
        assert_eq!(left_out(agent, now + PROBE_EVERY), None);
        // …and the probe starting closes the door behind it.
        starting(agent, now + PROBE_EVERY);
        assert!(left_out(agent, now + PROBE_EVERY + 1).is_some());
        recovered(agent);
        assert_eq!(left_out(agent, now + PROBE_EVERY + 1), None);
    }

    #[test]
    fn a_healthy_agent_is_never_left_out() {
        assert_eq!(left_out("health-test-never-failed", 0), None);
        starting("health-test-never-failed", 0);
        assert_eq!(left_out("health-test-never-failed", 1), None);
    }
}
