// =============================================================================
// git_monitor/gitlab.rs — GitLab API client
// =============================================================================
//
// Fetches commits and releases from the GitLab REST API.
// Supports project ID resolution from URL-encoded paths.

use anyhow::{bail, Result};

use crate::bot::BotContext;
use crate::git_monitor::format::{CommitInfo, ReleaseInfo};
use crate::git_monitor::store::Subscription;

/// Build the GitLab API base URL.
fn api_base(gitlab_host: &str) -> String {
    format!("{}/api/v4", gitlab_host)
}

/// URL-encode a project path for the GitLab API.
/// "soapbox-pub/agora" → "soapbox-pub%2Fagora"
fn encode_project_path(owner: &str, repo: &str) -> String {
    format!("{}%2F{}", owner, repo)
}

/// Build auth header value from token.
fn auth_header(token: Option<&str>) -> Option<String> {
    token.map(|t| format!("Bearer {}", t))
}

/// Build a reqwest client with proper UA.
fn build_client() -> Result<reqwest::Client> {
    Ok(reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(15))
        .user_agent(format!("concord-bots/{}", env!("CARGO_PKG_VERSION")))
        .build()?)
}

/// Fetch latest commits from a GitLab project.
pub async fn fetch_commits(
    owner: &str,
    repo: &str,
    branch: &str,
    token: Option<&str>,
    gitlab_host: &str,
) -> Result<Vec<CommitInfo>> {
    let client = build_client()?;
    let path = encode_project_path(owner, repo);
    let url = format!(
        "{}/projects/{}/repository/commits?per_page=5&ref_name={}",
        api_base(gitlab_host),
        path,
        branch
    );

    let mut req = client.get(&url);
    if let Some(ref t) = auth_header(token) {
        req = req.header("Authorization", t);
    }

    let resp = req.send().await?;
    let status = resp.status();

    if !status.is_success() {
        let body = resp.text().await.unwrap_or_default();
        bail!(
            "GitLab commits API {} for {}/{}: {}",
            status,
            owner,
            repo,
            body
        );
    }

    let data: serde_json::Value = resp.json().await?;
    Ok(parse_commits(&data))
}

/// Fetch the latest release from a GitLab project.
pub async fn fetch_latest_release(
    owner: &str,
    repo: &str,
    token: Option<&str>,
    gitlab_host: &str,
) -> Result<Option<ReleaseInfo>> {
    let client = build_client()?;
    let path = encode_project_path(owner, repo);
    let url = format!(
        "{}/projects/{}/releases?per_page=1",
        api_base(gitlab_host),
        path
    );

    let mut req = client.get(&url);
    if let Some(ref t) = auth_header(token) {
        req = req.header("Authorization", t);
    }

    let resp = req.send().await?;
    let status = resp.status();

    if !status.is_success() {
        // GitLab may return 404 if no releases exist
        if status.as_u16() == 404 {
            return Ok(None);
        }
        let body = resp.text().await.unwrap_or_default();
        bail!(
            "GitLab releases API {} for {}/{}: {}",
            status,
            owner,
            repo,
            body
        );
    }

    let data: serde_json::Value = resp.json().await?;
    let empty = vec![];
    let releases = data.as_array().unwrap_or(&empty);
    if releases.is_empty() {
        return Ok(None);
    }

    let r = &releases[0];
    let tag = r["tag_name"].as_str().unwrap_or("");
    if tag.is_empty() {
        return Ok(None);
    }

    Ok(Some(ReleaseInfo {
        tag: tag.to_string(),
        name: r["name"].as_str().map(|s| s.to_string()),
        body: r["description"].as_str().map(|s| s.to_string()),
        html_url: format!(
            "{}/{}/{}/-/releases/{}",
            gitlab_host, owner, repo, tag
        ),
    }))
}

/// Parse GitLab commits API response.
fn parse_commits(data: &serde_json::Value) -> Vec<CommitInfo> {
    let arr = match data.as_array() {
        Some(a) => a,
        None => return vec![],
    };

    arr.iter()
        .map(|c| CommitInfo {
            sha: c["id"].as_str().unwrap_or("").to_string(),
            message: c["message"].as_str().unwrap_or("").to_string(),
            author: c["author_name"]
                .as_str()
                .unwrap_or("Unknown")
                .to_string(),
            authored_at: c["created_at"]
                .as_str()
                .and_then(parse_gitlab_date),
        })
        .collect()
}

