# DESIGN.md — Feature Flags + Event Handling

**Task:** clawd-pju.3  
**Date:** 2026-06-28  
**Status:** Ready for implementation

---

## Table of Contents

1. [Problem 1: Feature Flag System](#problem-1-feature-flag-system)
2. [Problem 2: Event Handling (Welcome Messages Without the Blocking Bug)](#problem-2-event-handling)
3. [Implementation Safety Constraints](#implementation-safety-constraints)

---

## Problem 1: Feature Flag System

### 1.1 Goals

- Users toggle entire command groups via `[features]` in `bot.toml`
- `!help` only shows enabled commands
- Adding a new command is trivial (one struct entry + one match arm)
- Disabled commands silently no-op (no error spam to users)
- Core commands (`!ping`, `!help`, `!auth`, `!add`, `!remove`, `!list`, `!whoami`) are **always enabled** — not gated

### 1.2 Config Structure

Add a `FeaturesSection` to `BotConfig` in `config.rs`:

```rust
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
        }
    }
}

fn default_true() -> bool { true }
```

Add to `BotConfig`:

```rust
pub struct BotConfig {
    pub bot: BotSection,
    #[serde(default)]
    pub auth: AuthSection,
    #[serde(default)]
    pub communities: CommunitiesSection,
    #[serde(default)]
    pub scheduling: SchedulingSection,
    #[serde(default)]
    pub features: FeaturesSection,    // ← NEW
    #[serde(default)]
    pub custom: Option<toml::Value>,
}
```

**TOML usage:**

```toml
[features]
utility = true
fun = true
community = true
nostr = true
ai = false        # disabled by default
moderation = true
```

**Backward compatibility:** Existing configs without `[features]` get `Default::default()` → all `true` except `ai`. No breakage.

### 1.3 Feature Enum + Helper Methods

Add a `Feature` enum for compile-time safety:

```rust
/// Command groups that can be toggled via `[features]` in bot.toml.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Feature {
    Utility,
    Fun,
    Community,
    Nostr,
    Ai,
    Moderation,
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
        }
    }
}
```

### 1.4 Command Registry (for `!help` generation)

The current `help_text()` function returns a static string. To make it feature-aware, we need a declarative command registry. Define it as a const data structure:

```rust
/// Metadata for a single command, for help generation and feature gating.
struct CommandMeta {
    name: &'static str,
    description: &'static str,
    feature: Option<Feature>,  // None = always-on (core commands)
    auth: AuthLevel,
}

/// Single source of truth for all commands.
/// Used by: help_text(), feature gating in dispatch, and future !help <group> subcommands.
const COMMAND_REGISTRY: &[CommandMeta] = &[
    // Core (always enabled)
    CommandMeta { name: "!ping",     description: "Health check",            feature: None, auth: AuthLevel::Public },
    CommandMeta { name: "!help",     description: "Show this help",          feature: None, auth: AuthLevel::Public },
    CommandMeta { name: "!whoami",   description: "Bot identity",            feature: None, auth: AuthLevel::Public },
    CommandMeta { name: "!auth",     description: "Your auth status",        feature: None, auth: AuthLevel::Public },
    CommandMeta { name: "!stats",    description: "Bot statistics",          feature: None, auth: AuthLevel::Public },
    CommandMeta { name: "!add",      description: "Authorize a user",        feature: None, auth: AuthLevel::Owner },
    CommandMeta { name: "!remove",   description: "Deauthorize a user",      feature: None, auth: AuthLevel::Owner },
    CommandMeta { name: "!list",     description: "List authorized users",   feature: None, auth: AuthLevel::Owner },

    // Utility
    CommandMeta { name: "!price",    description: "Bitcoin price (USD)",     feature: Some(Feature::Utility), auth: AuthLevel::Public },
    CommandMeta { name: "!time",     description: "Current time [timezone]", feature: Some(Feature::Utility), auth: AuthLevel::Public },
    CommandMeta { name: "!roll",     description: "Dice roller [NdS]",       feature: Some(Feature::Utility), auth: AuthLevel::Public },
    CommandMeta { name: "!weather",  description: "Weather <zipcode>",       feature: Some(Feature::Utility), auth: AuthLevel::Public },
    CommandMeta { name: "!remind",   description: "Set a reminder",          feature: Some(Feature::Utility), auth: AuthLevel::Public },
    CommandMeta { name: "!poll",     description: "Create a poll",           feature: Some(Feature::Utility), auth: AuthLevel::Public },
    // ... future: !translate, !define, !quote, !joke, !fact, !meme, !shorten

    // Fun
    CommandMeta { name: "!8ball",    description: "Magic 8-ball",            feature: Some(Feature::Fun), auth: AuthLevel::Public },
    CommandMeta { name: "!flip",     description: "Flip a coin",             feature: Some(Feature::Fun), auth: AuthLevel::Public },
    CommandMeta { name: "!choose",   description: "Pick randomly",           feature: Some(Feature::Fun), auth: AuthLevel::Public },
    CommandMeta { name: "!rps",      description: "Rock paper scissors",     feature: Some(Feature::Fun), auth: AuthLevel::Public },

    // Community (future)
    // CommandMeta { name: "!leaderboard", ..., feature: Some(Feature::Community), ... },
    // CommandMeta { name: "!level",          ..., feature: Some(Feature::Community), ... },

    // Nostr (future)
    // CommandMeta { name: "!zap",   ..., feature: Some(Feature::Nostr), ... },

    // AI (future)
    // CommandMeta { name: "!ask",   ..., feature: Some(Feature::Ai), ... },

    // Moderation (future)
    // CommandMeta { name: "!kick",  ..., feature: Some(Feature::Moderation), ... },
];
```

### 1.5 Dynamic Help Generation

Replace the static `help_text()` with a feature-aware version:

```rust
fn help_text(features: &FeaturesSection) -> String {
    // Group commands by feature category
    let mut sections: Vec<(&str, Vec<&CommandMeta>)> = vec![
        ("📋 General", COMMAND_REGISTRY.iter()
            .filter(|c| c.feature.is_none() && c.auth != AuthLevel::Owner)
            .collect()),
        ("🛠️ Utility", COMMAND_REGISTRY.iter()
            .filter(|c| c.feature == Some(Feature::Utility) && features.is_enabled(Feature::Utility))
            .collect()),
        ("🎮 Fun", COMMAND_REGISTRY.iter()
            .filter(|c| c.feature == Some(Feature::Fun) && features.is_enabled(Feature::Fun))
            .collect()),
        ("🌟 Community", COMMAND_REGISTRY.iter()
            .filter(|c| c.feature == Some(Feature::Community) && features.is_enabled(Feature::Community))
            .collect()),
        ("⚡ Nostr", COMMAND_REGISTRY.iter()
            .filter(|c| c.feature == Some(Feature::Nostr) && features.is_enabled(Feature::Nostr))
            .collect()),
        ("🤖 AI", COMMAND_REGISTRY.iter()
            .filter(|c| c.feature == Some(Feature::Ai) && features.is_enabled(Feature::Ai))
            .collect()),
        ("🛡️ Moderation", COMMAND_REGISTRY.iter()
            .filter(|c| c.feature == Some(Feature::Moderation) && features.is_enabled(Feature::Moderation))
            .collect()),
        ("🔐 Owner", COMMAND_REGISTRY.iter()
            .filter(|c| c.feature.is_none() && c.auth == AuthLevel::Owner)
            .collect()),
    ];

    // Remove empty sections
    sections.retain(|(_, cmds)| !cmds.is_empty());

    let mut parts = Vec::new();
    for (header, cmds) in &sections {
        let lines: Vec<String> = cmds.iter()
            .map(|c| format!("  {} — {}", c.name, c.description))
            .collect();
        parts.push(format!("{}\n{}", header, lines.join("\n")));
    }

    format!("Available commands:\n\n{}", parts.join("\n\n"))
}
```

### 1.6 Dispatch Gating

In `commands.rs::on_message()`, gate each command group with a single feature check at the group level. This avoids per-command overhead and keeps the match arms readable:

```rust
pub async fn on_message(ctx: &BotContext, msg: &IncomingMessage) -> Result<()> {
    // ... existing rate limiter code ...

    let features = &ctx.config.features;

    match command {
        // =====================================================================
        // CORE (always enabled)
        // =====================================================================
        "!ping" => { msg.reply("pong 🏓").await?; }
        "!help" => { msg.reply(&help_text(features)).await?; }
        "!echo" => { /* ... */ }
        "!whoami" => { /* ... */ }
        "!auth" => { /* ... */ }

        // =====================================================================
        // UTILITY (gated by features.utility)
        // =====================================================================
        "!price" | "!time" | "!roll" | "!stats" | "!weather"
        | "!remind" | "!poll" if features.utility => {
            dispatch_utility(ctx, msg, command, args).await?;
        }

        // =====================================================================
        // FUN (gated by features.fun)
        // =====================================================================
        "!8ball" | "!flip" | "!choose" | "!rps" if features.fun => {
            dispatch_fun(ctx, msg, command, args).await?;
        }

        // =====================================================================
        // COMMUNITY (gated by features.community)
        // =====================================================================
        // "!leaderboard" | "!level" | "!giveaway" if features.community => { ... }

        // =====================================================================
        // NOSTR (gated by features.nostr)
        // =====================================================================
        // "!zap" | "!tip" | "!nostr" | "!nip05" | "!follow" if features.nostr => { ... }

        // =====================================================================
        // AI (gated by features.ai)
        // =====================================================================
        // "!ask" | "!summarize" | "!image" if features.ai => { ... }

        // =====================================================================
        // MODERATION (gated by features.moderation)
        // =====================================================================
        // "!kick" | "!warn" | "!purge" if features.moderation => { ... }

        // =====================================================================
        // AUTH MANAGEMENT (always enabled, owner-only)
        // =====================================================================
        "!add" => { /* ... existing ... */ }
        "!remove" => { /* ... existing ... */ }
        "!list" => { /* ... existing ... */ }

        // =====================================================================
        // UNKNOWN — silently ignore
        // =====================================================================
        _ => {
            tracing::debug!("Unknown or disabled command: {}", command);
        }
    }

    Ok(())
}
```

**Key design decision:** Disabled commands fall through to `_ =>` (silently ignored). We do NOT send "this command is disabled" to the user — that leaks the command's existence and adds noise. The command simply doesn't exist from the user's perspective.

### 1.7 Developer Experience: Adding a New Command

To add a new command (e.g., `!joke` in the Utility group):

1. **Add to COMMAND_REGISTRY** (one line):
   ```rust
   CommandMeta { name: "!joke", description: "Random dad joke", feature: Some(Feature::Utility), auth: AuthLevel::Public },
   ```

2. **Add to the match arm** for that group:
   ```rust
   "!price" | "!time" | "!roll" | "!stats" | "!weather" | "!remind" | "!poll" | "!joke"
       if features.utility => { dispatch_utility(ctx, msg, command, args).await?; }
   ```

3. **Implement** the handler in `utility.rs`:
   ```rust
   pub async fn joke_command(_ctx: &BotContext, msg: &IncomingMessage) -> Result<()> { ... }
   ```

4. **Add dispatch** in the utility dispatch function (or inline in the match).

**Total:** ~3 lines of plumbing + the handler implementation. Feature gating is automatic.

### 1.8 Config Logging

Add to `BotConfig::log_summary()`:

```rust
tracing::info!("  features:");
tracing::info!("    utility: {}, fun: {}, community: {}, nostr: {}, ai: {}, moderation: {}",
    self.features.utility, self.features.fun, self.features.community,
    self.features.nostr, self.features.ai, self.features.moderation);
```

### 1.9 Config Validation (optional, recommended)

Add a sanity check in `BotConfig::load()` after parsing:

```rust
if config.features.ai {
    // Check for API key
    if config.custom_string("ai.api_key").is_none() {
        tracing::warn!("AI feature enabled but no ai.api_key found in custom config — AI commands will fail");
    }
}
```

---

## Problem 2: Event Handling

### 2.1 The Bug

**Root cause:** Both `bot.on_message()` and `bot.on_event()` internally call `self.core.listen(handler)` — a method that **blocks forever** (it's the main event loop). The first call wins; the second never executes.

```rust
// SDK source (simplified):
pub async fn on_message<F>(&self, handler: F) -> Result<()> {
    self.prepare_listen().await;
    self.core.listen(Arc::new(ClosureHandler { .. })).await  // ← BLOCKS FOREVER
}

pub async fn on_event<F>(&self, handler: F) -> Result<()> {
    self.prepare_listen().await;
    self.core.listen(Arc::new(EventClosureHandler { .. })).await  // ← BLOCKS FOREVER
}
```

**v0.2.0 registered `on_event` first** → `on_message` never registered → bot received events but not messages → commands didn't work.

**v0.2.1 fix:** Removed `on_event`, kept `on_message` only. Commands work. But we lost access to `BotEvent::MemberJoin` (needed for welcome messages).

### 2.2 SDK Deep Dive: `EventClosureHandler` Handles Messages!

Reading the SDK source confirms that `EventClosureHandler` (used by `on_event`) handles **ALL** event types, including messages:

```rust
// In EventClosureHandler impl InboundEventHandler:
fn on_dm_received(&self, chat_id: &str, msg: &Message, _: bool) {
    self.message(chat_id, msg, false, false);  // → BotEvent::Message(IncomingMessage)
}
fn on_file_received(&self, chat_id: &str, msg: &Message, _: bool) {
    self.message(chat_id, msg, false, true);   // → BotEvent::Message(IncomingMessage)
}
fn on_community_message(&self, chat_id: &str, msg: &Message, _: bool) {
    self.message(chat_id, msg, true, !msg.attachments.is_empty());  // → BotEvent::Message
}
fn on_community_presence(&self, chat_id: &str, npub: &str, joined: bool, ...) {
    self.emit(if joined { BotEvent::MemberJoin { .. } } else { BotEvent::MemberLeave { .. } });
}
```

**Key finding:** `on_event` is a **strict superset** of `on_message`. Every message that `on_message` would deliver is also delivered as `BotEvent::Message(IncomingMessage)` through `on_event`, via `tokio::spawn` (non-blocking per-event).

### 2.3 Why Did the Previous Attempt Fail?

The v0.2.0 code registered **both** `on_event` and `on_message`. This is the bug — `core.listen()` is called twice, but it's a blocking call. Only the first registration runs.

The previous attempt to use `on_event` was NOT inherently broken. The problem was purely the double-registration. **Using `on_event` alone, with proper message dispatch, is completely sound.**

### 2.4 Solution: Use `on_event` Only

**Approach A — Recommended**

Replace `bot.on_message(...)` with `bot.on_event(...)`, and dispatch `BotEvent::Message` to the existing message handler. Everything else goes to the event handler.

#### 2.4.1 Changes to `bot.rs`

Replace the current message loop (Step 5) with:

```rust
// -------------------------------------------------------------------------
// Step 5: Event loop (handles BOTH messages AND member joins)
// -------------------------------------------------------------------------
// Use on_event — it's a superset of on_message. Messages arrive as
// BotEvent::Message(IncomingMessage), joins as BotEvent::MemberJoin, etc.
// We must NOT also register on_message — both call core.listen() and only
// the first registration runs (see commit c51e4b3).

bot.on_event({
    let ctx = ctx.clone();
    move |_bot, event| {
        let ctx = ctx.clone();
        async move {
            match event {
                BotEvent::Message(msg) => {
                    // Don't process our own messages.
                    if msg.is_mine() {
                        return;
                    }

                    tracing::info!(
                        "Incoming message from {}: {}",
                        msg.chat_id,
                        msg.text()
                    );

                    if let Err(e) = handlers::on_message(&ctx, &msg).await {
                        tracing::error!("Handler error: {:?}", e);
                    }
                }

                // All non-message events
                _ => {
                    if let Err(e) = handlers::on_event(&ctx, event).await {
                        tracing::error!("Event handler error: {:?}", e);
                    }
                }
            }
        }
    }
})
.await
.context("Failed to register on_event handler")?;
```

#### 2.4.2 Changes to `handlers/mod.rs`

The existing `on_event()` function already has the right shape — it matches on `BotEvent` variants. Just uncomment/implement the `MemberJoin` arm:

```rust
pub async fn on_event(ctx: &BotContext, event: BotEvent) -> Result<()> {
    match &event {
        BotEvent::MemberJoin { channel_id, npub } => {
            tracing::info!("Member {} joined channel {}", npub, channel_id);

            // Feature gate: only send welcome if community features are enabled
            if ctx.config.features.is_enabled(Feature::Community) {
                let welcome = format!(
                    "Welcome! 🎉 Type !help to see what I can do."
                );
                let _ = ctx.bot.channel(channel_id.clone()).send(&welcome).await;
            }
        }

        BotEvent::MemberLeave { channel_id, npub } => {
            tracing::info!("Member {} left channel {}", npub, channel_id);
        }

        BotEvent::Message(_) => {
            // Already handled by on_message above — this arm is unreachable
            // when using on_event for dispatch.
        }

        // ... other event variants (unchanged) ...
        _ => {}
    }

    commands::on_event(ctx, &event).await?;
    Ok(())
}
```

#### 2.4.3 What NOT to Change

- **Do NOT** register `on_message` anywhere. The `on_event` handler replaces it entirely.
- **Do NOT** spawn a separate tokio task for a second `listen()` call — `core.listen()` internally manages subscriptions and spawning another listener would cause subscription conflicts.
- The `handlers::on_message()` function signature stays the same — it's just called from a different place.

### 2.5 Why Not the Other Options?

**Option B — `listen_with()` with custom `InboundEventHandler`:**  
This works but requires implementing the full `InboundEventHandler` trait (8+ methods). It's more code for zero benefit over `on_event`, which already does this via `EventClosureHandler`. Use this only if we need fine-grained control over which events trigger what (we don't).

**Option C — Spawn a separate tokio task for `on_event`:**  
`core.listen()` internally subscribes to relays and manages global state. Calling it twice (once via `on_message`, once via `on_event` in a spawned task) would create **competing subscription handlers** on the same Nostr client — events would be processed twice, state would be mutated concurrently, and dedup caches would race. This is unsafe.

**Option D — `EventEmitter` trait:**  
`EventEmitter` is a lower-level trait for platform bridging (Tauri events, logging). It doesn't provide the `IncomingMessage` / `BotEvent` abstraction. Not suitable.

### 2.6 Conceptual Safety Check

Let's trace the exact flow to verify no blocking bug:

1. `bot.on_event(closure)` is called **once**
2. Inside `on_event()`:
   - `self.prepare_listen().await` — runs once (sync DMs, process invites)
   - `self.core.listen(Arc::new(EventClosureHandler { ... })).await` — blocks forever
3. `core.listen()` subscribes to Nostr relays and enters its event loop
4. For each inbound event, `core.listen()` calls the appropriate method on `EventClosureHandler`:
   - Incoming DM → `on_dm_received()` → `tokio::spawn(handler(BotEvent::Message(msg)))` (non-blocking)
   - Member join → `on_community_presence()` → `tokio::spawn(handler(BotEvent::MemberJoin { .. }))` (non-blocking)
5. Each spawned task independently calls our closure, which matches on the event type and dispatches

**✅ Single `listen()` call. No double-registration. Messages AND events both flow through. No blocking bug.**

### 2.7 Migration Path

The change is surgical:

| File | Change |
|------|--------|
| `src/bot.rs` | Replace `bot.on_message(closure)` with `bot.on_event(closure)` (Step 5 only) |
| `src/handlers/mod.rs` | Implement `MemberJoin` arm in existing `on_event()` |
| `src/config.rs` | Add `FeaturesSection` + `Feature` enum |
| `src/handlers/commands.rs` | Add feature guards to match arms + use `COMMAND_REGISTRY` for help |
| `config/bot.toml` | Add `[features]` section (optional — defaults work) |

**No changes to:** `utility.rs`, `fun.rs`, `scheduled.rs`, `ai_bridge.rs`, `auth.rs`, `rate_limiter.rs`.

### 2.8 Testing Welcome Messages

Since `MemberJoin` events only fire for community channels (not DMs), testing requires:
1. Bot must be in a community
2. A second user joins that community's channel
3. Bot receives the presence event and fires `BotEvent::MemberJoin`

For local testing, use two Vector accounts (the bot + a test user) in the same community.

---

## Implementation Safety Constraints

### MUST NOT break

1. **The working `on_message` handler** — The v0.2.1 fix must be preserved. The new `on_event` approach dispatches to the SAME `handlers::on_message()` function.
2. **Command dispatch** — All existing `!` commands must work identically.
3. **Rate limiting** — Runs before any feature gate check.
4. **Auth system** — Owner-only commands (`!add`, `!remove`, `!list`) are always available regardless of feature flags.

### MUST preserve

1. **Single `core.listen()` call** — Never call `on_message` and `on_event` together. Never spawn a second `listen()`.
2. **Backward compatibility** — Configs without `[features]` work identically to today (all groups enabled except AI).
3. **Core commands ungated** — `!ping`, `!help`, `!echo`, `!whoami`, `!auth`, `!add`, `!remove`, `!list` are always available.

### Order of implementation

1. **Event handling first** (bot.rs change) — Highest risk, must validate immediately
2. **Feature flag config** (config.rs) — Additive, no risk
3. **Dispatch gating** (commands.rs) — Once config is in place
4. **Help generation** (commands.rs) — Polish, once gating works
5. **Welcome message** (handlers/mod.rs) — Last, once events flow correctly

---

## Appendix: Complete File Diff Summary

### `src/config.rs`
- Add `FeaturesSection` struct with `Default` impl
- Add `Feature` enum
- Add `features: FeaturesSection` to `BotConfig`
- Add `is_enabled()` method to `FeaturesSection`
- Add `default_true()` helper function
- Update `log_summary()` to show feature status

### `src/bot.rs`
- Replace `bot.on_message(closure)` with `bot.on_event(closure)` in Step 5
- Add `use vector_sdk::BotEvent;` import
- Match on `BotEvent::Message(msg)` → dispatch to `handlers::on_message()`
- Match on `_` → dispatch to `handlers::on_event()`

### `src/handlers/mod.rs`
- Implement `MemberJoin` arm (welcome message, feature-gated)
- Add `use crate::config::Feature;` import

### `src/handlers/commands.rs`
- Add `COMMAND_REGISTRY` const array
- Add `CommandMeta` struct
- Replace `help_text()` with `help_text(features: &FeaturesSection)`
- Add `if features.utility =>` guards on match arms
- Add `if features.fun =>` guards on match arms
- Update `!help` arm to call `help_text(&ctx.config.features)`

### `config/bot.toml`
- Add `[features]` section (all true except `ai`)
