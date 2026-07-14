// =============================================================================
// handlers/ai_bridge.rs — OpenClaw AI integration (OPTIONAL EXTENSION POINT)
// =============================================================================
//
// This module integrates the bot with an AI backend (OpenClaw, OpenAI, etc.)
// to provide intelligent responses to messages.
//
// ============================================================================
// HOW IT WORKS
// ============================================================================
//
// When enabled, non-command messages are passed to the AI handler.
// The AI generates a response, which is sent back as a reply.
//
// Enable by setting in bot.toml:
//
//   [custom]
//   [custom.ai]
//   enabled = true
//   # provider = "openclaw"   # or "openai"
//   # model = "gpt-4o-mini"
//   # api_key = "sk-..."      # or set AI_API_KEY env var
//   # system_prompt = "You are a helpful assistant in a Vector community."
//
// ============================================================================

use anyhow::Result;
use vector_sdk::{BotEvent, IncomingMessage, VectorBot};

use crate::bot::BotContext;

/// Check if AI bridge is enabled in config.
pub fn is_enabled(ctx: &BotContext) -> bool {
    ctx.config.custom.as_ref()
        .and_then(|v| v.as_table())
        .and_then(|t| t.get("ai"))
        .and_then(|v| v.as_table())
        .and_then(|t| t.get("enabled"))
        .and_then(|v| v.as_bool())
        .unwrap_or(false)
}

/// Register the AI bridge (called at startup).
pub async fn register(_bot: &VectorBot, _ctx: BotContext) -> Result<()> {
    if is_enabled(&_ctx) {
        tracing::info!("AI bridge: enabled");
        // Any initialization (e.g., warm up the model) goes here.
    } else {
        tracing::info!("AI bridge: disabled (enable in bot.toml under [custom.ai])");
    }
    Ok(())
}

/// Handle an incoming message with AI (called for non-command messages).
pub async fn on_message(ctx: &BotContext, msg: &IncomingMessage) -> Result<()> {
    if !is_enabled(ctx) {
        return Ok(());
    }

    // Get the AI configuration.
    let ai_config = ctx.config.custom.as_ref()
        .and_then(|v| v.as_table())
        .and_then(|t| t.get("ai"))
        .and_then(|v| v.as_table());

    let system_prompt = ai_config
        .and_then(|t| t.get("system_prompt"))
        .and_then(|v| v.as_str())
        .unwrap_or("You are a helpful Vector bot. Keep responses concise.");

    let provider = ai_config
        .and_then(|t| t.get("provider"))
        .and_then(|v| v.as_str())
        .unwrap_or("openclaw");

    let model = ai_config
        .and_then(|t| t.get("model"))
        .and_then(|v| v.as_str())
        .unwrap_or("default");

    let api_key = ai_config
        .and_then(|t| t.get("api_key"))
        .and_then(|v| v.as_str())
        .map(|s| s.to_string())
        .or_else(|| std::env::var("AI_API_KEY").ok())
        .or_else(|| std::env::var("OPENAI_API_KEY").ok());

    // Show typing indicator while processing.
    let channel = msg.channel();
    let _ = channel.typing().await;

    // Generate response via the configured provider.
    let user_message = msg.text();
    let response = match provider {
        "openai" => generate_openai(&user_message, system_prompt, model, api_key.as_deref()).await?,
        _ => generate_openclaw(&user_message, system_prompt).await?,
    };

    // Send the AI response as a reply.
    super::reply(ctx, msg, &response).await?;

    Ok(())
}

/// Handle events (unused for AI bridge, but available if needed).
pub async fn on_event(_ctx: &BotContext, _event: &BotEvent) -> Result<()> {
    Ok(())
}

// =============================================================================
// AI PROVIDER IMPLEMENTATIONS
// =============================================================================

/// Generate a response using the OpenClaw CLI.
///
/// This shells out to `openclaw` if available, or falls back to a simple echo.
async fn generate_openclaw(message: &str, system_prompt: &str) -> Result<String> {
    // Try calling the openclaw CLI.
    let output = tokio::process::Command::new("openclaw")
        .args(["chat", "--system", system_prompt, "--message", message])
        .output()
        .await;

    match output {
        Ok(result) if result.status.success() => {
            let response = String::from_utf8_lossy(&result.stdout).trim().to_string();
            Ok(response)
        }
        _ => {
            tracing::warn!("OpenClaw CLI not available, using fallback response");
            Ok(format!("I received: \"{}\" — but my AI backend is not configured.", message))
        }
    }
}

/// Generate a response using the OpenAI Chat Completions API.
async fn generate_openai(
    message: &str,
    system_prompt: &str,
    model: &str,
    api_key: Option<&str>,
) -> Result<String> {
    let api_key = api_key.ok_or_else(|| {
        anyhow::anyhow!("OpenAI API key not configured (set ai.api_key or AI_API_KEY)")
    })?;

    let body = serde_json::json!({
        "model": model,
        "messages": [
            { "role": "system", "content": system_prompt },
            { "role": "user", "content": message }
        ],
        "max_tokens": 500,
    });

    let client = reqwest::Client::new();
    let resp = client
        .post("https://api.openai.com/v1/chat/completions")
        .header("Authorization", format!("Bearer {}", api_key))
        .json(&body)
        .send()
        .await?;

    let data: serde_json::Value = resp.json().await?;
    let response = data["choices"][0]["message"]["content"]
        .as_str()
        .unwrap_or("Error: Could not parse AI response")
        .trim()
        .to_string();

    Ok(response)
}

#[cfg(test)]
mod tests {
    use crate::config::BotConfig;

    #[test]
    fn test_default_config_has_no_custom() {
        let config = BotConfig::default();
        assert!(config.custom.is_none());
    }
}
