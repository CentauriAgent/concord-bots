// =============================================================================
// handlers/commands.rs — Command handlers (AGENT EXTENSION POINT)
// =============================================================================
//
// This is where you add !command handlers for your bot.
//
// ============================================================================
// HOW TO ADD A NEW COMMAND
// ============================================================================
//
// 1. Add a match arm in `on_message()` below:
//
//    "!mycommand" => {
//        my_command(ctx, msg, &args).await?;
//    }
//
// 2. (Optional) Gate it with an auth level:
//
//    "!admin" => {
//        if !require_auth(ctx, msg, AuthLevel::Owner).await? { return Ok(()); }
//        admin_command(ctx, msg, &args).await?;
//    }
//
// 3. Write the handler function:
//
//    async fn my_command(
//        ctx: &BotContext,
//        msg: &IncomingMessage,
//        args: &str,
//    ) -> Result<()> {
//        msg.reply("Hello!").await?;
//        Ok(())
//    }
//
// ============================================================================
// AUTHORIZATION SYSTEM
// ============================================================================
//
// When `[auth]` is configured in bot.toml with an `owner` npub, commands can
// be gated by permission level:
//
//   AuthLevel::Public     — Anyone can use (e.g., !ping, !price)
//   AuthLevel::Authorized — Owner + users added via !add (e.g., !status)
//   AuthLevel::Owner      — Only the configured owner (e.g., !add, !shutdown)
//
// Use the `require_auth()` helper to check permissions before running a command.
// If auth is not configured, all checks pass (backward-compatible).
//
// ============================================================================

use anyhow::Result;
use vector_sdk::{BotEvent, IncomingMessage};

use crate::auth::AuthLevel;
use crate::bot::BotContext;

// -----------------------------------------------------------------------------
// Auth helpers
// -----------------------------------------------------------------------------

/// Extract the sender's npub from an incoming message.
fn sender_npub(msg: &IncomingMessage) -> String {
    msg.message.npub.clone().unwrap_or_default()
}

/// Check if the sender meets the required auth level.
///
/// If the sender doesn't have permission, sends a denial message and
/// returns `Ok(false)`. If auth is not configured, always returns `Ok(true)`.
///
/// # Example
/// ```ignore
/// "!status" => {
///     if !require_auth(ctx, msg, AuthLevel::Authorized).await? { return Ok(()); }
///     // ... handler logic ...
/// }
/// ```
async fn require_auth(ctx: &BotContext, msg: &IncomingMessage, level: AuthLevel) -> Result<bool> {
    let Some(ref auth) = ctx.auth else {
        return Ok(true); // Auth not configured — allow all
    };

    let npub = sender_npub(msg);
    if auth.has_permission(&npub, level) {
        return Ok(true);
    }

    // Send denial message based on required level.
    let response = match level {
        AuthLevel::Owner => "⛔ Owner only.",
        AuthLevel::Authorized => {
            "⛔ Not authorized. Ask the owner to run !add <your-npub>"
        }
        AuthLevel::Public => unreachable!("Public level should never be checked"),
    };
    msg.reply(response).await?;
    Ok(false)
}

// -----------------------------------------------------------------------------
// Main command dispatcher
// -----------------------------------------------------------------------------

