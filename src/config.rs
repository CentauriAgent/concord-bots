// =============================================================================
// config.rs — TOML config loader (STABLE — do not edit)
// =============================================================================
//
// Loads configuration from config/bot.toml (or BOT_CONFIG env var).
// See config/bot.toml.example for a template.

use anyhow::{Context, Result};
use serde::Deserialize;
use std::path::PathBuf;

/// Top-level configuration loaded from bot.toml.
#[derive(Debug, Clone, Deserialize, Default)]
pub struct BotConfig {
    pub bot: BotSection,
    #[serde(default)]
    pub auth: AuthSection,
    #[serde(default)]
    pub communities: CommunitiesSection,
    #[serde(default)]
    pub scheduling: SchedulingSection,
    /// Feature flags for command groups.
    #[serde(default)]
    pub features: FeaturesSection,
    /// Cashu wallet configuration.
    #[serde(default)]
    pub wallet: WalletSection,
    /// Community engagement settings.
    #[serde(default)]
    pub community_settings: CommunitySettingsSection,
    /// Git repo monitor configuration.
    #[serde(default)]
    pub git_monitor: GitMonitorSection,
    /// Arbitrary key-value pairs for custom handler config.
    #[serde(default)]
    pub custom: Option<toml::Value>,
}

// -----------------------------------------------------------------------------
// Features section
// -----------------------------------------------------------------------------

/// Command groups that can be toggled via `[features]` in bot.toml.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Feature {
    Utility,
    Fun,
    Community,
    Nostr,
    Ai,
    Moderation,
    GitMonitor,
}

/// Feature flags for command groups. All default to `true` except `ai`.
#[derive(Debug, Clone, Deserialize)]
pub struct FeaturesSection {
    #[serde(default = "default_true")]
    pub utility: bool,
    #[serde(default = "default_true")]
    pub fun: bool,
    #[serde(default = "default_true")]
    pub community: bool,
    #[serde(default = "default_true")]
    pub nostr: bool,
    #[serde(default)]
    pub ai: bool,
    #[serde(default = "default_true")]
    pub moderation: bool,
    #[serde(default = "default_true")]
    pub git_monitor: bool,
}

impl Default for FeaturesSection {
    fn default() -> Self {
        Self {
            utility: true,
            fun: true,
            community: true,
            nostr: true,
            ai: false,
            moderation: true,
            git_monitor: true,
        }
    }
}

impl FeaturesSection {
    /// Check if a feature group is enabled.
    pub fn is_enabled(&self, feature: Feature) -> bool {
        match feature {
            Feature::Utility => self.utility,
            Feature::Fun => self.fun,
            Feature::Community => self.community,
            Feature::Nostr => self.nostr,
            Feature::Ai => self.ai,
            Feature::Moderation => self.moderation,
            Feature::GitMonitor => self.git_monitor,
        }
    }
}

fn default_true() -> bool {
    true
}

#[derive(Debug, Clone, Deserialize, Default)]
pub struct BotSection {
    /// The bot's nsec private key. If omitted, checks NSEC env var.
    /// If neither is set, the SDK auto-generates and persists one.
    pub nsec: Option<String>,

    /// Invite policy: "public", "whitelist", or "manual" (default).
    #[serde(default)]
    pub invite_policy: String,

    /// npubs allowed to invite the bot (only used with invite_policy = "whitelist").
    #[serde(default)]
    pub whitelist: Vec<String>,

    /// Display name for the bot profile (optional).
    pub display_name: Option<String>,

    /// Bot profile picture URL (optional).
    pub picture: Option<String>,

    /// About text for bot profile (optional).
    pub about: Option<String>,
}

// -----------------------------------------------------------------------------
// Auth section
// -----------------------------------------------------------------------------

/// Authorization configuration.
///
/// Set `owner` to enable the built-in auth system. When not configured,
/// all commands are public (backward-compatible with pre-auth bots).
#[derive(Debug, Clone, Deserialize)]
pub struct AuthSection {
    /// Bot owner's npub. When set, the auth system is enabled.
    pub owner: Option<String>,

