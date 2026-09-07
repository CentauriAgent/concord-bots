# Command Reference

Every command ships built-in. Users trigger them with `!` in any channel the
bot is in (or in DMs).

Two things control what actually runs:

- **Feature flags** — command groups can be switched off in `bot.toml` under
  `[features]`. Disabled commands silently no-op (they don't appear in `!help`
  and unknown-command replies never leak their existence). Defaults: everything
  on except `ai`.
- **Auth levels** — who may run a command.

| Level | Who |
|-------|-----|
| **Public** | Anyone |
| **Authorized** | The owner + users added with `!add` |
| **Owner** | Only the `[auth] owner` npub |

::: tip
With no `[auth]` section configured, every command is public
(backward-compatible) — but the table below still shows each command's intended
level for when auth is on.
:::

## Core commands (always enabled)

| Command | Level | Description |
|---------|-------|-------------|
| `!ping` | Public | Health check — replies `pong 🏓` |
| `!help` | Public | Lists available commands (feature-aware — only shows enabled groups) |
| `!echo <text>` | Public | Echoes the text back |
| `!whoami` | Public | Shows the bot's npub and version |
| `!auth` | Public | Shows *your* authorization status |
| `!stats` | Public | Bot statistics |
| `!repo` | Public | Bot repository URL |
| `!add <npub>` | Owner | Adds a user to the authorized list |
| `!remove <npub>` | Owner | Removes a user from the authorized list |
| `!list` | Owner | Lists authorized users |
| `!enable` | Owner | Enable the bot in this channel |
| `!disable` | Owner | Disable the bot in this channel |

`!enable` / `!disable` are useful for communities where you want the bot active
only in specific channels.

## Utility commands (`features.utility`)

| Command | Level | Description |
|---------|-------|-------------|
| `!price` | Public | Bitcoin price in USD |
| `!time [timezone]` | Public | Current time, optional timezone argument |
| `!roll [NdS]` | Public | Dice roller — `!roll`, `!roll 20`, `!roll 3d6` |
| `!weather <zipcode>` | Public | Weather for a US zipcode, e.g. `!weather 10001` |
| `!remind <time> <message>` | Public | Set a reminder — `!remind 30m Call mom`, `!remind 2h Check oven`, `!remind 1d Pay bills` |
| `!poll <question> \| opt1 \| opt2` | Public | Create a poll — `!poll Pizza? \| Yes \| No \| Maybe` |
| `!translate <lang> <text>` | Public | Translate — `!translate es Hello world` |
| `!define <word>` | Public | Dictionary definition |
| `!quote` | Public | Random inspirational quote |
| `!joke` | Public | Random dad joke |
| `!fact` | Public | Random fun fact |
| `!meme` | Public | Random meme |
| `!shorten <url>` | Public | Shorten a URL |
| `!delete <message_id>` | Authorized | Delete a message (your own, or where the bot has MANAGE_MESSAGES capability) |
| `!edit <message_id> <new text>` | Authorized | Edit a message by ID |
| `!savefile` | Authorized | Save a message attachment to disk |

## Fun commands (`features.fun`)

| Command | Level | Description |
|---------|-------|-------------|
| `!8ball [question]` | Public | Magic 8-ball |
| `!flip` | Public | Flip a coin |
| `!choose <a> or <b> or ...` | Public | Pick randomly between options |
| `!rps <rock\|paper\|scissors>` | Public | Rock paper scissors |

## AI bridge commands (`features.ai`, off by default)

