// =============================================================================
// handlers/wallet_cmds.rs — Cashu wallet commands (!balance, !tip, !deposit, !withdraw)
// =============================================================================
//
// Wallet commands provide Cashu ecash functionality:
//   !balance            — Show wallet balance
//   !tip <sats> [memo]  — Create Cashu token for anyone to claim
//   !deposit [sats]     — Generate Lightning invoice to fund wallet
//   !withdraw <invoice> — Pay a Lightning invoice from wallet

use anyhow::Result;
use vector_sdk::IncomingMessage;

use crate::bot::BotContext;

// -----------------------------------------------------------------------------
// !balance — Show wallet balance
// -----------------------------------------------------------------------------

pub async fn balance_command(ctx: &BotContext, msg: &IncomingMessage) -> Result<()> {
    let wallet = match get_wallet(ctx) {
        Some(w) => w,
        None => {
            msg.reply(
                "💰 Wallet not initialized. Ask the bot owner to enable the Cashu wallet in bot.toml.\n\
                 See `!help` for available commands.",
            )
            .await?;
            return Ok(());
        }
    };

    match wallet.balance().await {
        Ok(sats) => {
            let mint = wallet.mint_url();
            // Extract just the domain for a cleaner display
            let mint_display = mint
                .split("//")
                .nth(1)
                .unwrap_or(mint)
                .split('/')
                .next()
                .unwrap_or(mint);
            msg.reply(&format!("💰 Wallet: {} sats ({})", sats, mint_display)).await?;
        }
        Err(e) => {
            tracing::warn!("Balance check failed: {:?}", e);
            msg.reply("⚠️ Could not check wallet balance. The mint may be unreachable.").await?;
        }
    }

    Ok(())
}

// -----------------------------------------------------------------------------
// !tip <sats> [memo] — Create Cashu token
// -----------------------------------------------------------------------------

pub async fn tip_command(ctx: &BotContext, msg: &IncomingMessage, args: &str) -> Result<()> {
    let wallet = match get_wallet(ctx) {
        Some(w) => w,
        None => {
            msg.reply("💰 Wallet not initialized. Ask the bot owner to enable it.").await?;
            return Ok(());
        }
    };

    let args = args.trim();
    if args.is_empty() {
        msg.reply("Usage: !tip <sats> [memo]\nExample: !tip 21 Thanks for the help!").await?;
        return Ok(());
    }

    // Parse: first number is sats, rest is optional memo
    let parts: Vec<&str> = args.splitn(2, char::is_whitespace).collect();
    let sats: u64 = match parts[0].parse() {
        Ok(n) if n > 0 => n,
        _ => {
            msg.reply("⚠️ Please provide a valid positive number of sats.\nExample: !tip 21").await?;
            return Ok(());
        }
    };

    let memo = parts.get(1).map(|s| s.trim()).filter(|s| !s.is_empty());

    match wallet.send_tip(sats).await {
        Ok(token) => {
            let response = if let Some(m) = memo {
                format!("💸 {} sats up for grabs: {}\nMemo: {}", sats, token, m)
            } else {
                format!("💸 {} sats up for grabs: {}", sats, token)
            };
            msg.reply(&response).await?;
        }
        Err(e) => {
            let msg_text = format!("{:?}", e);
            if msg_text.contains("Insufficient") {
                msg.reply(&format!(
                    "❌ Insufficient balance for a {} sat tip. Use !deposit to add funds.",
                    sats
                )).await?;
            } else {
                tracing::warn!("Tip failed: {:?}", e);
                msg.reply("⚠️ Could not create tip. The mint may be unreachable.").await?;
            }
        }
    }

    Ok(())
}

// -----------------------------------------------------------------------------
// !deposit [sats] — Generate Lightning invoice
// -----------------------------------------------------------------------------

pub async fn deposit_command(ctx: &BotContext, msg: &IncomingMessage, args: &str) -> Result<()> {
    let wallet = match get_wallet(ctx) {
        Some(w) => w,
        None => {
            msg.reply("💰 Wallet not initialized. Ask the bot owner to enable it.").await?;
            return Ok(());
        }
    };

    let args = args.trim();
    let sats: u64 = if args.is_empty() {
        1000 // default deposit
    } else {
        match args.parse() {
            Ok(n) if n > 0 => n,
            _ => {
                msg.reply("Usage: !deposit [sats]\nExample: !deposit 1000").await?;
                return Ok(());
            }
        }
    };

    if sats < 100 {
        msg.reply("⚠️ Minimum deposit is 100 sats.").await?;
        return Ok(());
    }

    match wallet.deposit(sats).await {
        Ok((_quote_id, invoice)) => {
            msg.reply(&format!(
                "⚡ Deposit {} sats:\n{}\n\nPay the invoice to fund the wallet.",
                sats, invoice
            )).await?;
        }
        Err(e) => {
            tracing::warn!("Deposit quote failed: {:?}", e);
            msg.reply("⚠️ Could not generate deposit invoice. The mint may be unreachable.").await?;
        }
    }

    Ok(())
}

// -----------------------------------------------------------------------------
// !withdraw <invoice> — Pay Lightning invoice
// -----------------------------------------------------------------------------

pub async fn withdraw_command(ctx: &BotContext, msg: &IncomingMessage, args: &str) -> Result<()> {
    let wallet = match get_wallet(ctx) {
        Some(w) => w,
        None => {
            msg.reply("💰 Wallet not initialized. Ask the bot owner to enable it.").await?;
            return Ok(());
        }
    };

    let invoice = args.trim();
    if invoice.is_empty() {
        msg.reply("Usage: !withdraw <invoice>\nExample: !withdraw lnbc10u1p3...").await?;
        return Ok(());
    }

    if !invoice.starts_with("lnbc") {
        msg.reply("⚠️ That doesn't look like a Lightning invoice. Invoices start with \"lnbc\".").await?;
        return Ok(());
    }

    // Let the user know this may take a moment
    msg.reply("⏳ Processing payment...").await?;

    match wallet.withdraw(invoice).await {
        Ok(amount_paid) => {
            msg.reply(&format!("✅ Paid {} sats!", amount_paid)).await?;
        }
        Err(e) => {
            let msg_text = format!("{:?}", e);
            if msg_text.contains("Insufficient") {
                msg.reply("❌ Insufficient balance to pay this invoice. Use !deposit to add funds.").await?;
            } else {
                tracing::warn!("Withdraw failed: {:?}", e);
                msg.reply("⚠️ Could not pay invoice. It may be expired, invalid, or the mint is unreachable.").await?;
            }
        }
    }

    Ok(())
}

// -----------------------------------------------------------------------------
// Helpers
// -----------------------------------------------------------------------------

/// Get the wallet from context, or None if not initialized.
fn get_wallet(ctx: &BotContext) -> Option<&std::sync::Arc<crate::wallet::CashuWallet>> {
    ctx.wallet.as_ref()
}
