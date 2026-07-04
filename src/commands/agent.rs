//! `enox agent run <agent> <task>` — launch a configured agent in a circle's
//! workspace under a managed-process change session.
//!
//! This is the local, user-initiated counterpart to a push-policy chat
//! reaction: enoxian owns the process, so whatever the agent writes becomes an
//! attributed proposal via the ambient engine.

use crate::agent::config::AgentConfig;
use crate::agent::driver::{self, Initiator};
use crate::proposal::store::ProposalStore;
use anyhow::{bail, Context, Result};
use std::path::PathBuf;

pub async fn run(circle: Option<&str>, agent: String, task: String) -> Result<()> {
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

    println!("→ running agent '{agent}' ({:?}) in {}", cmd.driver, workspace.display());
    let outcome = driver::launch(
        &agent,
        &cmd,
        &task,
        &workspace,
        &base_snapshot,
        &circle_id,
        Initiator::Local,
    )
    .await
    .context("agent run failed")?;

    println!("✓ agent finished ({})", outcome.detail);
    println!("  session {} — any file changes will surface as a proposal.", outcome.session_id);
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
