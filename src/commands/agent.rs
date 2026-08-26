//! `enox agent run <agent> <task>` — launch a configured agent in a circle's
//! workspace under a managed-process change session.
//!
//! This is the local, user-initiated counterpart to a push-policy chat
//! reaction: enoxian owns the process, so whatever the agent writes becomes an
//! attributed proposal via the ambient engine.

use crate::agent::config::{AgentCommand, AgentConfig, Driver, Reaction};
use crate::agent::driver::{self, Initiator};
use crate::proposal::store::ProposalStore;
use anyhow::{bail, Context, Result};
use std::path::PathBuf;

/// `enox agent list` — show configured agents and the reaction policy.
pub fn list() -> Result<()> {
    let cfg = AgentConfig::load();
    println!("reaction: {:?}", cfg.reaction);
    println!("config:   {}", AgentConfig::path()?.display());
    if cfg.agents.is_empty() {
        println!("\n(no agents configured — mentions match nothing)");
        return Ok(());
    }
    println!("\nagents:");
    for (name, cmd) in &cfg.agents {
        let wd = cmd
            .working_dir
            .as_deref()
            .map(|d| format!("  (in {d})"))
            .unwrap_or_default();
        println!(
            "  @{name}  [{:?}]  {}{wd}",
            cmd.driver,
            cmd.command.join(" ")
        );
    }
    Ok(())
}

/// `enox agent plugins` — show deterministic managed adapter availability.
pub fn plugins() -> Result<()> {
    println!(
        "managed adapter root: {}",
        crate::agent::plugin::adapters_dir()?.display()
    );
    for plugin in crate::agent::plugin::views() {
        let runtime = match plugin.runtime_installed {
            Some(true) => format!(
                "  cli={}",
                plugin.runtime_program.as_deref().unwrap_or("ready")
            ),
            Some(false) => format!(
                "  CLI-MISSING({})",
                plugin.runtime_program.as_deref().unwrap_or("runtime")
            ),
            None => String::new(),
        };
        let node = if plugin.node_runtime_installed {
            format!(
                "  node={}",
                plugin.node_runtime_version.as_deref().unwrap_or("ready")
            )
        } else {
            format!(
                "  NODE-MISSING{}",
                plugin
                    .node_runtime_version
                    .as_deref()
                    .map(|version| format!("({version}; need 22+ with npm)"))
                    .unwrap_or_else(|| "(need 22+ with npm)".to_string())
            )
        };
        println!(
            "  {}  @{}  v{}  [{:?}]{}{}{}{}",
            plugin.id,
            plugin.agent,
            plugin.version,
            plugin.state,
            if plugin.configured {
                "  configured"
            } else {
                ""
            },
            if plugin.legacy_configured {
                "  legacy-config"
            } else {
                ""
            },
            runtime,
            node,
        );
    }
    Ok(())
}

/// `enox agent install <plugin>` — the explicit networked install phase.
pub async fn install(plugin: String) -> Result<()> {
    println!("→ checking prerequisites for adapter '{plugin}'");
    let command = crate::agent::plugin::install(&plugin).await?;
    println!("✓ installed and configured: {}", command.command.join(" "));
    if matches!(
        plugin.as_str(),
        "claude" | "claude-agent-acp" | "claude-code-acp"
    ) {
        println!("  using the authenticated Claude Code CLI through CLAUDE_CODE_EXECUTABLE");
    }
    println!("  mention the configured agent in chat; no package download occurs at runtime.");
    Ok(())
}

/// `enox agent add <name> --driver <d> -- <command...>`.
pub fn add(
    name: String,
    driver: String,
    working_dir: Option<String>,
    command: Vec<String>,
) -> Result<()> {
    let driver = match driver.as_str() {
        "acp" => Driver::Acp,
        "argv" => Driver::Argv,
        other => bail!("unknown driver '{other}' (expected 'acp' or 'argv')"),
    };
    let mut cfg = AgentConfig::load_for_edit()?;
    let existed = cfg.resolve(&name).is_some();
    cfg.set_agent(
        &name,
        AgentCommand {
            command,
            driver,
            working_dir,
        },
    );
    cfg.save()?;
    println!(
        "{} agent '@{name}'",
        if existed { "updated" } else { "added" }
    );
    println!(
        "mention @{name} in chat (needs reaction = push) or run `enox agent run {name} \"...\"`"
    );
    Ok(())
}

