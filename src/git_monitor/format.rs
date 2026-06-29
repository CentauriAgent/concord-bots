// =============================================================================
// git_monitor/format.rs — Message formatters for commits and releases
// =============================================================================

use crate::git_monitor::store::Subscription;

/// A commit to format for announcement.
#[derive(Debug, Clone)]
pub struct CommitInfo {
    pub sha: String,
    pub message: String,
    pub author: String,
    pub authored_at: Option<i64>, // unix ts
}

/// A release to format for announcement.
#[derive(Debug, Clone)]
pub struct ReleaseInfo {
    pub tag: String,
    pub name: Option<String>,
    pub body: Option<String>,
    pub html_url: String,
}

/// Short SHA (first 7 chars).
fn short_sha(sha: &str) -> &str {
    if sha.len() >= 7 {
        &sha[..7]
    } else {
        sha
    }
}

/// Shorten an author name (first + last initial, or first 15 chars).
fn short_author(name: &str) -> String {
    let name = name.trim();
    if name.is_empty() {
        return "Unknown".to_string();
    }
    let parts: Vec<&str> = name.split_whitespace().collect();
    if parts.len() >= 2 {
        format!("{} {}.", parts[0], &parts[1][..1])
    } else if name.len() > 15 {
        name[..15].to_string()
    } else {
        name.to_string()
    }
}

/// Human-readable "X ago" from a timestamp.
fn time_ago(ts: Option<i64>) -> String {
    let ts = match ts {
        Some(t) => t,
        None => return "unknown time".to_string(),
    };
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs() as i64;
    let diff = now - ts;
    if diff < 0 {
        return "just now".to_string();
    }
    if diff < 60 {
        return format!("{} min ago", diff / 60);
    }
    if diff < 3600 {
        return format!("{} min ago", diff / 60);
    }
    if diff < 86400 {
        return format!("{}h ago", diff / 3600);
    }
    format!("{}d ago", diff / 86400)
}

/// Format new commits announcement.
///
/// - 1 commit: detailed single-commit format
/// - 2-3 commits: batched list
/// - >3 commits: summary
pub fn format_commits(sub: &Subscription, commits: &[CommitInfo]) -> String {
    let branch = "main"; // default branch
    let slug = &sub.full_slug;
    let host = sub.host;

    match commits.len() {
        0 => String::new(),
        1 => {
            let c = &commits[0];
            let first_line = c.message.lines().next().unwrap_or("(no message)");
            format!(
                "📦 {} · {}\n{} {}\n{} · {}\n{}/{}/commit/{}",
                slug,
                branch,
                short_sha(&c.sha),
                first_line,
                short_author(&c.author),
                time_ago(c.authored_at),
                host.base_url(),
                slug,
                c.sha,
            )
        }
        n if n <= 3 => {
            let mut lines = vec![format!(
                "📦 {} · {} · {} new commits",
                slug, branch, n
            )];
            for c in commits {
                let first_line = c.message.lines().next().unwrap_or("(no message)");
                lines.push(format!(
                    "• {} {} — {}",
                    short_sha(&c.sha),
                    first_line,
                    short_author(&c.author)
                ));
            }
            lines.push(format!("{}/{}/commits/{}", host.base_url(), slug, branch));
            lines.join("\n")
        }
        _ => {
            let latest = commits.last().unwrap();
            let first_line = latest.message.lines().next().unwrap_or("(no message)");
            format!(
                "📦 {} · {} · {} new commits\nLatest: {} {} ({})\n{}/{}/commits/{}",
                slug,
                branch,
                commits.len(),
                short_sha(&latest.sha),
                first_line,
                short_author(&latest.author),
                host.base_url(),
                slug,
                branch,
            )
        }
    }
}

/// Format a new release announcement.
pub fn format_release(sub: &Subscription, release: &ReleaseInfo) -> String {
    let slug = &sub.full_slug;
    let host = sub.host;

    let mut body = String::new();
    if let Some(ref name) = release.name {
        if !name.is_empty() && name != &release.tag {
            body.push_str(&format!("\"{}\"\n", name));
        }
    }

    if let Some(ref notes) = release.body {
        let trimmed = notes.trim();
        if !trimmed.is_empty() {
            if trimmed.len() > 200 {
                body.push_str(&format!("{}…", &trimmed[..200]));
            } else {
                body.push_str(trimmed);
            }
        }
    }

    let url = if release.html_url.is_empty() {
        format!("{}/{}/releases/tag/{}", host.base_url(), slug, release.tag)
    } else {
        release.html_url.clone()
    };

    if body.is_empty() {
        format!("🚀 {} · {}\n{}", slug, release.tag, url)
    } else {
        format!("🚀 {} · {}\n{}\n{}", slug, release.tag, body, url)
    }
}

