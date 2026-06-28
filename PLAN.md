# Concord Bot Template — Feature Roadmap

**Epic:** `clawd-pju` — [View in beads](#)
**Created:** 2026-06-28
**Goal:** Make concord-bots the best bot template for the Nostr/Concord ecosystem.

---

## Why This Matters

Discord has MEE6 (21.5M servers), Dyno (14.6M), Carl-bot — massive bot ecosystems.
Telegram has BotFather, inline keyboards, mini-apps.

**Our advantage:** Nostr makes bots trivial. No tokens, no approval process, no rate limits,
no platform risk. Bitcoin/Lightning/Cashu are built into the protocol.

**Our challenge:** We need feature parity on utilities (polls, reminders, etc.) PLUS
Nostr-native features (zaps, tips, cross-posting) that create our moat.

---

## Release Plan

### v0.3.0 — Table Stakes (Tasks 1-4)

**Goal:** Every utility command people expect from a bot template.

| Task | Bead | Status |
|------|------|--------|
| Tier 1 utility commands | `clawd-pju.1` | open |
| Welcome messages | `clawd-pju.2` | open |
| Feature flag config | `clawd-pju.3` | open |
| Tag v0.3.0 release | `clawd-pju.4` | blocked by 1, 2, 3 |

**Commands to add:**
- `!remind <time> <msg>` — Persisted reminders (SQLite)
- `!poll <Q> \| A \| B \| C` — Reaction-based polls
- `!translate <lang> <text>` — Free translation API
- `!define <word>` — Dictionary definitions
- `!quote` — Random inspirational quote
- `!joke` — Dad jokes (icanhazdadjoke)
- `!fact` — Random fun fact
- `!meme` — Random meme from Reddit
- `!shorten <url>` — URL shortener

**Feature flag config:**
```toml
[features]
utility = true
fun = true
community = true
nostr = true
ai = false
moderation = true
```

### v0.4.0 — Nostr-Native Moat (Tasks 5-7)

**Goal:** Features impossible on Discord/Telegram. This is why people choose us.

| Task | Bead | Status |
|------|------|--------|
| Nostr-native commands | `clawd-pju.5` | open |
| Cross-posting | `clawd-pju.6` | open |
| Tag v0.4.0 release | `clawd-pju.7` | blocked by 5, 6, 4 |

**Commands to add:**
- `!zap <npub> <sats>` — Send a Lightning zap from bot wallet
- `!tip <@user> <sats>` — Tip community members
- `!balance` — Check bot wallet balance
- `!nostr <npub>` — Look up Nostr profile
- `!nip05 <user>` — Verify NIP-05 identity
- `!follow <npub>` — Follow a Nostr profile

**Wallet config:**
```toml
[features.zaps]
wallet = "cashu"  # or "lnbits", "alby"
mint_url = "https://mint.minibits.cash"
```

### v0.5.0 — AI + Community (Tasks 8-9)

**Goal:** Modern AI features and engagement mechanics.

| Task | Bead | Status |
|------|------|--------|
| AI commands | `clawd-pju.8` | blocked by 4 |
| Community features | `clawd-pju.9` | blocked by 4 |

**AI commands:**
- `!ask <question>` — AI-powered Q&A
- `!summarize [hours]` — Chat summary
- `!sentiment` — Channel mood analysis
- `!image <prompt>` — Image generation (optional)

**Community commands:**
- `!leaderboard` — Top active users
- `!level` / `!rank` — XP system
- `!profile` — User card
- `!giveaway <duration> <prize>` — Reaction giveaways

### v1.0.0 — Production Ready (Tasks 10-12)

**Goal:** Everything needed for production deployment.

| Task | Bead | Status |
|------|------|--------|
| Moderation tools | `clawd-pju.10` | blocked by 7 |
| Docker + deploy | `clawd-pju.11` | blocked by 7 |
| Documentation site | `clawd-pju.12` | blocked by 10, 11 |

**Moderation (Concord-native, uses SDK role system):**

Phase 1 — Core commands:
- `!kick <npub>` — Authorized+. Cooperative kick (can rejoin). Uses `member.kick()`
- `!ban <npub>` — Owner only. Terminal ban. Uses `member.ban()`
- `!unban <npub>` — Owner only. Lifts ban.
- `!warn <npub> <reason>` — Authorized+. Logs warning (SQLite, no protocol action)
- `!warnings <npub>` — Authorized+. Show warning history
- `!mods` — Public. Lists current mods/admins from `community.roles()`
- `!grantmod <npub>` — Owner only. Uses `member.grant_admin()` (no underscore!)
- `!revokemod <npub>` — Owner only. Uses `member.revoke_admin()`

Phase 2 — Auto-mod (config-driven):
- Word filter (auto-kick on blacklisted words)
- Link blocking (warn then kick)
- Spam protection (extend rate limiter to auto-kick repeat offenders)
- All auto-mod respects Concord auth (bot needs KICK permission in community)

Two-layer auth:
1. Bot auth: Is user allowed to use !ban? (Owner/Authorized)
2. Community auth: Does bot have BAN permission? (Concord role system)

Protocol limitations:
- Cannot delete others' messages (only own)
- No timeout/mute (kick or ban only)
- No slow-mode (per-bot rate limit only)

**Infrastructure:**
- Multi-stage Dockerfile
- docker-compose.yml
- `deploy/install.sh` one-command installer
- Documentation site with examples

---

## Dependency Graph

```
v0.3 ────┬── clawd-pju.1 (utility commands)
         ├── clawd-pju.2 (welcome messages)  
         └── clawd-pju.3 (feature flags)
              │
              ▼
         clawd-pju.4 (tag v0.3.0)
              │
              ├──────────────┬──────────────┐
              ▼              ▼              ▼
v0.4 ── clawd-pju.5    clawd-pju.8    clawd-pju.9
   │    (nostr cmds)    (AI)           (community)
   │    clawd-pju.6
   │    (cross-post)
   │         │
   │         ▼
   └──── clawd-pju.7 (tag v0.4.0)
              │
              ├──────────────┐
              ▼              ▼
v1.0 ── clawd-pju.10   clawd-pju.11
       (moderation)    (docker)
              │              │
              └──────┬───────┘
                     ▼
              clawd-pju.12 (docs)
```

---

## Agent Team Assignments

**v0.3.0 sprint:**
- 📐 Architect: Design feature flag system + reminder persistence
- 💻 Coder: Implement all Tier 1 commands + welcome handler
- 🧪 Tester: Verify each command works, edge cases
- 🔍 Reviewer: Code quality + pattern consistency
- 📝 Docs: Update README with new commands

**Rules for all agents:**
1. NEVER register both `on_event` and `on_message` — pick one (see commit c51e4b3)
2. Always build and test before committing
3. Follow existing patterns in `src/handlers/`
4. Gate new commands behind feature flags
5. Update `!help` text when adding commands