/// `enox agent remove <name>`.
pub fn remove(name: String) -> Result<()> {
    let mut cfg = AgentConfig::load_for_edit()?;
    if !cfg.remove_agent(&name) {
        bail!("no agent named '{name}' is configured");
    }
    cfg.save()?;
    println!("removed agent '@{name}'");
    Ok(())
}

/// `enox agent reaction push|pull`.
pub fn reaction(mode: String) -> Result<()> {
    let reaction = match mode.as_str() {
        "push" => Reaction::Push,
        "pull" => Reaction::Pull,
        other => bail!("unknown reaction '{other}' (expected 'push' or 'pull')"),
    };
    let mut cfg = AgentConfig::load_for_edit()?;
    cfg.reaction = reaction;
    cfg.save()?;
    match reaction {
        Reaction::Push => println!(
            "reaction set to PUSH — a circle member's @mention can now run a configured agent on this device."
        ),
        Reaction::Pull => println!("reaction set to PULL — mentions run nothing here automatically."),
    }
    Ok(())
}

pub async fn run(
    circle: Option<&str>,
    agent: String,
    task: String,
    actor_token: &str,
) -> Result<()> {
    let (circle_id, workspace) = resolve_circle_workspace(circle)?;

    let cfg = AgentConfig::load();
    let Some(cmd) = cfg.resolve(&agent).cloned() else {
        bail!(
            "agent '{agent}' is not configured in {}.\n\
             Add it under [agents.{agent}] with a `command` (and optional `driver = \"acp\"`).",
            AgentConfig::path()?.display()
        );
    };

    let store = ProposalStore::open(&workspace)?;
    let base_snapshot = store.baseline_id().unwrap_or_default();

    // Resume the agent's remembered conversation for this circle, if any.
    let circle_dir = crate::config::circle_dir(&circle_id)?;
    let resume = crate::agent::memory::load(&circle_dir, &agent);

    println!(
        "→ running agent '{agent}' ({:?}) in {}",
        cmd.driver,
        workspace.display()
    );
    if resume.is_some() {
        println!("  resuming previous session");
    }
    let outcome = driver::launch(driver::LaunchRequest {
        agent_name: &agent,
        cmd: &cmd,
        task: &task,
        workspace: &workspace,
        base_snapshot: &base_snapshot,
        circle_id: &circle_id,
        circle_dir: &circle_dir,
        actor_token: Some(actor_token),
        initiator: Initiator::Local,
        resume: resume.as_deref(),
    })
    .await
    .context("agent run failed")?;

    if let Some(sid) = &outcome.acp_session_id {
        let _ = crate::agent::memory::save(&circle_dir, &agent, sid);
    }

    println!("✓ agent finished ({})", outcome.detail);
    if let Some(reply) = &outcome.reply {
        println!("\n{reply}\n");
    }
    println!(
        "  session {} — any file changes will surface as a proposal.",
        outcome.session_id
    );
    println!("  run `enox proposal list` to review.");
    Ok(())
}

/// Resolve a circle selector (name or id, or the sole circle if omitted) to its
/// id and workspace directory, using local config — no daemon required.
pub fn resolve_circle_workspace(selector: Option<&str>) -> Result<(String, PathBuf)> {
    let configs = crate::config::load_all()?;
    if configs.is_empty() {
        bail!("no circles found — run `enox init` or `enox enter` first");
    }
    let cfg = match selector {
        Some(sel) => configs
            .iter()
            .find(|c| c.circle_id == sel || c.circle_name == sel)
            .with_context(|| format!("no circle matching '{sel}'"))?,
        None => {
            if configs.len() > 1 {
                bail!(
                    "multiple circles exist — pass --circle <name|id> to choose one:\n{}",
                    configs
                        .iter()
                        .map(|c| format!("  {} ({})", c.circle_name, c.circle_id))
                        .collect::<Vec<_>>()
                        .join("\n")
                );
            }
            &configs[0]
        }
    };
    Ok((cfg.circle_id.clone(), PathBuf::from(&cfg.workspace_dir)))
}
