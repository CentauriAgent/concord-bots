// =============================================================================
// lib/http.rs — HTTP fetch helper (STABLE — do not edit)
// =============================================================================
//
// Simple HTTP helpers for fetching JSON from external APIs.
// Uses reqwest with a shared client instance for connection pooling.

use anyhow::{Context, Result};
use once_cell::sync::Lazy;
use serde_json::Value;

static CLIENT: Lazy<reqwest::Client> = Lazy::new(|| {
    reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(30))
        .user_agent(format!("concord-bots/{}", env!("CARGO_PKG_VERSION")))
        .build()
        .expect("Failed to build HTTP client")
});

/// Get a reference to the shared HTTP client (for custom request building).
pub fn client() -> &'static reqwest::Client {
    &CLIENT
}

/// Fetch a URL and parse the response as JSON.
///
/// # Example
/// ```ignore
/// let data = fetch_json("https://api.coingecko.com/api/v3/simple/price").await?;
/// let price = data["bitcoin"]["usd"].as_f64().unwrap_or(0.0);
/// ```
pub async fn fetch_json(url: &str) -> Result<Value> {
    tracing::debug!("HTTP GET: {}", url);
    let resp = CLIENT
        .get(url)
        .send()
        .await
        .with_context(|| format!("HTTP GET failed: {}", url))?;

    let status = resp.status();
    if !status.is_success() {
        let body = resp.text().await.unwrap_or_default();
        anyhow::bail!("HTTP {} for {}: {}", status, url, body);
    }

    let value = resp
        .json::<Value>()
        .await
        .with_context(|| format!("Failed to parse JSON from {}", url))?;

    Ok(value)
}

/// Fetch a URL with a Bearer token and parse as JSON.
pub async fn fetch_json_with_auth(url: &str, token: Option<&str>) -> Result<Value> {
    tracing::debug!("HTTP GET (auth): {}", url);
    let mut req = CLIENT.get(url);
    if let Some(t) = token {
        req = req.header("Authorization", format!("Bearer {}", t));
    }
    let resp = req
        .send()
        .await
        .with_context(|| format!("HTTP GET failed: {}", url))?;

    let status = resp.status();
    if !status.is_success() {
        let body = resp.text().await.unwrap_or_default();
        anyhow::bail!("HTTP {} for {}: {}", status, url, body);
    }

    Ok(resp.json::<Value>().await?)
}

/// POST JSON to a URL and return the response as JSON.
pub async fn post_json(url: &str, body: &Value) -> Result<Value> {
    tracing::debug!("HTTP POST: {}", url);
    let resp = CLIENT
        .post(url)
        .json(body)
        .send()
        .await
        .with_context(|| format!("HTTP POST failed: {}", url))?;

    let status = resp.status();
    if !status.is_success() {
        let resp_body = resp.text().await.unwrap_or_default();
        anyhow::bail!("HTTP {} for {}: {}", status, url, resp_body);
    }

    Ok(resp.json::<Value>().await?)
}

/// Fetch plain text from a URL (not JSON).
pub async fn fetch_text(url: &str) -> Result<String> {
    tracing::debug!("HTTP GET (text): {}", url);
    let resp = CLIENT
        .get(url)
        .send()
        .await
        .with_context(|| format!("HTTP GET failed: {}", url))?;

    let status = resp.status();
    if !status.is_success() {
        anyhow::bail!("HTTP {} for {}", status, url);
    }

    Ok(resp.text().await?)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_client_exists() {
        // Just verify the client is initialized.
        let _ = &*CLIENT;
    }
}