    /// Initial authorized npubs (seed list from config).
    #[serde(default)]
    pub authorized: Vec<String>,

    /// Save the authorized list to a state file across restarts.
    #[serde(default = "default_persist_true")]
    pub persist: bool,

    /// Where to persist authorized users (relative to cwd or absolute).
    #[serde(default = "default_state_file")]
    pub state_file: String,
}

impl Default for AuthSection {
    fn default() -> Self {
        Self {
            owner: None,
            authorized: Vec::new(),
            persist: true,
            state_file: default_state_file(),
        }
    }
}

fn default_persist_true() -> bool {
    true
}

fn default_state_file() -> String {
    "auth_state.json".to_string()
}

// -----------------------------------------------------------------------------
// Communities section
// -----------------------------------------------------------------------------

#[derive(Debug, Clone, Deserialize, Default)]
pub struct CommunitiesSection {
    /// Community IDs to join on startup.
    #[serde(default)]
    pub join: Vec<String>,
}

#[derive(Debug, Clone, Deserialize, Default)]
pub struct SchedulingSection {
    /// Default interval in seconds for scheduled tasks (if not specified per-task).
    #[serde(default)]
    pub default_interval_secs: Option<u64>,
}

/// Invite policy enum for internal use.
#[derive(Debug, Clone)]
pub enum InvitePolicyConfig {
    Public,
    Whitelist(Vec<String>),
    Manual,
}

impl BotConfig {
    /// Load config from file, falling back to defaults.
    pub fn load() -> Result<Self> {
        let path = std::env::var("BOT_CONFIG")
            .map(PathBuf::from)
            .unwrap_or_else(|_| {
                // Look for config/bot.toml relative to the current directory.
                let candidate = PathBuf::from("config/bot.toml");
                if candidate.exists() {
                    candidate
                } else {
                    PathBuf::from("bot.toml")
                }
            });

        if path.exists() {
            tracing::info!("Loading config from {}", path.display());
            let contents = std::fs::read_to_string(&path)
                .with_context(|| format!("Failed to read config file: {}", path.display()))?;
            let config: BotConfig = toml::from_str(&contents)
                .with_context(|| format!("Failed to parse config file: {}", path.display()))?;
            Ok(config)
        } else {
            tracing::warn!(
                "No config file found at {} — using defaults (auto-generate identity, manual invites)",
                path.display()
            );
            Ok(BotConfig::default())
        }
    }

    /// Resolve the nsec: config value, then env var, then SDK-persisted file.
    pub fn bot_nsec(&self) -> Option<String> {
        // Config takes priority.
        if let Some(ref n) = self.bot.nsec {
            if !n.is_empty() && n != "auto" {
                return Some(n.clone());
            }
        }
        // Then env var.
        if let Ok(v) = std::env::var("NSEC") {
            if !v.is_empty() {
                return Some(v);
            }
        }
        // Then SDK's default identity file (auto-generated on first run).
        if let Some(home) = std::env::var_os("HOME") {
            let sdk_path = std::path::Path::new(&home)
                .join(".local/share/io.vectorapp/sdk/identity.nsec");
            if let Ok(saved) = std::fs::read_to_string(&sdk_path) {
                let saved = saved.trim();
                if saved.starts_with("nsec1") {
                    return Some(saved.to_string());
                }
            }
        }
        None
    }

    /// Parse the invite policy from config.
    pub fn invite_policy(&self) -> InvitePolicyConfig {
        match self.bot.invite_policy.as_str() {
            "public" => InvitePolicyConfig::Public,
            "whitelist" if !self.bot.whitelist.is_empty() => {
                InvitePolicyConfig::Whitelist(self.bot.whitelist.clone())
            }
            _ => InvitePolicyConfig::Manual,
        }
    }

