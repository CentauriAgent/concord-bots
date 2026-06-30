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
use std::time::Duration;
use tokio::io::AsyncWriteExt;
use vector_sdk::IncomingMessage;

use crate::bot::BotContext;
use crate::handlers::normalize_npub;
use crate::lib::http;

// -----------------------------------------------------------------------------
// Constants for zap operations
// -----------------------------------------------------------------------------

/// Relays used for Nostr read operations.
const READ_RELAYS: &[&str] = &[
    "wss://relay.ditto.pub",
    "wss://relay.primal.net",
    "wss://nos.lol",
];

/// Relays used for Nostr write operations.
const WRITE_RELAYS: &[&str] = &[
    "wss://relay.ditto.pub",
    "wss://relay.primal.net",
];

/// Timeout for nak CLI commands (seconds).
const NAK_TIMEOUT_SECS: u64 = 15;

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

            // Community XP: sender gets +5 XP for tipping
            if let Some(ref sender_npub) = msg.message.npub {
                if let Err(e) = ctx.community_db.award_xp(sender_npub, 5, &msg.chat_id) {
                    tracing::warn!("Failed to award tip XP to sender: {}", e);
                }
                if let Err(e) = ctx.community_db.add_sats_tipped(sender_npub, sats as i64) {
                    tracing::warn!("Failed to track tipped sats: {}", e);
                }
            }
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
// !zap <npub> <sats> [message] — NIP-57 Lightning zap
// -----------------------------------------------------------------------------
//
// Full NIP-57 zap flow:
//   1. Resolve recipient's Lightning address (lud16) from their Nostr profile
//   2. Resolve LNURL pay endpoint
//   3. Create and sign a kind 9734 zap request
//   4. Fetch a BOLT11 invoice from the recipient's LNURL service
//   5. Pay the invoice from the Cashu wallet
//   6. The recipient's service publishes the kind 9735 zap receipt

