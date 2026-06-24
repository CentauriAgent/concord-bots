# AGENTS.md — concord-bots: AI Agent Guide

> **You are an AI agent building a Vector bot.** This file teaches you everything you need to know.

## TL;DR

1. Edit files in `src/handlers/` — that's where ALL custom code goes
2. Don't touch `src/main.rs`, `src/lib/`, or `src/auth.rs`
3. Configure the bot in `config/bot.toml`
4. Test with `cargo check` and `cargo run`
5. Deploy with `./deploy/install.sh` or Docker

---

## Project Structure

```
┌─────────────────────────────────────────────────────────┐
│                    src/main.rs (stable)                  │
│                   Boots the framework                    │
└──────────────────────┬──────────────────────────────────┘
                       │
                       ▼
┌─────────────────────────────────────────────────────────┐
│                    src/bot.rs (stable)                   │
│  Builds VectorBot, registers handlers, runs event loop  │
│  Initializes AuthManager from [auth] config              │
└──────────────────────┬──────────────────────────────────┘
                       │
                       ▼
┌─────────────────────────────────────────────────────────┐
│                 src/handlers/ (YOU EDIT)                 │
│                                                         │
│  ┌─────────────┐  ┌──────────────┐  ┌───────────────┐  │
│  │ commands.rs  │  │ scheduled.rs │  │ ai_bridge.rs  │  │
│  │ !commands    │  │ cron/interval│  │ AI responses  │  │
│  │ +auth checks │  │              │  │               │  │
│  └─────────────┘  └──────────────┘  └───────────────┘  │
│                                                         │
│  ┌─────────────────────────────────────────────────┐   │
│  │ mod.rs — dispatch (wires it all together)        │   │
│  └─────────────────────────────────────────────────┘   │
└─────────────────────────────────────────────────────────┘

Core utilities (stable — don't edit):
┌────────────────────┐  ┌────────────────────────┐
│ src/auth.rs        │  │ src/config.rs           │
│ AuthManager        │  │ TOML loader             │
│ Permission levels  │  │ + AuthSection           │
└────────────────────┘  └────────────────────────┘
```

**The rule:** If it's in `src/handlers/`, you edit it. If it's not, you don't.

---

## How to Add a Command

Commands are triggered by messages starting with `!` (e.g., `!price`, `!weather NYC`).

### Step 1: Add a match arm in `src/handlers/commands.rs`

Find the `match command {` block in `on_message()` and add:

```rust
"!price" => {
    price_command(ctx, msg).await?;
}
```

### Step 2: Write the handler function

Add a function in the same file:

```rust
async fn price_command(ctx: &BotContext, msg: &IncomingMessage) -> Result<()> {
    // Use the HTTP helper to fetch data
    let data = crate::lib::http::fetch_json(
        "https://api.coingecko.com/api/v3/simple/price?ids=bitcoin&vs_currencies=usd"
    ).await?;

    let price = data["bitcoin"]["usd"]
        .as_f64()
        .map(|p| format!("${:.0}", p))
        .unwrap_or_else(|| "unavailable".to_string());

    // Reply in the same channel (works for both DMs and communities)
    msg.reply(&format!("₿ Bitcoin: {}", price)).await?;
    Ok(())
}
```

### Step 3: Add it to the !help text

Update `help_text()` in the same file:

```rust
commands.push(("!price", "Show current Bitcoin price"));
```

### That's it.

