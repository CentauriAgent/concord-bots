// =============================================================================
// handlers/automod.rs — Auto-Moderation Engine (spam detection + auto-kick/ban)
// =============================================================================
//
// Discord-style auto-moderation for the Flagship bot. Hooks into on_message()
// BEFORE command dispatch to scan every community message, scores it against a
// set of detection rules, and (if the score crosses a threshold) deletes the
// message and warns / kicks / bans the sender.
//
// Design notes:
//   - The engine is cloneable and thread-safe (Arc<RwLock<...>> internally),
//     mirroring the rate_limiter pattern.
//   - check_message() must be FAST — it runs on every message. All lookups are
//     in-memory; disk persistence is throttled (every ~60s) and off the hot path.
//   - Numeric thresholds are read from the immutable `AutoModSection` config that
//     lives in `ctx.config.automod`. Runtime-tunable lists (banned words, regex
//     patterns, link allowlist) and the enabled flag live inside the engine so
//     `!automod` commands can mutate them without a restart.
//   - Auto-mod is OFF by default (config `enabled = false`). When first enabled
//     it runs in dry-run (log-only) mode for `dry_run_minutes` so operators can
//     see what *would* be flagged before anything gets kicked.
//
// See PLAN-AUTO-MOD.md for the full design.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

use once_cell::sync::Lazy;
use regex::Regex;
use serde::{Deserialize, Serialize};
use tokio::sync::RwLock;
use vector_sdk::IncomingMessage;

use crate::bot::BotContext;
use crate::config::AutoModSection;

// -----------------------------------------------------------------------------
// Paths
// -----------------------------------------------------------------------------

fn state_file() -> PathBuf {
    PathBuf::from("data/automod-state.json")
}

/// Runtime-tunable settings that `!automod` mutates. Persisted separately from
/// per-user state so operator changes survive a restart (config TOML only seeds
/// the *initial* values).
fn runtime_config_file() -> PathBuf {
    PathBuf::from("data/automod-config.json")
}

fn audit_log_file() -> PathBuf {
    PathBuf::from("data/automod-log.json")
}

/// How often (seconds) state is flushed to disk from the hot path.
const PERSIST_INTERVAL_SECS: u64 = 60;

/// How often the background flusher writes per-user state to disk.
const BACKGROUND_PERSIST_SECS: u64 = 300;

// -----------------------------------------------------------------------------
// Shared regexes (compiled once)
// -----------------------------------------------------------------------------

/// Matches http(s) URLs. Deliberately simple — we only need host extraction.
static URL_RE: Lazy<Regex> =
    Lazy::new(|| Regex::new(r"https?://[^\s<>()]+").expect("URL regex is valid"));

/// Matches bech32 npub mentions (with or without the `nostr:` prefix).
static NPUB_RE: Lazy<Regex> =
    Lazy::new(|| Regex::new(r"(?:nostr:)?npub1[0-9a-z]{20,}").expect("npub regex is valid"));

// -----------------------------------------------------------------------------
// Action + verdict
// -----------------------------------------------------------------------------

/// The action auto-mod recommends / took for a message.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum AutoModAction {
    /// Nothing user-facing (score below the warn threshold).
    None,
    /// Delete the message and warn the user.
    Warn,
    /// Kick the user from the community.
    Kick,
    /// Ban the user from the community.
    Ban,
}

impl AutoModAction {
    pub fn as_str(&self) -> &'static str {
        match self {
            AutoModAction::None => "none",
            AutoModAction::Warn => "warn",
            AutoModAction::Kick => "kick",
            AutoModAction::Ban => "ban",
        }
    }
}

/// The result of scanning a single message.
#[derive(Debug, Clone)]
pub struct AutoModVerdict {
    /// Total weighted score.
    pub score: u32,
    /// Which rules fired (for logging + announcements).
    pub rules_triggered: Vec<String>,
    /// Resolved action after thresholds + escalation.
    pub action: AutoModAction,
    /// True if the engine is in dry-run mode — the caller should NOT enforce.
    pub dry_run: bool,
    /// True when an equal-or-stronger action was already taken against this user
    /// inside `action_cooldown_secs`. The message is still deleted, but the
    /// kick/ban call, announcements, and counters are skipped so a concurrent
    /// spam burst produces one action, not one per message.
    pub suppressed: bool,
}

impl AutoModVerdict {
    /// Whether this verdict warrants a user-facing action (warn/kick/ban).
    pub fn should_act(&self) -> bool {
        self.action != AutoModAction::None
    }
}

// -----------------------------------------------------------------------------
// Persisted state
// -----------------------------------------------------------------------------

/// One recorded message (for burst + duplicate detection).
#[derive(Debug, Clone, Serialize, Deserialize)]
struct MsgRecord {
    /// Unix seconds.
    at: u64,
    /// Normalized text (lowercased, whitespace-collapsed).
    normalized: String,
}

/// One recorded violation (for escalation + history).
#[derive(Debug, Clone, Serialize, Deserialize)]
struct ViolationRecord {
    /// Unix seconds.
    at: u64,
    /// Comma-joined rule names.
    rules: String,
    score: u32,
    action: String,
}

/// Per-user tracking record.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
struct UserRecord {
    #[serde(default)]
    messages: Vec<MsgRecord>,
    #[serde(default)]
    violations: Vec<ViolationRecord>,
    /// When the user joined the community (unix secs), if observed.
    #[serde(default)]
    join_time: Option<u64>,
    /// Number of messages sent since joining (during grace window).
    #[serde(default)]
    msgs_since_join: u32,
    /// The last action resolved for this user: (unix secs, action). Drives the
    /// `action_cooldown_secs` de-duplication for concurrent message bursts.
    #[serde(default)]
    last_action: Option<(u64, String)>,
}

/// Runtime-tunable settings persisted across restarts.
///
/// The TOML `[automod]` section seeds these on first run; after that this file
/// wins, so `!automod on`, `!automod dryrun off`, `!automod words add …` and
/// friends survive a restart. Delete `data/automod-config.json` to fall back to
/// the TOML values.
#[derive(Debug, Clone, Serialize, Deserialize)]
struct RuntimeConfig {
    enabled: bool,
    /// Manual dry-run override: Some(true)=force on, Some(false)=force off, None=auto.
    #[serde(default)]
    forced_dry_run: Option<bool>,
    /// Unix secs when auto-mod was last switched from off → on. The automatic
    /// dry-run window is measured from here (NOT from process start), so a
    /// restart doesn't silently put a live engine back into log-only mode.
    #[serde(default)]
    enabled_at: Option<u64>,
    #[serde(default)]
    banned_words: Vec<String>,
    #[serde(default)]
    banned_patterns: Vec<String>,
    #[serde(default)]
    link_allowlist: Vec<String>,
}

/// The full persisted state.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
struct AutoModState {
    #[serde(default)]
    users: HashMap<String, UserRecord>,
    #[serde(default)]
    total_warns: u64,
    #[serde(default)]
    total_kicks: u64,
    #[serde(default)]
    total_bans: u64,
}

// -----------------------------------------------------------------------------
// Engine
// -----------------------------------------------------------------------------

/// Thread-safe auto-moderation engine. Clone freely — backed by `Arc`.
#[derive(Clone)]
pub struct AutoModEngine {
    inner: Arc<Inner>,
}

struct Inner {
    /// Runtime on/off (seeded from config, toggled via `!automod on/off`).
    enabled: RwLock<bool>,
    /// Lowercased banned words (runtime-mutable).
    banned_words: RwLock<Vec<String>>,
    /// Compiled regex patterns (runtime-mutable).
    patterns: RwLock<Vec<Regex>>,
    /// Original pattern source strings (for `!automod` listing / persistence).
    pattern_sources: RwLock<Vec<String>>,
    /// Link allowlist domains (runtime-mutable).
    link_allowlist: RwLock<Vec<String>>,
    /// Per-user state (persisted).
    state: RwLock<AutoModState>,
    /// Unix secs when auto-mod was last switched on — drives the automatic
    /// dry-run window. Persisted, so the window expires an hour after the
    /// operator enabled auto-mod rather than an hour after every restart.
    enabled_at: RwLock<Option<u64>>,
    /// Last time state was flushed to disk (unix secs).
    last_persist: RwLock<u64>,
    /// Manual dry-run override: Some(true)=force on, Some(false)=force off, None=auto.
    forced_dry_run: RwLock<Option<bool>>,
    /// False for test engines, so a test run can't read or clobber the real
    /// `data/automod-*.json` files.
    persist_to_disk: bool,
}

impl AutoModEngine {
    /// Build the engine from config. Compiles regex patterns (bad patterns are
    /// logged + skipped, never fatal) and loads persisted state if present.
    ///
    /// The TOML `[automod]` section seeds the runtime-tunable lists and the
    /// enabled flag; if `data/automod-config.json` exists it overrides them, so
    /// `!automod` changes made by an operator survive a restart.
    pub fn new(cfg: &AutoModSection) -> Self {
        Self::build(cfg, load_runtime_config(), load_state(), true)
    }

