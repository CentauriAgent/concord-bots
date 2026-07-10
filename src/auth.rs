// =============================================================================
// auth.rs — Authorization manager (STABLE CORE UTILITY)
// =============================================================================
//
// Manages bot owner and authorized user permissions.
// Provides AuthLevel checking for command handlers.
//
// When the `[auth]` section is present in bot.toml with an `owner` npub,
// the auth system is enabled and commands can be gated by permission level.
// If not configured, the auth system is disabled and all commands are public.

use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::{Arc, RwLock};

// -----------------------------------------------------------------------------
// AuthLevel
// -----------------------------------------------------------------------------

/// Permission level for command access.
///
/// Ordered from least to most privileged. Comparisons use `PartialOrd`:
/// `Owner > Authorized > Public`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum AuthLevel {
    /// Anyone can use the command.
    Public,
    /// Only owner + explicitly authorized users.
    Authorized,
    /// Only the configured bot owner.
    Owner,
}

impl AuthLevel {
    pub fn as_str(&self) -> &'static str {
        match self {
            AuthLevel::Public => "public",
            AuthLevel::Authorized => "authorized",
            AuthLevel::Owner => "owner",
        }
    }
}

impl std::fmt::Display for AuthLevel {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

// -----------------------------------------------------------------------------
// AuthState (serialization)
// -----------------------------------------------------------------------------

/// Serializable state for persisting the authorized list.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
struct AuthState {
    authorized: Vec<String>,
}

// -----------------------------------------------------------------------------
// AuthManager
// -----------------------------------------------------------------------------

/// Thread-safe authorization manager.
///
/// Holds the owner npub and a set of authorized npubs. When `persist` is true,
/// the authorized set is saved to a JSON state file on every mutation and
/// loaded on construction.
///
/// Clone freely — internally backed by `Arc<RwLock<...>>`.
#[derive(Clone)]
pub struct AuthManager {
    inner: Arc<RwLock<AuthManagerInner>>,
}

struct AuthManagerInner {
    owner: String,
    authorized: HashSet<String>,
    persist: bool,
    state_file: PathBuf,
}

impl AuthManager {
    /// Create a new AuthManager.
    ///
    /// If `persist` is true and the state file exists, authorized users are
    /// loaded from it (merging with the seed list from config).
    pub fn new(
        owner: &str,
        initial_authorized: &[String],
        persist: bool,
        state_file: PathBuf,
    ) -> anyhow::Result<Self> {
        let mut authorized: HashSet<String> = initial_authorized.iter().cloned().collect();

        // Load persisted state if available.
        if persist && state_file.exists() {
            match std::fs::read_to_string(&state_file) {
                Ok(contents) => {
                    if let Ok(state) = serde_json::from_str::<AuthState>(&contents) {
                        for npub in state.authorized {
                            authorized.insert(npub);
                        }
                        tracing::debug!(
                            "Loaded {} authorized users from {}",
                            authorized.len(),
                            state_file.display()
                        );
                    }
                }
                Err(e) => {
                    tracing::warn!(
                        "Failed to read auth state file {}: {}",
                        state_file.display(),
                        e
                    );
                }
            }
        }

        Ok(Self {
            inner: Arc::new(RwLock::new(AuthManagerInner {
                owner: owner.to_string(),
                authorized,
                persist,
                state_file,
            })),
        })
    }

    // ---- Read operations --------------------------------------------------

    /// Check the auth level for a given npub.
    pub fn check(&self, npub: &str) -> AuthLevel {
        let inner = self.read();
        if npub == inner.owner {
            AuthLevel::Owner
        } else if inner.authorized.contains(npub) {
            AuthLevel::Authorized
        } else {
            AuthLevel::Public
        }
    }

    /// Returns true if the npub meets or exceeds the required auth level.
    pub fn has_permission(&self, npub: &str, required: AuthLevel) -> bool {
        self.check(npub) >= required
    }

    /// Check if an npub is the owner.
    pub fn is_owner(&self, npub: &str) -> bool {
        let inner = self.read();
        npub == inner.owner
    }

