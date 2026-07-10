// =============================================================================
// handlers/community_cmds.rs — Community engagement commands
// =============================================================================
//
// Commands:
//   !level [npub] / !rank [npub]  — Show level and XP progress
//   !leaderboard                   — Top 10 users by XP
//   !profile [npub]                — User profile card
//   !giveaway <duration> <prize>   — Start a giveaway (Authorized+)
//   !giveaway end                  — End giveaway in current channel early
//   !giveaway list                 — List active giveaways
//   !rep [npub]                    — Give or check reputation

use anyhow::Result;
use std::time::Duration;
use vector_sdk::IncomingMessage;

use crate::bot::BotContext;
use crate::handlers::normalize_npub;
use crate::community::{
    xp_for_level, xp_in_current_level,
};

// -----------------------------------------------------------------------------
// Helpers
// -----------------------------------------------------------------------------

/// Shorten an npub for display: npub1jrv...nev5
fn short_npub(npub: &str) -> String {
    if npub.len() > 16 {
        format!("{}...{}", &npub[..12], &npub[npub.len() - 4..])
    } else {
        npub.to_string()
    }
}

/// Format first_seen timestamp to a human-readable month/year.
fn format_member_since(ts: i64) -> String {
    if ts == 0 {
        return "Unknown".to_string();
    }
    let dt = chrono::DateTime::from_timestamp(ts, 0);
    match dt {
        Some(d) => d.format("%b %Y").to_string(),
        None => "Unknown".to_string(),
    }
}

/// Format a future timestamp as a human-readable countdown.
fn format_ends_in(ends_at: i64) -> String {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs() as i64;
    let remaining = ends_at - now;
    if remaining <= 0 {
        return "ended".to_string();
    }
    if remaining < 60 {
        return format!("{}s", remaining);
    }
    if remaining < 3600 {
        return format!("{}m", remaining / 60);
    }
    format!("{}h {}m", remaining / 3600, (remaining % 3600) / 60)
}

/// Parse a duration string like "10m", "1h", "30m", "2h".
fn parse_duration(s: &str) -> Option<u64> {
    let s = s.trim().to_lowercase();
    if s.is_empty() {
        return None;
    }

    let (num_str, unit) = if s.ends_with('m') {
        (&s[..s.len() - 1], 'm')
    } else if s.ends_with('h') {
        (&s[..s.len() - 1], 'h')
    } else if s.chars().all(|c| c.is_ascii_digit()) {
        (s.as_str(), 'm') // default to minutes
    } else {
        return None;
    };

    let num: u64 = num_str.parse().ok()?;
    match unit {
        'm' => Some(num * 60),
        'h' => Some(num * 3600),
        _ => None,
    }
}

// -----------------------------------------------------------------------------
// !level / !rank [npub]
// -----------------------------------------------------------------------------

pub async fn level_command(ctx: &BotContext, msg: &IncomingMessage, args: &str) -> Result<()> {
    let target_npub = if args.trim().is_empty() {
        msg.message.npub.clone().unwrap_or_default()
    } else {
        normalize_npub(args)
    };

    if target_npub.is_empty() {
        msg.reply("⚠️ Could not determine your npub.").await?;
        return Ok(());
    }

    let stats = match ctx.community_db.get_user(&target_npub) {
        Ok(s) => s,
        Err(e) => {
            tracing::warn!("Community DB error: {}", e);
            msg.reply("⚠️ Could not retrieve level data.").await?;
            return Ok(());
        }
    };

    let level = stats.level;
    let xp_in_level = xp_in_current_level(stats.xp, level);
    let xp_needed = xp_for_level(level + 1);
    let xp_remaining = xp_needed - xp_in_level;

    let display = if args.trim().is_empty() {
        "Your".to_string()
    } else {
        format!("{}'s", short_npub(&target_npub))
    };

    let response = format!(
        "📊 {} Level {}\n📈 {} / {} XP\n🎯 Next level in {} XP",
        display, level, xp_in_level, xp_needed, xp_remaining
    );

    msg.reply(&response).await?;
    Ok(())
}

// -----------------------------------------------------------------------------
// !leaderboard
// -----------------------------------------------------------------------------

