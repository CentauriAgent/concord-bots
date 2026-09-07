# Getting Started

This guide takes you from a fresh clone to a running bot in a Vector community.

## Prerequisites

| Tool | Version | Required for |
|------|---------|--------------|
| [Rust](https://rustup.rs) | 1.85+ (stable) | Building and running the bot |
| [Vector](https://vectorapp.io) | any | Talking to your bot (mobile/desktop app) |
| Docker | any | The Docker deploy path (optional) |
| Node.js | 18+ | Building this docs site only (optional) |

Install Rust with rustup if you don't have it:

```bash
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y
source "$HOME/.cargo/env"
```

## 1. Clone the template

```bash
git clone https://github.com/CentauriAgent/concord-bots.git
cd concord-bots
```

## 2. Configure the bot

Copy the example config and edit it:

```bash
cp config/bot.toml.example config/bot.toml
```

The minimum viable config is tiny — everything is optional:

```toml
[bot]
# "auto" generates a fresh Nostr identity on first run.
# Use an explicit nsec if you want a specific identity:
#   nsec = "nsec1YOUR_KEY_HERE"
nsec = "auto"

# Who is allowed to invite this bot to communities?
# "owner" | "authorized" | "public" | "whitelist" | "manual"
invite_policy = "owner"

display_name = "My Bot"
about = "Built with concord-bots"
```

::: warning Never commit a real nsec
`config/bot.toml` is gitignored for a reason — it holds your bot's private key.
Use placeholders (`nsec1YOUR_KEY_HERE`) in anything you share or commit, and
never paste a real nsec into documentation, issues, or chat.
:::

A few decisions worth making up front:

- **Identity** — `nsec = "auto"` creates a new key on first run and persists it
  (along with the wallet) under `data/`. Keep that directory; it *is* your bot.
- **Owner** — set `[auth] owner = "npub1..."` to enable the permission system
  (see [Authorization](#authorization-optional)).
- **Communities** — list community IDs in `[communities] join` to auto-join on
  startup, or accept invites manually.

See the [Configuration guide](/guide/configuration) for every option.

## 3. Build and run

```bash
cargo run --release
```

The first build takes a few minutes (Rust compiles the SDK). On startup the bot:

1. Loads `config/bot.toml` (or `$BOT_CONFIG` if set)
2. Logs in with the configured (or auto-generated) identity and prints its **npub**
3. Publishes its profile (kind 0) to relays
4. Joins the communities listed in `[communities] join`
5. Starts listening for messages

Successful startup looks like this:

```text
INFO  concord_bots: 🚀 Concord bot starting (v2.0.0)
INFO  concord_bots: 🔑 Identity: npub1abcdef...  ← your bot's npub
INFO  concord_bots: 👥 Joining N communities
INFO  concord_bots: ✅ Bot ready — listening for messages
```

## 4. Talk to it

From the Vector app:

1. Copy the bot's npub from the startup log
2. Send it a DM: `!ping`
3. It replies `pong 🏓` — your bot is alive

Invite it to a community (or use `[communities] join`), then try `!help` in any
channel it's in. The full command list lives in the
[Command Reference](/guide/commands).

## Authorization (optional)

Without an `[auth]` section every command is public. To lock things down:

```toml
[auth]
owner = "npub1YOUR_NPUB_HERE"          # you — full control
authorized = ["npub1FRIEND_NPUB_HERE"] # seed list, extendable via !add
```

Then in Vector, as the owner:

```text
!auth            → shows your authorization level
!add npub1...    → authorize another user
!list            → who is authorized
```

Authorized users persist across restarts in `auth_state.json`.

## Next: make it yours

The whole point of the template is customization:

- **[Custom Handler Tutorial](/guide/custom-handlers)** — add your first `!command`
- **[Command Reference](/guide/commands)** — everything that ships built-in
- **[Example Bots](/guide/examples)** — the echo-bot starting point and SDK snippets

## Deploy paths

For anything long-running, move off `cargo run`:

### Docker (one command once built)

```bash
# Build the image
docker build -t concord-bots .

# Run it — mount config, persist identity, pass the key via env
docker run -d \
  --name my-bot \
  -v $(pwd)/config:/app/config \
  -v concord-data:/app/data \
  -e NSEC=nsec1YOUR_KEY_HERE \
  concord-bots
```

::: tip
Mounting `config/` and the `concord-data` volume matters: `/app/data` holds the
auto-generated identity and Cashu wallet. Lose the volume and `nsec = "auto"`
mints a brand-new bot on next start.
:::

### systemd (one command, root)

```bash
sudo ./deploy/install.sh          # builds, installs to /opt/concord-bots, enables service
sudo nano /opt/concord-bots/config/bot.toml   # then edit config
sudo systemctl start concord-bots
journalctl -u concord-bots -f     # watch it run
```

The [Deployment guide](/guide/deployment) covers both paths in depth — plus how
this documentation site itself is published to Nostr with **nsite**.
