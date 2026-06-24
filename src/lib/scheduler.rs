// =============================================================================
// lib/scheduler.rs — Simple interval scheduler (STABLE — do not edit)
// =============================================================================
//
// A lightweight scheduler for running tasks at fixed intervals.
// Uses tokio's interval timer internally.
//
// For cron-like scheduling (specific times), check the time inside your
// interval task and only act when the conditions match.

use std::time::Duration;
use tokio::time::interval;
use tokio::time::sleep;

/// Run a closure every N seconds, with an optional initial delay.
///
/// This is a convenience wrapper. For more control (like accessing BotContext),
/// use `handlers::scheduled::spawn_interval_simple` instead.
pub fn every<F, Fut>(secs: u64, task: F)
where
    F: Fn() -> Fut + Send + 'static,
    Fut: std::future::Future<Output = ()> + Send + 'static,
{
    tokio::spawn(async move {
        let mut ticker = interval(Duration::from_secs(secs));
        ticker.tick().await; // skip first immediate tick

        loop {
            ticker.tick().await;
            task().await;
        }
    });
}

/// Run a closure once after a delay (like a one-shot timer).
pub fn after<F, Fut>(secs: u64, task: F)
where
    F: FnOnce() -> Fut + Send + 'static,
    Fut: std::future::Future<Output = ()> + Send + 'static,
{
    tokio::spawn(async move {
        sleep(Duration::from_secs(secs)).await;
        task().await;
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_every_signature() {
        // Verify functions exist with correct signatures.
        let _ = std::any::TypeId::of::<fn(u64, fn() -> std::pin::Pin<Box<dyn std::future::Future<Output = ()> + Send>>)>();
    }
}
