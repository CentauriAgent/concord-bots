// =============================================================================
// handlers/moderation_cmds.rs — Moderation commands (v2-aware)
// =============================================================================
//
// Concord-native moderation tools using the Vector SDK's v2 permission system:
//   !kick <npub>        — Kick a member (cooperative, they can rejoin)
//   !ban <npub>         — Ban a member (terminal; rekeys private communities)
//   !unban <npub>       — Lift a ban
//   !warn <npub> <reason> — Issue a warning (local only)
//   !warnings <npub>    — Show warning history
//   !mods               — List current community roles
//   !grantmod <npub>    — Grant admin role (requires MANAGE_ROLES)
//   !revokemod <npub>   — Revoke admin role (requires MANAGE_ROLES)
//
// v2 Changes:
//   - SDK now enforces KICK/BAN permissions + outranking at the protocol level
//   - Private communities trigger a rekey on ban
//   - Permission errors are detected and reported with clear guidance
//   - !mods uses community.roles() for full role information

use anyhow::Result;
use std::path::PathBuf;
use vector_sdk::IncomingMessage;

use crate::bot::BotContext;
use crate::handlers::normalize_npub;

// -----------------------------------------------------------------------------
// Warnings storage
// -----------------------------------------------------------------------------

/// Where warning records are persisted.
fn warnings_file() -> PathBuf {
    PathBuf::from("data/warnings.json")
}

/// A single warning record.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
struct Warning {
    npub: String,
    reason: String,
    timestamp: String,
    warned_by: String,
}

/// Load all warnings from disk. Returns empty vec if file doesn't exist.
fn load_warnings() -> Vec<Warning> {
    match std::fs::read_to_string(warnings_file()) {
        Ok(contents) => serde_json::from_str(&contents).unwrap_or_default(),
        Err(_) => Vec::new(),
    }
}

/// Save all warnings to disk.
fn save_warnings(warnings: &[Warning]) {
    let path = warnings_file();
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    match serde_json::to_string_pretty(warnings) {
        Ok(json) => {
            if let Err(e) = std::fs::write(&path, json) {
                tracing::error!("Failed to write warnings file: {}", e);
            }
        }
        Err(e) => tracing::error!("Failed to serialize warnings: {}", e),
    }
}

// -----------------------------------------------------------------------------
// Permission error detection helper
// -----------------------------------------------------------------------------\

/// Check if an SDK error is a permission/rank related error.
/// Accepts any `Debug` error type so it works with both `anyhow::Error` and `VectorError`.
fn is_permission_error<E: std::fmt::Debug>(e: &E) -> bool {
    let err = format!("{:?}", e).to_lowercase();
    err.contains("permission")
        || err.contains("outrank")
        || err.contains("rank")
        || err.contains("denied")
        || err.contains("unauthorized")
        || err.contains("forbidden")
        || err.contains("manage_roles")
        || err.contains("not allowed")
}

// -----------------------------------------------------------------------------
// !leave — Leave the current community (owner only)
// -----------------------------------------------------------------------------

pub async fn leave_command(ctx: &BotContext, msg: &IncomingMessage, _args: &str) -> Result<()> {
    let community = match msg.community() {
        Some(c) => c,
        None => {
            super::reply(ctx, msg, "⚠️ This command can only be used in a community channel.")
                .await?;
            return Ok(());
        }
    };

    let community_id = community.id().to_string();

    super::reply(ctx, msg, &format!("👋 Leaving community... ({})", community_id))
        .await?;

    match community.leave().await {
        Ok(()) => {
            super::reply(ctx, msg, "✅ Successfully left the community. Goodbye! 👋")
                .await?;
            tracing::info!("Bot left community {} (requested by owner)", community_id);
        }
        Err(e) => {
            let err_text = format!("{:?}", e);
            tracing::error!("Failed to leave community {}: {}", community_id, err_text);
            super::reply(ctx, msg, &format!("⚠️ Could not leave the community: {}", err_text))
                .await?;
        }
    }

    Ok(())
}

// -----------------------------------------------------------------------------
// !kick <npub> — Kick a member (cooperative)
// -----------------------------------------------------------------------------

