// =============================================================================
// handlers/commands.rs — Command dispatcher and built-in commands
// =============================================================================
//
// This is the central command dispatcher. All messages starting with "!" are
// routed here. The dispatcher:
//   1. Runs spam protection (rate limiter)
//   2. Parses the command name
//   3. Routes to the appropriate handler (built-in, utility, or fun)
//

use anyhow::Result;
use once_cell::sync::Lazy;
use std::collections::HashMap;
use std::sync::Mutex;
use std::time::{Duration, Instant};
use vector_sdk::{BotEvent, IncomingMessage};

use crate::auth::AuthLevel;
use crate::bot::BotContext;
use crate::config::{Feature, FeaturesSection};
use crate::handlers::fun;
use crate::handlers::utility;
use crate::handlers::wallet_cmds;
use crate::handlers::nostr_cmds;
use crate::handlers::moderation_cmds;
use crate::handlers::community_cmds;
use crate::handlers::{normalize_npub, git_cmds};
use crate::rate_limiter::RateLimitResult;

/// Track last !help per channel to prevent spam. Maps channel_id -> last help time.
static HELP_COOLDOWN: Lazy<Mutex<HashMap<String, Instant>>> =
    Lazy::new(|| Mutex::new(HashMap::new()));

/// Cooldown for !help per channel (3 minutes).
const HELP_COOLDOWN_DURATION: Duration = Duration::from_secs(180);

// -----------------------------------------------------------------------------
// Auth helpers
// -----------------------------------------------------------------------------

/// Extract the sender's npub from an incoming message.
fn sender_npub(msg: &IncomingMessage) -> String {
    msg.message.npub.clone().unwrap_or_default()
}

/// Check if the sender meets the required auth level.
/// Extract community_id from a message (for per-community auth scoping).
/// Returns None for DMs.
fn community_id_from_msg(msg: &IncomingMessage) -> Option<String> {
    msg.community().map(|c| c.id().to_string())
}

async fn require_auth(ctx: &BotContext, msg: &IncomingMessage, level: AuthLevel) -> Result<bool> {
    let Some(ref auth) = ctx.auth else {
        return Ok(true); // Auth not configured — allow all
    };

    let npub = sender_npub(msg);
    let cid = community_id_from_msg(msg);
    tracing::info!("Auth check for npub={} community={:?} level={:?}", npub, cid, level);
    if auth.has_permission(&npub, cid.as_deref(), level) {
        return Ok(true);
    }

    let response = match level {
        AuthLevel::Owner => "⛔ Owner only.".to_string(),
        AuthLevel::Authorized => {
            "⛔ Not authorized. Ask the owner to run !add <your-npub>".to_string()
        }
        AuthLevel::Public => unreachable!("Public level should never be checked"),
    };
    if let Err(e) = super::reply(ctx, msg, &response).await {
        tracing::error!("Failed to send auth denial reply: {:?}", e);
    }
    Ok(false)
}

// -----------------------------------------------------------------------------
// Main command dispatcher
// -----------------------------------------------------------------------------

