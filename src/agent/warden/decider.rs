//! Deciders: what the warden asks for probabilities, and how.
//!
//! A decider answers typed questions about a message — yes/no, pick one — with
//! probabilities and no text. It never runs an agent, posts, or sees anything
//! but the questions it is sent; what to do with its answers is the daemon's
//! decision ([`super::policy`]).
//!
//! Kinds are declared by manifest, like agent adapters (`plugin.rs`): a
//! built-in catalog, plus `~/.enoxian/wardens/*.toml` for anything else. The
//! boundary is the protocol: a decision API (`systemone`, `decisions`), or a
//! chat model asked for structured output (`openai_chat`, `openai_responses`,
//! `anthropic_messages`, `gemini`). A decider with logic of its own is a
//! server that speaks one of them; the daemon loads no code. A third-party
//! manifest cannot take a built-in id.
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
    /// OpenAI Chat Completions with a strict JSON schema. Also what
    /// OpenAI-compatible servers speak: OpenRouter, DeepSeek, Ollama, vLLM, a
    /// LiteLLM proxy.
    OpenaiChat,
    /// OpenAI Responses with a strict JSON schema.
    OpenaiResponses,
    /// Anthropic Messages, answered through one forced tool call.
    AnthropicMessages,
    /// Gemini `generateContent` with a response JSON schema.
    Gemini,
}

impl Protocol {
    /// A general chat model asked for structured output, rather than a
    /// decision API that answers with probabilities of its own.
    pub fn is_chat(self) -> bool {
        matches!(
            self,
            Protocol::OpenaiChat
                | Protocol::OpenaiResponses
                | Protocol::AnthropicMessages
                | Protocol::Gemini
        )
    }
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
    /// Where a person gets a key for this decider, for the settings panel.
    #[serde(default)]
    pub key_url: Option<String>,
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
            key_url: None,
            builtin: true,
        },
        // Hosted deciders, asked directly. Models are pinned, because the
        // spec's thresholds are calibrated against what a model answers.
        Manifest {
            id: "jev".into(),
            name: "TypeSafe Jev".into(),
            description: "TypeSafe's System One model, the one the default thresholds are tuned \
                          against. Fast and inexpensive."
                .into(),
            protocol: Protocol::Systemone,
            endpoint: Some("https://api.typesafe.ai/v1/systemone".into()),
            model: Some("jev-1.14.0".into()),
            needs_key: true,
            key_url: Some("https://docs.typesafe.ai/models".into()),
            builtin: true,
        },
        // Chat models, asked for probabilities through a JSON schema. The
        // default thresholds are not tuned against them, so the model here is
        // a cheap default the person can change.
        Manifest {
            id: "openai".into(),
            name: "OpenAI".into(),
            description: "An OpenAI model through the Responses API, with an OpenAI API key."
                .into(),
            protocol: Protocol::OpenaiResponses,
            endpoint: Some("https://api.openai.com/v1/responses".into()),
            model: Some("gpt-6-luna".into()),
            needs_key: true,
            key_url: Some("https://platform.openai.com/api-keys".into()),
            builtin: true,
        },
        Manifest {
            id: "anthropic".into(),
            name: "Anthropic".into(),
            description: "A Claude model through the Messages API, with an Anthropic API key."
                .into(),
            protocol: Protocol::AnthropicMessages,
            endpoint: Some("https://api.anthropic.com/v1/messages".into()),
            model: Some("claude-haiku-5-5".into()),
            needs_key: true,
            key_url: Some("https://console.anthropic.com/settings/keys".into()),
            builtin: true,
        },
        Manifest {
            id: "gemini".into(),
            name: "Google Gemini".into(),
            description: "A Gemini model through the Gemini API, with a Google AI Studio API key."
                .into(),
            protocol: Protocol::Gemini,
            endpoint: Some("https://generativelanguage.googleapis.com/v1beta".into()),
            model: Some("gemini-3.5-flash-lite".into()),
            needs_key: true,
            key_url: Some("https://aistudio.google.com/apikey".into()),
            builtin: true,
        },
        // No default address or model: they are whatever the server serves.
        // No key link either, for the same reason.
        Manifest {
            id: "openai-compatible".into(),
            name: "OpenAI-compatible endpoint".into(),
            description: "Any server that speaks OpenAI Chat Completions with JSON schema \
                          output, such as OpenRouter, DeepSeek, Ollama, vLLM or a LiteLLM proxy. \
                          Give its address and a model."
                .into(),
            protocol: Protocol::OpenaiChat,
            endpoint: None,
            model: None,
            needs_key: true,
            key_url: None,
            builtin: true,
        },
        Manifest {
            id: "openai-decisions".into(),
            name: "OpenAI Decisions".into(),
            description: "OpenAI's Decisions API, which answers with probabilities of its own."
                .into(),
            protocol: Protocol::Decisions,
            endpoint: Some("https://api.openai.com/v1/decisions".into()),
            model: Some("gpt-6-luna".into()),
            needs_key: true,
            key_url: Some("https://platform.openai.com/api-keys".into()),
            builtin: true,
        },
    ]
}

/// Built-in deciders that were removed, and what a saved config naming one
/// becomes: `(old id, new id, the old default endpoint, the old default
/// model)`. The old defaults become overrides, so the same server is asked.
const RETIRED: &[(&str, &str, &str, Option<&str>)] = &[
    (
        "litellm-systemone",
        "jev",
        "http://localhost:4000/v1/systemone",
        Some("jev"),
    ),
    (
        "litellm-decisions",
        "openai-decisions",
        "http://localhost:4000/v1/decisions",
        None,
    ),
];

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
    /// Let a message an agent posted, naming no one, reach the warden too, so
    /// a conversation agents start can carry on without a person in it.
    ///
    /// Configuration rather than spec, because it widens what can wake an
    /// agent on this device; a spec only chooses among what is allowed. What
    /// stops such a conversation is the relay budget, charged to the human
    /// message at its root, not the warden's judgement.
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub agent_messages: bool,
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
            agent_messages: false,
        }
    }
}