pub async fn kick_command(ctx: &BotContext, msg: &IncomingMessage, args: &str) -> Result<()> {
    let npub = normalize_npub(args);

    if npub.is_empty() {
        super::reply(ctx, msg, "Usage: !kick <npub>\nExample: !kick nostr:npub1abc...  OR  !kick npub1abc...")
            .await?;
        return Ok(());
    }

    if !npub.starts_with("npub1") {
        super::reply(ctx, msg, "⚠️ That doesn't look like a valid npub. Use npub1... or nostr:npub1...")
            .await?;
        return Ok(());
    }

    let community = match msg.community() {
        Some(c) => c,
        None => {
            super::reply(ctx, msg, "⚠️ This command can only be used in a community channel.")
                .await?;
            return Ok(());
        }
    };

    // Don't kick yourself or the owner.
    if npub == ctx.bot.npub() {
        super::reply(ctx, msg, "⚠️ I can't kick myself!").await?;
        return Ok(());
    }

    // Check if target is the owner.
    let target_member = community.member(&npub);
    if target_member.is_owner() {
        super::reply(ctx, msg, "⚠️ I can't kick the community owner.").await?;
        return Ok(());
    }

    match target_member.kick().await {
        Ok(()) => {
            super::reply(ctx, msg, &format!(
                "👢 Kicked {} from the community. (They can rejoin.)",
                npub
            ))
            .await?;
            tracing::info!("Kicked {} from community {}", npub, community.id());
        }
        Err(e) => {
            if is_permission_error(&e) {
                super::reply(ctx, msg, 
                    "⚠️ I don't have permission to kick that member. Need KICK capability + higher rank.",
                )
                .await?;
            } else {
                tracing::warn!("Kick failed: {:?}", e);
                super::reply(ctx, msg, &format!("⚠️ Kick failed: {}", e)).await?;
            }
        }
    }

    Ok(())
}

// -----------------------------------------------------------------------------
// !ban <npub> — Ban a member (terminal; rekeys private communities)
// -----------------------------------------------------------------------------

pub async fn ban_command(ctx: &BotContext, msg: &IncomingMessage, args: &str) -> Result<()> {
    let npub = normalize_npub(args);

    if npub.is_empty() {
        super::reply(ctx, msg, "Usage: !ban <npub>\nExample: !ban nostr:npub1abc...  OR  !ban npub1abc...")
            .await?;
        return Ok(());
    }

    if !npub.starts_with("npub1") {
        super::reply(ctx, msg, "⚠️ That doesn't look like a valid npub. Use npub1... or nostr:npub1...")
            .await?;
        return Ok(());
    }

    let community = match msg.community() {
        Some(c) => c,
        None => {
            super::reply(ctx, msg, "⚠️ This command can only be used in a community channel.")
                .await?;
            return Ok(());
        }
    };

    if npub == ctx.bot.npub() {
        super::reply(ctx, msg, "⚠️ I can't ban myself!").await?;
        return Ok(());
    }

    let target_member = community.member(&npub);
    if target_member.is_owner() {
        super::reply(ctx, msg, "⚠️ I can't ban the community owner.").await?;
        return Ok(());
    }

    match target_member.ban().await {
        Ok(()) => {
            super::reply(ctx, msg, &format!(
                "🔨 Banned {} from the community. (Community keys rotated for security)",
                npub
            ))
            .await?;
            tracing::info!("Banned {} from community {}", npub, community.id());
        }
        Err(e) => {
            if is_permission_error(&e) {
                super::reply(ctx, msg, 
                    "⚠️ I don't have permission to ban that member. Need BAN capability + higher rank.",
                )
                .await?;
            } else {
                tracing::warn!("Ban failed: {:?}", e);
                super::reply(ctx, msg, &format!("⚠️ Ban failed: {}", e)).await?;
            }
        }
    }

    Ok(())
}

// -----------------------------------------------------------------------------
// !unban <npub> — Lift a ban
// -----------------------------------------------------------------------------

pub async fn unban_command(ctx: &BotContext, msg: &IncomingMessage, args: &str) -> Result<()> {
    let npub = normalize_npub(args);

    if npub.is_empty() {
        super::reply(ctx, msg, "Usage: !unban <npub>\nExample: !unban nostr:npub1abc...  OR  !unban npub1abc...")
            .await?;
        return Ok(());
    }

    if !npub.starts_with("npub1") {
        super::reply(ctx, msg, "⚠️ That doesn't look like a valid npub. Use npub1... or nostr:npub1...")
            .await?;
        return Ok(());
    }

    let community = match msg.community() {
        Some(c) => c,
        None => {
            super::reply(ctx, msg, "⚠️ This command can only be used in a community channel.")
                .await?;
            return Ok(());
        }
    };

    let target_member = community.member(&npub);

    match target_member.unban().await {
        Ok(()) => {
            super::reply(ctx, msg, &format!("✅ Unbanned {} from the community.", npub))
                .await?;
            tracing::info!("Unbanned {} from community {}", npub, community.id());
        }
        Err(e) => {
            if is_permission_error(&e) {
                super::reply(ctx, msg, 
                    "⚠️ I don't have permission to unban that member. Need BAN capability.",
                )
                .await?;
            } else {
                tracing::warn!("Unban failed: {:?}", e);
                super::reply(ctx, msg, &format!("⚠️ Could not unban {}. They may not be banned or I lack permission.", npub))
                    .await?;
            }
        }
    }

    Ok(())
}