pub async fn on_message(ctx: &BotContext, msg: &IncomingMessage) -> Result<()> {
    // Track all messages for stats.
    utility::increment_message_count();

    let text = msg.text();
    let parts: Vec<&str> = text.splitn(2, ' ').collect();
    let command = parts[0];
    let args = parts.get(1).copied().unwrap_or("");

    // -------------------------------------------------------------------------
    // Channel enable/disable check — always allow !enable/!disable through,
    // even when the channel is disabled. Everything else is gated.
    // -------------------------------------------------------------------------
    if command != "!enable" && command != "!disable" {
        match ctx.community_db.is_channel_enabled(&msg.chat_id) {
            Ok(true) => {} // channel enabled, proceed
            Ok(false) => {
                tracing::debug!(
                    "Ignoring command {} in disabled channel {}",
                    command,
                    msg.chat_id
                );
                return Ok(());
            }
            Err(e) => {
                tracing::warn!("Failed to check channel state: {}", e);
                // Fail open — allow the command through if we can't check state
            }
        }
    }

    // -------------------------------------------------------------------------
    // Spam protection — check rate limit before any command.
    // -------------------------------------------------------------------------
    // Use the sender's npub (or chat_id as fallback for DMs without npub).
    let user_id = &sender_npub(msg);
    let user_key = if user_id.is_empty() {
        &msg.chat_id
    } else {
        user_id
    };

    match ctx.rate_limiter.check(user_key, command).await {
        RateLimitResult::Allow => {}
        RateLimitResult::Deny(reason) => {
            super::reply(ctx, msg, &reason).await?;
            return Ok(());
        }
    }

    let features = &ctx.config.features;

    match command {
        // =====================================================================
        // CORE (always enabled)
        // =====================================================================

        "!ping" => {
            super::reply(ctx, msg, "pong 🏓").await?;
        }

        "!repo" => {
            super::reply(ctx, msg, "📦 concord-bots\nhttps://github.com/CentauriAgent/concord-bots\n\nClone it, build your own bot, join the fleet! 🚢").await?;
        }

        "!help" => {
            // Cooldown: only show help once per channel per 3 minutes
            let channel_id = msg.chat_id.clone();
            let should_show = {
                let mut map = HELP_COOLDOWN.lock().unwrap();
                if let Some(last) = map.get(&channel_id) {
                    if last.elapsed() < HELP_COOLDOWN_DURATION {
                        false
                    } else {
                        map.insert(channel_id.clone(), Instant::now());
                        true
                    }
                } else {
                    map.insert(channel_id.clone(), Instant::now());
                    true
                }
            };
            if should_show {
                super::reply(ctx, msg, &help_text(&ctx.config.features)).await?;
            } else {
                super::reply(ctx, msg, "ℹ️ Help was recently shown in this channel. Try again in a few minutes.").await?;
            }
        }

        "!echo" => {
            if args.is_empty() {
                super::reply(ctx, msg, "Usage: !echo <text>").await?;
            } else {
                super::reply(ctx, msg, args).await?;
            }
        }

        "!whoami" => {
            let npub = ctx.bot.npub();
            let info = format!(
                "I am concord-bot 🤖\nnpub: {}\nFramework: concord-bots v{}",
                npub,
                env!("CARGO_PKG_VERSION")
            );
            super::reply(ctx, msg, &info).await?;
        }

        "!auth" => {
            auth_command(ctx, msg).await?;
        }

        // =====================================================================
        // UTILITY (gated by features.utility)
        // =====================================================================

        "!price" | "!time" | "!roll" | "!stats" | "!weather"
        | "!remind" | "!poll" | "!translate" | "!define" | "!quote"
        | "!joke" | "!fact" | "!meme" | "!shorten"
        | "!delete" | "!edit" | "!savefile"
            if features.utility => {
            dispatch_utility(ctx, msg, command, args).await?;
        }

        // =====================================================================
        // FUN (gated by features.fun)
        // =====================================================================

        "!8ball" | "!flip" | "!choose" | "!rps"
            if features.fun => {
            dispatch_fun(ctx, msg, command, args).await?;
        }

        // =====================================================================
        // AUTH MANAGEMENT (always enabled, owner-only)
        // =====================================================================

        "!add" => {
            if !require_auth(ctx, msg, AuthLevel::Owner).await? {
                return Ok(());
            }
            add_command(ctx, msg, args).await?;
        }

        "!remove" => {
            if !require_auth(ctx, msg, AuthLevel::Owner).await? {
                return Ok(());
            }
            remove_command(ctx, msg, args).await?;
        }

        "!list" => {
            if !require_auth(ctx, msg, AuthLevel::Owner).await? {
                return Ok(());
            }
            list_command(ctx, msg).await?;
        }

        "!enable" => {
            if !require_auth(ctx, msg, AuthLevel::Owner).await? {
                return Ok(());
            }
            enable_command(ctx, msg).await?;
        }

        "!disable" => {
            if !require_auth(ctx, msg, AuthLevel::Owner).await? {
                return Ok(());
            }
            disable_command(ctx, msg).await?;
        }

        // =====================================================================
        // WALLET (gated by features.nostr — Cashu is a Nostr-adjacent feature)
        // =====================================================================

        "!balance" | "!withdraw"
            if features.nostr => {
            // Owner-only: viewing balance and draining wallet
            if !require_auth(ctx, msg, AuthLevel::Owner).await? {
                return Ok(());
            }
            dispatch_wallet(ctx, msg, command, args).await?;
        }

        "!tip"
            if features.nostr => {
            // Authorized+: community members can tip, lurkers can't drain
            if !require_auth(ctx, msg, AuthLevel::Authorized).await? {
                return Ok(());
            }
            dispatch_wallet(ctx, msg, command, args).await?;
        }

        "!zap"
            if features.nostr => {
            // Authorized+: community members can zap, lurkers can't drain
            if !require_auth(ctx, msg, AuthLevel::Authorized).await? {
                return Ok(());
            }
            dispatch_wallet(ctx, msg, command, args).await?;
        }

        "!deposit"
            if features.nostr => {
            // Public: anyone can fund the bot
            dispatch_wallet(ctx, msg, command, args).await?;
        }

        // =====================================================================
        // NOSTR (gated by features.nostr)
        // =====================================================================

        "!nostr" | "!nip05"
            if features.nostr => {
            dispatch_nostr(ctx, msg, command, args).await?;
        }

        "!follow"
            if features.nostr => {
            if !require_auth(ctx, msg, AuthLevel::Owner).await? {
                return Ok(());
            }
            dispatch_nostr(ctx, msg, command, args).await?;
        }

        // =====================================================================
        // MODERATION (gated by features.moderation)
        // =====================================================================

        "!kick"
            if features.moderation => {
            if !require_auth(ctx, msg, AuthLevel::Authorized).await? {
                return Ok(());
            }
            dispatch_moderation(ctx, msg, command, args).await?;
        }

        "!ban" | "!unban"
            if features.moderation => {
            if !require_auth(ctx, msg, AuthLevel::Owner).await? {
                return Ok(());
            }
            dispatch_moderation(ctx, msg, command, args).await?;
        }

        "!warn" | "!warnings"
            if features.moderation => {
            if !require_auth(ctx, msg, AuthLevel::Authorized).await? {
                return Ok(());
            }
            dispatch_moderation(ctx, msg, command, args).await?;
        }

        "!grantmod" | "!revokemod"
            if features.moderation => {
            if !require_auth(ctx, msg, AuthLevel::Owner).await? {
                return Ok(());
            }
            dispatch_moderation(ctx, msg, command, args).await?;
        }

        "!welcome"
            if features.community => {
            if !require_auth(ctx, msg, AuthLevel::Owner).await? {
                return Ok(());
            }
            let enabled = match args.trim() {
                "on" => { crate::handlers::set_welcome_enabled(true); true }
                "off" => { crate::handlers::set_welcome_enabled(false); false }
                _ => {
                    let state = if crate::handlers::is_welcome_enabled() { "on" } else { "off" };
                    super::reply(ctx, msg, &format!("Welcome messages are currently **{}**. Usage: !welcome on/off", state)).await?;
                    return Ok(());
                }
            };
            let state = if enabled { "ON ✅" } else { "OFF ❌" };
            super::reply(ctx, msg, &format!("Welcome messages turned {}", state)).await?;
        }

        // =====================================================================
        // COMMUNITY (gated by features.community)
        // =====================================================================

        "!level" | "!rank"
            if features.community => {
            dispatch_community(ctx, msg, command, args).await?;
        }

        "!leaderboard"
            if features.community => {
            dispatch_community(ctx, msg, command, args).await?;
        }

        "!profile"
            if features.community => {
            dispatch_community(ctx, msg, command, args).await?;
        }

        "!giveaway"
            if features.community => {
            if !require_auth(ctx, msg, AuthLevel::Authorized).await? {
                return Ok(());
            }
            dispatch_community(ctx, msg, command, args).await?;
        }

        "!rep"
            if features.community => {
            dispatch_community(ctx, msg, command, args).await?;
        }

        // =====================================================================
        // GIT MONITOR (gated by features.git_monitor)
        // =====================================================================

        "!git"
            if features.git_monitor => {
            // Subcommands have their own auth:
            //   add/remove: Authorized+
            //   list: Public
            //   poll: Owner
            // Parse subcommand to check auth
            let sub = text.splitn(3, ' ').nth(1).unwrap_or("");
            let needs_auth = match sub {
                "add" | "remove" | "rm" | "delete" => {
                    if !require_auth(ctx, msg, AuthLevel::Authorized).await? {
                        return Ok(());
                    }
                    true
                }
                "poll" => {
                    if !require_auth(ctx, msg, AuthLevel::Owner).await? {
                        return Ok(());
                    }
                    true
                }
                _ => false,
            };
            let _ = needs_auth;
            // Extract args after "!git "
            let git_args = text.strip_prefix("!git ").unwrap_or("");
            git_cmds::git_command(ctx, msg, git_args).await?;
        }

        // =====================================================================
        // V2 COMMUNITY MANAGEMENT (gated by features.community)
        // =====================================================================

        "!community" | "!invite" | "!join" | "!members" | "!channels" | "!roles" | "!caps"
            if features.community => {
            dispatch_v2_community(ctx, msg, command, args).await?;
        }

        // =====================================================================
        // UNKNOWN COMMAND — silently ignore
        // =====================================================================

        _ => {
            tracing::debug!("Unknown command: {}", command);
        }
    }

    Ok(())
}

