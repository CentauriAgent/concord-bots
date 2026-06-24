// =============================================================================
// bot.rs — Vector connection and message loop (STABLE — do not edit)
// =============================================================================
//
// This module handles:
//   1. Building the VectorBot from config
//   2. Registering handlers (commands, scheduled tasks, AI bridge)
//   3. Running the bot until shutdown
//
// You should NOT need to edit this file. All customization is done via
// the handlers/ module and config/bot.toml.

use anyhow::{Context, Result};
use std::sync::Arc;
use vector_sdk::VectorBot;

use crate::config::BotConfig;
use crate::handlers;

/// Shared context passed to all handlers.
///
/// This is what your handler functions use to access the bot,
/// send messages, and read configuration. Clone it freely.
#[derive(Clone)]
pub struct BotContext {
    /// The Vector bot instance — use this to send messages, join channels, etc.
    pub bot: VectorBot,
    /// The parsed bot.toml configuration.
    pub config: Arc<BotConfig>,
}

/// Build the bot from config, register handlers, and run forever.
pub async fn run(config: BotConfig) -> Result<()> {
    tracing::info!("Starting concord-bots framework...");

    // -------------------------------------------------------------------------
    // Step 1: Build the VectorBot from config
    // -------------------------------------------------------------------------

    let mut builder = VectorBot::builder();

    // Set identity: explicit nsec from config, or NSEC env var, or auto-generate.
    let nsec = config.bot_nsec();

    if let Some(ref n) = nsec {
        tracing::info!("Using provided nsec identity");
        builder = builder.nsec(n);
    } else {
        tracing::info!("No nsec provided — bot will auto-generate and persist an identity");
        // The SDK auto-generates and stores identity.nsec when no key is given.
    }

    // Set invite policy: public, whitelist, or manual (default).
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

    // Build the bot.
    let bot = builder
        .build()
        .await
        .context("Failed to build VectorBot — check your nsec and network connection")?;

    tracing::info!("Bot online as {}", bot.npub());

    // -------------------------------------------------------------------------
    // Step 2: Create shared context and register handlers
    // -------------------------------------------------------------------------

    let ctx = BotContext {
        bot: bot.clone(),
        config: Arc::new(config),
    };

    // Register all handlers (commands, scheduled tasks, AI bridge).
    // This is where your custom code gets wired in.
    handlers::register(&bot, ctx.clone()).await?;

    // -------------------------------------------------------------------------
    // Step 3: Message loop — run until interrupted
    // -------------------------------------------------------------------------

    // The on_message handler is the main entry point for incoming messages.
    // It dispatches to command handlers and custom logic.
    bot.on_message({
        let ctx = ctx.clone();
        move |_bot, msg| {
            let ctx = ctx.clone();
            async move {
                // Skip our own messages to prevent loops.
                if msg.is_mine() {
                    return;
                }

                tracing::debug!(
                    "Message from {}: {}",
                    msg.chat_id,
                    msg.text()
                );

                // Dispatch to the handler module.
                if let Err(e) = handlers::on_message(&ctx, &msg).await {
                    tracing::error!("Handler error: {:?}", e);
                }
            }
        }
    })
    .await
    .context("Failed to register on_message handler")?;

    // Also register the event handler for non-message events (joins, reactions, etc.)
    bot.on_event({
        let ctx = ctx.clone();
        move |_bot, event| {
            let ctx = ctx.clone();
            async move {
                if let Err(e) = handlers::on_event(&ctx, event).await {
                    tracing::error!("Event handler error: {:?}", e);
                }
            }
        }
    })
    .await
    .context("Failed to register on_event handler")?;

    tracing::info!("Bot is running. Press Ctrl+C to stop.");

    // Keep the process alive. The bot runs in the background via SDK internals.
    // We just need to not exit.
    tokio::signal::ctrl_c()
        .await
        .context("Failed to listen for Ctrl+C")?;

    tracing::info!("Shutdown signal received. Goodbye!");
    Ok(())
}
