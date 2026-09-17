//! What `enox --version` reports.
//!
//! The bare crate version cannot tell a published release from a binary someone
//! built themselves with `enox update --dev` — both are `--release` builds of
//! the same source tree. A bug report that says only "0.9.0" is therefore
//! ambiguous, so the long version also names the build channel and the commit
//! it was built from. See `build.rs` for where those two come from.
//!
//! The format is `<semver> (<channel>, <commit>)`. The semver stays the second
//! whitespace-separated token of the clap output, which is what
//! [`crate::commands::update`] parses when it compares an installed binary
//! against a release tag.

/// Crate version alone — the machine-comparable half.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

/// `dev` for any build that is not a published release.
pub const CHANNEL: &str = env!("ENOX_BUILD_CHANNEL");

/// Short commit the binary was built from, `unknown` outside a git checkout.
pub const COMMIT: &str = env!("ENOX_GIT_COMMIT");

/// The full string clap prints for `enox --version`.
pub const LONG_VERSION: &str = concat!(
    env!("CARGO_PKG_VERSION"),
    " (",
    env!("ENOX_BUILD_CHANNEL"),
    ", ",
    env!("ENOX_GIT_COMMIT"),
    ")"
);

#[cfg(test)]
mod tests {
    use super::*;

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
    }
}
