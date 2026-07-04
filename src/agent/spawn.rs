//! Cross-platform child-process spawning for agent commands.
//!
//! On Windows, `Command::new("npx")` fails with "program not found" because
//! `npx` (and many JS-ecosystem tools) are `.cmd`/`.bat` batch scripts, and the
//! Win32 `CreateProcess` used under the hood neither applies `PATHEXT`
//! resolution nor executes batch files directly — only a real `.exe` or a
//! shell can. A user typing `npx` in a shell works because the shell does that
//! resolution; our daemon does not.
//!
//! This helper routes batch-style programs through `cmd /c` on Windows so
//! `["npx", "@zed-industries/claude-code-acp"]` runs the same way it would from
//! a terminal. On non-Windows it is a plain passthrough.

use tokio::process::Command;

/// Build a [`Command`] for `program` + `args`, transparently handling Windows
/// batch scripts (`.cmd`/`.bat`, and bare names like `npx` that resolve to
/// one), and scrubbing session-nesting guard vars (see [`scrub_env`]).
pub fn command(program: &str, args: &[String]) -> Command {
    let mut c;
    #[cfg(windows)]
    {
        if needs_cmd_wrapper(program) {
            // cmd /c <program> <args...> — cmd applies PATHEXT and can run .cmd.
            c = Command::new("cmd");
            c.arg("/c").arg(program).args(args);
        } else {
            c = Command::new(program);
            c.args(args);
        }
    }
    #[cfg(not(windows))]
    {
        c = Command::new(program);
        c.args(args);
    }
    scrub_env(&mut c);
    c
}

/// Remove environment variables that make a spawned agent think it is nested
/// inside its own session and refuse to start.
///
/// Concretely: if the enoxian daemon is itself launched from inside a Claude
/// Code session, `CLAUDECODE=1` is inherited, and `claude-code-acp` aborts
/// `session/new` with "Claude Code cannot be launched inside another Claude
/// Code session." Clearing these guard vars lets the ACP agent run regardless
/// of where the daemon was started.
fn scrub_env(c: &mut Command) {
    for var in ["CLAUDECODE", "CLAUDE_CODE_ENTRYPOINT", "CLAUDE_CODE_SSE_PORT"] {
        c.env_remove(var);
    }
}

/// Whether a Windows program name should be run through `cmd /c`. True for
/// explicit `.cmd`/`.bat`, and for extensionless names (e.g. `npx`, `pnpm`)
/// which commonly resolve to a batch script. A path ending in `.exe` runs
/// directly.
#[cfg(windows)]
fn needs_cmd_wrapper(program: &str) -> bool {
    let lower = program.to_ascii_lowercase();
    if lower.ends_with(".exe") {
        return false;
    }
    if lower.ends_with(".cmd") || lower.ends_with(".bat") {
        return true;
    }
    // Extensionless: let cmd resolve it (covers npx/npm/pnpm/yarn wrappers).
    // A program given as an absolute path to a real binary would normally carry
    // .exe; extensionless absolute paths are unusual and safe to route via cmd.
    !std::path::Path::new(program)
        .extension()
        .is_some()
}

/// Kill a process and all its descendants by PID. `npx`/`node` launchers spawn
/// the real agent as a child, so killing only the launcher orphans the tree;
/// this reaps the whole subtree so a finished or aborted run leaves nothing
/// behind.
pub fn kill_tree(pid: u32) {
    #[cfg(windows)]
    {
        // taskkill /T kills the tree, /F forces it. Detached, output ignored.
        let _ = std::process::Command::new("taskkill")
            .args(["/PID", &pid.to_string(), "/T", "/F"])
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status();
    }
    #[cfg(unix)]
    {
        // Negative pid targets the process group; the child is a group leader
        // only if spawned that way, so also try the pid directly as a fallback.
        let _ = std::process::Command::new("kill")
            .args(["-TERM", &format!("-{pid}")])
            .status();
        let _ = std::process::Command::new("kill")
            .args(["-TERM", &pid.to_string()])
            .status();
    }
}

#[cfg(all(test, windows))]
mod tests {
    use super::*;

    #[test]
    fn wraps_batch_and_bare_names() {
        assert!(needs_cmd_wrapper("npx"));
        assert!(needs_cmd_wrapper("npm.cmd"));
        assert!(needs_cmd_wrapper("thing.bat"));
        assert!(!needs_cmd_wrapper("codex.exe"));
        assert!(!needs_cmd_wrapper("C:\\tools\\agent.exe"));
    }
}
