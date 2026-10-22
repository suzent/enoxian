//! Deciders: what the warden asks for probabilities, and how.
//!
//! A decider answers typed questions about a message — yes/no, pick one — with
//! probabilities and no text. It never runs an agent, posts, or sees anything
//! but the questions it is sent; what to do with its answers is the daemon's
//! decision ([`super::policy`]).
//!
//! Kinds are declared by manifest, like agent adapters (`plugin.rs`): a
//! built-in catalog, plus `~/.enoxian/wardens/*.toml` for anything else. The
//! boundary is the protocol — `systemone` or `decisions` over HTTP — so a
//! decider with logic of its own is a server that speaks one of them; the
//! daemon loads no code. A third-party manifest cannot take a built-in id.
//!
//! Keys live in `~/.enoxian/secrets/wardens.toml` (0600), keyed by decider id,
//! never in `agents.toml`, which the settings API returns whole. They can also
//! come from an environment variable named in the config, in which case nothing
//! is written to disk.

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::BTreeMap;
use std::path::PathBuf;

/// The id of the decider that needs nothing: the spec's own rules.
pub const RULES: &str = "builtin:rules";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Protocol {
    /// No request: the spec's rules decide.
    Rules,
    /// TypeSafe System One: `state` plus a map of `noul` / `choice` questions.
    Systemone,
    /// OpenAI Decisions: `input` plus a list of `predicate` / `choice` questions.
    Decisions,
}

/// A form field the settings panel renders for a decider.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Field {
    pub label: String,
    #[serde(default)]
    pub default: Option<String>,
    #[serde(default)]
    pub secret: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Manifest {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub description: String,
    pub protocol: Protocol,
    /// Where requests go, unless the config overrides it.
    #[serde(default)]
    pub endpoint: Option<String>,
    #[serde(default)]
    pub model: Option<String>,
    #[serde(default)]
    pub needs_key: bool,
    #[serde(default, skip_deserializing)]
    pub builtin: bool,
}

impl Manifest {
    /// The form the settings panel shows. Derived, so a manifest cannot ask for
    /// a field the daemon would ignore.
    pub fn fields(&self) -> BTreeMap<&'static str, Field> {
        let mut fields = BTreeMap::new();
        if self.protocol == Protocol::Rules {
            return fields;
        }
        fields.insert(
            "endpoint",
            Field {
                label: "Endpoint".into(),
                default: self.endpoint.clone(),
                secret: false,
            },
        );
        fields.insert(
            "model",
            Field {
                label: "Model".into(),
                default: self.model.clone(),
                secret: false,
            },
        );
        if self.needs_key {
            fields.insert(
                "api_key",
                Field {
                    label: "API key".into(),
                    default: None,
                    secret: true,
                },
            );
        }
        fields
    }
}

fn builtins() -> Vec<Manifest> {
    vec![
        Manifest {
            id: RULES.into(),
            name: "Built-in rules".into(),
            description: "No model: short messages get one listener, long ones a blind vote, as \
                          the spec's rules say. Free, and the fallback whenever a decider cannot \
                          be asked."
                .into(),
            protocol: Protocol::Rules,
            endpoint: None,
            model: None,
            needs_key: false,
            builtin: true,
        },
        Manifest {
            id: "litellm-systemone".into(),
            name: "LiteLLM (System One)".into(),
            description: "A LiteLLM proxy's /v1/systemone route, serving any decision model in \
                          its model list — TypeSafe Jev, OpenAI, or a self-hosted vLLM."
                .into(),
            protocol: Protocol::Systemone,
            endpoint: Some("http://localhost:4000/v1/systemone".into()),
            model: Some("jev".into()),
            needs_key: true,
            builtin: true,
        },
        Manifest {
            id: "litellm-decisions".into(),
            name: "LiteLLM (OpenAI Decisions)".into(),
            description: "A LiteLLM proxy's /v1/decisions route, in OpenAI's Decisions format."
                .into(),
            protocol: Protocol::Decisions,
            endpoint: Some("http://localhost:4000/v1/decisions".into()),
            model: None,
            needs_key: true,
            builtin: true,
        },
    ]
}

fn manifest_dir() -> anyhow::Result<PathBuf> {
    Ok(crate::config::enoxian_dir()?.join("wardens"))
}

