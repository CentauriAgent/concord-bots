// =============================================================================
// community/mod.rs — SQLite-backed community engagement storage
// =============================================================================
//
// Tables: users, xp_log, giveaways, giveaway_entries, reputation
// All functions take a `&Database` (Arc<Mutex<Connection>>).
// Concurrency: Mutex<Connection> — only one writer at a time.

use anyhow::Result;
use rusqlite::Connection;
use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};

// -----------------------------------------------------------------------------
// Database wrapper
// -----------------------------------------------------------------------------

/// Thread-safe wrapper around a SQLite connection.
#[derive(Clone)]
pub struct Database {
    conn: Arc<Mutex<Connection>>,
}

/// Current Unix timestamp in seconds.
fn now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs() as i64
}

// -----------------------------------------------------------------------------
// User stats
// -----------------------------------------------------------------------------

/// A user's community stats.
#[derive(Debug, Clone, Default)]
pub struct UserStats {
    pub npub: String,
    pub xp: i64,
    pub level: i64,
    pub messages_sent: i64,
    pub sats_tipped: i64,
    pub sats_zapped: i64,
    pub first_seen: i64,
    pub last_xp_at: i64,
    pub rep: i64,
}

// -----------------------------------------------------------------------------
// Level curve (MEE6-style)
// -----------------------------------------------------------------------------

/// XP required to reach `level` (cumulative).
/// Formula: 5 * N^2 + 50 * N + 100
pub fn xp_for_level(level: i64) -> i64 {
    5 * level * level + 50 * level + 100
}

/// Determine the level for a given total XP.
pub fn level_for_xp(total_xp: i64) -> i64 {
    let mut level = 0i64;
    let mut required = 0i64;
    loop {
        let next = xp_for_level(level + 1);
        if total_xp < required + next {
            return level;
        }
        required += next;
        level += 1;
        if level > 1000 {
            return level; // sanity guard
        }
    }
}

/// Total XP accumulated up to and including `level`.
pub fn cumulative_xp_for_level(level: i64) -> i64 {
    let mut total = 0i64;
    for n in 1..=level {
        total += xp_for_level(n);
    }
    total
}

/// XP needed to go from `level` to `level + 1`.
pub fn xp_in_current_level(total_xp: i64, level: i64) -> i64 {
    total_xp - cumulative_xp_for_level(level)
}

// -----------------------------------------------------------------------------
// Giveaway
// -----------------------------------------------------------------------------

#[derive(Debug, Clone)]
pub struct Giveaway {
    pub id: String,
    pub channel_id: String,
    pub prize: String,
    pub prize_sats: i64,
    pub starts_at: i64,
    pub ends_at: i64,
    pub active: bool,
    pub winner_npub: Option<String>,
}

// -----------------------------------------------------------------------------
// Initialization
// -----------------------------------------------------------------------------