Enable with `ai = true` under `[features]`, and configure a provider under
`[custom.ai]` — see [AI Bridge](#ai-bridge) below for setup.

| Command | Level | Description |
|---------|-------|-------------|
| `!ask <question>` | Public | Ask the AI a question |
| `!summarize <text>` | Public | Summarize a block of text |
| `!sentiment <text>` | Public | Analyze the sentiment of text |
| `!image <prompt>` | Public | Generate an image from a prompt (OpenAI provider only) |

```text
!ask what's the difference between NIP-17 and NIP-04 DMs?
!summarize <paste of a long post>
!sentiment this community is on fire today 🚀
!image a lighthouse beaming purple light over a calm sea
```

### AI Bridge setup

Two config pieces are involved:

```toml
[features]
ai = true          # 1. turns on the !ask/!summarize/!sentiment/!image commands

[custom.ai]
provider = "openai"   # 2. which backend answers. "openclaw" (default) or "openai"
model = "gpt-4o-mini" # used by the openai provider
system_prompt = "You are a helpful assistant for a Nostr community."
# api_key = "sk-YOUR_KEY_HERE"   # or set the AI_API_KEY env var
```

- **`provider = "openclaw"`** (default) shells out to the `openclaw` CLI on the
  host (`openclaw chat --system ... --message ...`). If the CLI isn't installed,
  the bot replies with a fallback notice instead of a real answer.
- **`provider = "openai"`** calls the OpenAI Chat Completions API at
  `api.openai.com`. Requires `api_key` in `[custom.ai]`, or the `AI_API_KEY` /
  `OPENAI_API_KEY` environment variables. `!image` uses the OpenAI Images API
  and only works with this provider.
- **Conversational mode** — additionally setting `enabled = true` under
  `[custom.ai]` makes the bot answer *non-command* messages too (everything
  that doesn't start with `!`). Leave it off and the AI is command-only.

## Nostr & wallet commands (`features.nostr`)

Profile lookups plus the built-in Cashu wallet. Wallet commands require
`[wallet] enabled = true` — see the
[Configuration guide](/guide/configuration#wallet) for mint setup.

| Command | Level | Description |
|---------|-------|-------------|
| `!nostr <npub>` | Public | Look up a Nostr profile |
| `!nip05 <user@domain>` | Public | Verify a NIP-05 identifier — `!nip05 derekross@nostrplebs.com` |
| `!follow <npub>` | Owner | Follow a user on Nostr |
| `!balance` | Owner | Show Cashu wallet balance in sats |
| `!tip <sats> [memo]` | Authorized | Send a Cashu token tip — `!tip 21 Thanks for the help!` |
| `!deposit [sats]` | Public | Generate a BOLT11 invoice to add funds |
| `!withdraw <invoice>` | Owner | Pay a BOLT11 invoice from the wallet |
| `!zap <npub> <sats> [msg]` | Authorized | NIP-57 Lightning zap — `!zap npub1... 21 Great post!` |

**Receiving zaps:** set `lud16 = "<bot-npub>@npub.cash"` in `[bot]` and enable
`[npub_cash]`. Any Lightning wallet can then zap that address; the bot claims
the tokens into its Cashu wallet automatically every 5 minutes and announces
the receipt in its primary channel.

## Moderation commands (`features.moderation`)

| Command | Level | Description |
|---------|-------|-------------|
| `!kick <npub>` | Authorized | Kick a member — accepts `npub1...` or `nostr:npub1...` |
| `!ban <npub>` | Owner | Ban a member |
| `!unban <npub>` | Owner | Lift a ban |
| `!warn <npub> <reason>` | Authorized | Warn a member — `!warn npub1... Please be respectful` |
| `!warnings <npub>` | Authorized | Show a member's warning history |
| `!grantmod <npub>` | Owner | Grant the admin role |
| `!revokemod <npub>` | Owner | Revoke the admin role |

Notes:

- **Warnings persist** in `data/warnings.json` and survive restarts.
- **Concord v2 outranking applies** — kicks and bans fail if the target outranks
  the bot's role in the community.
- **Bans trigger an automatic community rekey** in v2 communities (part of the
  Concord protocol's forward secrecy).
- To see who has moderation powers, use [`!roles`](#v2-community-management) —
  it lists the community's roles and their holders.
- **Auto-moderation is not part of this template.** Spam detection and
  auto-kick/ban live in the separate `concord-automod` bot, which runs as its
  own npub alongside a concord-bot instance. This template ships the *manual*
  moderation suite only.

## Community engagement (`features.community`)

XP tracking, levels, and engagement. Level/XP commands additionally respect the
`leaderboard` feature flag (`true` by default), which can also be toggled
per-community at runtime.

| Command | Level | Description |
|---------|-------|-------------|
| `!level` / `!rank` | Public | Show your level and XP |
| `!leaderboard [on\|off\|status]` | Public | Top 10 by XP, or toggle XP tracking for this community |
| `!profile` | Public | Show your user profile card |
| `!giveaway <duration> <prize>` | Authorized | Start a giveaway |
| `!rep <npub>` | Public | Give another member +1 reputation |
| `!welcome [on\|off]` | Owner | Toggle welcome messages for new members (no args shows current state) |

Members earn XP by chatting and receiving reactions; level-ups are announced in
the channel.

## Git monitor (`features.git_monitor`)

Subscribe a channel to GitHub/GitLab repositories; the bot polls for new
activity and posts updates.

| Command | Level | Description |
|---------|-------|-------------|
| `!git add <url\|owner/repo>` | Authorized+ | Subscribe this channel to a repo |
| `!git list` | Public | List this channel's repo subscriptions |
| `!git remove <repo\|id>` | Authorized+ | Unsubscribe from a repo |
| `!git poll` | Owner | Force-poll all subscriptions in this channel |

## v2 community management (`features.community`)

Concord v2 protocol community administration:

| Command | Level | Description |
|---------|-------|-------------|
| `!community <create\|info\|leave\|dissolve>` | Authorized | Manage the current v2 community — `!community create <name>` makes a new one |
| `!invite [npub]` | Authorized | No args → create a shareable invite link; with npub → direct invite |
| `!join <invite_link>` | Owner | Join a community via invite link |
| `!members` | Public | List community members |
| `!channels` | Public | List community channels |
| `!roles` | Public | Show community roles |
| `!caps` | Public | Show community capabilities |

The bot works in v1 and v2 communities simultaneously — the SDK handles the
protocol differences transparently.
