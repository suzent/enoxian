//! Discovery of locally-installed agent binaries.
//!
//! `agents.toml` is entirely hand/UI-configured: nothing here decides *which*
//! agents a device will run. But the frontend can't ask a user to "add
//! `claude-agent-acp`" if it has no idea what's installed. This module answers a
//! narrower, read-only question — *is this program resolvable on this machine?*
//! — and offers a small catalog of well-known agents so the UI can suggest
//! one-click adds and badge configured entries as installed / missing.
//!
//! Detection is a `PATH` lookup only (plus `PATHEXT` on Windows). It never runs
//! the program: presence on `PATH` is a cheap, honest proxy for "installed",
//! and actually launching a candidate to version-check it would be both slow and
//! a surprising side effect of opening a settings panel.

use std::path::PathBuf;

/// A well-known agent the UI can suggest adding. `command[0]` is the program
/// whose presence on `PATH` decides whether it's [`installed`].
pub struct Candidate {
    /// Suggested agent name (the `@handle` used in chat mentions).
    pub name: &'static str,
    /// "acp" or "argv" — matches [`crate::agent::config::Driver`].
    pub driver: &'static str,
    /// Full launch command. For argv agents this includes the `{{task}}`
    /// placeholder so the added entry works without further editing.
    pub command: &'static [&'static str],
    /// One-line description shown in the picker.
    pub about: &'static str,
}

/// The built-in catalog of agents worth suggesting. Adding an entry here makes
/// it appear in `/api/agent-config/discover` whenever its program is on `PATH`.
pub const CATALOG: &[Candidate] = &[
    Candidate {
        name: "claude",
        driver: "acp",
        command: &["claude-agent-acp"],
        about:
            "Claude Code CLI through a local ACP bridge (subscription and native config preserved).",
    },
    Candidate {
        name: "codex",
        driver: "argv",
        command: &["codex", "{{task}}"],
        about: "OpenAI Codex CLI, fire-and-forget with the task text as its prompt.",
    },
    // Suzent speaks ACP itself, so `command[0]` is the product CLI rather than
    // an adapter: there is no bridge to be ready, and presence of `suzent` on
    // PATH is the whole prerequisite.
    Candidate {
        name: "suzent",
        driver: "acp",
        command: &["suzent", "acp"],
        about: "Your local Suzent, speaking ACP directly — no adapter bridge, no Node.js.",
    },
];

/// The program (`command[0]`) a candidate is detected by.
impl Candidate {
    pub fn program(&self) -> &str {
        // A catalog entry always has at least the program itself.
        self.command.first().copied().unwrap_or("")
    }
}

/// An adapter that bridges to a product CLI the user installs and
/// authenticates themselves, instead of shipping its own copy of that product.
///
/// Enoxian manages only the pinned bridge; the CLI underneath stays the
/// user's — their login, settings, MCP servers, and project configuration.
/// That has two consequences this table exists to keep consistent: a bridge is
/// not usable when its CLI is absent, and a bridge must be handed the exact
/// executable we resolved rather than left to find its own (a bundled copy, or
/// a different `PATH` entry than the one the settings page reported on).
pub struct BridgedCli {
    /// Program looked up on `PATH` to decide whether the bridge is usable.
    pub program: &'static str,
    /// Where to get it, shown when it is missing.
    pub install_url: &'static str,
    /// Command that authenticates it, shown when it is missing.
    pub login_command: &'static str,
    /// Variable the bridge reads to run one explicit executable.
    pub executable_env: &'static str,
    /// Subcommand that proves the CLI is signed in, for CLIs that have one and
    /// exit non-zero without it. `None` means presence is all we can check.
    pub auth_status_args: Option<&'static [&'static str]>,
}

