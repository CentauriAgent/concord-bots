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
use std::time::Instant;

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

fn audit_log_file() -> PathBuf {
    PathBuf::from("data/automod-log.json")
}

/// How often (seconds) state is flushed to disk from the hot path.
const PERSIST_INTERVAL_SECS: u64 = 60;

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
    /// When the engine was constructed (drives the dry-run window).
    started_at: Instant,
    /// Last time state was flushed to disk (unix secs).
    last_persist: RwLock<u64>,
}

impl AutoModEngine {
    /// Build the engine from config. Compiles regex patterns (bad patterns are
    /// logged + skipped, never fatal) and loads persisted state if present.
    pub fn new(cfg: &AutoModSection) -> Self {
        // Compile patterns; skip (with a warning) any that fail.
        let mut patterns = Vec::new();
        let mut pattern_sources = Vec::new();
        for p in &cfg.banned_patterns {
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

        let banned_words: Vec<String> =
            cfg.banned_words.iter().map(|w| w.to_lowercase()).collect();

        // Load persisted state (corrupt/missing → fresh start).
        let state = load_state();

        Self {
            inner: Arc::new(Inner {
                enabled: RwLock::new(cfg.enabled),
                banned_words: RwLock::new(banned_words),
                patterns: RwLock::new(patterns),
                pattern_sources: RwLock::new(pattern_sources),
                link_allowlist: RwLock::new(cfg.link_allowlist.clone()),
                state: RwLock::new(state),
                started_at: Instant::now(),
                last_persist: RwLock::new(now_secs()),
            }),
        }
    }

    // ---- runtime toggles --------------------------------------------------

    pub async fn is_enabled(&self) -> bool {
        *self.inner.enabled.read().await
    }

    pub async fn set_enabled(&self, on: bool) {
        *self.inner.enabled.write().await = on;
    }

    /// True while still inside the dry-run (log-only) window after enabling.
    pub fn in_dry_run(&self, cfg: &AutoModSection) -> bool {
        cfg.dry_run_minutes > 0
            && self.inner.started_at.elapsed().as_secs() < cfg.dry_run_minutes * 60
    }

    // ---- banned words -----------------------------------------------------

    pub async fn add_word(&self, word: &str) -> bool {
        let w = word.trim().to_lowercase();
        if w.is_empty() {
            return false;
        }
        let mut words = self.inner.banned_words.write().await;
        if words.iter().any(|x| x == &w) {
            return false;
        }
        words.push(w);
        true
    }

    pub async fn remove_word(&self, word: &str) -> bool {
        let w = word.trim().to_lowercase();
        let mut words = self.inner.banned_words.write().await;
        let before = words.len();
        words.retain(|x| x != &w);
        words.len() != before
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
        let mut list = self.inner.link_allowlist.write().await;
        if list.iter().any(|x| x.to_lowercase() == d) {
            return false;
        }
        list.push(d);
        true
    }

    pub async fn remove_allowlist(&self, domain: &str) -> bool {
        let d = domain.trim().to_lowercase();
        let mut list = self.inner.link_allowlist.write().await;
        let before = list.len();
        list.retain(|x| x.to_lowercase() != d);
        list.len() != before
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

    /// Forget a user's tracking state (e.g. when they leave).
    pub async fn forget_user(&self, npub: &str) {
        let mut state = self.inner.state.write().await;
        state.users.remove(npub);
    }

    /// Reset a user's violation history (but keep them tracked).
    pub async fn reset_user(&self, npub: &str) -> bool {
        let mut state = self.inner.state.write().await;
        if let Some(rec) = state.users.get_mut(npub) {
            let had = !rec.violations.is_empty() || !rec.messages.is_empty();
            rec.violations.clear();
            rec.messages.clear();
            rec.msgs_since_join = 0;
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
                if !w.is_empty() && lower.contains(w.as_str()) {
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

        // --- Escalation (only when there's a real, actionable violation) ---
        if action != AutoModAction::None {
            // Count prior violations still inside the escalation window.
            let prior = {
                let state = self.inner.state.read().await;
                state
                    .users
                    .get(npub)
                    .map(|r| {
                        r.violations
                            .iter()
                            .filter(|v| now.saturating_sub(v.at) <= cfg.escalation_window_secs)
                            .count() as u32
                    })
                    .unwrap_or(0)
            };
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
        }

        AutoModVerdict {
            score,
            rules_triggered: rules,
            action,
            dry_run: self.in_dry_run(cfg),
        }
    }

    // ---- enforcement ------------------------------------------------------

    /// Enforce a verdict: record the violation, delete the message (if
    /// configured), kick/ban the user via the SDK, announce, DM the owner, and
    /// write an audit log line. Degrades gracefully on permission errors.
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

        // Record the violation + bump counters (even in dry-run, so escalation
        // tracking stays accurate for when enforcement goes live).
        {
            let mut state = self.inner.state.write().await;
            let rec = state.users.entry(npub.to_string()).or_default();
            rec.violations.push(ViolationRecord {
                at: now_secs(),
                rules: rules_joined.clone(),
                score: verdict.score,
                action: verdict.action.as_str().to_string(),
            });
            if !verdict.dry_run {
                match verdict.action {
                    AutoModAction::Warn => state.total_warns += 1,
                    AutoModAction::Kick => state.total_kicks += 1,
                    AutoModAction::Ban => state.total_bans += 1,
                    AutoModAction::None => {}
                }
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

        // 1. Delete the offending message if configured.
        if cfg.delete_spam_messages {
            let channel = msg.channel();
            if let Err(e) = channel.delete(&msg.message.id).await {
                tracing::warn!("automod: could not delete message {}: {:?}", msg.message.id, e);
            }
        }

        // 2. Take the moderation action.
        let community = msg.community();
        let mut action_ok = true;
        let mut permission_gap = false;

        match verdict.action {
            AutoModAction::Kick | AutoModAction::Ban => {
                if let Some(ref community) = community {
                    let member = community.member(npub);
                    // Never act on the owner (defensive; immunity should catch this).
                    if member.is_owner() {
                        tracing::warn!("automod: refusing to {} community owner", verdict.action.as_str());
                        action_ok = false;
                    } else {
                        let res = if verdict.action == AutoModAction::Ban {
                            member.ban().await
                        } else {
                            member.kick().await
                        };
                        if let Err(e) = res {
                            action_ok = false;
                            if is_permission_error(&e) {
                                permission_gap = true;
                                tracing::error!(
                                    "automod: PERMISSION GAP — cannot {} (need capability + higher rank): {:?}",
                                    verdict.action.as_str(),
                                    e
                                );
                            } else {
                                tracing::error!("automod: {} failed: {:?}", verdict.action.as_str(), e);
                            }
                        }
                    }
                } else {
                    action_ok = false;
                    tracing::warn!("automod: no community context — cannot kick/ban");
                }
            }
            AutoModAction::Warn | AutoModAction::None => {}
        }

        // 3. Announce in-channel.
        if cfg.announce_actions {
            let announcement = if permission_gap {
                format!(
                    "⚠️ Detected spam from {} (score {}) but I lack permission to {}. Rules: {}",
                    npub,
                    verdict.score,
                    verdict.action.as_str(),
                    rules_joined
                )
            } else {
                match verdict.action {
                    AutoModAction::Warn => format!(
                        "🚫 {} — message removed for spam ({}). Please knock it off.",
                        npub, rules_joined
                    ),
                    AutoModAction::Kick if action_ok => {
                        format!("👢 Kicked {} for spam ({}).", npub, rules_joined)
                    }
                    AutoModAction::Ban if action_ok => {
                        format!("🔨 Banned {} for spam ({}).", npub, rules_joined)
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
    let path = state_file();
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    match serde_json::to_string_pretty(state) {
        Ok(json) => {
            if let Err(e) = std::fs::write(&path, json) {
                tracing::error!("automod: failed to write state: {}", e);
            }
        }
        Err(e) => tracing::error!("automod: failed to serialize state: {}", e),
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
            let extra = if on && engine.in_dry_run(cfg) {
                format!(
                    "\n⚠️ Running in DRY-RUN (log-only) for {} min from bot start — no kicks/bans yet.",
                    cfg.dry_run_minutes
                )
            } else {
                String::new()
            };
            super::reply(ctx, msg, &format!("🛡️ Auto-mod turned {}{}", state, extra)).await?;
        }

        "status" | "" => {
            let enabled = engine.is_enabled().await;
            let (tracked, warns, kicks, bans) = engine.stats().await;
            let words = engine.list_words().await;
            let patterns = engine.list_patterns().await;
            let allow = engine.list_allowlist().await;
            let dry = if enabled && engine.in_dry_run(cfg) { " (DRY-RUN)" } else { "" };
            let text = format!(
                "🛡️ Auto-mod status: {}{}\n\
                 • strict_mode: {}\n\
                 • burst: {} msgs / {}s\n\
                 • duplicates: {} / {}s\n\
                 • max_links: {} (action: {}), max_mentions: {}\n\
                 • new-user grace: {} msgs / {}s\n\
                 • thresholds warn/kick/ban: {}/{}/{}\n\
                 • escalation: kick after {}, ban after {} (window {}s)\n\
                 • banned words: {}, regex patterns: {}, allowlist domains: {}\n\
                 • tracked users: {}\n\
                 • actions taken — warns: {}, kicks: {}, bans: {}",
                if enabled { "ENABLED ✅" } else { "disabled ❌" },
                dry,
                cfg.strict_mode,
                cfg.max_messages, cfg.burst_window_secs,
                cfg.max_duplicates, cfg.dedupe_window_secs,
                cfg.max_links, cfg.link_action, cfg.max_mentions,
                cfg.new_user_max_msgs, cfg.new_user_grace_secs,
                cfg.warn_threshold, cfg.kick_threshold, cfg.ban_threshold,
                cfg.escalation_kick_after, cfg.escalation_ban_after, cfg.escalation_window_secs,
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
                "Usage: !automod <on|off|status|words|allowlist|history|reset>\n\
                 • !automod on/off — toggle (owner)\n\
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
        let engine = AutoModEngine::new(&c);

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
        let engine = AutoModEngine::new(&c);

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
        let engine = AutoModEngine::new(&c);
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
        let engine = AutoModEngine::new(&c);
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
        let engine = AutoModEngine::new(&c);

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
        let engine = AutoModEngine::new(&c);
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
        let engine = AutoModEngine::new(&c);

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
        let engine = AutoModEngine::new(&c);

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
        let engine = AutoModEngine::new(&c);
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
        let engine = AutoModEngine::new(&c);

        // 1st violation → warn.
        let v1 = engine
            .check_message(&c, "npub1esc", "https://spam.xyz/a", "chan")
            .await;
        assert_eq!(v1.action, AutoModAction::Warn);
        engine
            .execute_dry_record(&v1, "npub1esc")
            .await;

        // 2nd violation → escalated to kick.
        let v2 = engine
            .check_message(&c, "npub1esc", "https://spam.xyz/b", "chan")
            .await;
        assert_eq!(v2.action, AutoModAction::Kick);
        engine.execute_dry_record(&v2, "npub1esc").await;

        // 3rd violation → escalated to ban.
        let v3 = engine
            .check_message(&c, "npub1esc", "https://spam.xyz/c", "chan")
            .await;
        assert_eq!(v3.action, AutoModAction::Ban);
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
        let engine = AutoModEngine::new(&c);
        let _ = engine.check_message(&c, "npub1r", "spam", "chan").await;
        assert!(engine.reset_user("npub1r").await);
        // After reset, dedupe/violation history is cleared.
        assert!(!engine.reset_user("npub1nonexistent").await);
    }

    // --- test-only helper: record a violation the way execute_action would,
    //     without needing a BotContext/SDK. Used to drive escalation tests.
    impl AutoModEngine {
        async fn execute_dry_record(&self, verdict: &AutoModVerdict, npub: &str) {
            if verdict.action == AutoModAction::None {
                return;
            }
            let mut state = self.inner.state.write().await;
            let rec = state.users.entry(npub.to_string()).or_default();
            rec.violations.push(ViolationRecord {
                at: now_secs(),
                rules: verdict.rules_triggered.join(","),
                score: verdict.score,
                action: verdict.action.as_str().to_string(),
            });
        }
    }
}