impl WardenConfig {
    /// Point a config that names a removed built-in at its replacement, with
    /// the old default endpoint (and model) kept as overrides, so the same
    /// server is still asked. Returns the old id when it changed anything, so
    /// the caller can carry the stored key over (see [`secrets::adopt`]).
    pub fn migrate(&mut self) -> Option<&'static str> {
        let (old, new, endpoint, model) = RETIRED.iter().find(|(old, ..)| *old == self.decider)?;
        self.decider = (*new).to_string();
        if self.endpoint.as_deref().is_none_or(|e| e.trim().is_empty()) {
            self.endpoint = Some((*endpoint).to_string());
        }
        if self.model.as_deref().is_none_or(|m| m.trim().is_empty()) {
            self.model = model.map(str::to_string);
        }
        Some(old)
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

/// A proxy given by its address alone (`https://proxy.example.com/`) means
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

/// The URL a chat-model protocol posts to, from what was configured: the full
/// URL, the API's base (`https://openrouter.ai/api/v1`, as an OpenAI SDK's
/// `base_url`), or the bare origin. Gemini names the model in the path.
fn chat_url(protocol: Protocol, endpoint: &str, model: &str) -> String {
    let trimmed = endpoint.trim();
    let model = model.trim().trim_start_matches("models/");
    let (version, route) = match protocol {
        Protocol::OpenaiChat => ("v1", "chat/completions".to_string()),
        Protocol::OpenaiResponses => ("v1", "responses".to_string()),
        Protocol::AnthropicMessages => ("v1", "messages".to_string()),
        Protocol::Gemini => ("v1beta", format!("models/{model}:generateContent")),
        _ => return trimmed.to_string(),
    };
    let Ok(mut url) = reqwest::Url::parse(trimmed) else {
        return trimmed.to_string();
    };
    let path = url.path().trim_end_matches('/').to_string();
    let complete = match protocol {
        Protocol::Gemini => path.contains(":generateContent"),
        _ => path.ends_with(&format!("/{route}")),
    };
    if complete {
        url.set_path(&path);
    } else if path.is_empty() {
        url.set_path(&format!("/{version}/{route}"));
    } else {
        url.set_path(&format!("{path}/{route}"));
    }
    url.to_string()
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
    let endpoint = if manifest.protocol.is_chat() {
        // A chat API serves many models, and the request must name one.
        let model = model.as_deref().ok_or("no model is configured")?;
        chat_url(manifest.protocol, &endpoint, model)
    } else {
        endpoint
    };
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

    /// Give `new` the key stored for `old`, after a config was migrated from
    /// one to the other (see [`super::WardenConfig::migrate`]). A key `new`
    /// already has is kept: it was set on purpose.
    pub fn adopt(old: &str, new: &str) {
        let all = load();
        if all.contains_key(new) {
            return;
        }
        let Some(entry) = all.get(old).filter(|e| !e.api_key.is_empty()) else {
            return;
        };
        if let Err(error) = set(new, Some(&entry.api_key)) {
            tracing::warn!("[warden] could not carry the {old} key over to {new}: {error}");
        }
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
    /// In USD, when the server reports it in a response-cost header (see
    /// [`COST_HEADER`]).
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
        Protocol::OpenaiChat => json!({
            "model": model,
            "messages": [
                { "role": "system", "content": chat::SYSTEM },
                { "role": "user", "content": chat::prompt(input, questions) },
            ],
            "response_format": {
                "type": "json_schema",
                "json_schema": {
                    "name": chat::SCHEMA_NAME,
                    "strict": true,
                    "schema": chat::schema(questions),
                },
            },
        }),
        Protocol::OpenaiResponses => json!({
            "model": model,
            "instructions": chat::SYSTEM,
            "input": chat::prompt(input, questions),
            "text": {
                "format": {
                    "type": "json_schema",
                    "name": chat::SCHEMA_NAME,
                    "strict": true,
                    "schema": chat::schema(questions),
                },
            },
            "store": false,
        }),
        Protocol::AnthropicMessages => json!({
            "model": model,
            "max_tokens": 1024,
            "system": chat::SYSTEM,
            "messages": [ { "role": "user", "content": chat::prompt(input, questions) } ],
            "tools": [{
                "name": chat::SCHEMA_NAME,
                "description": "Record the probability for every question.",
                "input_schema": chat::schema(questions),
            }],
            "tool_choice": { "type": "tool", "name": chat::SCHEMA_NAME },
        }),
        // The model is in the URL, not the body.
        Protocol::Gemini => json!({
            "systemInstruction": { "parts": [ { "text": chat::SYSTEM } ] },
            "contents": [
                { "role": "user", "parts": [ { "text": chat::prompt(input, questions) } ] },
            ],
            "generationConfig": {
                "responseMimeType": "application/json",
                "responseJsonSchema": chat::schema(questions),
            },
        }),
    }
}

/// Chat models asked for probabilities: one JSON schema, built from the
/// questions, that every chat protocol carries its own way, and the reading
/// of what comes back.
///
/// Questions and options are properties named after them, and are read back
/// by name, never by position: Gemini does not keep property order. Names are
/// made safe for every provider's schema rules (`[A-Za-z0-9_-]`, at most 64),
/// so a candidate called `fake-acp:say-a#a` is asked about as
/// `fake-acp_say-a_a`.
mod chat {
    use super::{Answer, Ask, Question};
    use serde_json::{json, Map, Value};
    use std::collections::{BTreeMap, HashSet};

    /// The tool, schema or format name, where a provider wants one.
    pub const SCHEMA_NAME: &str = "warden_answers";

    pub const SYSTEM: &str = "You estimate probabilities for a group chat. Read the input, \
        then answer every question with numbers between 0 and 1. For a yes/no question, \
        p_yes is the probability that the answer is yes. For a choice, give every option \
        the probability that it is the right one, adding up to 1. Answer only through the \
        schema.";

    fn safe(raw: &str, taken: &mut HashSet<String>) -> String {
        let mut base: String = raw
            .chars()
            .map(|c| {
                if c.is_ascii_alphanumeric() || c == '_' || c == '-' {
                    c
                } else {
                    '_'
                }
            })
            .take(56)
            .collect();
        if base.is_empty() {
            base.push('_');
        }
        let mut key = base.clone();
        let mut n = 2;
        while !taken.insert(key.clone()) {
            key = format!("{base}_{n}");
            n += 1;
        }
        key
    }

    /// Each question's property name, and for a choice each option's,
    /// alongside the question. Deterministic, so the request and the reading
    /// agree.
    pub fn keys(questions: &[Question]) -> Vec<(String, Vec<(String, String)>)> {
        let mut taken = HashSet::new();
        questions
            .iter()
            .map(|q| {
                let key = safe(&q.name, &mut taken);
                let options = match &q.ask {
                    Ask::YesNo => Vec::new(),
                    Ask::Choice(options) => {
                        let mut taken = HashSet::new();
                        options
                            .iter()
                            .map(|(value, _)| (safe(value, &mut taken), value.clone()))
                            .collect()
                    }
                };
                (key, options)
            })
            .collect()
    }

