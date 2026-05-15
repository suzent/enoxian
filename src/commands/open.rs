use anyhow::Result;

pub fn run(daemon_root: &str) -> Result<()> {
    let url = format!("{}/app", daemon_root.trim_end_matches('/'));
    println!("Opening {url}");
    open::that(&url).map_err(|e| anyhow::anyhow!("failed to open browser: {e}"))?;
    Ok(())
}