    /// Log a summary of the config (redacting secrets).
    pub fn log_summary(&self) {
        tracing::info!("Config summary:");
        tracing::info!("  nsec: {}", if self.bot_nsec().is_some() { "provided" } else { "auto-generate" });
        tracing::info!("  invite_policy: {}", self.bot.invite_policy);
        if let Some(ref owner) = self.auth.owner {
            tracing::info!("  auth owner: {}", owner);
            tracing::info!("  auth authorized (seed): {}", self.auth.authorized.len());
        } else {
            tracing::info!("  auth: disabled (no owner configured)");
        }
        tracing::info!("  communities to join: {}", self.communities.join.len());
        if let Some(ref name) = self.bot.display_name {
            tracing::info!("  display_name: {}", name);
        }
        tracing::info!("  features:");
        tracing::info!(
            "    utility: {}, fun: {}, community: {}, nostr: {}, ai: {}, moderation: {}, git_monitor: {}",
            self.features.utility,
            self.features.fun,
            self.features.community,
            self.features.nostr,
            self.features.ai,
            self.features.moderation,
            self.features.git_monitor
        );
        if self.git_monitor.enabled {
            tracing::info!(
                "  git_monitor: poll={}s, max_repos={}, github_token={}, gitlab_token={}",
                self.git_monitor.poll_interval_secs,
                self.git_monitor.max_repos_per_channel,
                if self.git_monitor.github_token.is_empty() { "none" } else { "set" },
                if self.git_monitor.gitlab_token.is_empty() { "none" } else { "set" },
            );
        }
    }

    /// Access a custom config value by path (e.g., "api_keys.github_token").
    pub fn custom_get(&self, key: &str) -> Option<&toml::Value> {
        let parts: Vec<&str> = key.split('.').collect();
        let mut current = self.custom.as_ref()?;
        for part in parts {
            current = current.as_table()?.get(part)?;
        }
        Some(current)
    }

    /// Get a custom string value.
    pub fn custom_string(&self, key: &str) -> Option<String> {
        self.custom_get(key)?.as_str().map(|s| s.to_string())
    }
}

// -----------------------------------------------------------------------------
// Wallet section
// -----------------------------------------------------------------------------

/// Cashu wallet configuration.
#[derive(Debug, Clone, Deserialize)]
pub struct WalletSection {
    /// Enable the Cashu wallet (default: false).
    #[serde(default)]
    pub enabled: bool,
    /// Mint URL for the Cashu mint.
    #[serde(default = "default_mint_url")]
    pub mint_url: String,
}

impl Default for WalletSection {
    fn default() -> Self {
        Self {
            enabled: false,
            mint_url: default_mint_url(),
        }
    }
}

fn default_mint_url() -> String {
    "https://mint.minibits.cash/Bitcoin".to_string()
}

// -----------------------------------------------------------------------------
// Community settings section
// -----------------------------------------------------------------------------

/// Community engagement configuration (XP, giveaways, reputation).
/// All features default to enabled.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct CommunitySettingsSection {
    /// Enable XP gain from messages (default: true).
    #[serde(default = "default_true")]
    pub xp_enabled: bool,
    /// Enable giveaways (default: true).
    #[serde(default = "default_true")]
    pub giveaways_enabled: bool,
    /// Enable reputation system (default: true).
    #[serde(default = "default_true")]
    pub reputation_enabled: bool,
}

// -----------------------------------------------------------------------------
// Git Monitor section
// -----------------------------------------------------------------------------

/// Git repo monitor configuration.
#[derive(Debug, Clone, Deserialize)]
pub struct GitMonitorSection {
    /// Master switch (default: true).
    #[serde(default = "default_true")]
    pub enabled: bool,
    /// Poll interval in seconds (default: 300 = 5 min).
    #[serde(default = "default_poll_interval")]
    pub poll_interval_secs: u64,
    /// Optional GitHub PAT for higher rate limits.
    #[serde(default)]
    pub github_token: String,
    /// Optional GitLab token.
    #[serde(default)]
    pub gitlab_token: String,
    /// GitLab base URL (for self-hosted instances).
    #[serde(default = "default_gitlab_host")]
    pub gitlab_host: String,
    /// Default branch if not auto-detected.
    #[serde(default = "default_branch_main")]
    pub default_branch: String,
    /// Post commit announcements.
    #[serde(default = "default_true")]
    pub post_commits: bool,
    /// Post release announcements.
    #[serde(default = "default_true")]
    pub post_releases: bool,
    /// Spam guard: max repos per channel.
    #[serde(default = "default_max_repos")]
    pub max_repos_per_channel: usize,
    /// Delay between API calls in a poll cycle (ms).
    #[serde(default = "default_polite_sleep_ms")]
    pub polite_sleep_ms: u64,
}

