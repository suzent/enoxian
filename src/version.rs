//! What `enox --version` reports.
//!
//! The bare crate version cannot tell a published release from a binary someone
//! built themselves with `enox update --dev` — both are `--release` builds of
//! the same source tree. A bug report that says only "0.9.0" is therefore
//! ambiguous, so the long version also names the build channel and the commit
//! it was built from. See `build.rs` for where those two come from.
//!
//! The format is `<semver> (<channel>, <commit>[, <feature>…])`, the features
//! being the optional ones the binary was built with (`iroh-transport`,
//! `iroh-relay-server`). The semver stays the second
//! whitespace-separated token of the clap output, which is what
//! [`crate::commands::update`] parses when it compares an installed binary
//! against a release tag.

/// Crate version alone — the machine-comparable half.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

/// `dev` for any build that is not a published release.
pub const CHANNEL: &str = env!("ENOX_BUILD_CHANNEL");

/// Short commit the binary was built from, `unknown` outside a git checkout.
pub const COMMIT: &str = env!("ENOX_GIT_COMMIT");

/// Optional features compiled in, comma-separated; empty for a default build.
pub const FEATURES: &str = env!("ENOX_BUILD_FEATURES");

/// The full string clap prints for `enox --version`.
pub const LONG_VERSION: &str = concat!(
    env!("CARGO_PKG_VERSION"),
    " (",
    env!("ENOX_BUILD_CHANNEL"),
    ", ",
    env!("ENOX_GIT_COMMIT"),
    env!("ENOX_BUILD_FEATURE_SUFFIX"),
    ")"
);

/// Whether `version` is strictly older than `minimum`, comparing the numeric
/// `major.minor.patch` core. A pre-release counts as its core version.
///
/// `None` when either side does not parse: a server that sends something odd
/// must not tell every client to upgrade.
pub fn older_than(version: &str, minimum: &str) -> Option<bool> {
    Some(core(version)? < core(minimum)?)
}

fn core(version: &str) -> Option<(u64, u64, u64)> {
    let version = version.trim().trim_start_matches('v');
    let version = version.split(['-', '+']).next()?;
    let mut parts = version.split('.').map(|p| p.parse::<u64>().ok());
    let core = (parts.next()??, parts.next()??, parts.next()??);
    parts.next().is_none().then_some(core)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn older_than_compares_the_numeric_core() {
        assert_eq!(older_than("0.10.0", "0.11.0"), Some(true));
        assert_eq!(older_than("0.9.9", "0.10.0"), Some(true));
        assert_eq!(older_than("0.11.0", "0.11.0"), Some(false));
        assert_eq!(older_than("1.0.0", "0.11.0"), Some(false));
        assert_eq!(older_than("v0.10.0", "0.10.1"), Some(true));
        assert_eq!(older_than("0.11.0-rc.1", "0.11.0"), Some(false));
    }

    #[test]
    fn older_than_refuses_what_it_cannot_parse() {
        assert_eq!(older_than("0.10.0", "latest"), None);
        assert_eq!(older_than("0.10", "0.11.0"), None);
        assert_eq!(older_than("0.10.0.1", "0.11.0"), None);
    }

    /// `update::version_of` reads the semver back out of `enox --version`, and
    /// the release smoke test greps the plain version out of the same line.
    #[test]
    fn long_version_starts_with_the_bare_semver_and_names_channel_and_commit() {
        assert!(
            LONG_VERSION.starts_with(VERSION),
            "{LONG_VERSION} must lead with {VERSION}"
        );
        // Exactly how clap renders it: "enox <long version>".
        let rendered = format!("enox {LONG_VERSION}");
        let mut tokens = rendered.split_whitespace();
        assert_eq!(tokens.next(), Some("enox"));
        assert_eq!(tokens.next(), Some(VERSION));

        assert!(matches!(CHANNEL, "dev" | "release"), "channel: {CHANNEL}");
        assert!(LONG_VERSION.contains(CHANNEL));
        assert!(LONG_VERSION.contains(COMMIT));
        for feature in FEATURES.split(',').filter(|f| !f.is_empty()) {
            assert!(
                LONG_VERSION.contains(feature),
                "{LONG_VERSION} omits {feature}"
            );
        }
    }

    /// The relay updater parses `enox --version` with this pattern; a feature
    /// list must not break it.
    #[test]
    fn the_relay_updater_still_parses_the_long_version() {
        let line = format!("enox {LONG_VERSION}");
        let inside = line
            .split_once(" (")
            .and_then(|(_, rest)| rest.strip_suffix(')'))
            .expect("a parenthesised build stamp");
        assert!(!inside.contains('(') && !inside.contains(')'));
    }
}