// -----------------------------------------------------------------------------
// !warn <npub> <reason> — Issue a warning (local only)
// -----------------------------------------------------------------------------

pub async fn warn_command(ctx: &BotContext, msg: &IncomingMessage, args: &str) -> Result<()> {
    let args = args.trim();

    if args.is_empty() {
        super::reply(ctx, msg, "Usage: !warn <npub> <reason>\nExample: !warn npub1abc... Please be respectful")
            .await?;
        return Ok(());
    }

    // Parse: first token is npub, rest is reason
    let parts: Vec<&str> = args.splitn(2, char::is_whitespace).collect();
    let npub = normalize_npub(parts[0]);
    let reason = parts.get(1).copied().unwrap_or("").trim();

    if !npub.starts_with("npub1") {
        super::reply(ctx, msg, "⚠️ That doesn't look like a valid npub. Use npub1... or nostr:npub1...")
            .await?;
        return Ok(());
    }

    if reason.is_empty() {
        super::reply(ctx, msg, "⚠️ Please provide a reason.\nExample: !warn nostr:npub1abc... Spamming links  OR  !warn npub1abc... Spamming links")
            .await?;
        return Ok(());
    }

    let warned_by = ctx.bot.npub().to_string();
    let timestamp = chrono::Utc::now()
        .format("%Y-%m-%d %H:%M:%S UTC")
        .to_string();

    let warning = Warning {
        npub: npub.clone(),
        reason: reason.to_string(),
        timestamp: timestamp.clone(),
        warned_by,
    };

    let mut warnings = load_warnings();
    let warning_number = warnings.iter().filter(|w| w.npub == npub).count() + 1;
    warnings.push(warning);
    save_warnings(&warnings);

    super::reply(ctx, msg, &format!(
        "⚠️ Warning issued to {} (warning #{}): {}",
        npub, warning_number, reason
    ))
    .await?;

    tracing::info!("Warning #{} issued to {} by bot", warning_number, npub);
    Ok(())
}

// -----------------------------------------------------------------------------
// !warnings <npub> — Show warning history
// -----------------------------------------------------------------------------

pub async fn warnings_command(ctx: &BotContext, msg: &IncomingMessage, args: &str) -> Result<()> {
    let npub = normalize_npub(args);

    if npub.is_empty() {
        super::reply(ctx, msg, "Usage: !warnings <npub>\nExample: !warnings nostr:npub1abc...  OR  !warnings npub1abc...")
            .await?;
        return Ok(());
    }

    if !npub.starts_with("npub1") {
        super::reply(ctx, msg, "⚠️ That doesn't look like a valid npub. Use npub1... or nostr:npub1...")
            .await?;
        return Ok(());
    }

    let all_warnings = load_warnings();
    let user_warnings: Vec<&Warning> = all_warnings.iter().filter(|w| w.npub == npub).collect();

    if user_warnings.is_empty() {
        super::reply(ctx, msg, &format!("📋 No warnings on record for {}.", npub))
            .await?;
        return Ok(());
    }

    let mut response = format!("📋 Warnings for {} ({}):\n", npub, user_warnings.len());
    for (i, w) in user_warnings.iter().enumerate() {
        response.push_str(&format!(
            "  {}. {} ({})\n",
            i + 1,
            w.reason,
            w.timestamp
        ));
    }

    super::reply(ctx, msg, response.trim()).await?;
    Ok(())
}

// -----------------------------------------------------------------------------
// !mods — List community roles (v2: uses community.roles())
// -----------------------------------------------------------------------------

