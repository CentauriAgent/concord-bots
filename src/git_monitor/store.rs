// =============================================================================
// git_monitor/store.rs — SQLite subscription storage
// =============================================================================
//
// Tables: subscriptions (one row per channel+repo pair)
// Pattern follows community/mod.rs: Arc<Mutex<Connection>>

use anyhow::Result;
use rusqlite::Connection;
use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};

// -----------------------------------------------------------------------------
// Types
// -----------------------------------------------------------------------------

/// Which git host a subscription tracks.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum RepoHost {
    GitHub,
    GitLab,
}

impl RepoHost {
    /// String representation for DB storage.
    pub fn as_str(&self) -> &'static str {
        match self {
            RepoHost::GitHub => "github",
            RepoHost::GitLab => "gitlab",
        }
    }

    /// Parse from string.
    pub fn from_str(s: &str) -> Option<Self> {
        match s.to_lowercase().as_str() {
            "github" => Some(RepoHost::GitHub),
            "gitlab" => Some(RepoHost::GitLab),
            _ => None,
        }
    }

    /// Base URL for constructing links.
    pub fn base_url(&self) -> &'static str {
        match self {
            RepoHost::GitHub => "https://github.com",
            RepoHost::GitLab => "https://gitlab.com",
        }
    }
}

/// A subscription row.
#[derive(Debug, Clone)]
pub struct Subscription {
    pub id: i64,
    pub channel_id: String,
    pub host: RepoHost,
    pub owner: String,
    pub repo: String,
    pub full_slug: String,
    pub added_by: String,
    pub added_at: i64,
    pub last_commit_sha: Option<String>,
    pub last_release_tag: Option<String>,
    pub last_poll_at: Option<i64>,
}

// -----------------------------------------------------------------------------
// Database wrapper
// -----------------------------------------------------------------------------

/// Thread-safe wrapper around a SQLite connection for git subscriptions.
#[derive(Clone)]
pub struct SubscriptionStore {
    conn: Arc<Mutex<Connection>>,
}

/// Current Unix timestamp in seconds.
fn now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs() as i64
}