/// Handle non-message events for commands (typically unused).
pub async fn on_event(_ctx: &BotContext, _event: &BotEvent) -> Result<()> {
    Ok(())
}

// -----------------------------------------------------------------------------
// Feature-gated dispatch helpers
// -----------------------------------------------------------------------------

/// Dispatch utility commands by feature gate.
async fn dispatch_utility(
    ctx: &BotContext,
    msg: &IncomingMessage,
    command: &str,
    args: &str,
) -> Result<()> {
    match command {
        "!price" => utility::price_command(ctx, msg).await?,
        "!time" => utility::time_command(ctx, msg, args).await?,
        "!roll" => utility::roll_command(ctx, msg, args).await?,
        "!stats" => utility::stats_command(ctx, msg).await?,
        "!weather" => utility::weather_command(ctx, msg, args).await?,
        "!remind" => utility::remind_command(ctx, msg, args).await?,
        "!poll" => utility::poll_command(ctx, msg, args).await?,
        "!translate" => utility::translate_command(ctx, msg, args).await?,
        "!define" => utility::define_command(ctx, msg, args).await?,
        "!quote" => utility::quote_command(ctx, msg).await?,
        "!joke" => utility::joke_command(ctx, msg).await?,
        "!fact" => utility::fact_command(ctx, msg).await?,
        "!meme" => utility::meme_command(ctx, msg).await?,
        "!shorten" => utility::shorten_command(ctx, msg, args).await?,
        "!delete" => utility::delete_command(ctx, msg, args).await?,
        "!edit" => utility::edit_command(ctx, msg, args).await?,
        "!savefile" => utility::savefile_command(ctx, msg, args).await?,
        _ => unreachable!("dispatch_utility called with non-utility command: {}", command),
    }
    Ok(())
}

