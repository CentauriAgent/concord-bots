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
use std::collections::HashMap;
use std::sync::Mutex;
use std::time::{Duration, Instant};
use once_cell::sync::Lazy;
use vector_sdk::{BotEvent, VectorBot};

use crate::bot::BotContext;
use crate::config::Feature;

#[cfg(test)]
mod normalize_tests {
    use super::normalize_npub;

    #[test]
    fn test_normalize_npub_nostr_prefix() {
        assert_eq!(normalize_npub("nostr:npub1abc"), "npub1abc");
    }

    #[test]
    fn test_normalize_npub_at_sign() {
        assert_eq!(normalize_npub("@npub1abc"), "npub1abc");
    }

    #[test]
    fn test_normalize_npub_plain() {
        assert_eq!(normalize_npub("npub1abc"), "npub1abc");
    }

    #[test]
    fn test_normalize_npub_whitespace() {
        assert_eq!(normalize_npub("  nostr:npub1abc  "), "npub1abc");
    }
}

/// Track recently welcomed members to prevent duplicate welcomes.
/// Maps (channel_id, npub) -> last welcome time.
static WELCOMED: Lazy<Mutex<HashMap<(String, String), Instant>>> =
    Lazy::new(|| Mutex::new(HashMap::new()));

/// Minimum time between welcomes for the same member in the same channel.
const WELCOME_COOLDOWN: Duration = Duration::from_secs(3600); // 1 hour

/// Whether welcome messages are enabled (toggleable via !welcome on/off).
static WELCOME_ENABLED: Lazy<Mutex<bool>> = Lazy::new(|| Mutex::new(true));

/// Set welcome message on/off. Called from the command handler.
pub fn set_welcome_enabled(enabled: bool) {
    let mut state = WELCOME_ENABLED.lock().unwrap();
    *state = enabled;
}

/// Check if welcome messages are currently enabled.
pub fn is_welcome_enabled() -> bool {
    *WELCOME_ENABLED.lock().unwrap()
}

/// Normalize a user-supplied npub argument:
/// - Strips leading `nostr:` prefix (NIP-27 mention format)
/// - Strips leading `@` (redundant but defensive)
/// - Trims whitespace
pub fn normalize_npub(input: &str) -> String {
    let s = input.trim();
    let s = s.strip_prefix("nostr:").unwrap_or(s);
    let s = s.strip_prefix('@').unwrap_or(s);
    s.to_string()
}

pub mod commands;
pub mod community_cmds;
pub mod fun;
pub mod git_cmds;
pub mod scheduled;
pub mod thread;
pub mod utility;
pub mod wallet_cmds;
pub mod nostr_cmds;
pub mod moderation_cmds;
pub mod ai_bridge;

/// Send a command response reply.
///
/// When `features.thread_replies` is enabled (default), the response is sent
/// as a kind 1111 threaded reply. Otherwise, it falls back to a regular
/// `msg.reply()` (kind 9 inline reply).
///
/// Call this instead of `msg.reply()` in command handlers for consistent
/// threading behavior across the bot.
pub async fn reply(ctx: &BotContext, msg: &vector_sdk::IncomingMessage, text: &str) -> anyhow::Result<()> {
    if ctx.config.features.thread_replies {
        thread::reply_as_thread(ctx, msg, text).await
    } else {
        msg.reply(text).await?;
        Ok(())
    }
}

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
    // 0. XP tracking — award XP for non-command messages (before dispatch)
    // --------------------------------------------------------------------------
    if !text.starts_with('!') {
        if let Some(ref npub) = msg.message.npub {
            if !npub.is_empty() && npub != &ctx.bot.npub() {
                // Only award XP if community features are enabled
                if ctx.config.features.is_enabled(crate::config::Feature::Community) {
                    award_message_xp(ctx, npub, &msg.chat_id).await;
                }
            }
        }
    }

    // -------------------------------------------------------------------------
    // 0b. File attachment logging
    // --------------------------------------------------------------------------
    if msg.is_file {
        for att in &msg.message.attachments {
            tracing::info!(
                "Attachment received: {} ({} bytes, .{}) from {}",
                att.name, att.size, att.extension, msg.chat_id
            );
        }
    }

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

// -----------------------------------------------------------------------------
// XP awarding for non-command messages
// -----------------------------------------------------------------------------