pub async fn leaderboard_command(ctx: &BotContext, msg: &IncomingMessage) -> Result<()> {
    let entries = match ctx.community_db.get_leaderboard(10) {
        Ok(e) => e,
        Err(e) => {
            tracing::warn!("Community DB error: {}", e);
            msg.reply("⚠️ Could not retrieve leaderboard.").await?;
            return Ok(());
        }
    };

    if entries.is_empty() {
        msg.reply("🏆 Leaderboard is empty. Start chatting to earn XP!").await?;
        return Ok(());
    }

    let mut lines = vec!["🏆 **Leaderboard**".to_string()];
    for (i, (npub, level, xp)) in entries.iter().enumerate() {
        let medal = match i {
            0 => "🥇".to_string(),
            1 => "🥈".to_string(),
            2 => "🥉".to_string(),
            _ => format!("{}.", i + 1),
        };
        lines.push(format!(
            "{} nostr:{} — Level {} ({} XP)",
            medal,
            npub,
            level,
            xp
        ));
    }

    msg.reply(&lines.join("\n")).await?;
    Ok(())
}

// -----------------------------------------------------------------------------
// !profile [npub]
// -----------------------------------------------------------------------------

pub async fn profile_command(ctx: &BotContext, msg: &IncomingMessage, args: &str) -> Result<()> {
    let target_npub = if args.trim().is_empty() {
        msg.message.npub.clone().unwrap_or_default()
    } else {
        normalize_npub(args)
    };

    if target_npub.is_empty() {
        msg.reply("⚠️ Could not determine your npub.").await?;
        return Ok(());
    }

    let stats = match ctx.community_db.get_user(&target_npub) {
        Ok(s) => s,
        Err(e) => {
            tracing::warn!("Community DB error: {}", e);
            msg.reply("⚠️ Could not retrieve profile.").await?;
            return Ok(());
        }
    };

    let rank = ctx
        .community_db
        .get_rank(&target_npub)
        .ok()
        .flatten()
        .map(|r| format!("#{}", r))
        .unwrap_or_else(|| "Unranked".to_string());

    let total_sats = stats.sats_tipped + stats.sats_zapped;
    let member_since = format_member_since(stats.first_seen);

    let response = format!(
        "👤 {}\n📊 Level {} · {} XP\n📈 {} on leaderboard\n⚡ {} sats tipped/zapped\n💬 {} messages\n🎯 Member since {}\n⭐ Reputation: {}",
        short_npub(&target_npub),
        stats.level,
        stats.xp,
        rank,
        total_sats,
        stats.messages_sent,
        member_since,
        stats.rep
    );

    msg.reply(&response).await?;
    Ok(())
}

// -----------------------------------------------------------------------------
// !giveaway
// -----------------------------------------------------------------------------

