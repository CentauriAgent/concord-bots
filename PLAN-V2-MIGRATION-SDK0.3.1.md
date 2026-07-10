# concord-bots v2.0 — Concord v2 Migration & SDK 0.3.1 Upgrade Plan

**Created:** 2026-07-10
**Author:** Centauri
**Status:** Awaiting Derek's approval
**Target:** concord-bots v2.0.0 on vector_sdk 0.3.1 + vector-core 0.2.0

---

## Executive Summary

vector_sdk 0.3.1 (published today) ships full **Concord v2 protocol support** through the vector-core 0.2.0 engine rewrite. The SDK abstracts all v2 cryptographic complexity (editions, roster folding, giftwraps, rekeys, epochs) behind clean ergonomic methods.

**Our v1 migration plan was wrong** — it said "wait for SDK v2." v2 is here now. We can build everything.

This plan upgrades concord-bots from v1.3.0 → v2.0.0, adopting:
- Concord v2 communities (create, join, manage)
- Permission-based role system (replacing binary Owner/Admin/Member)
- v2 moderation (outranking, rekeys on ban)
- New SDK features (delete, edit, attachments, profiles)
- Cleanup of manual hacks we no longer need

**Estimated effort:** 7-10 days of focused work, phased so we can ship incrementally.

---

## Current State Assessment

### What We Have (v1.3.0)

| Component | Lines | Status |
|-----------|-------|--------|
| Core framework (bot.rs, config.rs, auth.rs, main.rs) | ~1,600 | Stable, well-structured |
| Handler modules (9 files) | ~4,100 | Working but v1-only |
| Community engagement DB (SQLite: XP, levels, giveaways, rep) | 823 | Unaffected by v2 |
| Git monitor (GitHub/GitLab subscriptions) | 1,628 | Unaffected by v2 |
| Cashu wallet integration | 239 | Unaffected by v2 |
| Utilities (http, nip98, npub_cash, scheduler, vector_client) | 396 | Some dead code |
| **Total** | **~10,900** | |

### Compile Status Against 0.3.1

✅ **Zero errors.** 108/108 tests pass. The version bump from 0.3.0 → 0.3.1 is fully backward compatible — our existing v1 code keeps working unchanged. This means we can upgrade in place, not rewrite.

---

## What Concord v2 Gives Us Through The SDK

### New API Methods (vs our current v1 calls)

| Area | v1 (what we do now) | v2 (what the SDK now provides) |
|------|---------------------|-------------------------------|
| **Create community** | Not supported | `bot.core().create_community_v2("name")` |
| **Join community** | Not supported | `bot.core().join_community(invite_link)` |
| **Community info** | `community.id()` | `community.id()`, `community.capabilities()`, `community.roles()`, `community.members().await`, `community.update(name, desc)`, `community.dissolve()` |
| **Invites** | Not supported | `community.create_invite()` → shareable link, `community.invite(npub)` → direct giftwrap |
| **Member actions** | `member.kick/ban/unban()` | Same methods, now v2-aware: kick requires KICK + outranking, ban triggers rekey |
| **Roles** | `member.grant_admin() / revoke_admin()` | Same methods + `community.roles()` returns full role/permission map |
| **Message ops** | `channel.send()`, `msg.reply()` | All of v1 + `channel.edit(msg_id, text)`, `channel.delete(msg_id)` |
| **Reactions** | `channel.react(msg_id, emoji)` | Same + custom image reactions via `channel.react_custom()` |
| **Files** | `channel.send_file(path)` | Same + `bot.save_attachment(att, path)`, `bot.download_attachment(att)` |
| **Profiles** | Manual kind 0 hack (~70 lines) | `bot.core().update_bot_profile(name, avatar, banner, about)`, `bot.fetch_profile(npub)`, `bot.upload_image(path)` |
| **Events** | `on_message`, `on_event` | Same + `BotEvent::MessageUpdate`, `BotEvent::Delete`, `BotEvent::Removed` |
| **Community list** | `bot.communities()` | `bot.communities()` + `bot.core().list_communities()` (returns version, channels, owner flag) |
| **Protocol detection** | None | `community` summary includes `"version": 2` field |

### What The SDK Handles (That We Don't Have To Build)

Per our old PLAN-V2-MIGRATION.md, these were "blocked on SDK v2":

