// =============================================================================
// git_monitor/github.rs — GitHub API client
// =============================================================================
//
// Fetches commits and releases from the GitHub REST API.
// Supports ETag conditional requests and rate-limit awareness.

use anyhow::{bail, Result};
use std::sync::atomic::{AtomicI64, Ordering};

use crate::bot::BotContext;
use crate::git_monitor::format::{CommitInfo, ReleaseInfo};
use crate::git_monitor::store::Subscription;

/// Track the last seen GitHub rate-limit remaining value.
/// Using AtomicI64 so it can be read without a lock.
static RATE_LIMIT_REMAINING: AtomicI64 = AtomicI64::new(5000);

/// Get the last observed rate-limit remaining, if recently set.
pub fn last_rate_limit_remaining() -> Option<i64> {
    let v = RATE_LIMIT_REMAINING.load(Ordering::Relaxed);
    if v < 5000 {
        Some(v)
    } else {
        None
    }
}

/// Parse and store rate-limit headers from a response.
fn update_rate_limit(headers: &reqwest::header::HeaderMap) {
    if let Some(remaining) = headers.get("x-ratelimit-remaining") {
        if let Ok(s) = remaining.to_str() {
            if let Ok(n) = s.parse::<i64>() {
                RATE_LIMIT_REMAINING.store(n, Ordering::Relaxed);
            }
        }
    }
}

/// Build the GitHub API base URL.
fn api_base() -> &'static str {
    "https://api.github.com"
}

/// Build auth header value.
fn auth_header(token: Option<&str>) -> Option<String> {
    token.map(|t| format!("Bearer {}", t))
}

/// Fetch latest commits from a repo.
///
/// Returns (commits, etag). If `if_none_match` is provided and the server
/// returns 304, returns an empty vec.
pub async fn fetch_commits(
    owner: &str,
    repo: &str,
    branch: &str,
    token: Option<&str>,
    if_none_match: Option<&str>,
) -> Result<(Vec<CommitInfo>, Option<String>)> {
    let url = format!(
        "{}/repos/{}/{}/commits?per_page=5&sha={}",
        api_base(),
        owner,
        repo,
        branch
    );

    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(15))
        .user_agent(format!("concord-bots/{}", env!("CARGO_PKG_VERSION")))
        .build()?;

    let mut req = client.get(&url);
    if let Some(ref t) = auth_header(token) {
        req = req.header("Authorization", t);
    }
    if let Some(etag) = if_none_match {
        req = req.header("If-None-Match", etag);
    }
    req = req.header("Accept", "application/vnd.github+json");

    let resp = req.send().await?;
    let status = resp.status();
    update_rate_limit(resp.headers());

    if status.as_u16() == 304 {
        return Ok((vec![], None));
    }

    if !status.is_success() {
        let body = resp.text().await.unwrap_or_default();
        bail!("GitHub commits API {} for {}/{}: {}", status, owner, repo, body);
    }

    let new_etag = resp
        .headers()
        .get("etag")
        .and_then(|v| v.to_str().ok())
        .map(|s| s.to_string());

    let data: serde_json::Value = resp.json().await?;
    let commits = parse_commits(&data);

    Ok((commits, new_etag))
}

/// Fetch the latest release (or tags fallback).
pub async fn fetch_latest_release(
    owner: &str,
    repo: &str,
    token: Option<&str>,
) -> Result<Option<ReleaseInfo>> {
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(15))
        .user_agent(format!("concord-bots/{}", env!("CARGO_PKG_VERSION")))
        .build()?;

    // Try /releases/latest first
    let url = format!("{}/repos/{}/{}/releases/latest", api_base(), owner, repo);
    let mut req = client.get(&url).header("Accept", "application/vnd.github+json");
    if let Some(ref t) = auth_header(token) {
        req = req.header("Authorization", t);
    }

    let resp = req.send().await?;
    update_rate_limit(resp.headers());

    if resp.status().as_u16() == 404 {
        // Fallback to tags
        return fetch_latest_tag(owner, repo, token).await;
    }

    if !resp.status().is_success() {
        let status = resp.status();
        let body = resp.text().await.unwrap_or_default();
        bail!(
            "GitHub releases API {} for {}/{}: {}",
            status,
            owner,
            repo,
            body
        );
    }

    let data: serde_json::Value = resp.json().await?;
    let release = parse_release(&data);
    Ok(release)
}

