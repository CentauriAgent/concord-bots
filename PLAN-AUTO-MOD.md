# PLAN-AUTO-MOD — Auto-Moderation (Spam Detection + Auto-Kick/Ban)

**Created:** 2026-08-01  
**Status:** Planning  
**Requested by:** Derek

---

## Overview

Add Discord-style auto-moderation to the Flagship bot. The bot monitors all community messages, detects spam patterns, and automatically kicks or bans spammers — no human intervention required.

This builds on the existing manual moderation system (`!kick`, `!ban`, `!warn`) already in `moderation_cmds.rs`.

---

## How Discord Does It (Reference)

Discord's AutoMod has:
1. **Keyword filters** — block messages matching word/regex lists
2. **Spam protection** — message frequency (X messages in Y seconds)
3. **Mention spam** — too many @mentions in one message
4. **Link filtering** — block suspicious/external links
5. **Escalation** — warn → timeout → kick → ban (progressive)
6. **Per-channel overrides** — some channels are stricter
7. **Audit log** — every action logged with reason
8. **Immune roles** — admins/mods bypass all checks

We'll model our system on this, adapted for Concord/Nostr.

---

## Architecture

### New Module: `src/handlers/automod.rs`

A standalone auto-moderation engine that:
- Hooks into `on_message()` BEFORE command dispatch (for content scanning)
- Hooks into `on_event()` for `MemberJoin` (new user flooding protection)
- Runs independently of the `!` command system
- Has its own config section: `[automod]`
- Maintains persistent state in `data/automod-state.json`

### Detection Pipeline

```
Incoming Message
    │
    ▼
┌─────────────────────┐
│  AutoMod Pre-Check   │  ← runs in on_message() BEFORE command dispatch
│  (all community msgs)│
└──────────┬──────────┘
           │
           ▼
┌─────────────────────────────────────┐
│  1. Immunity Check                   │  ← owner/admins bypass everything
│  2. Rate Burst Detection             │  ← X msgs in Y seconds → flag
│  3. Duplicate/Identical Content       │  ← same message N times → flag
│  4. Keyword/Pattern Match             │  ← banned words/regex → flag
│  5. Link Detection                    │  ← external/suspicious links → flag
│  6. Mention Spam                      │  ← too many npub mentions → flag
│  7. New Account Flooding              │  ← joined <60s ago, high msg rate
└──────────┬──────────────────────────┘
           │
           ▼
┌─────────────────────┐
│  Violation Scorer    │  ← each flag adds points
│  (weighted scoring)  │
└──────────┬──────────┘
           │
           ▼
┌──────────────────────────────────┐
│  Action Resolver (threshold-based)│
│                                   │
│  Score < kick_threshold  → DELETE + WARN
│  Score >= kick_threshold → KICK
│  Score >= ban_threshold  → BAN
│  Repeated violations     → ESCALATE
└──────────┬──────────────────────┘
           │
           ▼
┌─────────────────────┐
│  Action Executor     │  ← calls SDK kick()/ban()
│  + Audit Log         │  ← writes to data/automod-log.json
│  + Community Notice  │  ← posts reason in channel
└─────────────────────┘
```

---

## Detection Rules

### Rule 1: Message Rate Burst (Spam)
- **What:** User sends N messages within a sliding time window
- **Config:** `max_messages` (default: 8) in `burst_window_secs` (default: 10)
- **Score:** +3 per violation (escalates with each additional burst)
- **Example:** 8 messages in 10 seconds = flag

### Rule 2: Duplicate Content
- **What:** User sends identical (or near-identical) text multiple times
- **Config:** `max_duplicates` (default: 3) within `dedupe_window_secs` (default: 60)
- **Score:** +4 per violation
- **Implementation:** Normalize text (lowercase, strip whitespace), compare last N messages per user
- **Example:** Same "buy now" message 3 times in 60s = flag

### Rule 3: Banned Keywords / Patterns
- **What:** Message contains configured banned words or regex patterns
- **Config:** `banned_words` (list), `banned_patterns` (list of regex strings)
- **Score:** +5 per match (high — this is explicit spam)
- **Example:** Configured "free crypto giveaway" → any message containing it = flag

### Rule 4: Link Filtering
- **What:** Message contains URLs. Optionally restrict to allowlisted domains.
- **Config:** 
  - `link_action`: `"off"` | `"flag"` | `"block"` (default: `"flag"`)
  - `link_allowlist`: domains that are always OK (e.g., `["github.com", "gitlab.com", "nostr.org"]`)
  - `max_links`: max links per message (default: 3)
- **Score:** +2 per non-allowlisted link
- **New users (<5 min):** +4 per link (stricter for new accounts)

