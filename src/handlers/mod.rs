// =============================================================================
// handlers/mod.rs — Handler registration and dispatch (EXTENSION POINT)
// =============================================================================
//
// This is where you wire up your bot's behavior. The framework calls:
//   - `register()` once at startup (register scheduled tasks here)
//   - `on_message()` for every incoming message (dispatch commands here)
//   - `on_event()` for non-message events (joins, reactions, etc.)
//
// By default, this module dispatches to:
//   - `commands::on_message()` — for !command handling
//   - `scheduled::register()`  — for timed/scheduled tasks
//   - `ai_bridge::on_message()` — for AI-powered responses (optional)
//
// To add new behavior:
//   1. Add command logic to commands.rs (see patterns there)
//   2. Add scheduled tasks to scheduled.rs
//   3. Optionally enable AI in ai_bridge.rs
//
// You can also create new handler files and register them here.

use anyhow::Result;
use vector_sdk::{BotEvent, VectorBot};

use crate::bot::BotContext;
use crate::config::Feature;

pub mod commands;
pub mod fun;
pub mod scheduled;
pub mod utility;
pub mod wallet_cmds;
pub mod nostr_cmds;
pub mod moderation_cmds;
pub mod ai_bridge;

/// Called once at startup. Register scheduled tasks and any one-time setup here.
///
/// # What to do here
/// - Start background tasks (interval-based posting, polling APIs, etc.)
/// - Initialize state your handlers will need
/// - Set up the AI bridge if enabled
pub async fn register(bot: &VectorBot, ctx: BotContext) -> Result<()> {
    tracing::info!("Registering handlers...");

    // Register scheduled tasks (cron-like interval jobs).
    scheduled::register(bot, ctx.clone()).await?;

    // Register the AI bridge (no-op if not configured).
    ai_bridge::register(bot, ctx.clone()).await?;

    tracing::info!("All handlers registered.");
    Ok(())
}

/// Called for every incoming message (DMs and Community channels).
///
/// This function dispatches to command handlers and the AI bridge.
/// Modify the dispatch logic here if you need custom routing.
pub async fn on_message(ctx: &BotContext, msg: &vector_sdk::IncomingMessage) -> Result<()> {
    let text = msg.text();

    // -------------------------------------------------------------------------
    // 1. Command dispatch — messages starting with "!"
    // -------------------------------------------------------------------------
    if text.starts_with('!') {
        return commands::on_message(ctx, msg).await;
    }

    // -------------------------------------------------------------------------
    // 2. AI bridge — if enabled, pass non-command messages to the AI handler
    // -------------------------------------------------------------------------
    // Uncomment the following lines to enable AI responses:
    //
    // if ai_bridge::is_enabled(ctx) {
    //     return ai_bridge::on_message(ctx, msg).await;
    // }

    // -------------------------------------------------------------------------
    // 3. Default: no handler matched
    // -------------------------------------------------------------------------
    // Add custom message handling here. For example:
    // - React to certain keywords with emoji
    // - Forward messages to another channel
    // - Log or analyze messages
    //
    // Example (uncomment to use):
    // if text.to_lowercase().contains("hello") {
    //     let _ = msg.reply("Hi there! 👋").await;
    // }

    Ok(())
}

/// Called for non-message events (member joins, reactions, typing, etc.).
///
/// Match on `BotEvent` variants to handle specific event types.
pub async fn on_event(ctx: &BotContext, event: BotEvent) -> Result<()> {
    match &event {
        // Someone joined a community channel.
        BotEvent::MemberJoin { channel_id, npub } => {
            tracing::info!("Member {} joined channel {}", npub, channel_id);

            // Don't welcome ourselves
            if npub == ctx.bot.npub() {
                return Ok(());
            }

            // Feature gate: only send welcome if community features are enabled
            if ctx.config.features.is_enabled(Feature::Community) {
                let welcome = "Welcome! 🎉 Type !help to see what I can do.";
                let _ = ctx.bot.channel(channel_id.clone()).send(welcome).await;
            }
        }

        // Someone left a community channel.
        BotEvent::MemberLeave { channel_id, npub } => {
            tracing::info!("Member {} left channel {}", npub, channel_id);
        }

        // A message was edited or received a reaction.
        BotEvent::MessageUpdate { chat_id, .. } => {
            tracing::debug!("Message update in chat {}", chat_id);
            // Handle reaction/edit events here
        }

        // A message was deleted.
        BotEvent::Delete { chat_id, message_id } => {
            tracing::debug!("Message {} deleted in chat {}", message_id, chat_id);
        }

        // The bot received a community invite.
        BotEvent::Invite { community_id } => {
            tracing::info!("Received a community invite for {}", community_id);
        }

        // The bot was removed from a community.
        BotEvent::Removed { community_id } => {
            tracing::warn!("Bot was removed from community {}", community_id);
        }

        // Typing indicator.
        BotEvent::Typing { chat_id, npub, .. } => {
            tracing::trace!("{} typing in {}", npub, chat_id);
        }

        // A new message (when using on_event instead of on_message).
        BotEvent::Message(_) => {
            // Already handled by on_message handler above.
        }
    }

    // Dispatch to command-specific event handlers if needed.
    commands::on_event(ctx, &event).await?;
    Ok(())
}
