// =============================================================================
// git_monitor/mod.rs — Git repo monitor (public API)
// =============================================================================
//
// Subscribes channels to GitHub/GitLab repos and announces new commits/releases.
// Background poller runs on a configurable interval (default: 5 min).

pub mod detect;
pub mod format;
pub mod github;
pub mod gitlab;
pub mod store;

use crate::bot::BotContext;
use crate::git_monitor::store::RepoHost;
use std::time::Duration;

/// Run a single poll cycle across all subscriptions.
///
/// For each subscription:
/// 1. Fetch latest commits from the API
/// 2. Announce new commits (since last_commit_sha) to the channel
/// 3. Fetch latest release and announce if changed
/// 4. Update last_commit_sha, last_release_tag, last_poll_at
/// 5. Sleep polite_sleep_ms between subscriptions
pub async fn poll_all(ctx: &BotContext) {
    let store = match &ctx.git_store {
        Some(s) => s,
        None => return,
    };

    let subs = match store.list_all() {
        Ok(s) => s,
        Err(e) => {
            tracing::warn!("Git monitor: failed to list subscriptions: {}", e);
            return;
        }
    };

    if subs.is_empty() {
        return;
    }

    let config = &ctx.config.git_monitor;
    let polite = Duration::from_millis(config.polite_sleep_ms);

    // Resolve tokens: env var > config
    let github_token = std::env::var("GITHUB_TOKEN")
        .ok()
        .filter(|s| !s.is_empty())
        .or_else(|| {
            if config.github_token.is_empty() {
                None
            } else {
                Some(config.github_token.clone())
            }
        });

    let gitlab_token = std::env::var("GITLAB_TOKEN")
        .ok()
        .filter(|s| !s.is_empty())
        .or_else(|| {
            if config.gitlab_token.is_empty() {
                None
            } else {
                Some(config.gitlab_token.clone())
            }
        });

    let mut rate_limit_exhausted = false;

    for sub in &subs {
        if rate_limit_exhausted {
            tracing::warn!("Git monitor: skipping remaining subs due to rate limit");
            break;
        }

        let result = match sub.host {
            RepoHost::GitHub => {
                github::poll_subscription(ctx, sub, github_token.as_deref(), &config.default_branch)
                    .await
            }
            RepoHost::GitLab => {
                gitlab::poll_subscription(
                    ctx,
                    sub,
                    gitlab_token.as_deref(),
                    &config.gitlab_host,
                    &config.default_branch,
                )
                .await
            }
        };

        if let Err(e) = result {
            tracing::warn!(
                "Git monitor: error polling {}/{}: {}",
                sub.owner,
                sub.repo,
                e
            );
        }

        // Check for rate limit bail signal
        if let Some(ref gh) = github::last_rate_limit_remaining() {
            if *gh < 5 {
                tracing::warn!(
                    "Git monitor: GitHub rate limit low ({}), bailing cycle",
                    gh
                );
                rate_limit_exhausted = true;
            }
        }

        tokio::time::sleep(polite).await;
    }
}