    /// Check if an npub is in the authorized set.
    pub fn is_authorized(&self, npub: &str) -> bool {
        let inner = self.read();
        inner.authorized.contains(npub)
    }

    /// List all authorized npubs (sorted alphabetically).
    pub fn list(&self) -> Vec<String> {
        let inner = self.read();
        let mut list: Vec<String> = inner.authorized.iter().cloned().collect();
        list.sort();
        list
    }

    /// Get the owner npub.
    pub fn owner(&self) -> String {
        self.read().owner.clone()
    }

    /// Number of authorized users (excluding owner).
    pub fn authorized_count(&self) -> usize {
        self.read().authorized.len()
    }

    // ---- Write operations -------------------------------------------------

    /// Add an npub to the authorized set. Returns true if newly inserted.
    /// Adding the owner is a no-op (they already have Owner-level access).
    pub fn add(&self, npub: &str) -> bool {
        let inserted = {
            let mut inner = self.write();
            if npub == inner.owner {
                return false;
            }
            inner.authorized.insert(npub.to_string())
        };
        if inserted {
            self.save();
        }
        inserted
    }

    /// Remove an npub from the authorized set. Returns true if it was present.
    pub fn remove(&self, npub: &str) -> bool {
        let removed = {
            let mut inner = self.write();
            inner.authorized.remove(npub)
        };
        if removed {
            self.save();
        }
        removed
    }

    // ---- Persistence ------------------------------------------------------

    /// Persist the authorized set to the state file.
    fn save(&self) {
        let inner = self.read();
        if !inner.persist {
            return;
        }

        let state = AuthState {
            authorized: inner.authorized.iter().cloned().collect(),
        };

        let json = match serde_json::to_string_pretty(&state) {
            Ok(j) => j,
            Err(e) => {
                tracing::error!("Failed to serialize auth state: {}", e);
                return;
            }
        };

        // Create parent directory if needed.
        if let Some(parent) = inner.state_file.parent() {
            let _ = std::fs::create_dir_all(parent);
        }

        if let Err(e) = std::fs::write(&inner.state_file, json) {
            tracing::error!(
                "Failed to write auth state to {}: {}",
                inner.state_file.display(),
                e
            );
        } else {
            tracing::debug!("Auth state saved to {}", inner.state_file.display());
        }
    }

    // ---- Lock helpers (recover from poison) -------------------------------

    fn read(&self) -> std::sync::RwLockReadGuard<'_, AuthManagerInner> {
        self.inner.read().unwrap_or_else(|e| e.into_inner())
    }

    fn write(&self) -> std::sync::RwLockWriteGuard<'_, AuthManagerInner> {
        self.inner.write().unwrap_or_else(|e| e.into_inner())
    }
}

// -----------------------------------------------------------------------------
// v2 Community Capability Helpers
// -----------------------------------------------------------------------------

/// Check if the bot has a specific capability in the current community.
/// Common capabilities: "manage_roles", "manage_channels", "kick", "ban", "create_invite"
pub async fn bot_has_capability(msg: &vector_sdk::IncomingMessage, capability: &str) -> bool {
    match msg.community() {
        Some(community) => match community.capabilities() {
            Ok(caps) => {
                let caps_str = caps.to_string();
                caps_str.contains(capability)
            }
            Err(_) => false,
        },
        None => false,
    }
}