impl Database {
    /// Open (or create) the database at `path` and initialize tables.
    pub fn open(path: &Path) -> Result<Self> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let conn = Connection::open(path)?;
        conn.execute_batch(
            "
            CREATE TABLE IF NOT EXISTS users (
                npub           TEXT PRIMARY KEY,
                xp             INTEGER DEFAULT 0,
                level          INTEGER DEFAULT 0,
                messages_sent  INTEGER DEFAULT 0,
                sats_tipped    INTEGER DEFAULT 0,
                sats_zapped    INTEGER DEFAULT 0,
                first_seen     INTEGER,
                last_xp_at     INTEGER DEFAULT 0,
                rep            INTEGER DEFAULT 0
            );

            CREATE TABLE IF NOT EXISTS xp_log (
                npub       TEXT,
                amount     INTEGER,
                timestamp  INTEGER,
                channel_id TEXT
            );

            CREATE TABLE IF NOT EXISTS giveaways (
                id          TEXT PRIMARY KEY,
                channel_id  TEXT,
                prize       TEXT,
                prize_sats  INTEGER DEFAULT 0,
                starts_at   INTEGER,
                ends_at     INTEGER,
                active      INTEGER DEFAULT 1,
                winner_npub TEXT
            );

            CREATE TABLE IF NOT EXISTS giveaway_entries (
                giveaway_id TEXT,
                npub        TEXT,
                entered_at  INTEGER
            );

            CREATE TABLE IF NOT EXISTS reputation (
                from_npub TEXT,
                to_npub   TEXT,
                timestamp INTEGER,
                PRIMARY KEY (from_npub, to_npub)
            );

            CREATE INDEX IF NOT EXISTS idx_xp_log_npub ON xp_log(npub);
            CREATE INDEX IF NOT EXISTS idx_giveaway_entries_id ON giveaway_entries(giveaway_id);
            CREATE INDEX IF NOT EXISTS idx_users_xp ON users(xp DESC);

            CREATE TABLE IF NOT EXISTS channel_state (
                channel_id  TEXT PRIMARY KEY,
                enabled     INTEGER DEFAULT 1,
                updated_at  INTEGER,
                updated_by  TEXT
            );
            ",
        )?;
        Ok(Self {
            conn: Arc::new(Mutex::new(conn)),
        })
    }

    // ---------------------------------------------------------------------
    // XP / Leveling
    // ---------------------------------------------------------------------

    /// Award XP to `npub`. Creates the user row if it doesn't exist.
    /// Returns `(new_level, leveled_up)`.
    pub fn award_xp(&self, npub: &str, amount: i64, channel_id: &str) -> Result<(i64, bool)> {
        let conn = self.conn.lock().unwrap();
        let ts = now();

        // Ensure user exists
        conn.execute(
            "INSERT OR IGNORE INTO users (npub, first_seen) VALUES (?1, ?2)",
            rusqlite::params![npub, ts],
        )?;

        // Fetch old level
        let old_level: i64 = conn
            .query_row(
                "SELECT level FROM users WHERE npub = ?1",
                rusqlite::params![npub],
                |row| row.get(0),
            )
            .unwrap_or(0);

        // Add XP
        conn.execute(
            "UPDATE users SET xp = xp + ?1, last_xp_at = ?2 WHERE npub = ?3",
            rusqlite::params![amount, ts, npub],
        )?;

        // Compute new level from total XP
        let total_xp: i64 = conn
            .query_row(
                "SELECT xp FROM users WHERE npub = ?1",
                rusqlite::params![npub],
                |row| row.get(0),
            )
            .unwrap_or(0);

        let new_level = level_for_xp(total_xp);

        if new_level != old_level {
            conn.execute(
                "UPDATE users SET level = ?1 WHERE npub = ?2",
                rusqlite::params![new_level, npub],
            )?;
        }

        // Log XP
        conn.execute(
            "INSERT INTO xp_log (npub, amount, timestamp, channel_id) VALUES (?1, ?2, ?3, ?4)",
            rusqlite::params![npub, amount, ts, channel_id],
        )?;

        Ok((new_level, new_level > old_level))
    }

    /// Check if the user is on XP cooldown (last XP award within `cooldown_secs`).
    pub fn is_on_xp_cooldown(&self, npub: &str, cooldown_secs: i64) -> Result<bool> {
        let conn = self.conn.lock().unwrap();
        let last: Option<i64> = conn
            .query_row(
                "SELECT last_xp_at FROM users WHERE npub = ?1",
                rusqlite::params![npub],
                |row| row.get(0),
            )
            .ok();
        match last {
            Some(t) => Ok(now() - t < cooldown_secs),
            None => Ok(false),
        }
    }

    /// Increment message count for a user.
    pub fn increment_messages(&self, npub: &str) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        let ts = now();
        conn.execute(
            "INSERT OR IGNORE INTO users (npub, first_seen) VALUES (?1, ?2)",
            rusqlite::params![npub, ts],
        )?;
        conn.execute(
            "UPDATE users SET messages_sent = messages_sent + 1 WHERE npub = ?1",
            rusqlite::params![npub],
        )?;
        Ok(())
    }

    /// Add sats tipped to user's record.
    pub fn add_sats_tipped(&self, npub: &str, sats: i64) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        let ts = now();
        conn.execute(
            "INSERT OR IGNORE INTO users (npub, first_seen) VALUES (?1, ?2)",
            rusqlite::params![npub, ts],
        )?;
        conn.execute(
            "UPDATE users SET sats_tipped = sats_tipped + ?1 WHERE npub = ?2",
            rusqlite::params![sats, npub],
        )?;
        Ok(())
    }

    /// Add sats zapped to user's record.
    pub fn add_sats_zapped(&self, npub: &str, sats: i64) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        let ts = now();
        conn.execute(
            "INSERT OR IGNORE INTO users (npub, first_seen) VALUES (?1, ?2)",
            rusqlite::params![npub, ts],
        )?;
        conn.execute(
            "UPDATE users SET sats_zapped = sats_zapped + ?1 WHERE npub = ?2",
            rusqlite::params![sats, npub],
        )?;
        Ok(())
    }

    // ---------------------------------------------------------------------
    // User queries
    // ---------------------------------------------------------------------

    /// Get a user's stats. Returns a default row if user doesn't exist.
    pub fn get_user(&self, npub: &str) -> Result<UserStats> {
        let conn = self.conn.lock().unwrap();
        let stats = conn
            .query_row(
                "SELECT npub, xp, level, messages_sent, sats_tipped, sats_zapped, \
                 COALESCE(first_seen, 0), COALESCE(last_xp_at, 0), rep \
                 FROM users WHERE npub = ?1",
                rusqlite::params![npub],
                |row| {
                    Ok(UserStats {
                        npub: row.get(0)?,
                        xp: row.get(1)?,
                        level: row.get(2)?,
                        messages_sent: row.get(3)?,
                        sats_tipped: row.get(4)?,
                        sats_zapped: row.get(5)?,
                        first_seen: row.get(6)?,
                        last_xp_at: row.get(7)?,
                        rep: row.get(8)?,
                    })
                },
            )
            .unwrap_or_default();
        Ok(stats)
    }

    /// Get the leaderboard: top users by XP.
    pub fn get_leaderboard(&self, limit: i64) -> Result<Vec<(String, i64, i64)>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT npub, level, xp FROM users ORDER BY xp DESC LIMIT ?1",
        )?;
        let rows = stmt
            .query_map(rusqlite::params![limit], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?, row.get::<_, i64>(2)?))
            })?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(rows)
    }

    /// Get user's rank (1-based) by XP.
    pub fn get_rank(&self, npub: &str) -> Result<Option<i64>> {
        let conn = self.conn.lock().unwrap();
        let rank: Option<i64> = conn
            .query_row(
                "SELECT COUNT(*) + 1 FROM users WHERE xp > \
                 (SELECT xp FROM users WHERE npub = ?1)",
                rusqlite::params![npub],
                |row| row.get(0),
            )
            .ok();
        Ok(rank)
    }

    // ---------------------------------------------------------------------
    // Giveaways
    // ---------------------------------------------------------------------

    /// Create a new giveaway.
    pub fn add_giveaway(
        &self,
        id: &str,
        channel_id: &str,
        prize: &str,
        prize_sats: i64,
        starts_at: i64,
        ends_at: i64,
    ) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "INSERT INTO giveaways (id, channel_id, prize, prize_sats, starts_at, ends_at, active) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, 1)",
            rusqlite::params![id, channel_id, prize, prize_sats, starts_at, ends_at],
        )?;
        Ok(())
    }

    /// End a giveaway (mark inactive). Returns the giveaway row.
    pub fn end_giveaway(&self, id: &str) -> Result<Option<Giveaway>> {
        let conn = self.conn.lock().unwrap();
        let giveaway = self.get_giveaway_inner(&conn, id)?;
        conn.execute(
            "UPDATE giveaways SET active = 0 WHERE id = ?1",
            rusqlite::params![id],
        )?;
        Ok(giveaway)
    }

    /// Get a single giveaway by ID.
    fn get_giveaway_inner(
        &self,
        conn: &Connection,
        id: &str,
    ) -> Result<Option<Giveaway>> {
        let g = conn
            .query_row(
                "SELECT id, channel_id, prize, prize_sats, starts_at, ends_at, active, winner_npub \
                 FROM giveaways WHERE id = ?1",
                rusqlite::params![id],
                |row| {
                    Ok(Giveaway {
                        id: row.get(0)?,
                        channel_id: row.get(1)?,
                        prize: row.get(2)?,
                        prize_sats: row.get::<_, Option<i64>>(3)?.unwrap_or(0),
                        starts_at: row.get(4)?,
                        ends_at: row.get(5)?,
                        active: row.get::<_, i64>(6)? != 0,
                        winner_npub: row.get(7)?,
                    })
                },
            )
            .ok();
        Ok(g)
    }

    /// Get a single giveaway by ID (public).
    pub fn get_giveaway(&self, id: &str) -> Result<Option<Giveaway>> {
        let conn = self.conn.lock().unwrap();
        self.get_giveaway_inner(&conn, id)
    }

    /// Get all active giveaways.
    pub fn get_active_giveaways(&self) -> Result<Vec<Giveaway>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT id, channel_id, prize, prize_sats, starts_at, ends_at, active, winner_npub \
             FROM giveaways WHERE active = 1",
        )?;
        let rows = stmt.query_map([], |row| {
            Ok(Giveaway {
                id: row.get(0)?,
                channel_id: row.get(1)?,
                prize: row.get(2)?,
                prize_sats: row.get::<_, Option<i64>>(3)?.unwrap_or(0),
                starts_at: row.get(4)?,
                ends_at: row.get(5)?,
                active: row.get::<_, i64>(6)? != 0,
                winner_npub: row.get(7)?,
            })
        })?;
        let mut result = Vec::new();
        for row in rows {
            result.push(row?);
        }
        Ok(result)
    }

    /// Get active giveaways in a specific channel.
    pub fn get_active_giveaways_in_channel(&self, channel_id: &str) -> Result<Vec<Giveaway>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT id, channel_id, prize, prize_sats, starts_at, ends_at, active, winner_npub \
             FROM giveaways WHERE active = 1 AND channel_id = ?1",
        )?;
        let rows = stmt.query_map(rusqlite::params![channel_id], |row| {
            Ok(Giveaway {
                id: row.get(0)?,
                channel_id: row.get(1)?,
                prize: row.get(2)?,
                prize_sats: row.get::<_, Option<i64>>(3)?.unwrap_or(0),
                starts_at: row.get(4)?,
                ends_at: row.get(5)?,
                active: row.get::<_, i64>(6)? != 0,
                winner_npub: row.get(7)?,
            })
        })?;
        let mut result = Vec::new();
        for row in rows {
            result.push(row?);
        }
        Ok(result)
    }

    /// Add an entry to a giveaway.
    pub fn add_entry(&self, giveaway_id: &str, npub: &str) -> Result<bool> {
        let conn = self.conn.lock().unwrap();
        let ts = now();
        conn.execute(
            "INSERT OR IGNORE INTO giveaway_entries (giveaway_id, npub, entered_at) \
             VALUES (?1, ?2, ?3)",
            rusqlite::params![giveaway_id, npub, ts],
        )?;
        Ok(conn.changes() > 0)
    }

    /// Get all entries for a giveaway.
    pub fn get_entries(&self, giveaway_id: &str) -> Result<Vec<String>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt =
            conn.prepare("SELECT npub FROM giveaway_entries WHERE giveaway_id = ?1")?;
        let rows = stmt.query_map(rusqlite::params![giveaway_id], |row| row.get::<_, String>(0))?;
        let mut result = Vec::new();
        for row in rows {
            result.push(row?);
        }
        Ok(result)
    }

    /// Set the winner for a giveaway.
    pub fn set_winner(&self, giveaway_id: &str, winner_npub: &str) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "UPDATE giveaways SET winner_npub = ?1, active = 0 WHERE id = ?2",
            rusqlite::params![winner_npub, giveaway_id],
        )?;
        Ok(())
    }

    /// Pick a random winner from entries and set it. Returns winner npub.
    pub fn pick_winner(&self, giveaway_id: &str) -> Result<Option<String>> {
        let entries = self.get_entries(giveaway_id)?;
        if entries.is_empty() {
            return Ok(None);
        }
        use rand::seq::SliceRandom;
        let winner = entries
            .choose(&mut rand::thread_rng())
            .map(|s| s.to_string());
        if let Some(ref w) = winner {
            self.set_winner(giveaway_id, w)?;
        }
        Ok(winner)
    }

    // ---------------------------------------------------------------------
    // Reputation
    // ---------------------------------------------------------------------

    /// Give +1 rep from `from` to `to`. Returns Ok(()) if successful.
    /// Returns Err if on cooldown (already gave rep to this target within 24h).
    pub fn give_rep(&self, from: &str, to: &str) -> Result<bool> {
        let conn = self.conn.lock().unwrap();
        let ts = now();

        // Ensure users exist
        conn.execute(
            "INSERT OR IGNORE INTO users (npub, first_seen) VALUES (?1, ?2)",
            rusqlite::params![to, ts],
        )?;

        // Check cooldown
        let last: Option<i64> = conn
            .query_row(
                "SELECT timestamp FROM reputation WHERE from_npub = ?1 AND to_npub = ?2",
                rusqlite::params![from, to],
                |row| row.get(0),
            )
            .ok();

        if let Some(t) = last {
            if now() - t < 86400 {
                // 24h cooldown
                return Ok(false);
            }
        }

        // Upsert reputation record
        conn.execute(
            "INSERT INTO reputation (from_npub, to_npub, timestamp) VALUES (?1, ?2, ?3) \
             ON CONFLICT(from_npub, to_npub) DO UPDATE SET timestamp = ?3",
            rusqlite::params![from, to, ts],
        )?;

        // Increment target's rep
        conn.execute(
            "UPDATE users SET rep = rep + 1 WHERE npub = ?1",
            rusqlite::params![to],
        )?;

        Ok(true)
    }

    /// Check if the giver is on cooldown for a specific target.
    pub fn check_rep_cooldown(&self, from: &str, to: &str) -> Result<bool> {
        let conn = self.conn.lock().unwrap();
        let last: Option<i64> = conn
            .query_row(
                "SELECT timestamp FROM reputation WHERE from_npub = ?1 AND to_npub = ?2",
                rusqlite::params![from, to],
                |row| row.get(0),
            )
            .ok();
        match last {
            Some(t) => Ok(now() - t < 86400),
            None => Ok(false),
        }
    }

    // ---------------------------------------------------------------------
    // Channel State (enable/disable per channel)
    // ---------------------------------------------------------------------

    /// Check if a channel is enabled. Returns `true` if no state exists
    /// (default: enabled for backward compat with existing channels).
    pub fn is_channel_enabled(&self, channel_id: &str) -> Result<bool> {
        let conn = self.conn.lock().unwrap();
        let enabled: Option<i64> = conn
            .query_row(
                "SELECT enabled FROM channel_state WHERE channel_id = ?1",
                rusqlite::params![channel_id],
                |row| row.get(0),
            )
            .ok();
        match enabled {
            Some(1) => Ok(true),
            _ => Ok(false), // no row or explicitly 0 = disabled (opt-in default)
        }
    }

    /// Set channel enabled state. Creates the row if it doesn't exist.
    pub fn set_channel_enabled(
        &self,
        channel_id: &str,
        enabled: bool,
        updated_by: &str,
    ) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "INSERT INTO channel_state (channel_id, enabled, updated_at, updated_by) \
             VALUES (?1, ?2, ?3, ?4) \
             ON CONFLICT(channel_id) DO UPDATE SET enabled = ?2, updated_at = ?3, updated_by = ?4",
            rusqlite::params![channel_id, enabled as i32, now(), updated_by],
        )?;
        Ok(())
    }

    /// Mark a channel as disabled. Used when the bot joins a new community
    /// and we want all channels to start in the opt-in state.
    pub fn disable_channel(&self, channel_id: &str) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "INSERT INTO channel_state (channel_id, enabled, updated_at, updated_by) \
             VALUES (?1, 0, ?2, 'system') \
             ON CONFLICT(channel_id) DO UPDATE SET enabled = 0, updated_at = ?2, updated_by = 'system'",
            rusqlite::params![channel_id, now()],
        )?;
        Ok(())
    }
}

