mod config;
mod discord;
mod event;
mod steam;
mod webhook;

use std::path::PathBuf;
use std::sync::Arc;

use anyhow::Context;
use clap::Parser;
use tracing::info;
use tracing_subscriber::EnvFilter;

use crate::config::Settings;
use crate::webhook::WebhookSender;

#[derive(Debug, Parser)]
#[command(name = "statushub", about = "Discord status to webhook bridge")]
struct Cli {
    #[arg(
        short,
        long,
        default_value = "config.toml",
        help = "Path to configuration file"
    )]
    config: PathBuf,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    init_tracing();

    let cli = Cli::parse();
    let settings = Settings::load_from_path(&cli.config)
        .with_context(|| format!("failed to load configuration from {}", cli.config.display()))?;

    let sender = webhook::build_sender(&settings.webhook, &settings.message, &settings.steam)
        .context("failed to setup webhook sender")?;
    run(settings, sender).await
}

async fn run(settings: Settings, sender: Arc<dyn WebhookSender>) -> anyhow::Result<()> {
    tokio::select! {
        result = discord::run(settings.discord, sender) => {
            result
        }
        _ = tokio::signal::ctrl_c() => {
            info!("received shutdown signal");
            Ok(())
        }
    }
}

fn init_tracing() {
    let filter = EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| EnvFilter::new("statushub=info,serenity=warn"));

    tracing_subscriber::fmt()
        .compact()
        .with_env_filter(filter)
        .with_target(false)
        .init();
}