impl Default for GitMonitorSection {
    fn default() -> Self {
        Self {
            enabled: true,
            poll_interval_secs: 300,
            github_token: String::new(),
            gitlab_token: String::new(),
            gitlab_host: default_gitlab_host(),
            default_branch: default_branch_main(),
            post_commits: true,
            post_releases: true,
            max_repos_per_channel: 10,
            polite_sleep_ms: 500,
        }
    }
}

fn default_poll_interval() -> u64 {
    300
}

fn default_gitlab_host() -> String {
    "https://gitlab.com".to_string()
}

fn default_branch_main() -> String {
    "main".to_string()
}

fn default_max_repos() -> usize {
    10
}

fn default_polite_sleep_ms() -> u64 {
    500
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_config() {
        let config = BotConfig::default();
        // bot_nsec() may find the SDK's persisted identity file, so only check
        // that the config field itself is None.
        assert!(config.bot.nsec.is_none());
        assert!(matches!(config.invite_policy(), InvitePolicyConfig::Manual));
    }

    #[test]
    fn test_parse_toml() {
        let toml_str = r#"
[bot]
nsec = "nsec1test..."
invite_policy = "public"
display_name = "Test Bot"

[auth]
owner = "npub1owner..."
authorized = ["npub1friend..."]

[communities]
join = ["abc123"]
"#;
        let config: BotConfig = toml::from_str(toml_str).unwrap();
        assert_eq!(config.bot.nsec.as_deref(), Some("nsec1test..."));
        assert_eq!(config.bot.invite_policy, "public");
        assert_eq!(config.bot.display_name.as_deref(), Some("Test Bot"));
        assert_eq!(config.auth.owner.as_deref(), Some("npub1owner..."));
        assert_eq!(config.auth.authorized, vec!["npub1friend..."]);
        assert_eq!(config.communities.join, vec!["abc123"]);
    }

    #[test]
    fn test_auth_defaults() {
        let config = BotConfig::default();
        assert!(config.auth.owner.is_none());
        assert!(config.auth.authorized.is_empty());
        assert!(config.auth.persist); // defaults to true
        assert_eq!(config.auth.state_file, "auth_state.json");
    }

    #[test]
    fn test_feature_defaults() {
        let config = BotConfig::default();
        assert!(config.features.utility);
        assert!(config.features.fun);
        assert!(config.features.community);
        assert!(config.features.nostr);
        assert!(!config.features.ai); // disabled by default
        assert!(config.features.moderation);
        assert!(config.features.git_monitor); // enabled by default
    }

    #[test]
    fn test_feature_is_enabled() {
        let config = BotConfig::default();
        assert!(config.features.is_enabled(Feature::Utility));
        assert!(config.features.is_enabled(Feature::Fun));
        assert!(!config.features.is_enabled(Feature::Ai));
        assert!(config.features.is_enabled(Feature::GitMonitor));
    }

    #[test]
    fn test_git_monitor_defaults() {
        let config = BotConfig::default();
        assert!(config.git_monitor.enabled);
        assert_eq!(config.git_monitor.poll_interval_secs, 300);
        assert_eq!(config.git_monitor.max_repos_per_channel, 10);
        assert_eq!(config.git_monitor.default_branch, "main");
        assert_eq!(config.git_monitor.gitlab_host, "https://gitlab.com");
        assert!(config.git_monitor.post_commits);
        assert!(config.git_monitor.post_releases);
    }

    #[test]
    fn test_feature_override() {
        let toml_str = r#"
[bot]
npub = "test"

[features]
utility = false
ai = true
"#;
        let config: BotConfig = toml::from_str(toml_str).unwrap();
        assert!(!config.features.utility);
        assert!(config.features.ai);
        assert!(config.features.fun); // still default
    }
}
