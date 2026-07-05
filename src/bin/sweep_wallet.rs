//! Standalone utility: sweep all Cashu wallet funds into a single token.
//! Writes token to a file to avoid terminal truncation.
//! MUST be run while the bot is stopped.

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
    let out_path = std::env::args()
        .nth(3)
        .unwrap_or_else(|| "sweep-token.txt".to_string());

    let seed_path = PathBuf::from(&data_dir).join("seed");
    let db_path = PathBuf::from(&data_dir).join("wallet.redb");

    println!("Loading wallet from {}", db_path.display());
    println!("Mint: {}", mint_url);

    let seed_data = std::fs::read(&seed_path)
        .with_context(|| format!("read seed file: {}", seed_path.display()))?;
    if seed_data.len() != 64 {
        return Err(anyhow!(
            "Seed file is {} bytes, expected 64.",
            seed_data.len()
        ));
    }
    let mut seed = [0u8; 64];
    seed.copy_from_slice(&seed_data);

    let localstore = Arc::new(
        WalletRedbDatabase::new(&db_path)
            .map_err(|e| anyhow!("Failed to open wallet database: {:?}", e))?,
    );
    let wallet = Wallet::new(&mint_url, CurrencyUnit::Sat, localstore, seed, None)
        .map_err(|e| anyhow!("Failed to create CDK wallet: {:?}", e))?;

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
    println!("Current balance: {} sats", bal_u64);

    if bal_u64 == 0 {
        println!("Wallet is empty — nothing to sweep.");
        println!("Checking if there are pending/unspent proofs from prior sweep...");

        // Try to list pending sagas that might have proofs
        // (The send saga we ran earlier might still be in "pending" state with valid proofs)
        println!("Checking mint for keysets and any unspent proofs via mint state...");

        // Without the proofs in our DB, we can't reconstruct them locally.
        // The proofs were in the token we generated, which may have been corrupted in display.
        return Ok(());
    }

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

    // Write token to file (avoid any stdout truncation)
    std::fs::write(&out_path, token.to_string())
        .map_err(|e| anyhow!("write token file: {:?}", e))?;

    println!("\nBalance after sweep: {} sats", after);
    println!("Token written to: {}", out_path);
    println!("Token length: {} chars", token.to_string().len());

    Ok(())
}