/// Every decider kind this device knows: built-ins first, then valid local
/// manifests in file-name order. A manifest that is invalid or reuses an id is
/// skipped with a warning.
pub fn catalog() -> Vec<Manifest> {
    let mut all = builtins();
    let Ok(dir) = manifest_dir() else {
        return all;
    };
    let Ok(read) = std::fs::read_dir(&dir) else {
        return all;
    };
    let mut paths: Vec<PathBuf> = read
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.extension().is_some_and(|e| e == "toml"))
        .collect();
    paths.sort();
    for path in paths {
        let parsed = std::fs::read_to_string(&path)
            .map_err(|e| e.to_string())
            .and_then(|text| toml::from_str::<Manifest>(&text).map_err(|e| e.to_string()));
        match parsed {
            Ok(manifest) if manifest.protocol == Protocol::Rules => {
                tracing::warn!("[warden] {}: only built-ins use rules", path.display())
            }
            Ok(manifest) if all.iter().any(|m| m.id == manifest.id) => tracing::warn!(
                "[warden] {}: id {:?} is already taken",
                path.display(),
                manifest.id
            ),
            Ok(manifest) if manifest.id.trim().is_empty() => {
                tracing::warn!("[warden] {}: id is empty", path.display())
            }
            Ok(manifest) => all.push(manifest),
            Err(error) => tracing::warn!("[warden] {}: {error}", path.display()),
        }
    }
    all
}

pub fn find(id: &str) -> Option<Manifest> {
    catalog().into_iter().find(|m| m.id == id)
}

/// How this device uses the warden. Lives in `agents.toml` under `[warden]`;
/// device-wide, because the device pays for the decider.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct WardenConfig {
    pub decider: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub endpoint: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    /// Read the key from this environment variable instead of the secrets file.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub key_env: Option<String>,
    pub timeout_secs: u64,
}

impl Default for WardenConfig {
    fn default() -> Self {
        Self {
            decider: RULES.into(),
            endpoint: None,
            model: None,
            key_env: None,
            // Jev usually answers in under a second, but was seen to take
            // more than three on a long message; the rules decide past this.
            timeout_secs: 5,
        }
    }
}

/// Everything needed to ask: the manifest, with the config's overrides applied,
/// and the key if one is needed.
#[derive(Debug, Clone)]
pub struct Resolved {
    pub manifest: Manifest,
    pub endpoint: Option<String>,
    pub model: Option<String>,
    pub key: Option<String>,
    pub timeout: std::time::Duration,
}

impl Resolved {
    pub fn is_rules(&self) -> bool {
        self.manifest.protocol == Protocol::Rules
    }
}

/// A proxy given by its address alone (`https://litellm.example.com/`) means
/// the decider's usual route on it. Posting to the bare origin answers 405,
/// which is what someone pasting their proxy's address gets otherwise.
fn with_default_path(endpoint: &str, default: Option<&str>) -> String {
    let trimmed = endpoint.trim();
    let route = default
        .and_then(|d| reqwest::Url::parse(d).ok())
        .map(|d| d.path().to_string());
    match (reqwest::Url::parse(trimmed), route) {
        // Parsed rather than spliced, so a query or fragment stays after the
        // route instead of swallowing it.
        (Ok(mut url), Some(route)) if url.path().trim_matches('/').is_empty() => {
            url.set_path(&route);
            url.to_string()
        }
        _ => trimmed.to_string(),
    }
}

/// Resolve the configured decider, or say why it cannot be used. The caller
/// falls back to the rules on an error, and records why.
pub fn resolve(cfg: &WardenConfig) -> Result<Resolved, String> {
    let manifest =
        find(&cfg.decider).ok_or_else(|| format!("no decider named {:?}", cfg.decider))?;
    let endpoint = cfg
        .endpoint
        .clone()
        .filter(|e| !e.trim().is_empty())
        .map(|e| with_default_path(&e, manifest.endpoint.as_deref()))
        .or_else(|| manifest.endpoint.clone());
    let model = cfg
        .model
        .clone()
        .filter(|m| !m.trim().is_empty())
        .or_else(|| manifest.model.clone());
    let timeout = std::time::Duration::from_secs(cfg.timeout_secs.clamp(1, 30));
    if manifest.protocol == Protocol::Rules {
        return Ok(Resolved {
            manifest,
            endpoint: None,
            model: None,
            key: None,
            timeout,
        });
    }
    let endpoint = endpoint.ok_or("no endpoint is configured")?;
    if !(endpoint.starts_with("http://") || endpoint.starts_with("https://")) {
        return Err("the endpoint must be an http(s) URL".into());
    }
    let key = match &cfg.key_env {
        Some(var) if !var.trim().is_empty() => std::env::var(var).ok(),
        _ => secrets::get(&manifest.id),
    };
    if manifest.needs_key && key.is_none() {
        return Err("no API key is set".into());
    }
    Ok(Resolved {
        manifest,
        endpoint: Some(endpoint),
        model,
        key,
        timeout,
    })
}

