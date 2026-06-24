// =============================================================================
// Echo Bot — Scheduled Tasks
// =============================================================================
//
// This file shows how to set up a scheduled task that posts "I'm alive!"
// every 5 minutes.
//
// In a real bot, this would be wired into the scheduled::register() function.
// Here we show the pattern for reference.

// /// Post a heartbeat message every 5 minutes.
// ///
// /// To enable: uncomment this and register it in scheduled::register():
// /// ```ignore
// /// spawn_interval_simple(ctx.clone(), 300, heartbeat_task);
// /// ```
// async fn heartbeat_task(ctx: BotContext) {
//     // Get the first configured community channel.
//     if let Some(channel_id) = ctx.config.communities.join.first() {
//         let channel = ctx.bot.channel(channel_id.clone());
//         match channel.send("🫀 I'm alive!").await {
//             Ok(_) => tracing::info!("Heartbeat sent"),
//             Err(e) => tracing::warn!("Heartbeat failed: {}", e),
//         }
//     } else {
//         tracing::debug!("No community configured — skipping heartbeat");
//     }
// }
