# Cashu Wallet Integration Design — v0.4.0

**Research source:** [cashubtc/cdk](https://github.com/cashubtc/cdk) (official Cashu Development Kit)
**Status:** ✅ IMPLEMENTED (Jun 28, 2026) — commit `95014ea`
**Crate:** `cdk = "0.17.2-rc.2"` + `cdk-redb` (switched from cdk-sqlite due to rusqlite version conflict with vector_sdk)

---

## Implementation Notes

### What Changed from Original Design

1. **cdk-sqlite → cdk-redb**: `cdk-sqlite` uses rusqlite 0.31 which conflicts with `vector_sdk`'s rusqlite 0.37 (both link `libsqlite3-sys` natively). Switched to `cdk-redb` which uses pure-Rust `redb` embedded DB — no native lib conflicts.

2. **Wallet API**: Used `Wallet::new()` direct constructor instead of `WalletRepositoryBuilder`. The CDK `Wallet` struct handles everything for a single-mint wallet.

3. **`!zap` (NIP-57)**: Not implemented in this pass. The full NIP-57 flow (LNURL resolution, kind 9734 zap request signing, etc.) is complex and was deferred per task instructions ("If the full NIP-57 flow is too complex... stub it with a TODO and implement !tip properly first"). Can be added in a follow-up.

### What Works

- ✅ **Wallet init**: BIP39 mnemonic generation, Redb persistent storage, seed file (mode 600)
- ✅ **`!balance`**: Shows total unspent balance across the configured mint
- ✅ **`!tip <sats> [memo]`**: Creates Cashu V3 token via `prepare_send → confirm`
- ✅ **`!deposit [sats]`**: Creates Lightning mint quote (BOLT11 invoice)
- ✅ **`!withdraw <invoice>`**: Full melt flow: melt_quote → prepare_melt → confirm
- ✅ **Graceful errors**: Insufficient funds, mint unreachable, wallet not initialized
- ✅ **Config**: `[wallet]` section with `enabled` (default false) and `mint_url`
- ✅ **Feature gating**: Wallet commands gated by `features.nostr`

### Files Modified/Created

| File | Action |
|------|--------|
| `src/wallet/mod.rs` | **CREATED** — CashuWallet wrapper (init, balance, send_tip, deposit, withdraw) |
| `src/handlers/wallet_cmds.rs` | **CREATED** — !balance, !tip, !deposit, !withdraw command handlers |
| `src/handlers/mod.rs` | Modified — Added `wallet_cmds` module |
| `src/handlers/commands.rs` | Modified — Added wallet commands to registry and dispatch |
| `src/bot.rs` | Modified — Added wallet init + `wallet: Option<Arc<CashuWallet>>` to BotContext |
| `src/config.rs` | Modified — Added `WalletSection` with `enabled` and `mint_url` |
| `src/main.rs` | Modified — Added `mod wallet;` |
| `Cargo.toml` | Modified — Added cdk, cdk-redb, bip39 deps; version → 0.4.0 |
| `config/bot.toml.example` | Modified — Added `[wallet]` section documentation |

### To Enable

Add to `config/bot.toml`:
```toml
[wallet]
enabled = true
mint_url = "https://mint.minibits.cash/Bitcoin"
```

---

## Original Design (Reference)

### Wallet Module: `src/wallet/mod.rs`

The bot gets its own Cashu wallet. On first run:
1. Generate a BIP39 mnemonic (random 32 bytes → 24 words)
2. Store in `~/.concord-bots/seed` (file perm 600)
3. Create SQLite store at `~/.concord-bots/wallet.sqlite`
4. Create wallet for configured mint URL

```rust
// src/wallet/mod.rs
use cdk::wallet::WalletRepositoryBuilder;
use cdk_sqlite::WalletSqliteDatabase;

pub struct CashuWallet {
    repo: WalletRepository,
}

impl CashuWallet {
    pub async fn init(data_dir: &Path, mint_url: &str) -> Result<Self> {
        // 1. Load or generate mnemonic
        let seed_path = data_dir.join("seed");
        let mnemonic = load_or_create_mnemonic(&seed_path)?;

        // 2. Open SQLite store
        let db_path = data_dir.join("wallet.sqlite");
        let localstore = WalletSqliteDatabase::new(&db_path).await?;

        // 3. Build wallet repository
        let repo = WalletRepositoryBuilder::new()
            .localstore(Arc::new(localstore))
            .seed(mnemonic.to_seed_normalized(""))
            .build()
            .await?;

        // 4. Ensure wallet exists for mint
        let mint_url = MintUrl::from_str(mint_url)?;
        let unit = CurrencyUnit::Sat;
        // Wallet is auto-created on first use

        Ok(Self { repo })
    }

    pub async fn balance(&self) -> Result<Amount> {
        let balances = self.repo.get_balances().await?;
        let total = balances.values().copied().sum();
        Ok(total)
    }

    pub async fn send_tip(&self, amount: u64, memo: Option<String>) -> Result<String> {
        // Get wallet for default mint
        let wallets = self.repo.get_wallets().await?;
        let wallet = &wallets[0]; // first wallet = default mint

        let amount = Amount::from(amount);
        let balance = wallet.total_balance().await?;
        if balance < amount {
            return Err(anyhow!("Insufficient funds: {} < {}", balance, amount));
        }

        // Send creates a Cashu token
        let token = wallet
            .send(amount, None, memo, SendOptions::default())
            .await?;

        Ok(token.to_v3_string())
    }

    pub async fn deposit(&self, amount: u64) -> Result<String> {
        // Create a mint quote (Lightning invoice)
        let wallets = self.repo.get_wallets().await?;
        let wallet = &wallets[0];

        let quote = wallet.mint_quote(amount).await?;
        Ok(quote.request) // BOLT11 invoice
    }

    pub async fn withdraw(&self, invoice: &str) -> Result<()> {
        // Melt tokens to pay a BOLT11 invoice
        let wallets = self.repo.get_wallets().await?;
        let wallet = &wallets[0];

        wallet.melt(invoice.to_string()).await?;
        Ok(())
    }
}
```

### Config: `bot.toml`

```toml
[wallet]
enabled = true
mint_url = "https://mint.minibits.cash/Bitcoin"
# Optional: use existing mnemonic
# seed_file = "/path/to/seed"
```

### Commands

#### `!balance`
```
!balance → "💰 Wallet: 96 sats (mint.minibits.cash)"
```

#### `!tip <@user> <sats>` OR `!tip <sats>`
- `!tip @user 21` — Creates Cashu token, posts in chat for that user to claim
- `!tip 21` — Creates token for anyone to claim
- Token format: `cashuA...` (V3 format)
- Message: "💸 @user, here's 21 sats: `cashuA...`"

#### `!deposit [sats]`
- `!deposit` or `!deposit 1000` — Creates a Lightning invoice
- Message: "⚡ Deposit invoice: `lnbc10u1p3...`"
- Default amount: 1000 sats

#### `!withdraw <invoice>`
- `!withdraw lnbc...` — Pays a BOLT11 invoice from wallet balance
- Confirms: "✅ Paid: 1000 sats"

#### `!zap <npub> <sats> [message]`
- Full NIP-57 zap flow:
  1. Look up recipient's lud16 from Nostr kind 0
  2. Resolve LNURL endpoint
  3. Create kind 9734 zap request (signed by bot's Nostr key)
  4. Get BOLT11 invoice from LNURL callback
  5. Pay invoice via Cashu melt
  6. Recipient's wallet publishes kind 9735 receipt (visible on Nostr)
- This is the premium feature that makes Concord bots special

### Non-Wallet Nostr Commands (separate agent)

#### `!nostr <npub>` — Profile lookup
- Fetch kind 0 metadata
- Show: name, nip05, about, lud16, picture

#### `!nip05 <user@domain>` — Verify NIP-05
- Query `https://domain/.well-known/nostr.json?name=user`
- Show verified npub or "not found"

#### `!follow <npub>` — Follow on Nostr
- Publish kind 3 contact list with new follow
- Uses bot's Nostr key

---

## Dependencies to Add (Cargo.toml)

```toml
[dependencies]
# ... existing ...
cdk = { version = "0.17", features = ["wallet", "nostr"] }
cdk-sqlite = "0.17"
cdk-http-client = "0.17"
bip39 = "2"
nostr-sdk = "0.40"  # for NIP-57 zaps and profile lookups
```

## Files to Create/Modify

| File | Action |
|------|--------|
| `src/wallet/mod.rs` | NEW — Cashu wallet manager |
| `src/handlers/nostr_cmds.rs` | NEW — Nostr commands (!nostr, !nip05, !follow) |
| `src/handlers/wallet_cmds.rs` | NEW — Wallet commands (!balance, !tip, !zap, !deposit, !withdraw) |
| `src/handlers/mod.rs` | MODIFY — Add new modules to dispatch |
| `src/handlers/commands.rs` | MODIFY — Add commands to registry + feature gating |
| `src/bot.rs` | MODIFY — Initialize wallet on startup (add to BotContext) |
| `src/config.rs` | MODIFY — Add WalletSection |
| `Cargo.toml` | MODIFY — Add dependencies |
| `config/bot.toml.example` | MODIFY — Add [wallet] section |

## Safety Notes

- Wallet seed stored with file perm 600
- All wallet commands gated behind `features.nostr`
- If wallet not configured, commands show helpful error
- Never expose seed phrase in logs or commands
