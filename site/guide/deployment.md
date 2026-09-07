# Deployment

Three ways to run the bot, plus how this documentation site itself is hosted on
Nostr with **nsite**.

| Path | Best for |
|------|----------|
| [systemd](#systemd) | A VPS or home server — the recommended production path |
| [Docker](#docker) | Container hosts, isolated repeats, one-machine-many-bots |
| [Direct `cargo run`](#direct-development) | Development only |

For the docs site: [Publishing to nsite](#publishing-the-docs-to-nsite) —
no GitHub Pages, no DNS, just Nostr relays and Blossom servers.

## systemd

### One command

```bash
sudo ./deploy/install.sh [path/to/bot.toml]
```

The installer:

1. Installs Rust via rustup if `cargo` is missing
2. Creates a dedicated `concord-bot` system user (no login shell)
3. Builds the release binary from the repo
4. Installs the binary to `/usr/local/bin/concord-bots`
5. Copies your config (argument → repo `config/bot.toml` → the example) to
   `/opt/concord-bots/config/bot.toml`
6. Writes and enables a systemd unit (`enabled`, not started — config first!)

Then:

```bash
sudo nano /opt/concord-bots/config/bot.toml   # set nsec, owner, communities
sudo systemctl start concord-bots
journalctl -u concord-bots -f                 # logs
```

### The unit file

`deploy/concord-bots.service` is the template — the installer generates the
same shape:

```ini
[Unit]
Description=Concord Bot (Vector/Concord Protocol Bot)
After=network-online.target
Wants=network-online.target

[Service]
Type=simple
ExecStart=/usr/local/bin/concord-bots
WorkingDirectory=/opt/concord-bots
Environment=RUST_LOG=info
Environment=BOT_CONFIG=/opt/concord-bots/config/bot.toml
User=concord-bot
Restart=always
RestartSec=10

[Install]
WantedBy=multi-user.target
```

Notes:

- `Restart=always` + `RestartSec=10` — crashes self-heal in ten seconds
- `WorkingDirectory` matters: relative paths like `auth_state.json` and
  `data/` resolve against it, which is why the bot user's home is
  `/opt/concord-bots`
- Prefer passing the key via config file (chmod 600) or an
  `Environment=NSEC=...` override (`systemctl edit concord-bots`) over baking
  it into the unit

### Updating

```bash
cd /path/to/concord-bots && git pull
cargo build --release
sudo cp target/release/concord-bots /usr/local/bin/
sudo systemctl restart concord-bots
```

## Docker

The root `Dockerfile` is the canonical build. (`deploy/` also carries the
systemd helpers.)

### Build & run

```bash
docker build -t concord-bots .

docker run -d \
  --name my-bot \
  -v $(pwd)/config:/app/config \
  -v concord-data:/app/data \
  -e NSEC=nsec1YOUR_KEY_HERE \
  concord-bots

docker logs -f my-bot
```

### What the image does

Multi-stage, ~2 layers of runtime:

1. **`builder`** (`rust:1.85-slim`) — copies `Cargo.toml` + `Cargo.lock` and
   builds a dummy `main.rs` first, so dependency compilation is a cached layer
   that only invalidates when manifests change. Then the real `src/` is copied
   and built.
2. **`runtime`** (`debian:bookworm-slim`) — only the release binary plus
   `ca-certificates`, running as a non-root `concord` user, with
   `RUST_LOG=info` and `BOT_CONFIG=/app/config/bot.toml` preset.

`.dockerignore` keeps `target/`, `.git/`, `data/`, and real configs
(`config/bot.toml`) out of the build context and image.

### The two mounts (both matter)

| Mount | Why |
|-------|-----|
| `-v $(pwd)/config:/app/config` | The bot reads `/app/config/bot.toml` (`$BOT_CONFIG`). Mount your config dir — **never bake `bot.toml` into the image**, it holds the nsec. |
| `-v concord-data:/app/data` | `/app/data` persists the auto-generated identity, wallet, and warnings. **Lose this volume and `nsec = "auto"` mints a brand-new bot** on next start. |

If you pass `-e NSEC=...`, the env var wins over the config file — handy for
keeping the key out of mounted files entirely.

### Updating & operating

```bash
git pull
docker build -t concord-bots .
docker stop my-bot && docker rm my-bot
# ...then docker run again (see above)
```

- Logs: `docker logs -f my-bot`
- Restart policy: add `--restart unless-stopped` for self-healing like systemd
- Multiple bots from one image: repeat `docker run` with different names,
  config mounts, and `NSEC` values

## Direct (development)

```bash
cargo run --release
```

Fine while iterating (`cargo check` between changes), but for anything
long-running use systemd or Docker so restarts and logs are managed.

---

## Publishing the docs to nsite

This documentation site is a static site (VitePress) published to **Nostr** —
censorship-resistant hosting with no web server, no DNS, and no GitHub Pages.

### How nsite hosting works

- Every file of the built site is uploaded to one or more **Blossom servers**
  (blob storage) — each blob addressed by its SHA-256 hash
- The **file listing** (path → hash mapping) is published as **kind 34128
  Nostr events**, signed by the publishing key
- **Viewers** (nsite gateways) read the kind 34128 events from relays, fetch
  the blobs from Blossom servers, and serve them as a normal website
- Only the holder of the publishing key can alter the site — content is
  keyed, not hosted

Serving options:

- **Public gateways** — nsite viewers such as `https://nsite.cc/<npub>/`
  serve any published npub. Availability of public gateways varies; treat them
  as convenience, not contract.
- **Self-hosted gateway** — the reference implementation
  [`nsite-gateway`](https://github.com/hzrd149/nsite-gateway) (TypeScript, by
  the Blossom author):

  ```bash
  npx nsite-gateway          # reads .env config, serves on :3000
  # or
  docker run --rm -p 3000:3000 ghcr.io/hzrd149/nsite-gateway
  ```

  Your gateway, your uptime — pairs well with running it behind Tor/I2P (the
  gateway supports onion hosting out of the box).

### The tooling

We use [`nsite-cli`](https://github.com/flox1an/nsite-cli) (npm package
`nsite-cli`, current `0.1.18`):

```bash
npx nsite-cli            # interactive setup — saves .nsite/project.json
npx nsite-cli upload dist --fallback=/index.html
npx nsite-cli ls <npub>  # list a published site's files
```

::: warning Project status
The `nsite-cli` GitHub README marks the project **not actively maintained**
and points to [nsyte.run](https://nsyte.run/) as the actively developed
successor. `nsite-cli@0.1.18` still works and is pinned in our deploy script;
migrating the publish command to nsyte later is a one-line change in
[`site/deploy-nsite.sh`](https://github.com/CentauriAgent/concord-bots/blob/main/site/deploy-nsite.sh).
:::

Key details that matter for a VitePress site:

- `--fallback=/index.html` — uploads a copy of `index.html` as `/404.html` so
  deep links and client-side routing land on the app instead of an error
- Env vars beat flags — `NOSTR_RELAYS`, `BLOSSOM_SERVERS`,
  `NOSTR_PRIVATE_KEY` (nsec **or** hex) — so the key never appears in shell
  history or the repo
- Debug with `DEBUG=nsite*` if an upload stalls

Recommended infrastructure (verified reachable at the time of writing):

| Role | Endpoints |
|------|-----------|
| Relays | `wss://nos.lol`, `wss://relay.primal.net`, `wss://relay.damus.io` (add your own — e.g. a relay you run) |
| Blossom servers | `https://nostr.download`, `https://cdn.satellite.earth` (use at least two for redundancy) |

### The deploy script

[`site/deploy-nsite.sh`](https://github.com/CentauriAgent/concord-bots/blob/main/site/deploy-nsite.sh)
wraps the whole flow — build, then upload — with a safety interlock so it
can't fire by accident:

```bash
cd site
npm install
NSITE_PUBLISH=yes npm run docs:deploy     # refuses without NSITE_PUBLISH=yes
```

Without `NSITE_PUBLISH=yes` it builds, prints the exact command it *would*
run, and exits — useful for a dry run. The signing key comes from
`$NOSTR_PRIVATE_KEY` (never from a file in the repo), and relays/servers can
be overridden via `$NOSTR_RELAYS` / `$BLOSSOM_SERVERS`.

### Local preview

```bash
cd site
npm install
npm run docs:dev      # dev server with hot reload
npm run docs:build    # static output in site/dist/
npm run docs:preview  # serve the built dist/ locally
```

### Run this to go live

**One decision first: which npub hosts the docs.** The publishing key becomes
the site's identity and its address at every gateway (`/<npub>/`). Generate a
dedicated key (`nak key generate` or any Nostr client) — don't reuse the bot's
nsec.

Then, with the chosen key in hand:

```bash
cd site && npm install

NSITE_PUBLISH=yes \
NOSTR_PRIVATE_KEY=nsec1YOUR_DOCS_NPUB_HERE \
NOSTR_RELAYS=wss://nos.lol,wss://relay.primal.net,wss://relay.damus.io \
BLOSSOM_SERVERS=https://nostr.download,https://cdn.satellite.earth \
npm run docs:deploy
```

That single command builds the site and publishes it to Nostr. Afterwards it's
live at `https://nsite.cc/<docs-npub>/` (and any other nsite gateway), and
re-publishing after edits is the same command again — kind 34128 events are
replaced, blobs are content-addressed so unchanged files cost nothing.