- ✅ ~~Edition publishing~~ — vector-core handles Control Plane kind 3308 events
- ✅ ~~Roster folding~~ — chain verification + refuse-downgrade built in
- ✅ ~~Permission bits~~ — enforced by SDK; `capabilities()` returns them
- ✅ ~~Position ranking~~ — outranking checks happen in `member.kick()` / `member.ban()`
- ✅ ~~Epoch management~~ — key derivation per epoch handled by core
- ✅ ~~Private Stream subscription~~ — NIP-59 giftwrap at scale, handled
- ✅ ~~Banlist operations~~ — single-replaced-document semantics in core
- ✅ ~~Rekey/Refounding~~ — ban in private community triggers read-cut rekey automatically
- ✅ ~~Millisecond timestamps~~ — `["ms", N]` tag handling internal

**We build zero crypto. We build zero protocol code. We just call methods.**

---

## Migration Architecture

### Design Principles

1. **In-place upgrade** — No rewrite. Same repo, same structure, evolved code.
2. **v1 + v2 coexistence** — Bot works in both v1 and v2 communities simultaneously (SDK handles this transparently)
3. **Feature flags** — New v2 features gated behind `[features] v2 = true` in bot.toml
4. **Clean removal** — Dead code and manual hacks get cleaned up
5. **Backward compatible config** — Existing bot.toml files keep working

### Module Changes

```
src/
├── bot.rs              ← UPDATE: Clean up profile hacks, add v2 community bootstrap
├── config.rs           ← UPDATE: Add [v2] config section
├── auth.rs             ← UPDATE: Layer v2 permission awareness on top of AuthLevel
├── handlers/
│   ├── mod.rs          ← UPDATE: Handle new BotEvent variants
│   ├── commands.rs     ← UPDATE: Add !community, !invite, !leave commands
│   ├── moderation_cmds.rs ← REWRITE: v2 permission-aware moderation
│   ├── community_cmds.rs ← UPDATE: Add v2 community management
│   ├── fun.rs          ← UNCHANGED
│   ├── utility.rs      ← UPDATE: Add !delete, !edit, !members
│   ├── wallet_cmds.rs  ← UNCHANGED
│   ├── nostr_cmds.rs   ← UNCHANGED
│   ├── git_cmds.rs     ← UNCHANGED
│   ├── ai_bridge.rs    ← UPDATE: Attachment support for vision LLMs
│   └── scheduled.rs    ← UNCHANGED
├── lib/
│   ├── vector_client.rs ← CLEAN UP: Remove dead code
│   ├── http.rs         ← CLEAN UP: Remove dead code
│   ├── nip98.rs        ← UNCHANGED
│   ├── npub_cash.rs    ← UNCHANGED
│   ├── scheduler.rs    ← CLEAN UP: Remove dead code
│   └── mod.rs          ← UNCHANGED
├── community/mod.rs    ← UNCHANGED (our engagement system, not SDK community)
├── git_monitor/        ← UNCHANGED
├── wallet/mod.rs       ← UNCHANGED
├── rate_limiter.rs     ← UNCHANGED
└── main.rs             ← UNCHANGED
```

---

## Phased Plan

### Phase 0: Lock In The Bump (DONE ✅)

- [x] Update `Cargo.toml`: `vector_sdk = "0.3.1"`
- [x] `cargo update -p vector_sdk` (pulls vector-core 0.2.0)
- [x] Verify: `cargo check` — 0 errors
- [x] Verify: `cargo test` — 108/108 pass
- [ ] Commit to branch `sdk-0.3.1-bump`
- [ ] Push

**Risk:** Zero. No API changes.

---

### Phase 1: Cleanup Pass (1 day)

Remove dead code and manual hacks that the SDK now handles natively.

**1a. Remove manual profile publishing** (~70 lines → ~10 lines)
- `src/bot.rs` lines ~334-403: Replace manual Nostr kind 0 event construction with `bot.core().update_bot_profile()`
- Remove `vector_sdk::vector_core::state::nostr_client()` hack (line 388)
- Test: Bot profile still shows name, picture, about, lud16

**1b. Clean up dead utility code**
- `src/lib/vector_client.rs` — 6 unused functions. Either remove the module or remove the dead fns. Our handlers call SDK methods directly; the wrapper was never adopted.
- `src/lib/scheduler.rs` — `every()` and `after()` unused. Remove.
- `src/lib/http.rs` — `fetch_json_with_auth()`, `post_json()`, `fetch_text()` unused. Keep `fetch_json()` if used, remove rest.

