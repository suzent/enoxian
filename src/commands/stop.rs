use anyhow::Result;

pub async fn run(client: &reqwest::Client, root: &str) -> Result<()> {
    let resp = client
        .post(format!("{root}/shutdown"))
        .send()
        .await;

    match resp {
        Ok(r) if r.status().is_success() => println!("✓ enoxd stopped"),
        Ok(r) => anyhow::bail!("unexpected response: {}", r.status()),
        Err(_) => anyhow::bail!("enoxd is not running (or not reachable at {root})"),
    }
    Ok(())
}
