use anyhow::Result;
use serde_json::Value;

pub async fn run(client: &reqwest::Client, base: &str, json: bool) -> Result<()> {
    let resp = client.get(format!("{base}/who")).send().await?;
    let val: Value = resp.json().await?;
    if json {
        println!("{}", serde_json::to_string_pretty(&val)?);
    } else {
        let agents = val.as_array().map(|a| a.as_slice()).unwrap_or(&[]);
        if agents.is_empty() {
            println!("(no agents present)");
        } else {
            for a in agents {
                let id = a["agent_id"].as_str().unwrap_or("?");
                let status = a["status"].as_str().unwrap_or("?");
                let file = a["current_file"].as_str().unwrap_or("");
                if file.is_empty() {
                    println!("  {id}  [{status}]");
                } else {
                    println!("  {id}  [{status}]  {file}");
                }
            }
        }
    }
    Ok(())
}