/// Dispatch fun commands by feature gate.
async fn dispatch_fun(
    ctx: &BotContext,
    msg: &IncomingMessage,
    command: &str,
    args: &str,
) -> Result<()> {
    match command {
        "!8ball" => fun::eight_ball_command(ctx, msg, args).await?,
        "!flip" => fun::flip_command(ctx, msg).await?,
        "!choose" => fun::choose_command(ctx, msg, args).await?,
        "!rps" => fun::rps_command(ctx, msg, args).await?,
        _ => unreachable!("dispatch_fun called with non-fun command: {}", command),
    }
    Ok(())
}

/// Dispatch wallet commands.
async fn dispatch_wallet(
    ctx: &BotContext,
    msg: &IncomingMessage,
    command: &str,
    args: &str,
) -> Result<()> {
    match command {
        "!balance" => wallet_cmds::balance_command(ctx, msg).await?,
        "!tip" => wallet_cmds::tip_command(ctx, msg, args).await?,
        "!zap" => wallet_cmds::zap_command(ctx, msg, args).await?,
        "!deposit" => wallet_cmds::deposit_command(ctx, msg, args).await?,
        "!withdraw" => wallet_cmds::withdraw_command(ctx, msg, args).await?,
        _ => unreachable!("dispatch_wallet called with non-wallet command: {}", command),
    }
    Ok(())
}

/// Dispatch Nostr commands.
async fn dispatch_nostr(
    ctx: &BotContext,
    msg: &IncomingMessage,
    command: &str,
    args: &str,
) -> Result<()> {
    match command {
        "!nostr" => nostr_cmds::nostr_command(ctx, msg, args).await?,
        "!nip05" => nostr_cmds::nip05_command(ctx, msg, args).await?,
        "!follow" => nostr_cmds::follow_command(ctx, msg, args).await?,
        _ => unreachable!("dispatch_nostr called with non-nostr command: {}", command),
    }
    Ok(())
}

/// Dispatch moderation commands.
async fn dispatch_moderation(
    ctx: &BotContext,
    msg: &IncomingMessage,
    command: &str,
    args: &str,
) -> Result<()> {
    match command {
        "!kick" => moderation_cmds::kick_command(ctx, msg, args).await?,
        "!ban" => moderation_cmds::ban_command(ctx, msg, args).await?,
        "!unban" => moderation_cmds::unban_command(ctx, msg, args).await?,
        "!warn" => moderation_cmds::warn_command(ctx, msg, args).await?,
        "!warnings" => moderation_cmds::warnings_command(ctx, msg, args).await?,
        "!grantmod" => moderation_cmds::grantmod_command(ctx, msg, args).await?,
        "!revokemod" => moderation_cmds::revokemod_command(ctx, msg, args).await?,
        _ => unreachable!("dispatch_moderation called with non-moderation command: {}", command),
    }
    Ok(())
}

/// Dispatch community engagement commands.
async fn dispatch_community(
    ctx: &BotContext,
    msg: &IncomingMessage,
    command: &str,
    args: &str,
) -> Result<()> {
    match command {
        "!level" | "!rank" => community_cmds::level_command(ctx, msg, args).await?,
        "!leaderboard" => community_cmds::leaderboard_command(ctx, msg).await?,
        "!profile" => community_cmds::profile_command(ctx, msg, args).await?,
        "!giveaway" => community_cmds::giveaway_command(ctx, msg, args).await?,
        "!rep" => community_cmds::rep_command(ctx, msg, args).await?,
        _ => unreachable!("dispatch_community called with non-community command: {}", command),
    }
    Ok(())
}

/// Dispatch v2 community management commands.
async fn dispatch_v2_community(
    ctx: &BotContext,
    msg: &IncomingMessage,
    command: &str,
    args: &str,
) -> Result<()> {
    match command {
        "!community" => community_cmds::v2_community_command(ctx, msg, args).await?,
        "!invite" => community_cmds::v2_invite_command(ctx, msg, args).await?,
        "!join" => {
            if !require_auth(ctx, msg, AuthLevel::Owner).await? {
                return Ok(());
            }
            community_cmds::v2_join_command(ctx, msg, args).await?;
        }
        "!members" => community_cmds::v2_members_command(ctx, msg).await?,
        "!channels" => community_cmds::v2_channels_command(ctx, msg).await?,
        "!roles" => community_cmds::v2_roles_command(ctx, msg).await?,
        "!caps" => community_cmds::v2_caps_command(ctx, msg).await?,
        _ => unreachable!("dispatch_v2_community called with unknown command: {}", command),
    }
    Ok(())
}

// =============================================================================
// BUILT-IN AUTH COMMAND IMPLEMENTATIONS
// =============================================================================

async fn auth_command(ctx: &BotContext, msg: &IncomingMessage) -> Result<()> {
    let Some(ref auth) = ctx.auth else {
        super::reply(ctx, msg, "Auth system is not configured. All commands are public.").await?;
        return Ok(());
    };

    let npub = sender_npub(msg);
    if npub.is_empty() {
        super::reply(ctx, msg, "⚠️ Could not determine your npub from this message.").await?;
        return Ok(());
    }

    let cid = community_id_from_msg(msg);
    let level = auth.check(&npub, cid.as_deref());
    let scope = if cid.is_some() { "this community" } else { "DMs (global only)" };
    let status_text = match level {
        AuthLevel::Owner => format!("👑 You are the **owner**.\nnpub: {}", npub),
        AuthLevel::Authorized => format!("✅ You are **authorized** in {}.\nnpub: {}", scope, npub),
        AuthLevel::Public => format!(
            "❌ You are **not authorized** in {}.\nnpub: {}\nAsk the owner to run: !add {}",
            scope, npub, npub
        ),
    };

    super::reply(ctx, msg, &status_text).await?;
    Ok(())
}

