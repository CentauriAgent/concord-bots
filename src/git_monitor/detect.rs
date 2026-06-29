// =============================================================================
// git_monitor/detect.rs — URL/host parsing + slug normalization
// =============================================================================
//
// Parses user input into a normalized (host, owner, repo) tuple.
// Supports 5 input forms per the plan:
//   1. https://github.com/owner/repo
//   2. github.com/owner/repo
//   3. owner/repo (GitHub implied)
//   4. gitlab owner/repo (explicit host prefix)
//   5. https://gitlab.com/soapbox-pub/agora

use crate::git_monitor::store::RepoHost;

/// Parsed repo identification.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParsedRepo {
    pub host: RepoHost,
    pub owner: String,
    pub repo: String,
}

impl ParsedRepo {
    /// Full slug: "owner/repo"
    pub fn slug(&self) -> String {
        format!("{}/{}", self.owner, self.repo)
    }

    /// Full URL: "https://github.com/owner/repo"
    pub fn url(&self) -> String {
        format!("{}/{}/{}", self.host.base_url(), self.owner, self.repo)
    }
}

/// Parse user input into a ParsedRepo.
///
/// Accepted forms:
/// 1. `https://github.com/owner/repo`
/// 2. `github.com/owner/repo`
/// 3. `owner/repo` (GitHub implied)
/// 4. `gitlab owner/repo` (explicit host prefix)
/// 5. `https://gitlab.com/soapbox-pub/agora`
pub fn parse_repo(input: &str) -> Result<ParsedRepo, String> {
    let input = input.trim();

    if input.is_empty() {
        return Err("Empty input. Usage: !git add <url|owner/repo>".to_string());
    }

    // Form 4: explicit host prefix — "github owner/repo" or "gitlab owner/repo"
    if let Some(stripped) = try_explicit_prefix(input) {
        return Ok(stripped);
    }

    // Forms 1, 2, 5: URL-based detection
    // Strip protocol if present
    let cleaned = input
        .strip_prefix("https://")
        .or_else(|| input.strip_prefix("http://"))
        .unwrap_or(input);

    // Strip trailing slashes, .git suffix
    let cleaned = cleaned.trim_end_matches('/');
    let cleaned = cleaned.trim_end_matches(".git");

    // Check for host indicators in the path
    if cleaned.starts_with("github.com/") {
        return parse_slug_after_host(cleaned, "github.com/", RepoHost::GitHub);
    }

    // Check for gitlab — could be gitlab.com or self-hosted gitlab.*
    if cleaned.starts_with("gitlab.com/") {
        return parse_slug_after_host(cleaned, "gitlab.com/", RepoHost::GitLab);
    }
    // Generic gitlab detection: gitlab.* /host/owner/repo
    if cleaned.starts_with("gitlab.") && cleaned.contains('/') {
        // Self-hosted: e.g. "gitlab.example.com/owner/repo"
        return parse_gitlab_self_hosted(cleaned);
    }

    // Form 3: bare "owner/repo" → GitHub default
    if cleaned.contains('/') && !cleaned.contains(' ') && !cleaned.starts_with('/') {
        return parse_bare_slug(cleaned);
    }

    Err(format!(
        "Could not parse \"{}\". Usage: !git add <url|owner/repo>\n\
         Examples: !git add https://github.com/owner/repo, !git add owner/repo, !git add gitlab owner/repo",
        input
    ))
}

/// Try parsing "github owner/repo" or "gitlab owner/repo" prefix form.
fn try_explicit_prefix(input: &str) -> Option<ParsedRepo> {
    // Check for explicit host prefix case-insensitively, but preserve original case of slug
    let lower = input.to_lowercase();

    if let Some(_rest) = lower.strip_prefix("github ") {
        // Find the space in the original input and take everything after it
        let rest = input[input.find(' ')? + 1..].trim();
        return parse_bare_slug(rest).ok();
    }
    if let Some(_rest) = lower.strip_prefix("gitlab ") {
        let rest = input[input.find(' ')? + 1..].trim();
        return parse_bare_slug(rest)
            .ok()
            .map(|mut r| {
                r.host = RepoHost::GitLab;
                r
            });
    }

    None
}

/// Parse "owner/repo" bare slug → GitHub by default.
fn parse_bare_slug(s: &str) -> Result<ParsedRepo, String> {
    let parts: Vec<&str> = s.split('/').collect();
    if parts.len() < 2 || parts[0].is_empty() || parts[1].is_empty() {
        return Err(format!("Invalid slug \"{}\". Expected: owner/repo", s));
    }

    // Handle case where there might be extra path segments — take first two
    let owner = parts[0].to_string();
    let repo = parts[1].to_string();

    // Basic validation
    if owner.contains(' ') || repo.contains(' ') {
        return Err("Owner and repo must not contain spaces.".to_string());
    }

    Ok(ParsedRepo {
        host: RepoHost::GitHub,
        owner,
        repo,
    })
}