impl SubscriptionStore {
    /// Open (or create) the database at `path` and initialize tables.
    pub fn open(path: &Path) -> Result<Self> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let conn = Connection::open(path)?;
        conn.execute_batch(
            "
            CREATE TABLE IF NOT EXISTS subscriptions (
                id                  INTEGER PRIMARY KEY AUTOINCREMENT,
                channel_id          TEXT NOT NULL,
                host                TEXT NOT NULL,
                owner               TEXT NOT NULL,
                repo                TEXT NOT NULL,
                full_slug           TEXT NOT NULL,
                added_by            TEXT NOT NULL,
                added_at            INTEGER NOT NULL,
                last_commit_sha     TEXT,
                last_release_tag    TEXT,
                last_poll_at        INTEGER,
                UNIQUE(channel_id, host, owner, repo)
            );

            CREATE INDEX IF NOT EXISTS idx_subscriptions_channel ON subscriptions(channel_id);
            CREATE INDEX IF NOT EXISTS idx_subscriptions_host_repo ON subscriptions(host, owner, repo);
            ",
        )?;
        Ok(Self {
            conn: Arc::new(Mutex::new(conn)),
        })
    }

    /// Add a subscription. Returns Err if already exists (UNIQUE constraint).
    pub fn add(
        &self,
        channel_id: &str,
        host: RepoHost,
        owner: &str,
        repo: &str,
        added_by: &str,
    ) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        let full_slug = format!("{}/{}", owner, repo);
        conn.execute(
            "INSERT INTO subscriptions (channel_id, host, owner, repo, full_slug, added_by, added_at) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            rusqlite::params![
                channel_id,
                host.as_str(),
                owner,
                repo,
                full_slug,
                added_by,
                now(),
            ],
        )?;
        Ok(())
    }

    /// Remove a subscription by channel + host + slug. Returns true if removed.
    pub fn remove(&self, channel_id: &str, host: RepoHost, owner: &str, repo: &str) -> Result<bool> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "DELETE FROM subscriptions WHERE channel_id = ?1 AND host = ?2 AND owner = ?3 AND repo = ?4",
            rusqlite::params![channel_id, host.as_str(), owner, repo],
        )?;
        Ok(conn.changes() > 0)
    }

    /// Remove a subscription by its numeric ID. Returns true if removed.
    pub fn remove_by_id(&self, id: i64) -> Result<bool> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "DELETE FROM subscriptions WHERE id = ?1",
            rusqlite::params![id],
        )?;
        Ok(conn.changes() > 0)
    }

    /// List all subscriptions for a channel.
    pub fn list_for_channel(&self, channel_id: &str) -> Result<Vec<Subscription>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT id, channel_id, host, owner, repo, full_slug, added_by, added_at, \
             last_commit_sha, last_release_tag, last_poll_at \
             FROM subscriptions WHERE channel_id = ?1 ORDER BY added_at",
        )?;
        let rows = stmt.query_map(rusqlite::params![channel_id], map_subscription)?;
        let mut result = Vec::new();
        for row in rows {
            result.push(row?);
        }
        Ok(result)
    }

    /// List ALL subscriptions (for the poller).
    pub fn list_all(&self) -> Result<Vec<Subscription>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT id, channel_id, host, owner, repo, full_slug, added_by, added_at, \
             last_commit_sha, last_release_tag, last_poll_at \
             FROM subscriptions ORDER BY id",
        )?;
        let rows = stmt.query_map([], map_subscription)?;
        let mut result = Vec::new();
        for row in rows {
            result.push(row?);
        }
        Ok(result)
    }

    /// Count subscriptions in a channel.
    pub fn count_for_channel(&self, channel_id: &str) -> Result<usize> {
        let conn = self.conn.lock().unwrap();
        let count: i64 = conn.query_row(
            "SELECT COUNT(*) FROM subscriptions WHERE channel_id = ?1",
            rusqlite::params![channel_id],
            |row| row.get(0),
        )?;
        Ok(count as usize)
    }

    /// Check if a subscription already exists.
    pub fn exists(&self, channel_id: &str, host: RepoHost, owner: &str, repo: &str) -> Result<bool> {
        let conn = self.conn.lock().unwrap();
        let count: i64 = conn.query_row(
            "SELECT COUNT(*) FROM subscriptions WHERE channel_id = ?1 AND host = ?2 AND owner = ?3 AND repo = ?4",
            rusqlite::params![channel_id, host.as_str(), owner, repo],
            |row| row.get(0),
        )?;
        Ok(count > 0)
    }

    /// Update last_commit_sha for a subscription.
    pub fn set_last_commit_sha(&self, id: i64, sha: &str) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "UPDATE subscriptions SET last_commit_sha = ?1, last_poll_at = ?2 WHERE id = ?3",
            rusqlite::params![sha, now(), id],
        )?;
        Ok(())
    }

    /// Update last_release_tag for a subscription.
    pub fn set_last_release_tag(&self, id: i64, tag: &str) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "UPDATE subscriptions SET last_release_tag = ?1, last_poll_at = ?2 WHERE id = ?3",
            rusqlite::params![tag, now(), id],
        )?;
        Ok(())
    }

    /// Update last_poll_at timestamp.
    pub fn touch_poll(&self, id: i64) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "UPDATE subscriptions SET last_poll_at = ?1 WHERE id = ?2",
            rusqlite::params![now(), id],
        )?;
        Ok(())
    }
}

/// Map a rusqlite row to a Subscription.
fn map_subscription(row: &rusqlite::Row) -> rusqlite::Result<Subscription> {
    let host_str: String = row.get(2)?;
    let host = RepoHost::from_str(&host_str).unwrap_or(RepoHost::GitHub);
    Ok(Subscription {
        id: row.get(0)?,
        channel_id: row.get(1)?,
        host,
        owner: row.get(3)?,
        repo: row.get(4)?,
        full_slug: row.get(5)?,
        added_by: row.get(6)?,
        added_at: row.get(7)?,
        last_commit_sha: row.get(8)?,
        last_release_tag: row.get(9)?,
        last_poll_at: row.get(10)?,
    })
}