/// Parse ISO 8601 date from GitLab API → unix timestamp.
fn parse_gitlab_date(s: &str) -> Option<i64> {
    chrono::DateTime::parse_from_rfc3339(s)
        .ok()
        .map(|dt| dt.timestamp())
}

// -----------------------------------------------------------------------------
// Polling
// -----------------------------------------------------------------------------

/// Poll a single GitLab subscription for new commits and releases.
pub async fn poll_subscription(
    ctx: &BotContext,
    sub: &Subscription,
    token: Option<&str>,
    gitlab_host: &str,
    default_branch: &str,
) -> Result<()> {
    let store = ctx.git_store.as_ref().unwrap();
    let config = &ctx.config.git_monitor;

    // Fetch commits
    let commits =
        fetch_commits(&sub.owner, &sub.repo, default_branch, token, gitlab_host).await?;

    // Determine which commits are new
    let new_commits = match &sub.last_commit_sha {
        None => {
            // Silent init
            if let Some(latest) = commits.first() {
                store.set_last_commit_sha(sub.id, &latest.sha)?;
                tracing::info!(
                    "Git monitor: silent init for GitLab {} — SHA set to {}",
                    sub.full_slug,
                    &latest.sha[..7.min(latest.sha.len())]
                );
            }
            vec![]
        }
        Some(last_sha) => {
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
                found_new = commits.clone();
            }

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
        "Git monitor: polled GitLab {} — {} new commits",
        sub.full_slug,
        new_commits.len()
    );

    // Fetch latest release
    if config.post_releases {
        if let Some(release) =
            fetch_latest_release(&sub.owner, &sub.repo, token, gitlab_host).await?
        {
            let is_new = match &sub.last_release_tag {
                None => {
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

// -----------------------------------------------------------------------------
// Tests
// -----------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn test_encode_project_path() {
        assert_eq!(encode_project_path("soapbox-pub", "agora"), "soapbox-pub%2Fagora");
        assert_eq!(encode_project_path("myorg", "myrepo"), "myorg%2Fmyrepo");
    }

    #[test]
    fn test_parse_commits() {
        let data = json!([
            {
                "id": "abc1234567890abcdef",
                "message": "Fix login bug",
                "author_name": "Sam Thomson",
                "created_at": "2024-01-15T10:30:00Z"
            },
            {
                "id": "def567890123456789",
                "message": "Add feature",
                "author_name": "MK Dev",
                "created_at": "2024-01-16T12:00:00Z"
            }
        ]);

        let commits = parse_commits(&data);
        assert_eq!(commits.len(), 2);
        assert_eq!(commits[0].sha, "abc1234567890abcdef");
        assert_eq!(commits[0].message, "Fix login bug");
        assert_eq!(commits[0].author, "Sam Thomson");
        assert!(commits[0].authored_at.is_some());
    }

    #[test]
    fn test_parse_empty_commits() {
        let data = json!([]);
        let commits = parse_commits(&data);
        assert!(commits.is_empty());
    }

    #[test]
    fn test_parse_commits_missing_fields() {
        let data = json!([{ "id": "abc123" }]);
        let commits = parse_commits(&data);
        assert_eq!(commits.len(), 1);
        assert_eq!(commits[0].sha, "abc123");
        assert_eq!(commits[0].message, "");
        assert_eq!(commits[0].author, "Unknown");
    }

    #[test]
    fn test_api_base() {
        assert_eq!(api_base("https://gitlab.com"), "https://gitlab.com/api/v4");
        assert_eq!(
            api_base("https://gitlab.example.com"),
            "https://gitlab.example.com/api/v4"
        );
    }

    #[test]
    fn test_parse_gitlab_date() {
        let ts = parse_gitlab_date("2024-01-15T10:30:00Z");
        assert!(ts.is_some());
        assert!(ts.unwrap() > 0);

        assert!(parse_gitlab_date("invalid").is_none());
    }
}
