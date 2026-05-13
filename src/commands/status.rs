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
    }
    Ok(())
}
