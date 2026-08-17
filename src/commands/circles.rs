use anyhow::Result;

pub async fn run(client: &reqwest::Client, daemon_base: &str, json: bool) -> Result<()> {
    let url = format!("{daemon_base}/circles");
    match client.get(&url).send().await {
        Ok(resp) => {
            let circles: serde_json::Value = resp.json().await?;
            if json {
                println!("{}", serde_json::to_string_pretty(&circles)?);
            } else if let Some(arr) = circles.as_array() {
                if arr.is_empty() {
                    println!("No active circles.");
                } else {
                    for c in arr {
                        println!(
                            "  {} — {}",
                            c["circle_name"].as_str().unwrap_or("?"),
                            c["circle_id"].as_str().unwrap_or("?")
                        );
                    }
                }
            }
        }
        Err(_) => {
            // Daemon not running — fall back to local config
            let configs = crate::config::load_all()?;
            if json {
                let v: Vec<_> = configs
                    .iter()
                    .map(|c| {
                        serde_json::json!({
                            "circle_id": c.circle_id,
                            "circle_name": c.circle_name,
                            "disabled": c.disabled,
                        })
                    })
                    .collect();
                println!("{}", serde_json::to_string_pretty(&v)?);
            } else if configs.is_empty() {
                println!("No circles found — run `enox init` to create one.");
            } else {
                println!("Known circles (Enoxian is not running):");
                for c in &configs {
                    let tag = if c.disabled { " [paused]" } else { "" };
                    println!("  {}{} — {}", c.circle_name, tag, c.circle_id);
                }
            }
        }
    }
    Ok(())
}