pub mod secrets {
    //! `~/.enoxian/secrets/wardens.toml`: API keys by decider id, mode 0600.

    use std::collections::BTreeMap;

    #[derive(serde::Serialize, serde::Deserialize, Default)]
    struct Entry {
        api_key: String,
    }

    fn path() -> anyhow::Result<std::path::PathBuf> {
        Ok(crate::config::enoxian_dir()?
            .join("secrets")
            .join("wardens.toml"))
    }

    fn load() -> BTreeMap<String, Entry> {
        let Ok(path) = path() else {
            return BTreeMap::new();
        };
        crate::config::tighten_if_loose(&path);
        std::fs::read_to_string(&path)
            .ok()
            .and_then(|text| toml::from_str(&text).ok())
            .unwrap_or_default()
    }

    pub fn get(id: &str) -> Option<String> {
        load()
            .remove(id)
            .map(|e| e.api_key)
            .filter(|k| !k.is_empty())
    }

    /// The last four characters, for "set · ••••abcd". Never the key.
    pub fn hint(id: &str) -> Option<String> {
        get(id).map(|key| {
            let tail: String = key
                .chars()
                .rev()
                .take(4)
                .collect::<Vec<_>>()
                .into_iter()
                .rev()
                .collect();
            format!("••••{tail}")
        })
    }

    /// Set or, with `None`, remove the key for one decider.
    pub fn set(id: &str, key: Option<&str>) -> anyhow::Result<()> {
        let mut all = load();
        match key.map(str::trim).filter(|k| !k.is_empty()) {
            Some(key) => {
                all.insert(
                    id.to_string(),
                    Entry {
                        api_key: key.to_string(),
                    },
                );
            }
            None => {
                all.remove(id);
            }
        }
        let path = path()?;
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        crate::config::write_secret(&path, toml::to_string(&all)?)
    }
}

/// What kind of answer a question wants.
#[derive(Debug, Clone, PartialEq)]
pub enum Ask {
    /// Yes or no; answered with the probability of yes.
    YesNo,
    /// One of these `(value, description)`; answered with a probability each.
    Choice(Vec<(String, String)>),
}

#[derive(Debug, Clone, PartialEq)]
pub struct Question {
    pub name: String,
    pub instructions: String,
    pub ask: Ask,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "snake_case", tag = "type")]
pub enum Answer {
    YesNo { p: f64 },
    Choice { p: BTreeMap<String, f64> },
    Refused,
}

/// What came back, and what it cost.
#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub struct Reply {
    pub answers: BTreeMap<String, Answer>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    /// From `x-litellm-response-cost`, in USD, when the proxy reports it.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cost_usd: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub input_tokens: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cached_tokens: Option<u64>,
    pub latency_ms: u64,
}

impl Reply {
    pub fn yes(&self, name: &str) -> Option<f64> {
        match self.answers.get(name)? {
            Answer::YesNo { p } => Some(*p),
            _ => None,
        }
    }

    pub fn choice(&self, name: &str) -> Option<&BTreeMap<String, f64>> {
        match self.answers.get(name)? {
            Answer::Choice { p } => Some(p),
            _ => None,
        }
    }
}

