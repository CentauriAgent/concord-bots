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
│   ├── handlers/          ← 🔧 YOUR CODE GOES HERE
│   │   ├── mod.rs         ← Handler dispatch
│   │   ├── commands.rs    ← !command handlers
│   │   ├── scheduled.rs   ← Scheduled/cron tasks
│   │   └── ai_bridge.rs   ← AI integration (optional)
│   └── lib/               ← Pre-built utilities (stable — don't edit)
│       ├── http.rs        ← HTTP fetch helper
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
| `src/handlers/commands.rs` | ✅ **Yes** | Add `!command` handlers |
| `src/handlers/scheduled.rs` | ✅ **Yes** | Add scheduled/interval tasks |
| `src/handlers/ai_bridge.rs` | ✅ **Yes** | Configure AI integration |
| `src/handlers/mod.rs` | ✅ **Yes** | Change dispatch logic |
| `config/bot.toml` | ✅ **Yes** | Bot configuration |
| `src/main.rs` | ❌ No | Entry point (stable) |
| `src/bot.rs` | ❌ No | Connection logic (stable) |
| `src/config.rs` | ❌ No | Config loader (stable) |
| `src/lib/` | ❌ No | Utilities (stable) |

## Built-in commands

| Command | Description |
|---------|-------------|
| `!ping` | Health check — replies with `pong 🏓` |
| `!help` | Lists all available commands |
| `!echo <text>` | Echoes back the text |
| `!whoami` | Shows the bot's npub and version |

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