/// Fallback: fetch latest tag.
async fn fetch_latest_tag(
    owner: &str,
    repo: &str,
    token: Option<&str>,
) -> Result<Option<ReleaseInfo>> {
    let url = format!(
        "{}/repos/{}/{}/tags?per_page=1",
        api_base(),
        owner,
        repo
    );

    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(15))
        .user_agent(format!("concord-bots/{}", env!("CARGO_PKG_VERSION")))
        .build()?;

    let mut req = client.get(&url).header("Accept", "application/vnd.github+json");
    if let Some(ref t) = auth_header(token) {
        req = req.header("Authorization", t);
    }

    let resp = req.send().await?;
    update_rate_limit(resp.headers());

    if !resp.status().is_success() {
        return Ok(None);
    }

    let data: serde_json::Value = resp.json().await?;
    let empty = vec![];
    let tags = data.as_array().unwrap_or(&empty);
    if tags.is_empty() {
        return Ok(None);
    }

    let tag = &tags[0];
    let tag_name = tag["name"].as_str().unwrap_or("");
    if tag_name.is_empty() {
        return Ok(None);
    }

    Ok(Some(ReleaseInfo {
        tag: tag_name.to_string(),
        name: None,
        body: None,
        html_url: format!(
            "https://github.com/{}/{}/releases/tag/{}",
            owner, repo, tag_name
        ),
    }))
}

/// Parse GitHub commits API response into CommitInfo vec.
fn parse_commits(data: &serde_json::Value) -> Vec<CommitInfo> {
    let arr = match data.as_array() {
        Some(a) => a,
        None => return vec![],
    };

    arr.iter()
        .map(|c| CommitInfo {
            sha: c["sha"].as_str().unwrap_or("").to_string(),
            message: c["commit"]["message"]
                .as_str()
                .unwrap_or("")
                .to_string(),
            author: c["commit"]["author"]["name"]
                .as_str()
                .or_else(|| c["author"]["login"].as_str())
                .unwrap_or("Unknown")
                .to_string(),
            authored_at: c["commit"]["author"]["date"]
                .as_str()
                .and_then(parse_github_date),
        })
        .collect()
}

/// Parse a release JSON object.
fn parse_release(data: &serde_json::Value) -> Option<ReleaseInfo> {
    let tag = data["tag_name"].as_str()?;
    Some(ReleaseInfo {
        tag: tag.to_string(),
        name: data["name"].as_str().map(|s| s.to_string()),
        body: data["body"].as_str().map(|s| s.to_string()),
        html_url: data["html_url"]
            .as_str()
            .unwrap_or("")
            .to_string(),
    })
}

/// Parse ISO 8601 date from GitHub API → unix timestamp.
fn parse_github_date(s: &str) -> Option<i64> {
    chrono::DateTime::parse_from_rfc3339(s)
        .ok()
        .map(|dt| dt.timestamp())
}

// -----------------------------------------------------------------------------
// Polling
// -----------------------------------------------------------------------------

