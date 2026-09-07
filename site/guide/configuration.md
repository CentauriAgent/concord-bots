# Configuration

The bot reads a single TOML file: `config/bot.toml` (override the path with the
`BOT_CONFIG` environment variable). Start from the shipped template:

```bash
cp config/bot.toml.example config/bot.toml
```

Every field is optional. With no config at all, the bot auto-generates an
identity and parks all invites until you accept them manually.

::: warning Placeholders only
This page uses `nsec1YOUR_KEY_HERE` / `npub1YOUR_NPUB_HERE` everywhere. Never
put a real nsec in files you commit, paste, or screenshot — anyone holding it
*is* your bot.
:::

## `[bot]` — identity and profile

```toml
[bot]
nsec = "auto"                # "auto" | explicit nsec | omit to use $NSEC env var
invite_policy = "owner"      # "owner" | "authorized" | "public" | "whitelist" | "manual"
# whitelist = ["npub1..."]   # only used when invite_policy = "whitelist" (legacy)

# Profile (published as kind 0 to relays on startup)
# display_name = "My Bot"
# about = "A Concord Protocol bot built with concord-bots"
# picture = "https://example.com/avatar.png"
# banner = "https://example.com/banner.png"
# lud16 = "<bot-npub>@npub.cash"   # Lightning address — enables receiving zaps
```

| Option | Default | Meaning |
|--------|---------|---------|
| `nsec` | `"auto"` | `"auto"` generates and persists a key under `data/` on first run. An explicit nsec pins the identity. Omitted → `NSEC` env var. |
| `invite_policy` | `"owner"` | Who may invite the bot to communities: `owner` (only the auth owner), `authorized` (owner + authorized list), `public` (anyone), `whitelist` (legacy list below), `manual` (park all invites, accept in code) |
| `whitelist` | — | npub list, used only by `invite_policy = "whitelist"` |
| `display_name` / `about` / `picture` / `banner` | — | Bot profile shown in Vector, published as kind 0 |
| `lud16` | — | Lightning address. Set to `<bot-npub>@npub.cash` to receive zaps via the npub.cash bridge |

## `[auth]` — permission system

```toml
[auth]
owner = "npub1YOUR_NPUB_HERE"              # required to enable auth
authorized = ["npub1FRIEND_NPUB_HERE"]     # optional seed list
persist = true                             # save authorized list across restarts (default true)
state_file = "auth_state.json"             # persistence file location
```

With no `[auth]` section, all commands are public. When configured:

- **Owner** runs everything (`!add`, `!remove`, `!shutdown`-class commands)
- **Authorized** users (owner + list + runtime `!add`) run gated commands
- Users added via `!add` persist to `state_file` (default `auth_state.json`)

## `[communities]` — auto-join

```toml
[communities]
join = ["community-id-1", "community-id-2"]   # joined on every startup
```

Find community IDs in Vector's UI or via the API. Alternatively leave empty and
accept invites manually (see `invite_policy` above).

## `[scheduling]` — task defaults

```toml
[scheduling]
default_interval_secs = 300    # default interval for scheduled tasks
```

