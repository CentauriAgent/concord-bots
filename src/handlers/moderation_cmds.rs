// =============================================================================
// handlers/moderation_cmds.rs — Moderation commands
// =============================================================================
//
// Concord-native moderation tools using the Vector SDK's role system:
//   !kick <npub>        — Kick a member (cooperative, they can rejoin)
//   !ban <npub>         — Ban a member (terminal)
//   !unban <npub>       — Lift a ban
//   !warn <npub> <reason> — Issue a warning (local only)
//   !warnings <npub>    — Show warning history
//   !mods               — List current moderators/admins
//   !grantmod <npub>    — Grant admin role
//   !revokemod <npub>   — Revoke admin role

use anyhow::Result;
use std::path::PathBuf;
use vector_sdk::IncomingMessage;

use crate::bot::BotContext;

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
        Ok(contents) => {
            serde_json::from_str(&contents).unwrap_or_default()
        }
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
// !leave — Leave the current community (owner only)
// -----------------------------------------------------------------------------

pub async fn leave_command(ctx: &BotContext, msg: &IncomingMessage, _args: &str) -> Result<()> {
    let community = match msg.community() {
        Some(c) => c,
        None => {
            msg.reply("⚠️ This command can only be used in a community channel.").await?;
            return Ok(());
        }
    };

    let community_id = community.id().to_string();

    msg.reply(&format!("👋 Leaving community... ({})", community_id)).await?;

    match community.leave().await {
        Ok(()) => {
            msg.reply("✅ Successfully left the community. Goodbye! 👋").await?;
            tracing::info!("Bot left community {} (requested by owner)", community_id);
        }
        Err(e) => {
            let err_text = format!("{:?}", e);
            tracing::error!("Failed to leave community {}: {}", community_id, err_text);
            msg.reply(&format!("⚠️ Could not leave the community: {}", err_text)).await?;
        }
    }

    Ok(())
}

// -----------------------------------------------------------------------------
// !kick <npub> — Kick a member (cooperative)
// -----------------------------------------------------------------------------

pub async fn kick_command(_ctx: &BotContext, msg: &IncomingMessage, args: &str) -> Result<()> {
    let npub = args.trim();

    if npub.is_empty() {
        msg.reply("Usage: !kick <npub>\nExample: !kick npub1abc...").await?;
        return Ok(());
    }

    if !npub.starts_with("npub1") {
        msg.reply("⚠️ That doesn't look like a valid npub. npubs start with \"npub1\".").await?;
        return Ok(());
    }

    // Need a community context to act on members.
    let community = match msg.community() {
        Some(c) => c,
        None => {
            msg.reply("⚠️ This command can only be used in a community channel.").await?;
            return Ok(());
        }
    };

    // Don't kick yourself or the owner.
    if npub == _ctx.bot.npub() {
        msg.reply("⚠️ I can't kick myself!").await?;
        return Ok(());
    }

    // Check if target is the owner.
    let target_member = community.member(npub);
    if target_member.is_owner() {
        msg.reply("⚠️ I can't kick the community owner.").await?;
        return Ok(());
    }

    match target_member.kick().await {
        Ok(()) => {
            msg.reply(&format!("👢 Kicked {} from the community. (They can rejoin.)", npub)).await?;
            tracing::info!("Kicked {} from community {}", npub, community.id());
        }
        Err(e) => {
            let err_text = format!("{:?}", e);
            if err_text.contains("permission") || err_text.contains("Permission") || err_text.contains("KICK") {
                msg.reply(
                    "I don't have KICK permission in this community. Ask the owner to grant me the Admin role."
                ).await?;
            } else {
                tracing::warn!("Kick failed: {:?}", e);
                msg.reply(&format!("⚠️ Could not kick {}. They may not be a member or I lack permission.", npub)).await?;
            }
        }
    }

    Ok(())
}

// -----------------------------------------------------------------------------
// !ban <npub> — Ban a member (terminal)
// -----------------------------------------------------------------------------

pub async fn ban_command(_ctx: &BotContext, msg: &IncomingMessage, args: &str) -> Result<()> {
    let npub = args.trim();

    if npub.is_empty() {
        msg.reply("Usage: !ban <npub>\nExample: !ban npub1abc...").await?;
        return Ok(());
    }

    if !npub.starts_with("npub1") {
        msg.reply("⚠️ That doesn't look like a valid npub. npubs start with \"npub1\".").await?;
        return Ok(());
    }

    let community = match msg.community() {
        Some(c) => c,
        None => {
            msg.reply("⚠️ This command can only be used in a community channel.").await?;
            return Ok(());
        }
    };

    if npub == _ctx.bot.npub() {
        msg.reply("⚠️ I can't ban myself!").await?;
        return Ok(());
    }

    let target_member = community.member(npub);
    if target_member.is_owner() {
        msg.reply("⚠️ I can't ban the community owner.").await?;
        return Ok(());
    }

    match target_member.ban().await {
        Ok(()) => {
            msg.reply(&format!("🔨 Banned {} from the community.", npub)).await?;
            tracing::info!("Banned {} from community {}", npub, community.id());
        }
        Err(e) => {
            let err_text = format!("{:?}", e);
            if err_text.contains("permission") || err_text.contains("Permission") || err_text.contains("BAN") {
                msg.reply(
                    "I don't have BAN permission in this community. Ask the owner to grant me the Admin role."
                ).await?;
            } else {
                tracing::warn!("Ban failed: {:?}", e);
                msg.reply(&format!("⚠️ Could not ban {}. They may not be a member or I lack permission.", npub)).await?;
            }
        }
    }

    Ok(())
}

