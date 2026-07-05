//! Melt wallet balance via Lightning to Derek's LN address.
//!
//! Usage: `cargo run --release --bin melt_to_ln -- <data_dir> <amount_sats> [ln_address]`
//!
//! MUST be run while the bot is stopped.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::{anyhow, Context, Result};
use cdk::nuts::CurrencyUnit;
use cdk::wallet::Wallet;
use cdk_redb::wallet::WalletRedbDatabase;

async fn open_wallet(data_dir: &Path, mint_url: &str) -> Result<Wallet> {
    std::fs::create_dir_all(data_dir)
        .with_context(|| format!("create data dir: {:?}", data_dir))?;

    let seed_path = data_dir.join("seed");
    let seed_data = std::fs::read(&seed_path)
        .with_context(|| format!("read seed: {}", seed_path.display()))?;
    if seed_data.len() != 64 {
        return Err(anyhow!("seed file is {} bytes, expected 64", seed_data.len()));
    }
    let mut seed = [0u8; 64];
    seed.copy_from_slice(&seed_data);

    let db_path = data_dir.join("wallet.redb");
    let localstore = Arc::new(
        WalletRedbDatabase::new(&db_path)
            .map_err(|e| anyhow!("wallet db: {:?}", e))?,
    );
    let wallet = Wallet::new(mint_url, CurrencyUnit::Sat, localstore, seed, None)
        .map_err(|e| anyhow!("wallet create: {:?}", e))?;

    if let Err(e) = wallet.recover_incomplete_sagas().await {
        eprintln!("warn: saga recovery: {:?}", e);
    }
    if let Err(e) = wallet.mint_unissued_quotes().await {
        eprintln!("warn: pending quotes: {:?}", e);
    }

    Ok(wallet)
}

#[tokio::main]
async fn main() -> Result<()> {
    let data_dir = std::env::args()
        .nth(1)
        .context("pass data_dir as first arg")?;
    let amount_sats: u64 = std::env::args()
        .nth(2)
        .context("pass amount_sats as second arg")?
        .parse()
        .context("amount_sats must be a number")?;
    let ln_address = std::env::args()
        .nth(3)
        .unwrap_or_else(|| "pay@derekross.me".to_string());

    let data_dir_path = PathBuf::from(&data_dir);
    let mint_url = "https://mint.minibits.cash/Bitcoin";

    println!("Loading wallet from {}", data_dir_path.display());
    println!("Mint: {}", mint_url);
    println!("Target LN address: {}", ln_address);
    println!("Amount to send: {} sats", amount_sats);

    let wallet = open_wallet(&data_dir_path, mint_url).await?;

    let bal = wallet.total_balance().await
        .map_err(|e| anyhow!("balance: {:?}", e))?;
    let bal_u64 = u64::from(bal);
    println!("Wallet balance: {} sats", bal_u64);

    if bal_u64 < amount_sats {
        return Err(anyhow!(
            "Insufficient balance: have {} sats, need {} sats",
            bal_u64, amount_sats
        ));
    }

    // LNURL-pay
    let parts: Vec<&str> = ln_address.split('@').collect();
    if parts.len() != 2 {
        return Err(anyhow!("invalid LN address: {}", ln_address));
    }
    let user = parts[0];
    let domain = parts[1];
    let lnurlp_url = format!("https://{}/.well-known/lnurlp/{}", domain, user);
    println!("\nFetching LNURL endpoint: {}", lnurlp_url);

    let client = reqwest::Client::new();
    let resp = client.get(&lnurlp_url).send().await
        .map_err(|e| anyhow!("lnurl fetch: {:?}", e))?;
    let status = resp.status();
    let body = resp.text().await.unwrap_or_default();
    println!("HTTP {}", status);
    if !status.is_success() {
        return Err(anyhow!("LNURL endpoint returned {}: {}", status, body));
    }
    let lnurl_json: serde_json::Value = serde_json::from_str(&body)
        .map_err(|e| anyhow!("LNURL parse: {:?}", e))?;

    let callback = lnurl_json.get("callback")
        .and_then(|v| v.as_str())
        .ok_or_else(|| anyhow!("no callback in LNURL response"))?;
    let min_sendable = lnurl_json.get("minSendable")
        .and_then(|v| v.as_u64())
        .unwrap_or(1000);
    let max_sendable = lnurl_json.get("maxSendable")
        .and_then(|v| v.as_u64())
        .unwrap_or(1_000_000_000);
    println!("Callback: {}", callback);
    println!("minSendable: {} msat, maxSendable: {} msat", min_sendable, max_sendable);

    let amount_msat = amount_sats * 1000;
    if amount_msat < min_sendable || amount_msat > max_sendable {
        return Err(anyhow!(
            "Amount {} msat outside [{}, {}]",
            amount_msat, min_sendable, max_sendable
        ));
    }

    // Request invoice (callback may already have query params)
    let sep = if callback.contains('?') { '&' } else { '?' };
    let invoice_url = format!("{}{}amount={}", callback, sep, amount_msat);
    println!("\nRequesting invoice: {}", invoice_url);
    let resp = client.get(&invoice_url).send().await
        .map_err(|e| anyhow!("invoice fetch: {:?}", e))?;
    let status = resp.status();
    let body = resp.text().await.unwrap_or_default();
    println!("HTTP {}", status);
    if !status.is_success() {
        return Err(anyhow!("invoice endpoint returned {}: {}", status, body));
    }
    let inv_json: serde_json::Value = serde_json::from_str(&body)
        .map_err(|e| anyhow!("invoice parse: {:?}", e))?;

    let invoice = inv_json.get("pr")
        .and_then(|v| v.as_str())
        .ok_or_else(|| anyhow!("no 'pr' field in invoice response"))?;
    println!("\nGot invoice ({} chars)", invoice.len());

    // Melt via CDK directly
    use cdk::nuts::PaymentMethod;
    println!("\nMelting {} sats via wallet...", amount_sats);

    let melt_quote = wallet.melt_quote(PaymentMethod::BOLT11, invoice.to_string(), None, None).await
        .map_err(|e| anyhow!("melt quote failed: {:?}", e))?;
    println!("Melt quote: amount={}, fee={}", melt_quote.amount, melt_quote.fee_reserve);

    let prepared = wallet.prepare_melt(&melt_quote.id, std::collections::HashMap::new()).await
        .map_err(|e| anyhow!("prepare melt failed: {:?}", e))?;
    let pay_amount = u64::from(prepared.amount());
    println!("Prepared melt: paying {} sats", pay_amount);

    let confirmed = prepared.confirm().await
        .map_err(|e| anyhow!("melt confirm failed: {:?}", e))?;
    println!("✅ MELT COMPLETE — amount: {}, fee_paid: {}, state: {:?}",
        u64::from(confirmed.amount()),
        u64::from(confirmed.fee_paid()),
        confirmed.state()
    );

    let after = wallet.total_balance().await
        .map(|a| u64::from(a))
        .unwrap_or(0);
    println!("Wallet balance after: {} sats", after);

    Ok(())
}
