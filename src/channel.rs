//! Topic channels: partitions of a Circle's conversation.
//!
//! `#general` is implicit and has no entry: its messages live in the legacy
//! [`CHAT_KEY`](crate::control::CHAT_KEY) array, so transcripts written before
//! channels existed need no migration. Every other channel is an entry in the
//! [`CHANNELS_KEY`] map, and its messages live in
//! [`CHANNEL_CHAT_KEY`](crate::control::CHANNEL_CHAT_KEY), tagged with the
//! channel id.
//!
//! Throughout the daemon a channel is an `Option<&str>` id, with `None` meaning
//! `#general`. Ids are UUIDs; names are display labels and need not be unique
//! across peers: two people can create `#ops` while apart.

use crate::control::CHANNELS_KEY;
use crate::state::AppState;
use serde::{Deserialize, Serialize};
use yrs::{Any, Map, Out, ReadTxn, Transact, WriteTxn};

/// Display name of the implicit default channel.
pub const GENERAL: &str = "general";
pub const MAX_NAME_LEN: usize = 32;
pub const MAX_PURPOSE_LEN: usize = 200;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ChannelInfo {
    /// `None` only for `#general` as returned by [`list_with_general`].
    #[serde(default)]
    pub id: Option<String>,
    pub name: String,
    #[serde(default)]
    pub purpose: String,
    #[serde(default)]
    pub created_by: String,
    #[serde(default)]
    pub created_at: i64,
    #[serde(default)]
    pub archived: bool,
}

impl ChannelInfo {
    pub fn general() -> Self {
        Self {
            id: None,
            name: GENERAL.to_string(),
            purpose: String::new(),
            created_by: String::new(),
            created_at: 0,
            archived: false,
        }
    }
}

/// Normalize and check a channel name: lowercase `a-z 0-9 - _`, 1–32 chars,
/// with an optional leading `#` dropped.
pub fn validate_name(raw: &str) -> Result<String, String> {
    let name = raw.trim().trim_start_matches('#').to_ascii_lowercase();
    if name.is_empty() {
        return Err("channel name is empty".into());
    }
    if name.len() > MAX_NAME_LEN {
        return Err(format!(
            "channel name is longer than {MAX_NAME_LEN} characters"
        ));
    }
    if !name
        .chars()
        .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-' || c == '_')
    {
        return Err("channel names use only a-z, 0-9, '-' and '_'".into());
    }
    if name == GENERAL {
        return Err("#general already exists".into());
    }
    Ok(name)
}

/// Every channel entry in the control doc, oldest first. Excludes `#general`.
pub fn list(state: &AppState) -> Vec<ChannelInfo> {
    let Ok(txn) = state.control.try_transact() else {
        return Vec::new();
    };
    let Some(map) = txn.get_map(CHANNELS_KEY) else {
        return Vec::new();
    };
    let mut channels: Vec<ChannelInfo> = map
        .iter(&txn)
        .filter_map(|(id, value)| match value {
            Out::Any(Any::String(raw)) => {
                let mut info = serde_json::from_str::<ChannelInfo>(&raw).ok()?;
                info.id = Some(id.to_string());
                Some(info)
            }
            _ => None,
        })
        .collect();
    channels.sort_by(|a, b| (a.created_at, &a.name, &a.id).cmp(&(b.created_at, &b.name, &b.id)));
    channels
}

/// `#general` followed by every other channel.
pub fn list_with_general(state: &AppState) -> Vec<ChannelInfo> {
    let mut all = vec![ChannelInfo::general()];
    all.extend(list(state));
    all
}

/// The entry for `id`, if the control doc has one.
pub fn get(state: &AppState, id: &str) -> Option<ChannelInfo> {
    list(state)
        .into_iter()
        .find(|c| c.id.as_deref() == Some(id))
}

/// Display name for a channel id; `general` for `None`, the id itself for a
/// channel this peer has not heard of yet.
pub fn name_of(state: &AppState, channel: Option<&str>) -> String {
    match channel {
        None => GENERAL.to_string(),
        Some(id) => get(state, id)
            .map(|c| c.name)
            .unwrap_or_else(|| id.to_string()),
    }
}

