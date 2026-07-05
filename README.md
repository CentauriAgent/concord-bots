# concord-bots

> A generic **Vector/Concord Protocol bot template** for AI agents. Build custom bots without Rust expertise.

[![License: MIT](https://img.shields.io/badge/License-MIT-yellow.svg)](https://opensource.org/licenses/MIT)

## What is this?

**Vector** is a private, end-to-end-encrypted messenger built on Nostr. The `vector_sdk` Rust crate provides bot APIs.

**concord-bots** is a template repository that lets any AI agent (OpenClaw, Claude, Cursor, etc.) build and deploy custom Vector bots by writing simple handler functions — no Rust expertise required.

## How it works

1. **You** tell your AI agent what bot you want: *"Build a bot that posts Bitcoin prices every hour and answers !weather queries."*
2. **Your agent** reads `AGENTS.md`, implements the handlers in Rust, and configures `bot.toml`.
3. **The framework** handles the Vector connection, encryption, reconnection, message routing, and scheduling.
4. **Your bot** runs as a systemd service or Docker container.

## Quick start

```bash
# Clone the template
git clone https://github.com/CentauriAgent/concord-bots.git
cd concord-bots

# Copy and edit config
cp config/bot.toml.example config/bot.toml
# Edit bot.toml with your nsec and community IDs

# Build and run
cargo run --release
```

## Project structure

```
concord-bots/
├── AGENTS.md              ← AI agent instructions (the "prompt")
├── README.md              ← This file
├── Cargo.toml             ← Dependencies (pre-configured)
├── src/
│   ├── main.rs            ← Entry point (stable — don't edit)
│   ├── bot.rs             ← Vector connection (stable — don't edit)
│   ├── config.rs          ← TOML config loader (stable — don't edit)
│   ├── auth.rs            ← Auth manager (stable — don't edit)
│   ├── handlers/          ← 🔧 YOUR CODE GOES HERE
│   │   ├── mod.rs         ← Handler dispatch
│   │   ├── commands.rs    ← !command handlers
│   │   ├── scheduled.rs   ← Scheduled/cron tasks
│   │   ├── wallet_cmds.rs ← !balance, !tip, !zap, etc.
│   │   └── ai_bridge.rs   ← AI integration (optional)
│   ├── wallet/            ← Cashu ecash wallet
│   └── lib/               ← Pre-built utilities (stable — don't edit)
│       ├── http.rs        ← HTTP fetch helper
│       ├── nip98.rs       ← NIP-98 HTTP auth
│       ├── npub_cash.rs   ← npub.cash claim client
│       ├── scheduler.rs   ← Interval scheduler
│       └── vector_client.rs ← SDK convenience wrappers
├── config/
│   └── bot.toml.example   ← Config template
├── examples/
│   └── echo-bot/          ← Simplest possible bot
├── deploy/
│   ├── concord-bots.service ← systemd template
│   ├── Dockerfile           ← Docker deployment
│   └── install.sh           ← One-command installer
└── LICENSE                 ← MIT
```

### What's safe to edit?

| File | Edit? | What it does |
|------|-------|-------------|
| `src/handlers/commands.rs` | ✅ **Yes** | Add `!command` handlers + auth checks |
| `src/handlers/scheduled.rs` | ✅ **Yes** | Add scheduled/interval tasks |
| `src/handlers/ai_bridge.rs` | ✅ **Yes** | Configure AI integration |
| `src/handlers/mod.rs` | ✅ **Yes** | Change dispatch logic |
| `config/bot.toml` | ✅ **Yes** | Bot + auth configuration |
| `src/main.rs` | ❌ No | Entry point (stable) |
| `src/bot.rs` | ❌ No | Connection logic (stable) |
| `src/auth.rs` | ❌ No | Auth manager (stable core) |
| `src/config.rs` | ❌ No | Config loader (stable) |
| `src/lib/` | ❌ No | Utilities (stable) |

## Built-in commands

| Command | Auth Level | Description |
|---------|------------|-------------|
| `!ping` | Public | Health check — replies with `pong 🏓` |
| `!help` | Public | Lists all available commands |
| `!echo <text>` | Public | Echoes back the text |
| `!whoami` | Public | Shows the bot's npub and version |
| `!auth` | Public | Shows your authorization status |
| `!add <npub>` | Owner | Adds a user to the authorized list |
| `!remove <npub>` | Owner | Removes a user from the authorized list |
| `!list` | Owner | Lists all authorized users |
| `!git add <url\|owner/repo>` | Authorized+ | Subscribe channel to a git repo |
| `!git list` | Public | List this channel's repo subscriptions |
| `!git remove <repo\|id>` | Authorized+ | Unsubscribe from a repo |
| `!git poll` | Owner | Force-poll all subscriptions in this channel |
| `!balance` | Public | Show wallet balance (sats) |
| `!tip <sats>` | Authorized+ | Send a Cashu token tip |
| `!deposit [sats]` | Authorized+ | Generate BOLT11 invoice to add funds |
| `!withdraw <invoice>` | Authorized+ | Pay a BOLT11 invoice from wallet |
| `!zap <npub> <sats> [msg]` | Authorized+ | NIP-57 Lightning zap to a Nostr user |

## Authorization System

The framework includes a built-in permission system with three levels:

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
authorized = ["npub1friend1..."]  # optional seed list
persist = true                      # save across restarts (default)
```

When not configured, all commands are public (backward-compatible).

See [`AGENTS.md`](AGENTS.md) for details on adding auth checks to custom commands.

## Wallet & Zaps

The framework includes a built-in **Cashu wallet** that can send and receive Lightning payments via ecash. Both directions are supported:

- **Send zaps:** `!zap <npub> <sats> [message]` — full NIP-57 zap flow (resolves lud16, signs kind 9734, pays BOLT11 from wallet)
- **Receive zaps:** automatic via [npub.cash](https://npub.cash) — any zaps to `<your-bot-npub>@npub.cash` are claimed every 5 minutes and credited to the wallet

### Setup

**1. Enable the wallet** in `config/bot.toml`:

```toml
[wallet]
enabled = true
mint_url = "https://mint.minibits.cash/Bitcoin"
```

**2. Set the bot's Lightning address** so it can receive zaps:

```toml
[bot]
lud16 = "<your-bot-npub>@npub.cash"

[npub_cash]
enabled = true
url = "https://npub.cash"
claim_interval_secs = 300  # poll every 5 minutes
```

Any Lightning wallet (Wallet of Satoshi, Damus, Muun, etc.) can now zap `<your-bot-npub>@npub.cash`. The bot claims tokens automatically — no manual intervention.

### Wallet commands

| Command | Auth Level | Description |
|---------|------------|-------------|
| `!balance` | Public | Show wallet balance in sats |
| `!tip <sats>` | Authorized+ | Send a Cashu token tip |
| `!deposit [sats]` | Authorized+ | Generate a BOLT11 invoice to add funds |
| `!withdraw <invoice>` | Authorized+ | Pay a BOLT11 invoice from the wallet |
| `!zap <npub> <sats> [msg]` | Authorized+ | NIP-57 zap to another Nostr user |

### How receiving works

The bot uses [npub.cash](https://npub.cash), a free service that acts as a Lightning→Cashu bridge:

1. Someone zaps `<bot-npub>@npub.cash` via any Lightning wallet
2. npub.cash receives the payment, mints Cashu tokens on the same mint the bot uses
3. The bot's scheduled task (every 5 min) authenticates via **NIP-98** (signed kind 27235 Nostr event) and claims the tokens
4. Tokens are received into the bot's Cashu wallet
5. Bot announces `⚡ Received N sats via npub.cash zap!` in its primary community channel

No manual claim step. No separate wallet. The bot's single Cashu wallet handles both inbound zaps and outbound payments.

### Files

| File | Purpose |
|------|---------|
| `src/wallet/mod.rs` | Cashu wallet wrapper (CDK-based) |
| `src/lib/nip98.rs` | NIP-98 HTTP auth header builder |
| `src/lib/npub_cash.rs` | npub.cash claim/balance client |
| `src/handlers/wallet_cmds.rs` | `!balance`, `!tip`, `!zap`, etc. |
| `src/bin/sweep_wallet.rs` | Ops: sweep wallet to a single token |
| `src/bin/melt_to_ln.rs` | Ops: melt balance to a Lightning address |

## Deployment

### systemd

```bash
sudo ./deploy/install.sh
sudo systemctl start concord-bots
journalctl -u concord-bots -f
```

### Docker

```bash
docker build -t concord-bots .
docker run -d \
  --name my-bot \
  -v $(pwd)/config:/app/config \
  -e NSEC=nsec1... \
  concord-bots
```

## Configuration

See [`config/bot.toml.example`](config/bot.toml.example) for all options.

Key settings:

```toml
[bot]
nsec = "auto"              # auto-generate identity, or provide explicit key
invite_policy = "public"   # "public", "whitelist", or "manual"

[communities]
join = ["community-id-1"]  # auto-join these on startup
```

## Examples

### Echo Bot

The simplest possible bot. See [`examples/echo-bot/`](examples/echo-bot/).

- `!ping` → "pong 🏓"
- `!echo <text>` → echoes the text

### Building your own

Tell your AI agent:

> "Read the AGENTS.md file in the concord-bots repo and build me a bot that does [your requirements]."

The agent will read AGENTS.md, implement handlers, configure the bot, and deploy it.

## For AI Agents

If you're an AI agent building a bot from this template, **read [`AGENTS.md`](AGENTS.md)** first. It has everything you need.

## Dependencies

- **[vector_sdk](https://docs.rs/vector_sdk)** — Vector messaging SDK
- **tokio** — Async runtime
- **reqwest** — HTTP client
- **serde/toml** — Configuration parsing
- **tracing** — Structured logging

## License

MIT — see [LICENSE](LICENSE).

## Links

- **Vector app:** [vectorapp.io](https://vectorapp.io)
- **vector_sdk docs:** [docs.rs/vector_sdk](https://docs.rs/vector_sdk/latest/vector_sdk/)
- **Nostr protocol:** [github.com/nostr-protocol/nips](https://github.com/nostr-protocol/nips)