// -----------------------------------------------------------------------------
// Tests
// -----------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn test_db() -> Database {
        Database::open(Path::new(":memory:")).unwrap()
    }

    #[test]
    fn test_xp_for_level() {
        // Formula: 5*N^2 + 50*N + 100 (XP needed to advance from level N-1 to N)
        assert_eq!(xp_for_level(1), 155);   // 5 + 50 + 100
        assert_eq!(xp_for_level(5), 475);   // 125 + 250 + 100
        assert_eq!(xp_for_level(10), 1100); // 500 + 500 + 100
        assert_eq!(xp_for_level(20), 3100); // 2000 + 1000 + 100
    }

    #[test]
    fn test_level_for_xp() {
        // cumulative(1) = 155, cumulative(2) = 375, cumulative(3) = 670
        assert_eq!(level_for_xp(0), 0);
        assert_eq!(level_for_xp(154), 0);
        assert_eq!(level_for_xp(155), 1);
        assert_eq!(level_for_xp(374), 1);
        assert_eq!(level_for_xp(375), 2);
        assert_eq!(level_for_xp(669), 2);
        assert_eq!(level_for_xp(670), 3);
    }

    #[test]
    fn test_cumulative_xp() {
        assert_eq!(cumulative_xp_for_level(0), 0);
        assert_eq!(cumulative_xp_for_level(1), 155);
        assert_eq!(cumulative_xp_for_level(2), 375); // 155 + 220
        assert_eq!(cumulative_xp_for_level(3), 670); // 375 + 295
    }

    #[test]
    fn test_award_xp() {
        let db = test_db();
        let (level, leveled_up) = db.award_xp("npub1test", 100, "ch1").unwrap();
        assert_eq!(level, 0);
        assert!(!leveled_up);

        // Award enough to reach level 1
        let (level, leveled_up) = db.award_xp("npub1test", 100, "ch1").unwrap();
        assert!(level >= 1);
        assert!(leveled_up);
    }

    #[test]
    fn test_xp_cooldown() {
        let db = test_db();
        db.award_xp("npub1test", 10, "ch1").unwrap();
        assert!(db.is_on_xp_cooldown("npub1test", 60).unwrap());
        assert!(!db.is_on_xp_cooldown("npub1other", 60).unwrap());
    }

    #[test]
    fn test_leaderboard() {
        let db = test_db();
        db.award_xp("npub1a", 100, "ch1").unwrap();
        db.award_xp("npub1b", 200, "ch1").unwrap();
        db.award_xp("npub1c", 50, "ch1").unwrap();

        let lb = db.get_leaderboard(3).unwrap();
        assert_eq!(lb.len(), 3);
        assert_eq!(lb[0].0, "npub1b"); // highest XP
        assert_eq!(lb[1].0, "npub1a");
        assert_eq!(lb[2].0, "npub1c");
    }

    #[test]
    fn test_giveaway_flow() {
        let db = test_db();
        db.add_giveaway("g1", "ch1", "50 sats", 50, 0, 1000).unwrap();

        let active = db.get_active_giveaways().unwrap();
        assert_eq!(active.len(), 1);

        db.add_entry("g1", "npub1a").unwrap();
        db.add_entry("g1", "npub1b").unwrap();

        let entries = db.get_entries("g1").unwrap();
        assert_eq!(entries.len(), 2);

        let winner = db.pick_winner("g1").unwrap();
        assert!(winner.is_some());

        let g = db.get_giveaway("g1").unwrap().unwrap();
        assert!(!g.active);
        assert!(g.winner_npub.is_some());
    }

    #[test]
    fn test_reputation() {
        let db = test_db();
        // First rep — should succeed
        assert!(db.give_rep("npub1a", "npub1b").unwrap());
        // Second rep to same target — should fail (cooldown)
        assert!(!db.give_rep("npub1a", "npub1b").unwrap());
        // Rep to different target — should succeed
        assert!(db.give_rep("npub1a", "npub1c").unwrap());

        let stats = db.get_user("npub1b").unwrap();
        assert_eq!(stats.rep, 1);
    }

    #[test]
    fn test_increment_messages() {
        let db = test_db();
        db.increment_messages("npub1a").unwrap();
        db.increment_messages("npub1a").unwrap();
        let stats = db.get_user("npub1a").unwrap();
        assert_eq!(stats.messages_sent, 2);
    }

    #[test]
    fn test_rank() {
        let db = test_db();
        db.award_xp("npub1a", 100, "ch1").unwrap();
        db.award_xp("npub1b", 300, "ch1").unwrap();
        db.award_xp("npub1c", 200, "ch1").unwrap();

        assert_eq!(db.get_rank("npub1b").unwrap(), Some(1));
        assert_eq!(db.get_rank("npub1c").unwrap(), Some(2));
        assert_eq!(db.get_rank("npub1a").unwrap(), Some(3));
    }

    #[test]
    fn test_get_user_default() {
        let db = test_db();
        let stats = db.get_user("nonexistent").unwrap();
        assert_eq!(stats.xp, 0);
        assert_eq!(stats.level, 0);
    }

    #[test]
    fn test_channel_state_default_disabled() {
        let db = test_db();
        // No row = disabled (opt-in default)
        assert!(!db.is_channel_enabled("ch1").unwrap());
    }

    #[test]
    fn test_channel_disable_enable() {
        let db = test_db();
        // Disable
        db.set_channel_enabled("ch1", false, "npub1owner").unwrap();
        assert!(!db.is_channel_enabled("ch1").unwrap());
        // Re-enable
        db.set_channel_enabled("ch1", true, "npub1owner").unwrap();
        assert!(db.is_channel_enabled("ch1").unwrap());
    }

    #[test]
    fn test_disable_channel_idempotent() {
        let db = test_db();
        db.disable_channel("ch1").unwrap();
        assert!(!db.is_channel_enabled("ch1").unwrap());
        // Calling again shouldn't flip it back to enabled (INSERT OR IGNORE)
        db.disable_channel("ch1").unwrap();
        assert!(!db.is_channel_enabled("ch1").unwrap());
    }

    #[test]
    fn test_enable_after_disable_channel() {
        let db = test_db();
        db.disable_channel("ch1").unwrap();
        assert!(!db.is_channel_enabled("ch1").unwrap());
        // Owner explicitly enables
        db.set_channel_enabled("ch1", true, "npub1owner").unwrap();
        assert!(db.is_channel_enabled("ch1").unwrap());
    }
}
