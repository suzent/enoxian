//! Cross-platform child-process spawning for agent commands.
//!
//! On Windows, `Command::new("npx")` fails with "program not found" because
//! `npx` (and many JS-ecosystem tools) are `.cmd`/`.bat` batch scripts, and the
//! Win32 `CreateProcess` used under the hood neither applies `PATHEXT`
//! resolution nor executes batch files directly — only a real `.exe` or a
//! shell can. A user typing `npx` in a shell works because the shell does that
//! resolution; our daemon does not.
//!
//! This helper routes batch-style programs through `cmd /c` on Windows. It also
//! connects each managed ACP bridge to the user's real product CLI so the
//! bridge reuses that CLI's authentication and configuration.

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
    configure_underlying_cli(program, &mut c);
    c
}

/// Bind coordination commands launched anywhere in a managed agent's process
/// tree to the actor identity issued by the local daemon. Native file writes
/// are attributed separately by the managed change-session record.
pub fn apply_actor_env(
    command: &mut Command,
    agent_id: &str,
    circle_id: &str,
    actor_token: Option<&str>,
) {
    command
        .env("ENOXIAN_AGENT_ID", agent_id)
        .env("ENOXIAN_CIRCLE", circle_id);
    if let Some(token) = actor_token {
        command.env("ENOXIAN_ACTOR_TOKEN", token);
    }
}

/// A managed adapter is a transport bridge, not a replacement runtime. Point it
/// at the user's installed product CLI so it reuses that CLI's login,
/// settings, MCP configuration, and project skills instead of falling back to a
/// bundled copy. See [`super::probe::bridged_cli`] for the adapter table.
fn configure_underlying_cli(program: &str, command: &mut Command) {
    let Some(bridge) = super::probe::bridged_cli(program) else {
        return;
    };
    let Some(cli) = super::probe::resolve(bridge.program) else {
        return;
    };

    // Windows batch shims cannot be passed to CreateProcess by the Agent SDK.
    // In that case leave the variable unset and let the bridge resolve the real
    // executable through the inherited PATH, matching Buzz's compatibility
    // behavior.
    #[cfg(windows)]
    if matches!(
        cli.extension()
            .and_then(|extension| extension.to_str())
            .map(str::to_ascii_lowercase)
            .as_deref(),
        Some("cmd" | "bat")
    ) {
        return;
    }

    command.env(bridge.executable_env, cli);
}

/// Remove environment variables that make a spawned agent think it is nested
/// inside its own session and refuse to start.
///
/// Concretely: if the enoxian daemon is itself launched from inside a Claude
/// Code session, `CLAUDECODE=1` is inherited, and the Claude ACP bridge aborts
/// `session/new` with "Claude Code cannot be launched inside another Claude
/// Code session." Clearing these guard vars lets the ACP agent run regardless
/// of where the daemon was started.
fn scrub_env(c: &mut Command) {
    for var in [
        "CLAUDECODE",
        "CLAUDE_CODE_ENTRYPOINT",
        "CLAUDE_CODE_SSE_PORT",
    ] {
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
    std::path::Path::new(program).extension().is_none()
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

#[cfg(test)]
mod portable_tests {
    use super::{apply_actor_env, command};
    use crate::agent::probe::bridged_cli;

    #[test]
    fn managed_process_inherits_actor_identity_without_prompt_injection() {
        let mut child = command("agent", &[]);
        apply_actor_env(&mut child, "hermes", "circle-1", Some("secret"));
        let env: std::collections::HashMap<_, _> = child
            .as_std()
            .get_envs()
            .filter_map(|(key, value)| value.map(|v| (key.to_owned(), v.to_owned())))
            .collect();
        assert_eq!(env[std::ffi::OsStr::new("ENOXIAN_AGENT_ID")], "hermes");
        assert_eq!(env[std::ffi::OsStr::new("ENOXIAN_CIRCLE")], "circle-1");
        assert_eq!(env[std::ffi::OsStr::new("ENOXIAN_ACTOR_TOKEN")], "secret");
    }

    #[test]
    fn each_managed_bridge_targets_its_own_cli_and_variable() {
        let claude = bridged_cli("claude-agent-acp").expect("claude bridges to a CLI");
        assert_eq!(claude.program, "claude");
        assert_eq!(claude.executable_env, "CLAUDE_CODE_EXECUTABLE");

        let codex = bridged_cli("codex-acp").expect("codex bridges to a CLI");
        assert_eq!(codex.program, "codex");
        assert_eq!(codex.executable_env, "CODEX_PATH");
    }

    #[test]
    fn recognizes_managed_paths_legacy_names_and_windows_shims() {
        for program in [
            "/managed/bin/claude-agent-acp",
            r"C:\managed\claude-code-acp.cmd",
            r"C:\managed\CLAUDE-AGENT-ACP.CMD",
        ] {
            assert_eq!(
                bridged_cli(program).map(|bridge| bridge.program),
                Some("claude"),
                "{program} should bridge to the Claude CLI"
            );
        }
        assert_eq!(
            bridged_cli(r"C:\managed\CODEX-ACP.EXE").map(|bridge| bridge.program),
            Some("codex")
        );
    }

    #[test]
    fn unknown_adapters_bridge_to_nothing() {
        assert!(bridged_cli("some-third-party-acp").is_none());
        assert!(bridged_cli("").is_none());
    }
}
