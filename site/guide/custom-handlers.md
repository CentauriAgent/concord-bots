# Custom Handler Tutorial

This tutorial walks through adding a brand-new `!block` command that fetches
and posts the current Bitcoin block height — from empty file to registered,
auth-checked, and tested.

## Where code goes

The template has one rule:

| Path | Edit? |
|------|-------|
| `src/handlers/**` | ✅ **Yours** — commands, scheduled tasks, AI bridge, dispatch |
| `config/bot.toml` | ✅ Yours |
| `src/main.rs`, `src/bot.rs`, `src/config.rs`, `src/auth.rs`, `src/lib/**` | ❌ Stable — the framework |

Handlers live in `src/handlers/`:

```
src/handlers/
├── mod.rs           # dispatch wiring (on_message / on_event)
├── commands.rs      # !command dispatch + COMMAND_REGISTRY + built-in auth cmds
├── utility.rs       # utility-group command implementations
├── fun.rs           # fun-group implementations
├── moderation_cmds.rs
├── community_cmds.rs
├── nostr_cmds.rs
├── wallet_cmds.rs
├── git_cmds.rs
├── ai_bridge.rs     # optional AI integration
└── scheduled.rs     # interval/cron tasks
```

Commands are messages starting with `!` (e.g. `!block`). The framework parses
the command name, routes it to your handler, and logs errors without crashing —
you only write the interesting part.

## Step 1 — Write the handler function

Open the file for the feature group your command belongs to (we'll use
`utility.rs`, gated by `features.utility`). Add:

```rust
/// !block — current Bitcoin block height (mempool.space)
pub async fn block_command(_ctx: &BotContext, msg: &IncomingMessage, _args: &str) -> Result<()> {
    let data = crate::lib::http::fetch_json("https://mempool.space/api/blocks/tip/height")
        .await
        .map_err(|e| {
            // surface a friendly chat error instead of crashing the handler
            tracing::warn!("block height fetch failed: {}", e);
            e
        })?;

    let height = data.as_u64().unwrap_or(0);
    msg.reply(&format!("⛓️ Bitcoin block height: {}", height)).await?;
    Ok(())
}
```

The pieces:

- `ctx: &BotContext` — config, bot handle, auth manager (needed for scheduled
  tasks and auth checks; ignore it with `_ctx` when unused)
- `msg: &IncomingMessage` — the inbound message. `msg.reply(...)` answers in
  the same channel and works for both DMs and communities
- Return `Result<()>` — errors are logged by the framework, never crash the bot

Useful `IncomingMessage` members: `msg.text()`, `msg.reply("...")`,
`msg.is_mine()`, `msg.channel()`, `msg.member()`, `msg.chat_id`,
`msg.message.id`, `msg.message.attachments`.

## Step 2 — Register it in the dispatch

In `src/handlers/commands.rs`, find the dispatch function for the group —
`dispatch_utility` for utility commands — and add a match arm:

```rust
async fn dispatch_utility(
    ctx: &BotContext,
    msg: &IncomingMessage,
    command: &str,
    args: &str,
) -> Result<()> {
    match command {
        "!price" => utility::price_command(ctx, msg).await?,
        "!block" => utility::block_command(ctx, msg, args).await?,   // ← new
        // ...
    }
    Ok(())
}
```

Then add the command to the group's match pattern in `on_message()` so it
reaches the dispatcher (the big `match command` block, guarded by
`if features.utility`):

```rust
"!price" | "!block" | "!time" | "!roll" | "!weather" /* ... */
    if features.utility => {
        dispatch_utility(ctx, msg, command, args).await?;
    }
```

::: tip Command outside an existing group?
Add a new guard arm in `on_message()` (e.g. `if features.nostr =>`) or leave it
ungated like the core commands. Groups exist so operators can switch whole
suites off in `bot.toml`.
:::

## Step 3 — Add it to `!help` (COMMAND_REGISTRY)

Still in `commands.rs`, find `COMMAND_REGISTRY` — the single source of truth
for help text — and add one entry:

```rust
CommandMeta {
    name: "!block",
    description: "Bitcoin block height",
    feature: Some(Feature::Utility),
    auth: AuthLevel::Public,
},
```

`!help` now lists it automatically, and only when `features.utility` is on.

## Step 4 — (Optional) Gate it with auth

To restrict the command instead of leaving it public, wrap the handler call
with `require_auth` — it sends the ⛔ denial for you and returns `Ok(false)`:

```rust
"!block" => {
    if !require_auth(ctx, msg, AuthLevel::Authorized).await? {
        return Ok(());
    }
    utility::block_command(ctx, msg, args).await?;
}
```

Levels: `AuthLevel::Public` (no check needed), `AuthLevel::Authorized`, and
`AuthLevel::Owner`. With no `[auth]` config, `require_auth` always passes.

## Step 5 — Check, build, test

```bash
cargo check     # fast type check — run this after every change
cargo build --release
cargo run --release
```

Then in Vector: `!block` → `⛓️ Bitcoin block height: 874213`, and `!help` shows
the new entry. Done — that's a complete command: handler, dispatch, help, and
optional auth.

## Going further

### Scheduled tasks

In `src/handlers/scheduled.rs`, write a task function and register it with an
interval (seconds):

```rust
async fn block_height_task(ctx: BotContext) {
    let data = match crate::lib::http::fetch_json("https://mempool.space/api/blocks/tip/height").await {
        Ok(d) => d,
        Err(e) => { tracing::warn!("block fetch failed: {}", e); return; }
    };
    let height = data.as_u64().unwrap_or(0);

    // Post to the first configured community channel
    if let Some(channel_id) = ctx.config.communities.join.first() {
        let channel = ctx.bot.channel(channel_id.clone());
        let _ = channel.send(&format!("⛓️ New tip: block {}", height)).await;
    }
}

pub async fn register(_bot: &VectorBot, ctx: BotContext) -> Result<()> {
    spawn_interval_simple(ctx.clone(), 3600, block_height_task); // hourly
    Ok(())
}
```

### Reacting to non-command messages

For messages that don't start with `!`, add logic in `src/handlers/mod.rs` after
the command dispatch:

```rust
// React to keywords
if text.to_lowercase().contains("ship it") {
    let channel = msg.channel();
    let _ = channel.react(&msg.message.id, "🚢").await;
}

// Always answer certain phrases
if text.to_lowercase().contains("good morning") {
    msg.reply("Good morning! ☀️").await?;
}
```

### The HTTP helper

`src/lib/http.rs` (stable — use, don't rewrite):

```rust
use crate::lib::http;

let data = http::fetch_json("https://api.example.com/data").await?;
let data = http::fetch_json_with_auth("https://api.github.com/...", Some("ghp_TOKEN")).await?;
let body = serde_json::json!({"key": "value"});
let result = http::post_json("https://api.example.com/submit", &body).await?;
let text = http::fetch_text("https://wttr.in/?format=%t").await?;
```

### Reading custom config

Anything in `[custom]` of `bot.toml` reaches your handlers:

```rust
let api_base = ctx.config.custom_string("mytool.base_url").unwrap_or_default();
```

```toml
[custom.mytool]
base_url = "https://api.example.com"
```

### Cooldown / rate limiting pattern

```rust
use std::collections::HashMap;
use std::sync::Mutex;
use std::time::Instant;

static COOLDOWNS: Mutex<Option<HashMap<String, Instant>>> = Mutex::new(None);

async fn slow_command(msg: &IncomingMessage) -> Result<()> {
    let key = msg.chat_id.clone();
    let mut map = COOLDOWNS.lock().unwrap();
    let map = map.get_or_insert_with(HashMap::new);

    if let Some(last) = map.get(&key) {
        if last.elapsed().as_secs() < 30 {
            msg.reply("⏳ Command on cooldown. Try again in a bit.").await?;
            return Ok(());
        }
    }
    map.insert(key, Instant::now());
    drop(map);

    // ... handle the command ...
    Ok(())
}
```

### Multi-word arguments

```rust
// "!remind <when> <what>" → parts = ["!remind", "when", "what to remind"]
let parts: Vec<&str> = text.splitn(3, ' ').collect();
let when = parts.get(1).copied().unwrap_or("");
let what = parts.get(2).copied().unwrap_or("");
```

## Best practices

1. **Read the existing handlers first** — match the template's patterns
2. **Use the HTTP helper** — don't spin up your own HTTP client
3. **`cargo check` after every change** — seconds, not minutes
4. **One function per command**, one job per scheduled task
5. **Keys and channel IDs go in `bot.toml` `[custom]`**, never hardcoded
6. **`tracing::warn!`** for non-fatal errors, `tracing::error!` for serious ones
