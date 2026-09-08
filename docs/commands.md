# Command reference

Every command works in any community the bot belongs to. Auth levels:
**Public** → anyone · **Authorized+** → the owner's allowlist · **Owner** → bot owner only.

| Command | Auth | Description |
|---------|------|-------------|
| `!ping` | Public | Health check — replies `pong 🏓` |
| `!help` | Public | Lists available commands (respects feature flags) |
| `!echo <text>` | Public | Echoes the text |
| `!whoami` | Public | Bot npub + version |
| `!auth` | Public | Your authorization status |
| `!add <npub>` | Owner | Allowlist a user |
| `!remove <npub>` | Owner | Remove from allowlist |
| `!list` | Owner | List allowed users |
| `!git add <url\|owner/repo>` | Authorized+ | Subscribe channel to a repo (GitHub/GitLab) |
| `!git list` | Public | Channel's repo subscriptions |
| `!git remove <repo\|id>` | Authorized+ | Unsubscribe |
| `!git poll` | Owner | Force-poll subscriptions now |
| `!balance` | Public | Wallet balance (sats) |
| `!tip <sats>` | Authorized+ | Cashu token tip |
| `!deposit [sats]` | Authorized+ | BOLT11 invoice to fund wallet |
| `!withdraw <invoice>` | Authorized+ | Pay a BOLT11 invoice |
| `!zap <npub> <sats> [msg]` | Authorized+ | NIP-57 zap |
| `!ask <question>` | Public* | AI answer in-thread (needs `features.ai`) |
| `!summarize <text>` | Public* | AI summary (needs `features.ai`) |
| `!sentiment <text>` | Public* | Sentiment read (needs `features.ai`) |
| `!image <prompt>` | Public* | Generate an image (needs `features.ai`) |

\* Hidden from `!help` and rejected when the `ai` feature flag is off.