/// Main command dispatcher.
///
/// Called for every message starting with "!". Parses the command name
/// and dispatches to the appropriate handler function.
pub async fn on_message(ctx: &BotContext, msg: &IncomingMessage) -> Result<()> {
    let text = msg.text();
    let parts: Vec<&str> = text.splitn(2, ' ').collect();
    let command = parts[0];
    let args = parts.get(1).copied().unwrap_or("");

    match command {
            // -----------------------------------------------------------------
        // BUILT-IN COMMANDS (Public)
        // -----------------------------------------------------------------

        // `!ping` — Health check. Replies with "pong 🏓".
        // Auth: Public — anyone can use.
        "!ping" => {
            msg.reply("pong 🏓").await?;
        }

        // `!help` — List available commands.
        // Auth: Public — anyone can use.
        "!help" => {
            msg.reply(&help_text()).await?;
        }

        // `!echo <text>` — Echo back the provided text.
        // Auth: Public — anyone can use.
        "!echo" => {
            if args.is_empty() {
                msg.reply("Usage: !echo <text>").await?;
            } else {
                msg.reply(args).await?;
            }
        }

        // `!whoami` — Show the bot's npub and identity info.
        // Auth: Public — anyone can use.
        "!whoami" => {
            let npub = ctx.bot.npub();
            let info = format!(
                "I am concord-bot 🤖\nnpub: {}\nFramework: concord-bots v{}",
                npub,
                env!("CARGO_PKG_VERSION")
            );
            msg.reply(&info).await?;
        }

        // `!auth` — Show your authorization status.
        // Auth: Public — anyone can use (to check their own status).
        "!auth" => {
            auth_command(ctx, msg).await?;
        }

        // =====================================================================
        // BUILT-IN COMMANDS (Owner only)
        // =====================================================================

        // `!add <npub>` — Add an npub to the authorized users list.
        // Auth: Owner only.
        "!add" => {
            if !require_auth(ctx, msg, AuthLevel::Owner).await? {
                return Ok(());
            }
            add_command(ctx, msg, args).await?;
        }

        // `!remove <npub>` — Remove an npub from the authorized users list.
        // Auth: Owner only.
        "!remove" => {
            if !require_auth(ctx, msg, AuthLevel::Owner).await? {
                return Ok(());
            }
            remove_command(ctx, msg, args).await?;
        }

        // `!list` — List all authorized users.
        // Auth: Owner only.
        "!list" => {
            if !require_auth(ctx, msg, AuthLevel::Owner).await? {
                return Ok(());
            }
            list_command(ctx, msg).await?;
        }

        // =====================================================================
        // ADD YOUR CUSTOM COMMANDS BELOW
        // ====================================================================

        // Example: Bitcoin price command (Public)
        //
        // "!price" => {
        //     price_command(ctx, msg).await?;
        // }

        // Example: Status command (Authorized only)
        //
        // "!status" => {
        //     if !require_auth(ctx, msg, AuthLevel::Authorized).await? {
        //         return Ok(());
        //     }
        //     status_command(ctx, msg).await?;
        // }

        // Example: Shutdown command (Owner only)
        //
        // "!shutdown" => {
        //     if !require_auth(ctx, msg, AuthLevel::Owner).await? {
        //         return Ok(());
        //     }
        //     msg.reply("Shutting down...").await?;
        //     ctx.bot.shutdown().await;  // hypothetical
        // }

        // =====================================================================
        // UNKNOWN COMMAND
        // =====================================================================

        _ => {
            // Silently ignore unknown commands, or uncomment to notify:
            // msg.reply(&format!("Unknown command: {}. Try !help", command)).await?;
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

/// `!auth` — Shows the sender's authorization status.
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
        AuthLevel::Owner => {
            format!("👑 You are the **owner**.\nnpub: {}", npub)
        }
        AuthLevel::Authorized => {
            format!("✅ You are **authorized**.\nnpub: {}", npub)
        }
        AuthLevel::Public => {
            format!(
                "❌ You are **not authorized**.\nnpub: {}\nAsk the owner to run: !add {}",
                npub, npub
            )
        }
    };

    msg.reply(&status_text).await?;
    Ok(())
}

/// `!add <npub>` — Add an npub to the authorized users list. Owner only.
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

    // Basic validation — npubs start with "npub1".
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

/// `!remove <npub>` — Remove an npub from authorized users. Owner only.
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

/// `!list` — List all authorized users. Owner only.
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

/// Generate the !help text listing all commands.
fn help_text() -> String {
    let commands = vec![
        ("!ping", "Health check (replies with pong)"),
        ("!help", "Show this help message"),
        ("!echo <text>", "Echo back the provided text"),
        ("!whoami", "Show bot identity info"),
        ("!auth", "Show your authorization status"),
        // Auth management commands (shown with permission notes)
        ("!add <npub>", "⚠️ Owner: authorize a user"),
        ("!remove <npub>", "⚠️ Owner: deauthorize a user"),
        ("!list", "⚠️ Owner: list authorized users"),
    ];

    // TODO: Add your custom commands to this list when you add them above.
    // commands.push(("!price", "Show current Bitcoin price"));

    let body = commands
        .iter()
        .map(|(cmd, desc)| format!("  {} — {}", cmd, desc))
        .collect::<Vec<_>>()
        .join("\n");

    format!("Available commands:\n{}", body)
}

// =============================================================================
// EXAMPLE COMMAND IMPLEMENTATIONS
// =============================================================================
// Uncomment and adapt these for your use case.
// =============================================================================

// /// Fetch Bitcoin price from CoinGecko API. (Public — anyone can use)
// async fn price_command(ctx: &BotContext, msg: &IncomingMessage) -> Result<()> {
//     let data = crate::lib::http::fetch_json(
//         "https://api.coingecko.com/api/v3/simple/price?ids=bitcoin&vs_currencies=usd"
//     ).await?;
//
//     let price = data["bitcoin"]["usd"]
//         .as_f64()
//         .map(|p| format!("${:.2}", p))
//         .unwrap_or_else(|| "unavailable".to_string());
//
//     msg.reply(&format!("₿ Bitcoin: {}", price)).await?;
//     Ok(())
// }

// /// Show bot uptime and stats. (Authorized only — requires !add)
// async fn status_command(ctx: &BotContext, msg: &IncomingMessage) -> Result<()> {
//     let npub = ctx.bot.npub();
//     msg.reply(&format!("Bot {} is running.", npub)).await?;
//     Ok(())
// }

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_help_text_contains_builtins() {
        let help = help_text();
        assert!(help.contains("!ping"));
        assert!(help.contains("!help"));
        assert!(help.contains("!echo"));
        assert!(help.contains("!whoami"));
        assert!(help.contains("!auth"));
        assert!(help.contains("!add"));
        assert!(help.contains("!remove"));
        assert!(help.contains("!list"));
    }

    #[test]
    fn test_auth_level_imports() {
        // Verify AuthLevel is accessible.
        let _ = AuthLevel::Public;
        let _ = AuthLevel::Authorized;
        let _ = AuthLevel::Owner;
    }
}
