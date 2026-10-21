//! What a turn cost, as the agent reports it.
//!
//! ACP agents may send `usage_update` session updates: how much of the context
//! window is in use (`used` of `size` tokens) and, optionally, a running `cost`.
//! Not every report carries the cost — the Claude adapter interleaves reports
//! with and without it — so the cost is taken from the latest report that has
//! one. An agent may also answer `session/prompt` with the tokens that prompt
//! used. enoxian does not count tokens or price models itself: it records what
//! the agent says, and says nothing for an agent that is silent. An estimate
//! would look like a fact in the activity panel.
//!
//! The spec calls the cost cumulative for the session, but does not say whether
//! it survives the agent process. The Claude adapter's does not: reloaded in a
//! new process, a session's cost starts again from zero. enoxian starts one
//! agent process per turn, so a turn's cost is the last cost reported after the
//! prompt, less any reported before it (while the session loaded) — never a
//! figure carried over from an earlier process, which was what made a resumed
//! turn come out negative.
//!
//! A reported cost is the agent's figure, not a bill. An agent on a
//! subscription may report what the turn would cost at API prices.
//!
//! Recorded on this device's execution inbox only. Like the admission log, it
//! is this device's own account of what it spent; it is never published to the
//! Circle.

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// A monetary amount as an agent reported it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Cost {
    pub amount: f64,
    /// ISO 4217 code, e.g. `USD`.
    pub currency: String,
}

impl Cost {
    /// `self - earlier`, when both are in one currency and the difference is
    /// not negative (a negative one means the agent reset its own counter).
    fn since(&self, earlier: &Cost) -> Option<Cost> {
        let amount = self.amount - earlier.amount;
        (self.currency.eq_ignore_ascii_case(&earlier.currency) && amount >= 0.0).then(|| Cost {
            amount,
            currency: self.currency.clone(),
        })
    }
}

/// One `usage_update`.
#[derive(Debug, Clone, PartialEq)]
pub struct Report {
    /// Tokens currently in the context window.
    pub used: u64,
    /// Size of the context window in tokens.
    pub size: u64,
    /// The agent's running cost, when it reports one.
    pub cost: Option<Cost>,
}

/// Parse a `session/update` payload if it is a `usage_update`.
pub fn parse(update: &Value) -> Option<Report> {
    if super::acp::update_kind(update) != Some("usage_update") {
        return None;
    }
    let cost = update.get("cost").and_then(|cost| {
        Some(Cost {
            amount: cost.get("amount")?.as_f64().filter(|a| a.is_finite())?,
            currency: cost.get("currency")?.as_str()?.to_string(),
        })
    });
    Some(Report {
        used: update.get("used").and_then(Value::as_u64).unwrap_or(0),
        size: update.get("size").and_then(Value::as_u64).unwrap_or(0),
        cost,
    })
}

/// Tokens one `session/prompt` used, from its response's `usage`.
#[derive(Debug, Clone, Copy, Default, PartialEq, Serialize, Deserialize)]
pub struct Tokens {
    pub input: u64,
    pub output: u64,
    #[serde(default)]
    pub cached_read: u64,
    #[serde(default)]
    pub cached_write: u64,
    pub total: u64,
}

impl Tokens {
    fn add(self, other: Tokens) -> Tokens {
        Tokens {
            input: self.input + other.input,
            output: self.output + other.output,
            cached_read: self.cached_read + other.cached_read,
            cached_write: self.cached_write + other.cached_write,
            total: self.total + other.total,
        }
    }
}

/// Parse the `usage` of a `session/prompt` result, if the agent sent one.
pub fn parse_tokens(result: &Value) -> Option<Tokens> {
    let usage = result.get("usage").filter(|u| u.is_object())?;
    let n = |key: &str| usage.get(key).and_then(Value::as_u64).unwrap_or(0);
    let tokens = Tokens {
        input: n("inputTokens"),
        output: n("outputTokens"),
        cached_read: n("cachedReadTokens"),
        cached_write: n("cachedWriteTokens"),
        total: n("totalTokens"),
    };
    (tokens != Tokens::default()).then_some(tokens)
}

/// What one agent process reported, split at the moment the prompt is sent.
#[derive(Debug, Default)]
pub struct Meter {
    /// Last report before the prompt, e.g. replayed while a session loaded.
    before: Option<Report>,
    /// Last report since the prompt.
    during: Option<Report>,
    /// What this turn's prompts used, summed (the turn and any hold).
    tokens: Option<Tokens>,
}

impl Meter {
    pub fn observe(&mut self, report: Report, prompting: bool) {
        let slot = if prompting {
            &mut self.during
        } else {
            &mut self.before
        };
        // A report without a cost does not mean the cost went away.
        let cost = report
            .cost
            .or_else(|| slot.as_ref().and_then(|r| r.cost.clone()));
        *slot = Some(Report { cost, ..report });
    }

    pub fn add_tokens(&mut self, tokens: Option<Tokens>) {
        if let Some(tokens) = tokens {
            self.tokens = Some(self.tokens.unwrap_or_default().add(tokens));
        }
    }