### Rule 5: Mention Spam
- **What:** Message mentions too many npubs (NIP-27 `nostr:npub1...` tags or text mentions)
- **Config:** `max_mentions` (default: 5)
- **Score:** +3 if exceeded
- **Example:** Pinging 8 people at once = flag

### Rule 6: New Account Flooding
- **What:** User who joined the community very recently sends messages rapidly
- **Config:** `new_user_grace_secs` (default: 120), `new_user_max_msgs` (default: 3)
- **Score:** +4 per message over the limit during grace period
- **Tracked via:** `MemberJoin` event + per-user message counter

### Rule 7: Caps / Wall of Text
- **What:** Excessive caps or very long messages
- **Config:** `caps_threshold_pct` (default: 70), `caps_min_length` (default: 20), `max_msg_length` (default: 1000)
- **Score:** +1 (minor nuisance)
- **Skipped for:** Messages under `caps_min_length`

---

## Scoring & Escalation

### Violation Score Per Message
Each rule that triggers adds its score to the message's total. The total determines the action:

| Score Range | Action | Log Level |
|-------------|--------|-----------|
| 1–2 | Silent flag (log only, no user-facing action) | DEBUG |
| 3–4 | DELETE message + WARN user | INFO |
| 5–6 | KICK user | WARN |
| 7+ | BAN user | ERROR |

### Progressive Escalation (per-user, rolling 24h window)
If a user keeps getting flagged even after kicks:
- **1st violation cycle:** Action based on score (above)
- **2nd within 24h:** Minimum action bumped to KICK (even if score says warn)
- **3rd within 24h:** Minimum action bumped to BAN (zero tolerance for repeat offenders)
- **Counter resets** after 24h with no violations

### Per-User Violation History
Stored in memory (and persisted to `data/automod-state.json`):
```json
{
  "npub1abc...": {
    "violations": [
      { "timestamp": "2026-08-01T22:00:00Z", "rule": "duplicate_content", "score": 4, "action": "warn" },
      { "timestamp": "2026-08-01T22:05:00Z", "rule": "rate_burst", "score": 5, "action": "kick" }
    ],
    "total_violations_24h": 2,
    "last_action": "kick",
    "message_history": [
      { "timestamp": "...", "normalized_text": "buy now", "channel_id": "..." }
    ]
  }
}
```

---

## Immunity System

The following users bypass ALL auto-mod checks:
- **Owner** (from `auth.owner`)
- **Authorized users** (from `auth.authorized` or `!add`-ed) — unless `strict_mode` is on
- **Admins/mods** (granted via `!grantmod`)
- **Bot itself** (never mod yourself)

### Configurable strict mode
```toml
[automod]
strict_mode = false  # if true, even authorized users get checked (but never owner)
```

---

## Configuration

New `[automod]` section in `config/bot.toml`:

```toml
[automod]
# Master switch
enabled = true

# Escalation
strict_mode = false          # if true, authorized users also get checked

# Rule: Rate burst
max_messages = 8             # messages allowed in burst window
burst_window_secs = 10

# Rule: Duplicate content
max_duplicates = 3           # same text this many times
dedupe_window_secs = 60

# Rule: Banned keywords/patterns
banned_words = []            # exact match (case-insensitive)
banned_patterns = []         # regex patterns

# Rule: Link filtering
link_action = "flag"         # "off" | "flag" | "block"
link_allowlist = [
    "github.com",
    "gitlab.com",
    "nostr.org",
    "npmjs.com",
    "crates.io",
    "docs.rs",
]
max_links = 3

# Rule: Mention spam
max_mentions = 5

# Rule: New account flooding
new_user_grace_secs = 120
new_user_max_msgs = 3

# Rule: Caps / wall of text
caps_threshold_pct = 70
caps_min_length = 20
max_msg_length = 1000

# Scoring thresholds
warn_threshold = 3
kick_threshold = 5
ban_threshold = 7

# Escalation
escalation_window_secs = 86400   # 24h rolling window
escalation_kick_after = 2        # 2nd violation → force kick minimum
escalation_ban_after = 3         # 3rd violation → force ban

# Actions
announce_actions = true          # post "user X was kicked for spam" in channel
announce_dm_owner = true         # DM the owner on every ban
delete_spam_messages = true      # delete the flagged message(s)
```

### Config struct in Rust

