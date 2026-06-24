// =============================================================================
// lib/vector_client.rs — Wrapper around vector_sdk (STABLE — do not edit)
// =============================================================================
//
// Thin convenience wrappers around the vector_sdk API.
// Provides helper methods that reduce boilerplate in handlers.

use anyhow::Result;
use vector_sdk::{Channel, IncomingMessage, VectorBot};

/// Send a message to a specific channel by ID.
pub fn channel(bot: &VectorBot, channel_id: &str) -> Channel {
    bot.channel(channel_id.to_string())
}

/// Reply to a message with text.
pub async fn reply(msg: &IncomingMessage, text: &str) -> Result<()> {
    msg.reply(text).await?;
    Ok(())
}

/// React to a message with an emoji.
pub async fn react(msg: &IncomingMessage, emoji: &str) -> Result<()> {
    let channel = msg.channel();
    channel.react(&msg.message.id, emoji).await?;
    Ok(())
}

/// Send a typing indicator to a channel.
pub async fn typing(bot: &VectorBot, channel_id: &str) -> Result<()> {
    bot.channel(channel_id.to_string()).typing().await?;
    Ok(())
}

/// Send a file to a channel.
pub async fn send_file(bot: &VectorBot, channel_id: &str, file_path: &str) -> Result<()> {
    bot.channel(channel_id.to_string())
        .send_file(file_path)
        .await?;
    Ok(())
}
