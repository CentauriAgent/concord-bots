//! Standalone utility: sweep all Cashu wallet funds into a single token.
//!
//! Usage: `cargo run --release --bin sweep_wallet [data_dir] [mint_url]`
//!
//! Defaults: data_dir = "data", mint_url = "https://mint.minibits.cash/Bitcoin"
//!
//! MUST be run while the bot is stopped (redb holds an exclusive lock).

use std::path::PathBuf;
use std::sync::Arc;

use anyhow::{anyhow, Context, Result};
use cdk::nuts::CurrencyUnit;
use cdk::wallet::{SendOptions, Wallet};
use cdk::Amount;
use cdk_redb::wallet::WalletRedbDatabase;

#[tokio::main]
async fn main() -> Result<()> {
    let data_dir = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "data".to_string());
    let mint_url = std::env::args()
        .nth(2)
        .unwrap_or_else(|| "https://mint.minibits.cash/Bitcoin".to_string());

    let seed_path = PathBuf::from(&data_dir).join("seed");
    let db_path = PathBuf::from(&data_dir).join("wallet.redb");

    println!("Loading wallet from {}", db_path.display());
    println!("Mint: {}", mint_url);

    // Load the 64-byte seed
    let seed_data = std::fs::read(&seed_path)
        .with_context(|| format!("read seed file: {}", seed_path.display()))?;
    if seed_data.len() != 64 {
        return Err(anyhow!(
            "Seed file is {} bytes, expected 64. Handle manually.",
            seed_data.len()
        ));
    }
    let mut seed = [0u8; 64];
    seed.copy_from_slice(&seed_data);

    // Open the wallet
    let localstore = Arc::new(
        WalletRedbDatabase::new(&db_path)
            .map_err(|e| anyhow!("Failed to open wallet database: {:?}", e))?,
    );
    let wallet = Wallet::new(&mint_url, CurrencyUnit::Sat, localstore, seed, None)
        .map_err(|e| anyhow!("Failed to create CDK wallet: {:?}", e))?;

    // Recover any pending state
    if let Err(e) = wallet.recover_incomplete_sagas().await {
        eprintln!("warn: saga recovery error (non-fatal): {:?}", e);
    }
    if let Err(e) = wallet.mint_unissued_quotes().await {
        eprintln!("warn: pending quote mint error (non-fatal): {:?}", e);
    }

    let bal = wallet
        .total_balance()
        .await
        .map_err(|e| anyhow!("balance check failed: {:?}", e))?;
    let bal_u64 = u64::from(bal);
    println!("\nCurrent balance: {} sats", bal_u64);

    if bal_u64 == 0 {
        println!("Wallet is empty — nothing to sweep.");
        return Ok(());
    }

    // Sweep: prepare send for full balance, then confirm
    let prepared = wallet
        .prepare_send(Amount::from(bal_u64), SendOptions::default())
        .await
        .map_err(|e| anyhow!("prepare_send failed: {:?}", e))?;
    let token = prepared
        .confirm(None)
        .await
        .map_err(|e| anyhow!("confirm send failed: {:?}", e))?;

    let after = wallet
        .total_balance()
        .await
        .map(|a| u64::from(a))
        .unwrap_or(0);

    println!("\n=== CASHU TOKEN (redeem at any compatible wallet) ===");
    println!("{}", token);
    println!("=== END TOKEN ===\n");
    println!("Balance after sweep: {} sats", after);
    println!(
        "Safe to delete {}, {}, and the seed file now.",
        db_path.display(),
        seed_path.display()
    );

    Ok(())
}
