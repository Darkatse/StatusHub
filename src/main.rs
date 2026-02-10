mod cache;
mod config;
mod discord;
mod event;
mod state_cache;
mod steam;
mod webhook;

use std::path::PathBuf;
use std::sync::Arc;

use anyhow::Context;
use clap::Parser;
use tracing::info;
use tracing_subscriber::EnvFilter;

use crate::cache::CacheService;
use crate::config::Settings;
use crate::state_cache::PersistentStatusCache;
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

    let cache_service = Arc::new(
        CacheService::from_settings(&settings.cache)
            .await
            .context("failed to initialize cache service")?,
    );
    info!(
        cache_enabled = cache_service.is_enabled(),
        "cache service ready"
    );

    let state_cache = if settings.state_cache.enabled {
        Some(Arc::new(
            PersistentStatusCache::load(&settings.state_cache.path, Some(cache_service.clone()))
                .with_context(|| {
                    format!(
                        "failed to initialize state cache from {}",
                        settings.state_cache.path.display()
                    )
                })?,
        ))
    } else {
        None
    };

    let sender = webhook::build_sender(
        &settings.webhook,
        &settings.message,
        &settings.steam,
        cache_service,
    )
    .context("failed to setup webhook sender")?;
    run(settings, sender, state_cache).await
}

async fn run(
    settings: Settings,
    sender: Arc<dyn WebhookSender>,
    state_cache: Option<Arc<PersistentStatusCache>>,
) -> anyhow::Result<()> {
    tokio::select! {
        result = discord::run(settings.discord, sender, state_cache) => {
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