**1c. Fix pre-existing warnings**
- Remove unused imports (`Database` in community_cmds.rs)
- Remove trailing semicolons (nip98.rs:35, wallet/mod.rs:213)
- Prefix unused params with `_` (moderation_cmds.rs:70)
- Remove `mut` where not needed (community/mod.rs — 7 locations)

**Deliverable:** Cleaner codebase, zero warnings, same behavior.

---

### Phase 2: v2 Community Management (2-3 days)

Add the ability to create, join, and manage v2 communities.

**2a. Config section**
```toml
# bot.toml
[v2]
# Auto-create a community on first run if none exists
auto_create = false
# Default community name for auto-create
community_name = "My Bot Community"
# Invite links to join on startup (persist membership across restarts)
join_on_start = [
    # "https://vectorapp.io/invite#..."
]
```

**2b. New commands** in `src/handlers/commands.rs`:

| Command | Auth | What it does |
|---------|------|-------------|
| `!community create <name>` | Owner | `bot.core().create_community_v2(name)` — creates a v2 community |
| `!community info` | Public | Shows community ID, member count, channels, protocol version |
| `!community leave` | Owner | `community.leave()` — leaves current community |
| `!community dissolve` | Owner | `community.dissolve()` — irreversibly dissolves (owner only) |
| `!invite` | Authorized+ | `community.create_invite()` — generates shareable invite link |
| `!invite <npub>` | Authorized+ | `community.invite(npub)` — direct giftwrap invite |
| `!join <invite_link>` | Owner | `bot.core().join_community(link)` — joins a community from link |
| `!members` | Public | `community.members().await` — lists community members |

**2c. Bot startup v2 bootstrap** in `src/bot.rs`:
- After bot connects, check `[v2]` config
- If `auto_create = true` and no communities: create one
- If `join_on_start` has links: join each
- Log community IDs and protocol versions

**2d. Enhanced `!whoami` and `!info`**:
- `!info` shows protocol version (v1 vs v2), community name, capabilities, roles
- `!channels` lists channels (from `bot.core().list_communities()`)

**Deliverable:** Bot can create, join, and manage v2 communities.

---

### Phase 3: v2-Aware Moderation (2-3 days)

Upgrade moderation to use v2's permission system.

**3a. Permission-aware auth layer** in `src/auth.rs`:

Keep our existing `AuthLevel` (Public/Authorized/Owner) as our bot-level access control. Layer v2 community permissions on top:

```rust
/// Check if user has a v2 community capability
pub async fn has_capability(msg: &IncomingMessage, capability: &str) -> bool {
    match msg.community() {
        Some(community) => {
            match community.capabilities() {
                Ok(caps) => /* parse caps JSON for this bot's npub */,
                Err(_) => false,
            }
        }
        None => false, // DMs don't have community capabilities
    }
}
```

**3b. Updated moderation commands** in `src/handlers/moderation_cmds.rs`:

| Command | v1 Behavior | v2 Behavior |
|---------|------------|------------|
| `!kick <npub>` | SDK call, no permission check | SDK enforces KICK + outranking; we catch error and report |
| `!ban <npub>` | SDK call | SDK enforces BAN; triggers rekey in private communities |
| `!unban <npub>` | SDK call | Same (removes from banlist) |
| `!grantmod <npub>` | `grant_admin()` | Same (requires MANAGE_ROLES; SDK checks outranking) |
| `!revokemod <npub>` | `revoke_admin()` | Same |
| `!mods` | Lists admins | `community.roles()` — full role/permission dump |
| `!caps` | NEW | Shows this bot's capabilities in current community |
| `!roles` | NEW | Lists all community roles with permissions |

**3c. Error handling for v2 moderation failures:**

When `member.kick()` fails because the bot lacks KICK permission or doesn't outrank the target, we need to surface that clearly:

```rust
match member.kick().await {
    Ok(()) => msg.reply("✅ Kicked.").await?,
    Err(e) => {
        let err = format!("{:?}", e);
        if err.contains("permission") || err.contains("outrank") {
            msg.reply("⚠️ I don't have permission to kick that member (insufficient role or rank).").await?;
        } else {
            msg.reply(&format!("⚠️ Kick failed: {}", err)).await?;
        }
    }
}
```

**3d. Auto-moderation upgrade** in handler logic:

Use `msg.is_group` to distinguish community vs DM messages. Auto-mod only applies in communities. Check `member.is_admin()` before acting (already done in our code, but now it reflects v2 roles).

**Deliverable:** Full v2 moderation with proper permission handling.

---

### Phase 4: Message Operations (1-2 days)

Adopt new SDK message capabilities.

