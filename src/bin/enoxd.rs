use clap::Parser;
use enoxian::cli::{DaemonCli, ServeArgs};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::from_default_env()
                .add_directive("enoxian=info".parse()?),
        )
        .init();

    let cli = DaemonCli::parse();
    if cli.bootstrap {
        let relay_port = cli.relay_port.unwrap_or(cli.port.saturating_add(1));
        enoxian::bootstrap::run(cli.port, relay_port).await
    } else {
        enoxian::commands::serve::run(ServeArgs {
            port: cli.port,
            bind_lan: cli.bind_lan,
            bind: cli.bind,
        })
        .await
    }
}
