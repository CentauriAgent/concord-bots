# Configuration reference — `config/bot.toml`

Every concord-bot instance is configured by one TOML file. Keys marked
**secret** (`nsec`, tokens, api keys) must never be committed or baked into
Docker images — mount them at runtime (see the Dockerfile pattern).

## `[bot]` — identity & profile

| Key | Purpose |
|-----|---------|
| `nsec` | **Secret.** Bot identity. Auto-generated on first run if omitted; persisted to the data dir. |
| `invite_policy` | Who may add the bot to communities (`owner` restricts to the owner's npub). |
| `display_name`, `picture`, `banner`, `about` | Kind-0 profile fields (see `examples/set_profile.rs`). |
| `lud16` | Lightning address for receiving zaps (npub.cash addresses use the full npub as username). |

## `[auth]` — command authorization

| Key | Purpose |
|-----|---------|
| `owner` | Owner npub — full control (`!add`, `!remove`, `!git poll`, …). |
| `authorized` | List of npubs allowed to use Authorized+ commands. |
| `persist` | Persist authorization changes to the data dir across restarts. |

## `[features]` — feature gates

`utility`, `fun`, `community`, `nostr`, `ai`, `moderation`, `git_monitor` —
each `true`/`false` flag enables its command group. `ai` additionally requires
the `[ai]` table (see README → AI Bridge).

## `[ai]` — AI bridge (optional)

| Key | Purpose |
|-----|---------|
| `provider` | `openai` (any OpenAI-compatible endpoint: OpenAI, OpenRouter, llama.cpp, vLLM…). |
| `model` | Model name your endpoint serves (e.g. `gpt-4o-mini`). |
| `api_key` | **Secret.** Or set `AI_API_KEY` env var. |
| `system_prompt` | Optional persona/system prompt for `!ask`/`!summarize`/`!sentiment`. |

## `[git_monitor]` — repo activity feed

| Key | Purpose |
|-----|---------|
| `enabled` | Master switch. |
| `poll_interval_secs` | Poll cadence per repo. |
| `github_token` / `gitlab_token` | **Secret.** API tokens per forge. |
| `gitlab_host` | Self-hosted GitLab base URL (e.g. `https://gitlab.com`). |
| `default_branch` | Fallback branch when a repo doesn't report one (`main`). |
| `post_commits` / `post_releases` | Which events render into the channel feed. |
| `max_repos_per_channel` | Guard against subscription spam. |
| `polite_sleep_ms` | Delay between upstream API calls. |

## `[wallet]` — Cashu wallet (bot funds)

| Key | Purpose |
|-----|---------|
| `enabled` | Enables `!balance` / `!tip` / `!deposit` / `!withdraw` / `!zap`. |
| `mint_url` | Cashu mint the bot's wallet uses. |

## `[npub_cash]` — npub.cash claims

| Key | Purpose |
|-----|---------|
| `enabled` | Auto-claim pending Cashu tokens sent to the bot's npub.cash address. |
| `url` | Service base URL. |
| `claim_interval_secs` | Claim cadence (default 300). |

## `[v2]` — Concord v2 community hosting

| Key | Purpose |
|-----|---------|
| `auto_create` | Found the community on first start. |
| `community_name` | Display name of the hosted community. |
| `join_on_start` | Rejoin known communities at boot. |

## `[watchdog]` — connection health

| Key | Purpose |
|-----|---------|
| `silence_secs` | Relay silence threshold before reconnect (default 1800; lower for deployments whose subscriptions auto-close fast). |
| `check_interval_secs` | Watchdog tick. |
| `max_backoff_secs` | Reconnect backoff ceiling. |

---

Environment overrides: `AI_API_KEY` (AI provider). Secrets are read from the
TOML first, env second. Keep exactly one identity per data dir — sharing an
nsec across instances causes membership fights.
