use anyhow::Result;
use serde_json::Value;

pub async fn run(client: &reqwest::Client, base: &str, json: bool) -> Result<()> {
    let resp = client.get(format!("{base}/status")).send().await?;
    let val: Value = resp.json().await?;
    if json {
        println!("{}", serde_json::to_string_pretty(&val)?);
    } else {
        println!("◆ Circle:  {}", val["circle_name"].as_str().unwrap_or("?"));
        println!("  ID:      {}", val["circle_id"].as_str().unwrap_or("?"));
        println!("  Workspace: {}", val["workspace"].as_str().unwrap_or("?"));
        println!("  Docs:    {}", val["docs"]);

        if let Some(conflicts) = val["conflicts"].as_array() {
            if conflicts.is_empty() {
                println!("  Conflicts: none");
            } else {
                println!("  Conflicts: {} unresolved", conflicts.len());
                for c in conflicts {
                    if let Some(s) = c.as_str() {
                        println!("    ✗ {s}");
                    }
                }
            }
        }

        if let Some(locks) = val["locks"].as_array() {
            if locks.is_empty() {
                println!("  Locks:   none");
            } else {
                println!("  Locks:   {} held", locks.len());
                for l in locks {
                    let path = l["path"].as_str().unwrap_or("?");
                    let agent = l["agent_id"].as_str().unwrap_or("?");
                    let until = crate::commands::bind::until(&l["expires_at"]);
                    match l["run_id"].as_str() {
                        Some(run) => println!("    ⊘ {path}  ← {agent} (run {run}) until {until}"),
                        None => println!("    ⊘ {path}  ← {agent} until {until}"),
                    }
                }
            }
        }
    }
    Ok(())
}
