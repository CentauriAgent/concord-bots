# Concord v2 Migration Plan — concord-bots

**Created:** 2026-07-06
**Status:** Awaiting Derek's approval

---

## Current State

**Repo:** `/home/moltbot/projects/concord-bots` (Rust, v1.3.0)
**SDK:** `vector_sdk = "0.3"` — wraps Concord v1 APIs
**Existing features:**
- 8 handler modules (commands, fun, utility, AI bridge, wallet, nostr, moderation, community, git monitor)
- SQLite community engagement (XP, levels, giveaways, reputation)
- Cashu wallet integration
- Git repo monitor (GitHub/GitLab)
- Feature flag system
- Auth manager (owner/authorized/public)
- Rate limiter

**Existing moderation commands (v1 SDK calls):**
- `!kick` → `member.kick()`
- `!ban` → `member.ban()`
- `!unban` → `member.unban()`
- `!grantmod` → `member.grant_admin()`
- `!revokemod` → `member.revoke_admin()`
- `!mods` → `community.roles()`
- `!leave` → `community.leave()`

---

## What Changed in Concord v2

The v2 spec (CORD-01 through CORD-07) introduces fundamental changes to how communities, channels, roles, invites, and removals work. Here's what's new vs v1:

### Breaking Changes

| Area | v1 (current) | v2 (new) |
|------|-------------|----------|
| **Roles** | Binary: Owner/Admin/Member | Granular u64 permission bits, ranked positions, custom roles |
| **Authority** | SDK-level enforcement | Client-validated signed editions on Control Plane |
| **Channels** | Simple channel IDs | Public + Private channels with derived keys per epoch |
| **Invites** | URL-based | URL with encrypted bundle OR direct giftwrap (NIP-59) |
| **Removals** | Single API call | Three tiers: Banlist (instant silence) → Rekey (read-cut) → Refounding (full reset) |
| **Permissions** | Admin = all powers | 10 frozen bits (MANAGE_ROLES, MANAGE_CHANNELS, KICK, BAN, etc.) — no all-powerful bit |
| **Messages** | Plain SDK send/receive | Private Streams (CORD-01): NIP-59 giftwraps with millisecond timestamps |
| **Audio/Video** | None | CORD-07: blind token broker + ciphertext-only SFU |
| **Identity** | community_id = derive from key | community_id = sha256("concord/community" ‖ owner_xonly ‖ owner_salt) — self-certifying |
| **Access** | community_root = same as identity | community_root = separate 32-byte private key, rotatable |

### New Concepts to Implement

1. **Editions** — versioned, chained state mutations on the Control Plane (kind 3308 rumors)
2. **Roster folding** — clients fold edition chains to compute current state (refuse-downgrade)
3. **Permission bits** — u64 bitmask with 10 defined bits (MANAGE_ROLES, MANAGE_CHANNELS, etc.)
4. **Position ranking** — lower = higher authority. Owner = 0. Strict outranking required.
5. **Authority citation (`vac` tag)** — every action cites the Grant it acts under
6. **Banlist** — single replaced document, up to 500 npubs
7. **Epochs** — u64 counter that bumps on Rekey (membership change)
8. **Planes** — Control Plane, Chat Plane, Guestbook Plane (each a Private Stream)
9. **Rekeys & Refoundings** — post-removal secrecy via key rotation
10. **Audio/Video** — voice/video/screenshare in channels (CORD-07)

---

## Migration Strategy

**Key decision: SDK upgrade vs direct protocol implementation.**

The current bot uses `vector_sdk v0.3` which abstracts all protocol details. Two paths:

### Path A: Wait for vector_sdk v2.0 (Recommended)
- Soapbox ships a new SDK version that wraps v2 concepts
- We update API calls (similar to today but richer)
- Minimal protocol code in our bot — SDK handles editions, folding, giftwraps
- **Effort:** Medium (API migration, not protocol implementation)
- **Risk:** Low (SDK is tested, we just adapt)
- **Timeline:** Dependent on Soapbox SDK release

### Path B: Direct protocol implementation
- We implement CORD-01 through CORD-07 ourselves in Rust
- Full control, no SDK dependency
- **Effort:** Very High (months of work — crypto, giftwraps, folding, epoch management)
- **Risk:** High (protocol bugs, divergence from reference)
- **Timeline:** Not practical alongside other work

