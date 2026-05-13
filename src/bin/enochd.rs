use clap::Parser;
use enochian::cli::{DaemonCli, ServeArgs};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::from_default_env()
                .add_directive("enochian=info".parse()?),
        )
        .init();

    let cli = DaemonCli::parse();
    enochian::commands::serve::run(ServeArgs { port: cli.port }).await
}