/// The bridged CLI a managed adapter needs, keyed by the adapter's executable
/// name so plugin health and process spawning cannot disagree. Unknown
/// adapters (including third-party manifests) bridge to nothing and are judged
/// on their own executable alone.
pub fn bridged_cli(adapter: &str) -> Option<&'static BridgedCli> {
    const CLAUDE: BridgedCli = BridgedCli {
        program: "claude",
        install_url: "https://code.claude.com/docs/en/getting-started",
        login_command: "claude auth login",
        executable_env: "CLAUDE_CODE_EXECUTABLE",
        auth_status_args: Some(&["auth", "status"]),
    };
    const CODEX: BridgedCli = BridgedCli {
        program: "codex",
        install_url: "https://developers.openai.com/codex/cli",
        login_command: "codex login",
        executable_env: "CODEX_PATH",
        // The Codex CLI has no status subcommand we can rely on across the
        // versions users have installed, so presence is the honest check.
        auth_status_args: None,
    };

    match adapter_stem(adapter).as_str() {
        "claude-agent-acp" | "claude-code-acp" => Some(&CLAUDE),
        "codex-acp" => Some(&CODEX),
        _ => None,
    }
}

/// Whether an adapter can actually run right now: a bridge needs the CLI it
/// bridges to, so it is only usable once that CLI is installed.
///
/// An adapter that bridges to nothing (a third-party manifest, or a plain argv
/// agent) has no such prerequisite and is judged on its own executable alone,
/// so it stays usable here.
pub fn bridge_ready(adapter: &str) -> bool {
    match bridged_cli(adapter) {
        Some(bridge) => is_installed(bridge.program),
        None => true,
    }
}

/// The comparable name of an adapter executable: basename, lowercased, without
/// a Windows executable extension, so a managed absolute path and a bare name
/// resolve to the same adapter.
fn adapter_stem(program: &str) -> String {
    let normalized = program.replace('\\', "/");
    let file = normalized.rsplit('/').next().unwrap_or(&normalized);
    let lower = file.to_ascii_lowercase();
    lower
        .strip_suffix(".cmd")
        .or_else(|| lower.strip_suffix(".exe"))
        .or_else(|| lower.strip_suffix(".bat"))
        .unwrap_or(&lower)
        .to_string()
}

/// Whether `program` is resolvable as an executable on this machine.
///
/// - Absolute/relative paths are checked directly (with `PATHEXT` expansion on
///   Windows for extensionless paths).
/// - Bare names are searched across every `PATH` entry, applying `PATHEXT` so
///   `npx` matches `npx.cmd`, `codex` matches `codex.exe`, etc.
///
/// This mirrors what a shell does before spawning, so it agrees with whether
/// [`crate::agent::spawn::command`] would actually find the program.
pub fn is_installed(program: &str) -> bool {
    resolve(program).is_some()
}

/// Resolve `program` to the concrete executable that would be launched.
///
/// Unlike [`is_installed`], this preserves the path so managed adapters can
/// point SDK-based bridges at the user's real CLI executable (for example via
/// `CLAUDE_CODE_EXECUTABLE`).
pub fn resolve(program: &str) -> Option<PathBuf> {
    if program.is_empty() {
        return None;
    }

    let candidate = std::path::Path::new(program);
    // An explicit path (contains a separator) is resolved as-is, not searched.
    if candidate.components().count() > 1 || candidate.is_absolute() {
        return resolve_as_file(candidate.to_path_buf());
    }

    let path = std::env::var_os("PATH")?;
    std::env::split_paths(&path).find_map(|dir| resolve_as_file(dir.join(program)))
}

/// True if `base` resolves to a runnable executable, trying each `PATHEXT`
/// extension on Windows when `base` has none (so `dir/npx` matches
/// `dir/npx.cmd`). On Unix, a match must additionally be executable — a shell
/// won't run a non-`+x` file, so neither should we claim it's "installed".
fn resolve_as_file(base: PathBuf) -> Option<PathBuf> {
    if is_executable_file(&base) {
        return Some(base);
    }
    #[cfg(windows)]
    {
        // Only extensionless names get PATHEXT expansion; an explicit ".exe"
        // was already tried above.
        if base.extension().is_none() {
            let exts =
                std::env::var("PATHEXT").unwrap_or_else(|_| ".COM;.EXE;.BAT;.CMD".to_string());
            for ext in exts.split(';').filter(|e| !e.is_empty()) {
                // PATHEXT entries include the leading dot.
                let mut with_ext = base.clone().into_os_string();
                with_ext.push(ext);
                let path = PathBuf::from(with_ext);
                if path.is_file() {
                    return Some(path);
                }
            }
        }
    }
    None
}