### Path C: Hybrid — SDK for transport, we add v2 features via raw events
- Keep `vector_sdk` for connection/message handling
- Add raw Nostr event publishing for v2-specific features (roles with permission bits, banlist management, audit log)
- **Effort:** Medium-High
- **Risk:** Medium (dual code paths, SDK may conflict)
- **Timeline:** 2-3 weeks

**My recommendation: Path A** — wait for SDK v2, but prepare everything else now.

---

## Pre-Migration Work (Can Do Now)

Regardless of which path, these prepare the codebase:

### Phase 0: Preparation (1-2 days)

1. **Audit current SDK usage** — catalog every `vector_sdk` call (Community, Member, Channel methods)
2. **Document v1 → v2 API mapping** — what SDK calls map to what protocol actions
3. **Add feature flag for v2 migration** — `[features] v2_migration = false` so we can ship incrementally
4. **Create `concord_v2/` module** — new module for v2-specific types and logic, compiled but inactive

### Phase 1: Role System Overhaul (3-5 days, behind feature flag)

The biggest change. v1 has Owner/Admin binary. v2 has granular permission bits.

**Current `auth.rs`:**
```rust
pub enum AuthLevel { Public, Authorized, Owner }
```

**v2 equivalent:**
```rust
pub struct Permission(u64);
pub const MANAGE_ROLES: u64 = 1 << 0;
pub const MANAGE_CHANNELS: u64 = 1 << 1;
// ... 10 bits

pub struct Role {
    role_id: [u8; 32],
    name: String,
    position: u32,
    permissions: u64,
    scope: RoleScope,
    color: u32,
}

pub struct Grant {
    member: [u8; 32],  // hex pubkey
    role_ids: Vec<[u8; 32]>,
}
```

**Work:**
- Add `concord_v2/types.rs` — Role, Grant, Permission, Position types
- Add `concord_v2/permissions.rs` — permission bit constants, union computation, outranking check
- Add `concord_v2/roster.rs` — roster folding logic (simplified: trust SDK for v1, layer v2 checks on top)
- Update `auth.rs` to support BOTH v1 AuthLevel AND v2 permission bits (feature-flagged)
- Update `moderation_cmds.rs`:
  - `!kick` checks `KICK` bit AND strict outranking
  - `!ban` checks `BAN` bit AND strict outranking
  - `!grantmod` → `!grantrole <npub> <role>` with `MANAGE_ROLES` check
  - `!revokemod` → `!revokerole <npub> <role>` with `MANAGE_ROLES` check
  - New: `!createrole <name> <permissions...>` — create custom role
  - New: `!roles` — list all community roles with positions + permissions
  - New: `!permissions <npub>` — show effective permissions for a member
  - New: `!auditlog [N]` — show last N authority actions (requires VIEW_AUDIT_LOG)

### Phase 2: Moderation Commands Update (2-3 days)

Map existing commands to v2 semantics:

| v1 Command | v2 Command | Change |
|-----------|-----------|--------|
| `!kick <npub>` | `!kick <npub>` | Now checks KICK bit + outranking |
| `!ban <npub>` | `!ban <npub>` | Now checks BAN bit + outranking; triggers Banlist edition |
| `!unban <npub>` | `!unban <npub>` | Removes from Banlist |
| `!grantmod <npub>` | `!grantrole <npub> <role>` | Generalized — admin is just a role |
| `!revokemod <npub>` | `!revokerole <npub> <role>` | Generalized |
| `!mods` | `!roles` | Lists all roles, not just "mods" |
| — | `!createrole` | NEW |
| — | `!deleterole <role>` | NEW |
| — | `!permissions <npub>` | NEW |
| — | `!auditlog [N]` | NEW |
| — | `!rekey <channel>` | NEW — trigger channel rekey (MANAGE_CHANNELS) |
| — | `!refound` | NEW — community-wide refounding (owner only) |

### Phase 3: Channel Management (2-3 days)

v2 distinguishes Public vs Private channels with derived keys.

- `!createchannel <name> [private]` — MANAGE_CHANNELS required
- `!editchannel <channel> <name|topic>` — MANAGE_CHANNELS required
- `!deletechannel <channel>` — MANAGE_CHANNELS required (marks as deleted, history sealed)
- `!channels` — list all channels with type (public/private)

