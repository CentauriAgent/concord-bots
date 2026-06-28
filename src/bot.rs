// =============================================================================
// bot.rs — Vector connection and message loop
// =============================================================================
//
// Handles:
//   1. Building the VectorBot from config
//   2. Registering handlers (commands, scheduled tasks, AI bridge)
//   3. Running the bot until shutdown

use anyhow::{Context, Result};
use std::sync::Arc;
use vector_sdk::VectorBot;

use crate::auth::AuthManager;
use crate::config::BotConfig;
use crate::handlers;
use crate::rate_limiter::RateLimiter;

/// Shared context passed to all handlers.
#[derive(Clone)]
pub struct BotContext {
    /// The Vector bot instance.
    pub bot: VectorBot,
    /// The parsed bot.toml configuration.
    pub config: Arc<BotConfig>,
    /// Authorization manager (None if auth is not configured).
    pub auth: Option<AuthManager>,
    /// Per-user spam protection.
    pub rate_limiter: RateLimiter,
}

/// Build the bot from config, register handlers, and run forever.
pub async fn run(config: BotConfig) -> Result<()> {
    tracing::info!("Starting concord-bots framework...");

    // -------------------------------------------------------------------------
    // Step 1: Build the VectorBot from config
    // -------------------------------------------------------------------------

    let mut builder = VectorBot::builder();

    let nsec = config.bot_nsec();

    if let Some(ref n) = nsec {
        tracing::info!("Using provided nsec identity");
        builder = builder.nsec(n);
    } else {
        tracing::info!("No nsec provided — bot will auto-generate and persist an identity");
    }

    match config.invite_policy() {
        crate::config::InvitePolicyConfig::Public => {
            tracing::info!("Invite policy: public (accept all invites)");
            builder = builder.public();
        }
        crate::config::InvitePolicyConfig::Whitelist(ref npubs) => {
            tracing::info!("Invite policy: whitelist ({} accounts)", npubs.len());
            builder = builder.whitelist(npubs.iter().map(|s| s.as_str()));
        }
        crate::config::InvitePolicyConfig::Manual => {
            tracing::info!("Invite policy: manual (invites require explicit acceptance)");
        }
    }

    let bot = builder
        .build()
        .await
        .context("Failed to build VectorBot — check your nsec and network connection")?;

    tracing::info!("Bot online as {}", bot.npub());

    // -------------------------------------------------------------------------
    // Step 2: Initialize auth system
    // -------------------------------------------------------------------------

    let auth = if let Some(ref owner) = config.auth.owner {
        if !owner.is_empty() {
            let state_file = std::path::PathBuf::from(&config.auth.state_file);
            match AuthManager::new(
                owner,
                &config.auth.authorized,
                config.auth.persist,
                state_file,
            ) {
                Ok(m) => {
                    tracing::info!(
                        "Auth system enabled — owner: {}, authorized users: {}",
                        owner,
                        m.authorized_count()
                    );
                    Some(m)
                }
                Err(e) => {
                    tracing::error!("Failed to initialize auth system: {}", e);
                    None
                }
            }
        } else {
            None
        }
    } else {
        tracing::info!("Auth system disabled (no owner npub configured — all commands are public)");
        None
    };

    // -------------------------------------------------------------------------
    // Step 3: Create shared context
    // -------------------------------------------------------------------------

    let ctx = BotContext {
        bot: bot.clone(),
        config: Arc::new(config),
        auth,
        rate_limiter: RateLimiter::default(),
    };

    // -------------------------------------------------------------------------
    // Step 4: Register all handlers
    // -------------------------------------------------------------------------

    handlers::register(&bot, ctx.clone()).await?;

    // -------------------------------------------------------------------------
    // Step 5: Message loop
    // -------------------------------------------------------------------------
    // The SDK's on_message() call IS the event loop — it blocks forever.
    // Do NOT also register on_event() — both call core.listen() and whichever
    // is registered first blocks the other from ever running.

    bot.on_message({
        let ctx = ctx.clone();
        move |_bot, msg| {
            let ctx = ctx.clone();
            async move {
                if msg.is_mine() {
                    return;
                }

                tracing::info!(
                    "Incoming message from {}: {}",
                    msg.chat_id,
                    msg.text()
                );

                if let Err(e) = handlers::on_message(&ctx, &msg).await {
                    tracing::error!("Handler error: {:?}", e);
                }
            }
        }
    })
    .await
    .context("Failed to register on_message handler")?;

    tracing::info!("Bot is running. Press Ctrl+C to stop.");

    tokio::signal::ctrl_c()
        .await
        .context("Failed to listen for Ctrl+C")?;

    tracing::info!("Shutdown signal received. Goodbye!");
    Ok(())
}