// -----------------------------------------------------------------------------
// !unban <npub> — Lift a ban
// -----------------------------------------------------------------------------

pub async fn unban_command(_ctx: &BotContext, msg: &IncomingMessage, args: &str) -> Result<()> {
    let npub = args.trim();

    if npub.is_empty() {
        msg.reply("Usage: !unban <npub>\nExample: !unban npub1abc...").await?;
        return Ok(());
    }

    if !npub.starts_with("npub1") {
        msg.reply("⚠️ That doesn't look like a valid npub. npubs start with \"npub1\".").await?;
        return Ok(());
    }

    let community = match msg.community() {
        Some(c) => c,
        None => {
            msg.reply("⚠️ This command can only be used in a community channel.").await?;
            return Ok(());
        }
    };

    let target_member = community.member(npub);

    match target_member.unban().await {
        Ok(()) => {
            msg.reply(&format!("✅ Unbanned {} from the community.", npub)).await?;
            tracing::info!("Unbanned {} from community {}", npub, community.id());
        }
        Err(e) => {
            let err_text = format!("{:?}", e);
            if err_text.contains("permission") || err_text.contains("Permission") {
                msg.reply(
                    "I don't have BAN permission in this community. Ask the owner to grant me the Admin role."
                ).await?;
            } else {
                tracing::warn!("Unban failed: {:?}", e);
                msg.reply(&format!("⚠️ Could not unban {}. They may not be banned or I lack permission.", npub)).await?;
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
        msg.reply("Usage: !warn <npub> <reason>\nExample: !warn npub1abc... Please be respectful").await?;
        return Ok(());
    }

    // Parse: first token is npub, rest is reason
    let parts: Vec<&str> = args.splitn(2, char::is_whitespace).collect();
    let npub = parts[0];
    let reason = parts.get(1).copied().unwrap_or("").trim();

    if !npub.starts_with("npub1") {
        msg.reply("⚠️ That doesn't look like a valid npub. npubs start with \"npub1\".").await?;
        return Ok(());
    }

    if reason.is_empty() {
        msg.reply("⚠️ Please provide a reason.\nExample: !warn npub1abc... Spamming links").await?;
        return Ok(());
    }

    let warned_by = ctx.bot.npub().to_string();
    let timestamp = chrono::Utc::now().format("%Y-%m-%d %H:%M:%S UTC").to_string();

    let warning = Warning {
        npub: npub.to_string(),
        reason: reason.to_string(),
        timestamp: timestamp.clone(),
        warned_by,
    };

    let mut warnings = load_warnings();
    let warning_number = warnings.iter().filter(|w| w.npub == npub).count() + 1;
    warnings.push(warning);
    save_warnings(&warnings);

    msg.reply(&format!(
        "⚠️ Warning issued to {} (warning #{}): {}",
        npub, warning_number, reason
    )).await?;

    tracing::info!("Warning #{} issued to {} by bot", warning_number, npub);
    Ok(())
}

// -----------------------------------------------------------------------------
// !warnings <npub> — Show warning history
// -----------------------------------------------------------------------------

pub async fn warnings_command(_ctx: &BotContext, msg: &IncomingMessage, args: &str) -> Result<()> {
    let npub = args.trim();

    if npub.is_empty() {
        msg.reply("Usage: !warnings <npub>\nExample: !warnings npub1abc...").await?;
        return Ok(());
    }

    if !npub.starts_with("npub1") {
        msg.reply("⚠️ That doesn't look like a valid npub. npubs start with \"npub1\".").await?;
        return Ok(());
    }

    let all_warnings = load_warnings();
    let user_warnings: Vec<&Warning> = all_warnings.iter().filter(|w| w.npub == npub).collect();

    if user_warnings.is_empty() {
        msg.reply(&format!("📋 No warnings on record for {}.", npub)).await?;
        return Ok(());
    }

    let mut response = format!("📋 Warnings for {} ({}):\n", npub, user_warnings.len());
    for (i, w) in user_warnings.iter().enumerate() {
        response.push_str(&format!("  {}. {} ({})\n", i + 1, w.reason, w.timestamp));
    }

    msg.reply(response.trim()).await?;
    Ok(())
}

// -----------------------------------------------------------------------------
// !mods — List current moderators/admins
// -----------------------------------------------------------------------------

pub async fn mods_command(_ctx: &BotContext, msg: &IncomingMessage, _args: &str) -> Result<()> {
    let community = match msg.community() {
        Some(c) => c,
        None => {
            msg.reply("⚠️ This command can only be used in a community channel.").await?;
            return Ok(());
        }
    };

    let roles = match community.roles() {
        Ok(r) => r,
        Err(e) => {
            tracing::warn!("Failed to fetch roles: {:?}", e);
            msg.reply("⚠️ Could not retrieve community roles.").await?;
            return Ok(());
        }
    };

    let owner = roles.get("owner").and_then(|o| o.as_str()).unwrap_or("unknown");
    let admins = roles.get("admins").and_then(|a| a.as_array());

    let mut response = format!("🛡️ **Community Roles**\n👑 Owner: {}\n", owner);

    if let Some(admins) = admins {
        if admins.is_empty() {
            response.push_str("🔧 Admins: (none)");
        } else {
            response.push_str("🔧 Admins:\n");
            for admin in admins {
                if let Some(n) = admin.as_str() {
                    response.push_str(&format!("  • {}", n));
                }
            }
        }
    } else {
        response.push_str("🔧 Admins: (none)");
    }

    msg.reply(response.trim()).await?;
    Ok(())
}

// -----------------------------------------------------------------------------
// !grantmod <npub> — Grant admin role
// -----------------------------------------------------------------------------

pub async fn grantmod_command(_ctx: &BotContext, msg: &IncomingMessage, args: &str) -> Result<()> {
    let npub = args.trim();

    if npub.is_empty() {
        msg.reply("Usage: !grantmod <npub>\nExample: !grantmod npub1abc...").await?;
        return Ok(());
    }

    if !npub.starts_with("npub1") {
        msg.reply("⚠️ That doesn't look like a valid npub. npubs start with \"npub1\".").await?;
        return Ok(());
    }

    let community = match msg.community() {
        Some(c) => c,
        None => {
            msg.reply("⚠️ This command can only be used in a community channel.").await?;
            return Ok(());
        }
    };

    let target_member = community.member(npub);

    // Check if already admin
    if target_member.is_admin() {
        msg.reply(&format!("ℹ️ {} is already an admin.", npub)).await?;
        return Ok(());
    }

    match target_member.grant_admin().await {
        Ok(()) => {
            msg.reply(&format!("✅ Granted admin role to {}.", npub)).await?;
            tracing::info!("Granted admin to {} in community {}", npub, community.id());
        }
        Err(e) => {
            let err_text = format!("{:?}", e);
            if err_text.contains("permission") || err_text.contains("Permission") || err_text.contains("MANAGE_ROLES") {
                msg.reply(
                    "I don't have MANAGE_ROLES permission in this community. Ask the owner to grant me the Admin role."
                ).await?;
            } else {
                tracing::warn!("Grant admin failed: {:?}", e);
                msg.reply(&format!("⚠️ Could not grant admin to {}. I may lack the required permission.", npub)).await?;
            }
        }
    }

    Ok(())
}

// -----------------------------------------------------------------------------
// !revokemod <npub> — Revoke admin role
// -----------------------------------------------------------------------------

pub async fn revokemod_command(_ctx: &BotContext, msg: &IncomingMessage, args: &str) -> Result<()> {
    let npub = args.trim();

    if npub.is_empty() {
        msg.reply("Usage: !revokemod <npub>\nExample: !revokemod npub1abc...").await?;
        return Ok(());
    }

    if !npub.starts_with("npub1") {
        msg.reply("⚠️ That doesn't look like a valid npub. npubs start with \"npub1\".").await?;
        return Ok(());
    }

    let community = match msg.community() {
        Some(c) => c,
        None => {
            msg.reply("⚠️ This command can only be used in a community channel.").await?;
            return Ok(());
        }
    };

    // Don't revoke from owner.
    let target_member = community.member(npub);
    if target_member.is_owner() {
        msg.reply("⚠️ Cannot revoke admin from the community owner.").await?;
        return Ok(());
    }

    // Check if they're actually an admin.
    if !target_member.is_admin() {
        msg.reply(&format!("ℹ️ {} is not an admin.", npub)).await?;
        return Ok(());
    }

    match target_member.revoke_admin().await {
        Ok(()) => {
            msg.reply(&format!("✅ Revoked admin role from {}.", npub)).await?;
            tracing::info!("Revoked admin from {} in community {}", npub, community.id());
        }
        Err(e) => {
            let err_text = format!("{:?}", e);
            if err_text.contains("permission") || err_text.contains("Permission") || err_text.contains("MANAGE_ROLES") {
                msg.reply(
                    "I don't have MANAGE_ROLES permission in this community. Ask the owner to grant me the Admin role."
                ).await?;
            } else {
                tracing::warn!("Revoke admin failed: {:?}", e);
                msg.reply(&format!("⚠️ Could not revoke admin from {}. I may lack the required permission.", npub)).await?;
            }
        }
    }

    Ok(())
}
