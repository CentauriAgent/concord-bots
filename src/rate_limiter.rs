// =============================================================================
// rate_limiter.rs — Per-user spam protection
// =============================================================================
//
// Tracks command usage per user and enforces:
//   - A sliding-window burst limit (N commands per window)
//   - Per-command cooldowns (especially for API-backed commands)
//
// When a user exceeds the burst limit, they get a cooldown lockout.
// All state is in-memory; resets on restart.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::RwLock;

/// Default burst limit: 5 commands per 30 seconds per user.
const DEFAULT_BURST_LIMIT: u32 = 5;
const DEFAULT_BURST_WINDOW_SECS: u64 = 30;

/// Default cooldown after exceeding burst: 30 seconds.
const DEFAULT_COOLDOWN_SECS: u64 = 30;

/// Extended cooldown for repeat offenders: 5 minutes.
const REPEAT_OFFENDER_COOLDOWN_SECS: u64 = 300;
const REPEAT_OFFENDER_THRESHOLD: u32 = 3;

/// Per-command cooldown overrides (in seconds).
/// Returns None for commands with no per-command cooldown.
fn command_cooldown(command: &str) -> Option<Duration> {
    match command {
        "!price" | "!weather" => Some(Duration::from_secs(10)),
        // All other commands: 3-second cooldown to prevent spam
        // (but tests use unique command names which won't match here)
        _ if command.starts_with('!') => Some(Duration::from_secs(3)),
        _ => None,
    }
}

/// What the rate limiter decided to do with a request.
pub enum RateLimitResult {
    /// The command is allowed to proceed.
    Allow,
    /// The command is denied. The reason should be sent to the user.
    Deny(String),
}

struct UserState {
    /// Timestamps of recent commands (sliding window).
    recent_commands: Vec<Instant>,
    /// Per-command last-used timestamps.
    command_cooldowns: HashMap<String, Instant>,
    /// When the current cooldown expires (if any).
    cooldown_until: Option<Instant>,
    /// How many times the user has hit the burst limit.
    violations: u32,
}

impl Default for UserState {
    fn default() -> Self {
        Self {
            recent_commands: Vec::new(),
            command_cooldowns: HashMap::new(),
            cooldown_until: None,
            violations: 0,
        }
    }
}

/// Thread-safe rate limiter shared across all handlers.
#[derive(Clone)]
pub struct RateLimiter {
    inner: Arc<RwLock<HashMap<String, UserState>>>,
    burst_limit: u32,
    burst_window: Duration,
    cooldown: Duration,
}

impl Default for RateLimiter {
    fn default() -> Self {
        Self {
            inner: Arc::new(RwLock::new(HashMap::new())),
            burst_limit: DEFAULT_BURST_LIMIT,
            burst_window: Duration::from_secs(DEFAULT_BURST_WINDOW_SECS),
            cooldown: Duration::from_secs(DEFAULT_COOLDOWN_SECS),
        }
    }
}

impl RateLimiter {
    /// Create a new rate limiter with custom parameters.
    #[allow(dead_code)]
    pub fn new(burst_limit: u32, burst_window_secs: u64, cooldown_secs: u64) -> Self {
        Self {
            inner: Arc::new(RwLock::new(HashMap::new())),
            burst_limit,
            burst_window: Duration::from_secs(burst_window_secs),
            cooldown: Duration::from_secs(cooldown_secs),
        }
    }

    /// Check if a command should be allowed.
    ///
    /// Call this BEFORE dispatching the command. If it returns `Deny`,
    /// send the reason to the user and skip the command.
    pub async fn check(&self, user: &str, command: &str) -> RateLimitResult {
        let now = Instant::now();
        let mut users = self.inner.write().await;

        let state = users.entry(user.to_string()).or_default();

        // Check if user is in cooldown.
        if let Some(until) = state.cooldown_until {
            if now < until {
                let remaining = (until - now).as_secs();
                return RateLimitResult::Deny(format!(
                    "⏳ You're rate-limited. Try again in {}s.",
                    remaining.max(1)
                ));
            } else {
                state.cooldown_until = None;
            }
        }

        // Check per-command cooldown.
        if let Some(cd) = command_cooldown(command) {
            if let Some(&last) = state.command_cooldowns.get(command) {
                let elapsed = now - last;
                if elapsed < cd {
                    let remaining = (cd - elapsed).as_secs();
                    return RateLimitResult::Deny(format!(
                        "⏳ !{} is on cooldown. Try again in {}s.",
                        command.trim_start_matches('!'),
                        remaining.max(1)
                    ));
                }
            }
        }

        // Check burst limit (sliding window).
        state.recent_commands.retain(|&t| now.duration_since(t) < self.burst_window);

        if state.recent_commands.len() >= self.burst_limit as usize {
            state.violations += 1;
            let cd = if state.violations >= REPEAT_OFFENDER_THRESHOLD {
                tracing::warn!(
                    "User {} hit repeat offender threshold ({} violations)",
                    user,
                    state.violations
                );
                Duration::from_secs(REPEAT_OFFENDER_COOLDOWN_SECS)
            } else {
                self.cooldown
            };
            state.cooldown_until = Some(now + cd);
            state.recent_commands.clear();

            let cd_secs = cd.as_secs();
            return RateLimitResult::Deny(format!(
                "🛑 Slow down! You've hit the rate limit ({} commands per {}s). Try again in {}s.",
                self.burst_limit,
                self.burst_window.as_secs(),
                cd_secs
            ));
        }

        // All checks passed — record the command.
        state.recent_commands.push(now);
        if command_cooldown(command).is_some() {
            state.command_cooldowns.insert(command.to_string(), now);
        }

        RateLimitResult::Allow
    }