async fn add_command(ctx: &BotContext, msg: &IncomingMessage, args: &str) -> Result<()> {
    let Some(ref auth) = ctx.auth else {
        super::reply(ctx, msg, "Auth system is not configured.").await?;
        return Ok(());
    };

    // Parse: !add <npub> [global]
    let parts: Vec<&str> = args.split_whitespace().collect();
    if parts.is_empty() {
        super::reply(ctx, msg, "Usage: !add <npub> [global]\n\nAdds user to this community. Use 'global' to authorize in all communities.").await?;
        return Ok(());
    }

    let npub = normalize_npub(parts[0]);
    let as_global = parts.get(1).map(|s| *s == "global").unwrap_or(false);

    if npub.is_empty() || !npub.starts_with("npub1") {
        super::reply(ctx, msg, "⚠️ That doesn't look like a valid npub. Use npub1... or nostr:npub1...").await?;
        return Ok(());
    }

    if auth.is_owner(&npub) {
        super::reply(ctx, msg, "ℹ️ That npub is already the owner — no need to add.").await?;
        return Ok(());
    }

    let cid = if as_global { None } else { community_id_from_msg(msg) };
    let scope_label = if as_global { "globally" } else { "in this community" };

    if auth.is_authorized(&npub, cid.as_deref()) {
        super::reply(ctx, msg, &format!("ℹ️ {} is already authorized {}.", npub, scope_label)).await?;
        return Ok(());
    }

    auth.add(&npub, cid.as_deref());
    super::reply(ctx, msg, &format!("✅ Added {} to authorized users {}.", npub, scope_label)).await?;
    tracing::info!("Authorized user added: {} scope={:?}", npub, cid);
    Ok(())
}

async fn remove_command(ctx: &BotContext, msg: &IncomingMessage, args: &str) -> Result<()> {
    let Some(ref auth) = ctx.auth else {
        super::reply(ctx, msg, "Auth system is not configured.").await?;
        return Ok(());
    };

    let parts: Vec<&str> = args.split_whitespace().collect();
    if parts.is_empty() {
        super::reply(ctx, msg, "Usage: !remove <npub> [global]").await?;
        return Ok(());
    }

    let npub = normalize_npub(parts[0]);
    let from_global = parts.get(1).map(|s| *s == "global").unwrap_or(false);

    if npub.is_empty() {
        super::reply(ctx, msg, "Usage: !remove <npub>").await?;
        return Ok(());
    }

    if auth.is_owner(&npub) {
        super::reply(ctx, msg, "⚠️ Cannot remove the owner.").await?;
        return Ok(());
    }

    let cid = if from_global { None } else { community_id_from_msg(msg) };
    let scope_label = if from_global { "global" } else { "this community" };

    if !auth.is_authorized(&npub, cid.as_deref()) {
        super::reply(ctx, msg, &format!("ℹ️ {} is not authorized in {}.", npub, scope_label)).await?;
        return Ok(());
    }

    auth.remove(&npub, cid.as_deref());
    super::reply(ctx, msg, &format!("✅ Removed {} from {} authorized users.", npub, scope_label)).await?;
    tracing::info!("Authorized user removed: {} scope={:?}", npub, cid);
    Ok(())
}

async fn list_command(ctx: &BotContext, msg: &IncomingMessage) -> Result<()> {
    let Some(ref auth) = ctx.auth else {
        super::reply(ctx, msg, "Auth system is not configured.").await?;
        return Ok(());
    };

    let owner = auth.owner();
    let cid = community_id_from_msg(msg);
    let (global, community) = auth.list(cid.as_deref());

    let mut lines = vec![format!("Owner: {}", owner)];

    if !global.is_empty() {
        lines.push(format!("\n🌍 Global authorized ({}):", global.len()));
        for n in &global {
            lines.push(format!("  • {}", n));
        }
    }

    if !community.is_empty() {
        lines.push(format!("\n📍 This community authorized ({}):", community.len()));
        for n in &community {
            lines.push(format!("  • {}", n));
        }
    }

    if global.is_empty() && community.is_empty() {
        lines.push("Authorized users: (none)".to_string());
    }

    super::reply(ctx, msg, &lines.join("\n")).await?;
    Ok(())
}

// -----------------------------------------------------------------------------
// Channel enable/disable
// -----------------------------------------------------------------------------