pub async fn giveaway_command(ctx: &BotContext, msg: &IncomingMessage, args: &str) -> Result<()> {
    let args = args.trim();

    if args.is_empty() {
        msg.reply(
            "Usage: !giveaway <duration> <prize>\n\
             Or: !giveaway end | !giveaway list\n\
             Examples: !giveaway 10m 50 sats, !giveaway 1h Premium codes x3",
        )
        .await?;
        return Ok(());
    }

    // Subcommands
    if args == "end" {
        return giveaway_end_command(ctx, msg).await;
    }
    if args == "list" {
        return giveaway_list_command(ctx, msg).await;
    }

    // Parse: <duration> <prize>
    let parts: Vec<&str> = args.splitn(2, char::is_whitespace).collect();
    if parts.len() < 2 {
        msg.reply(
            "⚠️ Please provide both duration and prize.\n\
             Example: !giveaway 10m 50 sats",
        )
        .await?;
        return Ok(());
    }

    let duration_str = parts[0];
    let prize_str = parts[1].trim();

    let duration_secs = match parse_duration(duration_str) {
        Some(d) => d,
        None => {
            msg.reply(&format!(
                "⚠️ Could not parse duration \"{}\". Use: 10m, 1h, 30m",
                duration_str
            ))
            .await?;
            return Ok(());
        }
    };

    if duration_secs < 60 || duration_secs > 86400 {
        msg.reply("⚠️ Duration must be between 1 minute and 24 hours.").await?;
        return Ok(());
    }

    // Parse prize — check for "N sats" pattern
    let prize_sats: i64 = {
        let lower = prize_str.to_lowercase();
        if let Some(sats_part) = lower
            .strip_suffix(" sats")
            .or_else(|| lower.strip_suffix(" sat"))
        {
            sats_part.trim().parse().unwrap_or(0)
        } else {
            0
        }
    };

    // Generate a unique giveaway ID
    let giveaway_id = format!(
        "gw-{}",
        chrono::Utc::now().timestamp_millis() % 1_000_000_000
    );

    let now_ts = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs() as i64;
    let ends_at = now_ts + duration_secs as i64;

    // Store in DB
    if let Err(e) = ctx.community_db.add_giveaway(
        &giveaway_id,
        &msg.chat_id,
        prize_str,
        prize_sats,
        now_ts,
        ends_at,
    ) {
        tracing::warn!("Failed to create giveaway: {}", e);
        msg.reply("⚠️ Could not create giveaway.").await?;
        return Ok(());
    }

    let ends_human = format_ends_in(ends_at);

    let response = format!(
        "🎉 **GIVEAWAY!**\n🎁 Prize: {}\n⏱️ Ends: in {}\n\nReact with 🎉 to enter!",
        prize_str, ends_human
    );

    msg.reply(&response).await?;

    tracing::info!(
        "Giveaway {} created in channel {} by {}, ends at {}",
        giveaway_id,
        msg.chat_id,
        msg.message.npub.as_deref().unwrap_or("?"),
        ends_at
    );

    // Spawn a background task to auto-end the giveaway
    let db = ctx.community_db.clone();
    let channel_id = msg.chat_id.clone();
    let bot = ctx.bot.clone();
    let prize = prize_str.to_string();
    let prize_sats_val = prize_sats;
    let gid = giveaway_id.clone();

    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_secs(duration_secs)).await;

        match db.pick_winner(&gid) {
            Ok(Some(winner)) => {
                let short = short_npub(&winner);
                let _ = bot
                    .channel(channel_id.clone())
                    .send(&format!(
                        "🎉 Giveaway ended!\n🎁 Prize: {}\n🏆 Winner: {}",
                        prize, short
                    ))
                    .await;

                if prize_sats_val > 0 {
                    tracing::info!(
                        "Giveaway {} winner {} should receive {} sats",
                        gid,
                        short,
                        prize_sats_val
                    );
                }
            }
            Ok(None) => {
                let _ = bot
                    .channel(channel_id.clone())
                    .send(&format!(
                        "🎉 Giveaway ended — no entries received.\n🎁 Prize: {}",
                        prize
                    ))
                    .await;
            }
            Err(e) => {
                tracing::warn!("Giveaway {} winner pick failed: {}", gid, e);
            }
        }
    });

    Ok(())
}

/// End active giveaways in the current channel early.
async fn giveaway_end_command(ctx: &BotContext, msg: &IncomingMessage) -> Result<()> {
    let channel_id = &msg.chat_id;

    let active = match ctx.community_db.get_active_giveaways_in_channel(channel_id) {
        Ok(g) => g,
        Err(e) => {
            tracing::warn!("Failed to get active giveaways: {}", e);
            msg.reply("⚠️ Could not check active giveaways.").await?;
            return Ok(());
        }
    };

    if active.is_empty() {
        msg.reply("ℹ️ No active giveaways in this channel.").await?;
        return Ok(());
    }

    let mut results = Vec::new();
    for giveaway in active {
        match ctx.community_db.pick_winner(&giveaway.id) {
            Ok(Some(winner)) => {
                let short = short_npub(&winner);
                results.push(format!(
                    "🎉 Giveaway ended!\n🎁 Prize: {}\n🏆 Winner: {}",
                    giveaway.prize, short
                ));
            }
            Ok(None) => {
                results.push(format!(
                    "🎉 Giveaway ended — no entries.\n🎁 Prize: {}",
                    giveaway.prize
                ));
            }
            Err(e) => {
                tracing::warn!("Giveaway {} error: {}", giveaway.id, e);
            }
        }
    }

    msg.reply(&results.join("\n")).await?;
    Ok(())
}

/// List all active giveaways.
async fn giveaway_list_command(ctx: &BotContext, msg: &IncomingMessage) -> Result<()> {
    let active = match ctx.community_db.get_active_giveaways() {
        Ok(g) => g,
        Err(e) => {
            tracing::warn!("Failed to get active giveaways: {}", e);
            msg.reply("⚠️ Could not retrieve giveaways.").await?;
            return Ok(());
        }
    };

    if active.is_empty() {
        msg.reply("📦 No active giveaways right now.").await?;
        return Ok(());
    }

    let mut lines = vec!["📦 **Active Giveaways**".to_string()];
    for g in &active {
        let ends = format_ends_in(g.ends_at);
        let entries = ctx
            .community_db
            .get_entries(&g.id)
            .map(|e| e.len())
            .unwrap_or(0);
        lines.push(format!(
            "  • {} — {} ({} entries, ends in {})",
            g.prize,
            short_npub(&g.channel_id),
            entries,
            ends
        ));
    }

    msg.reply(&lines.join("\n")).await?;
    Ok(())
}

