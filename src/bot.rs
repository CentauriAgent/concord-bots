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
use crate::community::Database;
use crate::config::BotConfig;
use crate::git_monitor::store::SubscriptionStore;
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
    /// Community engagement database (XP, levels, giveaways, reputation).
    pub community_db: Database,
    /// Git repo monitor subscription store.
    pub git_store: Option<SubscriptionStore>,
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
            // Determine which policy name to display
            let policy_name = match config.bot.invite_policy.as_str() {
                "authorized" => "authorized",
                "whitelist" => "whitelist (legacy)",
                "" => "owner (default)",
                _ => "owner",
            };
            tracing::info!(
                "Invite policy: {} ({} allowed npubs)",
                policy_name,
                npubs.len()
            );
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

    // -----------------------------------------------------------------------
    // Step 1b: Push profile metadata to relays (kind 0) if configured
    // -----------------------------------------------------------------------
    // The SDK's update_bot_profile() doesn't accept lud16. If lud16 is set,
    // we publish a custom kind 0 event that includes it alongside the other
    // fields. Otherwise we fall back to the SDK's built-in profile update.
    let has_profile_fields = config.bot.display_name.is_some()
        || config.bot.picture.is_some()
        || config.bot.banner.is_some()
        || config.bot.about.is_some();

    if has_profile_fields || config.bot.lud16.is_some() {
        let name = config.bot.display_name.as_deref().unwrap_or("Flagship");
        let picture = config.bot.picture.as_deref().unwrap_or("");
        let banner = config.bot.banner.as_deref().unwrap_or("");
        let about = config.bot.about.as_deref().unwrap_or("");
        let lud16 = config.bot.lud16.as_deref().unwrap_or("");
        tracing::info!(
            "Updating bot profile: name={}, picture={}, banner={}, lud16={}",
            name, !picture.is_empty(), !banner.is_empty(), if lud16.is_empty() { "none" } else { lud16 }
        );

        if !lud16.is_empty() {
            // Custom kind 0 with lud16 — sign and publish via nostr-sdk
            match publish_profile_with_lud16(
                nsec.as_deref(),
                name,
                picture,
                banner,
                about,
                lud16,
            ).await {
                true => tracing::info!("Profile metadata (with lud16) published to relays"),
                false => tracing::warn!("Failed to publish profile metadata with lud16 — will retry on next restart"),
            }
        } else {
            // No lud16 — use the SDK's built-in profile update
            if bot.update_profile(name, picture, banner, about).await {
                tracing::info!("Profile metadata published to relays");
            } else {
                tracing::warn!("Failed to publish profile metadata — will retry on next restart");
            }
        }
    }

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
    // Step 2c: Initialize community engagement database
    // -------------------------------------------------------------------------

    let community_db_path = std::path::PathBuf::from(
        std::env::var("COMMUNITY_DB_PATH").unwrap_or_else(|_| "./data/community.sqlite".to_string())
    );
    let community_db = match Database::open(&community_db_path) {
        Ok(db) => {
            tracing::info!("Community database initialized at {}", community_db_path.display());
            db
        }
        Err(e) => {
            tracing::error!("Failed to init community database: {}", e);
            return Err(e);
        }
    };

    // -------------------------------------------------------------------------
    // Step 2d: Initialize git monitor store (optional, feature-gated)
    // -------------------------------------------------------------------------

    let git_store = if config.features.git_monitor && config.git_monitor.enabled {
        let git_db_path = std::path::PathBuf::from(
            std::env::var("GIT_MONITOR_DB_PATH")
                .unwrap_or_else(|_| "./data/repos.sqlite".to_string()),
        );
        match SubscriptionStore::open(&git_db_path) {
            Ok(s) => {
                tracing::info!("Git monitor store initialized at {}", git_db_path.display());
                Some(s)
            }
            Err(e) => {
                tracing::error!("Failed to init git monitor store: {}", e);
                None
            }
        }
    } else {
        tracing::info!("Git monitor disabled (feature flag or config)");
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
        community_db,
        git_store,
    };

    // -------------------------------------------------------------------------
    // Step 4: Register all handlers
    // -------------------------------------------------------------------------

    handlers::register(&bot, ctx.clone()).await?;

    // -------------------------------------------------------------------------
    // Step 4b: v2 community bootstrap
    // -------------------------------------------------------------------------
    {
        let v2_config = &ctx.config.v2;

        if v2_config.auto_create {
            // Check if we're in any communities already
            let existing = bot.communities().await;
            if existing.is_empty() {
                let name = v2_config.community_name.as_deref().unwrap_or("Bot Community");
                match bot.core().create_community_v2(name).await {
                    Ok(summary) => {
                        let id = summary
                            .get("community_id")
                            .and_then(|v| v.as_str())
                            .unwrap_or("?");
                        tracing::info!("Auto-created v2 community: {}", id);
                    }
                    Err(e) => tracing::error!("Failed to auto-create community: {:?}", e),
                }
            } else {
                tracing::info!("Already in {} community/communities — skipping auto-create", existing.len());
            }
        }

        for link in &v2_config.join_on_start {
            match bot.core().join_community(link).await {
                Ok(summary) => {
                    tracing::info!("Joined community: {:?}", summary);
                    // Disable all channels from the join summary (opt-in default)
                    if let Some(channels) = summary.get("channels").and_then(|v| v.as_array()) {
                        for ch in channels {
                            if let Some(ch_id) = ch.get("channel_id").and_then(|v| v.as_str()) {
                                if let Err(e) = ctx.community_db.disable_channel(ch_id) {
                                    tracing::warn!("Failed to disable channel {}: {}", ch_id, e);
                                } else {
                                    tracing::info!("Channel {} disabled (opt-in default)", ch_id);
                                }
                            }
                        }
                    }
                }
                Err(e) => tracing::error!("Failed to join {}: {:?}", link, e),
            }
        }
    }

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
                            "Incoming message from {} (npub={:?}): {}",
                            msg.chat_id,
                            msg.message.npub,
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

