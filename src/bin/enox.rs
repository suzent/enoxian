use clap::Parser;
use enoxian::cli::{AgentCli, AgentCommands};

fn daemon_root() -> String {
    let base = std::env::var("ENOXIAN_API")
        .or_else(|_| std::env::var("enoxian_API"))
        .unwrap_or_else(|_| "http://127.0.0.1:36521".to_string());
    base.trim_end_matches("/api").to_string()
}

/// Resolve the target circle and return the per-circle API base URL.
/// e.g. http://127.0.0.1:36521/circles/<uuid>/api
fn resolve_api_base(circle_hint: Option<&str>) -> anyhow::Result<String> {
    let configs = enoxian::config::load_all()?;
    let cfg = match circle_hint {
        Some(hint) => enoxian::resolve::resolve(hint, &configs)?,
        None => enoxian::resolve::resolve_default(&configs)?,
    };
    Ok(format!("{}/circles/{}/api", daemon_root(), cfg.circle_id))
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::from_default_env()
                .add_directive("enoxian=info".parse()?),
        )
        .init();

    let cli = AgentCli::parse();
    let client = reqwest::Client::new();
    let root = daemon_root();

    match cli.command {
        AgentCommands::Init(args) => enoxian::commands::init::run(args).await,
        AgentCommands::Enter(args) => enoxian::commands::enter::run(args, &client).await,
        AgentCommands::Invite(args) => {
            let configs = enoxian::config::load_all()?;
            let cfg = enoxian::resolve::resolve(&args.circle, &configs).map_err(|_| {
                anyhow::anyhow!(
                    "circle '{}' not found — run `enox circles` to list known circles",
                    args.circle
                )
            })?;
            let api_base = format!("{}/circles/{}/api", root, cfg.circle_id);
            enoxian::commands::invite::run(args, &client, &api_base).await
        }
        AgentCommands::Circles => enoxian::commands::circles::run(&client, &root, cli.json).await,
        AgentCommands::Open => enoxian::commands::open::run(&root).map_err(Into::into),
        AgentCommands::Start { port } => enoxian::commands::start::run(port).await,
        AgentCommands::Stop => enoxian::commands::stop::run(&client, &root).await,
        AgentCommands::Update { dev, src, no_pull } => {
            enoxian::commands::update::run(dev, src, no_pull).await
        }
        AgentCommands::Identity(args) => {
            enoxian::commands::identity::run(args).map_err(Into::into)
        }

        // All other commands need a resolved circle
        cmd => {
            // Lifecycle commands hit daemon_root directly (not per-circle /api)
            match &cmd {
                AgentCommands::Disable => {
                    return enoxian::commands::disable::run(&client, &root, cli.circle.as_deref())
                        .await;
                }
                AgentCommands::Enable => {
                    return enoxian::commands::enable::run(&client, &root, cli.circle.as_deref())
                        .await;
                }
                AgentCommands::Leave { yes } => {
                    return enoxian::commands::leave::run(
                        &client,
                        &root,
                        cli.circle.as_deref(),
                        *yes,
                    )
                    .await;
                }
                _ => {}
            }

            let base = resolve_api_base(cli.circle.as_deref())?;
            match cmd {
                AgentCommands::Status => {
                    enoxian::commands::status::run(&client, &base, cli.json).await
                }
                AgentCommands::Who => enoxian::commands::who::run(&client, &base, cli.json).await,
                AgentCommands::Tasks { status } => {
                    enoxian::commands::tasks::run(&client, &base, status, cli.json).await
                }
                AgentCommands::TaskCreate { title, description } => {
                    enoxian::commands::tasks::create(&client, &base, title, description, cli.json)
                        .await
                }
                AgentCommands::Claim { task_id } => {
                    enoxian::commands::claim::run(&client, &base, task_id, cli.json).await
                }
                AgentCommands::Done { task_id } => {
                    enoxian::commands::done_cmd::run(&client, &base, task_id, cli.json).await
                }
                AgentCommands::Bind { path } => {
                    enoxian::commands::bind::run(&client, &base, path, cli.json).await
                }
                AgentCommands::Release { path } => {
                    enoxian::commands::release::run(&client, &base, path, cli.json).await
                }
                AgentCommands::Watch => enoxian::commands::watch::run(&client, &base).await,
                AgentCommands::Member(args) => {
                    enoxian::commands::member::run(
                        &client,
                        &root,
                        cli.circle.as_deref(),
                        args.action,
                        cli.json,
                    )
                    .await
                }
                AgentCommands::Chat { follow, since } => {
                    enoxian::commands::chat::run(&client, &base, follow, since).await
                }
                AgentCommands::Say { text } => {
                    enoxian::commands::say::run(&client, &base, text).await
                }
                // Already handled above
                AgentCommands::Init(_)
                | AgentCommands::Enter(_)
                | AgentCommands::Invite(_)
                | AgentCommands::Circles
                | AgentCommands::Open
                | AgentCommands::Disable
                | AgentCommands::Enable
                | AgentCommands::Leave { .. }
                | AgentCommands::Start { .. }
                | AgentCommands::Stop
                | AgentCommands::Update { .. }
                | AgentCommands::Identity(_) => unreachable!(),
            }
        }
    }
}