    /// Build an engine that never touches disk — for tests, so a developer's
    /// real `data/automod-*.json` files can't leak into (or be clobbered by) a
    /// test run.
    #[cfg(test)]
    fn new_ephemeral(cfg: &AutoModSection) -> Self {
        Self::build(cfg, None, AutoModState::default(), false)
    }

    fn build(
        cfg: &AutoModSection,
        saved: Option<RuntimeConfig>,
        state: AutoModState,
        persist_to_disk: bool,
    ) -> Self {
        let enabled = saved.as_ref().map(|s| s.enabled).unwrap_or(cfg.enabled);
        let forced_dry_run = saved.as_ref().and_then(|s| s.forced_dry_run);
        // No persisted enable-time but auto-mod is on → treat "now" as the start
        // of the dry-run window (first boot with `enabled = true` in TOML).
        let enabled_at = match saved.as_ref().and_then(|s| s.enabled_at) {
            Some(t) => Some(t),
            None if enabled => Some(now_secs()),
            None => None,
        };

        let word_src: Vec<String> = match saved.as_ref() {
            Some(s) => s.banned_words.clone(),
            None => cfg.banned_words.clone(),
        };
        let pattern_src: Vec<String> = match saved.as_ref() {
            Some(s) => s.banned_patterns.clone(),
            None => cfg.banned_patterns.clone(),
        };
        let allowlist: Vec<String> = match saved.as_ref() {
            Some(s) => s.link_allowlist.clone(),
            None => cfg.link_allowlist.clone(),
        };

        // Compile patterns; skip (with a warning) any that fail.
        let mut patterns = Vec::new();
        let mut pattern_sources = Vec::new();
        for p in &pattern_src {
            match Regex::new(p) {
                Ok(re) => {
                    patterns.push(re);
                    pattern_sources.push(p.clone());
                }
                Err(e) => {
                    tracing::error!("automod: skipping invalid banned_pattern {:?}: {}", p, e);
                }
            }
        }

        let banned_words: Vec<String> = word_src.iter().map(|w| w.to_lowercase()).collect();

        if saved.is_some() {
            tracing::info!(
                "automod: loaded runtime config (enabled={}, dry_run_override={:?}, words={}, patterns={}, allowlist={})",
                enabled,
                forced_dry_run,
                banned_words.len(),
                patterns.len(),
                allowlist.len()
            );
        }

        Self {
            inner: Arc::new(Inner {
                enabled: RwLock::new(enabled),
                banned_words: RwLock::new(banned_words),
                patterns: RwLock::new(patterns),
                pattern_sources: RwLock::new(pattern_sources),
                link_allowlist: RwLock::new(allowlist),
                state: RwLock::new(state),
                enabled_at: RwLock::new(enabled_at),
                last_persist: RwLock::new(now_secs()),
                forced_dry_run: RwLock::new(forced_dry_run),
                persist_to_disk,
            }),
        }
    }

    /// Snapshot the runtime-tunable settings and write them to disk. Called
    /// after every `!automod` mutation so operator changes survive a restart.
    async fn save_runtime(&self) {
        if !self.inner.persist_to_disk {
            return;
        }
        let cfg = RuntimeConfig {
            enabled: *self.inner.enabled.read().await,
            forced_dry_run: *self.inner.forced_dry_run.read().await,
            enabled_at: *self.inner.enabled_at.read().await,
            banned_words: self.inner.banned_words.read().await.clone(),
            banned_patterns: self.inner.pattern_sources.read().await.clone(),
            link_allowlist: self.inner.link_allowlist.read().await.clone(),
        };
        save_runtime_config(&cfg);
    }

    /// Spawn a background task that flushes per-user state to disk periodically.
    ///
    /// Without this, state is only written when an enforcement action fires, so
    /// join times (needed for the new-user flooding rule) and recent message
    /// history are lost on a clean restart.
    pub fn spawn_persistence_task(&self) {
        let engine = self.clone();
        tokio::spawn(async move {
            let mut ticker =
                tokio::time::interval(std::time::Duration::from_secs(BACKGROUND_PERSIST_SECS));
            ticker.tick().await; // the first tick fires immediately — skip it
            loop {
                ticker.tick().await;
                engine.persist().await;
            }
        });
    }

    // ---- runtime toggles --------------------------------------------------

    pub async fn is_enabled(&self) -> bool {
        *self.inner.enabled.read().await
    }

    pub async fn set_enabled(&self, on: bool) {
        let was = {
            let mut enabled = self.inner.enabled.write().await;
            let was = *enabled;
            *enabled = on;
            was
        };
        // Start the automatic dry-run window on the off → on transition only, so
        // toggling an already-live engine doesn't re-arm log-only mode.
        if on && !was {
            *self.inner.enabled_at.write().await = Some(now_secs());
        }
        self.save_runtime().await;
    }

    /// True while in dry-run (log-only) mode — either time-based or manually forced.
    pub async fn in_dry_run(&self, cfg: &AutoModSection) -> bool {
        let forced = *self.inner.forced_dry_run.read().await;
        match forced {
            Some(true) => true,
            Some(false) => false,
            None => {
                if cfg.dry_run_minutes == 0 {
                    return false;
                }
                match *self.inner.enabled_at.read().await {
                    Some(at) => now_secs().saturating_sub(at) < cfg.dry_run_minutes * 60,
                    None => false,
                }
            }
        }
    }

    /// Manually set dry-run override.
    /// `Some(true)` = force on, `Some(false)` = force off, `None` = auto (time-based).
    pub async fn set_dry_run(&self, mode: Option<bool>) {
        *self.inner.forced_dry_run.write().await = mode;
        self.save_runtime().await;
    }

    // ---- banned words -----------------------------------------------------

    pub async fn add_word(&self, word: &str) -> bool {
        let w = word.trim().to_lowercase();
        if w.is_empty() {
            return false;
        }
        {
            let mut words = self.inner.banned_words.write().await;
            if words.iter().any(|x| x == &w) {
                return false;
            }
            words.push(w);
        }
        self.save_runtime().await;
        true
    }

    pub async fn remove_word(&self, word: &str) -> bool {
        let w = word.trim().to_lowercase();
        let changed = {
            let mut words = self.inner.banned_words.write().await;
            let before = words.len();
            words.retain(|x| x != &w);
            words.len() != before
        };
        if changed {
            self.save_runtime().await;
        }
        changed
    }

    pub async fn list_words(&self) -> Vec<String> {
        self.inner.banned_words.read().await.clone()
    }

    // ---- link allowlist ---------------------------------------------------

    pub async fn add_allowlist(&self, domain: &str) -> bool {
        let d = domain.trim().to_lowercase();
        if d.is_empty() {
            return false;
        }
        {
            let mut list = self.inner.link_allowlist.write().await;
            if list.iter().any(|x| x.to_lowercase() == d) {
                return false;
            }
            list.push(d);
        }
        self.save_runtime().await;
        true
    }

    pub async fn remove_allowlist(&self, domain: &str) -> bool {
        let d = domain.trim().to_lowercase();
        let changed = {
            let mut list = self.inner.link_allowlist.write().await;
            let before = list.len();
            list.retain(|x| x.to_lowercase() != d);
            list.len() != before
        };
        if changed {
            self.save_runtime().await;
        }
        changed
    }

    pub async fn list_allowlist(&self) -> Vec<String> {
        self.inner.link_allowlist.read().await.clone()
    }

    pub async fn list_patterns(&self) -> Vec<String> {
        self.inner.pattern_sources.read().await.clone()
    }

    // ---- join tracking ----------------------------------------------------

    /// Record that `npub` joined a community (for new-user flooding checks).
    pub async fn record_join(&self, npub: &str) {
        let now = now_secs();
        let mut state = self.inner.state.write().await;
        let rec = state.users.entry(npub.to_string()).or_default();
        rec.join_time = Some(now);
        rec.msgs_since_join = 0;
    }

    /// A member left (or was kicked from) a channel.
    ///
    /// Clears the transient buffers — recent messages and the new-user grace
    /// counters — but deliberately KEEPS the violation history. A kick emits a
    /// `MemberLeave`, so dropping violations here would reset the offender's
    /// escalation counter every time they were kicked: they could rejoin, spam,
    /// get kicked, and never escalate to a ban. Violations age out on their own
    /// via `escalation_window_secs`.
    pub async fn on_member_leave(&self, npub: &str) {
        let mut state = self.inner.state.write().await;
        if let Some(rec) = state.users.get_mut(npub) {
            rec.messages.clear();
            rec.join_time = None;
            rec.msgs_since_join = 0;
            // Drop users with nothing left worth remembering.
            if rec.violations.is_empty() {
                state.users.remove(npub);
            }
        }
    }

    /// Reset a user's violation history (but keep them tracked).
    pub async fn reset_user(&self, npub: &str) -> bool {
        let mut state = self.inner.state.write().await;
        if let Some(rec) = state.users.get_mut(npub) {
            let had = !rec.violations.is_empty() || !rec.messages.is_empty();
            rec.violations.clear();
            rec.messages.clear();
            rec.msgs_since_join = 0;
            rec.last_action = None;
            return had;
        }
        false
    }

