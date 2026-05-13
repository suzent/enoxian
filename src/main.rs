mod cli;
mod commands;
mod config;
mod crypto;
mod network;

use clap::Parser;
use cli::{Cli, Commands};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::from_default_env()
                .add_directive("enochd=info".parse()?),
        )
        .init();

    let cli = Cli::parse();
    match cli.command {
        Commands::Init(args) => commands::init::run(args).await,
        Commands::Serve(args) => commands::serve::run(args).await,
        Commands::Enter(args) => commands::enter::run(args).await,
    }
}
