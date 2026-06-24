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
// 2. Write the handler function:
//
//    async fn my_command(
//        ctx: &BotContext,
//        msg: &IncomingMessage,
//        args: &str,
//    ) -> Result<()> {
//        // Your logic here
//        msg.reply("Hello!").await?;
//        Ok(())
//    }
//
// 3. Use the HTTP helper for external APIs:
//
//    let data = lib::http::fetch_json("https://api.example.com/data").await?;
//    let price = data["price"].as_str().unwrap_or("unknown");
//    msg.reply(&format!("Price: {}", price)).await?;
//
// 4. Use the bot to send to other channels:
//
//    let channel = ctx.bot.channel(channel_id);
//    channel.send("Hello from another channel!").await?;
//
// ============================================================================
// AVAILABLE UTILITIES
// ============================================================================
//
// - msg.reply("text")        → Reply in the same channel/DM
// - msg.text()               → Get the message text
// - msg.author_npub()        → Get the sender's npub
// - msg.is_mine()            → Check if we sent this message
// - msg.channel()            → Get a Channel handle for this conversation
// - msg.member()             → Get a Member handle (community channels only)
// - ctx.bot.channel(id)      → Open any channel by ID
// - ctx.bot.npub()           → Get the bot's own npub
// - lib::http::fetch_json()  → HTTP GET that returns serde_json::Value
// - lib::http::post_json()   → HTTP POST with JSON body
// - ctx.config.custom_string("key") → Custom config values from bot.toml
//
// ============================================================================

use anyhow::Result;
use vector_sdk::{BotEvent, IncomingMessage};

use crate::bot::BotContext;

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
        // =====================================================================
        // BUILT-IN COMMANDS
        // =====================================================================

        /// `!ping` — Health check. Replies with "pong 🏓".
        "!ping" => {
            msg.reply("pong 🏓").await?;
        }

        /// `!help` — List available commands.
        "!help" => {
            msg.reply(&help_text()).await?;
        }

        /// `!echo <text>` — Echo back the provided text.
        "!echo" => {
            if args.is_empty() {
                msg.reply("Usage: !echo <text>").await?;
            } else {
                msg.reply(args).await?;
            }
        }

        /// `!whoami` — Show the bot's npub and identity info.
        "!whoami" => {
            let npub = ctx.bot.npub();
            let info = format!(
                "I am concord-bot 🤖\nnpub: {}\nFramework: concord-bots v{}",
                npub,
                env!("CARGO_PKG_VERSION")
            );
            msg.reply(&info).await?;
        }

        // =====================================================================
        // ADD YOUR CUSTOM COMMANDS BELOW
        // =====================================================================

        // Example: Bitcoin price command (uncomment to use)
        //
        // "!price" => {
        //     price_command(ctx, msg).await?;
        // }
        //
        // Example: GitHub issue lookup
        //
        // "!issue" => {
        //     issue_command(ctx, msg, args).await?;
        // }
        //
        // Example: Weather lookup
        //
        // "!weather" => {
        //     weather_command(ctx, msg, args).await?;
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

/// Handle non-message events for commands (typically unused, but available
/// if you need to react to reactions, joins, etc. in the context of commands).
pub async fn on_event(_ctx: &BotContext, _event: &BotEvent) -> Result<()> {
    Ok(())
}

/// Generate the !help text listing all commands.
fn help_text() -> String {
    let commands = vec![
        ("!ping", "Health check (replies with pong)"),
        ("!help", "Show this help message"),
        ("!echo <text>", "Echo back the provided text"),
        ("!whoami", "Show bot identity info"),
    ];

    // TODO: Add your custom commands to this list when you add them above.
    // commands.push(("!price", "Show current Bitcoin price"));
    // commands.push(("!weather <city>", "Show weather for a city"));

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

// /// Fetch Bitcoin price from CoinGecko API.
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

// /// Look up a GitHub issue by number.
// async fn issue_command(
//     ctx: &BotContext,
//     msg: &IncomingMessage,
//     args: &str,
// ) -> Result<()> {
//     if args.is_empty() {
//         msg.reply("Usage: !issue <number>").await?;
//         return Ok(());
//     }
//
//     let repo = ctx.config.custom_string("github.repo")
//         .unwrap_or_else(|| "your-org/your-repo".to_string());
//     let token = ctx.config.custom_string("github.token");
//
//     let url = format!("https://api.github.com/repos/{}/issues/{}", repo, args);
//     let data = crate::lib::http::fetch_json_with_auth(&url, token.as_deref()).await?;
//
//     let title = data["title"].as_str().unwrap_or("Untitled");
//     let state = data["state"].as_str().unwrap_or("unknown");
//     let html_url = data["html_url"].as_str().unwrap_or("");
//
//     msg.reply(&format!("#{} {} [{}]\n{}", args, title, state, html_url)).await?;
//     Ok(())
// }

// /// Fetch weather for a city using wttr.in.
// async fn weather_command(
//     ctx: &BotContext,
//     msg: &IncomingMessage,
//     args: &str,
// ) -> Result<()> {
//     if args.is_empty() {
//         msg.reply("Usage: !weather <city>").await?;
//         return Ok(());
//     }
//
//     let city = args.trim();
//     let url = format!("https://wttr.in/{}?format=j1", city);
//     let data = crate::lib::http::fetch_json(&url).await?;
//
//     let temp = data["current_condition"][0]["temp_C"]
//         .as_str()
//         .unwrap_or("N/A");
//     let desc = data["current_condition"][0]["weatherDesc"][0]["value"]
//         .as_str()
//         .unwrap_or("Unknown");
//
//     msg.reply(&format!("{}: {}°C, {}", city, temp, desc)).await?;
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
    }
}
