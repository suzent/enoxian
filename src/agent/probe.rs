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
];

/// The program (`command[0]`) a candidate is detected by.
impl Candidate {
    pub fn program(&self) -> &str {
        // A catalog entry always has at least the program itself.
        self.command.first().copied().unwrap_or("")
    }
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
