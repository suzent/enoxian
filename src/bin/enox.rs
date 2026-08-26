use clap::Parser;
use enoxian::cli::{AgentCli, AgentCommands, BootstrapAction, DaemonAction};

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
    let actor_token = cli.token.clone();
    // Present the local API token on every request (the daemon requires it).
    // Read from ~/.enoxian/api.token, written by the daemon on first start.
    let client = {
        let mut builder = reqwest::Client::builder();
        if let Some(token) = enoxian::api::auth::load() {
            let mut headers = reqwest::header::HeaderMap::new();
            if let Ok(val) = reqwest::header::HeaderValue::from_str(&format!("Bearer {token}")) {
                headers.insert(reqwest::header::AUTHORIZATION, val);
            }
            builder = builder.default_headers(headers);
        }
        builder.build().unwrap_or_else(|_| reqwest::Client::new())
    };
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
        AgentCommands::Open => enoxian::commands::open::run(&root),
        AgentCommands::Start { port } => enoxian::commands::start::run(port).await,
        AgentCommands::Stop => {
            if enoxian::commands::service::is_installed() {
                let _ = enoxian::commands::stop::run(&client, &root).await;
                enoxian::commands::service::stop_managed()
            } else {
                enoxian::commands::stop::run(&client, &root).await
            }
        }
        AgentCommands::Daemon(args) => match args.action {
            DaemonAction::Run(args) => match enoxian::commands::serve::run(args).await {
                Ok(()) => {
                    // Graceful cleanup has completed. Exit directly instead of
                    // waiting indefinitely for third-party blocking runtime
                    // work (for example DNS/filesystem workers) to unwind.
                    std::process::exit(0)
                }
                Err(error) => Err(error),
            },
        },
        AgentCommands::Bootstrap(args) => match args.action {
            BootstrapAction::Serve(args) => {
                let relay_port = args.relay_port.unwrap_or(args.port.saturating_add(1));
                enoxian::bootstrap::run(args.port, relay_port, args.advertise_host.as_deref()).await
            }
        },
        AgentCommands::Service(args) => {
            enoxian::commands::service::run(args.action, &client, &root).await
        }
        AgentCommands::Update {
            dev,
            src,
            no_pull,
            status,
            record_stable,
        } => enoxian::commands::update::run(dev, src, no_pull, status, record_stable).await,
        AgentCommands::UpdateApply(args) => enoxian::commands::update::apply(args),
        AgentCommands::Identity(args) => enoxian::commands::identity::run(args),

        // Local workspace commands — operate on the circle's files/config
        // directly, no daemon API round-trip. Handled before the API-base
        // resolution below since they need neither the daemon nor a base URL.
        AgentCommands::Agent(args) => {
            use enoxian::cli::AgentAction;
            match args.action {
                AgentAction::Run { agent, task } => {
                    enoxian::commands::agent::run(cli.circle.as_deref(), agent, task).await
                }
                AgentAction::List => enoxian::commands::agent::list(),
                AgentAction::Plugins => enoxian::commands::agent::plugins(),
                AgentAction::Install { plugin } => enoxian::commands::agent::install(plugin).await,
                AgentAction::Add {
                    name,
                    driver,
                    working_dir,
                    command,
                } => enoxian::commands::agent::add(name, driver, working_dir, command),
                AgentAction::Remove { name } => enoxian::commands::agent::remove(name),
                AgentAction::Reaction { mode } => enoxian::commands::agent::reaction(mode),
            }
        }
        AgentCommands::Session(args) => {
            use enoxian::cli::SessionAction;
            match args.action {
                SessionAction::Start { actor } => {
                    enoxian::commands::session::start(cli.circle.as_deref(), actor).await
                }
                SessionAction::Finish => {
                    enoxian::commands::session::finish(cli.circle.as_deref()).await
                }
            }
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
                AgentCommands::Register { agent_id } => {
                    enoxian::commands::register::run(&client, &base, agent_id, cli.json).await
                }
                AgentCommands::Tasks { status } => {
                    enoxian::commands::tasks::run(&client, &base, status, cli.json).await
                }
                AgentCommands::TaskCreate { title, description } => {
                    enoxian::commands::tasks::create(
                        &client,
                        &base,
                        title,
                        description,
                        actor_token.as_deref(),
                        cli.json,
                    )
                    .await
                }
                AgentCommands::Claim { task_id } => {
                    enoxian::commands::claim::run(
                        &client,
                        &base,
                        task_id,
                        actor_token.as_deref(),
                        cli.json,
                    )
                    .await
                }
                AgentCommands::Done { task_id } => {
                    enoxian::commands::done_cmd::run(
                        &client,
                        &base,
                        task_id,
                        actor_token.as_deref(),
                        cli.json,
                    )
                    .await
                }
                AgentCommands::Bind { path } => {
                    enoxian::commands::bind::run(
                        &client,
                        &base,
                        path,
                        actor_token.as_deref(),
                        cli.json,
                    )
                    .await
                }
                AgentCommands::Release { path } => {
                    enoxian::commands::release::run(
                        &client,
                        &base,
                        path,
                        actor_token.as_deref(),
                        cli.json,
                    )
                    .await
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
                    enoxian::commands::say::run(&client, &base, text, actor_token.as_deref()).await
                }
                AgentCommands::Proposal(args) => {
                    use enoxian::cli::ProposalAction;
                    match args.action {
                        ProposalAction::List => {
                            enoxian::commands::proposals::list(&client, &base, cli.json).await
                        }
                        ProposalAction::Show { id } => {
                            enoxian::commands::proposals::show(&client, &base, id, cli.json).await
                        }
                        ProposalAction::Accept { id } => {
                            enoxian::commands::proposals::decide(
                                &client, &base, id, "accept", cli.json,
                            )
                            .await
                        }
                        ProposalAction::Reject { id } => {
                            enoxian::commands::proposals::decide(
                                &client, &base, id, "reject", cli.json,
                            )
                            .await
                        }
                        ProposalAction::Revert { id } => {
                            enoxian::commands::proposals::decide(
                                &client, &base, id, "revert", cli.json,
                            )
                            .await
                        }
                    }
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
                | AgentCommands::Daemon(_)
                | AgentCommands::Bootstrap(_)
                | AgentCommands::Service(_)
                | AgentCommands::Update { .. }
                | AgentCommands::UpdateApply(_)
                | AgentCommands::Identity(_)
                | AgentCommands::Agent(_)
                | AgentCommands::Session(_) => unreachable!(),
            }
        }
    }
}
