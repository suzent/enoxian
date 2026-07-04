//! `enox session start --actor X` / `enox session finish` — claimed session mode.
//!
//! A claimed session declares that workspace changes until `finish` belong to
//! `actor`. It never grants authority — the filesystem mutation still creates
//! the proposal — it only raises attribution to user-declared confidence. One
//! open claimed session per workspace (concurrent actors are an open question;
//! see agent-workspaces.md → Claimed Session Mode).

use crate::proposal::session::{LocalChangeSession, SessionMode};
use crate::proposal::store::ProposalStore;
use anyhow::{bail, Result};

use super::agent::resolve_circle_workspace;

pub async fn start(circle: Option<&str>, actor: String) -> Result<()> {
    let (circle_id, workspace) = resolve_circle_workspace(circle)?;
    let circle_dir = crate::config::circle_dir(&circle_id)?;

    if let Some(existing) = LocalChangeSession::load_claimed(&circle_dir) {
        if existing.is_open() {
            bail!(
                "a claimed session is already open (actor '{}', started {}). \
                 Run `enox session finish` first.",
                existing.actor_id.as_deref().unwrap_or("?"),
                existing.started_at
            );
        }
    }

    // Anchor on the engine's current baseline so the actor's changes diff
    // cleanly from where the workspace stood when the session opened.
    let store = ProposalStore::open(&workspace)?;
    let base_snapshot = store.baseline_id().unwrap_or_default();

    let mut session =
        LocalChangeSession::start(circle_id, base_snapshot, SessionMode::ClaimedSession);
    session.actor_id = Some(actor.clone());
    session.save_claimed(&circle_dir)?;

    println!("✓ claimed session open — changes attributed to '{actor}'");
    println!("  session {}", session.session_id);
    println!("  run your tool, then `enox session finish`.");
    Ok(())
}

pub async fn finish(circle: Option<&str>) -> Result<()> {
    let (circle_id, _workspace) = resolve_circle_workspace(circle)?;
    let circle_dir = crate::config::circle_dir(&circle_id)?;

    match LocalChangeSession::load_claimed(&circle_dir) {
        Some(mut session) if session.is_open() => {
            session.finish();
            let actor = session.actor_id.clone().unwrap_or_default();
            LocalChangeSession::clear_claimed(&circle_dir)?;
            println!("✓ claimed session for '{actor}' closed");
            println!("  any changes made during it will surface as a proposal — `enox proposal list`.");
            Ok(())
        }
        _ => bail!("no open claimed session on this workspace"),
    }
}