// -----------------------------------------------------------------------------
// Tests
// -----------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp_state_file() -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "concord-auth-test-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let _ = std::fs::create_dir_all(&dir);
        dir.join("auth_state.json")
    }

    #[test]
    fn test_owner_is_owner() {
        let sf = tmp_state_file();
        let auth = AuthManager::new("npub1owner", &[], false, sf).unwrap();
        assert_eq!(auth.check("npub1owner"), AuthLevel::Owner);
        assert!(auth.is_owner("npub1owner"));
    }

    #[test]
    fn test_authorized_user() {
        let sf = tmp_state_file();
        let auth =
            AuthManager::new("npub1owner", &["npub1friend".to_string()], false, sf).unwrap();
        assert_eq!(auth.check("npub1friend"), AuthLevel::Authorized);
        assert!(auth.is_authorized("npub1friend"));
    }

    #[test]
    fn test_unknown_is_public() {
        let sf = tmp_state_file();
        let auth = AuthManager::new("npub1owner", &[], false, sf).unwrap();
        assert_eq!(auth.check("npub1random"), AuthLevel::Public);
    }

    #[test]
    fn test_add_and_remove() {
        let sf = tmp_state_file();
        let auth = AuthManager::new("npub1owner", &[], false, sf).unwrap();

        assert!(auth.add("npub1new"));
        assert_eq!(auth.check("npub1new"), AuthLevel::Authorized);

        // Duplicate add is a no-op.
        assert!(!auth.add("npub1new"));
        // Can't add owner to authorized set.
        assert!(!auth.add("npub1owner"));

        assert!(auth.remove("npub1new"));
        assert_eq!(auth.check("npub1new"), AuthLevel::Public);

        // Double remove is a no-op.
        assert!(!auth.remove("npub1new"));
    }

    #[test]
    fn test_has_permission() {
        let sf = tmp_state_file();
        let auth =
            AuthManager::new("npub1owner", &["npub1friend".to_string()], false, sf).unwrap();

        // Owner can do everything.
        assert!(auth.has_permission("npub1owner", AuthLevel::Owner));
        assert!(auth.has_permission("npub1owner", AuthLevel::Authorized));
        assert!(auth.has_permission("npub1owner", AuthLevel::Public));

        // Authorized can do Authorized + Public.
        assert!(!auth.has_permission("npub1friend", AuthLevel::Owner));
        assert!(auth.has_permission("npub1friend", AuthLevel::Authorized));
        assert!(auth.has_permission("npub1friend", AuthLevel::Public));

        // Public can only do Public.
        assert!(!auth.has_permission("npub1random", AuthLevel::Owner));
        assert!(!auth.has_permission("npub1random", AuthLevel::Authorized));
        assert!(auth.has_permission("npub1random", AuthLevel::Public));
    }

    #[test]
    fn test_list_sorted() {
        let sf = tmp_state_file();
        let auth = AuthManager::new(
            "npub1owner",
            &["npub1charlie".to_string(), "npub1alpha".to_string()],
            false,
            sf,
        )
        .unwrap();

        assert_eq!(auth.list(), vec!["npub1alpha", "npub1charlie"]);
    }

    #[test]
    fn test_persist_roundtrip() {
        let sf = tmp_state_file();

        {
            let auth = AuthManager::new("npub1owner", &[], true, sf.clone()).unwrap();
            auth.add("npub1persist1");
            auth.add("npub1persist2");
        }

        // Reload — should have persisted users.
        let auth = AuthManager::new("npub1owner", &[], true, sf.clone()).unwrap();
        assert!(auth.is_authorized("npub1persist1"));
        assert!(auth.is_authorized("npub1persist2"));
        assert_eq!(auth.authorized_count(), 2);

        let _ = std::fs::remove_file(&sf);
    }

    #[test]
    fn test_persist_disabled() {
        let sf = tmp_state_file();

        {
            let auth = AuthManager::new("npub1owner", &[], false, sf.clone()).unwrap();
            auth.add("npub1nopersist");
        }

        assert!(!sf.exists(), "state file should not exist when persist=false");
    }

    #[test]
    fn test_auth_level_ordering() {
        assert!(AuthLevel::Owner > AuthLevel::Authorized);
        assert!(AuthLevel::Authorized > AuthLevel::Public);
        assert!(AuthLevel::Owner > AuthLevel::Public);
    }

    #[test]
    fn test_display() {
        assert_eq!(AuthLevel::Public.to_string(), "public");
        assert_eq!(AuthLevel::Authorized.to_string(), "authorized");
        assert_eq!(AuthLevel::Owner.to_string(), "owner");
    }
}
