# Example Bots

The repo ships two kinds of examples in `examples/`:

1. **`echo-bot/`** — a complete, minimal reference bot to copy as a starting point
2. **Standalone SDK snippets** — single-file utilities run with `cargo run --example <name>`

## Echo Bot — the simplest possible bot

`examples/echo-bot/` is a complete reference implementation with just enough to
show every moving part:

```
examples/echo-bot/
├── config/
│   └── bot.toml        # auto identity, public invite policy, display name
└── handlers/
    ├── commands.rs     # !ping → "pong 🏓", !echo <text>, !help
    └── scheduled.rs    # commented-out "I'm alive!" heartbeat every 5 min
```

What it demonstrates:

- **`!ping`** → `pong 🏓` — the smallest possible command handler
- **`!echo <text>`** → echoes the text back, with a usage message when called
  without arguments
- **`!help`** — a hand-written help string (the full template generates this
  from `COMMAND_REGISTRY` instead)
- **A scheduled task pattern** (commented out) — post a heartbeat to the first
  configured community channel every 300 seconds

To use it as your starting point, copy the directory into a fresh clone, then
customize the handlers — the config already shows the two decisions every bot
makes (identity policy and invite policy):

```toml
[bot]
nsec = "auto"                 # fresh identity on first run
invite_policy = "public"      # anyone can invite (testing only — tighten for prod)
display_name = "Echo Bot"
```

::: tip From echo-bot to real bot
The echo-bot handlers are intentionally standalone (they don't import the
crate's framework). When you build on the real template, your handlers get
dispatch, auth checks, rate limiting, and `!help` generation for free — see the
[Custom Handler Tutorial](/guide/custom-handlers).
:::

## Standalone SDK snippets

Single-file utilities that talk to the Vector SDK directly. Each is a cargo
example — run it from the repo root:

### `diagnostic.rs` — inspect the bot's world

Builds a bot from the existing identity and dumps everything it can see:

```bash
cargo run --example diagnostic
```

Prints the bot's npub, all communities, all chats, and any pending community
invites as JSON. **First stop when something looks wrong** — if the bot can see
a community here but doesn't respond in it, the problem is in handlers; if it
can't see it here, the problem is connection/identity.

### `join_community.rs` — join via invite link

```bash
cargo run --example join_community -- "https://vector.app/invite/..."
```

Joins a community from an invite URL and prints the result JSON. Useful when
you want membership handled outside the bot's `invite_policy` — e.g. the policy
says `manual`, and you join deliberately with this snippet.

### `leave_community.rs` — leave a community

```bash
cargo run --example leave_community -- <community_id>
```

Publishes the leave-presence event and exits non-zero on failure. Get the
community ID from `diagnostic` output or Vector's UI.

### `set_profile.rs` — publish a full profile

Uploads avatar and banner images, then publishes a display name + about text +
images as the bot's kind 0 profile:

```bash
cargo run --example set_profile
```

The image paths are hardcoded near the top of the file
(`/tmp/flagship-avatar.png`, `/tmp/flagship-banner.png`) — edit them for your
own use. The template also publishes profile fields from `[bot]` config
(`display_name`, `about`, `picture`, `banner`, `lud16`) on every startup; this
snippet is for one-off profile surgery with uploaded images.

## Which one should I start from?

| Goal | Start with |
|------|-----------|
| Brand-new bot, full framework (auth, features, wallet, `!help` generation) | The main template — follow [Getting Started](/guide/getting-started) |
| Learning how the pieces fit together | `echo-bot/` — read both handler files top to bottom |
| Debugging a live bot | `diagnostic` |
| One-off membership/profile operations | `join_community` / `leave_community` / `set_profile` |