**4a. New message commands** in `src/handlers/utility.rs`:

| Command | Auth | What it does |
|---------|------|-------------|
| `!delete <msg_id>` | Authorized+ | Deletes bot's own message (or MANAGE_MESSAGES in v2 community) |
| `!edit <msg_id> <text>` | Authorized+ | Edits bot's own message |
| `!savefile` | Authorized+ | Saves the replied-to attachment to `data/downloads/` |

**4b. New BotEvent handlers** in `src/handlers/mod.rs`:

```rust
// In on_event handler:
BotEvent::MessageUpdate { message, .. } => {
    tracing::debug!("Message {} updated: {} reactions", message.id, message.reactions.len());
    // Could trigger engagement tracking (reaction = +XP?)
}

BotEvent::Delete { message_id, .. } => {
    tracing::info!("Message {} deleted", message_id);
}

BotEvent::Removed { community_id } => {
    tracing::warn!("Bot removed from community {}", community_id);
    // Notify owner via DM or Signal
    if let Some(owner) = &ctx.auth.as_ref().and_then(|a| a.owner()) {
        let _ = bot.dm(owner).send(&format!(
            "⚠️ I was removed from community {}", community_id
        )).await;
    }
}
```

**4c. Reaction tracking** (community engagement integration):

If `BotEvent::MessageUpdate` shows a new reaction on a message, award XP to the reactor via our existing SQLite engagement system. This bridges v2 SDK events with our custom gamification.

**Deliverable:** Full message lifecycle support + engagement integration.

---

### Phase 5: Attachment & File Handling (2 days)

Enable the bot to receive, process, and respond to files.

**5a. Attachment detection in `src/handlers/mod.rs`:**

```rust
pub async fn on_message(ctx: &BotContext, msg: &IncomingMessage) -> Result<()> {
    if msg.is_file {
        return handle_file(ctx, msg).await;
    }
    // ... existing command dispatch
}

async fn handle_file(ctx: &BotContext, msg: &IncomingMessage) -> Result<()> {
    for att in &msg.message.attachments {
        tracing::info!("Attachment: {} ({} bytes, .{})", att.name, att.size, att.extension);
        // Auto-save to data/downloads/
        // If image + AI bridge enabled: send to vision LLM
    }
    Ok(())
}
```

**5b. AI Bridge vision support** in `src/handlers/ai_bridge.rs`:

If the bot receives an image in a community where AI bridge is enabled:
1. Download attachment via `bot.save_attachment(att, temp_path)`
2. Convert to base64
3. Send to vision-capable LLM endpoint (GPT-4o, Claude, GLM-5V)
4. Reply with description/analysis

**5c. `!savefile` command:**

Manually trigger save of an attachment to disk for archival.

**Deliverable:** Bot can receive and process files, with optional vision AI.

---

### Phase 6: Profile Management Cleanup (1 day)

Fully adopt SDK profile methods and remove all manual Nostr event code.

**6a. Replace manual kind 0 publishing:**

Current (70+ lines of manual event construction):
```rust
// bot.rs:334-403 — manual kind 0 with lud16
```

New (3 lines):
```rust
bot.core().update_bot_profile(&name, &avatar_url, &banner_url, &about).await;
// For lud16: if update_bot_profile doesn't include it, use core facade:
// bot.core().set_lud16("bot@voltage.cloud").await; // or whatever method exists
```

**6b. Avatar upload:**

Replace manual Blossom/NIP-96 upload with:
```rust
let avatar_url = bot.core().upload_public_image("config/avatar.png").await?;
```

**6c. Investigate lud16 support:**

Test whether `update_bot_profile` includes lud16. If not, check if there's a core method for it. If still missing, keep a minimal manual kind 0 fallback (should be <20 lines, not 70).

**Deliverable:** Clean profile management, minimal manual protocol code.

---

### Phase 7: Testing & Documentation (1-2 days)

**7a. Update examples:**

- Update `examples/echo-bot/` to demonstrate v2 features
- Add `examples/v2-moderation/` — permission-aware mod bot
- Add `examples/v2-community-manager/` — full community lifecycle

**7b. Update documentation:**

- `README.md` — Add v2 features section, update quick start
- `AGENTS.md` — Update AI agent instructions for v2 commands
- `config/bot.toml.example` — Add `[v2]` section

**7c. Integration test plan:**

1. Deploy bot to a test v2 community
2. Verify: `!community info` shows protocol=v2
3. Verify: `!kick`/`!ban` enforce outranking
4. Verify: Bot reconnects and catches up after restart
5. Verify: Invites work (both link and direct)
6. Verify: File send/receive works
7. Verify: Profile shows correctly with avatar

