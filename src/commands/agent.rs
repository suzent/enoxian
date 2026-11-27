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

fn matches_search(plugin: &crate::agent::plugin::PluginView, query: &str) -> bool {
    let query = query.to_lowercase();
    let name = plugin
        .registry
        .as_ref()
        .map(|r| r.name.as_str())
        .unwrap_or("");
    [
        plugin.id.as_str(),
        plugin.agent.as_str(),
        name,
        plugin.about.as_str(),
    ]
    .iter()
    .any(|field| field.to_lowercase().contains(&query))
}

/// `enox agent plugins` — show deterministic managed adapter availability.
pub async fn plugins(registry: bool, search: Option<String>) -> Result<()> {
    crate::agent::acp_registry::refresh_quietly().await;
    println!(
        "managed adapter root: {}",
        crate::agent::plugin::adapters_dir()?.display()
    );
    let all: Vec<_> = crate::agent::plugin::views()
        .into_iter()
        .filter(|p| {
            search
                .as_deref()
                .map(|q| matches_search(p, q))
                .unwrap_or(true)
        })
        .collect();
    // A registry agent that is not installed is only a listing entry; it is
    // shown in its own section, and only on request.
    let (available, shown): (Vec<_>, Vec<_>) = all.into_iter().partition(|p| {
        p.origin == "registry"
            && p.state == crate::agent::plugin::PluginState::Missing
            && !p.configured
    });
    for plugin in shown {
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
        // A native plugin never touches Node, so reporting on it would imply a
        // prerequisite that does not exist.
        let node = if !plugin.requires_node {
            String::new()
        } else if plugin.node_runtime_installed {
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
        let uv = if plugin.requires_uv && !plugin.uv_installed {
            "  UV-MISSING"
        } else {
            ""
        };
        // Native plugins pin no version, so there is no "v" to print.
        let version = if plugin.version.is_empty() {
            format!("[{}]", plugin.kind)
        } else {
            format!("v{}", plugin.version)
        };
        let origin = if plugin.origin == "registry" {
            "  (acp registry)"
        } else {
            ""
        };
        let update = plugin
            .update_version
            .as_deref()
            .map(|v| format!("  update-available(v{v})"))
            .unwrap_or_default();
        println!(
            "  {}  @{}  {}  [{:?}]{}{}{}{}{}{}{}",
            plugin.id,
            plugin.agent,
            version,
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
            uv,
            origin,
            update,
        );
    }
    if registry {
        println!(
            "\nACP Registry ({}):",
            crate::agent::acp_registry::REGISTRY_URL
        );
        if available.is_empty() {
            println!("  (nothing to add — the registry may not be reachable yet)");
        }
        for plugin in &available {
            let name = plugin
                .registry
                .as_ref()
                .map(|r| r.name.as_str())
                .unwrap_or("");
            let license = plugin
                .registry
                .as_ref()
                .map(|r| r.license.as_str())
                .unwrap_or("");
            let unverified = if plugin.verified == Some(false) {
                "  unverified"
            } else {
                ""
            };
            let mut about: String = plugin.about.chars().take(70).collect();
            if plugin.about.chars().count() > 70 {
                about.push('…');
            }
            println!(
                "  {}  @{}  v{}  [{}]  {name} ({license}){unverified}\n      {about}",
                plugin.id, plugin.agent, plugin.version, plugin.kind
            );
        }
        println!("\ninstall one with `enox agent install <id>` (add `--as <handle>` to rename it)");
    } else if !available.is_empty() {
        println!(
            "\n{} more agents are available from the ACP Registry: `enox agent plugins --registry`",
            available.len()
        );
    }
    Ok(())
}

/// `enox agent registry refresh`.
pub async fn registry_refresh() -> Result<()> {
    use crate::agent::acp_registry::{self, Refresh};
    let outcome = acp_registry::refresh(true).await?;
    let installable = acp_registry::cached()
        .iter()
        .filter(|a| acp_registry::to_manifest(a).is_some())
        .count();
    let total = acp_registry::cached().len();
    let what = match outcome {
        Refresh::Updated => "updated",
        Refresh::NotModified | Refresh::Fresh => "unchanged",
    };
    println!("✓ ACP Registry {what}: {total} agents, {installable} installable on this device");
    Ok(())
}

/// Describe a registry plugin before installing it: this is third-party code
/// the user is about to run, so say what and from where.
fn describe_registry_install(plugin: &crate::agent::plugin::PluginView, upgrade: bool) {
    let Some(info) = &plugin.registry else {
        return;
    };
    let version = if upgrade {
        plugin.update_version.as_deref().unwrap_or(&plugin.version)
    } else {
        &plugin.version
    };
    println!(
        "→ {} v{version} from the ACP Registry (third-party)",
        info.name
    );
    match plugin.kind.as_str() {
        "binary" => println!(
            "  prebuilt release, {}",
            if plugin.verified == Some(true) {
                "checked against its published SHA-256"
            } else {
                "NO published checksum"
            }
        ),
        "uvx" => println!("  PyPI package {} via uv", plugin.package),
        _ => println!("  npm package {}", plugin.package),
    }
    if !info.license.is_empty() {
        println!("  license: {}", info.license);
    }
    if let Some(repo) = info.repository.as_deref().or(info.website.as_deref()) {
        println!("  source:  {repo}");
    }
}

/// `enox agent install <plugin>` — the explicit networked install phase.
///
/// For a native plugin nothing is networked at all: it only checks the CLI is
/// present and writes the handle, so the output must not claim an install.
pub async fn install(plugin: String, options: crate::agent::plugin::InstallOptions) -> Result<()> {
    // A registry id typed before the index was ever fetched still resolves.
    crate::agent::acp_registry::refresh_quietly().await;
    let view = crate::agent::plugin::views()
        .into_iter()
        .find(|p| Some(p.id.clone()) == crate::agent::plugin::find(&plugin).map(|e| e.manifest.id));
    let native = view.as_ref().map(|v| v.kind == "native").unwrap_or(false);
    if let Some(view) = &view {
        describe_registry_install(view, options.upgrade);
    }
    if native {
        println!("→ checking that the '{plugin}' CLI is installed");
    } else {
        println!("→ checking prerequisites for '{plugin}'");
    }
    let command = crate::agent::plugin::install_with(&plugin, &options).await?;
    let verb = if native {
        "configured (nothing downloaded)"
    } else {
        "installed and configured"
    };
    println!("✓ {verb}: {}", command.command.join(" "));
    if matches!(
        plugin.as_str(),
        "claude" | "claude-agent-acp" | "claude-code-acp"
    ) {
        println!("  using the authenticated Claude Code CLI through CLAUDE_CODE_EXECUTABLE");
    }
    if let Some(handle) = crate::agent::plugin::find(&plugin).map(|e| e.manifest.agent) {
        println!("  mention @{handle} in chat; no package download occurs at runtime.");
    }
    Ok(())
}

/// `enox agent uninstall <plugin>`.
pub fn uninstall(plugin: String) -> Result<()> {
    let removed = crate::agent::plugin::uninstall(&plugin)?;
    match (&removed.handle, removed.files) {
        (None, false) => println!("nothing to remove for '{plugin}'"),
        _ => {
            if let Some(handle) = &removed.handle {
                println!("✓ removed @{handle}");
            }
            if removed.files {
                println!("✓ deleted the files downloaded for '{plugin}'");
            }
        }
    }
    Ok(())
}

/// Tell a running daemon that `agents.toml` changed, so the Circle sees the
/// agents this device now has without a restart. A daemon that is not running
/// reads the file when it starts, so that is not an error.
pub async fn readvertise(client: &reqwest::Client, daemon_root: &str) {
    let url = format!("{daemon_root}/api/agent-config/readvertise");
    match client.post(&url).send().await {
        Ok(response) if response.status().is_success() => {}
        Ok(response) => println!(
            "  could not update the Circle ({}); `enox service restart` will",
            response.status()
        ),
        Err(_) => println!("  enoxian is not running — the Circle sees this when it starts."),
    }
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
            // Editing the command keeps the variables its installer set.
            env: cfg
                .resolve(&name)
                .map(|c| c.env.clone())
                .unwrap_or_default(),
            ..Default::default()
        }
        .inheriting_delegation(cfg.resolve(&name)),
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
    let resume = crate::agent::memory::load(&circle_dir, &agent, None);

    println!(
        "→ running agent '{agent}' ({:?}) in {}",
        cmd.driver,
        workspace.display()
    );
    if resume.is_some() {
        println!("  resuming previous session");
    }
    let outcome = driver::launch(driver::LaunchRequest {
        run_id: None,
        trigger_id: None,
        coordination: None,
        // A one-off CLI run builds no room context, so it withholds none.
        withheld: &[],
        channel: None,
        shape: None,
        agent_name: &agent,
        cmd: &cmd,
        task: &task,
        workspace: &workspace,
        base_snapshot: &base_snapshot,
        circle_id: &circle_id,
        circle_dir: &circle_dir,
        actor_token: Some(actor_token),
        relay_path: Vec::new(),
        initiator: Initiator::Local,
        resume: resume.as_ref().map(|r| r.session_id.as_str()),
    })
    .await
    .context("agent run failed")?;

    println!("✓ agent finished ({})", outcome.detail);
    if let Some(usage) = &outcome.usage {
        println!("  {}", describe_usage(usage));
    }
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

/// One line on what a run used, as the agent reported it.
fn describe_usage(usage: &crate::agent::usage::TurnUsage) -> String {
    let mut parts = Vec::new();
    match &usage.cost {
        Some(c) => parts.push(format!("cost {:.4} {}", c.amount, c.currency)),
        None => parts.push("cost not reported".into()),
    }
    if let Some(t) = usage.tokens {
        parts.push(format!("{} tokens", t.total));
    }
    if usage.context_size > 0 {
        parts.push(format!(
            "context {} / {}",
            usage.context_used, usage.context_size
        ));
    }
    format!("used: {} (as reported by the agent)", parts.join(" · "))
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