    // ---- immunity ---------------------------------------------------------

    /// Whether the sender bypasses ALL auto-mod checks.
    ///
    /// Owner and the bot itself are always immune. Admins are immune. Authorized
    /// users are immune unless `strict_mode` is on.
    pub fn is_immune(
        &self,
        ctx: &BotContext,
        npub: &str,
        community_id: Option<&str>,
        is_admin: bool,
        strict_mode: bool,
    ) -> bool {
        if npub.is_empty() {
            return false;
        }
        if npub == ctx.bot.npub() {
            return true;
        }
        if let Some(ref auth) = ctx.auth {
            if auth.is_owner(npub) {
                return true;
            }
            if !strict_mode && auth.is_authorized(npub, community_id) {
                return true;
            }
        }
        is_admin
    }

    // ---- stats ------------------------------------------------------------

    /// Returns (tracked_users, total_warns, total_kicks, total_bans).
    pub async fn stats(&self) -> (usize, u64, u64, u64) {
        let state = self.inner.state.read().await;
        (
            state.users.len(),
            state.total_warns,
            state.total_kicks,
            state.total_bans,
        )
    }

    /// Recent action history, most-recent first. If `npub` is Some, filter to
    /// that user. Returns formatted strings.
    pub async fn history(&self, npub: Option<&str>, limit: usize) -> Vec<String> {
        let state = self.inner.state.read().await;
        let mut out: Vec<(u64, String)> = Vec::new();
        for (user, rec) in &state.users {
            if let Some(filter) = npub {
                if user != filter {
                    continue;
                }
            }
            for v in &rec.violations {
                out.push((
                    v.at,
                    format!(
                        "{} — {} (score {}, {}) [{}]",
                        fmt_ts(v.at),
                        short_npub(user),
                        v.score,
                        v.action,
                        v.rules
                    ),
                ));
            }
        }
        out.sort_by_key(|(ts, _)| std::cmp::Reverse(*ts)); // most-recent first
        out.into_iter().take(limit).map(|(_, s)| s).collect()
    }

    // ---- core scan --------------------------------------------------------

    /// Scan a message and return a verdict. Records the message into per-user
    /// history (needed for burst/duplicate/new-user rules). Does NOT enforce —
    /// call [`execute_action`] with the verdict for that.
    pub async fn check_message(
        &self,
        cfg: &AutoModSection,
        npub: &str,
        text: &str,
        _channel_id: &str,
    ) -> AutoModVerdict {
        let now = now_secs();
        let normalized = normalize_text(text);

        let mut score: u32 = 0;
        let mut rules: Vec<String> = Vec::new();

        let mut state = self.inner.state.write().await;
        let rec = state.users.entry(npub.to_string()).or_default();

        // Prune stale message history (keep the widest window we care about).
        let max_window = cfg
            .burst_window_secs
            .max(cfg.dedupe_window_secs)
            .max(cfg.new_user_grace_secs);
        rec.messages.retain(|m| now.saturating_sub(m.at) <= max_window);
        // Prune stale violations beyond the escalation window.
        rec.violations
            .retain(|v| now.saturating_sub(v.at) <= cfg.escalation_window_secs);

        // Record this message BEFORE counting (so bursts include it).
        rec.messages.push(MsgRecord {
            at: now,
            normalized: normalized.clone(),
        });

        // --- Rule 1: rate burst -------------------------------------------
        let burst_count = rec
            .messages
            .iter()
            .filter(|m| now.saturating_sub(m.at) <= cfg.burst_window_secs)
            .count() as u32;
        if burst_count > cfg.max_messages {
            score += 3;
            rules.push("rate_burst".to_string());
        }

        // --- Rule 2: duplicate content ------------------------------------
        if !normalized.is_empty() {
            let dup_count = rec
                .messages
                .iter()
                .filter(|m| {
                    now.saturating_sub(m.at) <= cfg.dedupe_window_secs
                        && m.normalized == normalized
                })
                .count() as u32;
            if dup_count >= cfg.max_duplicates {
                score += 4;
                rules.push("duplicate_content".to_string());
            }
        }

        // --- Rule 6 prep: new-user flooding -------------------------------
        // (computed here so we know new-user status for stricter link scoring)
        let is_new_user = match rec.join_time {
            Some(jt) => now.saturating_sub(jt) <= cfg.new_user_grace_secs,
            None => false,
        };
        if is_new_user {
            rec.msgs_since_join = rec.msgs_since_join.saturating_add(1);
            if rec.msgs_since_join > cfg.new_user_max_msgs {
                score += 4;
                rules.push("new_user_flooding".to_string());
            }
        }

        // We've finished touching per-user state; drop the lock before the
        // stateless content checks to keep the critical section short.
        drop(state);

        // --- Rule 3: banned keywords / patterns ---------------------------
        {
            let lower = text.to_lowercase();
            let words = self.inner.banned_words.read().await;
            let mut keyword_hits = 0u32;
            for w in words.iter() {
                if contains_banned_word(&lower, w) {
                    keyword_hits += 1;
                }
            }
            drop(words);
            let patterns = self.inner.patterns.read().await;
            for re in patterns.iter() {
                if re.is_match(text) {
                    keyword_hits += 1;
                }
            }
            if keyword_hits > 0 {
                score += 5 * keyword_hits;
                rules.push("banned_keyword".to_string());
            }
        }

        // --- Rule 4: link filtering ---------------------------------------
        if cfg.link_action != "off" {
            let allowlist = self.inner.link_allowlist.read().await;
            let mut bad_links = 0u32;
            for m in URL_RE.find_iter(text) {
                let url = m.as_str();
                if !is_allowlisted(url, &allowlist) {
                    bad_links += 1;
                }
            }
            drop(allowlist);
            if bad_links > 0 {
                let per = if is_new_user { 4 } else { 2 };
                score += per * bad_links;
                rules.push("link_filter".to_string());
                // "block" mode: guarantee at least a warn even for one link.
                if cfg.link_action == "block" && score < cfg.warn_threshold {
                    score = cfg.warn_threshold;
                }
            }
            // Too many links is spammy regardless of allowlist.
            let total_links = URL_RE.find_iter(text).count() as u32;
            if total_links > cfg.max_links && !rules.contains(&"link_filter".to_string()) {
                score += 2;
                rules.push("link_filter".to_string());
            }
        }

        // --- Rule 5: mention spam -----------------------------------------
        let mention_count = NPUB_RE.find_iter(text).count() as u32;
        if mention_count > cfg.max_mentions {
            score += 3;
            rules.push("mention_spam".to_string());
        }

        // --- Rule 7: caps / wall of text ----------------------------------
        if text.len() >= cfg.caps_min_length {
            let caps_pct = caps_percentage(text);
            if caps_pct >= cfg.caps_threshold_pct {
                score += 1;
                rules.push("caps".to_string());
            }
        }
        if text.len() > cfg.max_msg_length {
            score += 1;
            if !rules.contains(&"wall_of_text".to_string()) {
                rules.push("wall_of_text".to_string());
            }
        }

        // --- Resolve base action from score -------------------------------
        let mut action = if score >= cfg.ban_threshold {
            AutoModAction::Ban
        } else if score >= cfg.kick_threshold {
            AutoModAction::Kick
        } else if score >= cfg.warn_threshold {
            AutoModAction::Warn
        } else {
            AutoModAction::None
        };

        // --- Escalation + violation recording (one critical section) -------
        //
        // Messages are dispatched one tokio task each, so counting prior
        // violations and appending this one MUST happen under a single write
        // lock. Doing the count here and the append in execute_action (as an
        // earlier cut did) let a concurrent burst all observe `prior == 0`,
        // so escalation never fired against exactly the spam it exists for.
        let mut suppressed = false;
        if action != AutoModAction::None {
            let mut state = self.inner.state.write().await;
            let rec = state.users.entry(npub.to_string()).or_default();

            let prior = rec
                .violations
                .iter()
                .filter(|v| now.saturating_sub(v.at) <= cfg.escalation_window_secs)
                .count() as u32;
            // This is the (prior + 1)-th violation in the window.
            let this_violation_number = prior + 1;
            if cfg.escalation_ban_after > 0 && this_violation_number >= cfg.escalation_ban_after {
                action = action.max(AutoModAction::Ban);
                if !rules.contains(&"escalation".to_string()) {
                    rules.push("escalation".to_string());
                }
            } else if cfg.escalation_kick_after > 0
                && this_violation_number >= cfg.escalation_kick_after
            {
                action = action.max(AutoModAction::Kick);
                if !rules.contains(&"escalation".to_string()) {
                    rules.push("escalation".to_string());
                }
            }

            // Was an equal-or-stronger action already taken very recently? If so
            // this is the tail of a burst we've already handled — delete the
            // message but don't re-kick, re-announce, or re-count.
            if let Some((at, ref last)) = rec.last_action {
                if now.saturating_sub(at) <= cfg.action_cooldown_secs
                    && action_rank(last) >= action as u8
                {
                    suppressed = true;
                }
            }

            if !suppressed {
                rec.violations.push(ViolationRecord {
                    at: now,
                    rules: rules.join(","),
                    score,
                    action: action.as_str().to_string(),
                });
                rec.last_action = Some((now, action.as_str().to_string()));
            }
        }

        AutoModVerdict {
            score,
            rules_triggered: rules,
            action,
            dry_run: self.in_dry_run(cfg).await,
            suppressed,
        }
    }

