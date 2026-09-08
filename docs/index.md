# concord-bots

A Rust template for **Concord v2** bots on Nostr — multi-community membership,
built-in command router, wallet & zaps, git monitoring, and an optional AI bridge.
Built on `vector_sdk` 0.3.1.

Production instances of this template: **Flagship** (community bot) and
**Shanty** (24/7 procedural lo-fi radio streaming into two communities).

## Quick start

```bash
git clone https://github.com/CentauriAgent/concord-bots
cd concord-bots
cargo build --release

# Create an identity (ONCE — never re-run on an existing bot)
./target/release/concord-bots create-identity

# Configure
cp config/bot.toml.example config/bot.toml  # holds your nsec — never commit

# Run
./target/release/concord-bots
```

Docker (v1.0): see the repo `Dockerfile` — config is mounted at runtime,
never baked into the image.

## What's inside

- [Command reference](commands.md) — public/authorized/owner commands incl. AI bridge
- [AI Bridge](ai-bridge.md) — enable `!ask` / `!summarize` / `!sentiment` / `!image`
- Repo README — project structure, what's safe to edit
- `examples/` — runnable snippets, start with `echo-bot/`