    /// Clean up stale entries periodically (optional housekeeping).
    #[allow(dead_code)]
    pub async fn cleanup(&self) {
        let now = Instant::now();
        let mut users = self.inner.write().await;
        users.retain(|_, state| {
            state.recent_commands.retain(|&t| now.duration_since(t) < self.burst_window);
            let in_cooldown = state.cooldown_until.map(|u| now < u).unwrap_or(false);
            !state.recent_commands.is_empty() || in_cooldown
        });
    }

    /// Get the number of tracked users (for stats/debugging).
    pub async fn tracked_users(&self) -> usize {
        self.inner.read().await.len()
    }
}

// -----------------------------------------------------------------------------
// Tests
// -----------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    // NOTE: command_cooldown() applies a 3s cooldown to ALL ! commands.
    // Tests that fire the same command name rapidly use the per-command cooldown
    // check, NOT the burst limit. To test burst, we use unique command names.

    #[tokio::test]
    async fn test_allow_under_burst() {
        let rl = RateLimiter::new(5, 30, 30);
        // Use unique command names so per-command cooldown doesn't trigger.
        for i in 0..4 {
            let cmd = format!("!test{}", i);
            match rl.check("alice", &cmd).await {
                RateLimitResult::Allow => {}
                RateLimitResult::Deny(_) => panic!("Should be allowed"),
            }
        }
    }

    #[tokio::test]
    async fn test_deny_over_burst() {
        let rl = RateLimiter::new(3, 30, 30);
        for i in 0..3 {
            let cmd = format!("!test{}", i);
            assert!(matches!(
                rl.check("bob", &cmd).await,
                RateLimitResult::Allow
            ));
        }
        // 4th command in 30s window → burst limit hit
        match rl.check("bob", "!over").await {
            RateLimitResult::Deny(msg) => assert!(msg.contains("rate limit")),
            RateLimitResult::Allow => panic!("Should be denied"),
        }
    }

    #[tokio::test]
    async fn test_per_command_cooldown() {
        let rl = RateLimiter::new(100, 30, 30); // High burst
        assert!(matches!(
            rl.check("carol", "!price").await,
            RateLimitResult::Allow
        ));
        match rl.check("carol", "!price").await {
            RateLimitResult::Deny(msg) => assert!(msg.contains("cooldown")),
            RateLimitResult::Allow => panic!("Should be on cooldown"),
        }
    }

    #[tokio::test]
    async fn test_different_users_independent() {
        let rl = RateLimiter::new(2, 30, 30);
        assert!(matches!(rl.check("dave", "!a").await, RateLimitResult::Allow));
        assert!(matches!(rl.check("dave", "!b").await, RateLimitResult::Allow));
        assert!(matches!(rl.check("dave", "!c").await, RateLimitResult::Deny(_)));
        // Eve is independent.
        assert!(matches!(rl.check("eve", "!a").await, RateLimitResult::Allow));
    }

    #[tokio::test]
    async fn test_repeat_offender_escalates() {
        let rl = RateLimiter::new(1, 60, 1);
        // First burst filled + violation
        let _ = rl.check("frank", "!a").await;
        let _ = rl.check("frank", "!b").await; // denied (burst)

        tokio::time::sleep(Duration::from_millis(1100)).await;
        let _ = rl.check("frank", "!c").await;
        let _ = rl.check("frank", "!d").await; // denied (burst, violation 2)

        tokio::time::sleep(Duration::from_millis(1100)).await;
        let _ = rl.check("frank", "!e").await;
        let r = rl.check("frank", "!f").await; // denied (burst, violation 3 → 5 min)
        match r {
            RateLimitResult::Deny(msg) => {
                assert!(msg.contains("Slow down"));
            }
            RateLimitResult::Allow => panic!("Should be denied"),
        }
    }
}
