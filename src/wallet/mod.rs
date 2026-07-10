// =============================================================================
// wallet/mod.rs — Cashu wallet manager (CDK-based)
// =============================================================================
//
// Native Cashu ecash wallet using the official CDK library.
// Provides: balance, send_tip (Cashu token), deposit (Lightning), withdraw (melt).

use std::path::Path;
use std::sync::Arc;

use anyhow::{anyhow, Context, Result};
use cdk::nuts::{CurrencyUnit, PaymentMethod};
use cdk::wallet::{SendOptions, Wallet};
use cdk::Amount;
use cdk_redb::wallet::WalletRedbDatabase;
use tracing::instrument;

/// Wrapper around the CDK Wallet for Cashu ecash operations.
#[derive(Clone)]
pub struct CashuWallet {
    wallet: Arc<Wallet>,
    mint_url: String,
}

impl CashuWallet {
    /// Initialize the wallet: load or generate seed, open DB, create wallet.
    pub async fn init(data_dir: &Path, mint_url: &str) -> Result<Self> {
        // Ensure data dir exists
        std::fs::create_dir_all(data_dir)
            .with_context(|| format!("Failed to create data dir: {:?}", data_dir))?;

        // 1. Load or generate mnemonic seed
        let seed_path = data_dir.join("seed");
        let seed = load_or_create_seed(&seed_path)?;

        // 2. Open persistent Redb store
        let db_path = data_dir.join("wallet.redb");
        let localstore = Arc::new(
            WalletRedbDatabase::new(&db_path)
                .map_err(|e| anyhow!("Failed to open wallet database: {:?}", e))?,
        );

        // 3. Create the CDK wallet
        let wallet = Wallet::new(mint_url, CurrencyUnit::Sat, localstore, seed, None)
            .map_err(|e| anyhow!("Failed to create CDK wallet: {:?}", e))?;

        // 4. Recover any incomplete operations from previous runs
        if let Err(e) = wallet.recover_incomplete_sagas().await {
            tracing::warn!("Wallet saga recovery error (non-fatal): {:?}", e);
        }

        // 5. Try to mint any pending paid quotes
        if let Err(e) = wallet.mint_unissued_quotes().await {
            tracing::warn!("Pending quote mint error (non-fatal): {:?}", e);
        }

        tracing::info!(
            "Cashu wallet initialized — mint: {}, db: {}",
            mint_url,
            db_path.display()
        );

        Ok(Self {
            wallet: Arc::new(wallet),
            mint_url: mint_url.to_string(),
        })
    }

    /// Get the configured mint URL.
    pub fn mint_url(&self) -> &str {
        &self.mint_url
    }

    /// Total available balance in sats.
    #[instrument(skip(self))]
    pub async fn balance(&self) -> Result<u64> {
        let bal = self
            .wallet
            .total_balance()
            .await
            .map_err(|e| anyhow!("Balance check failed: {:?}", e))?;
        Ok(u64::from(bal))
    }

    /// Create a Cashu token for someone to claim.
    /// Returns the V3-encoded token string.
    #[instrument(skip(self))]
    pub async fn send_tip(&self, amount_sats: u64) -> Result<String> {
        let amount = Amount::from(amount_sats);

        // Check balance first for a friendly error
        let bal = self.wallet.total_balance().await
            .map_err(|e| anyhow!("Balance check failed: {:?}", e))?;
        if bal < amount {
            return Err(anyhow!(
                "Insufficient funds: have {} sats, need {} sats",
                u64::from(bal),
                amount_sats
            ));
        }

        // Prepare and confirm the send
        let prepared = self
            .wallet
            .prepare_send(amount, SendOptions::default())
            .await
            .map_err(|e| anyhow!("Send preparation failed: {:?}", e))?;

        let token = prepared
            .confirm(None)
            .await
            .map_err(|e| anyhow!("Send confirmation failed: {:?}", e))?;

        Ok(token.to_string())
    }

    /// Create a Lightning invoice for depositing sats into the wallet.
    /// Returns (quote_id, bolt11_invoice).
    #[instrument(skip(self))]
    pub async fn deposit(&self, amount_sats: u64) -> Result<(String, String)> {
        let amount = Amount::from(amount_sats);

        let quote = self
            .wallet
            .mint_quote(PaymentMethod::BOLT11, Some(amount), None, None)
            .await
            .map_err(|e| anyhow!("Mint quote failed: {:?}", e))?;

        Ok((quote.id, quote.request))
    }