pub async fn mods_command(ctx: &BotContext, msg: &IncomingMessage, _args: &str) -> Result<()> {
    let community = match msg.community() {
        Some(c) => c,
        None => {
            super::reply(ctx, msg, "⚠️ This command can only be used in a community channel.")
                .await?;
            return Ok(());
        }
    };

    match community.roles() {
        Ok(roles) => {
            super::reply(ctx, msg, &format!("📋 Community roles:\n```{}```", roles))
                .await?;
        }
        Err(e) => {
            tracing::warn!("Failed to fetch roles: {:?}", e);
            super::reply(ctx, msg, &format!("⚠️ Could not fetch roles: {}", e))
                .await?;
        }
    }

    Ok(())
}

// -----------------------------------------------------------------------------
// !grantmod <npub> — Grant admin role (requires MANAGE_ROLES)
// -----------------------------------------------------------------------------

pub async fn grantmod_command(
    ctx: &BotContext,
    msg: &IncomingMessage,
    args: &str,
) -> Result<()> {
    let npub = normalize_npub(args);

    if npub.is_empty() {
        super::reply(ctx, msg, "Usage: !grantmod <npub>\nExample: !grantmod nostr:npub1abc...  OR  !grantmod npub1abc...")
            .await?;
        return Ok(());
    }

    if !npub.starts_with("npub1") {
        super::reply(ctx, msg, "⚠️ That doesn't look like a valid npub. Use npub1... or nostr:npub1...")
            .await?;
        return Ok(());
    }

    let community = match msg.community() {
        Some(c) => c,
        None => {
            super::reply(ctx, msg, "⚠️ This command can only be used in a community channel.")
                .await?;
            return Ok(());
        }
    };

    let target_member = community.member(&npub);

    // Check if already admin
    if target_member.is_admin() {
        super::reply(ctx, msg, &format!("ℹ️ {} is already an admin.", npub))
            .await?;
        return Ok(());
    }

    match target_member.grant_admin().await {
        Ok(()) => {
            super::reply(ctx, msg, &format!("✅ Granted admin role to {}.", npub))
                .await?;
            tracing::info!("Granted admin to {} in community {}", npub, community.id());
        }
        Err(e) => {
            if is_permission_error(&e) {
                super::reply(ctx, msg, 
                    "⚠️ I don't have permission to manage roles. Need MANAGE_ROLES capability.",
                )
                .await?;
            } else {
                tracing::warn!("Grant admin failed: {:?}", e);
                super::reply(ctx, msg, &format!("⚠️ Could not grant admin to {}: {}", npub, e))
                    .await?;
            }
        }
    }

    Ok(())
}

// -----------------------------------------------------------------------------
// !revokemod <npub> — Revoke admin role (requires MANAGE_ROLES)
// -----------------------------------------------------------------------------

pub async fn revokemod_command(
    ctx: &BotContext,
    msg: &IncomingMessage,
    args: &str,
) -> Result<()> {
    let npub = normalize_npub(args);

    if npub.is_empty() {
        super::reply(ctx, msg, "Usage: !revokemod <npub>\nExample: !revokemod nostr:npub1abc...  OR  !revokemod npub1abc...")
            .await?;
        return Ok(());
    }

    if !npub.starts_with("npub1") {
        super::reply(ctx, msg, "⚠️ That doesn't look like a valid npub. Use npub1... or nostr:npub1...")
            .await?;
        return Ok(());
    }

    let community = match msg.community() {
        Some(c) => c,
        None => {
            super::reply(ctx, msg, "⚠️ This command can only be used in a community channel.")
                .await?;
            return Ok(());
        }
    };

    // Don't revoke from owner.
    let target_member = community.member(&npub);
    if target_member.is_owner() {
        super::reply(ctx, msg, "⚠️ Cannot revoke admin from the community owner.")
            .await?;
        return Ok(());
    }

    // Check if they're actually an admin.
    if !target_member.is_admin() {
        super::reply(ctx, msg, &format!("ℹ️ {} is not an admin.", npub))
            .await?;
        return Ok(());
    }

    match target_member.revoke_admin().await {
        Ok(()) => {
            super::reply(ctx, msg, &format!("✅ Revoked admin role from {}.", npub))
                .await?;
            tracing::info!("Revoked admin from {} in community {}", npub, community.id());
        }
        Err(e) => {
            if is_permission_error(&e) {
                super::reply(ctx, msg, 
                    "⚠️ I don't have permission to manage roles. Need MANAGE_ROLES capability.",
                )
                .await?;
            } else {
                tracing::warn!("Revoke admin failed: {:?}", e);
                super::reply(ctx, msg, &format!("⚠️ Could not revoke admin from {}: {}", npub, e))
                    .await?;
            }
        }
    }

    Ok(())
}
