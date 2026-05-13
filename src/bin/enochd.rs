use clap::Parser;
use enochian::cli::{DaemonCli, DaemonCommands};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::from_default_env()
                .add_directive("enochian=info".parse()?),
        )
        .init();

    let cli = DaemonCli::parse();
    match cli.command {
        DaemonCommands::Serve(args) => enochian::commands::serve::run(args).await,
    }
}