    /// What this turn used, or `None` if the agent reported nothing.
    pub fn turn(&self) -> Option<TurnUsage> {
        if self.during.is_none() && self.tokens.is_none() {
            return None;
        }
        let before = self.before.as_ref().and_then(|b| b.cost.as_ref());
        let cost =
            self.during
                .as_ref()
                .and_then(|d| d.cost.as_ref())
                .and_then(|now| match before {
                    Some(then) => now.since(then),
                    None => Some(now.clone()),
                });
        Some(TurnUsage {
            context_used: self.during.as_ref().map_or(0, |d| d.used),
            context_size: self.during.as_ref().map_or(0, |d| d.size),
            tokens: self.tokens,
            cost,
        })
    }
}

/// What one turn used, as recorded on its execution inbox entry.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TurnUsage {
    /// Tokens in the context window when the turn ended; 0 of 0 when the agent
    /// sent no `usage_update`.
    pub context_used: u64,
    pub context_size: u64,
    /// What the turn's prompts used, if the agent said.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tokens: Option<Tokens>,
    /// What the turn cost, if the agent said.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cost: Option<Cost>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn usd(amount: f64) -> Cost {
        Cost {
            amount,
            currency: "USD".into(),
        }
    }

    fn report(used: u64, cost: Option<f64>) -> Report {
        Report {
            used,
            size: 200_000,
            cost: cost.map(usd),
        }
    }

    #[test]
    fn parses_a_usage_update_and_nothing_else() {
        let update = json!({
            "sessionUpdate": "usage_update", "used": 41_000, "size": 200_000,
            "cost": { "amount": 0.25, "currency": "USD" }
        });
        assert_eq!(parse(&update), Some(report(41_000, Some(0.25))));
        let no_cost = json!({ "sessionUpdate": "usage_update", "used": 5, "size": 10 });
        assert_eq!(parse(&no_cost).unwrap().cost, None);
        let chunk = json!({ "sessionUpdate": "agent_message_chunk", "content": { "text": "x" } });
        assert_eq!(parse(&chunk), None);
    }

    #[test]
    fn a_turn_costs_what_its_process_reports_after_the_prompt() {
        let mut meter = Meter::default();
        meter.observe(report(10_000, Some(0.10)), true);
        meter.observe(report(12_000, Some(0.14)), true);
        let turn = meter.turn().unwrap();
        assert_eq!(turn.context_used, 12_000);
        assert_eq!(turn.cost, Some(usd(0.14)));
    }

    #[test]
    fn a_cost_reported_while_loading_is_not_this_turns() {
        let mut meter = Meter::default();
        meter.observe(report(30_000, Some(1.00)), false);
        meter.observe(report(34_000, Some(1.25)), true);
        assert_eq!(meter.turn().unwrap().cost, Some(usd(0.25)));
    }

    /// The Claude adapter's order: a context report, one with only rate-limit
    /// metadata, then one with the cost. Any of them can come last.
    #[test]
    fn a_report_without_a_cost_keeps_the_last_cost() {
        let mut meter = Meter::default();
        meter.observe(report(31_524, Some(0.315)), true);
        meter.observe(report(33_477, None), true);
        let turn = meter.turn().unwrap();
        assert_eq!(turn.context_used, 33_477);
        assert_eq!(turn.cost, Some(usd(0.315)));
    }

    #[test]
    fn a_reset_counter_or_another_currency_is_not_a_cost() {
        let mut meter = Meter::default();
        meter.observe(report(1_000, Some(2.00)), false);
        meter.observe(report(1_000, Some(0.05)), true);
        assert_eq!(meter.turn().unwrap().cost, None);

        let mut meter = Meter::default();
        let eur = Report {
            cost: Some(Cost {
                amount: 0.01,
                currency: "EUR".into(),
            }),
            ..report(1, None)
        };
        meter.observe(eur, false);
        meter.observe(report(1, Some(0.05)), true);
        assert_eq!(meter.turn().unwrap().cost, None);
    }

    #[test]
    fn prompt_tokens_are_summed_over_the_turn() {
        let result = json!({ "stopReason": "end_turn", "usage": {
            "inputTokens": 2, "outputTokens": 4, "cachedReadTokens": 0,
            "cachedWriteTokens": 31_518, "totalTokens": 31_524 } });
        let tokens = parse_tokens(&result).unwrap();
        assert_eq!((tokens.cached_write, tokens.total), (31_518, 31_524));
        assert_eq!(parse_tokens(&json!({ "usage": null })), None);
        assert_eq!(parse_tokens(&json!({ "stopReason": "end_turn" })), None);

        // An agent that reports tokens and nothing else still has usage.
        let mut meter = Meter::default();
        meter.add_tokens(Some(tokens));
        meter.add_tokens(None);
        meter.add_tokens(Some(tokens));
        let turn = meter.turn().unwrap();
        assert_eq!(turn.tokens.unwrap().total, 63_048);
        assert_eq!((turn.context_size, turn.cost), (0, None));
    }

    #[test]
    fn an_agent_that_never_reports_has_no_usage() {
        let mut meter = Meter::default();
        assert_eq!(meter.turn(), None);
        // A report from loading alone says nothing about this turn.
        meter.observe(report(1, Some(0.01)), false);
        assert_eq!(meter.turn(), None);
    }
}