/// A regular file that this OS would actually execute. Windows decides
/// runnability by extension (handled by the `PATHEXT` loop in the caller), so
/// existence-as-a-file is enough here. Unix requires the execute bit set for
/// at least one class, matching what a shell checks before spawning.
fn is_executable_file(path: &std::path::Path) -> bool {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        match std::fs::metadata(path) {
            // `metadata` follows symlinks, so a symlinked binary resolves to its
            // target's type and mode — the common Homebrew/apt layout works.
            Ok(m) => m.is_file() && (m.permissions().mode() & 0o111 != 0),
            Err(_) => false,
        }
    }
    #[cfg(not(unix))]
    {
        path.is_file()
    }
}

/// Fold the login shell's `PATH` into this process's environment, once, at
/// daemon startup.
///
/// Managed services (macOS `launchd`, Linux `systemd --user`) start a bare
/// `PATH` and never source shell rc files, so anything a version manager
/// (nvm, pyenv-style tools, Homebrew on some setups, etc.) only adds to
/// `PATH` from `.zshrc`/`.bash_profile` is invisible here even though it
/// works in every terminal. Without this, [`resolve`]/[`is_installed`] and
/// anything spawned via [`crate::agent::spawn::command`] silently disagree
/// with what the user sees in a shell.
///
/// Best-effort and bounded: if the login shell can't be probed within a few
/// seconds (or this is Windows, where the interactive logon token already
/// carries the full user environment), the process `PATH` is left as-is.
#[cfg(unix)]
pub fn adopt_login_shell_path() {
    let Some(login_path) = login_shell_path() else {
        return;
    };
    let current = std::env::var("PATH").unwrap_or_default();
    let merged = merge_path_lists(&login_path, &current);
    if !merged.is_empty() {
        std::env::set_var("PATH", merged);
    }
}

#[cfg(not(unix))]
pub fn adopt_login_shell_path() {}

/// Union of two `:`-separated `PATH` lists, de-duplicated, `a`'s entries
/// first — the login shell's resolution should win over the sparse default
/// a managed service started with.
#[cfg(unix)]
fn merge_path_lists(a: &str, b: &str) -> String {
    let mut seen = std::collections::HashSet::new();
    a.split(':')
        .chain(b.split(':'))
        .filter(|dir| !dir.is_empty() && seen.insert(*dir))
        .collect::<Vec<_>>()
        .join(":")
}