async fn enable_command(ctx: &BotContext, msg: &IncomingMessage) -> Result<()> {
    let channel_id = &msg.chat_id;
    let npub = sender_npub(msg);

    match ctx.community_db.is_channel_enabled(channel_id) {
        Ok(true) => {
            super::reply(ctx, msg, "✅ This channel is already enabled.").await?;
        }
        Ok(false) => {
            if let Err(e) = ctx.community_db.set_channel_enabled(channel_id, true, &npub) {
                tracing::error!("Failed to enable channel: {}", e);
                super::reply(ctx, msg, "⚠️ Failed to enable channel.").await?;
                return Ok(());
            }
            super::reply(ctx, msg, "✅ Bot enabled for this channel. I'm listening!").await?;
            tracing::info!("Channel {} enabled by {}", channel_id, npub);
        }
        Err(e) => {
            tracing::error!("Channel state check failed: {}", e);
            super::reply(ctx, msg, "⚠️ Could not check channel state.").await?;
        }
    }
    Ok(())
}

async fn disable_command(ctx: &BotContext, msg: &IncomingMessage) -> Result<()> {
    let channel_id = &msg.chat_id;
    let npub = sender_npub(msg);

    match ctx.community_db.is_channel_enabled(channel_id) {
        Ok(true) => {
            if let Err(e) = ctx.community_db.set_channel_enabled(channel_id, false, &npub) {
                tracing::error!("Failed to disable channel: {}", e);
                super::reply(ctx, msg, "⚠️ Failed to disable channel.").await?;
                return Ok(());
            }
            super::reply(ctx, msg, "🔇 Bot disabled for this channel. I'll stop responding here. Use !enable to turn me back on.").await?;
            tracing::info!("Channel {} disabled by {}", channel_id, npub);
        }
        Ok(false) => {
            super::reply(ctx, msg, "🔇 This channel is already disabled.").await?;
        }
        Err(e) => {
            tracing::error!("Channel state check failed: {}", e);
            super::reply(ctx, msg, "⚠️ Could not check channel state.").await?;
        }
    }
    Ok(())
}

// =============================================================================
// COMMAND REGISTRY — single source of truth for help generation
// =============================================================================

/// Metadata for a single command, for help generation and feature gating.
struct CommandMeta {
    name: &'static str,
    description: &'static str,
    feature: Option<Feature>, // None = always-on (core commands)
    auth: AuthLevel,
}

