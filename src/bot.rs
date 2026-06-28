// =============================================================================
// bot.rs — Vector connection and message loop
// =============================================================================
//
// Handles:
//   1. Building the VectorBot from config
//   2. Registering handlers (commands, scheduled tasks, AI bridge)
//   3. Running the bot until shutdown

use anyhow::{Context, Result};
use std::sync::Arc;
use vector_sdk::{BotEvent, VectorBot};

use crate::auth::AuthManager;
use crate::config::BotConfig;
use crate::handlers;
use crate::rate_limiter::RateLimiter;
use crate::wallet::CashuWallet;

/// Shared context passed to all handlers.
#[derive(Clone)]
pub struct BotContext {
    /// The Vector bot instance.
    pub bot: VectorBot,
    /// The parsed bot.toml configuration.
    pub config: Arc<BotConfig>,
    /// Authorization manager (None if auth is not configured).
    pub auth: Option<AuthManager>,
    /// Per-user spam protection.
    pub rate_limiter: RateLimiter,
    /// Cashu wallet (None if not configured).
    pub wallet: Option<Arc<CashuWallet>>,
}

/// Build the bot from config, register handlers, and run forever.
pub async fn run(config: BotConfig) -> Result<()> {
    tracing::info!("Starting concord-bots framework...");

    // -------------------------------------------------------------------------
    // Step 1: Build the VectorBot from config
    // -------------------------------------------------------------------------

    let mut builder = VectorBot::builder();

    // Resolve nsec: explicit config, persisted SDK file, or generate new.
    // The SDK persists auto-generated keys to ~/.local/share/io.vectorapp/sdk/identity.nsec
    let nsec = config.bot_nsec().or_else(|| {
        // Try SDK's default identity location
        if let Some(home) = std::env::var_os("HOME") {
            let sdk_path = std::path::Path::new(&home)
                .join(".local/share/io.vectorapp/sdk/identity.nsec");
            if let Ok(saved) = std::fs::read_to_string(&sdk_path) {
                let saved = saved.trim();
                if saved.starts_with("nsec1") {
                    tracing::info!("Using SDK-persisted nsec from {:?}", sdk_path);
                    return Some(saved.to_string());
                }
            }
        }
        None
    });

    let nsec = if let Some(ref n) = nsec {
        tracing::info!("Using provided nsec identity");
        builder = builder.nsec(n);
        Some(n.clone())
    } else {
        tracing::info!("No nsec provided — bot will auto-generate and persist an identity");
        None
    };

    match config.invite_policy() {
        crate::config::InvitePolicyConfig::Public => {
            tracing::info!("Invite policy: public (accept all invites)");
            builder = builder.public();
        }
        crate::config::InvitePolicyConfig::Whitelist(ref npubs) => {
            tracing::info!("Invite policy: whitelist ({} accounts)", npubs.len());
            builder = builder.whitelist(npubs.iter().map(|s| s.as_str()));
        }
        crate::config::InvitePolicyConfig::Manual => {
            tracing::info!("Invite policy: manual (invites require explicit acceptance)");
        }
    }

    let bot = builder
        .build()
        .await
        .context("Failed to build VectorBot — check your nsec and network connection")?;

    tracing::info!("Bot online as {}", bot.npub());

    // -------------------------------------------------------------------------
    // Step 2: Initialize auth system
    // -------------------------------------------------------------------------

    let auth = if let Some(ref owner) = config.auth.owner {
        if !owner.is_empty() {
            let state_file = std::path::PathBuf::from(&config.auth.state_file);
            match AuthManager::new(
                owner,
                &config.auth.authorized,
                config.auth.persist,
                state_file,
            ) {
                Ok(m) => {
                    tracing::info!(
                        "Auth system enabled — owner: {}, authorized users: {}",
                        owner,
                        m.authorized_count()
                    );
                    Some(m)
                }
                Err(e) => {
                    tracing::error!("Failed to initialize auth system: {}", e);
                    None
                }
            }
        } else {
            None
        }
    } else {
        tracing::info!("Auth system disabled (no owner npub configured — all commands are public)");
        None
    };

    // -------------------------------------------------------------------------
    // Step 2b: Initialize Cashu wallet (optional)
    // -------------------------------------------------------------------------

    let wallet = if config.wallet.enabled {
        let data_dir = std::path::PathBuf::from(
            std::env::var("WALLET_DATA_DIR").unwrap_or_else(|_| "./data".to_string())
        );
        match CashuWallet::init(&data_dir, &config.wallet.mint_url).await {
            Ok(w) => {
                tracing::info!("Cashu wallet initialized — mint: {}", config.wallet.mint_url);
                Some(Arc::new(w))
            }
            Err(e) => {
                tracing::error!("Failed to init Cashu wallet: {:?}", e);
                None
            }
        }
    } else {
        tracing::info!("Cashu wallet disabled (config [wallet] enabled = false)");
        None
    };

    // -------------------------------------------------------------------------
    // Step 3: Create shared context
    // -------------------------------------------------------------------------

    let ctx = BotContext {
        bot: bot.clone(),
        config: Arc::new(config),
        auth,
        rate_limiter: RateLimiter::default(),
        wallet,
    };

    // -------------------------------------------------------------------------
    // Step 4: Register all handlers
    // -------------------------------------------------------------------------

    handlers::register(&bot, ctx.clone()).await?;

    // -------------------------------------------------------------------------
    // Step 5: Event loop (handles BOTH messages AND member joins)
    // -------------------------------------------------------------------------
    // Use on_event — it's a superset of on_message. Messages arrive as
    // BotEvent::Message(IncomingMessage), joins as BotEvent::MemberJoin, etc.
    // We must NOT also register on_message — both call core.listen() and only
    // the first registration runs (see commit c51e4b3).

    bot.on_event({
        let ctx = ctx.clone();
        move |_bot, event| {
            let ctx = ctx.clone();
            async move {
                match event {
                    BotEvent::Message(msg) => {
                        // Don't process our own messages.
                        if msg.is_mine() {
                            return;
                        }

                        tracing::info!(
                            "Incoming message from {}: {}",
                            msg.chat_id,
                            msg.text()
                        );

                        if let Err(e) = handlers::on_message(&ctx, &msg).await {
                            tracing::error!("Handler error: {:?}", e);
                        }
                    }

                    // All non-message events
                    _ => {
                        if let Err(e) = handlers::on_event(&ctx, event).await {
                            tracing::error!("Event handler error: {:?}", e);
                        }
                    }
                }
            }
        }
    })
    .await
    .context("Failed to register on_event handler")?;

    tracing::info!("Bot is running. Press Ctrl+C to stop.");

    tokio::signal::ctrl_c()
        .await
        .context("Failed to listen for Ctrl+C")?;

    tracing::info!("Shutdown signal received. Goodbye!");
    Ok(())
}
