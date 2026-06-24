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
    /// Arbitrary key-value pairs for custom handler config.
    #[serde(default)]
    pub custom: Option<toml::Value>,
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

    /// Resolve the nsec: config value, then env var, then None (auto-generate).
    pub fn bot_nsec(&self) -> Option<String> {
        // Config takes priority, then env var.
        if let Some(ref n) = self.bot.nsec {
            if !n.is_empty() && n != "auto" {
                return Some(n.clone());
            }
        }
        std::env::var("NSEC").ok().filter(|s| !s.is_empty())
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_config() {
        let config = BotConfig::default();
        assert!(config.bot_nsec().is_none());
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
}
