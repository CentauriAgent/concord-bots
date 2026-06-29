# Plan — Repo Monitor (`!git` commands + background polling)

**Epic:** concord-bots v1.1.0 (post-v1.0.0 feature)
**Owner:** Derek
**Status:** DRAFT — pending Derek approval

---

## Goal

Let community members subscribe a channel to Git repos (GitHub + GitLab) and have the bot auto-announce new commits and releases. Managed entirely via chat commands.

```
!git add <url-or-owner/repo>
!git list
!git remove <repo-or-id>
!git info <repo>
```

---

## Why This Feature

- Community channels want to see "what just shipped" without leaving chat
- Nostr/Soapbox projects live across GitHub (Shakespeare, Foxhole) AND GitLab (Agora, MKStack) — both are first-class
- Current alternatives (raw RSS, IFTTT) are clunky and require setup per repo. A bot command is instant.
- Fits the concord-bots v1.0+ mission: utilities every community expects, with Nostr-native flavor

---

## Commands

All commands live under the `!git` namespace (avoids collision with `!add`/`!remove`/`!list` which are auth-only).

| Command | Auth | Behavior |
|---|---|---|
| `!git add <repo>` | Authorized+ | Subscribe this channel to a repo |
| `!git list` | Public | List this channel's subscriptions (all) |
| `!git remove <repo-or-id>` | Authorized+ | Unsubscribe this channel from a repo |
| `!git info <repo>` | Public | Show latest commit + latest release on demand |
| `!git poll` | Owner | Force a poll right now (debug) |

### Input parsing — flexible

All of these should work:
- `!git add https://github.com/owner/repo`
- `!git add github.com/owner/repo`
- `!git add owner/repo` (GitHub implied)
- `!git add gitlab owner/repo` (explicit host)
- `!git add https://gitlab.com/soapbox-pub/agora`

Auto-detect logic:
1. If arg contains `github.com/` → GitHub
2. If arg contains `gitlab.com/` (or any `gitlab.*` host) → GitLab
3. If prefixed with `github ` or `gitlab ` → that host, rest is `owner/repo`
4. Bare `owner/repo` → default to GitHub (most common case)
5. Else: error with usage hint

### Auth rationale

- **Authorized+ for add/remove:** Lurkers shouldn't be able to spam a channel with 50 repo subscriptions. Must be on the authorized list.
- **Public for list/info:** Read-only, no spam risk. Matches pattern of `!nostr`, `!nip05`.
- **Owner for force-poll:** Debug lever, shouldn't be triggered by mods.

---

## Storage — SQLite (`data/repos.sqlite`)

Following the existing community.sqlite pattern. Separate file for separation of concerns.

### Tables

```sql
CREATE TABLE subscriptions (
    id            INTEGER PRIMARY KEY AUTOINCREMENT,
    channel_id    TEXT NOT NULL,          -- Concord channel ID
    host          TEXT NOT NULL,          -- 'github' | 'gitlab'
    owner         TEXT NOT NULL,          -- e.g. 'soapbox-pub'
    repo          TEXT NOT NULL,          -- e.g. 'agora'
    full_slug     TEXT NOT NULL,          -- 'soapbox-pub/agora' (for display)
    added_by      TEXT NOT NULL,          -- npub of subscriber
    added_at      INTEGER NOT NULL,       -- unix ts
    last_commit_sha      TEXT,            -- highest SHA seen (or NULL = not yet polled)
    last_release_tag     TEXT,            -- highest release tag seen
    last_poll_at         INTEGER,         -- for staleness tracking
    UNIQUE(channel_id, host, owner, repo)
);

CREATE INDEX idx_subscriptions_channel ON subscriptions(channel_id);
CREATE INDEX idx_subscriptions_host_repo ON subscriptions(host, owner, repo);
```

**Why track `last_commit_sha` instead of a `seen_commits` set?**
- Self-cleaning: old SHAs naturally age out as the pointer advances
- One row to update per poll, not N
- If we miss polls (downtime), catching up just walks the API from the last SHA forward

### Deduplication across channels

Multiple channels can subscribe to the same repo. We **do** poll once per subscription (simpler), but the `last_commit_sha` is per-row. This means if 3 channels sub to `soapbox-pub/agora`, we hit the API 3 times per cycle.

**Phase 2 optimization:** Add a separate `repo_state(host, owner, repo, last_commit_sha, last_release_tag, last_poll_at)` table and have subscriptions reference it. One poll per unique repo, fan-out on new activity. Defer until we have >20 subscriptions to justify the complexity.

---

## Polling

### Interval