    fn probability() -> Value {
        json!({ "type": "number", "minimum": 0, "maximum": 1 })
    }

    /// Strict: every object closed and every property required, as OpenAI's
    /// strict mode demands; the others accept the same.
    fn object(properties: Map<String, Value>) -> Value {
        let required: Vec<&String> = properties.keys().collect();
        json!({
            "type": "object",
            "properties": properties,
            "required": required,
            "additionalProperties": false,
        })
    }

    pub fn schema(questions: &[Question]) -> Value {
        let mut properties = Map::new();
        for (q, (key, options)) in questions.iter().zip(keys(questions)) {
            let inner = match &q.ask {
                Ask::YesNo => {
                    let mut p = Map::new();
                    p.insert("p_yes".into(), probability());
                    object(p)
                }
                Ask::Choice(_) => object(
                    options
                        .iter()
                        .map(|(key, _)| (key.clone(), probability()))
                        .collect(),
                ),
            };
            properties.insert(key, inner);
        }
        object(properties)
    }

    /// The input, then the questions under the names the schema uses.
    pub fn prompt(input: &str, questions: &[Question]) -> String {
        let mut out = format!("{input}\n\nQuestions:\n");
        for (q, (key, options)) in questions.iter().zip(keys(questions)) {
            match &q.ask {
                Ask::YesNo => out.push_str(&format!("- {key} (yes/no): {}\n", q.instructions)),
                Ask::Choice(described) => {
                    out.push_str(&format!("- {key} (choose one): {}\n", q.instructions));
                    for ((key, _), (_, description)) in options.iter().zip(described) {
                        out.push_str(&format!("  - {key}: {description}\n"));
                    }
                }
            }
        }
        out
    }

    fn p(value: Option<&Value>) -> Option<f64> {
        value
            .and_then(Value::as_f64)
            .filter(|p| p.is_finite())
            .map(|p| p.clamp(0.0, 1.0))
    }

    /// Every question's answer from the object the model filled in. A
    /// question missing from it, or answered with something other than
    /// probabilities, is refused; unknown keys are dropped. `None` — nothing
    /// usable came back at all — refuses every question.
    pub fn answers(object: Option<&Value>, questions: &[Question]) -> BTreeMap<String, Answer> {
        let object = object.and_then(Value::as_object);
        questions
            .iter()
            .zip(keys(questions))
            .map(|(q, (key, options))| {
                let given = object.and_then(|o| o.get(&key));
                let answer = match &q.ask {
                    Ask::YesNo => match p(given.and_then(|g| g.get("p_yes")))
                        // A bare number is what was meant.
                        .or_else(|| p(given))
                    {
                        Some(p) => Answer::YesNo { p },
                        None => Answer::Refused,
                    },
                    Ask::Choice(_) => {
                        let mut probs: BTreeMap<String, f64> = options
                            .iter()
                            .filter_map(|(key, value)| Some((value.clone(), p(given?.get(key))?)))
                            .collect();
                        let total: f64 = probs.values().sum();
                        if total > 0.0 {
                            probs.values_mut().for_each(|p| *p /= total);
                            Answer::Choice { p: probs }
                        } else {
                            Answer::Refused
                        }
                    }
                };
                (q.name.clone(), answer)
            })
            .collect()
    }

    /// A JSON object the model wrote as text.
    pub fn from_text(text: Option<&str>) -> Option<Value> {
        let text = text?.trim();
        // Some compatible servers wrap it in a code fence despite the schema.
        let text = text
            .strip_prefix("```json")
            .or_else(|| text.strip_prefix("```"))
            .and_then(|t| t.strip_suffix("```"))
            .map(str::trim)
            .unwrap_or(text);
        serde_json::from_str::<Value>(text)
            .ok()
            .filter(Value::is_object)
    }