```rust
#[derive(Debug, Clone, Deserialize)]
pub struct AutoModSection {
    pub enabled: bool,
    pub strict_mode: bool,
    
    // Rate burst
    #[serde(default = "default_max_messages")]
    pub max_messages: u32,
    #[serde(default = "default_burst_window")]
    pub burst_window_secs: u64,
    
    // Duplicate content
    #[serde(default = "default_max_duplicates")]
    pub max_duplicates: u32,
    #[serde(default = "default_dedupe_window")]
    pub dedupe_window_secs: u64,
    
    // Banned keywords/patterns
    #[serde(default)]
    pub banned_words: Vec<String>,
    #[serde(default)]
    pub banned_patterns: Vec<String>,
    
    // Link filtering
    #[serde(default = "default_link_action")]
    pub link_action: String,
    #[serde(default = "default_link_allowlist")]
    pub link_allowlist: Vec<String>,
    #[serde(default = "default_max_links")]
    pub max_links: u32,
    
    // Mention spam
    #[serde(default = "default_max_mentions")]
    pub max_mentions: u32,
    
    // New account flooding
    #[serde(default = "default_new_user_grace")]
    pub new_user_grace_secs: u64,
    #[serde(default = "default_new_user_max_msgs")]
    pub new_user_max_msgs: u32,
    
    // Caps / wall of text
    #[serde(default = "default_caps_pct")]
    pub caps_threshold_pct: u32,
    #[serde(default = "default_caps_min_len")]
    pub caps_min_length: usize,
    #[serde(default = "default_max_msg_len")]
    pub max_msg_length: usize,
    
    // Scoring thresholds
    #[serde(default = "default_warn_threshold")]
    pub warn_threshold: u32,
    #[serde(default = "default_kick_threshold")]
    pub kick_threshold: u32,
    #[serde(default = "default_ban_threshold")]
    pub ban_threshold: u32,
    
    // Escalation
    #[serde(default = "default_escalation_window")]
    pub escalation_window_secs: u64,
    #[serde(default = "default_escalation_kick")]
    pub escalation_kick_after: u32,
    #[serde(default = "default_escalation_ban")]
    pub escalation_ban_after: u32,
    
    // Actions
    #[serde(default = "default_true")]
    pub announce_actions: bool,
    #[serde(default = "default_true")]
    pub announce_dm_owner: bool,
    #[serde(default = "default_true")]
    pub delete_spam_messages: bool,
}
```

---

## Implementation Plan

### Phase 1: Core Engine — `automod.rs`

**Files:** `src/handlers/automod.rs` (new)

1. Define `AutoModEngine` struct holding:
   - Reference to config (`AutoModSection`)
   - Per-user message tracking (HashMap<npub, Vec<(Instant, String)>>)
   - Per-user violation history (HashMap<npub, ViolationRecord>)
   - Join tracking (HashMap<(channel_id, npub), Instant>)
   - Compiled regex patterns from `banned_patterns`

2. Implement `check_message()` method:
   - Takes: npub, text, channel_id, community, is_new_user
   - Runs all 7 rules
   - Returns `AutoModVerdict { score, rules_triggered, recommended_action }`

3. Implement `execute_action()` method:
   - Takes: verdict, message context
   - Executes: warn (in-channel), kick, or ban via SDK
   - Optionally deletes the message
   - Posts announcement to channel
   - DMs owner on bans
   - Logs to `data/automod-log.json`

4. Implement `is_immune()` check:
   - Owner, authorized users, admins bypass
   - Respects `strict_mode` config

### Phase 2: Integration — Hook Into Existing Flow

**Files:** `src/handlers/mod.rs`, `src/config.rs`, `src/bot.rs`

1. **`config.rs`:**
   - Add `AutoModSection` to `BotConfig`
   - Add defaults
   - Add to `log_summary()`

2. **`mod.rs` — `on_message()`:**
   - After XP tracking, BEFORE command dispatch:
   ```rust
   // Auto-mod pre-check (community messages only)
   if ctx.config.automod.enabled {
       if let Some(ref npub) = msg.message.npub {
           if msg.is_group && npub != &ctx.bot.npub() {
               let verdict = ctx.automod.check_message(npub, text, &msg.chat_id, msg.community()).await;
               if verdict.should_act() {
                   ctx.automod.execute_action(&verdict, ctx, msg).await;
                   return Ok(());  // message handled, don't dispatch command
               }
           }
       }
   }
   ```

3. **`mod.rs` — `on_event()`:**
   - In `MemberJoin` arm: record join timestamp for new-user grace period tracking
   - In `MemberLeave` arm: optionally clean up tracking state

4. **`bot.rs`:**
   - Initialize `AutoModEngine` and add to `BotContext`
   - Load compiled regex patterns at startup
   - Log automod config in startup summary

### Phase 3: New Commands

**Files:** `src/handlers/commands.rs`, `src/handlers/automod.rs`

Add commands for managing auto-mod at runtime:

| Command | Auth | Description |
|---------|------|-------------|
| `!automod on/off` | Owner | Toggle auto-mod globally |
| `!automod status` | Authorized | Show current config + stats |
| `!automod words add <word>` | Owner | Add a banned word at runtime |
| `!automod words remove <word>` | Owner | Remove a banned word |
| `!automod words list` | Authorized | List current banned words |
| `!automod allowlist add <domain>` | Owner | Add link allowlist domain |
| `!automod allowlist remove <domain>` | Owner | Remove link allowlist domain |
| `!automod history [npub]` | Authorized | Show recent automod actions |
| `!automod reset <npub>` | Owner | Reset a user's violation history |

Runtime changes persist to `data/automod-config.json` (overrides TOML defaults).

### Phase 4: Persistence & Audit

**Files:** `src/handlers/automod.rs`, `data/` dir

1. **State persistence:** `data/automod-state.json`
   - Per-user violation history (for escalation)
   - Per-user message dedupe buffers
   - Join timestamps
   - Save every 60s and on graceful shutdown

2. **Audit log:** `data/automod-log.json` (append-only JSONL)
   ```json
   {"timestamp":"...","npub":"...","action":"kick","score":5,"rules":["rate_burst","duplicate_content"],"channel":"...","message_snippet":"buy now..."}
   ```

3. **Stats integration:** Add to `!stats` output:
   - Total automod actions (warns/kicks/bans)
   - Active monitored users
   - Most common triggered rule

### Phase 5: Testing & Polish

1. **Unit tests** (`automod.rs`):
   - Each rule tested independently with mock messages
   - Score accumulation + threshold crossing
   - Escalation logic (2nd/3rd violation bumps)
   - Immunity bypass
   - Regex pattern compilation + matching

2. **Integration test:**
   - Simulate spam burst → verify kick fires
   - Simulate banned keyword → verify ban fires
   - Simulate authorized user → verify immunity

3. **Error handling:**
   - SDK kick/ban failures (permission errors → log + notify owner)
   - Corrupted state file → start fresh with warning
   - Regex compilation failure → log error, disable that pattern

---

## Files Changed

| File | Change | Risk |
|------|--------|------|
| `src/handlers/automod.rs` | **NEW** — entire auto-mod engine | Core |
| `src/handlers/mod.rs` | Add automod hook in `on_message()` + `on_event()` | Medium |
| `src/handlers/commands.rs` | Add `!automod` command group + registry entries | Low |
| `src/config.rs` | Add `AutoModSection` to `BotConfig` | Low |
| `src/bot.rs` | Initialize `AutoModEngine` in `BotContext` | Medium |
| `config/bot.toml` | Add `[automod]` section | Low |

**Not touched:** `auth.rs`, `rate_limiter.rs`, `utility.rs`, `fun.rs`, `scheduled.rs`, `ai_bridge.rs`, `main.rs`, `lib/`

---

## Safety Constraints

### MUST NOT break
1. **Command dispatch flow** — automod runs before commands but returns early; commands never see flagged messages
2. **Rate limiter** — automod is separate from the `!` command rate limiter; both run independently
3. **Existing manual moderation** — `!kick`, `!ban`, `!warn` still work alongside automod
4. **Bot permissions** — if bot lacks KICK/BAN permissions in a community, automod degrades gracefully (warns + logs, skips the action, DMs owner about the permission gap)

### Safe defaults
- **`enabled = false`** by default (opt-in, never surprises)
- All thresholds pre-tuned for reasonable spam detection without false positives
- First-time activation starts in "log only" mode for the first hour (dry-run to see what would be flagged)

---

## Discord Parity Checklist

| Discord AutoMod Feature | Our Implementation | Status |
|------------------------|-------------------|--------|
| Keyword filter | Banned words + regex patterns | ✅ |
| Spam frequency | Rate burst rule | ✅ |
| Mention spam | Mention count rule | ✅ |
| Link filtering | Domain allowlist + link count | ✅ |
| Block/quarantine | Delete message option | ✅ |
| Alert/notify | Channel announce + owner DM | ✅ |
| Timeout | Kick (Concord has no timeout) | ✅ (closest equivalent) |
| Escalation | Progressive scoring → kick → ban | ✅ |
| Audit log | JSONL log file | ✅ |
| Immune roles | Owner/admin/auth bypass | ✅ |
| Per-channel overrides | Phase 2 (future) | 🔜 |
| ML-based detection | Phase 3 (future, AI bridge) | 🔜 |

---

## Build & Deploy

```bash
cd ~/projects/concord-bots
cargo build --release
systemctl --user restart concord-bots
journalctl --user -fu concord-bots
```

---

## Estimated Effort

| Phase | Effort | Description |
|-------|--------|-------------|
| 1 | Large | Core engine (rules, scoring, actions) |
| 2 | Medium | Integration into on_message/on_event + config |
| 3 | Medium | Runtime commands |
| 4 | Small | Persistence + audit |
| 5 | Medium | Tests + polish |

**Total:** Substantial feature. Recommend spawning a sub-agent for implementation.