// -----------------------------------------------------------------------------
// !rep
// -----------------------------------------------------------------------------

pub async fn rep_command(ctx: &BotContext, msg: &IncomingMessage, args: &str) -> Result<()> {
    let sender = msg.message.npub.clone().unwrap_or_default();

    let target_npub = if args.trim().is_empty() {
        // Show own rep
        if sender.is_empty() {
            msg.reply("⚠️ Could not determine your npub.").await?;
            return Ok(());
        }
        let stats = match ctx.community_db.get_user(&sender) {
            Ok(s) => s,
            Err(e) => {
                tracing::warn!("Community DB error: {}", e);
                msg.reply("⚠️ Could not retrieve reputation.").await?;
                return Ok(());
            }
        };
        msg.reply(&format!("⭐ Your reputation: {}", stats.rep)).await?;
        return Ok(());
    } else {
        normalize_npub(args)
    };

    if sender.is_empty() {
        msg.reply("⚠️ Could not determine your npub.").await?;
        return Ok(());
    }

    // Can't rep yourself
    if sender == target_npub {
        msg.reply("⚠️ You can't give reputation to yourself!").await?;
        return Ok(());
    }

    match ctx.community_db.give_rep(&sender, &target_npub) {
        Ok(true) => {
            let total_rep = ctx
                .community_db
                .get_user(&target_npub)
                .map(|s| s.rep)
                .unwrap_or(1);

            // Award 50 XP to the recipient for receiving rep.
            let xp_amount = 50;
            let (new_level, leveled_up) = match ctx
                .community_db
                .award_xp(&target_npub, xp_amount, &msg.chat_id)
            {
                Ok((lvl, lu)) => (lvl, lu),
                Err(e) => {
                    tracing::warn!("Failed to award rep XP: {}", e);
                    (0, false)
                }
            };

            let mut reply = format!(
                "⭐ You gave +1 rep to nostr:{}! (Total: {})",
                target_npub,
                total_rep
            );

            if leveled_up {
                reply.push_str(&format!("\n🎉 nostr:{} reached Level {}!", target_npub, new_level));
            }

            msg.reply(&reply).await?;
        }
        Ok(false) => {
            msg.reply(
                "⏰ You've already given rep to this person recently. Try again in 24 hours.",
            )
            .await?;
        }
        Err(e) => {
            tracing::warn!("Rep failed: {}", e);
            msg.reply("⚠️ Could not give reputation right now.").await?;
        }
    }

    Ok(())
}

// -----------------------------------------------------------------------------
// Tests
// -----------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_duration() {
        assert_eq!(parse_duration("10m"), Some(600));
        assert_eq!(parse_duration("1h"), Some(3600));
        assert_eq!(parse_duration("30m"), Some(1800));
        assert_eq!(parse_duration("2h"), Some(7200));
        assert_eq!(parse_duration("15"), Some(900)); // default minutes
        assert_eq!(parse_duration("abc"), None);
        assert_eq!(parse_duration(""), None);
    }

    #[test]
    fn test_short_npub() {
        let npub = "npub1jrvdfzf9aglmkt3nzpm4y6x3tq056qwh5v6ge2x2g9wkx27j58gsj7nev5";
        let short = short_npub(npub);
        assert!(short.contains("..."));
        assert!(short.starts_with("npub1jrv"));
        assert!(short.ends_with("nev5"));

        let short_input = "short";
        assert_eq!(short_npub(short_input), "short");
    }

    #[test]
    fn test_format_member_since() {
        assert_eq!(format_member_since(0), "Unknown");
        // Jan 1, 2026 = 1767225600
        let result = format_member_since(1767225600);
        assert!(result.contains("2026") || result.contains("Jan"));
    }

    #[test]
    fn test_format_ends_in() {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs() as i64;

        // Already passed
        assert_eq!(format_ends_in(now - 100), "ended");

        // 30 seconds from now
        assert!(format_ends_in(now + 30).ends_with('s'));

        // 5 minutes from now
        assert!(format_ends_in(now + 300).ends_with('m'));

        // 2 hours from now
        assert!(format_ends_in(now + 7200).contains('h'));
    }
}