/// Resolve a user-supplied channel reference — a name (with or without `#`),
/// an id, or empty / `general` for the default — to a channel id.
///
/// `Ok(None)` is `#general`. A name shared by several channels is an error
/// naming their ids, since guessing would post into the wrong one.
pub fn resolve(state: &AppState, reference: Option<&str>) -> Result<Option<String>, String> {
    let Some(raw) = reference.map(str::trim).filter(|r| !r.is_empty()) else {
        return Ok(None);
    };
    let name = raw.trim_start_matches('#');
    if name.eq_ignore_ascii_case(GENERAL) {
        return Ok(None);
    }
    let channels = list(state);
    if let Some(found) = channels.iter().find(|c| c.id.as_deref() == Some(raw)) {
        return Ok(found.id.clone());
    }
    let lowered = name.to_ascii_lowercase();
    let matches: Vec<&ChannelInfo> = channels.iter().filter(|c| c.name == lowered).collect();
    match matches.as_slice() {
        [] => Err(format!("unknown channel #{name}")),
        [one] => Ok(one.id.clone()),
        many => {
            // Prefer the one live channel when the rest are archived.
            let live: Vec<_> = many.iter().filter(|c| !c.archived).collect();
            if let [one] = live.as_slice() {
                return Ok(one.id.clone());
            }
            let ids: Vec<&str> = many.iter().filter_map(|c| c.id.as_deref()).collect();
            Err(format!(
                "several channels are named #{name}; use an id: {}",
                ids.join(", ")
            ))
        }
    }
}

/// Create a channel. Refuses a name already used by a live channel this peer
/// knows of; peers that are apart can still collide, which [`resolve`] handles.
pub fn create(
    state: &AppState,
    name: &str,
    purpose: &str,
    created_by: &str,
) -> Result<ChannelInfo, String> {
    let name = validate_name(name)?;
    let purpose = purpose.trim();
    if purpose.chars().count() > MAX_PURPOSE_LEN {
        return Err(format!(
            "purpose is longer than {MAX_PURPOSE_LEN} characters"
        ));
    }
    if list(state).iter().any(|c| c.name == name && !c.archived) {
        return Err(format!("#{name} already exists"));
    }
    let id = uuid::Uuid::new_v4().to_string();
    let info = ChannelInfo {
        id: Some(id.clone()),
        name,
        purpose: purpose.to_string(),
        created_by: created_by.to_string(),
        created_at: chrono::Utc::now().timestamp(),
        archived: false,
    };
    write(state, &id, &info)?;
    Ok(info)
}

/// Update a channel's purpose and/or archived flag. Channels are archived,
/// never deleted: their history stays readable.
pub fn update(
    state: &AppState,
    id: &str,
    purpose: Option<&str>,
    archived: Option<bool>,
) -> Result<ChannelInfo, String> {
    let mut info = get(state, id).ok_or_else(|| format!("unknown channel {id}"))?;
    if let Some(purpose) = purpose {
        let purpose = purpose.trim();
        if purpose.chars().count() > MAX_PURPOSE_LEN {
            return Err(format!(
                "purpose is longer than {MAX_PURPOSE_LEN} characters"
            ));
        }
        info.purpose = purpose.to_string();
    }
    if let Some(archived) = archived {
        info.archived = archived;
    }
    write(state, id, &info)?;
    Ok(info)
}

fn write(state: &AppState, id: &str, info: &ChannelInfo) -> Result<(), String> {
    let mut stored = info.clone();
    stored.id = None;
    let json = serde_json::to_string(&stored).map_err(|e| e.to_string())?;
    let mut txn = state
        .control
        .try_transact_mut()
        .map_err(|_| "circle state busy".to_string())?;
    let map = txn.get_or_insert_map(CHANNELS_KEY);
    map.insert(&mut txn, id, Any::String(json.as_str().into()));
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_are_normalized_and_checked() {
        assert_eq!(validate_name("#Ops").unwrap(), "ops");
        assert_eq!(validate_name(" bug-fix_2 ").unwrap(), "bug-fix_2");
        assert!(validate_name("").is_err());
        assert!(validate_name("has space").is_err());
        assert!(validate_name("general").is_err());
        assert!(validate_name(&"a".repeat(33)).is_err());
    }
}