/// Parse after a known host prefix like "github.com/owner/repo".
fn parse_slug_after_host(
    cleaned: &str,
    host_prefix: &str,
    host: RepoHost,
) -> Result<ParsedRepo, String> {
    let rest = &cleaned[host_prefix.len()..];
    let parts: Vec<&str> = rest.split('/').collect();

    if parts.len() < 2 || parts[0].is_empty() || parts[1].is_empty() {
        return Err(format!(
            "Invalid repo path after {}. Expected: owner/repo",
            host_prefix
        ));
    }

    let owner = parts[0].to_string();
    let repo = parts[1].to_string();

    Ok(ParsedRepo { host, owner, repo })
}

/// Parse self-hosted GitLab URL like "gitlab.example.com/owner/repo".
fn parse_gitlab_self_hosted(cleaned: &str) -> Result<ParsedRepo, String> {
    // Find the first '/' — everything before is the host, after is owner/repo
    let slash_idx = cleaned
        .find('/')
        .ok_or_else(|| "Invalid GitLab URL".to_string())?;

    let rest = &cleaned[slash_idx + 1..];
    let parts: Vec<&str> = rest.split('/').collect();

    if parts.len() < 2 || parts[0].is_empty() || parts[1].is_empty() {
        return Err("Invalid repo path after GitLab host. Expected: owner/repo".to_string());
    }

    Ok(ParsedRepo {
        host: RepoHost::GitLab,
        owner: parts[0].to_string(),
        repo: parts[1].to_string(),
    })
}

// -----------------------------------------------------------------------------
// Tests
// -----------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_form1_https_github() {
        let r = parse_repo("https://github.com/owner/repo").unwrap();
        assert_eq!(r.host, RepoHost::GitHub);
        assert_eq!(r.owner, "owner");
        assert_eq!(r.repo, "repo");
        assert_eq!(r.slug(), "owner/repo");
        assert_eq!(r.url(), "https://github.com/owner/repo");
    }

    #[test]
    fn test_form2_bare_github_com() {
        let r = parse_repo("github.com/owner/repo").unwrap();
        assert_eq!(r.host, RepoHost::GitHub);
        assert_eq!(r.owner, "owner");
        assert_eq!(r.repo, "repo");
    }

    #[test]
    fn test_form3_bare_slug() {
        let r = parse_repo("owner/repo").unwrap();
        assert_eq!(r.host, RepoHost::GitHub);
        assert_eq!(r.owner, "owner");
        assert_eq!(r.repo, "repo");
    }

    #[test]
    fn test_form4_explicit_prefix_github() {
        let r = parse_repo("github owner/repo").unwrap();
        assert_eq!(r.host, RepoHost::GitHub);
        assert_eq!(r.owner, "owner");
        assert_eq!(r.repo, "repo");
    }

    #[test]
    fn test_form4_explicit_prefix_gitlab() {
        let r = parse_repo("gitlab soapbox-pub/agora").unwrap();
        assert_eq!(r.host, RepoHost::GitLab);
        assert_eq!(r.owner, "soapbox-pub");
        assert_eq!(r.repo, "agora");
    }

    #[test]
    fn test_form5_https_gitlab() {
        let r = parse_repo("https://gitlab.com/soapbox-pub/agora").unwrap();
        assert_eq!(r.host, RepoHost::GitLab);
        assert_eq!(r.owner, "soapbox-pub");
        assert_eq!(r.repo, "agora");
    }

    #[test]
    fn test_self_hosted_gitlab() {
        let r = parse_repo("https://gitlab.example.com/myorg/myrepo").unwrap();
        assert_eq!(r.host, RepoHost::GitLab);
        assert_eq!(r.owner, "myorg");
        assert_eq!(r.repo, "myrepo");
    }

    #[test]
    fn test_trailing_slash() {
        let r = parse_repo("https://github.com/owner/repo/").unwrap();
        assert_eq!(r.owner, "owner");
        assert_eq!(r.repo, "repo");
    }

    #[test]
    fn test_dot_git_suffix() {
        let r = parse_repo("https://github.com/owner/repo.git").unwrap();
        assert_eq!(r.owner, "owner");
        assert_eq!(r.repo, "repo");
    }

    #[test]
    fn test_case_insensitive_prefix() {
        let r = parse_repo("GitHub Owner/Repo").unwrap();
        assert_eq!(r.host, RepoHost::GitHub);
        assert_eq!(r.owner, "Owner");
        assert_eq!(r.repo, "Repo");
    }

    #[test]
    fn test_empty_input() {
        assert!(parse_repo("").is_err());
        assert!(parse_repo("   ").is_err());
    }

    #[test]
    fn test_invalid_no_slash() {
        assert!(parse_repo("justsometext").is_err());
    }

    #[test]
    fn test_url_method() {
        let r = parse_repo("gitlab org/repo").unwrap();
        assert_eq!(r.url(), "https://gitlab.com/org/repo");

        let r = parse_repo("owner/repo").unwrap();
        assert_eq!(r.url(), "https://github.com/owner/repo");
    }

    #[test]
    fn test_extra_path_segments() {
        // Should take first two segments
        let r = parse_repo("https://github.com/owner/repo/tree/main").unwrap();
        assert_eq!(r.owner, "owner");
        assert_eq!(r.repo, "repo");
    }
}