    /// The filled-in object from a response body, or `None` when the model
    /// refused, was cut off, or answered with something unreadable.
    pub fn output(protocol: super::Protocol, body: &Value) -> Option<Value> {
        use super::Protocol;
        match protocol {
            Protocol::OpenaiChat => {
                let message = body.pointer("/choices/0/message")?;
                if message
                    .get("refusal")
                    .is_some_and(|r| !r.is_null() && r.as_str() != Some(""))
                {
                    return None;
                }
                from_text(message.get("content").and_then(Value::as_str))
            }
            Protocol::OpenaiResponses => {
                if body.get("status").and_then(Value::as_str) == Some("incomplete") {
                    return None;
                }
                let message = body
                    .get("output")?
                    .as_array()?
                    .iter()
                    .find(|item| item.get("type").and_then(Value::as_str) == Some("message"))?;
                let parts = message.get("content")?.as_array()?;
                let kind =
                    |part: &Value| part.get("type").and_then(Value::as_str).map(str::to_string);
                if parts
                    .iter()
                    .any(|part| kind(part).as_deref() == Some("refusal"))
                {
                    return None;
                }
                let part = parts
                    .iter()
                    .find(|part| kind(part).as_deref() == Some("output_text"))?;
                from_text(part.get("text").and_then(Value::as_str))
            }
            Protocol::AnthropicMessages => {
                if body.get("stop_reason").and_then(Value::as_str) == Some("refusal") {
                    return None;
                }
                body.get("content")?
                    .as_array()?
                    .iter()
                    .find(|block| {
                        block.get("type").and_then(Value::as_str) == Some("tool_use")
                            && block.get("name").and_then(Value::as_str) == Some(SCHEMA_NAME)
                    })?
                    .get("input")
                    .filter(|input| input.is_object())
                    .cloned()
            }
            Protocol::Gemini => {
                // The text may come in several parts; thoughts are not it.
                let parts = body.pointer("/candidates/0/content/parts")?.as_array()?;
                let text: String = parts
                    .iter()
                    .filter(|part| part.get("thought").and_then(Value::as_bool) != Some(true))
                    .filter_map(|part| part.get("text").and_then(Value::as_str))
                    .collect();
                from_text(Some(&text))
            }
            _ => None,
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
    let at = |path: &str| body.pointer(path).and_then(Value::as_u64);
    // Input tokens count the cached ones too, whichever way a provider
    // reports them.
    let (input_tokens, cached_tokens) = match protocol {
        Protocol::OpenaiChat => (
            at("/usage/prompt_tokens"),
            at("/usage/prompt_tokens_details/cached_tokens"),
        ),
        Protocol::AnthropicMessages => {
            // Anthropic's `input_tokens` leaves out what was read from or
            // written to the cache.
            let read = at("/usage/cache_read_input_tokens");
            let written = at("/usage/cache_creation_input_tokens");
            (
                at("/usage/input_tokens").map(|n| n + read.unwrap_or(0) + written.unwrap_or(0)),
                read,
            )
        }
        Protocol::Gemini => (
            at("/usageMetadata/promptTokenCount"),
            at("/usageMetadata/cachedContentTokenCount"),
        ),
        _ => (
            at("/usage/input_tokens"),
            at("/usage/input_tokens_details/cached_tokens").or_else(|| at("/usage/cached_tokens")),
        ),
    };
    let model = match protocol {
        Protocol::Gemini => body.get("modelVersion"),
        _ => body.get("model"),
    };
    let mut reply = Reply {
        model: model.and_then(Value::as_str).map(str::to_string),
        input_tokens,
        cached_tokens,
        ..Reply::default()
    };
    if protocol.is_chat() {
        reply.answers = chat::answers(chat::output(protocol, body).as_ref(), questions);
        return reply;
    }
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

pub mod breaker {
    //! Stop asking a decider that keeps failing. Without this, a decider that
    //! is down or rate limited costs every message its full timeout before the
    //! rules decide. After [`THRESHOLD`] failures in a row it is skipped for
    //! [`COOLDOWN`]; the first ask after that is a probe, and one success
    //! closes it again.
    //!
    //! Keyed by decider, endpoint and key, so changing any of them — the usual
    //! fix — starts afresh rather than waiting out the cooldown.

    use std::collections::HashMap;
    use std::sync::Mutex;
    use std::time::{Duration, Instant};

    pub const THRESHOLD: u32 = 3;
    pub const COOLDOWN: Duration = Duration::from_secs(60);

    #[derive(Default)]
    struct State {
        failures: u32,
        open_until: Option<Instant>,
    }

    static STATES: Mutex<Option<HashMap<u64, State>>> = Mutex::new(None);

    pub fn key(decider: &super::Resolved) -> u64 {
        use std::hash::{Hash, Hasher};
        let mut h = std::collections::hash_map::DefaultHasher::new();
        decider.manifest.id.hash(&mut h);
        decider.endpoint.hash(&mut h);
        decider.key.hash(&mut h);
        h.finish()
    }

    fn with<T>(f: impl FnOnce(&mut HashMap<u64, State>) -> T) -> T {
        let mut guard = STATES.lock().unwrap_or_else(|e| e.into_inner());
        f(guard.get_or_insert_with(HashMap::new))
    }

    /// How long until `key` may be asked again, if it is paused now.
    pub fn paused(key: u64, now: Instant) -> Option<Duration> {
        with(|all| {
            let until = all.get(&key)?.open_until?;
            (until > now).then(|| until - now)
        })
    }

    pub fn record(key: u64, ok: bool, now: Instant) {
        with(|all| {
            if ok {
                all.remove(&key);
                return;
            }
            let state = all.entry(key).or_default();
            state.failures += 1;
            if state.failures >= THRESHOLD {
                state.open_until = Some(now + COOLDOWN);
            }
        })
    }

    /// Forget every failure, so the next ask goes out. For an explicit test.
    pub fn reset() {
        with(|all| all.clear())
    }
}

/// Put the questions to a decider over HTTP, unless it keeps failing (see
/// [`breaker`]).
pub async fn ask(decider: &Resolved, input: &str, questions: &[Question]) -> anyhow::Result<Reply> {
    let key = breaker::key(decider);
    if let Some(left) = breaker::paused(key, std::time::Instant::now()) {
        anyhow::bail!(
            "paused after {} failures in a row; trying again in {}s",
            breaker::THRESHOLD,
            left.as_secs().max(1)
        );
    }
    let result = ask_now(decider, input, questions).await;
    breaker::record(key, result.is_ok(), std::time::Instant::now());
    result
}

/// What a request's cost comes back in, in USD, from a proxy that reports it.
/// LiteLLM sends it; servers that do not are simply not costed.
pub const COST_HEADER: &str = "x-litellm-response-cost";

/// The version of the Anthropic Messages API the request is written for.
const ANTHROPIC_VERSION: &str = "2023-06-01";

/// Put the key where `protocol` expects it.
fn authorize(
    request: reqwest::RequestBuilder,
    protocol: Protocol,
    key: Option<&str>,
) -> reqwest::RequestBuilder {
    let request = match protocol {
        // Sent even without a key: it says which API the body is written for.
        Protocol::AnthropicMessages => request.header("anthropic-version", ANTHROPIC_VERSION),
        _ => request,
    };
    let Some(key) = key else {
        return request;
    };
    let header = match protocol {
        Protocol::AnthropicMessages => "x-api-key",
        Protocol::Gemini => "x-goog-api-key",
        _ => return request.bearer_auth(key),
    };
    // Marked sensitive, as `bearer_auth` marks its own, so it is not logged.
    match reqwest::header::HeaderValue::from_str(key) {
        Ok(mut value) => {
            value.set_sensitive(true);
            request.header(header, value)
        }
        // Not a valid header value: let the request fail as unauthorised
        // rather than send something else.
        Err(_) => request,
    }
}

async fn ask_now(decider: &Resolved, input: &str, questions: &[Question]) -> anyhow::Result<Reply> {
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
    let request = crate::outbound::client()
        .post(endpoint)
        .timeout(decider.timeout)
        .json(&body);
    let request = authorize(request, decider.manifest.protocol, decider.key.as_deref());
    let response = request.send().await?;
    let status = response.status();
    let cost_usd = response
        .headers()
        .get(COST_HEADER)
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
            with_default_path("https://proxy.example.com/", default),
            "https://proxy.example.com/v1/decisions"
        );
        assert_eq!(
            with_default_path("https://proxy.example.com", default),
            "https://proxy.example.com/v1/decisions"
        );
        // A path someone wrote is theirs.
        assert_eq!(
            with_default_path("https://proxy.example.com/team/v1/decisions", default),
            "https://proxy.example.com/team/v1/decisions"
        );
        assert_eq!(
            with_default_path("https://proxy.example.com/", None),
            "https://proxy.example.com/"
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
            decider: "openai-decisions".into(),
            key_env: Some("ENOXIAN_TEST_WARDEN_KEY_THAT_IS_NOT_SET".into()),
            ..WardenConfig::default()
        };
        assert_eq!(resolve(&cfg).unwrap_err(), "no API key is set");
    }

    #[test]
    fn hosted_deciders_are_pinned_and_need_a_key() {
        let jev = find("jev").unwrap();
        assert_eq!(jev.protocol, Protocol::Systemone);
        assert_eq!(
            jev.endpoint.as_deref(),
            Some("https://api.typesafe.ai/v1/systemone")
        );
        assert_eq!(jev.model.as_deref(), Some("jev-1.14.0"));
        assert!(jev.needs_key && jev.builtin);
        let openai = find("openai-decisions").unwrap();
        assert_eq!(openai.protocol, Protocol::Decisions);
        assert_eq!(
            openai.endpoint.as_deref(),
            Some("https://api.openai.com/v1/decisions")
        );
        assert_eq!(openai.model.as_deref(), Some("gpt-6-luna"));
        assert!(openai.needs_key);
    }

    #[test]
    fn the_breaker_opens_after_repeated_failures_and_closes_on_success() {
        let key = 0x0b4e_a4e7_u64;
        let now = std::time::Instant::now();
        for _ in 0..breaker::THRESHOLD - 1 {
            breaker::record(key, false, now);
        }
        assert!(breaker::paused(key, now).is_none(), "not yet");
        breaker::record(key, false, now);
        assert!(breaker::paused(key, now).is_some());
        let later = now + breaker::COOLDOWN + std::time::Duration::from_secs(1);
        assert!(breaker::paused(key, later).is_none(), "a probe may go out");
        // The probe fails: paused again at once, without three more failures.
        breaker::record(key, false, later);
        assert!(breaker::paused(key, later).is_some());
        breaker::record(key, true, later);
        assert!(breaker::paused(key, later).is_none());
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
            manifest: find("jev").unwrap(),
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

    // ---- Chat-model protocols ------------------------------------------

    /// Questions whose names and options are not safe as schema keys, as a
    /// real candidate list's are not.
    fn awkward() -> Vec<Question> {
        vec![
            Question {
                name: "success:single".into(),
                instructions: "one answer enough?".into(),
                ask: Ask::YesNo,
            },
            Question {
                name: "agent".into(),
                instructions: "who?".into(),
                ask: Ask::Choice(vec![
                    ("fake-acp:say-a#a".into(), "says a".into()),
                    ("fake-acp:say-a@a".into(), "also says a".into()),
                    ("codex".into(), "runs tests".into()),
                ]),
            },
        ]
    }

    #[test]
    fn chat_keys_are_safe_unique_and_stable() {
        let keys = chat::keys(&awkward());
        assert_eq!(keys[0].0, "success_single");
        assert_eq!(keys[1].0, "agent");
        let options: Vec<&str> = keys[1].1.iter().map(|(k, _)| k.as_str()).collect();
        assert_eq!(options, ["fake-acp_say-a_a", "fake-acp_say-a_a_2", "codex"]);
        assert_eq!(
            keys,
            chat::keys(&awkward()),
            "the reading agrees with the request"
        );
    }

    #[test]
    fn the_chat_schema_is_strict() {
        let schema = chat::schema(&awkward());
        assert_eq!(schema["additionalProperties"], false);
        assert_eq!(schema["required"], json!(["agent", "success_single"]));
        let yes = &schema["properties"]["success_single"];
        assert_eq!(yes["required"], json!(["p_yes"]));
        assert_eq!(yes["additionalProperties"], false);
        assert_eq!(yes["properties"]["p_yes"]["maximum"], 1);
        let agent = &schema["properties"]["agent"];
        assert_eq!(agent["additionalProperties"], false);
        assert_eq!(agent["required"].as_array().unwrap().len(), 3);
        assert_eq!(agent["properties"]["codex"]["type"], "number");
        let prompt = chat::prompt("hello", &awkward());
        assert!(prompt.starts_with("hello\n"));
        assert!(prompt.contains("- success_single (yes/no): one answer enough?"));
        assert!(prompt.contains("  - fake-acp_say-a_a_2: also says a"));
    }

    #[test]
    fn openai_chat_request_uses_a_strict_json_schema() {
        let body = request_body(Protocol::OpenaiChat, Some("gpt-x"), "hi", &questions());
        assert_eq!(body["model"], "gpt-x");
        assert_eq!(body["messages"][0]["role"], "system");
        assert!(body["messages"][1]["content"]
            .as_str()
            .unwrap()
            .starts_with("hi"));
        let format = &body["response_format"];
        assert_eq!(format["type"], "json_schema");
        assert_eq!(format["json_schema"]["name"], chat::SCHEMA_NAME);
        assert_eq!(format["json_schema"]["strict"], true);
        assert_eq!(
            format["json_schema"]["schema"]["properties"]["admit"]["required"],
            json!(["p_yes"])
        );
    }

    #[test]
    fn openai_responses_request_uses_text_format_and_does_not_store() {
        let body = request_body(Protocol::OpenaiResponses, Some("gpt-x"), "hi", &questions());
        assert_eq!(body["store"], false);
        assert_eq!(body["instructions"], chat::SYSTEM);
        assert!(body["input"].as_str().unwrap().starts_with("hi"));
        let format = &body["text"]["format"];
        assert_eq!(format["type"], "json_schema");
        assert_eq!(format["name"], chat::SCHEMA_NAME);
        assert_eq!(format["strict"], true);
        assert!(format["schema"]["properties"]["agent"]["properties"]["codex"].is_object());
    }

    #[test]
    fn anthropic_request_forces_one_tool() {
        let body = request_body(
            Protocol::AnthropicMessages,
            Some("claude-x"),
            "hi",
            &questions(),
        );
        assert_eq!(body["model"], "claude-x");
        assert!(body["max_tokens"].as_u64().unwrap() > 0);
        assert_eq!(body["system"], chat::SYSTEM);
        assert_eq!(body["tools"][0]["name"], chat::SCHEMA_NAME);
        assert_eq!(body["tools"][0]["input_schema"]["type"], "object");
        assert_eq!(
            body["tool_choice"],
            json!({ "type": "tool", "name": chat::SCHEMA_NAME })
        );
    }

    #[test]
    fn gemini_request_asks_for_json_by_schema_and_leaves_the_model_out() {
        let body = request_body(Protocol::Gemini, Some("gemini-x"), "hi", &questions());
        assert!(body.get("model").is_none());
        let config = &body["generationConfig"];
        assert_eq!(config["responseMimeType"], "application/json");
        assert!(config.get("responseSchema").is_none(), "deprecated");
        assert_eq!(config["responseJsonSchema"]["type"], "object");
        assert_eq!(body["systemInstruction"]["parts"][0]["text"], chat::SYSTEM);
        assert!(body["contents"][0]["parts"][0]["text"]
            .as_str()
            .unwrap()
            .starts_with("hi"));
    }

    /// The answers a model would fill in, keyed as the schema asks.
    fn filled() -> Value {
        json!({
            // Reordered, as Gemini may; an unknown key is dropped.
            "agent": { "codex": 0.3, "claude": 0.9, "nobody": 0.5 },
            "admit": { "p_yes": 1.4 },
        })
    }

    fn assert_filled(reply: &Reply) {
        assert_eq!(reply.yes("admit"), Some(1.0), "clamped");
        let agent = reply.choice("agent").unwrap();
        assert_eq!(agent.len(), 2, "unknown option dropped");
        assert!((agent["claude"] - 0.75).abs() < 1e-9, "renormalised");
        assert!((agent["codex"] - 0.25).abs() < 1e-9);
    }

    #[test]
    fn openai_chat_response_is_read_by_name() {
        let body = json!({
            "model": "gpt-x-2026",
            "choices": [{ "message": { "role": "assistant", "content": filled().to_string(), "refusal": null } }],
            "usage": { "prompt_tokens": 120, "prompt_tokens_details": { "cached_tokens": 64 } }
        });
        let reply = parse_response(Protocol::OpenaiChat, &body, &questions());
        assert_filled(&reply);
        assert_eq!(reply.model.as_deref(), Some("gpt-x-2026"));
        assert_eq!(reply.input_tokens, Some(120));
        assert_eq!(reply.cached_tokens, Some(64));
    }

    #[test]
    fn openai_chat_refusal_and_junk_are_refused() {
        let refused = json!({
            "choices": [{ "message": { "content": null, "refusal": "I can't help with that." } }]
        });
        let reply = parse_response(Protocol::OpenaiChat, &refused, &questions());
        assert_eq!(reply.answers["admit"], Answer::Refused);
        assert_eq!(reply.answers["agent"], Answer::Refused);
        let junk = json!({ "choices": [{ "message": { "content": "{\"admit\": " } }] });
        let reply = parse_response(Protocol::OpenaiChat, &junk, &questions());
        assert_eq!(reply.answers.len(), 2);
        assert!(reply.answers.values().all(|a| *a == Answer::Refused));
        let empty = json!({});
        let reply = parse_response(Protocol::OpenaiChat, &empty, &questions());
        assert!(reply.answers.values().all(|a| *a == Answer::Refused));
    }

    #[test]
    fn a_partial_or_malformed_answer_refuses_only_that_question() {
        let partial =
            json!({ "admit": { "p_yes": "high" }, "agent": { "claude": 0.0, "codex": 0.0 } });
        let answers = chat::answers(Some(&partial), &questions());
        assert_eq!(answers["admit"], Answer::Refused, "not a number");
        assert_eq!(answers["agent"], Answer::Refused, "no mass anywhere");
        let one = json!({ "agent": { "codex": 0.4 } });
        let answers = chat::answers(Some(&one), &questions());
        assert_eq!(answers["admit"], Answer::Refused, "missing");
        assert_eq!(
            answers["agent"],
            Answer::Choice {
                p: BTreeMap::from([("codex".to_string(), 1.0)])
            }
        );
        // A bare number for a yes/no is taken as p_yes.
        let bare = json!({ "admit": 0.2 });
        assert_eq!(
            chat::answers(Some(&bare), &questions())["admit"],
            Answer::YesNo { p: 0.2 }
        );
    }

    #[test]
    fn awkward_names_are_read_back_under_their_own_names() {
        let body = json!({
            "success_single": { "p_yes": 0.6 },
            "agent": { "fake-acp_say-a_a_2": 0.2, "fake-acp_say-a_a": 0.6, "codex": 0.2 },
        });
        let answers = chat::answers(Some(&body), &awkward());
        assert_eq!(answers["success:single"], Answer::YesNo { p: 0.6 });
        let Answer::Choice { p } = &answers["agent"] else {
            panic!("a choice")
        };
        assert!((p["fake-acp:say-a#a"] - 0.6).abs() < 1e-9);
        assert!((p["fake-acp:say-a@a"] - 0.2).abs() < 1e-9);
    }

    #[test]
    fn openai_responses_response_is_read_from_the_message_item() {
        let body = json!({
            "model": "gpt-x",
            "status": "completed",
            "output": [
                { "type": "reasoning", "summary": [] },
                { "type": "message", "content": [ { "type": "output_text", "text": filled().to_string() } ] }
            ],
            "usage": { "input_tokens": 99, "input_tokens_details": { "cached_tokens": 3 } }
        });
        let reply = parse_response(Protocol::OpenaiResponses, &body, &questions());
        assert_filled(&reply);
        assert_eq!(reply.input_tokens, Some(99));
        assert_eq!(reply.cached_tokens, Some(3));
    }

    #[test]
    fn openai_responses_refusal_or_incomplete_is_refused() {
        let refused = json!({
            "status": "completed",
            "output": [ { "type": "message", "content": [ { "type": "refusal", "refusal": "no" } ] } ]
        });
        let reply = parse_response(Protocol::OpenaiResponses, &refused, &questions());
        assert!(reply.answers.values().all(|a| *a == Answer::Refused));
        let incomplete = json!({
            "status": "incomplete",
            "incomplete_details": { "reason": "max_output_tokens" },
            "output": [ { "type": "message", "content": [ { "type": "output_text", "text": "{\"admit\":{\"p_yes\":0.5}}" } ] } ]
        });
        let reply = parse_response(Protocol::OpenaiResponses, &incomplete, &questions());
        assert!(reply.answers.values().all(|a| *a == Answer::Refused));
    }

    #[test]
    fn anthropic_response_is_read_from_the_tool_call() {
        let body = json!({
            "model": "claude-x",
            "stop_reason": "tool_use",
            "content": [
                { "type": "text", "text": "Sure." },
                { "type": "tool_use", "id": "t1", "name": chat::SCHEMA_NAME, "input": filled() }
            ],
            "usage": { "input_tokens": 10, "cache_read_input_tokens": 200, "cache_creation_input_tokens": 5 }
        });
        let reply = parse_response(Protocol::AnthropicMessages, &body, &questions());
        assert_filled(&reply);
        assert_eq!(reply.input_tokens, Some(215));
        assert_eq!(reply.cached_tokens, Some(200));
        let refused = json!({ "stop_reason": "refusal", "content": [] });
        let reply = parse_response(Protocol::AnthropicMessages, &refused, &questions());
        assert!(reply.answers.values().all(|a| *a == Answer::Refused));
        let text_only =
            json!({ "stop_reason": "end_turn", "content": [ { "type": "text", "text": "{}" } ] });
        let reply = parse_response(Protocol::AnthropicMessages, &text_only, &questions());
        assert!(reply.answers.values().all(|a| *a == Answer::Refused));
    }

    #[test]
    fn gemini_response_is_read_from_the_candidate_text() {
        let text = filled().to_string();
        let (a, b) = text.split_at(text.len() / 2);
        let body = json!({
            "modelVersion": "gemini-x-001",
            "candidates": [{ "content": { "role": "model", "parts": [
                { "text": "thinking...", "thought": true },
                { "text": a }, { "text": b }
            ] }, "finishReason": "STOP" }],
            "usageMetadata": { "promptTokenCount": 300, "cachedContentTokenCount": 256 }
        });
        let reply = parse_response(Protocol::Gemini, &body, &questions());
        assert_filled(&reply);
        assert_eq!(reply.model.as_deref(), Some("gemini-x-001"));
        assert_eq!(reply.input_tokens, Some(300));
        assert_eq!(reply.cached_tokens, Some(256));
        let blocked = json!({ "promptFeedback": { "blockReason": "SAFETY" } });
        let reply = parse_response(Protocol::Gemini, &blocked, &questions());
        assert!(reply.answers.values().all(|a| *a == Answer::Refused));
    }

    #[test]
    fn a_fenced_json_answer_is_still_read() {
        let fenced = format!("```json\n{}\n```", filled());
        assert!(chat::from_text(Some(&fenced)).is_some());
        assert!(chat::from_text(Some("[1, 2]")).is_none(), "not an object");
    }

    #[test]
    fn chat_urls_accept_a_full_url_a_base_or_an_origin() {
        use Protocol::*;
        let cases = [
            (
                OpenaiChat,
                "https://openrouter.ai/api/v1",
                "https://openrouter.ai/api/v1/chat/completions",
            ),
            (
                OpenaiChat,
                "http://localhost:11434",
                "http://localhost:11434/v1/chat/completions",
            ),
            (
                OpenaiChat,
                "http://localhost:4000/v1/chat/completions/",
                "http://localhost:4000/v1/chat/completions",
            ),
            (
                OpenaiResponses,
                "https://api.openai.com/v1/responses",
                "https://api.openai.com/v1/responses",
            ),
            (
                OpenaiResponses,
                "https://api.openai.com",
                "https://api.openai.com/v1/responses",
            ),
            (
                AnthropicMessages,
                "https://api.anthropic.com",
                "https://api.anthropic.com/v1/messages",
            ),
            (
                AnthropicMessages,
                "https://gw.example.com/anthropic/v1",
                "https://gw.example.com/anthropic/v1/messages",
            ),
            (
                Gemini,
                "https://generativelanguage.googleapis.com/v1beta",
                "https://generativelanguage.googleapis.com/v1beta/models/m-1:generateContent",
            ),
            (
                Gemini,
                "https://generativelanguage.googleapis.com",
                "https://generativelanguage.googleapis.com/v1beta/models/m-1:generateContent",
            ),
            (
                Gemini,
                "https://g.example.com/v1beta/models/other:generateContent",
                "https://g.example.com/v1beta/models/other:generateContent",
            ),
            (
                OpenaiChat,
                "https://proxy.example.com/v1?team=x",
                "https://proxy.example.com/v1/chat/completions?team=x",
            ),
        ];
        for (protocol, endpoint, want) in cases {
            assert_eq!(
                chat_url(protocol, endpoint, "models/m-1"),
                want,
                "{endpoint}"
            );
        }
    }

    fn resolve_with_key(
        decider: &str,
        endpoint: Option<&str>,
        model: Option<&str>,
    ) -> Result<Resolved, String> {
        resolve(&WardenConfig {
            decider: decider.into(),
            endpoint: endpoint.map(str::to_string),
            model: model.map(str::to_string),
            key_env: Some("PATH".into()),
            ..WardenConfig::default()
        })
    }

    #[test]
    fn chat_builtins_resolve_to_their_routes() {
        let openai = resolve_with_key("openai", None, None).unwrap();
        assert_eq!(openai.manifest.protocol, Protocol::OpenaiResponses);
        assert_eq!(
            openai.endpoint.as_deref(),
            Some("https://api.openai.com/v1/responses")
        );
        let anthropic = resolve_with_key("anthropic", None, None).unwrap();
        assert_eq!(
            anthropic.endpoint.as_deref(),
            Some("https://api.anthropic.com/v1/messages")
        );
        assert_eq!(anthropic.model.as_deref(), Some("claude-haiku-5-5"));
        let gemini = resolve_with_key("gemini", None, Some("gemini-y")).unwrap();
        assert_eq!(
            gemini.endpoint.as_deref(),
            Some(
                "https://generativelanguage.googleapis.com/v1beta/models/gemini-y:generateContent"
            )
        );
        // A compatible endpoint has nothing to fall back on.
        assert_eq!(
            resolve_with_key("openai-compatible", None, Some("m")).unwrap_err(),
            "no endpoint is configured"
        );
        assert_eq!(
            resolve_with_key(
                "openai-compatible",
                Some("https://openrouter.ai/api/v1"),
                None
            )
            .unwrap_err(),
            "no model is configured"
        );
        let compatible = resolve_with_key(
            "openai-compatible",
            Some("https://openrouter.ai/api/v1"),
            Some("m"),
        )
        .unwrap();
        assert_eq!(
            compatible.endpoint.as_deref(),
            Some("https://openrouter.ai/api/v1/chat/completions")
        );
        for id in ["openai", "anthropic", "gemini"] {
            let m = find(id).unwrap();
            assert!(
                m.builtin && m.needs_key && m.key_url.is_some() && m.model.is_some(),
                "{id}"
            );
        }
    }

    #[test]
    fn litellm_builtins_are_gone_and_saved_configs_migrate() {
        assert!(find("litellm-systemone").is_none());
        assert!(find("litellm-decisions").is_none());

        let mut cfg = WardenConfig {
            decider: "litellm-systemone".into(),
            ..WardenConfig::default()
        };
        assert_eq!(cfg.migrate(), Some("litellm-systemone"));
        assert_eq!(cfg.decider, "jev");
        assert_eq!(
            cfg.endpoint.as_deref(),
            Some("http://localhost:4000/v1/systemone")
        );
        assert_eq!(cfg.model.as_deref(), Some("jev"), "the proxy's model name");
        assert_eq!(cfg.migrate(), None, "once");

        let mut cfg = WardenConfig {
            decider: "litellm-decisions".into(),
            endpoint: Some("https://proxy.example.com".into()),
            model: Some("my-model".into()),
            timeout_secs: 9,
            ..WardenConfig::default()
        };
        assert_eq!(cfg.migrate(), Some("litellm-decisions"));
        assert_eq!(cfg.decider, "openai-decisions");
        assert_eq!(
            cfg.endpoint.as_deref(),
            Some("https://proxy.example.com"),
            "kept"
        );
        assert_eq!(cfg.model.as_deref(), Some("my-model"));
        assert_eq!(cfg.timeout_secs, 9);
        // And it resolves to the proxy's route, not OpenAI's.
        cfg.key_env = Some("PATH".into());
        assert_eq!(
            resolve(&cfg).unwrap().endpoint.as_deref(),
            Some("https://proxy.example.com/v1/decisions")
        );

        let mut current = WardenConfig {
            decider: "jev".into(),
            ..WardenConfig::default()
        };
        assert_eq!(current.migrate(), None);
        assert_eq!(current.endpoint, None);
    }

    #[test]
    fn a_saved_litellm_config_loads_as_its_replacement() {
        let cfg = crate::agent::config::AgentConfig::from_toml(
            "[warden]\ndecider = \"litellm-decisions\"\nendpoint = \"http://10.0.0.2:4000\"\n",
        )
        .unwrap();
        assert_eq!(cfg.warden.decider, "openai-decisions");
        assert_eq!(cfg.warden.endpoint.as_deref(), Some("http://10.0.0.2:4000"));
    }

    /// Serve one canned JSON response, and hand back the raw request.
    async fn serve_once(reply: Value) -> (std::net::SocketAddr, tokio::task::JoinHandle<String>) {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            let mut buf = vec![0u8; 16384];
            let mut seen = String::new();
            loop {
                let n = socket.read(&mut buf).await.unwrap();
                if n == 0 {
                    break;
                }
                seen.push_str(&String::from_utf8_lossy(&buf[..n]));
                let Some((head, body)) = seen.split_once("\r\n\r\n") else {
                    continue;
                };
                let length = head
                    .lines()
                    .find_map(|l| {
                        let (k, v) = l.split_once(':')?;
                        k.eq_ignore_ascii_case("content-length")
                            .then(|| v.trim().parse::<usize>().ok())?
                    })
                    .unwrap_or(0);
                if body.len() >= length {
                    break;
                }
            }
            let body = reply.to_string();
            let response = format!(
                "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\n\r\n{body}",
                body.len()
            );
            socket.write_all(response.as_bytes()).await.unwrap();
            seen
        });
        (addr, server)
    }

