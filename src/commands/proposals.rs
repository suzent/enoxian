//! `enox proposal ...` — terminal access to the review queue that the daemon's
//! ambient engine produces. Thin client over the REST proposal API
//! (`src/api/proposals.rs`); all logic (diffing, reverse-apply, replication)
//! lives daemon-side.

use anyhow::Result;
use serde_json::Value;

/// Short, table-friendly id (first 8 chars).
fn short(id: &str) -> &str {
    if id.len() >= 8 { &id[..8] } else { id }
}

/// `list` prints 8-char id prefixes, but the daemon looks proposals up by exact
/// id. Accept a prefix on the CLI by resolving it against the proposal list to
/// the one full id it uniquely matches. A full uuid (36 chars) is used as-is.
async fn resolve_id(client: &reqwest::Client, base: &str, id: &str) -> Result<String> {
    if id.len() >= 36 {
        return Ok(id.to_string());
    }
    let resp = client.get(format!("{base}/proposals")).send().await?;
    let val: Value = resp.json().await?;
    let proposals = val.as_array().map(|a| a.as_slice()).unwrap_or(&[]);
    let matches: Vec<&str> = proposals
        .iter()
        .filter_map(|p| p["id"].as_str())
        .filter(|full| full.starts_with(id))
        .collect();
    match matches.as_slice() {
        [] => anyhow::bail!("no proposal matches id '{id}' — run `enox proposal list`"),
        [one] => Ok(one.to_string()),
        many => anyhow::bail!(
            "id '{id}' is ambiguous ({} matches) — use more characters",
            many.len()
        ),
    }
}

pub async fn list(client: &reqwest::Client, base: &str, json: bool) -> Result<()> {
    let resp = client.get(format!("{base}/proposals")).send().await?;
    let val: Value = resp.json().await?;
    if json {
        println!("{}", serde_json::to_string_pretty(&val)?);
        return Ok(());
    }
    let proposals = val.as_array().map(|a| a.as_slice()).unwrap_or(&[]);
    if proposals.is_empty() {
        println!("(no proposals)");
        return Ok(());
    }
    for p in proposals {
        let id = short(p["id"].as_str().unwrap_or("?"));
        let status = p["status"].as_str().unwrap_or("?");
        let device = p["origin_device"].as_str().unwrap_or("");
        let paths = p["changed_paths"].as_array().map(|a| a.len()).unwrap_or(0);
        let first = p["changed_paths"]
            .as_array()
            .and_then(|a| a.first())
            .and_then(|v| v.as_str())
            .unwrap_or("");
        // "hi.txt" for a single path, "hi.txt (+2)" for several.
        let files = match paths {
            0 => "(no files)".to_string(),
            1 => first.to_string(),
            n => format!("{first} (+{})", n - 1),
        };
        let by = if device.is_empty() { String::new() } else { format!("  @ {device}") };
        println!("  [{status:<8}] {id}  {files}{by}");
    }
    Ok(())
}

pub async fn show(client: &reqwest::Client, base: &str, id: String, json: bool) -> Result<()> {
    let id = resolve_id(client, base, &id).await?;
    let resp = client.get(format!("{base}/proposals/{id}")).send().await?;
    if !resp.status().is_success() {
        anyhow::bail!("proposal {id} not found");
    }
    let val: Value = resp.json().await?;
    if json {
        println!("{}", serde_json::to_string_pretty(&val)?);
        return Ok(());
    }

    let status = val["status"].as_str().unwrap_or("?");
    let source = val["source"].as_str().unwrap_or("?");
    let device = val["origin_device"].as_str().unwrap_or("");
    println!("Proposal {}", val["id"].as_str().unwrap_or(&id));
    println!("  status: {status}   source: {source}   device: {device}");
    println!();

    let files = val["files"].as_array().map(|a| a.as_slice()).unwrap_or(&[]);
    for f in files {
        let path = f["path"].as_str().unwrap_or("?");
        let change = f["change"].as_str().unwrap_or("?");
        println!("── {path}  ({change})");
        if f["binary"].as_bool().unwrap_or(false) {
            println!("   (binary file — diff suppressed)");
            continue;
        }
        let before = f["before"].as_str().unwrap_or("");
        let after = f["after"].as_str().unwrap_or("");
        print_line_diff(before, after);
        println!();
    }
    Ok(())
}

/// Unified line diff via `diffy` (already used daemon-side for three-way merge).
/// Prints the hunk body with `+`/`-`/space prefixes, skipping diffy's file
/// header lines since we already printed the path above.
fn print_line_diff(before: &str, after: &str) {
    let patch = diffy::create_patch(before, after);
    for line in patch.to_string().lines() {
        // Drop the "--- original" / "+++ modified" header diffy emits.
        if line.starts_with("---") || line.starts_with("+++") {
            continue;
        }
        println!("   {line}");
    }
}

/// accept / reject / revert all POST to a sub-path and report the new status.
pub async fn decide(
    client: &reqwest::Client,
    base: &str,
    id: String,
    action: &str,
    json: bool,
) -> Result<()> {
    let id = resolve_id(client, base, &id).await?;
    let resp = client
        .post(format!("{base}/proposals/{id}/{action}"))
        .send()
        .await?;
    let ok = resp.status().is_success();
    let val: Value = resp.json().await?;
    if json {
        println!("{}", serde_json::to_string_pretty(&val)?);
        return Ok(());
    }
    if ok {
        let status = val["status"].as_str().unwrap_or(action);
        println!("✦ proposal {} → {status}", short(&id));
    } else {
        let err = val["error"].as_str().unwrap_or("request failed");
        anyhow::bail!("{err}");
    }
    Ok(())
}