    // ---- enforcement ------------------------------------------------------

    /// Enforce a verdict: delete the message (if configured), kick/ban the user
    /// via the SDK, announce, DM the owner, and write an audit log line.
    /// Degrades gracefully on permission errors. The violation itself was
    /// already recorded by [`check_message`].
    ///
    /// Returns `true` if the message was "handled" (caller should stop
    /// dispatching a command for it).
    pub async fn execute_action(
        &self,
        cfg: &AutoModSection,
        ctx: &BotContext,
        msg: &IncomingMessage,
        npub: &str,
        verdict: &AutoModVerdict,
    ) -> bool {
        if verdict.action == AutoModAction::None {
            // Silent flag: log only, no enforcement, no violation record.
            if verdict.score > 0 {
                tracing::debug!(
                    "automod: silent flag npub={} score={} rules={:?}",
                    short_npub(npub),
                    verdict.score,
                    verdict.rules_triggered
                );
            }
            return false;
        }

        let rules_joined = verdict.rules_triggered.join(",");
        let snippet: String = msg.text().chars().take(80).collect();

        // A repeat action inside the cooldown: the offender was already kicked /
        // banned for this burst. Still remove the message, but stay quiet —
        // otherwise a 10-message flood becomes 10 bans and 10 announcements.
        if verdict.suppressed {
            tracing::debug!(
                "automod: suppressed duplicate {} for {} (recent action within cooldown)",
                verdict.action.as_str(),
                short_npub(npub)
            );
            if !verdict.dry_run && cfg.delete_spam_messages {
                self.delete_message(ctx, msg).await;
            }
            return true;
        }

        // The violation was recorded in check_message (atomically with the
        // escalation count). Here we only bump the lifetime counters, and only
        // for real enforcement — dry-run keeps escalation tracking accurate
        // without inflating the "actions taken" stats.
        if !verdict.dry_run {
            let mut state = self.inner.state.write().await;
            match verdict.action {
                AutoModAction::Warn => state.total_warns += 1,
                AutoModAction::Kick => state.total_kicks += 1,
                AutoModAction::Ban => state.total_bans += 1,
                AutoModAction::None => {}
            }
        }

        // Audit log always (records dry-run flag too).
        write_audit_log(npub, verdict, &msg.chat_id, &snippet);

        if verdict.dry_run {
            tracing::info!(
                "automod[DRY-RUN]: WOULD {} npub={} score={} rules={} — no action taken",
                verdict.action.as_str(),
                short_npub(npub),
                verdict.score,
                rules_joined
            );
            // Announce in-channel so operators can see what WOULD be flagged.
            let dry_msg = format!(
                "🟡 **DRY-RUN** — would {} {} (score: {}, rules: {}) — no action taken",
                verdict.action.as_str(),
                short_npub(npub),
                verdict.score,
                rules_joined
            );
            let _ = ctx.bot.channel(msg.chat_id.clone()).send(&dry_msg).await;
            self.maybe_persist().await;
            return true;
        }

        tracing::warn!(
            "automod: {} npub={} score={} rules={}",
            verdict.action.as_str(),
            short_npub(npub),
            verdict.score,
            rules_joined
        );

        // 1. Delete the offending message if configured. Do this BEFORE a ban —
        //    banning a member of a private community triggers a read-cut rekey,
        //    after which the deletion may no longer land for existing members.
        if cfg.delete_spam_messages {
            self.delete_message(ctx, msg).await;
        }

        // 2. Take the moderation action.
        let community = msg.community();
        let mut action_ok = true;
        let mut permission_gap = false;

        tracing::info!(
            "automod: enforce — action={:?} npub={} community={}",
            verdict.action,
            short_npub(npub),
            if community.is_some() { "present" } else { "NONE" }
        );

        match verdict.action {
            AutoModAction::Kick | AutoModAction::Ban => {
                if let Some(ref community) = community {
                    let member = community.member(npub);
                    // Never act on the owner (defensive; immunity should catch this).
                    if member.is_owner() {
                        tracing::warn!("automod: refusing to {} community owner", verdict.action.as_str());
                        action_ok = false;
                    } else {
                        tracing::info!(
                            "automod: calling {}.await on npub={}",
                            verdict.action.as_str(),
                            short_npub(npub)
                        );
                        let res = if verdict.action == AutoModAction::Ban {
                            member.ban().await
                        } else {
                            member.kick().await
                        };
                        match &res {
                            Ok(()) => {
                                tracing::info!("automod: {} succeeded for {}", verdict.action.as_str(), short_npub(npub));
                            }
                            Err(e) => {
                                action_ok = false;
                                tracing::error!("automod: {} FAILED for {}: {:?}", verdict.action.as_str(), short_npub(npub), e);
                                if is_permission_error(&e) {
                                    permission_gap = true;
                                }
                            }
                        }
                    }
                } else {
                    action_ok = false;
                    // community() resolves via the channel → community mapping in
                    // the SDK's local DB. If that row is missing (bot joined
                    // before the mapping was written, or sync_communities hasn't
                    // caught up) there is nothing to kick them *from*.
                    tracing::error!(
                        "automod: CANNOT {} {} — no community resolved for channel {}; \
                         the channel→community mapping is missing locally",
                        verdict.action.as_str(),
                        short_npub(npub),
                        msg.chat_id
                    );
                }
            }
            AutoModAction::Warn | AutoModAction::None => {}
        }

        // 3. Announce in-channel. Use `nostr:npub…` so clients render a mention
        //    rather than a wall of bech32 (matches the level-up announcements).
        if cfg.announce_actions {
            let mention = format!("nostr:{}", npub);
            let announcement = if permission_gap {
                format!(
                    "⚠️ Detected spam from {} (score {}) but I lack permission to {}. Rules: {}",
                    mention,
                    verdict.score,
                    verdict.action.as_str(),
                    rules_joined
                )
            } else {
                match verdict.action {
                    AutoModAction::Warn => format!(
                        "🚫 {} — message removed for spam ({}). Please knock it off.",
                        mention, rules_joined
                    ),
                    AutoModAction::Kick if action_ok => {
                        format!("👢 Kicked {} for spam ({}).", mention, rules_joined)
                    }
                    AutoModAction::Ban if action_ok => {
                        format!("🔨 Banned {} for spam ({}).", mention, rules_joined)
                    }
                    _ => String::new(),
                }
            };
            if !announcement.is_empty() {
                let _ = ctx.bot.channel(msg.chat_id.clone()).send(&announcement).await;
            }
        }

        // 4. DM the owner on bans (or permission gaps on kick/ban).
        if cfg.announce_dm_owner && (verdict.action == AutoModAction::Ban || permission_gap) {
            if let Some(ref auth) = ctx.auth {
                if let Some(owner) = auth.owner_npub() {
                    let dm = if permission_gap {
                        format!(
                            "⚠️ auto-mod wanted to {} {} for spam (score {}, {}) but I lack permission in community {}.",
                            verdict.action.as_str(),
                            npub,
                            verdict.score,
                            rules_joined,
                            msg.chat_id
                        )
                    } else {
                        format!(
                            "🔨 auto-mod banned {} for spam (score {}, rules: {}) in {}.",
                            npub, verdict.score, rules_joined, msg.chat_id
                        )
                    };
                    let _ = ctx.bot.dm(&owner).send(&dm).await;
                }
            }
        }

        self.maybe_persist().await;
        true
    }

    /// Delete a flagged message.
    ///
    /// Goes through `delete_community_message_in` rather than the SDK's
    /// `Channel::delete`: the latter calls `delete_community_message`, which
    /// resolves the channel by looking the message up in local state — the GUI
    /// path. A headless bot keeps no such history, so that call fails with
    /// "message not found" even when the deletion itself would be valid. We
    /// already know the channel id, so pass it explicitly.
    async fn delete_message(&self, ctx: &BotContext, msg: &IncomingMessage) {
        match ctx
            .bot
            .core()
            .delete_community_message_in(&msg.chat_id, &msg.message.id)
            .await
        {
            Ok(()) => tracing::debug!("automod: deleted message {}", msg.message.id),
            Err(e) => tracing::warn!(
                "automod: could not delete message {}: {:?}",
                msg.message.id,
                e
            ),
        }
    }

    // ---- persistence ------------------------------------------------------

    /// Flush state to disk if enough time has elapsed since the last flush.
    async fn maybe_persist(&self) {
        let now = now_secs();
        {
            let last = *self.inner.last_persist.read().await;
            if now.saturating_sub(last) < PERSIST_INTERVAL_SECS {
                return;
            }
        }
        *self.inner.last_persist.write().await = now;
        self.persist().await;
    }

    /// Force a state flush to disk.
    pub async fn persist(&self) {
        if !self.inner.persist_to_disk {
            return;
        }
        let state = self.inner.state.read().await;
        save_state(&state);
    }
}