/// The request body for `protocol`.
pub fn request_body(
    protocol: Protocol,
    model: Option<&str>,
    input: &str,
    questions: &[Question],
) -> Value {
    match protocol {
        Protocol::Rules => Value::Null,
        Protocol::Systemone => {
            let mut map = serde_json::Map::new();
            for q in questions {
                let body = match &q.ask {
                    Ask::YesNo => json!({
                        "type": "noul",
                        "instructions": q.instructions,
                        "criteria": { "true": "Yes", "false": "No" },
                    }),
                    Ask::Choice(options) => json!({
                        "type": "choice",
                        "instructions": q.instructions,
                        "criteria": options
                            .iter()
                            .map(|(v, d)| (v.clone(), Value::String(d.clone())))
                            .collect::<serde_json::Map<_, _>>(),
                    }),
                };
                map.insert(q.name.clone(), body);
            }
            let mut body = json!({ "state": input, "questions": map });
            if let Some(model) = model {
                body["model"] = json!(model);
            }
            body
        }
        Protocol::Decisions => {
            let list: Vec<Value> = questions
                .iter()
                .map(|q| match &q.ask {
                    Ask::YesNo => json!({
                        "type": "predicate", "name": q.name, "instructions": q.instructions,
                    }),
                    Ask::Choice(options) => json!({
                        "type": "choice", "name": q.name, "instructions": q.instructions,
                        "choices": options
                            .iter()
                            .map(|(v, d)| json!({ "value": v, "description": d }))
                            .collect::<Vec<_>>(),
                    }),
                })
                .collect();
            let mut body = json!({ "input": input, "questions": list });
            if let Some(model) = model {
                body["model"] = json!(model);
            }
            body
        }
    }
}

/// Probabilities by value, from either a `{value: p}` map or a list of
/// `{value|label, probability}`.
fn probabilities(value: Option<&Value>) -> BTreeMap<String, f64> {
    let mut out = BTreeMap::new();
    match value {
        Some(Value::Object(map)) => {
            for (k, v) in map {
                if let Some(p) = v.as_f64() {
                    out.insert(k.clone(), p);
                }
            }
        }
        Some(Value::Array(list)) => {
            for item in list {
                let key = item
                    .get("value")
                    .or_else(|| item.get("label"))
                    .map(|v| match v {
                        Value::String(s) => s.clone(),
                        other => other.to_string(),
                    });
                if let (Some(key), Some(p)) = (key, item.get("probability").and_then(Value::as_f64))
                {
                    out.insert(key, p);
                }
            }
        }
        _ => {}
    }
    out
}

fn parse_answer(kind: &str, body: &Value, ask: &Ask) -> Answer {
    let p = |v: Option<&Value>| v.and_then(Value::as_f64).map(|p| p.clamp(0.0, 1.0));
    match (kind, ask) {
        ("refusal", _) => Answer::Refused,
        (_, Ask::YesNo) => match p(body.get("noul")).or_else(|| p(body.get("probability"))) {
            Some(p) => Answer::YesNo { p },
            None => Answer::Refused,
        },
        (_, Ask::Choice(options)) => {
            let mut probs = probabilities(body.get("probabilities"));
            if probs.is_empty() {
                // Only the pick came back: all the mass on it.
                if let Some(pick) = body.get("choice").and_then(Value::as_str) {
                    probs.insert(pick.to_string(), p(body.get("confidence")).unwrap_or(1.0));
                }
            }
            probs.retain(|k, _| options.iter().any(|(v, _)| v == k));
            if probs.is_empty() {
                Answer::Refused
            } else {
                Answer::Choice { p: probs }
            }
        }
    }
}

/// Read a response body for `protocol`. Questions it did not answer are left
/// out, which the policy treats like a refusal.
pub fn parse_response(protocol: Protocol, body: &Value, questions: &[Question]) -> Reply {
    let mut reply = Reply {
        model: body
            .get("model")
            .and_then(Value::as_str)
            .map(str::to_string),
        input_tokens: body.pointer("/usage/input_tokens").and_then(Value::as_u64),
        cached_tokens: body
            .pointer("/usage/input_tokens_details/cached_tokens")
            .or_else(|| body.pointer("/usage/cached_tokens"))
            .and_then(Value::as_u64),
        ..Reply::default()
    };
    let ask_of = |name: &str| questions.iter().find(|q| q.name == name).map(|q| &q.ask);
    match (protocol, body.get("answers")) {
        (Protocol::Systemone, Some(Value::Object(map))) => {
            for (name, answer) in map {
                if let Some(ask) = ask_of(name) {
                    let kind = answer.get("type").and_then(Value::as_str).unwrap_or("");
                    reply
                        .answers
                        .insert(name.clone(), parse_answer(kind, answer, ask));
                }
            }
        }
        (Protocol::Decisions, Some(Value::Array(list))) => {
            for (i, answer) in list.iter().enumerate() {
                // Answers come back in question order; `name` is echoed when set.
                let name = answer
                    .get("name")
                    .and_then(Value::as_str)
                    .map(str::to_string)
                    .or_else(|| questions.get(i).map(|q| q.name.clone()));
                let Some(name) = name else { continue };
                if let Some(ask) = ask_of(&name) {
                    let kind = answer.get("type").and_then(Value::as_str).unwrap_or("");
                    reply.answers.insert(name, parse_answer(kind, answer, ask));
                }
            }
        }
        _ => {}
    }
    reply
}

