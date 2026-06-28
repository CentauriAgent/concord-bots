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
use vector_sdk::{BotEvent, IncomingMessage};

use crate::auth::AuthLevel;
use crate::bot::BotContext;
use crate::config::{Feature, FeaturesSection};
use crate::handlers::fun;
use crate::handlers::utility;
use crate::handlers::wallet_cmds;
use crate::handlers::nostr_cmds;
use crate::handlers::moderation_cmds;
use crate::rate_limiter::RateLimitResult;

// -----------------------------------------------------------------------------
// Auth helpers
// -----------------------------------------------------------------------------

/// Extract the sender's npub from an incoming message.
fn sender_npub(msg: &IncomingMessage) -> String {
    msg.message.npub.clone().unwrap_or_default()
}

/// Check if the sender meets the required auth level.
async fn require_auth(ctx: &BotContext, msg: &IncomingMessage, level: AuthLevel) -> Result<bool> {
    let Some(ref auth) = ctx.auth else {
        return Ok(true); // Auth not configured — allow all
    };

    let npub = sender_npub(msg);
    if auth.has_permission(&npub, level) {
        return Ok(true);
    }

    let response = match level {
        AuthLevel::Owner => "⛔ Owner only.".to_string(),
        AuthLevel::Authorized => {
            "⛔ Not authorized. Ask the owner to run !add <your-npub>".to_string()
        }
        AuthLevel::Public => unreachable!("Public level should never be checked"),
    };
    msg.reply(&response).await?;
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
            msg.reply(&reason).await?;
            return Ok(());
        }
    }

    let features = &ctx.config.features;

    match command {
        // =====================================================================
        // CORE (always enabled)
        // =====================================================================

        "!ping" => {
            msg.reply("pong 🏓").await?;
        }

        "!repo" => {
            msg.reply("📦 concord-bots\nhttps://github.com/CentauriAgent/concord-bots\n\nClone it, build your own bot, join the fleet! 🚢").await?;
        }

        "!help" => {
            msg.reply(&help_text(&ctx.config.features)).await?;
        }

        "!echo" => {
            if args.is_empty() {
                msg.reply("Usage: !echo <text>").await?;
            } else {
                msg.reply(args).await?;
            }
        }

        "!whoami" => {
            let npub = ctx.bot.npub();
            let info = format!(
                "I am concord-bot 🤖\nnpub: {}\nFramework: concord-bots v{}",
                npub,
                env!("CARGO_PKG_VERSION")
            );
            msg.reply(&info).await?;
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

        "!mods"
            if features.moderation => {
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
                    msg.reply(&format!("Welcome messages are currently **{}**. Usage: !welcome on/off", state)).await?;
                    return Ok(());
                }
            };
            let state = if enabled { "ON ✅" } else { "OFF ❌" };
            msg.reply(&format!("Welcome messages turned {}", state)).await?;
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
        "!mods" => moderation_cmds::mods_command(ctx, msg, args).await?,
        "!grantmod" => moderation_cmds::grantmod_command(ctx, msg, args).await?,
        "!revokemod" => moderation_cmds::revokemod_command(ctx, msg, args).await?,
        _ => unreachable!("dispatch_moderation called with non-moderation command: {}", command),
    }
    Ok(())
}

// =============================================================================
// BUILT-IN AUTH COMMAND IMPLEMENTATIONS
// =============================================================================

async fn auth_command(ctx: &BotContext, msg: &IncomingMessage) -> Result<()> {
    let Some(ref auth) = ctx.auth else {
        msg.reply("Auth system is not configured. All commands are public.").await?;
        return Ok(());
    };

    let npub = sender_npub(msg);
    if npub.is_empty() {
        msg.reply("⚠️ Could not determine your npub from this message.").await?;
        return Ok(());
    }

    let level = auth.check(&npub);
    let status_text = match level {
        AuthLevel::Owner => format!("👑 You are the **owner**.\nnpub: {}", npub),
        AuthLevel::Authorized => format!("✅ You are **authorized**.\nnpub: {}", npub),
        AuthLevel::Public => format!(
            "❌ You are **not authorized**.\nnpub: {}\nAsk the owner to run: !add {}",
            npub, npub
        ),
    };

    msg.reply(&status_text).await?;
    Ok(())
}

async fn add_command(ctx: &BotContext, msg: &IncomingMessage, args: &str) -> Result<()> {
    let Some(ref auth) = ctx.auth else {
        msg.reply("Auth system is not configured.").await?;
        return Ok(());
    };

    let npub = args.trim();
    if npub.is_empty() {
        msg.reply("Usage: !add <npub>").await?;
        return Ok(());
    }

    if !npub.starts_with("npub1") {
        msg.reply("⚠️ That doesn't look like a valid npub. npubs start with \"npub1\".").await?;
        return Ok(());
    }

    if auth.is_owner(npub) {
        msg.reply("ℹ️ That npub is already the owner — no need to add.").await?;
        return Ok(());
    }

    if auth.is_authorized(npub) {
        msg.reply(&format!("ℹ️ {} is already authorized.", npub)).await?;
        return Ok(());
    }

    auth.add(npub);
    msg.reply(&format!("✅ Added {} to authorized users.", npub)).await?;
    tracing::info!("Authorized user added: {}", npub);
    Ok(())
}

async fn remove_command(ctx: &BotContext, msg: &IncomingMessage, args: &str) -> Result<()> {
    let Some(ref auth) = ctx.auth else {
        msg.reply("Auth system is not configured.").await?;
        return Ok(());
    };

    let npub = args.trim();
    if npub.is_empty() {
        msg.reply("Usage: !remove <npub>").await?;
        return Ok(());
    }

    if auth.is_owner(npub) {
        msg.reply("⚠️ Cannot remove the owner.").await?;
        return Ok(());
    }

    if !auth.is_authorized(npub) {
        msg.reply(&format!("ℹ️ {} is not in the authorized list.", npub)).await?;
        return Ok(());
    }

    auth.remove(npub);
    msg.reply(&format!("✅ Removed {} from authorized users.", npub)).await?;
    tracing::info!("Authorized user removed: {}", npub);
    Ok(())
}

async fn list_command(ctx: &BotContext, msg: &IncomingMessage) -> Result<()> {
    let Some(ref auth) = ctx.auth else {
        msg.reply("Auth system is not configured.").await?;
        return Ok(());
    };

    let owner = auth.owner();
    let authorized = auth.list();

    if authorized.is_empty() {
        msg.reply(&format!("Owner: {}\nAuthorized users: (none)", owner)).await?;
        return Ok(());
    }

    let body = authorized
        .iter()
        .map(|n| format!("  • {}", n))
        .collect::<Vec<_>>()
        .join("\n");

    msg.reply(&format!(
        "Owner: {}\nAuthorized users ({}):\n{}",
        owner,
        authorized.len(),
        body
    )).await?;
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
    CommandMeta { name: "!mods",     description: "List mods/admins",                    feature: Some(Feature::Moderation), auth: AuthLevel::Public },
    CommandMeta { name: "!welcome", description: "Toggle welcome messages on/off",       feature: Some(Feature::Community), auth: AuthLevel::Owner },
    CommandMeta { name: "!grantmod", description: "Grant admin role",                    feature: Some(Feature::Moderation), auth: AuthLevel::Owner },
    CommandMeta { name: "!revokemod", description: "Revoke admin role",                  feature: Some(Feature::Moderation), auth: AuthLevel::Owner },
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
        assert!(help.contains("!mods"));
        assert!(help.contains("!grantmod"));
        assert!(help.contains("!revokemod"));
    }
}
