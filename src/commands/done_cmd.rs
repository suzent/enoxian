use anyhow::Result;
use serde_json::{json, Value};

pub async fn run(client: &reqwest::Client, base: &str, task_id: String, json_out: bool) -> Result<()> {
    let resp = client
        .post(format!("{base}/done"))
        .json(&json!({ "task_id": task_id }))
        .send().await?;
    let val: Value = resp.json().await?;
    if json_out {
        println!("{}", serde_json::to_string_pretty(&val)?);
    } else {
        println!("✦ done: {task_id}");
    }
    Ok(())
}