/// Single source of truth for all commands.
const COMMAND_REGISTRY: &[CommandMeta] = &[
    // Core (always enabled)
    CommandMeta { name: "!ping",     description: "Health check",                       feature: None, auth: AuthLevel::Public },
    CommandMeta { name: "!repo",     description: "Bot repo URL",                        feature: None, auth: AuthLevel::Public },
    CommandMeta { name: "!help",     description: "Show this help",                     feature: None, auth: AuthLevel::Public },
    CommandMeta { name: "!echo",     description: "Echo text back",                     feature: None, auth: AuthLevel::Public },
    CommandMeta { name: "!whoami",   description: "Bot identity",                       feature: None, auth: AuthLevel::Public },
    CommandMeta { name: "!auth",     description: "Your auth status",                   feature: None, auth: AuthLevel::Public },
    CommandMeta { name: "!stats",    description: "Bot statistics",                     feature: None, auth: AuthLevel::Public },
    CommandMeta { name: "!add",      description: "Authorize a user",                   feature: None, auth: AuthLevel::Owner },
    CommandMeta { name: "!remove",   description: "Deauthorize a user",                 feature: None, auth: AuthLevel::Owner },
    CommandMeta { name: "!list",     description: "List authorized users",              feature: None, auth: AuthLevel::Owner },
    CommandMeta { name: "!enable",   description: "Enable bot in this channel",          feature: None, auth: AuthLevel::Owner },
    CommandMeta { name: "!disable",  description: "Disable bot in this channel",         feature: None, auth: AuthLevel::Owner },

    // Utility
    CommandMeta { name: "!price",    description: "Bitcoin price (USD)",                feature: Some(Feature::Utility), auth: AuthLevel::Public },
    CommandMeta { name: "!time",     description: "Current time [timezone]",            feature: Some(Feature::Utility), auth: AuthLevel::Public },
    CommandMeta { name: "!roll",     description: "Dice roller [NdS]",                  feature: Some(Feature::Utility), auth: AuthLevel::Public },
    CommandMeta { name: "!weather",  description: "Weather <zipcode>",                  feature: Some(Feature::Utility), auth: AuthLevel::Public },
    CommandMeta { name: "!remind",   description: "Set a reminder",                    feature: Some(Feature::Utility), auth: AuthLevel::Public },
    CommandMeta { name: "!poll",     description: "Create a poll",                     feature: Some(Feature::Utility), auth: AuthLevel::Public },
    CommandMeta { name: "!translate", description: "Translate <lang> <text>",          feature: Some(Feature::Utility), auth: AuthLevel::Public },
    CommandMeta { name: "!define",   description: "Dictionary definition",             feature: Some(Feature::Utility), auth: AuthLevel::Public },
    CommandMeta { name: "!quote",    description: "Random inspirational quote",        feature: Some(Feature::Utility), auth: AuthLevel::Public },
    CommandMeta { name: "!joke",     description: "Random dad joke",                   feature: Some(Feature::Utility), auth: AuthLevel::Public },
    CommandMeta { name: "!fact",     description: "Random fun fact",                  feature: Some(Feature::Utility), auth: AuthLevel::Public },
    CommandMeta { name: "!meme",     description: "Random meme",                      feature: Some(Feature::Utility), auth: AuthLevel::Public },
    CommandMeta { name: "!shorten",  description: "Shorten a URL",                   feature: Some(Feature::Utility), auth: AuthLevel::Public },
    CommandMeta { name: "!delete",   description: "Delete a message by ID",             feature: Some(Feature::Utility), auth: AuthLevel::Authorized },
    CommandMeta { name: "!edit",     description: "Edit a message by ID",               feature: Some(Feature::Utility), auth: AuthLevel::Authorized },
    CommandMeta { name: "!savefile", description: "Save an attachment to disk",          feature: Some(Feature::Utility), auth: AuthLevel::Authorized },

    // Fun
    CommandMeta { name: "!8ball",    description: "Magic 8-ball",                     feature: Some(Feature::Fun), auth: AuthLevel::Public },
    CommandMeta { name: "!flip",     description: "Flip a coin",                     feature: Some(Feature::Fun), auth: AuthLevel::Public },
    CommandMeta { name: "!choose",   description: "Pick randomly",                  feature: Some(Feature::Fun), auth: AuthLevel::Public },
    CommandMeta { name: "!rps",      description: "Rock paper scissors",           feature: Some(Feature::Fun), auth: AuthLevel::Public },

    // Wallet (gated by Nostr feature)
    CommandMeta { name: "!balance",  description: "Show Cashu wallet balance",         feature: Some(Feature::Nostr), auth: AuthLevel::Owner },
    CommandMeta { name: "!tip",      description: "Tip sats as Cashu token",           feature: Some(Feature::Nostr), auth: AuthLevel::Authorized },
    CommandMeta { name: "!zap",      description: "Zap sats via NIP-57",                   feature: Some(Feature::Nostr), auth: AuthLevel::Authorized },
    CommandMeta { name: "!deposit",  description: "Generate Lightning deposit invoice", feature: Some(Feature::Nostr), auth: AuthLevel::Public },
    CommandMeta { name: "!withdraw", description: "Pay Lightning invoice from wallet", feature: Some(Feature::Nostr), auth: AuthLevel::Owner },

    // Nostr
    CommandMeta { name: "!nostr",    description: "Look up a Nostr profile",            feature: Some(Feature::Nostr), auth: AuthLevel::Public },
    CommandMeta { name: "!nip05",    description: "Verify a NIP-05 identifier",         feature: Some(Feature::Nostr), auth: AuthLevel::Public },
    CommandMeta { name: "!follow",   description: "Follow a user on Nostr",             feature: Some(Feature::Nostr), auth: AuthLevel::Owner },

    // Moderation
    CommandMeta { name: "!kick",     description: "Kick a member",                       feature: Some(Feature::Moderation), auth: AuthLevel::Authorized },
    CommandMeta { name: "!ban",      description: "Ban a member",                        feature: Some(Feature::Moderation), auth: AuthLevel::Owner },
    CommandMeta { name: "!unban",    description: "Lift a ban",                          feature: Some(Feature::Moderation), auth: AuthLevel::Owner },
    CommandMeta { name: "!warn",     description: "Warn a member",                       feature: Some(Feature::Moderation), auth: AuthLevel::Authorized },
    CommandMeta { name: "!warnings", description: "Show warning history",                feature: Some(Feature::Moderation), auth: AuthLevel::Authorized },
    CommandMeta { name: "!welcome", description: "Toggle welcome messages on/off",       feature: Some(Feature::Community), auth: AuthLevel::Owner },
    CommandMeta { name: "!grantmod", description: "Grant admin role",                    feature: Some(Feature::Moderation), auth: AuthLevel::Owner },
    CommandMeta { name: "!revokemod", description: "Revoke admin role",                  feature: Some(Feature::Moderation), auth: AuthLevel::Owner },

    // Community Engagement
    CommandMeta { name: "!level",     description: "Show your level and XP",               feature: Some(Feature::Community), auth: AuthLevel::Public },
    CommandMeta { name: "!rank",      description: "Show your level and XP",               feature: Some(Feature::Community), auth: AuthLevel::Public },
    CommandMeta { name: "!leaderboard", description: "Top 10 users by XP",                 feature: Some(Feature::Community), auth: AuthLevel::Public },
    CommandMeta { name: "!profile",   description: "Show user profile card",               feature: Some(Feature::Community), auth: AuthLevel::Public },
    CommandMeta { name: "!giveaway",  description: "Start a giveaway (Authorized+)",      feature: Some(Feature::Community), auth: AuthLevel::Authorized },
    CommandMeta { name: "!rep",       description: "Give reputation (+1)",                 feature: Some(Feature::Community), auth: AuthLevel::Public },

    // Git Monitor
    CommandMeta { name: "!git",      description: "Git repo monitor (add/list/remove/poll)",   feature: Some(Feature::GitMonitor), auth: AuthLevel::Public },

    // V2 Community Management
    CommandMeta { name: "!community", description: "v2 community management (create/info/leave/dissolve)", feature: Some(Feature::Community), auth: AuthLevel::Authorized },
    CommandMeta { name: "!invite",   description: "Create invite link or invite by npub",      feature: Some(Feature::Community), auth: AuthLevel::Authorized },
    CommandMeta { name: "!join",     description: "Join a community via invite link",           feature: Some(Feature::Community), auth: AuthLevel::Owner },
    CommandMeta { name: "!members",  description: "List community members",                     feature: Some(Feature::Community), auth: AuthLevel::Public },
    CommandMeta { name: "!channels", description: "List community channels",                    feature: Some(Feature::Community), auth: AuthLevel::Public },
    CommandMeta { name: "!roles",    description: "Show community roles",                       feature: Some(Feature::Community), auth: AuthLevel::Public },
    CommandMeta { name: "!caps",     description: "Show community capabilities",                 feature: Some(Feature::Community), auth: AuthLevel::Public },
];

