// =============================================================================
// handlers/git_cmds.rs — !git command handlers
// =============================================================================
//
// Commands:
//   !git add <url|owner/repo>     — Subscribe channel to a repo (Authorized+)
//   !git list                      — List channel's subscriptions (Public)
//   !git remove <repo-or-id>       — Unsubscribe from a repo (Authorized+)
//   !git poll                      — Force a poll cycle (Owner)

use anyhow::Result;
use vector_sdk::IncomingMessage;

use crate::bot::BotContext;
use crate::git_monitor::detect;
use crate::git_monitor::store::RepoHost;

// -----------------------------------------------------------------------------
// !git add <repo>
// -----------------------------------------------------------------------------

pub async fn git_add_command(ctx: &BotContext, msg: &IncomingMessage, args: &str) -> Result<()> {
    let input = args.trim();

    if input.is_empty() {
        msg.reply(
            "Usage: !git add <url|owner/repo>\n\
             Examples:\n\
             !git add https://github.com/owner/repo\n\
             !git add owner/repo\n\
             !git add gitlab owner/repo\n\
             !git add https://gitlab.com/owner/repo",
        )
        .await?;
        return Ok(());
    }

    let store = match &ctx.git_store {
        Some(s) => s,
        None => {
            msg.reply("⚠️ Git monitor is not initialized.").await?;
            return Ok(());
        }
    };

    // Parse the input
    let parsed = match detect::parse_repo(input) {
        Ok(p) => p,
        Err(e) => {
            msg.reply(&format!("⚠️ {}", e)).await?;
            return Ok(());
        }
    };

    let channel_id = &msg.chat_id;
    let max_repos = ctx.config.git_monitor.max_repos_per_channel;

    // Check max repos limit
    match store.count_for_channel(channel_id) {
        Ok(count) if count >= max_repos => {
            msg.reply(&format!(
                "⚠️ This channel already has {} subscriptions (max: {}). \
                 Use !git remove to free up space.",
                count, max_repos
            ))
            .await?;
            return Ok(());
        }
        Ok(_) => {}
        Err(e) => {
            tracing::warn!("Git monitor: count error: {}", e);
            msg.reply("⚠️ Could not check subscription count.").await?;
            return Ok(());
        }
    }

    // Check if already subscribed
    if let Ok(true) = store.exists(channel_id, parsed.host, &parsed.owner, &parsed.repo) {
        msg.reply(&format!(
            "ℹ️ Already subscribed to {} in this channel.",
            parsed.slug()
        ))
        .await?;
        return Ok(());
    }

    // Add the subscription
    let added_by = msg.message.npub.clone().unwrap_or_default();
    if let Err(e) = store.add(channel_id, parsed.host, &parsed.owner, &parsed.repo, &added_by) {
        msg.reply(&format!("⚠️ Could not add subscription: {}", e)).await?;
        return Ok(());
    }

    let host_label = match parsed.host {
        RepoHost::GitHub => "GitHub",
        RepoHost::GitLab => "GitLab",
    };

    msg.reply(&format!(
        "✅ Subscribed to {} {} ({})\n\
         New commits and releases will be announced here automatically.\n\
         Use !git remove {} to unsubscribe.",
        host_label,
        parsed.slug(),
        parsed.url(),
        parsed.slug(),
    ))
    .await?;

    tracing::info!(
        "Git monitor: {} subscribed {} to {}",
        added_by,
        channel_id,
        parsed.slug()
    );

    Ok(())
}

// -----------------------------------------------------------------------------
// !git list
// -----------------------------------------------------------------------------

pub async fn git_list_command(ctx: &BotContext, msg: &IncomingMessage) -> Result<()> {
    let store = match &ctx.git_store {
        Some(s) => s,
        None => {
            msg.reply("⚠️ Git monitor is not initialized.").await?;
            return Ok(());
        }
    };

    let channel_id = &msg.chat_id;
    let subs = match store.list_for_channel(channel_id) {
        Ok(s) => s,
        Err(e) => {
            tracing::warn!("Git monitor: list error: {}", e);
            msg.reply("⚠️ Could not retrieve subscriptions.").await?;
            return Ok(());
        }
    };

    if subs.is_empty() {
        msg.reply(
            "📦 No repo subscriptions in this channel.\n\
             Use !git add <url|owner/repo> to subscribe.",
        )
        .await?;
        return Ok(());
    }

    let mut lines = vec![format!("📦 Repo subscriptions in this channel ({})", subs.len())];
    for (i, sub) in subs.iter().enumerate() {
        let host_icon = match sub.host {
            RepoHost::GitHub => "🐙",
            RepoHost::GitLab => "🦊",
        };
        let status = if sub.last_commit_sha.is_none() {
            " (initializing…)"
        } else {
            ""
        };
        lines.push(format!(
            "{}. {} {} — {}{}",
            i + 1, host_icon, sub.full_slug, sub.host.base_url(), status
        ));
    }
    lines.push("\nUse !git remove <number|slug> to unsubscribe.".to_string());

    msg.reply(&lines.join("\n")).await?;
    Ok(())
}

// -----------------------------------------------------------------------------
// !git remove <repo-or-id>
// -----------------------------------------------------------------------------