// -----------------------------------------------------------------------------
// Free helpers
// -----------------------------------------------------------------------------

fn now_secs() -> u64 {
    chrono::Utc::now().timestamp().max(0) as u64
}

/// Severity rank of a persisted action string, for cooldown comparison.
fn action_rank(action: &str) -> u8 {
    match action {
        "ban" => AutoModAction::Ban as u8,
        "kick" => AutoModAction::Kick as u8,
        "warn" => AutoModAction::Warn as u8,
        _ => AutoModAction::None as u8,
    }
}

/// Whether `needle` (already lowercased) occurs in `haystack` (already
/// lowercased) as a banned term.
///
/// A multi-word phrase matches as a plain substring. A single word must match on
/// word boundaries — otherwise banning "scam" also bans "scamper" and
/// "descambiar", and since one keyword hit scores +5 (>= the default kick
/// threshold) those false positives get people kicked on their first message.
fn contains_banned_word(haystack: &str, needle: &str) -> bool {
    if needle.is_empty() {
        return false;
    }
    if needle.contains(char::is_whitespace) {
        return haystack.contains(needle);
    }
    let mut from = 0usize;
    while let Some(rel) = haystack[from..].find(needle) {
        let start = from + rel;
        let end = start + needle.len();
        let before_ok = haystack[..start]
            .chars()
            .next_back()
            .map(|c| !c.is_alphanumeric())
            .unwrap_or(true);
        let after_ok = haystack[end..]
            .chars()
            .next()
            .map(|c| !c.is_alphanumeric())
            .unwrap_or(true);
        if before_ok && after_ok {
            return true;
        }
        // Advance past this occurrence's first char to keep scanning.
        from = start + haystack[start..].chars().next().map(|c| c.len_utf8()).unwrap_or(1);
        if from >= haystack.len() {
            break;
        }
    }
    false
}

/// Normalize text for duplicate comparison: lowercase + collapse whitespace.
fn normalize_text(text: &str) -> String {
    text.split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_lowercase()
}

/// Percentage (0–100) of alphabetic characters that are uppercase.
fn caps_percentage(text: &str) -> u32 {
    let mut upper = 0usize;
    let mut letters = 0usize;
    for c in text.chars() {
        if c.is_alphabetic() {
            letters += 1;
            if c.is_uppercase() {
                upper += 1;
            }
        }
    }
    match (upper * 100).checked_div(letters) {
        Some(pct) => pct as u32,
        None => 0,
    }
}

/// Extract the host from a URL and check it against the allowlist (matches the
/// domain or any subdomain of an allowlisted domain).
fn is_allowlisted(url: &str, allowlist: &[String]) -> bool {
    let host = url_host(url);
    if host.is_empty() {
        return false;
    }
    let host = host.to_lowercase();
    allowlist.iter().any(|d| {
        let d = d.to_lowercase();
        host == d || host.ends_with(&format!(".{}", d))
    })
}

/// Extract the host portion of an http(s) URL.
fn url_host(url: &str) -> String {
    let after_scheme = url
        .strip_prefix("https://")
        .or_else(|| url.strip_prefix("http://"))
        .unwrap_or(url);
    // host ends at the first '/', '?', '#', or ':'
    let end = after_scheme
        .find(['/', '?', '#', ':'])
        .unwrap_or(after_scheme.len());
    let mut host = &after_scheme[..end];
    // strip userinfo if present
    if let Some(at) = host.rfind('@') {
        host = &host[at + 1..];
    }
    host.to_string()
}

/// Shorten an npub for logs / announcements.
fn short_npub(npub: &str) -> String {
    if npub.len() > 16 {
        format!("{}…{}", &npub[..10], &npub[npub.len() - 4..])
    } else {
        npub.to_string()
    }
}

/// Format a unix timestamp as a compact UTC string.
fn fmt_ts(ts: u64) -> String {
    chrono::DateTime::from_timestamp(ts as i64, 0)
        .map(|dt| dt.format("%Y-%m-%d %H:%M UTC").to_string())
        .unwrap_or_else(|| ts.to_string())
}

/// Check whether an SDK error looks like a permission/rank error.
fn is_permission_error<E: std::fmt::Debug>(e: &E) -> bool {
    let err = format!("{:?}", e).to_lowercase();
    err.contains("permission")
        || err.contains("outrank")
        || err.contains("rank")
        || err.contains("denied")
        || err.contains("unauthorized")
        || err.contains("forbidden")
        || err.contains("not allowed")
}

// -----------------------------------------------------------------------------
// Disk I/O
// -----------------------------------------------------------------------------

fn load_state() -> AutoModState {
    match std::fs::read_to_string(state_file()) {
        Ok(contents) => match serde_json::from_str::<AutoModState>(&contents) {
            Ok(s) => {
                tracing::info!("automod: loaded state ({} users tracked)", s.users.len());
                s
            }
            Err(e) => {
                tracing::warn!("automod: state file corrupt ({}) — starting fresh", e);
                AutoModState::default()
            }
        },
        Err(_) => AutoModState::default(),
    }
}

fn save_state(state: &AutoModState) {
    write_json_atomic(&state_file(), state, "state");
}

fn load_runtime_config() -> Option<RuntimeConfig> {
    let contents = std::fs::read_to_string(runtime_config_file()).ok()?;
    match serde_json::from_str::<RuntimeConfig>(&contents) {
        Ok(c) => Some(c),
        Err(e) => {
            tracing::warn!(
                "automod: runtime config corrupt ({}) — falling back to bot.toml [automod]",
                e
            );
            None
        }
    }
}

fn save_runtime_config(cfg: &RuntimeConfig) {
    write_json_atomic(&runtime_config_file(), cfg, "runtime config");
}

/// Serialize to a temp file and rename into place, so an interrupted write
/// (SIGTERM during a redeploy) can't leave a half-written JSON file behind that
/// silently resets auto-mod on the next boot.
fn write_json_atomic<T: Serialize>(path: &PathBuf, value: &T, what: &str) {
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let json = match serde_json::to_string_pretty(value) {
        Ok(j) => j,
        Err(e) => {
            tracing::error!("automod: failed to serialize {}: {}", what, e);
            return;
        }
    };
    let tmp = path.with_extension("json.tmp");
    if let Err(e) = std::fs::write(&tmp, json) {
        tracing::error!("automod: failed to write {}: {}", what, e);
        return;
    }
    if let Err(e) = std::fs::rename(&tmp, path) {
        tracing::error!("automod: failed to commit {}: {}", what, e);
        let _ = std::fs::remove_file(&tmp);
    }
}

/// Append one JSONL line to the audit log.
fn write_audit_log(npub: &str, verdict: &AutoModVerdict, channel: &str, snippet: &str) {
    use std::io::Write;

    let path = audit_log_file();
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }

    let entry = serde_json::json!({
        "timestamp": chrono::Utc::now().to_rfc3339(),
        "npub": npub,
        "action": verdict.action.as_str(),
        "score": verdict.score,
        "rules": verdict.rules_triggered,
        "channel": channel,
        "message_snippet": snippet,
        "dry_run": verdict.dry_run,
    });

    match std::fs::OpenOptions::new().create(true).append(true).open(&path) {
        Ok(mut f) => {
            if let Err(e) = writeln!(f, "{}", entry) {
                tracing::error!("automod: failed to append audit log: {}", e);
            }
        }
        Err(e) => tracing::error!("automod: failed to open audit log: {}", e),
    }
}

// -----------------------------------------------------------------------------
// Integration glue — called from handlers::on_message()
// -----------------------------------------------------------------------------

/// Auto-mod entry point for a single community message.
///
/// Returns `true` if auto-mod handled (enforced on) the message, in which case
/// the caller should NOT dispatch a command for it. Returns `false` to let the
/// message flow through normally.
///
/// Fast path: immune users and empty scores return quickly. The engine's
/// runtime `enabled` flag is honored in addition to the config flag (checked by
/// the caller), so `!automod off` takes effect without a restart.
pub async fn on_message(ctx: &BotContext, msg: &IncomingMessage, npub: &str) -> bool {
    let engine = &ctx.automod;
    let cfg = &ctx.config.automod;

    // Runtime toggle (independent of the config flag the caller checked).
    if !engine.is_enabled().await {
        return false;
    }

    // Immunity: owner + bot always bypass; admins bypass; authorized bypass
    // unless strict_mode. Determining admin status requires a community lookup.
    let community_id = msg.community().map(|c| c.id().to_string());
    let is_admin = msg.member().map(|m| m.is_admin()).unwrap_or(false);
    if engine.is_immune(ctx, npub, community_id.as_deref(), is_admin, cfg.strict_mode) {
        return false;
    }

    let verdict = engine
        .check_message(cfg, npub, msg.text(), &msg.chat_id)
        .await;

    if !verdict.should_act() {
        // Log-only for sub-threshold flags; nothing enforced.
        if verdict.score > 0 {
            tracing::debug!(
                "automod: sub-threshold npub={} score={} rules={:?}",
                short_npub(npub),
                verdict.score,
                verdict.rules_triggered
            );
        }
        return false;
    }

    engine.execute_action(cfg, ctx, msg, npub, &verdict).await
}