/// Put the questions to a decider over HTTP.
pub async fn ask(decider: &Resolved, input: &str, questions: &[Question]) -> anyhow::Result<Reply> {
    let endpoint = decider
        .endpoint
        .as_deref()
        .ok_or_else(|| anyhow::anyhow!("the rules are not asked over HTTP"))?;
    let body = request_body(
        decider.manifest.protocol,
        decider.model.as_deref(),
        input,
        questions,
    );
    let started = std::time::Instant::now();
    // The credential-free client: a decider is somebody else's server, and the
    // daemon's own token must never reach it. The key goes in the header only.
    let mut request = crate::outbound::client()
        .post(endpoint)
        .timeout(decider.timeout)
        .json(&body);
    if let Some(key) = &decider.key {
        request = request.bearer_auth(key);
    }
    let response = request.send().await?;
    let status = response.status();
    let cost_usd = response
        .headers()
        .get("x-litellm-response-cost")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.trim().parse::<f64>().ok());
    let text = response.text().await?;
    if !status.is_success() {
        let detail: String = text.chars().take(300).collect();
        anyhow::bail!("decider returned {status}: {detail}");
    }
    let value: Value = serde_json::from_str(&text)?;
    let mut reply = parse_response(decider.manifest.protocol, &value, questions);
    reply.cost_usd = cost_usd;
    reply.latency_ms = started.elapsed().as_millis() as u64;
    Ok(reply)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn questions() -> Vec<Question> {
        vec![
            Question {
                name: "admit".into(),
                instructions: "needs an answer?".into(),
                ask: Ask::YesNo,
            },
            Question {
                name: "agent".into(),
                instructions: "who?".into(),
                ask: Ask::Choice(vec![
                    ("claude".into(), "reviews code".into()),
                    ("codex".into(), "runs tests".into()),
                ]),
            },
        ]
    }

    #[test]
    fn systemone_request_uses_criteria() {
        let body = request_body(Protocol::Systemone, Some("jev"), "hi", &questions());
        assert_eq!(body["model"], "jev");
        assert_eq!(body["state"], "hi");
        assert_eq!(body["questions"]["admit"]["type"], "noul");
        assert_eq!(
            body["questions"]["agent"]["criteria"]["codex"],
            "runs tests"
        );
    }

    #[test]
    fn decisions_request_is_a_list_in_order() {
        let body = request_body(Protocol::Decisions, None, "hi", &questions());
        assert!(body.get("model").is_none());
        assert_eq!(body["questions"][0]["type"], "predicate");
        assert_eq!(body["questions"][1]["choices"][1]["value"], "codex");
    }

    #[test]
    fn systemone_response_is_read() {
        let body = json!({
            "model": "jev-1.13.0",
            "answers": {
                "admit": { "type": "noul", "noul": 0.83 },
                "agent": { "type": "choice", "choice": "codex", "confidence": 0.7,
                           "probabilities": { "claude": 0.3, "codex": 0.7 } },
                "stray": { "type": "noul", "noul": 0.1 }
            },
            "usage": { "input_tokens": 391, "output_tokens": 67 }
        });
        let reply = parse_response(Protocol::Systemone, &body, &questions());
        assert_eq!(reply.yes("admit"), Some(0.83));
        assert_eq!(reply.choice("agent").unwrap()["codex"], 0.7);
        assert!(!reply.answers.contains_key("stray"), "unasked is ignored");
        assert_eq!(reply.input_tokens, Some(391));
        assert_eq!(reply.model.as_deref(), Some("jev-1.13.0"));
    }

    #[test]
    fn decisions_response_is_read_with_list_probabilities_and_refusals() {
        let body = json!({
            "answers": [
                { "type": "refusal", "name": "admit" },
                { "type": "choice", "name": "agent", "choice": "claude",
                  "probabilities": [ { "value": "claude", "probability": 0.6 },
                                     { "value": "codex", "probability": 0.4 } ] }
            ],
            "usage": { "input_tokens": 10, "input_tokens_details": { "cached_tokens": 8 } }
        });
        let reply = parse_response(Protocol::Decisions, &body, &questions());
        assert_eq!(reply.answers["admit"], Answer::Refused);
        assert_eq!(reply.choice("agent").unwrap()["claude"], 0.6);
        assert_eq!(reply.cached_tokens, Some(8));
    }

    #[test]
    fn a_choice_with_only_a_pick_puts_its_confidence_on_it() {
        let body = json!({ "answers": { "agent": { "type": "choice", "choice": "claude", "confidence": 0.9 } } });
        let reply = parse_response(Protocol::Systemone, &body, &questions());
        assert_eq!(reply.choice("agent").unwrap().get("claude"), Some(&0.9));
    }

    #[test]
    fn a_bare_proxy_address_gets_the_deciders_route() {
        let default = Some("http://localhost:4000/v1/decisions");
        assert_eq!(
            with_default_path("https://litellm.example.com/", default),
            "https://litellm.example.com/v1/decisions"
        );
        assert_eq!(
            with_default_path("https://litellm.example.com", default),
            "https://litellm.example.com/v1/decisions"
        );
        // A path someone wrote is theirs.
        assert_eq!(
            with_default_path("https://proxy.example.com/team/v1/decisions", default),
            "https://proxy.example.com/team/v1/decisions"
        );
        assert_eq!(
            with_default_path("https://litellm.example.com/", None),
            "https://litellm.example.com/"
        );
        // A query or fragment stays after the route.
        assert_eq!(
            with_default_path("https://proxy.example.com?token=x", default),
            "https://proxy.example.com/v1/decisions?token=x"
        );
        assert_eq!(
            with_default_path("https://proxy.example.com/?token=x#f", default),
            "https://proxy.example.com/v1/decisions?token=x#f"
        );
    }

    #[test]
    fn rules_resolve_without_a_key_or_endpoint() {
        let resolved = resolve(&WardenConfig::default()).unwrap();
        assert!(resolved.is_rules());
    }

    #[test]
    fn an_http_decider_without_a_key_is_refused() {
        let cfg = WardenConfig {
            decider: "litellm-systemone".into(),
            key_env: Some("ENOXIAN_TEST_WARDEN_KEY_THAT_IS_NOT_SET".into()),
            ..WardenConfig::default()
        };
        assert_eq!(resolve(&cfg).unwrap_err(), "no API key is set");
    }

    #[tokio::test]
    async fn ask_posts_with_the_key_and_reads_the_cost_header() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            let mut buf = vec![0u8; 8192];
            let mut seen = String::new();
            loop {
                let n = socket.read(&mut buf).await.unwrap();
                seen.push_str(&String::from_utf8_lossy(&buf[..n]));
                if seen.contains("\r\n\r\n") && seen.contains("}}") {
                    break;
                }
            }
            let body = r#"{"answers":{"admit":{"type":"noul","noul":0.25}}}"#;
            let response = format!(
                "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\nx-litellm-response-cost: 0.00042\r\ncontent-length: {}\r\n\r\n{body}",
                body.len()
            );
            socket.write_all(response.as_bytes()).await.unwrap();
            seen
        });
        let resolved = Resolved {
            manifest: find("litellm-systemone").unwrap(),
            endpoint: Some(format!("http://{addr}/v1/systemone")),
            model: Some("jev".into()),
            key: Some("sk-test".into()),
            timeout: std::time::Duration::from_secs(5),
        };
        let reply = ask(&resolved, "hello", &questions()[..1]).await.unwrap();
        assert_eq!(reply.yes("admit"), Some(0.25));
        assert_eq!(reply.cost_usd, Some(0.00042));
        let seen = server.await.unwrap();
        assert!(
            seen.contains("authorization: Bearer sk-test")
                || seen.contains("Authorization: Bearer sk-test")
        );
        assert!(seen.starts_with("POST /v1/systemone"));
    }
}