// -----------------------------------------------------------------------------
// Profile publishing with lud16
// -----------------------------------------------------------------------------

/// Publish a kind 0 (Metadata) event that includes lud16 alongside the standard fields.
///
/// Publish a kind 0 event with lud16 (SDK doesn't accept lud16 yet).
async fn publish_profile_with_lud16(
    nsec: Option<&str>,
    name: &str,
    picture: &str,
    banner: &str,
    about: &str,
    lud16: &str,
) -> bool {
    use nostr_sdk::prelude::*;

    let nsec = match nsec {
        Some(n) => n,
        None => return false,
    };

    let keys = match Keys::parse(nsec) {
        Ok(k) => k,
        Err(e) => { tracing::warn!("Failed to parse nsec: {:?}", e); return false; }
    };

    let mut meta = serde_json::json!({ "name": name, "about": about, "bot": true });
    if !picture.is_empty() { meta["picture"] = serde_json::Value::String(picture.to_string()); }
    if !banner.is_empty() { meta["banner"] = serde_json::Value::String(banner.to_string()); }
    if !lud16.is_empty() { meta["lud16"] = serde_json::Value::String(lud16.to_string()); }

    let event = match EventBuilder::new(Kind::Metadata, meta.to_string()).sign(&keys).await {
        Ok(e) => e,
        Err(e) => { tracing::warn!("Failed to sign kind 0: {:?}", e); return false; }
    };

    if let Some(client) = vector_sdk::vector_core::state::nostr_client() {
        match client.send_event(&event).await {
            Ok(_) => { tracing::info!("Published kind 0 with lud16={}", lud16); true }
            Err(e) => { tracing::warn!("Failed to send kind 0: {:?}", e); false }
        }
    } else {
        tracing::warn!("Nostr client not available");
        false
    }
}