pub async fn zap_command(ctx: &BotContext, msg: &IncomingMessage, args: &str) -> Result<()> {
    let args = args.trim();
    if args.is_empty() {
        msg.reply(
            "Usage: !zap <npub> <sats> [message]\nExample: !zap npub1jrvd... 21 Great post!"
        ).await?;
        return Ok(());
    }

    // Parse: <npub> <sats> [optional message]
    let parts: Vec<&str> = args.splitn(3, char::is_whitespace).collect();
    let npub_input = normalize_npub(parts[0]);
    let sats_str = parts.get(1).copied().unwrap_or("");
    let zap_message = parts.get(2).copied().unwrap_or("");

    // Validate npub format
    if !npub_input.starts_with("npub1") {
        msg.reply("⚠️ That doesn't look like a valid npub. Use npub1... or nostr:npub1...").await?;
        return Ok(());
    }

    // Validate sats amount
    let sats: u64 = match sats_str.parse() {
        Ok(n) if n > 0 => n,
        _ => {
            msg.reply(
                "⚠️ Please provide a valid positive number of sats.\nExample: !zap npub1... 21"
            ).await?;
            return Ok(());
        }
    };

    // Check wallet is initialized
    let wallet = match get_wallet(ctx) {
        Some(w) => w,
        None => {
            msg.reply("💰 Wallet not initialized. Ask the bot owner to enable it.").await?;
            return Ok(());
        }
    };

    // Get bot's nsec for signing the zap request
    let nsec = match ctx.config.bot_nsec() {
        Some(k) => k,
        None => {
            msg.reply("⚠️ Bot does not have an nsec configured. Cannot create zap requests.").await?;
            return Ok(());
        }
    };

    // Let user know it's processing (this takes several network calls)
    msg.reply("⚡ Creating NIP-57 zap...").await?;

    // -- Step 1: Resolve npub → hex pubkey --
    let hex_pubkey = match resolve_hex_pubkey(&npub_input).await {
        Ok(h) => h,
        Err(e) => {
            tracing::warn!("Failed to resolve npub {}: {}", npub_input, e);
            msg.reply(&format!("⚠️ Could not resolve npub \"{}\".", npub_input)).await?;
            return Ok(());
        }
    };

    // -- Step 2: Fetch kind 0 profile → extract lud16 --
    let lud16 = match fetch_lud16(&hex_pubkey).await {
        Ok(Some(l)) => l,
        Ok(None) => {
            msg.reply("⚠️ Recipient doesn't have a Lightning address configured.").await?;
            return Ok(());
        }
        Err(e) => {
            tracing::warn!("Failed to fetch profile for {}: {}", hex_pubkey, e);
            msg.reply("⚠️ Couldn't fetch recipient's Nostr profile. Try again later.").await?;
            return Ok(());
        }
    };

    // -- Step 3: Resolve LNURL pay endpoint --
    let (user, domain) = match lud16.split_once('@') {
        Some((u, d)) if !u.is_empty() && !d.is_empty() => (u, d),
        _ => {
            msg.reply("⚠️ Recipient has an invalid Lightning address.").await?;
            return Ok(());
        }
    };

    let lnurl_url = format!("https://{}/.well-known/lnurlp/{}", domain, user);

    let lnurl_data = match http::fetch_json(&lnurl_url).await {
        Ok(d) => d,
        Err(e) => {
            tracing::warn!("LNURL fetch failed for {}: {}", lud16, e);
            msg.reply("⚠️ Couldn't reach recipient's Lightning service.").await?;
            return Ok(());
        }
    };

    // Verify the service supports Nostr zaps
    if lnurl_data["allowsNostr"].as_bool() != Some(true) {
        msg.reply("⚠️ Recipient's Lightning service doesn't support Nostr zaps.").await?;
        return Ok(());
    }

    let callback = match lnurl_data["callback"].as_str() {
        Some(c) => c.to_string(),
        None => {
            msg.reply("⚠️ Recipient's Lightning service is misconfigured (no callback URL).").await?;
            return Ok(());
        }
    };

    let millisats = sats * 1000;

    // -- Step 4: Encode LNURL to bech32 (for the lnurl tag in zap request) --
    let lnurl_bech32 = encode_lnurl_bech32(&lnurl_url)
        .await
        .unwrap_or_default();

    // -- Step 5: Create and sign kind 9734 zap request --
    let zap_request = match create_sign_zap_request(
        &nsec,
        &hex_pubkey,
        millisats,
        zap_message,
        &lnurl_bech32,
    )
    .await
    {
        Ok(r) => r,
        Err(e) => {
            tracing::warn!("Failed to create zap request: {}", e);
            msg.reply("⚠️ Failed to create signed zap request.").await?;
            return Ok(());
        }
    };

    // -- Step 6: Get BOLT11 invoice from LNURL callback --
    let encoded_request = urlencoding::encode(&zap_request);
    let callback_url = format!(
        "{}?amount={}&nostr={}",
        callback, millisats, encoded_request
    );

    let invoice_data = match http::fetch_json(&callback_url).await {
        Ok(d) => d,
        Err(e) => {
            tracing::warn!("LNURL callback failed: {}", e);
            msg.reply("⚠️ Couldn't get invoice from recipient's Lightning service.").await?;
            return Ok(());
        }
    };

    let bolt11 = match invoice_data["pr"].as_str() {
        Some(p) if !p.is_empty() => p.to_string(),
        _ => {
            msg.reply("⚠️ Lightning service didn't return an invoice.").await?;
            return Ok(());
        }
    };

    // -- Step 7: Pay the invoice from the Cashu wallet --
    match wallet.withdraw(&bolt11).await {
        Ok(_) => {
            msg.reply(&format!(
                "⚡ Zapped {} {} sats! (NIP-57 receipt published by recipient's service)",
                &npub_input, sats
            )).await?;

            // Community XP: sender gets +5, recipient gets +10
            if let Some(ref sender_npub) = msg.message.npub {
                if let Err(e) = ctx.community_db.award_xp(sender_npub, 5, &msg.chat_id) {
                    tracing::warn!("Failed to award zap XP to sender: {}", e);
                }
                if let Err(e) = ctx.community_db.add_sats_zapped(sender_npub, sats as i64) {
                    tracing::warn!("Failed to track zapped sats: {}", e);
                }
            }
            // Award recipient XP (using npub_input as the recipient identifier)
            if let Err(e) = ctx.community_db.award_xp(&npub_input, 10, &msg.chat_id) {
                tracing::warn!("Failed to award zap XP to recipient: {}", e);
            }
        }
        Err(e) => {
            let err_text = format!("{:?}", e);
            if err_text.contains("Insufficient") {
                let balance = wallet.balance().await.unwrap_or(0);
                msg.reply(&format!(
                    "❌ Not enough sats in wallet. Current balance: {} sats",
                    balance
                )).await?;
            } else {
                tracing::warn!("Zap payment failed: {:?}", e);
                msg.reply(
                    "⚠️ Could not pay the Lightning invoice. The mint may be unreachable."
                ).await?;
            }
        }
    }

    Ok(())
}

