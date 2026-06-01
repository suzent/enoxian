use anyhow::Result;
use serde_json::Value;

pub async fn run(
    client: &reqwest::Client,
    base: &str,
    status_filter: Option<String>,
    json: bool,
) -> Result<()> {
    let mut url = format!("{base}/tasks");
    if let Some(s) = &status_filter {
        url = format!("{url}?status={s}");
    }
    let resp = client.get(&url).send().await?;
    let val: Value = resp.json().await?;
    if json {
        println!("{}", serde_json::to_string_pretty(&val)?);
    } else {
        let tasks = val.as_array().map(|a| a.as_slice()).unwrap_or(&[]);
        if tasks.is_empty() {
            println!("(no tasks)");
        } else {
            for t in tasks {
                let id = &t["task_id"].as_str().unwrap_or("?")[..8]; // short ID
                let title = t["title"].as_str().unwrap_or("?");
                let status = t["status"].as_str().unwrap_or("?");
                let by = t["claimed_by"].as_str().unwrap_or("");
                if by.is_empty() {
                    println!("  [{status}] {id}  {title}");
                } else {
                    println!("  [{status}] {id}  {title}  (→ {by})");
                }
            }
        }
    }
    Ok(())
}

pub async fn create(
    client: &reqwest::Client,
    base: &str,
    title: String,
    description: Option<String>,
    json: bool,
) -> Result<()> {
    let mut body = serde_json::json!({ "title": title });
    if let Some(desc) = description {
        body["description"] = serde_json::Value::String(desc);
    }
    let resp = client.post(format!("{base}/tasks")).json(&body).send().await?;
    let val: Value = resp.json().await?;
    if json {
        println!("{}", serde_json::to_string_pretty(&val)?);
    } else {
        let id = val["task_id"].as_str().unwrap_or("?");
        let short = if id.len() >= 8 { &id[..8] } else { id };
        println!("✦ Task created: {short}  {title}");
    }
    Ok(())
}