Default: **5 minutes** (`poll_interval_secs = 300`).
Configurable. Minimum enforced at 60s (don't hammer APIs).

### GitHub API

Unauthenticated limits:
- **60 requests/hour/IP** — very tight. With 6 subscriptions on a 5-min poll, that's 72 req/hr (commits + releases per sub). Will hit the wall.
- **Authenticated (token): 5,000 req/hr** — effectively unlimited for normal use.

**GitHub strongly recommends a token.** Even a read-only public-repo PAT is fine.

Endpoints:
- Commits: `GET /repos/{owner}/{repo}/commits?per_page=5&sha={branch}`
- Latest release: `GET /repos/{owner}/{repo}/releases/latest`
- Tags fallback (if releases returns 404): `GET /repos/{owner}/{repo}/tags?per_page=1`

Use **ETag / If-None-Match** headers for conditional requests. A 304 response doesn't count against rate limit. Big win for efficiency.

### GitLab API

Unauthenticated limits: ~600 req/min per IP (more generous than GitHub).
Endpoints:
- Project lookup: `GET /projects/{url-encoded-path}` — to resolve numeric project ID from `soapbox-pub/agora`
- Commits: `GET /projects/{id}/repository/commits?per_page=5&ref_name={branch}`
- Releases: `GET /projects/{id}/releases?per_page=1`

**Self-hosted GitLab:** Configurable base URL — `gitlab_host = "https://gitlab.example.com"`. Default: `gitlab.com`.

### Poll loop (per cycle)

```
for each subscription in DB:
    if host == github:
        fetch latest commits since last_commit_sha
        fetch latest release
    else:
        fetch latest commits
        fetch latest release
    
    for each new commit (in order, oldest first):
        send "📦 new commit" message to channel_id
    
    if release tag != last_release_tag and release tag != None:
        send "🚀 new release" message to channel_id
    
    update last_commit_sha, last_release_tag, last_poll_at
    sleep 500ms  -- be polite between API calls
```

### Rate-limit handling

- Parse `X-RateLimit-Remaining` and `X-RateLimit-Reset` (GitHub) on every response
- If remaining < 5: skip rest of this poll cycle, log warning
- On 403 secondary rate limit: sleep until reset timestamp + 60s buffer, then bail out of this cycle
- Exponential backoff on network errors (max 3 retries per repo)

---

## Announcement message format

Concord/Vector messages are plain text (no markdown rendering confirmed — keep formatting simple).

### New commit (batched if 1-3, summary if more)

**Single commit:**
```
📦 soapbox-pub/agora · main
abc1234 Fix login redirect loop
Sam Thomson · 2 min ago
https://github.com/soapbox-pub/agora/commit/abc1234
```

**Multiple commits (up to 3):**
```
📦 soapbox-pub/agora · main · 3 new commits
• abc1234 Fix login redirect loop — Sam T.
• def5678 Add dark mode toggle — MK
• 9abcdef Bump deps — Sam T.
https://github.com/soapbox-pub/agora/commits/main
```

**Many commits (>3):**
```
📦 soapbox-pub/agora · main · 14 new commits
Latest: abc1234 Fix login redirect loop (Sam T.)
https://github.com/soapbox-pub/agora/commits/main
```

### New release

```
🚀 soapbox-pub/agora · v2.4.0
"Venezuela translation fixes + dark mode"
https://github.com/soapbox-pub/agora/releases/tag/v2.4.0
```

If release body > 200 chars, truncate with `…` and link for full notes.

### Dedup / anti-spam rules

- **Never re-announce** a SHA/tag we've already posted (DB-backed)
- On first subscription: **don't dump** the current latest commit. Initialize `last_commit_sha` to current HEAD silently. Only announce *new* commits after the subscription is active. Otherwise adding a busy repo spams the channel with backlog.
- Optional config: `announce_on_subscribe = true` to flip this (Phase 2).

---

## Config additions (`bot.toml`)

```toml
[features]
# ... existing ...
git_monitor = true        # new feature flag, default: true

[git_monitor]
enabled = true                    # master switch (requires features.git_monitor)
poll_interval_secs = 300          # 5 min default
github_token = ""                 # optional PAT (strongly recommended)
gitlab_token = ""                 # optional
gitlab_host = "https://gitlab.com"  # for self-hosted instances
default_branch = "main"           # default branch if not auto-detected
post_commits = true
post_releases = true
max_repos_per_channel = 10        # spam guard
polite_sleep_ms = 500             # delay between API calls in a poll cycle
```

Tokens can also be loaded from env vars (higher priority than config):
- `GITHUB_TOKEN`
- `GITLAB_TOKEN`

Tokens should be redacted in `log_summary()`.

---

## Module structure

```
src/
├── git_monitor/
│   ├── mod.rs          # public API: poll_all(), SubscriptionStore
│   ├── store.rs        # SQLite layer (subscriptions + state)
│   ├── github.rs       # GitHub API client
│   ├── gitlab.rs       # GitLab API client
│   ├── detect.rs       # URL/host parsing + slug normalization
│   └── format.rs       # message formatters
├── handlers/
│   └── git_cmds.rs     # !git add/list/remove/info/poll command handlers
```

Wire-up:
- `src/config.rs`: add `GitMonitorSection` (serde default), new `Feature::GitMonitor` variant
- `src/handlers/scheduled.rs`: register `git_monitor::poll_all` on the interval
- `src/handlers/commands.rs`: add `!git` to dispatcher, gated by `features.git_monitor`
- `src/handlers/mod.rs`: `pub mod git_cmds;`
- `src/bot.rs`: open the SQLite store in `run()`, add to `BotContext`

---

## Implementation phases

### Phase 1 — MVP (this sprint, v1.1.0)

Scope: **minimum useful feature**.

- [ ] Config sections + feature flag
- [ ] SQLite store with subscriptions table
- [ ] URL/host detection (`detect.rs`)
- [ ] GitHub client: commits + releases
- [ ] GitLab client: commits + releases (project ID resolution)
- [ ] `!git add/list/remove` commands
- [ ] Background poller registered in `scheduler.rs`
- [ ] Commit + release announcements (formatted per above)
- [ ] ETag caching for GitHub (stretch — if time-boxed, skip for MVP)
- [ ] Anti-spam: silent init (no backlog dump)
- [ ] Rate-limit awareness (skip cycle on low remaining, log)
- [ ] Tests: URL parser, slug detection, message formatters, mock API responses
- [ ] Update `COMMAND_REGISTRY` with `!git` entries
- [ ] Update README + `!help`

### Phase 2 — Polish (v1.2.0)

- [ ] `!git info <repo>` — current state on demand
- [ ] Branch-specific subscriptions (`!git add owner/repo branch`)
- [ ] Tag announcements (separate from releases)
- [ ] Dedupe table: poll once per unique repo, fan-out to channels
- [ ] PR announcements (open/merge/close) — configurable
- [ ] Issue announcements (new issues only)
- [ ] ETag/If-None-Match for GitLab too
- [ ] `announce_on_subscribe` config option
- [ ] Self-hosted GitLab instance testing (only gitlab.com tested in Phase 1)

### Phase 3 — Power user (v1.3.0+)

- [ ] AI-summarized commit log (`!git summarize <repo> [N days]`)
- [ ] Diffstat in announcements (`+120 -45 across 4 files`)
- [ ] Release notes pretty-printed (changelog formatting)
- [ ] Inbound webhook receiver (real-time, no polling) — requires public HTTP endpoint
- [ ] Per-channel format customization
- [ ] Multi-host federation: custom Gitea, Forgejo, Codeberg instances
- [ ] Subscription import/export (`!git export`, `!git import <json>`)

---

## Failure modes & mitigations

| Scenario | Behavior |
|---|---|
| Repo deleted / made private | 404 → mark subscription `unhealthy`, log, skip future polls. After 7 days unhealthy → auto-remove + notify channel. |
| Auth token invalid | 401 → log error once, fall back to unauthenticated, log warning each cycle |
| Rate limited (secondary) | Sleep until reset, bail cycle |
| Network timeout | Retry 3x with backoff, skip repo on 3rd failure |
| Bot offline during a release | On next poll, only announce the **latest** release (not all missed) — avoids spam |
| Branch force-push | New SHAs all look "new" → announce. If >50 commits in one poll, summarize instead of listing |
| Concord channel becomes unreachable | Mark subscription `unhealthy`, retry on next cycle with backoff |

---

## Test plan

Unit tests (`cargo test`):
- URL parsing: all 5 input forms above
- Host detection edge cases (`gitlab.com/...`, `github.com/...`, bare `owner/repo`, explicit prefix)
- Message formatters: single commit, multi-commit, >3 commits, release, truncation
- SQLite: add subscription, dedupe (same channel+repo = error), remove, list
- Mock HTTP responses: fixtures for GitHub/GitLab JSON

Integration tests (in `examples/`):
- Live poll of a known-stable repo (e.g. `octocat/Hello-World`) — guarded behind a `#[ignore]` attribute so CI doesn't hit network

Manual smoke test:
1. `!git add soapbox-pub/agora`
2. Verify response confirms subscription
3. Wait for someone to push (or push a test commit to a fork)
4. Verify announcement appears in channel within 1 poll cycle
5. `!git list` shows the subscription
6. `!git remove soapbox-pub/agora`
7. Verify future commits don't announce

---

## Open questions for Derek

1. **Default for `max_repos_per_channel`** — I proposed 10. Reasonable, or do you want it higher/lower? Auth-only users could bypass.
2. **Token storage** — plain text in `bot.toml`, env vars only, or something fancier (OS keyring like the gws setup)? Plain text is the pattern used elsewhere in concord-bots (nsec is in config), so I'd default to that with env var override.
3. **Should `!git add` accept `!git watch` as an alias**? Some bots use "watch" / "track" / "subscribe". I lean toward keeping just `add` for simplicity.
4. **Commit message author name vs npub** — commit author email is what GitHub/GitLab expose. Show "Sam Thomson" or commit author email? I have it as name only (cleaner).
5. **Do you want PRs/issues in Phase 1 or Phase 2?** I've put them in Phase 2 since they're noisier and need filtering (you don't want every PR open). Could be convinced to do a minimal version in Phase 1.
6. **Should polling be opt-in per-channel** (owner runs `!git enable` first) or available as soon as the feature flag is on? I've assumed the latter — any Authorized+ user can add repos.

---

## Estimate

Phase 1 MVP: **~6-8 hours of dev time** for a solid implementation with tests. Can be split across 2-3 sub-agent tasks (store+config, GitHub client, GitLab client, commands+scheduler).

---

*Last updated: 2026-06-29. Review with Derek, then break into beads tasks for execution.*