The framework handles:
- Parsing the command name from the message
- Dispatching to your handler
- Error handling (logs errors, doesn't crash)
- Routing replies to the correct channel

---

## Authorization System

The framework has a built-in permission system so you can control who can use which commands.

### Permission Levels

| Level | Who | Example Commands |
|-------|-----|------------------|
| **Public** | Anyone | `!ping`, `!price`, `!help` |
| **Authorized** | Owner + users added via `!add` | `!status`, `!weather` |
| **Owner** | Only the configured owner | `!add`, `!remove`, `!shutdown` |

### Setup

Set the owner npub in `config/bot.toml`:

```toml
[auth]
owner = "npub1yournpub..."
authorized = ["npub1friend1...", "npub1friend2..."]  # optional seed list
```

When not configured, all commands are public (backward-compatible).

### Built-in Auth Commands

| Command | Level | Description |
|---------|-------|-------------|
| `!auth` | Public | Shows your authorization status |
| `!add <npub>` | Owner | Adds a user to the authorized list |
| `!remove <npub>` | Owner | Removes a user from the authorized list |
| `!list` | Owner | Lists all authorized users |

Authorized users persist across restarts (saved to `auth_state.json` by default).

### Adding Auth to Your Commands

Use the `require_auth()` helper in `src/handlers/commands.rs`:

```rust
use crate::auth::AuthLevel;

// In the match block:

"!price" => {
    // Public — no auth check needed
    price_command(ctx, msg).await?;
}

"!status" => {
    // Authorized only — owner + added users
    if !require_auth(ctx, msg, AuthLevel::Authorized).await? { return Ok(()); }
    status_command(ctx, msg).await?;
}

"!shutdown" => {
    // Owner only
    if !require_auth(ctx, msg, AuthLevel::Owner).await? { return Ok(()); }
    msg.reply("Shutting down...").await?;
}
```

The `require_auth()` helper:
- Checks the sender's npub against the AuthManager
- Sends a ⛔ denial message if not authorized
- Returns `Ok(false)` so you can early-return from the handler
- If auth is not configured, always returns `Ok(true)` (backward-compatible)

### Auth Config Reference

```toml
[auth]
owner = "npub1..."           # Required to enable auth
authorized = []              # Seed list of authorized npubs
persist = true               # Save authorized list across restarts (default: true)
state_file = "auth_state.json"  # Persistence file (default: auth_state.json)
```

---

## How to Add a Scheduled Task

Scheduled tasks run on intervals (e.g., every hour, every 5 minutes).

### Step 1: Write the task function in `src/handlers/scheduled.rs`

```rust
async fn bitcoin_price_task(ctx: BotContext) {
    let data = match crate::lib::http::fetch_json(
        "https://api.coingecko.com/api/v3/simple/price?ids=bitcoin&vs_currencies=usd"
    ).await {
        Ok(d) => d,
        Err(e) => {
            tracing::warn!("Failed to fetch BTC price: {}", e);
            return;
        }
    };

    let price = data["bitcoin"]["usd"]
        .as_f64()
        .map(|p| format!("${:.0}", p))
        .unwrap_or_else(|| "unavailable".to_string());

    // Send to the first configured community channel
    if let Some(channel_id) = ctx.config.communities.join.first() {
        let channel = ctx.bot.channel(channel_id.clone());
        let _ = channel.send(&format!("₿ Bitcoin: {}", price)).await;
    }
}
```

### Step 2: Register it in `register()`

```rust
pub async fn register(_bot: &VectorBot, ctx: BotContext) -> Result<()> {
    // Post Bitcoin price every hour (3600 seconds)
    spawn_interval_simple(ctx.clone(), 3600, bitcoin_price_task);
    Ok(())
}
```

---

## How to React to Messages (Non-command)

For messages that don't start with `!`, you can add custom logic in `src/handlers/mod.rs`:

```rust
// In the on_message function, after the command dispatch:

// React to keywords
if text.to_lowercase().contains("agreed") {
    let channel = msg.channel();
    let _ = channel.react(msg.id().to_string(), "👍").await;
}

// Always respond to certain phrases
if text.to_lowercase().contains("good morning") {
    msg.reply("Good morning! ☀️").await?;
}
```

---

## How to Use the HTTP Helper

The `src/lib/http.rs` module provides simple HTTP helpers:

```rust
use crate::lib::http;

// GET JSON
let data = http::fetch_json("https://api.example.com/data").await?;
let name = data["name"].as_str().unwrap_or("unknown");

// GET JSON with auth
let data = http::fetch_json_with_auth(
    "https://api.github.com/repos/my-org/my-repo",
    Some("ghp_token...")
).await?;

// POST JSON
let body = serde_json::json!({"key": "value"});
let result = http::post_json("https://api.example.com/submit", &body).await?;

// GET plain text
let text = http::fetch_text("https://wttr.in/?format=%t").await?;
```

---

## How to Configure AI Responses (Optional)

To make your bot respond intelligently to non-command messages:

### Step 1: Enable in config/bot.toml

```toml
[custom.ai]
enabled = true
provider = "openclaw"  # or "openai"
system_prompt = "You are a helpful assistant in a Vector community."
```

### Step 2: Uncomment the AI dispatch in `src/handlers/mod.rs`

```rust
// In on_message(), after the command dispatch:
if ai_bridge::is_enabled(ctx) {
    return ai_bridge::on_message(ctx, msg).await;
}
```

---

## Configuration Reference

### bot.toml

```toml
[bot]
nsec = "auto"                    # "auto" to generate, or explicit nsec
invite_policy = "manual"         # "public", "whitelist", or "manual"
whitelist = ["npub1..."]         # only for invite_policy = "whitelist"
display_name = "My Bot"          # shown in Vector
about = "Description"            # shown in Vector profile

[communities]
join = ["community-id-1"]        # auto-join on startup

[scheduling]
default_interval_secs = 300      # default for scheduled tasks

[custom]                         # your custom config values
# Access via ctx.config.custom_string("key")
```

### Custom config values

Any TOML under `[custom]` is accessible in handlers:

```rust
let repo = ctx.config.custom_string("github.repo").unwrap_or_default();
let token = ctx.config.custom_string("github.token");
```

---

## Deployment

### Option 1: systemd (recommended for VPS)

```bash
# Build and install
sudo ./deploy/install.sh

# Edit config
sudo nano /opt/concord-bots/config/bot.toml

# Start
sudo systemctl start concord-bots

# Watch logs
journalctl -u concord-bots -f
```

### Option 2: Docker

```bash
docker build -t concord-bots .
docker run -d \
  --name my-bot \
  -v $(pwd)/config:/app/config \
  -e NSEC=nsec1... \
  concord-bots

# Logs
docker logs -f my-bot
```

### Option 3: Direct (for development)

```bash
cargo run --release
```

---

## Vector SDK API Reference

### IncomingMessage fields and methods

```rust
msg.text()               // -> &str     The message text
msg.reply("text")        // Reply in same channel (DM or community)
msg.is_mine()            // -> bool     Did we send this?
msg.channel()            // -> Channel  Channel handle
msg.member()             // -> Option<Member>  Community member (if in community)
msg.community()          // -> Option<Community>

// Access message fields via msg.message:
msg.message.id           // -> String   Message ID (for reactions)
msg.message.content      // -> String   Raw message content
msg.message.npub         // -> Option<String>  Sender's npub
msg.message.mine         // -> bool     Same as is_mine()
msg.message.attachments  // -> Vec<Attachment>  File attachments

// IncomingMessage also has:
msg.chat_id              // -> String   Sender's npub (DM) or channel ID
msg.is_group             // -> bool     True if from a community channel
msg.is_file              // -> bool     True if message has file attachment
```

### Channel methods

```rust
let channel = bot.channel(id);
channel.send("text").await?;            // Send a message
channel.react(msg_id, "👍").await?;     // React with emoji (msg_id is &str)
channel.send_file("./photo.png").await?; // Send a file
channel.typing().await?;                // Show typing indicator
```

### Member methods (community moderation)

```rust
if let Some(member) = msg.member() {
    member.kick().await?;       // Kick from community
    member.ban().await?;        // Ban from community
    member.grant_admin().await?; // Promote to admin
    member.is_admin()           // -> bool
}
```

### Bot methods

```rust
bot.npub()              // -> &str    Bot's npub
bot.channel(id)         // -> Channel Open any channel
bot.community(id)       // -> Community
bot.pending_invites()   // -> Vec<...> Pending community invites
bot.accept_invite(id)   // Accept a community invite
```

### Events (BotEvent variants)

```rust
BotEvent::Message(msg)                       // A new message
BotEvent::MessageUpdate { chat_id, message } // Edit or reaction
BotEvent::Delete { chat_id, message_id }     // Message deleted
BotEvent::MemberJoin { channel_id, npub }    // Someone joined
BotEvent::MemberLeave { channel_id, npub }   // Someone left
BotEvent::Typing { chat_id, npub, until }    // Typing indicator
BotEvent::Invite { community_id }            // Community invite
BotEvent::Removed { community_id }           // Bot was kicked/banned
```

---

## Common Patterns

### Pattern: Cooldown/rate limiting

```rust
use std::collections::HashMap;
use std::sync::Mutex;
use std::time::Instant;

static COOLDOWNS: Mutex<Option<HashMap<String, Instant>>> = Mutex::new(None);

async fn rate_limited_command(msg: &IncomingMessage) -> Result<()> {
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
    msg.reply("Done!").await?;
    Ok(())
}
```

### Pattern: Multi-word arguments

```rust
let parts: Vec<&str> = text.splitn(3, ' ').collect();
// "!remind <when> <what>" → parts = ["!remind", "when", "what to remind"]
let when = parts.get(1).copied().unwrap_or("");
let what = parts.get(2).copied().unwrap_or("");
```

### Pattern: Polling an API for changes

```rust
static LAST_PRICE: Mutex<Option<f64>> = Mutex::new(None);

async fn price_alert_task(ctx: BotContext) {
    let data = match crate::lib::http::fetch_json(
        "https://api.coingecko.com/api/v3/simple/price?ids=bitcoin&vs_currencies=usd"
    ).await {
        Ok(d) => d,
        Err(_) => return,
    };

    let current = data["bitcoin"]["usd"].as_f64().unwrap_or(0.0);
    let mut last = LAST_PRICE.lock().unwrap();

    if let Some(prev) = *last {
        let change = ((current - prev) / prev * 100.0).abs();
        if change > 5.0 {
            if let Some(channel_id) = ctx.config.communities.join.first() {
                let channel = ctx.bot.channel(channel_id.clone());
                let _ = channel.send(&format!(
                    "🚨 BTC moved {:.1}%: ${:.0} → ${:.0}", change, prev, current
                )).await;
            }
        }
    }
    *last = Some(current);
}
```

### Pattern: React to all messages containing a keyword

```rust
// In src/handlers/mod.rs, in on_message():
if text.to_lowercase().contains("ship it") {
    let channel = msg.channel();
    let _ = channel.react(&msg.message.id, "🚢").await;
}
```

---

## Testing

```bash
# Type check (fast)
cargo check

# Full build
cargo build

# Run tests
cargo test

# Run the bot (needs a valid nsec or auto-generate)
cargo run --release
```

---

## Troubleshooting

### Bot doesn't connect
- Check your nsec is valid (or use `nsec = "auto"`)
- Ensure network access to Nostr relays
- Check logs: `journalctl -u concord-bots -f`

### Bot doesn't respond to commands
- Verify the message starts with `!` (e.g., `!ping`)
- Check that your handler is in the `match` block
- Look for error logs from the handler

### Bot doesn't join communities
- Set `invite_policy = "public"` or `"whitelist"` in config
- Or manually accept invites in code

### Build fails
- Ensure you have Rust installed (`rustup`)
- Run `cargo update` to refresh dependencies
- Check you're using a compatible Rust version (1.75+)

---

## Best Practices for AI Agents

1. **Read the existing code first** — Understand the patterns before writing
2. **Use the HTTP helper** — Don't create your own HTTP client
3. **Handle errors gracefully** — Use `tracing::warn!` for non-fatal errors, `tracing::error!` for serious ones
4. **Add comments** — Explain what your handlers do for the next agent
5. **Test incrementally** — Run `cargo check` after each change
6. **Keep handlers focused** — One function per command, one task per scheduled job
7. **Use config values** — Put API keys and channel IDs in `bot.toml`, not hardcoded
8. **Follow the existing patterns** — Match the code style in the template

---

## Need help?

- **Vector SDK docs:** https://docs.rs/vector_sdk/latest/vector_sdk/
- **Nostr protocol:** https://github.com/nostr-protocol/nips
- **Framework repo:** https://github.com/CentauriAgent/concord-bots
