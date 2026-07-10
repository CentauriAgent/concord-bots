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
use std::collections::{HashMap, HashSet};
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

/// Serializable state for persisting authorized users.
/// Supports per-community scoping. Legacy format (flat `authorized` array)
/// is auto-migrated to `authorized_global`.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
struct AuthState {
    /// Legacy field — if present, migrated to authorized_global on load.
    #[serde(default)]
    authorized: Vec<String>,
    /// Globally authorized users (work in all communities).
    #[serde(default)]
    authorized_global: Vec<String>,
    /// Per-community authorized users: community_id → list of npubs.
    #[serde(default)]
    authorized_by_community: HashMap<String, Vec<String>>,
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
    /// Globally authorized (work in all communities + DMs).
    authorized_global: HashSet<String>,
    /// Per-community authorized: community_id → set of npubs.
    authorized_by_community: HashMap<String, HashSet<String>>,
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
        let mut authorized_global: HashSet<String> = initial_authorized.iter().cloned().collect();
        let mut authorized_by_community: HashMap<String, HashSet<String>> = HashMap::new();

        // Load persisted state if available.
        if persist && state_file.exists() {
            match std::fs::read_to_string(&state_file) {
                Ok(contents) => {
                    if let Ok(state) = serde_json::from_str::<AuthState>(&contents) {
                        // Migrate legacy flat authorized list to global
                        for npub in state.authorized {
                            authorized_global.insert(npub);
                        }
                        // Load global authorized
                        for npub in state.authorized_global {
                            authorized_global.insert(npub);
                        }
                        // Load per-community authorized
                        for (cid, npubs) in state.authorized_by_community {
                            let set: HashSet<String> = npubs.into_iter().collect();
                            authorized_by_community.insert(cid, set);
                        }
                        let total = authorized_global.len() + authorized_by_community.values().map(|s| s.len()).sum::<usize>();
                        tracing::debug!(
                            "Loaded {} authorized users ({} global, {} communities) from {}",
                            total, authorized_global.len(), authorized_by_community.len(),
                            state_file.display()
                        );
                    }
                }
                Err(e) => {
                    tracing::warn!("Failed to read auth state file {}: {}", state_file.display(), e);
                }
            }
        }