    async fn ask_chat(id: &str, reply: Value) -> (Reply, String) {
        let (addr, server) = serve_once(reply).await;
        let manifest = find(id).unwrap();
        let protocol = manifest.protocol;
        let resolved = Resolved {
            manifest,
            endpoint: Some(chat_url(protocol, &format!("http://{addr}"), "test-model")),
            model: Some("test-model".into()),
            key: Some("sk-chat".into()),
            timeout: std::time::Duration::from_secs(5),
        };
        let reply = ask(&resolved, "hello", &questions()).await.unwrap();
        (reply, server.await.unwrap().to_lowercase())
    }

    #[tokio::test]
    async fn openai_chat_is_asked_with_a_bearer_key() {
        let (reply, seen) = ask_chat(
            "openai-compatible",
            json!({ "choices": [{ "message": { "content": filled().to_string() } }] }),
        )
        .await;
        assert_filled(&reply);
        assert!(seen.starts_with("post /v1/chat/completions "), "{seen}");
        assert!(seen.contains("authorization: bearer sk-chat"));
        assert!(seen.contains("\"response_format\""));
    }

    #[tokio::test]
    async fn openai_responses_is_asked_with_a_bearer_key() {
        let (reply, seen) = ask_chat(
            "openai",
            json!({ "output": [ { "type": "message", "content": [ { "type": "output_text", "text": filled().to_string() } ] } ] }),
        )
        .await;
        assert_filled(&reply);
        assert!(seen.starts_with("post /v1/responses "), "{seen}");
        assert!(seen.contains("authorization: bearer sk-chat"));
    }

    #[tokio::test]
    async fn anthropic_is_asked_with_its_own_headers() {
        let (reply, seen) = ask_chat(
            "anthropic",
            json!({ "content": [ { "type": "tool_use", "name": chat::SCHEMA_NAME, "input": filled() } ] }),
        )
        .await;
        assert_filled(&reply);
        assert!(seen.starts_with("post /v1/messages "), "{seen}");
        assert!(seen.contains("x-api-key: sk-chat"));
        assert!(seen.contains(&format!("anthropic-version: {ANTHROPIC_VERSION}")));
        assert!(!seen.contains("authorization:"), "no bearer");
    }

    #[tokio::test]
    async fn gemini_is_asked_with_the_model_in_the_path() {
        let (reply, seen) = ask_chat(
            "gemini",
            json!({ "candidates": [ { "content": { "parts": [ { "text": filled().to_string() } ] } } ] }),
        )
        .await;
        assert_filled(&reply);
        assert!(
            seen.starts_with("post /v1beta/models/test-model:generatecontent "),
            "{seen}"
        );
        assert!(seen.contains("x-goog-api-key: sk-chat"));
        assert!(!seen.contains("authorization:"), "no bearer");
    }
}