// -----------------------------------------------------------------------------
// !automod command group
// -----------------------------------------------------------------------------
//
// Auth is enforced by the dispatcher in commands.rs (per-subcommand), but each
// handler re-checks owner-level operations defensively via the passed `is_owner`
// flag so it can't be misused.

/// Handle the `!automod ...` command. `args` is everything after `!automod`.
/// `is_owner` indicates whether the sender is the bot owner (owner-gated
/// subcommands are refused otherwise).
pub async fn automod_command(
    ctx: &BotContext,
    msg: &IncomingMessage,
    args: &str,
    is_owner: bool,
) -> anyhow::Result<()> {
    let engine = &ctx.automod;
    let cfg = &ctx.config.automod;
    let parts: Vec<&str> = args.split_whitespace().collect();
    let sub = parts.first().copied().unwrap_or("");

    match sub {
        "on" | "off" => {
            if !is_owner {
                super::reply(ctx, msg, "⛔ Owner only.").await?;
                return Ok(());
            }
            let on = sub == "on";
            engine.set_enabled(on).await;
            let state = if on { "ON ✅" } else { "OFF ❌" };
            let extra = if on && engine.in_dry_run(cfg).await {
                format!(
                    "\n⚠️ Running in DRY-RUN (log-only) for {} min from now — no kicks/bans yet.\nUse `!automod dryrun off` to go live immediately.",
                    cfg.dry_run_minutes
                )
            } else {
                String::new()
            };
            super::reply(ctx, msg, &format!("🛡️ Auto-mod turned {}{}", state, extra)).await?;
        }

        "dryrun" => {
            let action = parts.get(1).copied().unwrap_or("");
            match action {
                "on" => {
                    if !is_owner {
                        super::reply(ctx, msg, "⛔ Owner only.").await?;
                        return Ok(());
                    }
                    engine.set_dry_run(Some(true)).await;
                    super::reply(ctx, msg, "🟡 Dry-run FORCED ON — automod will log + announce in channel but take no real action until you turn it off.").await?;
                }
                "off" => {
                    if !is_owner {
                        super::reply(ctx, msg, "⛔ Owner only.").await?;
                        return Ok(());
                    }
                    engine.set_dry_run(Some(false)).await;
                    super::reply(ctx, msg, "🔴 Dry-run FORCED OFF — automod is now LIVE and will take real action.").await?;
                }
                "auto" => {
                    if !is_owner {
                        super::reply(ctx, msg, "⛔ Owner only.").await?;
                        return Ok(());
                    }
                    engine.set_dry_run(None).await;
                    super::reply(ctx, msg, &format!("🔄 Dry-run reset to auto (time-based: first {} min after auto-mod was switched on).", cfg.dry_run_minutes)).await?;
                }
                "" | "status" => {
                    let forced = *engine.inner.forced_dry_run.read().await;
                    let time_based = engine.in_dry_run(cfg).await;
                    let mode = match forced {
                        Some(true) => "FORCED ON (manual)",
                        Some(false) => "FORCED OFF (manual)",
                        None => if time_based { "ON (time-based)" } else { "OFF (time-based window expired)" },
                    };
                    super::reply(ctx, msg, &format!("🟡 Dry-run: {}\n• !automod dryrun on — force on\n• !automod dryrun off — force off (go live)\n• !automod dryrun auto — reset to time-based", mode)).await?;
                }
                _ => {
                    super::reply(ctx, msg, "Usage: !automod dryrun <on|off|auto|status>").await?;
                }
            }
        }

        "status" | "" => {
            let enabled = engine.is_enabled().await;
            let (tracked, warns, kicks, bans) = engine.stats().await;
            let words = engine.list_words().await;
            let patterns = engine.list_patterns().await;
            let allow = engine.list_allowlist().await;
            let forced = *engine.inner.forced_dry_run.read().await;
            let dry_label = match forced {
                Some(true) => "FORCED ON (manual)".to_string(),
                Some(false) => "OFF (forced)".to_string(),
                None => {
                    if engine.in_dry_run(cfg).await {
                        format!("ON ({}min window)", cfg.dry_run_minutes)
                    } else {
                        "OFF (expired)".to_string()
                    }
                }
            };
            let dry = if enabled && engine.in_dry_run(cfg).await { " (DRY-RUN)" } else { "" };
            let text = format!(
                "🛡️ Auto-mod status: {}{}\n\
                 • strict_mode: {}
                 • dry-run: {}\n\
                 • burst: {} msgs / {}s\n\
                 • duplicates: {} / {}s\n\
                 • max_links: {} (action: {}), max_mentions: {}\n\
                 • new-user grace: {} msgs / {}s\n\
                 • thresholds warn/kick/ban: {}/{}/{}\n\
                 • escalation: kick after {}, ban after {} (window {}s)\n\
                 • action cooldown: {}s\n\
                 • banned words: {}, regex patterns: {}, allowlist domains: {}\n\
                 • tracked users: {}\n\
                 • actions taken — warns: {}, kicks: {}, bans: {}",
                if enabled { "ENABLED ✅" } else { "disabled ❌" },
                dry,
                cfg.strict_mode,
                dry_label,
                cfg.max_messages, cfg.burst_window_secs,
                cfg.max_duplicates, cfg.dedupe_window_secs,
                cfg.max_links, cfg.link_action, cfg.max_mentions,
                cfg.new_user_max_msgs, cfg.new_user_grace_secs,
                cfg.warn_threshold, cfg.kick_threshold, cfg.ban_threshold,
                cfg.escalation_kick_after, cfg.escalation_ban_after, cfg.escalation_window_secs,
                cfg.action_cooldown_secs,
                words.len(), patterns.len(), allow.len(),
                tracked, warns, kicks, bans,
            );
            super::reply(ctx, msg, &text).await?;
        }

        "words" => {
            let action = parts.get(1).copied().unwrap_or("");
            match action {
                "add" => {
                    if !is_owner {
                        super::reply(ctx, msg, "⛔ Owner only.").await?;
                        return Ok(());
                    }
                    let word = args.splitn(3, char::is_whitespace).nth(2).unwrap_or("").trim();
                    if word.is_empty() {
                        super::reply(ctx, msg, "Usage: !automod words add <word or phrase>").await?;
                        return Ok(());
                    }
                    if engine.add_word(word).await {
                        super::reply(ctx, msg, &format!("✅ Added banned word: {:?}", word.to_lowercase())).await?;
                    } else {
                        super::reply(ctx, msg, "ℹ️ That word is already banned (or empty).").await?;
                    }
                }
                "remove" | "rm" => {
                    if !is_owner {
                        super::reply(ctx, msg, "⛔ Owner only.").await?;
                        return Ok(());
                    }
                    let word = args.splitn(3, char::is_whitespace).nth(2).unwrap_or("").trim();
                    if word.is_empty() {
                        super::reply(ctx, msg, "Usage: !automod words remove <word or phrase>").await?;
                        return Ok(());
                    }
                    if engine.remove_word(word).await {
                        super::reply(ctx, msg, &format!("✅ Removed banned word: {:?}", word.to_lowercase())).await?;
                    } else {
                        super::reply(ctx, msg, "ℹ️ That word wasn't in the banned list.").await?;
                    }
                }
                "list" | "" => {
                    let words = engine.list_words().await;
                    let patterns = engine.list_patterns().await;
                    if words.is_empty() && patterns.is_empty() {
                        super::reply(ctx, msg, "📝 No banned words or patterns configured.").await?;
                    } else {
                        let mut out = String::from("📝 Banned filters:\n");
                        if !words.is_empty() {
                            out.push_str(&format!("Words ({}):\n", words.len()));
                            for w in &words {
                                out.push_str(&format!("  • {}\n", w));
                            }
                        }
                        if !patterns.is_empty() {
                            out.push_str(&format!("Regex patterns ({}):\n", patterns.len()));
                            for p in &patterns {
                                out.push_str(&format!("  • {}\n", p));
                            }
                        }
                        super::reply(ctx, msg, out.trim()).await?;
                    }
                }
                _ => {
                    super::reply(ctx, msg, "Usage: !automod words <add|remove|list> [word]").await?;
                }
            }
        }

        "allowlist" => {
            let action = parts.get(1).copied().unwrap_or("");
            match action {
                "add" => {
                    if !is_owner {
                        super::reply(ctx, msg, "⛔ Owner only.").await?;
                        return Ok(());
                    }
                    let domain = parts.get(2).copied().unwrap_or("").trim();
                    if domain.is_empty() {
                        super::reply(ctx, msg, "Usage: !automod allowlist add <domain>").await?;
                        return Ok(());
                    }
                    if engine.add_allowlist(domain).await {
                        super::reply(ctx, msg, &format!("✅ Allowlisted domain: {}", domain.to_lowercase())).await?;
                    } else {
                        super::reply(ctx, msg, "ℹ️ That domain is already allowlisted (or empty).").await?;
                    }
                }
                "remove" | "rm" => {
                    if !is_owner {
                        super::reply(ctx, msg, "⛔ Owner only.").await?;
                        return Ok(());
                    }
                    let domain = parts.get(2).copied().unwrap_or("").trim();
                    if domain.is_empty() {
                        super::reply(ctx, msg, "Usage: !automod allowlist remove <domain>").await?;
                        return Ok(());
                    }
                    if engine.remove_allowlist(domain).await {
                        super::reply(ctx, msg, &format!("✅ Removed allowlist domain: {}", domain.to_lowercase())).await?;
                    } else {
                        super::reply(ctx, msg, "ℹ️ That domain wasn't allowlisted.").await?;
                    }
                }
                "list" | "" => {
                    let allow = engine.list_allowlist().await;
                    if allow.is_empty() {
                        super::reply(ctx, msg, "🔗 Link allowlist is empty.").await?;
                    } else {
                        let mut out = format!("🔗 Allowlisted domains ({}):\n", allow.len());
                        for d in &allow {
                            out.push_str(&format!("  • {}\n", d));
                        }
                        super::reply(ctx, msg, out.trim()).await?;
                    }
                }
                _ => {
                    super::reply(ctx, msg, "Usage: !automod allowlist <add|remove|list> [domain]").await?;
                }
            }
        }

        "history" => {
            let filter = parts.get(1).map(|s| crate::handlers::normalize_npub(s));
            let filter_ref = filter.as_deref().filter(|s| !s.is_empty());
            let entries = engine.history(filter_ref, 15).await;
            if entries.is_empty() {
                super::reply(ctx, msg, "📋 No auto-mod actions on record.").await?;
            } else {
                let header = match filter_ref {
                    Some(n) => format!("📋 Recent auto-mod actions for {}:", n),
                    None => "📋 Recent auto-mod actions:".to_string(),
                };
                let body = entries
                    .iter()
                    .map(|e| format!("  • {}", e))
                    .collect::<Vec<_>>()
                    .join("\n");
                super::reply(ctx, msg, &format!("{}\n{}", header, body)).await?;
            }
        }

        "reset" => {
            if !is_owner {
                super::reply(ctx, msg, "⛔ Owner only.").await?;
                return Ok(());
            }
            let target = parts.get(1).map(|s| crate::handlers::normalize_npub(s)).unwrap_or_default();
            if target.is_empty() || !target.starts_with("npub1") {
                super::reply(ctx, msg, "Usage: !automod reset <npub>").await?;
                return Ok(());
            }
            if engine.reset_user(&target).await {
                engine.persist().await;
                super::reply(ctx, msg, &format!("✅ Reset violation history for {}.", target)).await?;
            } else {
                super::reply(ctx, msg, &format!("ℹ️ No tracked history for {}.", target)).await?;
            }
        }

        _ => {
            super::reply(
                ctx,
                msg,
                "Usage: !automod <on|off|dryrun|status|words|allowlist|history|reset>\n\
                 • !automod on/off — toggle (owner)\n\
                 • !automod dryrun <on|off|auto|status> — control dry-run mode (owner)\n\
                 • !automod status — config + stats\n\
                 • !automod words <add|remove|list> [word] (owner to modify)\n\
                 • !automod allowlist <add|remove|list> [domain] (owner to modify)\n\
                 • !automod history [npub] — recent actions\n\
                 • !automod reset <npub> — clear a user's history (owner)",
            )
            .await?;
        }
    }

    Ok(())
}

