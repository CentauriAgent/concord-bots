// =============================================================================
// main.rs — Entry point (STABLE — do not edit)
// =============================================================================
//
// This file boots the bot framework. You should NOT need to modify it.
// All customization happens in src/handlers/.
//
// To configure your bot, edit config/bot.toml (see config/bot.toml.example).

use anyhow::Result;

mod auth;
mod bot;
mod community;
mod config;
mod git_monitor;
mod handlers;
mod lib;
mod rate_limiter;
mod wallet;

#[tokio::main]
async fn main() -> Result<()> {
    // Initialize structured logging.
    // Logs go to stdout, captured by systemd/journald or Docker.
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .init();

    // Load configuration from config/bot.toml (or path in BOT_CONFIG env var).
    let bot_config = config::BotConfig::load()?;
    bot_config.log_summary();

    // Build and start the bot.
    bot::run(bot_config).await
}
