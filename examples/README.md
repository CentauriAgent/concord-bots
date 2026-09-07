# concord-bots examples

Small, runnable snippets that show one thing each. Run them from the repo root:

```bash
cargo run --example <name>
```

| Example | What it shows |
|---------|---------------|
| `join_community` | Build a bot from an existing identity and join a community from an invite URL |
| `leave_community` | Leave a community cleanly |
| `set_profile` | Publish/update the bot's kind-0 profile (name, about, picture) |
| `diagnostic` | Sanity-check identity, relays, and community memberships |
| `echo-bot/` | A complete minimal bot project — start here for your own |

## Try the AI commands in your community

If your bot config has `[features] ai = true` and an `[ai]` table (see README →
AI Bridge), every member of its communities can use:

```text
!ask <question>        — ask anything; the bot answers in-thread
!summarize <text>      — condense a long paste
!sentiment <text>      — quick sentiment read
!image <prompt>        — generate an image
```

A good smoke test after enabling: `!ask give me a one-line haiku about nostr`.

## Writing your own handler

`echo-bot/` shows the full shape: a `handlers/` module wired into the bot
builder, plus a `config/` directory with its own `bot.toml`. Copy it, rename,
add commands to the router — the framework handles keys, relays, and the
community membership lifecycle for you.