**7d. Version bump:**

- `Cargo.toml`: `version = "2.0.0"`
- Update CHANGELOG
- Tag release `v2.0.0`

**Deliverable:** Shippable v2.0.0.

---

## What's NOT In Scope

These are deliberately excluded from this plan:

| Feature | Why Not Now |
|---------|-----------|
| **Audio/Video (CORD-07)** | SDK doesn't expose voice/video methods yet. Future SDK release. |
| **Custom roles creation** | SDK exposes `roles()` read-only. Creating custom permission roles may need core facade methods. Investigate in Phase 3. |
| **MCP Agent Server integration** | Vector v0.4.0 ships an MCP server for AI agents. Separate investigation — could replace concord-bots for personal use, but concord-bots still valuable for community bots. |
| **Tor integration** | Add `features = ["tor"]` to Cargo.toml when we need it. No code changes. |
| **Multiple accounts** | SDK supports it via separate processes. Not needed for our bot template. |
| **Remote signer (NIP-46)** | Bot uses nsec directly. Bunker support can be added later if needed. |

---

## Risk Assessment

| Risk | Likelihood | Impact | Mitigation |
|------|-----------|--------|------------|
| v2 community creation has unexpected behavior | Low | Medium | Test in isolated community first |
| Profile lud16 not exposed by SDK | Medium | Low | Keep minimal manual kind 0 fallback |
| Permission errors confuse users | Medium | Medium | Clear error messages with actionable text |
| v1 communities break | Very Low | High | SDK is backward compatible; v1 path unchanged |
| vector-core 0.2.0 runtime behavior differs | Low | Medium | Tests pass; gradual rollout |
| Concurrent v1+v2 community handling | Low | Medium | SDK abstracts protocol; we treat all uniformly |

**Overall risk: Low.** This is additive — v1 code keeps working, v2 features are net-new.

---

## Timeline

| Phase | Duration | Deliverable |
|-------|----------|-------------|
| Phase 0: SDK bump | ✅ Done | Compiles, tests pass |
| Phase 1: Cleanup | 1 day | Cleaner codebase |
| Phase 2: Community mgmt | 2-3 days | Create/join/manage v2 communities |
| Phase 3: Moderation | 2-3 days | v2 permission-aware mod commands |
| Phase 4: Message ops | 1-2 days | Delete, edit, event handlers |
| Phase 5: Attachments | 2 days | File handling + vision AI |
| Phase 6: Profiles | 1 day | Clean profile management |
| Phase 7: Testing & docs | 1-2 days | v2.0.0 release |
| **Total** | **10-14 days** | **concord-bots v2.0.0** |

Phases can overlap — 2+3 in parallel with 4+5+6.

---

## Open Questions

1. **lud16 support:** Does `update_bot_profile()` include Lightning address? Need to test. If not, is there a core method?
2. **Custom roles:** Can we create roles with specific permission bits via the SDK, or only read existing roles?
3. **Community channel ops:** Does the SDK expose channel creation/deletion for v2, or only the community-level methods?
4. **MCP coexistence:** Should concord-bots v2.0 also expose an MCP interface, or keep it as a standalone binary?
5. **Test community:** Do you have a v2 community we can test in? If not, should I create one?

---

## Comparison: Old Plan vs New Plan

| Aspect | Old Plan (PLAN-V2-MIGRATION.md) | New Plan (This Document) |
|--------|-------------------------------|-------------------------|
| **SDK v2 availability** | "Wait for SDK v2.0" | ✅ Already here (0.3.1) |
| **Strategy** | Path A: Wait, Path B: Direct impl, Path C: Hybrid | In-place upgrade using SDK |
| **Protocol implementation** | Potentially months of crypto work | Zero — SDK handles all crypto |
| **Timeline** | 2 weeks pre-work + TBD post-SDK | 10-14 days total |
| **Risk** | Medium-High (dual code paths) | Low (additive, backward compatible) |
| **Phases** | 9 phases, most blocked on SDK | 8 phases, none blocked |

The old plan was correct given the information we had — the SDK wasn't out yet. Now it is. Time to build.

---

## Recommendation

**Approve this plan and I'll start Phase 1 immediately.** The SDK bump is already locked in (Cargo.toml updated, tests passing). Phase 1 is zero-risk cleanup. Phases 2-3 are where the real v2 value lands.

Want me to start?
