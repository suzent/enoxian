//! Circle resolution: map a name/prefix/UUID-prefix to a CircleConfig.
//!
//! Resolution order:
//!   1. Exact name match (case-sensitive)
//!   2. Case-insensitive name prefix match
//!   3. UUID prefix match
//!   4. Error — not found or ambiguous

use anyhow::{bail, Result};
use crate::config::CircleConfig;

/// Resolve `target` against a slice of known circle configs.
pub fn resolve<'a>(target: &str, configs: &'a [CircleConfig]) -> Result<&'a CircleConfig> {
    // 1. Exact name match
    if let Some(c) = configs.iter().find(|c| c.circle_name == target) {
        return Ok(c);
    }

    // 2. Case-insensitive name prefix
    let lower = target.to_lowercase();
    let name_hits: Vec<_> = configs
        .iter()
        .filter(|c| c.circle_name.to_lowercase().starts_with(&lower))
        .collect();

    match name_hits.len() {
        1 => return Ok(name_hits[0]),
        n if n > 1 => {
            let names: Vec<_> = name_hits.iter().map(|c| c.circle_name.as_str()).collect();
            bail!("'{}' is ambiguous — matches: {}", target, names.join(", "));
        }
        _ => {}
    }

    // 3. UUID prefix
    let uuid_hits: Vec<_> = configs
        .iter()
        .filter(|c| c.circle_id.starts_with(target))
        .collect();

    match uuid_hits.len() {
        1 => return Ok(uuid_hits[0]),
        n if n > 1 => {
            let ids: Vec<_> = uuid_hits.iter().map(|c| c.circle_id.as_str()).collect();
            bail!("'{}' is ambiguous — matches: {}", target, ids.join(", "));
        }
        _ => {}
    }

    bail!("no circle found matching '{}' — run `enoch circles` to list known circles", target)
}

/// Pick the one active circle, or error if there are zero or many.
pub fn resolve_default(configs: &[CircleConfig]) -> Result<&CircleConfig> {
    match configs.len() {
        0 => bail!("no circles found — run `enoch init` to create one"),
        1 => Ok(&configs[0]),
        _ => {
            let names: Vec<_> = configs.iter().map(|c| c.circle_name.as_str()).collect();
            bail!(
                "multiple circles found ({}): specify one with --circle or ENOCHIAN_CIRCLE",
                names.join(", ")
            )
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cfg(id: &str, name: &str) -> CircleConfig {
        CircleConfig {
            circle_id: id.to_string(),
            circle_name: name.to_string(),
            psk_hex: String::new(),
            keypair_proto_hex: String::new(),
        }
    }

    #[test]
    fn exact_name() {
        let cfgs = vec![cfg("aaa-111", "Work"), cfg("bbb-222", "Personal")];
        assert_eq!(resolve("Work", &cfgs).unwrap().circle_id, "aaa-111");
    }

    #[test]
    fn name_prefix() {
        let cfgs = vec![cfg("aaa-111", "Work"), cfg("bbb-222", "Personal")];
        assert_eq!(resolve("per", &cfgs).unwrap().circle_id, "bbb-222");
    }

    #[test]
    fn uuid_prefix() {
        let cfgs = vec![cfg("aaa-111", "Work"), cfg("bbb-222", "Personal")];
        assert_eq!(resolve("bbb", &cfgs).unwrap().circle_id, "bbb-222");
    }

    #[test]
    fn ambiguous_name() {
        let cfgs = vec![cfg("aaa-111", "WorkA"), cfg("bbb-222", "WorkB")];
        assert!(resolve("Work", &cfgs).is_err());
    }

    #[test]
    fn not_found() {
        let cfgs = vec![cfg("aaa-111", "Work")];
        assert!(resolve("xyz", &cfgs).is_err());
    }
}