// -----------------------------------------------------------------------------
// Tests
// -----------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::git_monitor::store::RepoHost;

    fn mock_sub(host: RepoHost) -> Subscription {
        Subscription {
            id: 1,
            channel_id: "ch1".to_string(),
            host,
            owner: "soapbox-pub".to_string(),
            repo: "agora".to_string(),
            full_slug: "soapbox-pub/agora".to_string(),
            added_by: "npub1test".to_string(),
            added_at: 0,
            last_commit_sha: None,
            last_release_tag: None,
            last_poll_at: None,
        }
    }

    fn mock_commit(sha: &str, msg: &str, author: &str) -> CommitInfo {
        CommitInfo {
            sha: sha.to_string(),
            message: msg.to_string(),
            author: author.to_string(),
            authored_at: Some(1700000000),
        }
    }

    #[test]
    fn test_single_commit() {
        let sub = mock_sub(RepoHost::GitHub);
        let commit = mock_commit("abc1234567", "Fix login redirect loop", "Sam Thomson");
        let result = format_commits(&sub, &[commit]);
        assert!(result.contains("soapbox-pub/agora"));
        assert!(result.contains("abc1234"));
        assert!(result.contains("Fix login redirect loop"));
        assert!(result.contains("Sam T."));
        assert!(result.contains("github.com"));
    }

    #[test]
    fn test_multi_commit() {
        let sub = mock_sub(RepoHost::GitHub);
        let commits = vec![
            mock_commit("abc1234", "Fix login", "Sam Thomson"),
            mock_commit("def5678", "Add dark mode", "MK Dev"),
            mock_commit("9abcdef", "Bump deps", "Sam Thomson"),
        ];
        let result = format_commits(&sub, &commits);
        assert!(result.contains("3 new commits"));
        assert!(result.contains("abc1234"));
        assert!(result.contains("def5678"));
        assert!(result.contains("9abcdef"));
        assert!(result.contains("/commits/main"));
    }

    #[test]
    fn test_many_commits_summary() {
        let sub = mock_sub(RepoHost::GitLab);
        let commits: Vec<CommitInfo> = (0..14)
            .map(|i| mock_commit(&format!("sha{:07x}", i), &format!("Commit {}", i), "Dev"))
            .collect();
        let result = format_commits(&sub, &commits);
        assert!(result.contains("14 new commits"));
        assert!(result.contains("Latest:"));
        assert!(result.contains("gitlab.com"));
        assert!(result.contains("/commits/main"));
    }

    #[test]
    fn test_empty_commits() {
        let sub = mock_sub(RepoHost::GitHub);
        let result = format_commits(&sub, &[]);
        assert!(result.is_empty());
    }

    #[test]
    fn test_release_format() {
        let sub = mock_sub(RepoHost::GitHub);
        let release = ReleaseInfo {
            tag: "v2.4.0".to_string(),
            name: Some("Venezuela translation fixes".to_string()),
            body: Some("Fix missing translations\nDark mode improvements".to_string()),
            html_url: "https://github.com/soapbox-pub/agora/releases/tag/v2.4.0".to_string(),
        };
        let result = format_release(&sub, &release);
        assert!(result.contains("v2.4.0"));
        assert!(result.contains("Venezuela translation fixes"));
        assert!(result.contains("releases/tag/v2.4.0"));
    }

    #[test]
    fn test_release_truncation() {
        let sub = mock_sub(RepoHost::GitHub);
        let long_body = "A".repeat(300);
        let release = ReleaseInfo {
            tag: "v1.0".to_string(),
            name: None,
            body: Some(long_body),
            html_url: String::new(),
        };
        let result = format_release(&sub, &release);
        assert!(result.contains("…"));
        assert!(result.contains("releases/tag/v1.0"));
    }

    #[test]
    fn test_short_sha() {
        assert_eq!(short_sha("abc1234567890"), "abc1234");
        assert_eq!(short_sha("short"), "short");
    }

    #[test]
    fn test_short_author() {
        assert_eq!(short_author("Sam Thomson"), "Sam T.");
        assert_eq!(short_author("MK"), "MK");
        assert_eq!(short_author(""), "Unknown");
        assert_eq!(short_author("  spaces  "), "spaces");
    }

    #[test]
    fn test_gitlab_commit_url() {
        let sub = mock_sub(RepoHost::GitLab);
        let commit = mock_commit("abc1234567", "Test", "Dev");
        let result = format_commits(&sub, &[commit]);
        assert!(result.contains("gitlab.com/soapbox-pub/agora/commit/abc1234567"));
    }
}
