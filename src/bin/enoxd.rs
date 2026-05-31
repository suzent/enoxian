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
        enoxian::bootstrap::run(cli.port).await
    } else {
        enoxian::commands::serve::run(ServeArgs { port: cli.port }).await
    }
}