    /// Pay a BOLT11 invoice from the wallet balance.
    #[instrument(skip(self))]
    pub async fn withdraw(&self, invoice: &str) -> Result<u64> {
        // Create melt quote
        let melt_quote = self
            .wallet
            .melt_quote(PaymentMethod::BOLT11, invoice.to_string(), None, None)
            .await
            .map_err(|e| anyhow!("Melt quote failed: {:?}", e))?;

        // Prepare the melt
        let prepared = self
            .wallet
            .prepare_melt(&melt_quote.id, std::collections::HashMap::new())
            .await
            .map_err(|e| anyhow!("Melt preparation failed: {:?}", e))?;

        let amount = u64::from(prepared.amount());

        // Confirm (execute) the melt
        let confirmed = prepared
            .confirm()
            .await
            .map_err(|e| anyhow!("Melt confirmation failed: {:?}", e))?;

        tracing::info!(
            "Withdrawal complete — amount: {}, fee_paid: {}, state: {:?}",
            u64::from(confirmed.amount()),
            u64::from(confirmed.fee_paid()),
            confirmed.state()
        );

        Ok(amount)
    }

    /// Receive a Cashu token (e.g. from a tip or npub.cash claim) into the wallet.
    /// Returns the amount received in sats.
    #[instrument(skip(self, encoded_token))]
    pub async fn receive(&self, encoded_token: &str) -> Result<u64> {
        use cdk::wallet::ReceiveOptions;
        let amount = self
            .wallet
            .receive(encoded_token, ReceiveOptions::default())
            .await
            .map_err(|e| anyhow!("Receive failed: {:?}", e))?;
        Ok(u64::from(amount))
    }
}

// -----------------------------------------------------------------------------
// Seed management
// -----------------------------------------------------------------------------

/// Load an existing 64-byte seed from file, or generate a new one from a BIP39 mnemonic.
/// The seed file contains the raw 64-byte seed (NOT the mnemonic phrase).
fn load_or_create_seed(seed_path: &Path) -> Result<[u8; 64]> {
    if seed_path.exists() {
        tracing::info!("Loading existing wallet seed from {}", seed_path.display());
        let data = std::fs::read(seed_path)
            .with_context(|| format!("Failed to read seed file: {}", seed_path.display()))?;

        if data.len() == 64 {
            // Raw 64-byte seed
            let mut seed = [0u8; 64];
            seed.copy_from_slice(&data);
            return Ok(seed);
        } else {
            // Treat as BIP39 mnemonic text
            let mnemonic_str = String::from_utf8(data)
                .with_context(|| "Seed file is neither 64 bytes nor valid UTF-8")?;
            let mnemonic = bip39::Mnemonic::parse_normalized(mnemonic_str.trim())
                .with_context(|| "Failed to parse mnemonic from seed file")?;
            let seed = mnemonic.to_seed_normalized("");
            zeroize_mnemonic_string(mnemonic_str);
            return Ok(seed);
        }
    }

    // Generate new mnemonic
    tracing::info!("Generating new wallet seed at {}", seed_path.display());
    let mnemonic = bip39::Mnemonic::generate(24)
        .map_err(|e| anyhow!("Failed to generate mnemonic: {:?}", e))?;

    // Derive the 64-byte seed
    let seed = mnemonic.to_seed_normalized("");

    // Write the raw seed (NOT the mnemonic) with permissions 600
    use std::os::unix::fs::PermissionsExt;
    std::fs::write(seed_path, &seed)
        .with_context(|| format!("Failed to write seed file: {}", seed_path.display()))?;
    std::fs::set_permissions(seed_path, std::fs::Permissions::from_mode(0o600))
        .with_context(|| "Failed to set seed file permissions")?;

    tracing::info!("Wallet seed created and stored with mode 600");
    Ok(seed)
}

/// Best-effort zeroization of a String that held mnemonic text.
fn zeroize_mnemonic_string(mut s: String) {
    // Overwrite the string contents
    unsafe {
        let bytes = s.as_bytes_mut();
        for b in bytes.iter_mut() {
            *b = 0;
        }
    }
    let _ = s; // dropped
}