// -----------------------------------------------------------------------------
// Zap helper functions
// -----------------------------------------------------------------------------

/// Resolve an npub to a hex pubkey using nak decode.
async fn resolve_hex_pubkey(input: &str) -> Result<String> {
    if input.starts_with("npub1") {
        let mut cmd = tokio::process::Command::new("nak");
        cmd.args(["decode", input])
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped());

        let output = tokio::time::timeout(Duration::from_secs(10), cmd.output()).await??;
        let stdout = String::from_utf8_lossy(&output.stdout);

        // Look for a 64-char hex string in the output
        for line in stdout.lines() {
            let trimmed = line.trim();
            if trimmed.len() == 64 && trimmed.chars().all(|c| c.is_ascii_hexdigit()) {
                return Ok(trimmed.to_lowercase());
            }
        }

        for word in stdout.split_whitespace() {
            let cleaned = word.trim_matches(|c: char| !c.is_ascii_hexdigit());
            if cleaned.len() == 64 && cleaned.chars().all(|c| c.is_ascii_hexdigit()) {
                return Ok(cleaned.to_lowercase());
            }
        }

        anyhow::bail!("Could not parse nak decode output: {}", stdout);
    } else if input.len() == 64 && input.chars().all(|c| c.is_ascii_hexdigit()) {
        return Ok(input.to_lowercase());
    } else {
        anyhow::bail!("Input is neither a valid npub nor hex pubkey: {}", input);
    }
}

/// Fetch a user's kind 0 metadata from Nostr relays and extract their lud16
/// (Lightning address in user@domain format).
async fn fetch_lud16(hex_pubkey: &str) -> Result<Option<String>> {
    let relay_args: Vec<&str> = READ_RELAYS.iter().copied().collect();
    let mut cmd = tokio::process::Command::new("nak");
    cmd.args(["req", "-k", "0", "-a", hex_pubkey])
        .args(&relay_args)
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped());

    let output = tokio::time::timeout(
        Duration::from_secs(NAK_TIMEOUT_SECS),
        cmd.output(),
    )
    .await??;

    let stdout = String::from_utf8_lossy(&output.stdout);

    // nak outputs NDJSON. Find the first valid event.
    let event = stdout
        .lines()
        .filter(|l| !l.trim().is_empty())
        .find_map(|line| serde_json::from_str::<serde_json::Value>(line).ok());

    let event = match event {
        Some(e) => e,
        None => return Ok(None), // No profile found
    };

    // Parse content JSON (kind 0 stores metadata as JSON in content field)
    let content_str = event["content"].as_str().unwrap_or("{}");
    let metadata: serde_json::Value = serde_json::from_str(content_str).unwrap_or_default();

    // Prefer lud16 (user@domain format)
    if let Some(lud16) = metadata["lud16"].as_str() {
        if !lud16.is_empty() {
            return Ok(Some(lud16.to_string()));
        }
    }

    // lud06 is legacy LNURL format — not decoded in this version
    // If only lud06 is present, treat as no Lightning address

    Ok(None)
}