// =============================================================================
// TESTS
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::AutoModSection;

    fn cfg() -> AutoModSection {
        let mut c = AutoModSection::default();
        // Disable dry-run so tests exercise real thresholds/actions.
        c.dry_run_minutes = 0;
        c
    }

    #[test]
    fn test_normalize_text() {
        assert_eq!(normalize_text("  Hello   WORLD  "), "hello world");
        assert_eq!(normalize_text("Buy\tNow\nBuy"), "buy now buy");
    }

    #[test]
    fn test_caps_percentage() {
        assert_eq!(caps_percentage("HELLO"), 100);
        assert_eq!(caps_percentage("hello"), 0);
        assert_eq!(caps_percentage("Hello"), 20);
        assert_eq!(caps_percentage("12345"), 0); // no letters
    }

    #[test]
    fn test_url_host() {
        assert_eq!(url_host("https://github.com/foo/bar"), "github.com");
        assert_eq!(url_host("http://sub.example.com:8080/x"), "sub.example.com");
        assert_eq!(url_host("https://user@evil.com/path"), "evil.com");
        assert_eq!(url_host("https://spam.xyz"), "spam.xyz");
    }

    #[test]
    fn test_is_allowlisted() {
        let allow = vec!["github.com".to_string(), "nostr.org".to_string()];
        assert!(is_allowlisted("https://github.com/x", &allow));
        assert!(is_allowlisted("https://api.github.com/x", &allow)); // subdomain
        assert!(!is_allowlisted("https://evil.com", &allow));
        assert!(!is_allowlisted("https://notgithub.com", &allow));
    }

    #[tokio::test]
    async fn test_rate_burst_detection() {
        let mut c = cfg();
        c.max_messages = 3;
        c.burst_window_secs = 10;
        let engine = AutoModEngine::new_ephemeral(&c);

        // First 3 messages (unique text so dedupe doesn't fire) are fine.
        for i in 0..3 {
            let v = engine
                .check_message(&c, "npub1spammer", &format!("msg {}", i), "chan")
                .await;
            assert_eq!(v.score, 0, "message {} should not be flagged", i);
        }
        // 4th message within window → burst (count 4 > max 3).
        let v = engine
            .check_message(&c, "npub1spammer", "msg 4", "chan")
            .await;
        assert!(v.rules_triggered.contains(&"rate_burst".to_string()));
        assert!(v.score >= 3);
    }

    #[tokio::test]
    async fn test_duplicate_detection() {
        let mut c = cfg();
        c.max_duplicates = 3;
        c.dedupe_window_secs = 60;
        // Keep burst out of the way.
        c.max_messages = 100;
        let engine = AutoModEngine::new_ephemeral(&c);

        let v1 = engine.check_message(&c, "npub1dup", "buy now", "chan").await;
        assert!(!v1.rules_triggered.contains(&"duplicate_content".to_string()));
        let v2 = engine.check_message(&c, "npub1dup", "buy now", "chan").await;
        assert!(!v2.rules_triggered.contains(&"duplicate_content".to_string()));
        // 3rd identical → duplicate (count 3 >= max 3).
        let v3 = engine.check_message(&c, "npub1dup", "BUY NOW", "chan").await;
        assert!(v3.rules_triggered.contains(&"duplicate_content".to_string()));
        assert!(v3.score >= 4);
    }

    #[tokio::test]
    async fn test_banned_keyword() {
        let mut c = cfg();
        c.banned_words = vec!["free crypto giveaway".to_string()];
        let engine = AutoModEngine::new_ephemeral(&c);
        let v = engine
            .check_message(&c, "npub1x", "Click here for a FREE CRYPTO GIVEAWAY!!!", "chan")
            .await;
        assert!(v.rules_triggered.contains(&"banned_keyword".to_string()));
        assert!(v.score >= 5);
        // Banned keyword alone (score 5) → kick threshold.
        assert_eq!(v.action, AutoModAction::Kick);
    }

    #[tokio::test]
    async fn test_banned_regex_pattern() {
        let mut c = cfg();
        c.banned_patterns = vec![r"(?i)t\.me/\w+".to_string()];
        let engine = AutoModEngine::new_ephemeral(&c);
        let v = engine
            .check_message(&c, "npub1x", "join my telegram t.me/scamchannel", "chan")
            .await;
        assert!(v.rules_triggered.contains(&"banned_keyword".to_string()));
        assert!(v.score >= 5);
    }

    #[tokio::test]
    async fn test_link_filtering_new_vs_old() {
        let mut c = cfg();
        c.link_action = "flag".to_string();
        c.link_allowlist = vec!["github.com".to_string()];
        let engine = AutoModEngine::new_ephemeral(&c);

        // Allowlisted link → no score.
        let v = engine
            .check_message(&c, "npub1a", "check https://github.com/x", "chan")
            .await;
        assert!(!v.rules_triggered.contains(&"link_filter".to_string()));

        // Non-allowlisted link → flagged (+2 for established user).
        let v = engine
            .check_message(&c, "npub1b", "visit https://spam.xyz/promo", "chan")
            .await;
        assert!(v.rules_triggered.contains(&"link_filter".to_string()));
        assert_eq!(v.score, 2);
    }

    #[tokio::test]
    async fn test_mention_spam() {
        let mut c = cfg();
        c.max_mentions = 2;
        let engine = AutoModEngine::new_ephemeral(&c);
        let text = "hey nostr:npub1aaaaaaaaaaaaaaaaaaaaa npub1bbbbbbbbbbbbbbbbbbbbb npub1ccccccccccccccccccccc";
        let v = engine.check_message(&c, "npub1x", text, "chan").await;
        assert!(v.rules_triggered.contains(&"mention_spam".to_string()));
        assert!(v.score >= 3);
    }

    #[tokio::test]
    async fn test_new_user_flooding() {
        let mut c = cfg();
        c.new_user_grace_secs = 120;
        c.new_user_max_msgs = 2;
        c.max_messages = 100; // keep burst out of the way
        let engine = AutoModEngine::new_ephemeral(&c);

        engine.record_join("npub1new").await;
        // First 2 messages within grace are OK.
        for i in 0..2 {
            let v = engine
                .check_message(&c, "npub1new", &format!("hi {}", i), "chan")
                .await;
            assert!(!v.rules_triggered.contains(&"new_user_flooding".to_string()));
        }
        // 3rd message during grace → flooding.
        let v = engine.check_message(&c, "npub1new", "hi 3", "chan").await;
        assert!(v.rules_triggered.contains(&"new_user_flooding".to_string()));
    }

    #[tokio::test]
    async fn test_scoring_thresholds() {
        let c = cfg(); // warn=3, kick=5, ban=7
        let engine = AutoModEngine::new_ephemeral(&c);

        // Wall of text alone → +1 → below warn → None.
        let mut long = String::from("a");
        long.push_str(&"b".repeat(c.max_msg_length + 10));
        let v = engine.check_message(&c, "npub1a", &long, "chan").await;
        assert_eq!(v.action, AutoModAction::None);
        assert!(v.score >= 1);
    }

    #[tokio::test]
    async fn test_ban_threshold() {
        let mut c = cfg();
        // Two banned words → 5 + 5 = 10 ≥ ban_threshold(7).
        c.banned_words = vec!["scam".to_string(), "giveaway".to_string()];
        let engine = AutoModEngine::new_ephemeral(&c);
        let v = engine
            .check_message(&c, "npub1x", "scam giveaway now", "chan")
            .await;
        assert!(v.score >= 7);
        assert_eq!(v.action, AutoModAction::Ban);
    }

    #[tokio::test]
    async fn test_escalation_bumps_action() {
        let mut c = cfg();
        // A single non-allowlisted link = score 2 (below warn=3), so on its own
        // it would be None. Force warns by lowering the warn threshold to 2.
        c.warn_threshold = 2;
        c.kick_threshold = 50;
        c.ban_threshold = 100;
        c.escalation_kick_after = 2;
        c.escalation_ban_after = 3;
        c.link_allowlist = vec![];
        let engine = AutoModEngine::new_ephemeral(&c);

        // check_message records the violation itself (atomically with the
        // escalation count), so each call advances the ladder on its own.
        // 1st violation → warn.
        let v1 = engine
            .check_message(&c, "npub1esc", "https://spam.xyz/a", "chan")
            .await;
        assert_eq!(v1.action, AutoModAction::Warn);
        assert!(!v1.suppressed);

        // 2nd violation → escalated to kick.
        let v2 = engine
            .check_message(&c, "npub1esc", "https://spam.xyz/b", "chan")
            .await;
        assert_eq!(v2.action, AutoModAction::Kick);
        assert!(!v2.suppressed);

        // 3rd violation → escalated to ban.
        let v3 = engine
            .check_message(&c, "npub1esc", "https://spam.xyz/c", "chan")
            .await;
        assert_eq!(v3.action, AutoModAction::Ban);
    }

    #[tokio::test]
    async fn test_action_cooldown_suppresses_repeat() {
        let mut c = cfg();
        c.banned_words = vec!["scam".to_string()];
        c.action_cooldown_secs = 60;
        c.max_messages = 100;
        let engine = AutoModEngine::new_ephemeral(&c);

        // First hit: real action, recorded.
        let v1 = engine.check_message(&c, "npub1burst", "scam one", "chan").await;
        assert_eq!(v1.action, AutoModAction::Kick);
        assert!(!v1.suppressed);

        // Same burst, same second: equal-severity repeat is suppressed so we
        // don't fire N kicks and N announcements for one flood.
        let v2 = engine.check_message(&c, "npub1burst", "scam two", "chan").await;
        assert!(v2.suppressed, "repeat action inside the cooldown should be suppressed");

        // Only the first violation was recorded.
        let history = engine.history(Some("npub1burst"), 10).await;
        assert_eq!(history.len(), 1);
    }

    #[tokio::test]
    async fn test_cooldown_does_not_suppress_escalation() {
        let mut c = cfg();
        c.warn_threshold = 2;
        c.kick_threshold = 50;
        c.ban_threshold = 100;
        c.escalation_kick_after = 2;
        c.escalation_ban_after = 3;
        c.action_cooldown_secs = 3600; // very long — must not block escalation
        c.link_allowlist = vec![];
        let engine = AutoModEngine::new_ephemeral(&c);

        let v1 = engine.check_message(&c, "npub1esc2", "https://spam.xyz/a", "chan").await;
        assert_eq!(v1.action, AutoModAction::Warn);
        assert!(!v1.suppressed);

        // Stronger action than the last one → not suppressed, even inside the
        // cooldown. Otherwise a persistent spammer could never be escalated.
        let v2 = engine.check_message(&c, "npub1esc2", "https://spam.xyz/b", "chan").await;
        assert_eq!(v2.action, AutoModAction::Kick);
        assert!(!v2.suppressed);

        let v3 = engine.check_message(&c, "npub1esc2", "https://spam.xyz/c", "chan").await;
        assert_eq!(v3.action, AutoModAction::Ban);
        assert!(!v3.suppressed);
    }

    #[tokio::test]
    async fn test_member_leave_keeps_violations() {
        let mut c = cfg();
        c.banned_words = vec!["scam".to_string()];
        let engine = AutoModEngine::new_ephemeral(&c);

        let v = engine.check_message(&c, "npub1kicked", "scam", "chan").await;
        assert!(v.should_act());
        assert_eq!(engine.history(Some("npub1kicked"), 10).await.len(), 1);

        // A kick emits MemberLeave. The violation must survive it, or the
        // offender resets their escalation ladder every time they're kicked.
        engine.on_member_leave("npub1kicked").await;
        assert_eq!(
            engine.history(Some("npub1kicked"), 10).await.len(),
            1,
            "violations must survive a kick/leave"
        );

        // `!automod reset <npub>` is the explicit wipe.
        assert!(engine.reset_user("npub1kicked").await);
        assert!(engine.history(Some("npub1kicked"), 10).await.is_empty());
    }

    #[test]
    fn test_contains_banned_word_boundaries() {
        // Single words match on word boundaries only.
        assert!(contains_banned_word("this is a scam!", "scam"));
        assert!(contains_banned_word("scam", "scam"));
        assert!(contains_banned_word("a (scam) here", "scam"));
        assert!(!contains_banned_word("he scampered off", "scam"));
        assert!(!contains_banned_word("descambiar", "scam"));
        // Phrases still match as plain substrings.
        assert!(contains_banned_word("a free crypto giveaway now", "free crypto giveaway"));
        assert!(!contains_banned_word("nothing here", "free crypto giveaway"));
    }

    #[tokio::test]
    async fn test_dry_run_window_measured_from_enable() {
        let mut c = cfg();
        c.dry_run_minutes = 60;
        let engine = AutoModEngine::new_ephemeral(&c);

        // Not enabled yet → nothing to be dry about.
        assert!(!engine.in_dry_run(&c).await);

        // Switching on starts the window.
        engine.set_enabled(true).await;
        assert!(engine.in_dry_run(&c).await);

        // Forcing it off is honored regardless of the window.
        engine.set_dry_run(Some(false)).await;
        assert!(!engine.in_dry_run(&c).await);
    }

    #[tokio::test]
    async fn test_immunity_check() {
        // is_immune() needs a BotContext, which is heavy to construct in a unit
        // test. Instead we validate the pure decision table via a standalone
        // helper mirroring is_immune()'s logic.
        fn immune(
            npub: &str,
            bot: &str,
            owner: &str,
            authorized: &[&str],
            is_admin: bool,
            strict: bool,
        ) -> bool {
            if npub.is_empty() {
                return false;
            }
            if npub == bot {
                return true;
            }
            if npub == owner {
                return true;
            }
            if !strict && authorized.contains(&npub) {
                return true;
            }
            is_admin
        }

        assert!(immune("npub1bot", "npub1bot", "npub1owner", &[], false, false));
        assert!(immune("npub1owner", "npub1bot", "npub1owner", &[], false, true)); // owner immune even in strict
        assert!(immune("npub1friend", "npub1bot", "npub1owner", &["npub1friend"], false, false));
        assert!(!immune("npub1friend", "npub1bot", "npub1owner", &["npub1friend"], false, true)); // strict: friend checked
        assert!(immune("npub1mod", "npub1bot", "npub1owner", &[], true, true)); // admin immune
        assert!(!immune("npub1rando", "npub1bot", "npub1owner", &[], false, false));
    }

    #[tokio::test]
    async fn test_reset_user() {
        let mut c = cfg();
        c.banned_words = vec!["spam".to_string()];
        let engine = AutoModEngine::new_ephemeral(&c);
        let _ = engine.check_message(&c, "npub1r", "spam", "chan").await;
        assert!(engine.reset_user("npub1r").await);
        // After reset, dedupe/violation history is cleared.
        assert!(!engine.reset_user("npub1nonexistent").await);
    }

}