        Ok(Self {
            inner: Arc::new(RwLock::new(AuthManagerInner {
                owner: owner.to_string(),
                authorized_global,
                authorized_by_community,
                persist,
                state_file,
            })),
        })
    }

    // ---- Read operations --------------------------------------------------

    /// Check the auth level for a given npub in a specific community.
    /// `community_id` is None for DMs (only owner + global authorized pass).
    pub fn check(&self, npub: &str, community_id: Option<&str>) -> AuthLevel {
        let inner = self.read();
        if npub == inner.owner {
            AuthLevel::Owner
        } else if inner.authorized_global.contains(npub) {
            AuthLevel::Authorized
        } else if let Some(cid) = community_id {
            if let Some(set) = inner.authorized_by_community.get(cid) {
                if set.contains(npub) {
                    return AuthLevel::Authorized;
                }
            }
            AuthLevel::Public
        } else {
            AuthLevel::Public
        }
    }

    /// Returns true if the npub meets or exceeds the required auth level
    /// in the given community context.
    pub fn has_permission(&self, npub: &str, community_id: Option<&str>, required: AuthLevel) -> bool {
        self.check(npub, community_id) >= required
    }

    /// Check if an npub is the owner.
    pub fn is_owner(&self, npub: &str) -> bool {
        let inner = self.read();
        npub == inner.owner
    }

    /// Check if an npub is authorized in a specific community (or globally).
    pub fn is_authorized(&self, npub: &str, community_id: Option<&str>) -> bool {
        let inner = self.read();
        if inner.authorized_global.contains(npub) {
            return true;
        }
        if let Some(cid) = community_id {
            if let Some(set) = inner.authorized_by_community.get(cid) {
                return set.contains(npub);
            }
        }
        false
    }

    /// List authorized npubs for a specific community (includes global).
    /// Returns (global_list, community_list) tuple.
    pub fn list(&self, community_id: Option<&str>) -> (Vec<String>, Vec<String>) {
        let inner = self.read();
        let mut global: Vec<String> = inner.authorized_global.iter().cloned().collect();
        global.sort();

        let community: Vec<String> = if let Some(cid) = community_id {
            match inner.authorized_by_community.get(cid) {
                Some(set) => {
                    let mut v: Vec<String> = set.iter().cloned().collect();
                    v.sort();
                    v
                }
                None => Vec::new(),
            }
        } else {
            Vec::new()
        };

        (global, community)
    }

    /// Count total authorized users (global + all communities).
    pub fn authorized_count(&self) -> usize {
        let inner = self.read();
        inner.authorized_global.len()
            + inner.authorized_by_community.values().map(|s| s.len()).sum::<usize>()
    }

    /// Get the owner npub.
    pub fn owner(&self) -> String {
        self.read().owner.clone()
    }

    /// Get the owner npub as a reference (for comparison without cloning).
    pub fn owner_npub(&self) -> Option<String> {
        let owner = self.read().owner.clone();
        if owner.is_empty() { None } else { Some(owner) }
    }

    // ---- Write operations -------------------------------------------------

    /// Add an npub to the authorized set for a specific community.
    /// If community_id is None, adds globally.
    /// Returns true if newly inserted.
    pub fn add(&self, npub: &str, community_id: Option<&str>) -> bool {
        let inserted = {
            let mut inner = self.write();
            if npub == inner.owner {
                return false;
            }
            match community_id {
                None => inner.authorized_global.insert(npub.to_string()),
                Some(cid) => {
                    let set = inner.authorized_by_community.entry(cid.to_string()).or_default();
                    set.insert(npub.to_string())
                }
            }
        };
        if inserted {
            self.save();
        }
        inserted
    }

    /// Remove an npub from authorized. If community_id is Some, only removes
    /// from that community. If None, removes from global.
    /// Returns true if it was present.
    pub fn remove(&self, npub: &str, community_id: Option<&str>) -> bool {
        let removed = {
            let mut inner = self.write();
            match community_id {
                None => inner.authorized_global.remove(npub),
                Some(cid) => {
                    if let Some(set) = inner.authorized_by_community.get_mut(cid) {
                        set.remove(npub)
                    } else {
                        false
                    }
                }
            }
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
            authorized: Vec::new(), // legacy, always empty now
            authorized_global: inner.authorized_global.iter().cloned().collect(),
            authorized_by_community: inner
                .authorized_by_community
                .iter()
                .map(|(k, v)| (k.clone(), v.iter().cloned().collect()))
                .collect(),
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
        assert_eq!(auth.check("npub1owner", None), AuthLevel::Owner);
        assert!(auth.is_owner("npub1owner"));
    }

    #[test]
    fn test_authorized_user() {
        let sf = tmp_state_file();
        let auth =
            AuthManager::new("npub1owner", &["npub1friend".to_string()], false, sf).unwrap();
        assert_eq!(auth.check("npub1friend", None), AuthLevel::Authorized);
        assert!(auth.is_authorized("npub1friend", None));
    }

    #[test]
    fn test_unknown_is_public() {
        let sf = tmp_state_file();
        let auth = AuthManager::new("npub1owner", &[], false, sf).unwrap();
        assert_eq!(auth.check("npub1random", None), AuthLevel::Public);
    }

    #[test]
    fn test_add_and_remove() {
        let sf = tmp_state_file();
        let auth = AuthManager::new("npub1owner", &[], false, sf).unwrap();

        // Per-community add
        assert!(auth.add("npub1new", Some("community_a")));
        assert_eq!(auth.check("npub1new", Some("community_a")), AuthLevel::Authorized);
        // Not authorized in other communities
        assert_eq!(auth.check("npub1new", Some("community_b")), AuthLevel::Public);
        assert_eq!(auth.check("npub1new", None), AuthLevel::Public);

        // Duplicate add is a no-op.
        assert!(!auth.add("npub1new", Some("community_a")));
        // Can't add owner.
        assert!(!auth.add("npub1owner", Some("community_a")));

        // Global add works everywhere
        assert!(auth.add("npub1global", None));
        assert_eq!(auth.check("npub1global", Some("community_a")), AuthLevel::Authorized);
        assert_eq!(auth.check("npub1global", None), AuthLevel::Authorized);

        // Remove from specific community
        assert!(auth.remove("npub1new", Some("community_a")));
        assert_eq!(auth.check("npub1new", Some("community_a")), AuthLevel::Public);

        // Global remove
        assert!(auth.remove("npub1global", None));
        assert_eq!(auth.check("npub1global", Some("community_a")), AuthLevel::Public);
    }

    #[test]
    fn test_has_permission() {
        let sf = tmp_state_file();
        let auth = AuthManager::new("npub1owner", &["npub1friend".to_string()], false, sf).unwrap();

        // Owner can do everything (any context).
        assert!(auth.has_permission("npub1owner", Some("comm_a"), AuthLevel::Owner));
        assert!(auth.has_permission("npub1owner", None, AuthLevel::Owner));

        // Global authorized (from seed list) works in any context.
        assert!(auth.has_permission("npub1friend", Some("comm_a"), AuthLevel::Authorized));
        assert!(auth.has_permission("npub1friend", None, AuthLevel::Authorized));
        assert!(!auth.has_permission("npub1friend", Some("comm_a"), AuthLevel::Owner));

        // Community-scoped authorized only works in that community.
        auth.add("npub1local", Some("comm_a"));
        assert!(auth.has_permission("npub1local", Some("comm_a"), AuthLevel::Authorized));
        assert!(!auth.has_permission("npub1local", Some("comm_b"), AuthLevel::Authorized));
        assert!(!auth.has_permission("npub1local", None, AuthLevel::Authorized));

        // Public can only do Public.
        assert!(auth.has_permission("npub1random", Some("comm_a"), AuthLevel::Public));
        assert!(!auth.has_permission("npub1random", Some("comm_a"), AuthLevel::Authorized));
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

        // Seed list goes to global
        let (global, community) = auth.list(Some("comm_a"));
        assert_eq!(global, vec!["npub1alpha", "npub1charlie"]);
        assert!(community.is_empty());

        // Add a community-scoped user
        auth.add("npub1bob", Some("comm_a"));
        let (global, community) = auth.list(Some("comm_a"));
        assert_eq!(global, vec!["npub1alpha", "npub1charlie"]);
        assert_eq!(community, vec!["npub1bob"]);
    }

    #[test]
    fn test_persist_roundtrip() {
        let sf = tmp_state_file();

        {
            let auth = AuthManager::new("npub1owner", &[], true, sf.clone()).unwrap();
            auth.add("npub1persist1", None); // global
            auth.add("npub1persist2", None);
        }

        // Reload — should have persisted users.
        let auth = AuthManager::new("npub1owner", &[], true, sf.clone()).unwrap();
        assert!(auth.is_authorized("npub1persist1", None));
        assert!(auth.is_authorized("npub1persist2", None));
        assert_eq!(auth.authorized_count(), 2);

        let _ = std::fs::remove_file(&sf);
    }

    #[test]
    fn test_persist_disabled() {
        let sf = tmp_state_file();

        {
            let auth = AuthManager::new("npub1owner", &[], false, sf.clone()).unwrap();
            auth.add("npub1nopersist", None);
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
