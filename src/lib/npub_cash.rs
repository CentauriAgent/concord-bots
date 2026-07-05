//! npub.cash client — claim pending Cashu tokens via NIP-98 auth.
//!
//! When someone zaps `<npub>@npub.cash`, npub.cash receives the Lightning payment,
//! mints Cashu tokens, and holds them. The bot claims them by authenticating
//! with a NIP-98 header (signed kind 27235 event) against `GET /api/v1/claim`.
//!
//! Reference: https://github.com/cashubtc/npubcash-server

use anyhow::{anyhow, Context, Result};
use serde_json::Value;

use crate::lib::http;
use crate::lib::nip98;

/// Result of a claim attempt.
#[derive(Debug, Clone)]
pub struct ClaimResult {
    /// Cashu token strings ready to receive into the wallet.
    pub tokens: Vec<String>,
    /// Total sats across all tokens (0 if parsing failed).
    pub total_sats: u64,
}

/// Claim all pending tokens from npub.cash.
///
/// Uses NIP-98 auth (kind 27235 Nostr event signed with the bot's nsec).
/// Returns the token strings; empty if nothing was pending.
pub async fn claim(base_url: &str, nsec: &str) -> Result<ClaimResult> {
    let url = format!("{}/api/v1/claim", base_url.trim_end_matches('/'));
    let auth = nip98::build_auth_header(&url, "GET", None, nsec)
        .await
        .context("Failed to build NIP-98 auth header")?;

    let resp = http::client()
        .get(&url)
        .header("Authorization", auth)
        .send()
        .await
        .with_context(|| format!("HTTP GET failed: {}", url))?;

    let status = resp.status();
    if !status.is_success() {
        let body = resp.text().await.unwrap_or_default();
        return Err(anyhow!("HTTP {} for {}: {}", status, url, body));
    }

    let body: Value = resp.json().await.context("Failed to parse claim response as JSON")?;

    // npub.cash response shapes:
    //   No tokens:  {"error": true, "message": "No proofs to claim"}
    //   Success:    {"error": false, "data": "<cashu-token-string>"}
    //               (or data may be an array of token strings)
    if body.get("error").and_then(|e| e.as_bool()) == Some(true) {
        let msg = body.get("message").and_then(|m| m.as_str()).unwrap_or("unknown error");
        // "No proofs to claim" is the normal empty case — return empty rather than error.
        if msg.contains("No proofs") || msg.contains("nothing") {
            return Ok(ClaimResult { tokens: vec![], total_sats: 0 });
        }
        return Err(anyhow!("npub.cash claim error: {}", msg));
    }

    // Extract token strings from the response.
    let mut tokens: Vec<String> = Vec::new();
    if let Some(data_str) = body.get("data").and_then(|d| d.as_str()) {
        tokens.push(data_str.to_string());
    } else if let Some(data_arr) = body.get("data").and_then(|d| d.as_array()) {
        for item in data_arr {
            if let Some(s) = item.as_str() {
                tokens.push(s.to_string());
            }
        }
    }

    // Best-effort total — we won't parse the CBOR token here; the wallet will report.
    tracing::debug!("Claim returned {} token(s) from npub.cash", tokens.len());

    Ok(ClaimResult {
        total_sats: 0, // actual amount reported by wallet.receive() in caller
        tokens,
    })
}

/// Query the bot's npub.cash balance (unclaimed sats still held by the service).
pub async fn balance(base_url: &str, nsec: &str) -> Result<u64> {
    let url = format!("{}/api/v1/balance", base_url.trim_end_matches('/'));
    let auth = nip98::build_auth_header(&url, "GET", None, nsec)
        .await
        .context("Failed to build NIP-98 auth header")?;

    let resp = http::client()
        .get(&url)
        .header("Authorization", auth)
        .send()
        .await
        .with_context(|| format!("HTTP GET failed: {}", url))?;

    let status = resp.status();
    if !status.is_success() {
        let body = resp.text().await.unwrap_or_default();
        return Err(anyhow!("HTTP {} for {}: {}", status, url, body));
    }

    let body: Value = resp.json().await?;
    let bal = body.get("data").and_then(|d| d.as_u64()).unwrap_or(0);
    Ok(bal)
}