/// Encode a URL to LNURL bech32 format using nak CLI.
/// Returns the lnurl1... encoded string.
async fn encode_lnurl_bech32(url: &str) -> Result<String> {
    let mut cmd = tokio::process::Command::new("nak");
    cmd.args(["encode", "lnurl", url])
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped());

    let output = tokio::time::timeout(Duration::from_secs(10), cmd.output()).await??;
    let stdout = String::from_utf8_lossy(&output.stdout);
    let result = stdout.trim().to_string();

    if result.starts_with("lnurl1") {
        Ok(result)
    } else {
        anyhow::bail!("Could not encode LNURL: {}", stdout)
    }
}

/// Create and sign a kind 9734 NIP-57 zap request event.
/// Returns the signed event JSON string.
async fn create_sign_zap_request(
    nsec: &str,
    recipient_hex: &str,
    millisats: u64,
    message: &str,
    lnurl_bech32: &str,
) -> Result<String> {
    // Build the tags for the zap request
    let mut tags = vec![
        vec![
            "relays".to_string(),
            "wss://relay.ditto.pub".to_string(),
            "wss://relay.primal.net".to_string(),
        ],
        vec!["amount".to_string(), millisats.to_string()],
        vec!["p".to_string(), recipient_hex.to_string()],
    ];

    // Add lnurl tag if we have the bech32-encoded value
    if !lnurl_bech32.is_empty() {
        tags.push(vec!["lnurl".to_string(), lnurl_bech32.to_string()]);
    }

    // Convert tags to JSON
    let tags_json: Vec<serde_json::Value> = tags
        .iter()
        .map(|tag| {
            serde_json::Value::Array(
                tag.iter()
                    .map(|s| serde_json::Value::String(s.clone()))
                    .collect(),
            )
        })
        .collect();

    let event_json = serde_json::json!({
        "kind": 9734,
        "content": message,
        "tags": tags_json,
    });

    let event_str = serde_json::to_string(&event_json)?;

    // Sign via nak event (pipe event JSON to stdin)
    let mut cmd = tokio::process::Command::new("nak");
    cmd.args(["event", "--sec", nsec])
        .args(WRITE_RELAYS)
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped());

    let mut child = cmd.spawn()?;

    // Write event JSON to nak's stdin
    if let Some(mut stdin) = child.stdin.take() {
        let _ = stdin.write_all(event_str.as_bytes()).await;
        // stdin drops here, signaling EOF
    }

    let output = tokio::time::timeout(
        Duration::from_secs(NAK_TIMEOUT_SECS),
        child.wait_with_output(),
    )
    .await??;

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);

    // nak event outputs the signed event JSON on stdout
    let signed = stdout
        .lines()
        .filter(|l| !l.trim().is_empty())
        .find_map(|line| {
            let trimmed = line.trim();
            if trimmed.starts_with('{') {
                if serde_json::from_str::<serde_json::Value>(trimmed).is_ok() {
                    return Some(trimmed.to_string());
                }
            }
            None
        });

    match signed {
        Some(s) => Ok(s),
        None => {
            tracing::warn!("nak event stdout: {}", stdout);
            tracing::warn!("nak event stderr: {}", stderr);
            anyhow::bail!("Could not parse signed zap request from nak output")
        }
    }
}

// -----------------------------------------------------------------------------
// Helpers
// -----------------------------------------------------------------------------

/// Get the wallet from context, or None if not initialized.
fn get_wallet(ctx: &BotContext) -> Option<&std::sync::Arc<crate::wallet::CashuWallet>> {
    ctx.wallet.as_ref()
}