pub async fn git_remove_command(ctx: &BotContext, msg: &IncomingMessage, args: &str) -> Result<()> {
    let input = args.trim();

    if input.is_empty() {
        msg.reply(
            "Usage: !git remove <repo-slug|id>\n\
             Examples: !git remove owner/repo, !git remove 3",
        )
        .await?;
        return Ok(());
    }

    let store = match &ctx.git_store {
        Some(s) => s,
        None => {
            msg.reply("⚠️ Git monitor is not initialized.").await?;
            return Ok(());
        }
    };

    let channel_id = &msg.chat_id;

    // Try parsing as a positional number (1-based index into this channel's list)
    if let Ok(pos) = input.parse::<usize>() {
        if pos == 0 {
            msg.reply("⚠️ Numbers start at 1. Use !git list to see positions.").await?;
            return Ok(());
        }
        let subs = match store.list_for_channel(channel_id) {
            Ok(s) => s,
            Err(e) => {
                tracing::warn!("Git monitor: list error during remove: {}", e);
                msg.reply("⚠️ Could not look up subscriptions.").await?;
                return Ok(());
            }
        };
        if pos > subs.len() {
            msg.reply(&format!(
                "⚠️ No subscription #{}. This channel has {} repo(s). Use !git list to see them.",
                pos, subs.len()
            ))
            .await?;
            return Ok(());
        }
        let sub = &subs[pos - 1];
        match store.remove_by_id(sub.id) {
            Ok(true) => {
                msg.reply(&format!("✅ Removed {} (was #{})", sub.full_slug, pos)).await?;
                tracing::info!("Git monitor: removed {} from channel {}", sub.full_slug, channel_id);
            }
            Ok(false) => {
                msg.reply("⚠️ Subscription vanished before removal.").await?;
            }
            Err(e) => {
                tracing::warn!("Git monitor: remove error: {}", e);
                msg.reply("⚠️ Could not remove subscription.").await?;
            }
        }
        return Ok(());
    }

    // Parse as repo slug
    let parsed = match detect::parse_repo(input) {
        Ok(p) => p,
        Err(e) => {
            msg.reply(&format!("⚠️ {}", e)).await?;
            return Ok(());
        }
    };

    match store.remove(channel_id, parsed.host, &parsed.owner, &parsed.repo) {
        Ok(true) => {
            msg.reply(&format!(
                "✅ Unsubscribed from {} {}.",
                match parsed.host {
                    RepoHost::GitHub => "GitHub",
                    RepoHost::GitLab => "GitLab",
                },
                parsed.slug()
            ))
            .await?;
            tracing::info!(
                "Git monitor: unsubscribed {} from {}",
                channel_id,
                parsed.slug()
            );
        }
        Ok(false) => {
            msg.reply(&format!(
                "⚠️ No subscription for {} in this channel.",
                parsed.slug()
            ))
            .await?;
        }
        Err(e) => {
            tracing::warn!("Git monitor: remove error: {}", e);
            msg.reply("⚠️ Could not remove subscription.").await?;
        }
    }

    Ok(())
}

// -----------------------------------------------------------------------------
// !git poll (Owner — force poll)
// -----------------------------------------------------------------------------

pub async fn git_poll_command(ctx: &BotContext, msg: &IncomingMessage) -> Result<()> {
    let store = match &ctx.git_store {
        Some(s) => s,
        None => {
            msg.reply("⚠️ Git monitor is not initialized.").await?;
            return Ok(());
        }
    };

    let channel_id = &msg.chat_id;
    let subs = match store.list_for_channel(channel_id) {
        Ok(s) => s,
        Err(e) => {
            tracing::warn!("Git monitor: list error: {}", e);
            msg.reply("⚠️ Could not retrieve subscriptions.").await?;
            return Ok(());
        }
    };

    if subs.is_empty() {
        msg.reply("📦 No subscriptions in this channel to poll.").await?;
        return Ok(());
    }

    msg.reply(&format!("🔄 Force-polling {} subscriptions…", subs.len()))
        .await?;

    // Run poll_all in a spawned task so we don't block the command handler
    let ctx_clone = ctx.clone();
    tokio::spawn(async move {
        crate::git_monitor::poll_all(&ctx_clone).await;
    });

    Ok(())
}

// -----------------------------------------------------------------------------
// !git (dispatcher)
// -----------------------------------------------------------------------------

pub async fn git_command(ctx: &BotContext, msg: &IncomingMessage, args: &str) -> Result<()> {
    let args = args.trim();
    let parts: Vec<&str> = args.splitn(2, char::is_whitespace).collect();
    let subcommand = parts.first().copied().unwrap_or("");
    let rest = parts.get(1).copied().unwrap_or("");

    match subcommand {
        "add" => git_add_command(ctx, msg, rest).await?,
        "list" => git_list_command(ctx, msg).await?,
        "remove" | "rm" | "delete" => git_remove_command(ctx, msg, rest).await?,
        "poll" => git_poll_command(ctx, msg).await?,
        "" => {
            msg.reply(
                "📦 Git Monitor commands:\n\
                 !git add <url|owner/repo> — Subscribe to a repo\n\
                 !git list — Show subscriptions\n\
                 !git remove <repo|id> — Unsubscribe\n\
                 !git poll — Force poll (owner only)",
            )
            .await?;
        }
        _ => {
            msg.reply(&format!(
                "⚠️ Unknown !git subcommand \"{}\". Try: !git add, !git list, !git remove, !git poll",
                subcommand
            ))
            .await?;
        }
    }

    Ok(())
}