/// Poll a single GitHub subscription for new commits and releases.
/// Sends announcements to the subscribed channel.
pub async fn poll_subscription(
    ctx: &BotContext,
    sub: &Subscription,
    token: Option<&str>,
    default_branch: &str,
) -> Result<()> {
    let store = ctx.git_store.as_ref().unwrap();
    let config = &ctx.config.git_monitor;

    // Fetch commits
    let (commits, _etag) =
        fetch_commits(&sub.owner, &sub.repo, default_branch, token, None).await?;

    // Determine which commits are new
    let new_commits: Vec<CommitInfo> = match &sub.last_commit_sha {
        None => {
            // Silent init: don't dump backlog. Set the pointer but don't announce.
            if let Some(latest) = commits.first() {
                store.set_last_commit_sha(sub.id, &latest.sha)?;
                tracing::info!(
                    "Git monitor: silent init for {} — SHA set to {}",
                    sub.full_slug,
                    short_sha(&latest.sha)
                );
            }
            vec![]
        }
        Some(last_sha) => {
            // Filter: only commits after the last known SHA
            let mut found_new = Vec::new();
            let mut seen_old = false;
            for c in &commits {
                if c.sha == *last_sha {
                    seen_old = true;
                    break;
                }
                found_new.push(c.clone());
            }

            if !seen_old && !commits.is_empty() {
                // The last_sha is not in the latest 5 commits — could be
                // many new commits. Just announce what we have.
                found_new = commits.clone();
            }

            // Reverse to oldest-first for chronological announcement
            found_new.reverse();

            if !found_new.is_empty() {
                let latest_sha = found_new.last().unwrap().sha.clone();

                if config.post_commits {
                    let msg = crate::git_monitor::format::format_commits(sub, &found_new);
                    if !msg.is_empty() {
                        let _ = ctx.bot.channel(sub.channel_id.clone()).send(&msg).await;
                    }
                }

                store.set_last_commit_sha(sub.id, &latest_sha)?;
            }

            found_new
        }
    };

    tracing::debug!(
        "Git monitor: polled GitHub {} — {} new commits",
        sub.full_slug,
        new_commits.len()
    );

    // Fetch latest release
    if config.post_releases {
        if let Some(release) = fetch_latest_release(&sub.owner, &sub.repo, token).await? {
            let is_new = match &sub.last_release_tag {
                None => {
                    // Silent init for releases too
                    store.set_last_release_tag(sub.id, &release.tag)?;
                    false
                }
                Some(prev) => *prev != release.tag,
            };

            if is_new {
                let msg = crate::git_monitor::format::format_release(sub, &release);
                let _ = ctx.bot.channel(sub.channel_id.clone()).send(&msg).await;
                store.set_last_release_tag(sub.id, &release.tag)?;
            }
        }
    }

    store.touch_poll(sub.id)?;
    Ok(())
}

fn short_sha(sha: &str) -> &str {
    if sha.len() >= 7 {
        &sha[..7]
    } else {
        sha
    }
}

// -----------------------------------------------------------------------------
// Tests
// -----------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn test_parse_commits() {
        let data = json!([
            {
                "sha": "abc1234567890",
                "commit": {
                    "message": "Fix bug",
                    "author": {
                        "name": "Sam Thomson",
                        "date": "2024-01-15T10:30:00Z"
                    }
                },
                "author": { "login": "samthomson" }
            },
            {
                "sha": "def5678901234",
                "commit": {
                    "message": "Add feature\n\nDetailed body",
                    "author": {
                        "name": "MK Dev",
                        "date": "2024-01-16T12:00:00Z"
                    }
                }
            }
        ]);

        let commits = parse_commits(&data);
        assert_eq!(commits.len(), 2);
        assert_eq!(commits[0].sha, "abc1234567890");
        assert_eq!(commits[0].message, "Fix bug");
        assert_eq!(commits[0].author, "Sam Thomson");
        assert!(commits[0].authored_at.is_some());

        assert_eq!(commits[1].sha, "def5678901234");
        assert_eq!(commits[1].message, "Add feature\n\nDetailed body");
        assert_eq!(commits[1].author, "MK Dev");
    }

    #[test]
    fn test_parse_empty_commits() {
        let data = json!([]);
        let commits = parse_commits(&data);
        assert!(commits.is_empty());
    }

    #[test]
    fn test_parse_release() {
        let data = json!({
            "tag_name": "v2.4.0",
            "name": "Dark Mode + Bug Fixes",
            "body": "Release notes here",
            "html_url": "https://github.com/owner/repo/releases/tag/v2.4.0"
        });

        let release = parse_release(&data).unwrap();
        assert_eq!(release.tag, "v2.4.0");
        assert_eq!(release.name.as_deref(), Some("Dark Mode + Bug Fixes"));
        assert_eq!(release.body.as_deref(), Some("Release notes here"));
    }

    #[test]
    fn test_parse_github_date() {
        let ts = parse_github_date("2024-01-15T10:30:00Z");
        assert!(ts.is_some());
        assert!(ts.unwrap() > 0);

        assert!(parse_github_date("invalid").is_none());
    }

    #[test]
    fn test_parse_commits_missing_fields() {
        let data = json!([
            { "sha": "abc123", "commit": {} }
        ]);
        let commits = parse_commits(&data);
        assert_eq!(commits.len(), 1);
        assert_eq!(commits[0].sha, "abc123");
        assert_eq!(commits[0].message, "");
        assert_eq!(commits[0].author, "Unknown");
    }
}
