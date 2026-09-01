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

    let cfg = ai_config(ctx);

    // Show typing indicator while processing.
    let channel = msg.channel();
    let _ = channel.typing().await;

    // Generate response via the configured provider.
    let user_message = msg.text();
    let response = generate(user_message, &cfg).await?;

    // Send the AI response as a reply.
    super::reply(ctx, msg, &response).await?;

    Ok(())
}

/// Handle the `!ask` command: generate an AI response to the given question.
pub async fn ask(ctx: &BotContext, msg: &IncomingMessage, args: &str) -> Result<()> {
    let question = args.trim();
    if question.is_empty() {
        super::reply(ctx, msg, "Usage: !ask <question>").await?;
        return Ok(());
    }

    let cfg = ai_config(ctx);

    // Show typing indicator while processing.
    let channel = msg.channel();
    let _ = channel.typing().await;

    let response = generate(question, &cfg).await?;
    super::reply(ctx, msg, &response).await?;

    Ok(())
}

/// Handle the `!summarize` command: summarize the given text via the AI provider.
///
/// Scope is intentionally minimal: prompt-based summary only. The args are the
/// text to summarize; reply-target / channel-history resolution is out of scope
/// (no channel-history API access assumed).
pub async fn summarize(ctx: &BotContext, msg: &IncomingMessage, args: &str) -> Result<()> {
    let text = get_recent_or_reply(args);
    if text.is_empty() {
        super::reply(ctx, msg, "Usage: !summarize <text>").await?;
        return Ok(());
    }

    let cfg = ai_config(ctx);

    // Show typing indicator while processing.
    let channel = msg.channel();
    let _ = channel.typing().await;

    let prompt = format!("Summarize this: {}", text);
    let response = generate(&prompt, &cfg).await?;
    super::reply(ctx, msg, &response).await?;

    Ok(())
}

/// Handle the `!sentiment` command: analyze the sentiment of the given text.
pub async fn sentiment(ctx: &BotContext, msg: &IncomingMessage, args: &str) -> Result<()> {
    let text = args.trim();
    if text.is_empty() {
        super::reply(ctx, msg, "Usage: !sentiment <text>").await?;
        return Ok(());
    }

    let cfg = ai_config(ctx);

    // Show typing indicator while processing.
    let channel = msg.channel();
    let _ = channel.typing().await;

    let prompt = format!("Analyze the sentiment of this: {}", text);
    let response = generate(&prompt, &cfg).await?;
    super::reply(ctx, msg, &response).await?;

    Ok(())
}

/// Handle the `!image` command: generate an image via the OpenAI Images API.
///
/// OpenAI-compatible providers only (provider = "openai"). The openclaw CLI
/// text path cannot generate images, and that is stated plainly to the user.
/// Replies with the image URL from `data[0].url` as text (the reply helper has
/// no attachment helper; URL-as-text is the established pattern here).
pub async fn image(ctx: &BotContext, msg: &IncomingMessage, args: &str) -> Result<()> {
    let prompt = args.trim();
    if prompt.is_empty() {
        super::reply(ctx, msg, "Usage: !image <prompt>").await?;
        return Ok(());
    }

    let cfg = ai_config(ctx);

    if cfg.provider != "openai" {
        super::reply(
            ctx,
            msg,
            "⚠️ Image generation requires `provider = \"openai\"` in [custom.ai]. \
             The openclaw CLI text path cannot generate images.",
        )
        .await?;
        return Ok(());
    }

    // Show typing indicator while processing.
    let channel = msg.channel();
    let _ = channel.typing().await;

    let url = generate_image_openai(prompt, &cfg).await?;
    super::reply(ctx, msg, &format!("🖼️ {}", url)).await?;

    Ok(())
}

/// Resolve the source text for `!summarize`.
///
/// Currently args-only: reply-target and channel-ref resolution would need
/// channel-history access, which is out of scope. Returns the trimmed args.
fn get_recent_or_reply(args: &str) -> String {
    args.trim().to_string()
}

// =============================================================================
// AI CONFIG EXTRACTION (shared by on_message and !ask)
// =============================================================================

/// AI configuration pulled from `[custom.ai]` with defaults applied.
struct AiConfig {
    provider: String,
    model: String,
    system_prompt: String,
    api_key: Option<String>,
}

/// Extract the AI configuration from the bot config (provider/model/prompt/key).
fn ai_config(ctx: &BotContext) -> AiConfig {
    let ai_table = ctx.config.custom.as_ref()
        .and_then(|v| v.as_table())
        .and_then(|t| t.get("ai"))
        .and_then(|v| v.as_table());

    let system_prompt = ai_table
        .and_then(|t| t.get("system_prompt"))
        .and_then(|v| v.as_str())
        .unwrap_or("You are a helpful Vector bot. Keep responses concise.")
        .to_string();

    let provider = ai_table
        .and_then(|t| t.get("provider"))
        .and_then(|v| v.as_str())
        .unwrap_or("openclaw")
        .to_string();

    let model = ai_table
        .and_then(|t| t.get("model"))
        .and_then(|v| v.as_str())
        .unwrap_or("default")
        .to_string();

    let api_key = ai_table
        .and_then(|t| t.get("api_key"))
        .and_then(|v| v.as_str())
        .map(|s| s.to_string())
        .or_else(|| std::env::var("AI_API_KEY").ok())
        .or_else(|| std::env::var("OPENAI_API_KEY").ok());

    AiConfig { provider, model, system_prompt, api_key }
}

/// Dispatch to the configured provider (openclaw by default, openai otherwise).
async fn generate(message: &str, cfg: &AiConfig) -> Result<String> {
    match cfg.provider.as_str() {
        "openai" => generate_openai(message, &cfg.system_prompt, &cfg.model, cfg.api_key.as_deref()).await,
        _ => generate_openclaw(message, &cfg.system_prompt).await,
    }
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

/// Generate an image via the OpenAI Images API (`/v1/images/generations`) and
/// return the URL from `data[0].url`.
async fn generate_image_openai(prompt: &str, cfg: &AiConfig) -> Result<String> {
    let api_key = cfg.api_key.as_deref().ok_or_else(|| {
        anyhow::anyhow!("OpenAI API key not configured (set ai.api_key or AI_API_KEY)")
    })?;

    // Use the configured model if it names an image model, else default to
    // dall-e-3; a chat model name would 400, so fall back unless it looks like
    // an image model.
    let model = if cfg.model.contains("dall-e") || cfg.model.contains("image") {
        cfg.model.clone()
    } else {
        "dall-e-3".to_string()
    };

    let body = serde_json::json!({
        "model": model,
        "prompt": prompt,
        "n": 1,
        "size": "1024x1024",
    });

    let client = reqwest::Client::new();
    let resp = client
        .post("https://api.openai.com/v1/images/generations")
        .header("Authorization", format!("Bearer {}", api_key))
        .json(&body)
        .send()
        .await?;

    let status = resp.status();
    let data: serde_json::Value = resp.json().await?;

    let url = data["data"][0]["url"].as_str().ok_or_else(|| {
        anyhow::anyhow!(
            "OpenAI Images API error ({}): {}",
            status,
            data["error"]["message"].as_str().unwrap_or("no URL in response")
        )
    })?;

    Ok(url.to_string())
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