// -----------------------------------------------------------------------------
// Tests
// -----------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn test_store() -> SubscriptionStore {
        SubscriptionStore::open(Path::new(":memory:")).unwrap()
    }

    #[test]
    fn test_add_and_list() {
        let store = test_store();
        store
            .add("ch1", RepoHost::GitHub, "soapbox-pub", "agora", "npub1abc")
            .unwrap();

        let subs = store.list_for_channel("ch1").unwrap();
        assert_eq!(subs.len(), 1);
        assert_eq!(subs[0].owner, "soapbox-pub");
        assert_eq!(subs[0].repo, "agora");
        assert_eq!(subs[0].full_slug, "soapbox-pub/agora");
        assert_eq!(subs[0].host, RepoHost::GitHub);
        assert!(subs[0].last_commit_sha.is_none());
    }

    #[test]
    fn test_dedupe() {
        let store = test_store();
        store
            .add("ch1", RepoHost::GitHub, "owner", "repo", "npub1a")
            .unwrap();

        // Same channel + repo → error
        let result = store.add("ch1", RepoHost::GitHub, "owner", "repo", "npub1b");
        assert!(result.is_err());

        // Different channel → OK
        store
            .add("ch2", RepoHost::GitHub, "owner", "repo", "npub1c")
            .unwrap();

        let all = store.list_all().unwrap();
        assert_eq!(all.len(), 2);
    }

    #[test]
    fn test_remove() {
        let store = test_store();
        store
            .add("ch1", RepoHost::GitHub, "owner", "repo", "npub1a")
            .unwrap();

        let removed = store
            .remove("ch1", RepoHost::GitHub, "owner", "repo")
            .unwrap();
        assert!(removed);

        let subs = store.list_for_channel("ch1").unwrap();
        assert_eq!(subs.len(), 0);

        // Remove non-existent → false
        let removed_again = store
            .remove("ch1", RepoHost::GitHub, "owner", "repo")
            .unwrap();
        assert!(!removed_again);
    }

    #[test]
    fn test_remove_by_id() {
        let store = test_store();
        store
            .add("ch1", RepoHost::GitHub, "owner", "repo", "npub1a")
            .unwrap();

        let subs = store.list_for_channel("ch1").unwrap();
        let id = subs[0].id;

        let removed = store.remove_by_id(id).unwrap();
        assert!(removed);

        let subs = store.list_for_channel("ch1").unwrap();
        assert_eq!(subs.len(), 0);
    }

    #[test]
    fn test_count() {
        let store = test_store();
        store.add("ch1", RepoHost::GitHub, "a", "b", "npub1").unwrap();
        store.add("ch1", RepoHost::GitLab, "c", "d", "npub1").unwrap();
        store.add("ch2", RepoHost::GitHub, "e", "f", "npub1").unwrap();

        assert_eq!(store.count_for_channel("ch1").unwrap(), 2);
        assert_eq!(store.count_for_channel("ch2").unwrap(), 1);
        assert_eq!(store.count_for_channel("ch3").unwrap(), 0);
    }

    #[test]
    fn test_exists() {
        let store = test_store();
        store
            .add("ch1", RepoHost::GitHub, "owner", "repo", "npub1")
            .unwrap();

        assert!(store
            .exists("ch1", RepoHost::GitHub, "owner", "repo")
            .unwrap());
        assert!(!store
            .exists("ch1", RepoHost::GitLab, "owner", "repo")
            .unwrap());
        assert!(!store
            .exists("ch2", RepoHost::GitHub, "owner", "repo")
            .unwrap());
    }

    #[test]
    fn test_update_sha_and_tag() {
        let store = test_store();
        store
            .add("ch1", RepoHost::GitHub, "owner", "repo", "npub1")
            .unwrap();

        let subs = store.list_for_channel("ch1").unwrap();
        let id = subs[0].id;

        store.set_last_commit_sha(id, "abc123").unwrap();
        store.set_last_release_tag(id, "v1.0").unwrap();

        let subs = store.list_for_channel("ch1").unwrap();
        assert_eq!(subs[0].last_commit_sha.as_deref(), Some("abc123"));
        assert_eq!(subs[0].last_release_tag.as_deref(), Some("v1.0"));
        assert!(subs[0].last_poll_at.is_some());
    }

    #[test]
    fn test_repo_host_roundtrip() {
        assert_eq!(RepoHost::GitHub.as_str(), "github");
        assert_eq!(RepoHost::GitLab.as_str(), "gitlab");
        assert_eq!(RepoHost::from_str("github"), Some(RepoHost::GitHub));
        assert_eq!(RepoHost::from_str("gitlab"), Some(RepoHost::GitLab));
        assert_eq!(RepoHost::from_str("unknown"), None);
    }
}