// =============================================================================
// HELP TEXT — generated from COMMAND_REGISTRY, filtered by feature flags
// =============================================================================

fn help_text(features: &FeaturesSection) -> String {
    // Group commands by feature category.
    let mut sections: Vec<(&str, Vec<&CommandMeta>)> = vec![
        ("📋 General", COMMAND_REGISTRY.iter()
            .filter(|c| c.feature.is_none() && c.auth != AuthLevel::Owner)
            .collect()),
        ("🛠️ Utility", COMMAND_REGISTRY.iter()
            .filter(|c| c.feature == Some(Feature::Utility) && features.is_enabled(Feature::Utility))
            .collect()),
        ("🎮 Fun", COMMAND_REGISTRY.iter()
            .filter(|c| c.feature == Some(Feature::Fun) && features.is_enabled(Feature::Fun))
            .collect()),
        ("🌟 Community", COMMAND_REGISTRY.iter()
            .filter(|c| c.feature == Some(Feature::Community) && features.is_enabled(Feature::Community))
            .collect()),
        ("⚡ Nostr", COMMAND_REGISTRY.iter()
            .filter(|c| c.feature == Some(Feature::Nostr) && features.is_enabled(Feature::Nostr))
            .collect()),
        ("🤖 AI", COMMAND_REGISTRY.iter()
            .filter(|c| c.feature == Some(Feature::Ai) && features.is_enabled(Feature::Ai))
            .collect()),
        ("🛡️ Moderation", COMMAND_REGISTRY.iter()
            .filter(|c| c.feature == Some(Feature::Moderation) && features.is_enabled(Feature::Moderation))
            .collect()),
        ("📦 Git Monitor", COMMAND_REGISTRY.iter()
            .filter(|c| c.feature == Some(Feature::GitMonitor) && features.is_enabled(Feature::GitMonitor))
            .collect()),
        ("🔐 Owner", COMMAND_REGISTRY.iter()
            .filter(|c| c.feature.is_none() && c.auth == AuthLevel::Owner)
            .collect()),
    ];

    // Remove empty sections.
    sections.retain(|(_, cmds)| !cmds.is_empty());

    let mut parts = Vec::new();
    for (header, cmds) in &sections {
        let lines: Vec<String> = cmds.iter()
            .map(|c| format!("  {} — {}", c.name, c.description))
            .collect();
        parts.push(format!("{}\n{}", header, lines.join("\n")));
    }

    format!("Available commands:\n\n{}", parts.join("\n\n"))
}

// =============================================================================
// TESTS
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_help_text_contains_all_commands() {
        let help = help_text(&FeaturesSection::default());
        // General
        assert!(help.contains("!ping"));
        assert!(help.contains("!help"));
        assert!(help.contains("!stats"));
        // Utility
        assert!(help.contains("!price"));
        assert!(help.contains("!time"));
        assert!(help.contains("!roll"));
        assert!(help.contains("!weather"));
        // New utility commands
        assert!(help.contains("!remind"));
        assert!(help.contains("!poll"));
        assert!(help.contains("!translate"));
        assert!(help.contains("!define"));
        assert!(help.contains("!quote"));
        assert!(help.contains("!joke"));
        assert!(help.contains("!fact"));
        assert!(help.contains("!meme"));
        assert!(help.contains("!shorten"));
        assert!(help.contains("!delete"));
        // Fun
        assert!(help.contains("!8ball"));
        assert!(help.contains("!flip"));
        assert!(help.contains("!choose"));
        assert!(help.contains("!rps"));
        // Wallet
        assert!(help.contains("!balance"));
        assert!(help.contains("!tip"));
        assert!(help.contains("!deposit"));
        assert!(help.contains("!withdraw"));
        assert!(help.contains("!zap"));
        // Owner
        assert!(help.contains("!add"));
        assert!(help.contains("!remove"));
        assert!(help.contains("!list"));
    }

    #[test]
    fn test_help_text_contains_moderation_commands() {
        let help = help_text(&FeaturesSection::default());
        assert!(help.contains("!kick"));
        assert!(help.contains("!ban"));
        assert!(help.contains("!unban"));
        assert!(help.contains("!warn"));
        assert!(help.contains("!warnings"));
        assert!(help.contains("!grantmod"));
        assert!(help.contains("!grantmod"));
        assert!(help.contains("!revokemod"));
    }
}
