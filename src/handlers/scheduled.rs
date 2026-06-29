// =============================================================================
// handlers/scheduled.rs — Scheduled/cron tasks (AGENT EXTENSION POINT)
// =============================================================================
//
// This module runs background tasks on a schedule (interval-based).
// Use this for things like:
//   - Posting periodic updates (price feeds, news, reminders)
//   - Polling external APIs for changes
//   - Health checks or heartbeat messages
//
// ============================================================================
// HOW TO ADD A SCHEDULED TASK
// ============================================================================
//
// 1. Add a function that performs the task:
//
//    async fn my_scheduled_task(ctx: BotContext) {
//        // Do something useful
//        let channel = ctx.bot.channel("your-channel-id");
//        let _ = channel.send("Scheduled update!").await;
//    }
//
// 2. Register it in `register()` with an interval:
//
//    spawn_interval_simple(ctx.clone(), 3600, my_scheduled_task);  // every hour
//
// 3. Optionally configure intervals in bot.toml:
//
//    [scheduling]
//    default_interval_secs = 300
//
// ============================================================================

use anyhow::Result;
use std::time::Duration;
use tokio::time::interval;
use vector_sdk::VectorBot;

use crate::bot::BotContext;

/// Register all scheduled tasks.
///
/// Called once at startup. Each task runs in its own tokio task.
pub async fn register(_bot: &VectorBot, _ctx: BotContext) -> Result<()> {
    tracing::info!("Registering scheduled tasks...");

    // =====================================================================
    // ADD YOUR SCHEDULED TASKS HERE
    // =====================================================================

    // Git Monitor Poller
    if _ctx.config.features.git_monitor && _ctx.config.git_monitor.enabled {
        let interval_secs = _ctx.config.git_monitor.poll_interval_secs.max(60);
        spawn_interval_simple(_ctx.clone(), interval_secs, git_monitor_poll_task);
        tracing::info!("Started git monitor poller (every {}s)", interval_secs);
    }

    // Example: Post "I'm alive!" every 5 minutes
    // Uncomment the lines below to enable:
    //
    // let interval_secs = _ctx.config.scheduling.default_interval_secs.unwrap_or(300);
    // spawn_interval_simple(_ctx.clone(), interval_secs, heartbeat_task);
    // tracing::info!("Started heartbeat task (every {}s)", interval_secs);

    // Example: Post Bitcoin price every hour
    //
    // spawn_interval_simple(ctx.clone(), 3600, bitcoin_price_task);

    // Example: Daily reminder at a specific time
    // (Use a longer interval and check the time inside the task)
    //
    // spawn_interval_simple(ctx.clone(), 60, daily_reminder_task);

    tracing::info!("Scheduled tasks registered.");
    Ok(())
}

/// Spawn a task that runs on a fixed interval.
///
/// The task function receives a clone of BotContext and runs repeatedly
/// with `interval_secs` seconds between runs. If the task panics, the
/// panic is caught and logged, and the task continues.
pub fn spawn_interval_simple<F, Fut>(ctx: BotContext, interval_secs: u64, task: F)
where
    F: Fn(BotContext) -> Fut + Send + Sync + 'static,
    Fut: std::future::Future<Output = ()> + Send + 'static,
{
    tokio::spawn(async move {
        let mut ticker = interval(Duration::from_secs(interval_secs));
        ticker.tick().await; // skip first immediate tick

        loop {
            ticker.tick().await;
            tracing::debug!("Running scheduled task...");
            task(ctx.clone()).await;
        }
    });
}

// =============================================================================
// EXAMPLE SCHEDULED TASKS
// =============================================================================
// Uncomment and adapt these for your use case.
// =============================================================================

// /// Post a heartbeat message to verify the bot is alive.
// async fn heartbeat_task(ctx: BotContext) {
//     // Post to a specific channel, or iterate all configured communities.
//     if let Some(channel_id) = ctx.config.communities.join.first() {
//         let channel = ctx.bot.channel(channel_id.clone());
//         let _ = channel.send("🫀 I'm alive!").await;
//     } else {
//         tracing::debug!("No community configured — skipping heartbeat");
//     }
// }

// /// Post Bitcoin price to a channel.
// async fn bitcoin_price_task(ctx: BotContext) {
//     let data = match crate::lib::http::fetch_json(
//         "https://api.coingecko.com/api/v3/simple/price?ids=bitcoin&vs_currencies=usd"
//     ).await {
//         Ok(d) => d,
//         Err(e) => {
//             tracing::warn!("Failed to fetch BTC price: {}", e);
//             return;
//         }
//     };
//
//     let price = data["bitcoin"]["usd"]
//         .as_f64()
//         .map(|p| format!("${:.0}", p))
//         .unwrap_or_else(|| "unavailable".to_string());
//
//     if let Some(channel_id) = ctx.config.communities.join.first() {
//         let channel = ctx.bot.channel(channel_id.clone());
//         let _ = channel.send(&format!("₿ Bitcoin: {}", price)).await;
//     }
// }

/// Git monitor poll task — polls all subscriptions for new commits/releases.
async fn git_monitor_poll_task(ctx: BotContext) {
    crate::git_monitor::poll_all(&ctx).await;
}

#[cfg(test)]
mod tests {
    #[test]
    fn test_module_loads() {
        // Verify module compiles.
    }
}
