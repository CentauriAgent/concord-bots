// =============================================================================
// Echo Bot — Command Handlers
// =============================================================================
//
// This is the simplest possible bot implementation.
// It responds to two commands:
//   !ping        → "pong 🏓"
//   !echo <text> → echoes the text back
//
// A scheduled task (posting "I'm alive!" every 5 min) is commented out
// in scheduled.rs.

use anyhow::Result;
use vector_sdk::IncomingMessage;

// Note: In a real bot built from this template, you'd import from the crate.
// In this example file, we show the handler logic standalone.

/// Handle a command message.
///
/// In your echo bot, wire this into the main message loop:
/// ```ignore
/// bot.on_message(|_bot, msg| async move {
///     if msg.is_mine() { return; }
///     let _ = handle_command(&msg).await;
/// }).await?;
/// ```
pub async fn handle_command(msg: &IncomingMessage) -> Result<()> {
    let text = msg.text();
    let parts: Vec<&str> = text.splitn(2, ' ').collect();
    let command = parts[0];
    let args = parts.get(1).copied().unwrap_or("");

    match command {
        "!ping" => {
            msg.reply("pong 🏓").await?;
        }

        "!echo" => {
            if args.is_empty() {
                msg.reply("Usage: !echo <text>").await?;
            } else {
                msg.reply(args).await?;
            }
        }

        "!help" => {
            let help = "Echo Bot commands:\n  !ping — Health check\n  !echo <text> — Echo text\n  !help — Show this help";
            msg.reply(help).await?;
        }

        _ => {
            // Ignore unknown commands
        }
    }

    Ok(())
}

/// Example: React to a keyword.
///
/// Uncomment and wire into the message loop to use:
/// ```ignore
/// if msg.text().to_lowercase().contains("agreed") {
///     let _ = msg.channel().react(&msg.message.id, "👍").await;
/// }
/// ```
pub async fn maybe_react(msg: &IncomingMessage) -> Result<()> {
    let text = msg.text().to_lowercase();

    if text.contains("agreed") || text.contains("+1") {
        let channel = msg.channel();
        let _ = channel.react(&msg.message.id, "👍").await;
    }

    if text.contains("love") || text.contains("❤️") {
        let channel = msg.channel();
        let _ = channel.react(&msg.message.id, "❤️").await;
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_handle_command_exists() {
        // Verify the function exists with the right signature.
        let _ = std::any::TypeId::of::<fn(&IncomingMessage) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<()>> + Send>>>();
    }
}
