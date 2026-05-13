use clap::Parser;
use enochian::cli::{AgentCli, AgentCommands};

fn api_base() -> String {
    std::env::var("ENOCHIAN_API")
        .unwrap_or_else(|_| "http://127.0.0.1:9090/api".to_string())
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::from_default_env()
                .add_directive("enochian=info".parse()?),
        )
        .init();

    let cli = AgentCli::parse();
    let base = api_base();
    let client = reqwest::Client::new();

    match cli.command {
        AgentCommands::Init(args)  => enochian::commands::init::run(args).await,
        AgentCommands::Enter(args) => enochian::commands::enter::run(args).await,
        AgentCommands::Status      => enochian::commands::status::run(&client, &base, cli.json).await,
        AgentCommands::Who         => enochian::commands::who::run(&client, &base, cli.json).await,
        AgentCommands::Tasks { status } =>
            enochian::commands::tasks::run(&client, &base, status, cli.json).await,
        AgentCommands::Claim { task_id } =>
            enochian::commands::claim::run(&client, &base, task_id, cli.json).await,
        AgentCommands::Done { task_id } =>
            enochian::commands::done_cmd::run(&client, &base, task_id, cli.json).await,
        AgentCommands::Bind { path } =>
            enochian::commands::bind::run(&client, &base, path, cli.json).await,
        AgentCommands::Release { path } =>
            enochian::commands::release::run(&client, &base, path, cli.json).await,
        AgentCommands::Watch =>
            enochian::commands::watch::run(&client, &base).await,
    }
}