### Phase 4: Invite System (2 days)

v2 invites are richer — encrypted bundles or direct giftwraps.

- `!invite` — generate invite (CREATE_INVITE required)
- `!invites` — list active invites (MANAGE_INVITES or owner)
- `!revokeinvite <code>` — revoke an invite

### Phase 5: Audit & Observability (1-2 days)

The edition chain IS the audit log. Expose it:

- `!auditlog [N]` — last N authority actions (VIEW_AUDIT_LOG)
- `!whobanned <npub>` — trace ban authority
- `!roster` — full roster dump (mods+)
- `!communityinfo` — community metadata, epoch, member count

---

## What's Blocked on SDK v2

These can't be built until `vector_sdk` supports v2 primitives:

1. **Edition publishing** — Control Plane kind 3308 events with `vac` citations
2. **Roster folding** — chain verification, refuse-downgrade logic
3. **Epoch management** — key derivation per epoch, rekey triggers
4. **Private Stream subscription** — NIP-59 giftwrap handling at scale
5. **Banlist operations** — single-replaced-document semantics with re-heal
6. **Rekey/Refounding** — CORD-06 key rotation
7. **Audio/Video** — CORD-07 SFU + token broker
8. **Millisecond timestamps** — `["ms", N]` tag handling

**Action item:** Check with Soapbox team on vector_sdk v2 timeline. Until then, we build everything that CAN layer on top of v1 SDK calls (Phases 0-5 above).

---

## Risk Assessment

| Risk | Likelihood | Impact | Mitigation |
|------|-----------|--------|------------|
| SDK v2 breaks v1 APIs | Medium | High | Feature-flag migration, keep v1 path working |
| Permission bit mismatches | Low | Medium | Test suite comparing v2 bits to spec |
| Roster folding race conditions | Medium | High | Defer to SDK; if doing ourselves, property-based tests |
| Auth regression (bot loses access) | Low | Critical | Integration tests in real community before deploy |
| Community incompatibility | Medium | High | Bot must support BOTH v1 and v2 communities during transition |

---

## Open Questions for Derek

1. **SDK v2 timeline:** Has Soapbox indicated when `vector_sdk` v2 will ship? Do they want our input on the API surface?
2. **Dual-community support:** Should the bot work in both v1 and v2 communities simultaneously (transition period)? Or cut over hard?
3. **Audio/Video (CORD-07):** Priority? The bot could schedule/announce voice events but can't participate as a speaker. Do we want that?
4. **Direct protocol implementation:** Do you want me to implement any CORD primitives directly (Path C), or strictly wait for the SDK (Path A)?
5. **Bot community role:** Should the bot itself hold a specific Role with scoped permissions (e.g., MOD role with KICK + MANAGE_MESSAGES), or always run as Admin/Owner?
6. **Migration testing:** Do you have a test community where we can safely break things during migration?

---

## Proposed Timeline

| Phase | Duration | Dependency |
|-------|----------|------------|
| Phase 0: Preparation | 1-2 days | None |
| Phase 1: Role system | 3-5 days | Phase 0 |
| Phase 2: Moderation cmds | 2-3 days | Phase 1 |
| Phase 3: Channel mgmt | 2-3 days | Phase 1 |
| Phase 4: Invites | 2 days | Phase 1 |
| Phase 5: Audit/observability | 1-2 days | Phase 1 |
| **Total pre-SDK work** | **~2 weeks** | |
| Phase 6: SDK v2 integration | TBD | SDK v2 release |
| Phase 7: Edition/folding/crypto | TBD | SDK v2 release |
| Phase 8: Rekey/Refounding | TBD | SDK v2 release |
| Phase 9: Audio/Video | TBD | SDK v2 release |

---

## Recommendation

**Start Phases 0-5 now** (role system, moderation, channels, invites, audit). These are UI/logic changes that layer on top of whatever the SDK provides, and they're the bulk of user-facing work.

**Wait on SDK v2** for the cryptographic heavy lifting (editions, folding, rekeys, epochs). Building that ourselves is months of work with high bug risk — Soapbox will ship it tested.

**Ship incrementally** behind feature flags so nothing breaks for existing communities.