/// Award XP for a non-command message. Enforces 60-second cooldown and
/// announces level-ups. 15-25 random XP per message.
async fn award_message_xp(ctx: &BotContext, npub: &str, channel_id: &str) {
    // Check 60-second cooldown
    match ctx.community_db.is_on_xp_cooldown(npub, 60) {
        Ok(true) => return, // still on cooldown
        Ok(false) => {}
        Err(e) => {
            tracing::warn!("XP cooldown check failed: {}", e);
            return;
        }
    }

    // Increment message count
    if let Err(e) = ctx.community_db.increment_messages(npub) {
        tracing::warn!("Message count increment failed: {}", e);
    }

    // Award 15-25 random XP (compute before any .await)
    let xp = {
        use rand::Rng;
        let mut rng = rand::thread_rng();
        rng.gen_range(15..=25)
    };

    let leveled_up_info = ctx.community_db.award_xp(npub, xp, channel_id)
        .ok()
        .filter(|(_, leveled_up)| *leveled_up);

    if let Some((new_level, true)) = leveled_up_info {
        let announcement = format!("🎉 nostr:{} reached Level {}!", npub, new_level);
        let _ = ctx.bot.channel(channel_id.to_string()).send(&announcement).await;
    }
}

/// Called for non-message events (member joins, reactions, typing, etc.).
///
/// Match on `BotEvent` variants to handle specific event types.
pub async fn on_event(ctx: &BotContext, event: BotEvent) -> Result<()> {
    match &event {
        // Someone joined a community channel.
        BotEvent::MemberJoin { channel_id, npub } => {
            tracing::info!("Member {} joined channel {}", npub, channel_id);

            // If the bot itself is joining a new channel, mark it disabled by default.
            // The community owner can then !enable it explicitly.
            if npub == ctx.bot.npub() {
                if let Err(e) = ctx.community_db.disable_channel(channel_id) {
                    tracing::warn!("Failed to mark channel {} as disabled-on-join: {}", channel_id, e);
                } else {
                    tracing::info!("Channel {} marked disabled (opt-in default)", channel_id);
                }
                return Ok(());
            }

            // Feature gate: only send welcome if community features are enabled AND welcome is on
            if ctx.config.features.is_enabled(Feature::Community) && is_welcome_enabled() {
                // Dedup: only welcome once per member per channel per hour
                let key = (channel_id.clone(), npub.clone());
                let should_welcome = {
                    let mut map = WELCOMED.lock().unwrap();
                    if let Some(last) = map.get(&key) {
                        if last.elapsed() < WELCOME_COOLDOWN {
                            false
                        } else {
                            map.insert(key.clone(), Instant::now());
                            true
                        }
                    } else {
                        map.insert(key.clone(), Instant::now());
                        true
                    }
                };

                if should_welcome {
                    let welcome = "Welcome! 🎉 Type !help to see what I can do.";
                    let _ = ctx.bot.channel(channel_id.clone()).send(welcome).await;
                }
            }
        }

        // Someone left a community channel.
        BotEvent::MemberLeave { channel_id, npub } => {
            tracing::info!("Member {} left channel {}", npub, channel_id);
        }

        // A message was edited or received a reaction.
        BotEvent::MessageUpdate { chat_id, message } => {
            tracing::debug!("Message update in chat {}: {} reactions", chat_id, message.reactions.len());
            // Award XP for reactions on the author's message (community engagement)
            if ctx.config.features.is_enabled(Feature::Community) && !message.reactions.is_empty() {
                if let Some(ref npub) = message.npub {
                    if npub != &ctx.bot.npub() {
                        // Small XP for getting a reaction (3-5 XP)
                        let xp = {
                            use rand::Rng;
                            let mut rng = rand::thread_rng();
                            rng.gen_range(3..=5)
                        };
                        let _ = ctx.community_db.award_xp(npub, xp, chat_id);
                    }
                }
            }
        }

        // A message was deleted.
        BotEvent::Delete { chat_id, message_id } => {
            tracing::info!("Message {} deleted in chat {}", message_id, chat_id);
        }

        // The bot received a community invite.
        BotEvent::Invite { community_id } => {
            tracing::info!("Received a community invite for {}", community_id);
        }

        // The bot was removed from a community.
        BotEvent::Removed { community_id } => {
            tracing::warn!("Bot was removed from community {}", community_id);
            // Notify the owner via DM if auth is configured
            if let Some(ref auth) = ctx.auth {
                if let Some(ref owner) = auth.owner_npub() {
                    let _ = ctx.bot.dm(owner).send(
                        &format!("⚠️ I was removed from community {}", community_id)
                    ).await;
                }
            }
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
