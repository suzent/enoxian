use clap::Parser;
use enochian::cli::{AgentCli, AgentCommands};

fn daemon_root() -> String {
    let base = std::env::var("ENOCHIAN_API")
        .unwrap_or_else(|_| "http://127.0.0.1:9090".to_string());
    base.trim_end_matches("/api").to_string()
}

/// Resolve the target circle and return the per-circle API base URL.
/// e.g. http://127.0.0.1:9090/circles/<uuid>/api
fn resolve_api_base(circle_hint: Option<&str>) -> anyhow::Result<String> {
    let configs = enochian::config::load_all()?;
    let cfg = match circle_hint {
        Some(hint) => enochian::resolve::resolve(hint, &configs)?,
        None => enochian::resolve::resolve_default(&configs)?,
    };
    Ok(format!("{}/circles/{}/api", daemon_root(), cfg.circle_id))
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
    let client = reqwest::Client::new();
    let root = daemon_root();

    match cli.command {
        AgentCommands::Init(args)   => enochian::commands::init::run(args).await,
        AgentCommands::Enter(args)  => enochian::commands::enter::run(args).await,
        AgentCommands::Invite(args) => enochian::commands::invite::run(args).await,
        AgentCommands::Circles      => enochian::commands::circles::run(&client, &root, cli.json).await,
        AgentCommands::Update { dev, src, no_pull } => enochian::commands::update::run(dev, src, no_pull).await,

        // All other commands need a resolved circle
        cmd => {
            // Lifecycle commands hit daemon_root directly (not per-circle /api)
            match &cmd {
                AgentCommands::Disable => {
                    return enochian::commands::disable::run(&client, &root, cli.circle.as_deref()).await;
                }
                AgentCommands::Enable => {
                    return enochian::commands::enable::run(&client, &root, cli.circle.as_deref()).await;
                }
                AgentCommands::Leave { yes } => {
                    return enochian::commands::leave::run(&client, &root, cli.circle.as_deref(), *yes).await;
                }
                _ => {}
            }

            let base = resolve_api_base(cli.circle.as_deref())?;
            match cmd {
                AgentCommands::Status =>
                    enochian::commands::status::run(&client, &base, cli.json).await,
                AgentCommands::Who =>
                    enochian::commands::who::run(&client, &base, cli.json).await,
                AgentCommands::Tasks { status } =>
                    enochian::commands::tasks::run(&client, &base, status, cli.json).await,
                AgentCommands::TaskCreate { title, description } =>
                    enochian::commands::tasks::create(&client, &base, title, description, cli.json).await,
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
                AgentCommands::Member(args) =>
                    enochian::commands::member::run(&client, &root, cli.circle.as_deref(), args.action, cli.json).await,
                AgentCommands::Chat { follow, since } =>
                    enochian::commands::chat::run(&client, &base, follow, since).await,
                AgentCommands::Say { text } =>
                    enochian::commands::say::run(&client, &base, text).await,
                // Already handled above
                AgentCommands::Init(_)
                | AgentCommands::Enter(_)
                | AgentCommands::Invite(_)
                | AgentCommands::Circles
                | AgentCommands::Disable
                | AgentCommands::Enable
                | AgentCommands::Leave { .. }
                | AgentCommands::Update { .. } => unreachable!(),
            }
        }
    }
}
