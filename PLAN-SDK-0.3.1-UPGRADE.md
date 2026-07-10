# vector_sdk 0.3.1 Upgrade Plan

**Created:** 2026-07-10
**Status:** Ready to execute
**Current:** vector_sdk 0.3.0 + vector-core 0.1.0
**Target:** vector_sdk 0.3.1 + vector-core 0.2.0

---

## Compile Status: ✅ CLEAN

- `cargo check` — 0 errors, 37 warnings (all pre-existing dead code)
- `cargo test` — 108 passed, 0 failed
- **No breaking API changes** between 0.3.0 → 0.3.1

The bump is already applied to Cargo.toml and Cargo.lock is updated.

---

## What 0.3.1 Gives Us (vector-core rewrite)

The SDK is now a thin layer over the same `vector-core` engine that powers Vector v0.4.0 desktop/mobile (440+ tests). Key improvements:

1. **Engine reliability** — Reconnect + catch-up on missed messages (we get this for free)
2. **Message delete/edit** — `channel.delete(msg_id)`, `channel.edit(msg_id, text)` — NEW capabilities we didn't have
3. **New BotEvent variants** — `MessageUpdate`, `Delete`, `Removed` (bot kicked/banned) — we can now react to these
4. **Attachment download** — `bot.download_attachment()` / `bot.save_attachment()` — NEW
5. **Block & nickname** — `bot.block(npub)`, `bot.nickname(npub, name)` — NEW
6. **Profile management** — `bot.fetch_profile()`, `bot.update_profile()`, `bot.upload_image()` — improves on what we hack together manually in bot.rs
7. **Tor feature flag** — `vector_sdk = { version = "0.3.1", features = ["tor"] }` — optional, for censorship-resistance bots
8. **Auto-identity** — Creates/persists nsec on first run if none supplied (we already handle this manually, but the SDK does it natively now)

---

## Upgrade Plan

### Phase 1: Lock In The Bump (5 minutes) ✅ DONE

- [x] Update `Cargo.toml`: `vector_sdk = "0.3.1"`
- [x] `cargo update -p vector_sdk`
- [x] Verify compile (0 errors)
- [x] Verify tests (108/108 pass)
- [ ] Commit and push

### Phase 2: Adopt New Event Handlers (1-2 hours)

The SDK now delivers `MessageUpdate`, `Delete`, and `Removed` events via `on_event`. Our current `on_event` handler in `handlers/mod.rs` and `handlers/ai_bridge.rs` ignore these.

**Changes:**
- `src/handlers/mod.rs` — Add match arms for `BotEvent::MessageUpdate`, `BotEvent::Delete`, `BotEvent::Removed`
- `src/handlers/mod.rs` — Log reaction/edit events (useful for engagement tracking)
- `src/handlers/mod.rs` — On `Removed`, send owner notification via Signal/DM

**Priority:** Low — logging only, no user-facing impact

### Phase 3: Replace Manual Profile Hacks (2-3 hours)

Our `bot.rs` has ~70 lines of manual kind 0 event publishing because the old SDK's `update_profile()` didn't accept `lud16`. The new SDK has improved profile methods.

**Changes:**
- `src/bot.rs` lines ~334-403 — Replace manual Nostr event publishing with `bot.update_profile()` if it now supports lud16
- Test that Lightning address still appears on the bot's profile
- Remove `vector_sdk::vector_core::state::nostr_client()` hack on line 388

**Priority:** Medium — code cleanup, reduces maintenance burden

### Phase 4: Add Delete/Edit Commands (2-3 hours)

New SDK capabilities we can expose as bot commands:

**Changes:**
- `src/handlers/moderation_cmds.rs` — Add `!delete <msg_id>` (MANAGE_MESSAGES permission)
- `src/handlers/utility.rs` — Add `!edit <msg_id> <new text>` for bot's own messages
- Update `!help` output

**Priority:** Medium — genuinely useful for community management

### Phase 5: Attachment Handling (3-4 hours)

Bots can now download files sent to them. Enables image responses, file processing, etc.

**Changes:**
- `src/handlers/mod.rs` — Detect attachments on incoming messages
- `src/handlers/ai_bridge.rs` — Pass images to vision-capable LLM endpoints
- `src/handlers/utility.rs` — `!savefile` command to persist attachments

**Priority:** Low-Medium — enables future features (vision, file processing)

### Phase 6: MCP Agent Server Investigation (research, 1-2 hours)

Vector v0.4.0 ships a **Model Context Protocol server** (`vector-agent`) with 21 tools for AI agents. This could let ME (Centauri) drive Vector directly without the concord-bots Rust intermediary.

**Questions to investigate:**
- Can OpenClaw connect to the Vector MCP server?
- Would this replace concord-bots entirely, or complement it?
- Does it support community bots or just personal use?

**Priority:** Research only — no code changes yet

---

## What's NOT Changing

- **Concord v2 protocol features** (permission bits, editions, roster folding, rekeys, epochs, audio/video) — still not in the SDK. Our `PLAN-V2-MIGRATION.md` Path A prediction was correct: wait for SDK v2.0.
- **All existing commands** — `!ping`, `!kick`, `!ban`, `!help`, wallet commands, git monitor, etc. — zero changes needed
- **Bot configuration** — `bot.toml` format unchanged
- **Auth system** — Our AuthLevel enum is separate from SDK roles
- **Community engagement DB** — SQLite XP/levels/giveaways untouched

---

## Risk Assessment

| Risk | Likelihood | Impact | Mitigation |
|------|-----------|--------|------------|
| Runtime behavior change in vector-core 0.2.0 | Low | Medium | Tests pass; deploy to test community first |
| Profile publishing regression | Low | Low | Manual kind 0 fallback already exists |
| Reconnect behavior differs | Very Low | Low | SDK auto-reconnect is strictly better |
| Tor feature conflicts with our networking | None | N/A | Not enabling Tor feature flag |

**Overall risk: Very Low.** This is a patch-level bump with no breaking changes.

---

## Deployment Steps

1. **Commit the Cargo.toml/Cargo.lock change** to a branch
2. **Build release binary**: `cargo build --release`
3. **Test in a staging community** for 24h (if we have one)
4. **Deploy**: Update systemd service, restart bot
5. **Monitor logs** for any new warnings from vector-core 0.2.0

---

## Recommendation

**Ship Phase 1 immediately** (just the version bump). It's zero-risk and gets us on the new engine.

Then phase the feature adoptions (Phases 2-5) as separate PRs over the next week or two.

Phase 6 (MCP investigation) is worth exploring — if the Vector MCP server lets me drive communities directly from OpenClaw, concord-bots might evolve into a thinner layer or be replaced entirely for our use case. But that's a bigger conversation.