/// The `PATH` a fresh login shell would see, by actually asking it — the
/// only reliable way to account for whatever a user's rc files do (nvm,
/// asdf, custom exports, ...). Run on a background thread with a bounded
/// wait: an rc file that hangs (or a `$SHELL` that isn't really a shell)
/// must not stall daemon startup.
#[cfg(unix)]
fn login_shell_path() -> Option<String> {
    use std::sync::mpsc;
    use std::time::Duration;

    let shell = std::env::var("SHELL").unwrap_or_else(|_| "/bin/sh".to_string());
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        let result = std::process::Command::new(&shell)
            .args(["-ilc", "printf %s \"$PATH\""])
            .stdin(std::process::Stdio::null())
            .output();
        let _ = tx.send(result);
    });

    let output = rx.recv_timeout(Duration::from_secs(3)).ok()?.ok()?;
    if !output.status.success() {
        return None;
    }
    let path = String::from_utf8_lossy(&output.stdout).trim().to_string();
    (!path.is_empty()).then_some(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_program_is_not_installed() {
        assert!(!is_installed(""));
    }

    #[test]
    fn missing_program_is_not_installed() {
        assert!(!is_installed("definitely-not-a-real-agent-xyz"));
    }

    #[test]
    fn catalog_entries_carry_a_program() {
        for c in CATALOG {
            assert!(!c.program().is_empty(), "{} has no program", c.name);
        }
    }

    // Suzent is detected by its own CLI, not by an adapter executable, so the
    // program probed must be `suzent` itself and it must bridge to nothing.
    #[test]
    fn suzent_is_discovered_by_its_own_cli() {
        let suzent = CATALOG
            .iter()
            .find(|c| c.name == "suzent")
            .expect("suzent is a catalog candidate");
        assert_eq!(suzent.program(), "suzent");
        assert_eq!(suzent.driver, "acp");
        assert!(bridged_cli(suzent.program()).is_none());
        assert!(bridge_ready(suzent.program()));
    }

    // A bridge is exactly as usable as the CLI underneath it. Asserting the
    // equivalence rather than a fixed verdict keeps this true on a machine that
    // has the CLI and one that does not.
    #[test]
    fn bridge_readiness_tracks_the_bridged_cli() {
        for adapter in ["claude-agent-acp", "claude-code-acp", "codex-acp"] {
            let bridge = bridged_cli(adapter).expect("built-in adapter bridges to a CLI");
            assert_eq!(
                bridge_ready(adapter),
                is_installed(bridge.program),
                "{adapter} readiness should follow {}",
                bridge.program
            );
        }
    }

    // An adapter that bridges to nothing has no external prerequisite, so it
    // must not be filtered out as unusable.
    #[test]
    fn adapter_without_a_bridge_has_no_prerequisite() {
        assert!(bridge_ready("some-third-party-acp"));
        assert!(bridge_ready(""));
    }

    // The advertisement path passes the configured command's first element,
    // which is a full managed path, not a bare adapter name.
    #[test]
    fn readiness_accepts_a_full_adapter_path() {
        let path = "/Users/x/.enoxian/adapters/codex-acp/1.1.14/node_modules/.bin/codex-acp";
        assert_eq!(bridge_ready(path), is_installed("codex"));
    }

    #[cfg(unix)]
    #[test]
    fn merge_path_lists_prefers_login_shell_entries_and_dedupes() {
        let merged = merge_path_lists(
            "/Users/a/.nvm/versions/node/v22.0.0/bin:/usr/bin:/bin",
            "/usr/bin:/bin:/usr/sbin:/sbin",
        );
        assert_eq!(
            merged,
            "/Users/a/.nvm/versions/node/v22.0.0/bin:/usr/bin:/bin:/usr/sbin:/sbin"
        );
    }

    #[cfg(unix)]
    #[test]
    fn merge_path_lists_ignores_empty_segments() {
        assert_eq!(merge_path_lists("/a::/b", "::/c:"), "/a:/b:/c");
    }

    #[test]
    fn finds_a_program_that_exists_on_path() {
        // Every platform we target has *some* always-present executable on PATH.
        #[cfg(windows)]
        assert!(is_installed("cmd"));
        #[cfg(not(windows))]
        assert!(is_installed("sh"));
    }

    #[test]
    fn resolves_an_explicit_path_directly() {
        // A path with a separator is checked as-is, not searched on PATH.
        #[cfg(windows)]
        {
            let sys = std::env::var("SystemRoot").unwrap_or_else(|_| "C:\\Windows".into());
            assert!(is_installed(&format!("{sys}\\System32\\cmd.exe")));
        }
        #[cfg(not(windows))]
        {
            // /bin/sh exists on every unix we target; a bare "sh" would be
            // PATH-searched, so this specifically exercises the explicit branch.
            assert!(is_installed("/bin/sh"));
        }
        assert!(!is_installed("./definitely-not-here-xyz"));
    }

    #[cfg(unix)]
    #[test]
    fn unix_requires_the_execute_bit() {
        use std::io::Write;
        use std::os::unix::fs::PermissionsExt;

        let dir = std::env::temp_dir().join(format!("enox-probe-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("faux-agent");
        std::fs::File::create(&file)
            .unwrap()
            .write_all(b"#!/bin/sh\n")
            .unwrap();

        // A non-executable regular file is present but must NOT count as installed.
        std::fs::set_permissions(&file, std::fs::Permissions::from_mode(0o644)).unwrap();
        assert!(
            resolve_as_file(file.clone()).is_none(),
            "non-+x file counted as installed"
        );

        // Flip the execute bit and it should now resolve.
        std::fs::set_permissions(&file, std::fs::Permissions::from_mode(0o755)).unwrap();
        assert!(
            resolve_as_file(file.clone()).is_some(),
            "+x file not detected"
        );

        std::fs::remove_dir_all(&dir).ok();
    }
}