Applies to scheduled tasks that don't specify their own interval — see the
[Custom Handler Tutorial](/guide/custom-handlers#scheduled-tasks).

## `[wallet]` — Cashu ecash wallet

```toml
[wallet]
enabled = false                                   # turn on the wallet
mint_url = "https://mint.minibits.cash/Bitcoin"   # Cashu mint
```

Enables `!balance`, `!tip`, `!deposit`, `!withdraw`. Other known mints:
`https://8333.space:3338` (bitcoin space), `https://stablenut.umint.cash`
(test mint). The wallet state persists under `data/`.

## `[npub_cash]` — receive zaps

```toml
[npub_cash]
enabled = true
url = "https://npub.cash"
claim_interval_secs = 300   # poll every 5 minutes
```

When someone zaps `<bot-npub>@npub.cash`, npub.cash mints Cashu tokens and
holds them. The bot authenticates with **NIP-98** (a signed kind 27235 event)
and claims them into its wallet automatically, then announces the receipt in
its primary community channel. Requires `lud16` set in `[bot]` and the wallet
enabled on the same mint.

## `[watchdog]` — deafness watchdog

Works around an SDK gap where a relay subscription can silently close while
the connection stays up — leaving the bot deaf with no error. When no message
arrives for `silence_secs`, the bot force-refreshes its v1 + v2 realtime
subscriptions and re-syncs communities and DMs.

```toml
[watchdog]
enabled = true
silence_secs = 1800         # 30 min of silence before first refresh
check_interval_secs = 60    # how often the timer is evaluated
startup_grace_secs = 120    # arm this long after boot (initial sync finishes first)
max_backoff_secs = 3600     # repeated refreshes back off, capped here
```

::: warning Tune `silence_secs` above your quietest normal gap
Silence is not proof of deafness — a quiet community looks exactly like a
broken subscription. Set it comfortably above the longest normal gap between
messages in your busiest channel, or the watchdog will resubscribe against a
perfectly healthy bot. Check real gaps with:
`journalctl --user -u concord-bots | grep 'Incoming message'`
:::

## `[features]` — command group flags

All default to `true` except `ai`. Disabled groups silently no-op and drop out
of `!help`.

```toml
[features]
utility = true          # !price, !time, !roll, !weather, !remind, !poll, !translate, ...
fun = true              # !8ball, !flip, !choose, !rps
community = true        # !level, !leaderboard, !profile, !giveaway, !rep, !community, !invite, ...
nostr = true            # !nostr, !nip05, !follow, !balance, !tip, !zap, !deposit, !withdraw
moderation = true       # !kick, !ban, !unban, !warn, !warnings, !grantmod, !revokemod
git_monitor = true      # !git add/list/remove/poll
ai = false              # !ask, !summarize, !sentiment, !image (+ conversational bridge)
thread_replies = true   # command responses as kind 1111 threaded replies (CORD-03 §3);
                        # false = regular kind 9 inline replies
leaderboard = true      # XP tracking, level-ups, !level/!rank/!leaderboard
                        # (also toggleable per-community with !leaderboard on|off)
```

## `[custom]` — your handler config

Anything under `[custom]` is available to handlers:

```rust
let repo = ctx.config.custom_string("github.repo").unwrap_or_default();
let token = ctx.config.custom_string("github.token");
```

### `[custom.ai]` — AI provider

```toml
[custom.ai]
enabled = true              # conversational mode: AI replies to NON-command messages
provider = "openclaw"       # "openclaw" (shells out to the openclaw CLI) or "openai"
model = "gpt-4o-mini"       # used by the openai provider
system_prompt = "You are a helpful assistant for a Nostr community."
# api_key = "sk-YOUR_KEY_HERE"   # or AI_API_KEY / OPENAI_API_KEY env vars
```

- `provider = "openclaw"` (default) runs the local `openclaw` CLI; falls back
  to a canned notice if the CLI is missing.
- `provider = "openai"` calls `api.openai.com` (Chat Completions; `!image` uses
  the Images API). Note: the endpoint is fixed to `api.openai.com` — there is
  no base-URL override.
- The `!ask`-family commands additionally need `ai = true` under `[features]`;
  `enabled = true` here additionally turns on replies to non-command messages.

## `[v2]` — Concord v2 bootstrap

```toml
[v2]
auto_create = false             # create a v2 community on startup if in none
community_name = "My Bot Community"
join_on_start = [               # invite links to join on startup
  "https://vector.app/invite/...",
]
```

## Environment variables

| Variable | Purpose |
|----------|---------|
| `NSEC` | Bot private key — alternative to `bot.nsec` (used by the Docker image) |
| `BOT_CONFIG` | Path to the config file (Docker default: `/app/config/bot.toml`) |
| `RUST_LOG` | Log level, e.g. `info`, `debug` (Docker default: `info`) |
| `AI_API_KEY` | API key for the `openai` AI provider |
| `OPENAI_API_KEY` | Fallback if `AI_API_KEY` is unset |

## Removed: `[automod]`

Automatic spam detection and auto-kick/ban now live in the standalone
**concord-automod** bot, which runs as its own npub alongside a concord-bot
instance. An `[automod]` section left in an old `bot.toml` is ignored (unknown
sections parse fine) — delete it at your convenience. This template keeps the
manual moderation commands under `features.moderation`.
