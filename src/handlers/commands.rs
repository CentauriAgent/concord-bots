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
use crate::handlers::fun;
use crate::handlers::utility;
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

    match command {
        // =====================================================================
        // BUILT-IN COMMANDS (Public)
        // =====================================================================

        "!ping" => {
            msg.reply("pong 🏓").await?;
        }

        "!help" => {
            msg.reply(&help_text()).await?;
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
        // UTILITY COMMANDS (Public)
        // =====================================================================

        "!price" => {
            utility::price_command(ctx, msg).await?;
        }

        "!time" => {
            utility::time_command(ctx, msg, args).await?;
        }

        "!roll" => {
            utility::roll_command(ctx, msg, args).await?;
        }

        "!stats" => {
            utility::stats_command(ctx, msg).await?;
        }

        "!weather" => {
            utility::weather_command(ctx, msg, args).await?;
        }

        "!remind" => {
            utility::remind_command(ctx, msg, args).await?;
        }

        "!poll" => {
            utility::poll_command(ctx, msg, args).await?;
        }

        "!translate" => {
            utility::translate_command(ctx, msg, args).await?;
        }

        "!define" => {
            utility::define_command(ctx, msg, args).await?;
        }

        "!quote" => {
            utility::quote_command(ctx, msg).await?;
        }

        "!joke" => {
            utility::joke_command(ctx, msg).await?;
        }

        "!fact" => {
            utility::fact_command(ctx, msg).await?;
        }

        "!meme" => {
            utility::meme_command(ctx, msg).await?;
        }

        "!shorten" => {
            utility::shorten_command(ctx, msg, args).await?;
        }

        // =====================================================================
        // FUN COMMANDS (Public)
        // =====================================================================

        "!8ball" => {
            fun::eight_ball_command(ctx, msg, args).await?;
        }

        "!flip" => {
            fun::flip_command(ctx, msg).await?;
        }

        "!choose" => {
            fun::choose_command(ctx, msg, args).await?;
        }

        "!rps" => {
            fun::rps_command(ctx, msg, args).await?;
        }

        // =====================================================================
        // AUTH MANAGEMENT (Owner only)
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
// HELP TEXT
// =============================================================================

fn help_text() -> String {
    let groups = [
        ("📋 General", vec![
            ("!ping", "Health check"),
            ("!help", "Show this help"),
            ("!whoami", "Bot identity"),
            ("!auth", "Your auth status"),
            ("!stats", "Bot statistics"),
        ]),
        ("🛠️ Utility", vec![
            ("!price", "Bitcoin price (USD)"),
            ("!time [tz]", "Current time (e.g. !time US/Eastern)"),
            ("!roll [NdS]", "Dice — !roll, !roll 20, !roll 3d6"),
            ("!weather <zip>", "Weather for a US zipcode"),
            ("!remind <time> <msg>", "Set a reminder (30m, 2h, 1d)"),
            ("!poll <q> | a | b", "Create a poll"),
            ("!translate <lang> <text>", "Translate text (e.g. !translate es Hi)"),
            ("!define <word>", "Dictionary definition"),
            ("!quote", "Random inspirational quote"),
            ("!joke", "Random dad joke"),
            ("!fact", "Random fun fact"),
            ("!meme", "Random meme"),
            ("!shorten <url>", "Shorten a URL"),
        ]),
        ("🎮 Fun", vec![
            ("!8ball <q>", "Magic 8-ball"),
            ("!flip", "Flip a coin"),
            ("!choose <a|b|c>", "Pick randomly"),
            ("!rps <r|p|s>", "Rock paper scissors"),
        ]),
        ("🔐 Owner", vec![
            ("!add <npub>", "Authorize a user"),
            ("!remove <npub>", "Deauthorize a user"),
            ("!list", "List authorized users"),
        ]),
    ];

    let mut sections = Vec::new();
    for (header, cmds) in &groups {
        let lines: Vec<String> = cmds
            .iter()
            .map(|(cmd, desc)| format!("  {} — {}", cmd, desc))
            .collect();
        sections.push(format!("{}\n{}", header, lines.join("\n")));
    }

    format!("Available commands:\n\n{}", sections.join("\n\n"))
}

// =============================================================================
// TESTS
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_help_text_contains_all_commands() {
        let help = help_text();
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
        // Owner
        assert!(help.contains("!add"));
        assert!(help.contains("!remove"));
        assert!(help.contains("!list"));
    }
}
