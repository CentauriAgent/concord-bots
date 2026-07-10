// =============================================================================
// lib/http.rs — HTTP fetch helper
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_client_exists() {
        let _ = &*CLIENT;
    }
}
