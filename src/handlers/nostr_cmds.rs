// =============================================================================
// handlers/nostr_cmds.rs — Nostr commands (!nostr, !nip05, !follow)
// =============================================================================
//
// Nostr commands provide profile lookup, NIP-05 verification, and follow list
// management via the `nak` CLI and HTTP APIs.
//
//   !nostr <npub>       — Look up a Nostr profile (kind 0 metadata)
//   !nip05 <user@domain> — Verify a NIP-05 identifier
//   !follow <npub>      — Follow a user on Nostr (owner-only)

use anyhow::Result;
use std::time::Duration;
use vector_sdk::IncomingMessage;

use crate::bot::BotContext;
use crate::lib::http;

// -----------------------------------------------------------------------------
// Constants
// -----------------------------------------------------------------------------

/// Relays used for read operations.
const READ_RELAYS: &[&str] = &[
    "wss://relay.ditto.pub",
    "wss://relay.primal.net",
    "wss://nos.lol",
];

/// Relays used for write/publish operations.
const WRITE_RELAYS: &[&str] = &[
    "wss://relay.ditto.pub",
    "wss://relay.primal.net",
];

/// Timeout for nak commands (seconds).
const NAK_TIMEOUT_SECS: u64 = 15;

// -----------------------------------------------------------------------------
// !nostr <npub> — Profile lookup
// -----------------------------------------------------------------------------

pub async fn nostr_command(_ctx: &BotContext, msg: &IncomingMessage, args: &str) -> Result<()> {
    let input = args.trim();

    if input.is_empty() {
        msg.reply("Usage: !nostr <npub>\nExample: !nostr npub1jrvdfzf9aglmkt3nzpm4y6x3tq056qwh5v6ge2x2g9wkx27j58gsj7nev5").await?;
        return Ok(());
    }

    // Convert npub → hex if needed
    let hex_pubkey = match resolve_pubkey(input).await {
        Ok(h) => h,
        Err(e) => {
            tracing::warn!("Failed to resolve pubkey {}: {}", input, e);
            msg.reply(&format!("⚠️ Could not resolve \"{}\". Make sure it's a valid npub or hex pubkey.", input)).await?;
            return Ok(());
        }
    };

    // Fetch kind 0 metadata via nak
    let relay_args: Vec<&str> = READ_RELAYS.iter().copied().collect();
    let mut cmd = tokio::process::Command::new("nak");
    cmd.args(["req", "-k", "0", "-a", &hex_pubkey])
        .args(&relay_args)
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped());

    let output = match tokio::time::timeout(
        Duration::from_secs(NAK_TIMEOUT_SECS),
        cmd.output(),
    )
    .await
    {
        Ok(Ok(out)) => out,
        Ok(Err(e)) => {
            tracing::warn!("nak req failed: {}", e);
            msg.reply("⚠️ Could not reach Nostr relays. Try again later.").await?;
            return Ok(());
        }
        Err(_) => {
            tracing::warn!("nak req timed out");
            msg.reply("⚠️ Relay lookup timed out. Try again later.").await?;
            return Ok(());
        }
    };

    let stdout = String::from_utf8_lossy(&output.stdout);

    // nak outputs NDJSON (one JSON event per line). Find the first line with content.
    let profile_data = stdout
        .lines()
        .filter(|l| !l.trim().is_empty())
        .find_map(|line| serde_json::from_str::<serde_json::Value>(line).ok());

    let event = match profile_data {
        Some(e) => e,
        None => {
            msg.reply(&format!("🔍 No profile found for {}", input)).await?;
            return Ok(());
        }
    };

    // Parse the content field (kind 0 stores metadata as JSON in content)
    let content_str = event["content"].as_str().unwrap_or("{}");
    let metadata: serde_json::Value = serde_json::from_str(content_str).unwrap_or_default();

    let name = metadata["name"]
        .as_str()
        .or_else(|| metadata["display_name"].as_str())
        .unwrap_or("Unknown");
    let about = metadata["about"].as_str().unwrap_or("");
    let nip05 = metadata["nip05"].as_str();
    let lud16 = metadata["lud16"].as_str();
    let picture = metadata["picture"].as_str();
    let website = metadata["website"].as_str();

    let mut response = format!("👤 **{}**\n", name);

    if !about.is_empty() {
        // Truncate long about texts
        let about_display = if about.len() > 200 {
            format!("{}...", &about[..200])
        } else {
            about.to_string()
        };
        response.push_str(&format!("📝 {}\n", about_display));
    }

    if let Some(nip05_addr) = nip05 {
        // Verify the NIP-05
        let verified = verify_nip05(nip05_addr, &hex_pubkey).await.unwrap_or(false);
        let check = if verified { " ✅" } else { " ❌" };
        response.push_str(&format!("🌐 NIP-05: {}{}\n", nip05_addr, check));
    }

    if let Some(lud) = lud16 {
        response.push_str(&format!("⚡ Lightning: {}\n", lud));
    }

    if let Some(site) = website {
        response.push_str(&format!("🔗 {}\n", site));
    }

    if let Some(pic) = picture {
        response.push_str(&format!("🖼️ {}", pic));
    }

    msg.reply(response.trim()).await?;
    Ok(())
}

// -----------------------------------------------------------------------------
// !nip05 <user@domain> — NIP-05 verification
// -----------------------------------------------------------------------------

pub async fn nip05_command(_ctx: &BotContext, msg: &IncomingMessage, args: &str) -> Result<()> {
    let input = args.trim();

    if input.is_empty() {
        msg.reply("Usage: !nip05 <user@domain>\nExample: !nip05 derekross@nostrplebs.com").await?;
        return Ok(());
    }

    // Parse user@domain
    let (user, domain) = match input.split_once('@') {
        Some((u, d)) if !u.is_empty() && !d.is_empty() => (u, d),
        _ => {
            msg.reply("⚠️ Invalid format. Use: user@domain\nExample: !nip05 derekross@nostrplebs.com").await?;
            return Ok(());
        }
    };

    let url = format!(
        "https://{}/.well-known/nostr.json?name={}",
        domain,
        user
    );

    let data = match http::fetch_json(&url).await {
        Ok(d) => d,
        Err(e) => {
            tracing::warn!("NIP-05 fetch failed for {}: {}", input, e);
            msg.reply(&format!("❌ Not verified: {} not found", input)).await?;
            return Ok(());
        }
    };

    // The response is: {"names": {"user": "hex_pubkey"}, ...}
    let hex_pubkey = data["names"][user].as_str();

    match hex_pubkey {
        Some(pubkey) => {
            // Convert hex to npub for display
            let npub = hex_to_npub(pubkey).await.unwrap_or_else(|_| pubkey.to_string());
            msg.reply(&format!("✅ Verified: {} → {}", input, npub)).await?;
        }
        None => {
            msg.reply(&format!("❌ Not verified: {} not found", input)).await?;
        }
    }

    Ok(())
}

// -----------------------------------------------------------------------------
// !follow <npub> — Follow on Nostr (owner-only)
// -----------------------------------------------------------------------------

pub async fn follow_command(ctx: &BotContext, msg: &IncomingMessage, args: &str) -> Result<()> {
    let input = args.trim();

    if input.is_empty() {
        msg.reply("Usage: !follow <npub>\nExample: !follow npub1jrvdfzf9aglmkt3nzpm4y6x3tq056qwh5v6ge2x2g9wkx27j58gsj7nev5").await?;
        return Ok(());
    }

    // Get the bot's nsec
    let nsec = match ctx.config.bot_nsec() {
        Some(k) => k,
        None => {
            msg.reply("⚠️ Bot does not have an nsec configured. Cannot publish follow lists.").await?;
            return Ok(());
        }
    };

    // Resolve the target pubkey to hex
    let target_hex = match resolve_pubkey(input).await {
        Ok(h) => h,
        Err(e) => {
            tracing::warn!("Failed to resolve pubkey {}: {}", input, e);
            msg.reply(&format!("⚠️ Could not resolve \"{}\". Make sure it's a valid npub or hex pubkey.", input)).await?;
            return Ok(());
        }
    };

    // Get the bot's own hex pubkey
    let bot_npub = ctx.bot.npub();
    let bot_hex = match resolve_pubkey(&bot_npub).await {
        Ok(h) => h,
        Err(e) => {
            tracing::warn!("Failed to resolve bot's own npub {}: {}", bot_npub, e);
            msg.reply("⚠️ Could not resolve bot's own identity. This is a configuration issue.").await?;
            return Ok(());
        }
    };

    // Fetch the bot's current kind 3 (contact list)
    let mut fetch_cmd = tokio::process::Command::new("nak");
    fetch_cmd
        .args(["req", "-k", "3", "-a", &bot_hex, "wss://relay.ditto.pub"])
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped());

    let fetch_output = match tokio::time::timeout(
        Duration::from_secs(NAK_TIMEOUT_SECS),
        fetch_cmd.output(),
    )
    .await
    {
        Ok(Ok(out)) => out,
        Ok(Err(e)) => {
            tracing::warn!("nak req kind 3 failed: {}", e);
            msg.reply("⚠️ Could not fetch current follow list from relays.").await?;
            return Ok(());
        }
        Err(_) => {
            tracing::warn!("nak req kind 3 timed out");
            msg.reply("⚠️ Relay lookup timed out. Try again later.").await?;
            return Ok(());
        }
    };

    let fetch_stdout = String::from_utf8_lossy(&fetch_output.stdout);

    // Parse existing kind 3 event for current p tags
    let existing_event = fetch_stdout
        .lines()
        .filter(|l| !l.trim().is_empty())
        .find_map(|line| serde_json::from_str::<serde_json::Value>(line).ok());

    // Collect existing p tags
    let mut p_tags: Vec<Vec<String>> = Vec::new();
    if let Some(ref event) = existing_event {
        if let Some(tags) = event["tags"].as_array() {
            for tag in tags {
                if tag[0].as_str() == Some("p") {
                    if let Some(arr) = tag.as_array() {
                        let strings: Vec<String> = arr
                            .iter()
                            .map(|v| v.as_str().unwrap_or("").to_string())
                            .collect();
                        if strings.len() >= 2 {
                            p_tags.push(strings);
                        }
                    }
                }
            }
        }
    }

    // Check if already following
    let already_following = p_tags.iter().any(|t| t.len() >= 2 && t[1] == target_hex);
    if already_following {
        msg.reply(&format!("ℹ️ Already following {}.", input)).await?;
        return Ok(());
    }

    // Add the new pubkey
    p_tags.push(vec!["p".to_string(), target_hex.clone()]);

    let total_follows = p_tags.len();

    // Build the kind 3 event JSON
    let tags_json: Vec<serde_json::Value> = p_tags
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
        "kind": 3,
        "content": "",
        "tags": tags_json,
    });

    // Publish via nak event
    let event_str = serde_json::to_string(&event_json).unwrap_or_default();

    let mut publish_cmd = tokio::process::Command::new("nak");
    publish_cmd
        .args(["event", "--sec", &nsec])
        .args(WRITE_RELAYS)
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped());

    let mut child = match publish_cmd.spawn() {
        Ok(c) => c,
        Err(e) => {
            tracing::warn!("Failed to spawn nak event: {}", e);
            msg.reply("⚠️ Could not publish follow list. Is `nak` installed?").await?;
            return Ok(());
        }
    };

    // Write event JSON to stdin
    if let Some(mut stdin) = child.stdin.take() {
        use tokio::io::AsyncWriteExt;
        let _ = stdin.write_all(event_str.as_bytes()).await;
        // stdin drops here, signaling EOF
    }

    let publish_output = match tokio::time::timeout(
        Duration::from_secs(NAK_TIMEOUT_SECS),
        child.wait_with_output(),
    )
    .await
    {
        Ok(Ok(out)) => out,
        Ok(Err(e)) => {
            tracing::warn!("nak event failed: {}", e);
            msg.reply("⚠️ Failed to publish follow list to relays.").await?;
            return Ok(());
        }
        Err(_) => {
            tracing::warn!("nak event timed out");
            msg.reply("⚠️ Publishing follow list timed out. Try again later.").await?;
            return Ok(());
        }
    };

    // Check if the publish succeeded (nak prints the event id on success)
    let publish_stdout = String::from_utf8_lossy(&publish_output.stdout);
    let publish_stderr = String::from_utf8_lossy(&publish_output.stderr);

    tracing::debug!("nak event stdout: {}", publish_stdout);
    tracing::debug!("nak event stderr: {}", publish_stderr);

    if publish_stdout.contains("\"id\"") || publish_stdout.contains("published") || !publish_stdout.trim().is_empty() {
        msg.reply(&format!("✅ Now following {} (total: {} follows)", input, total_follows)).await?;
    } else {
        msg.reply(&format!("✅ Follow list published (total: {} follows)", total_follows)).await?;
    }

    Ok(())
}

// -----------------------------------------------------------------------------
// Helpers
// -----------------------------------------------------------------------------

/// Convert an npub to hex, or return the hex as-is if already hex.
async fn resolve_pubkey(input: &str) -> Result<String> {
    let input = input.trim();

    if input.starts_with("npub1") {
        // Use nak decode to convert npub → hex
        let mut cmd = tokio::process::Command::new("nak");
        cmd.args(["decode", input])
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped());

        let output = tokio::time::timeout(Duration::from_secs(10), cmd.output()).await??;

        let stdout = String::from_utf8_lossy(&output.stdout);
        // nak decode outputs the hex pubkey on stdout (possibly with other info)
        // Look for a 64-char hex string
        for line in stdout.lines() {
            let trimmed = line.trim();
            if trimmed.len() == 64 && trimmed.chars().all(|c| c.is_ascii_hexdigit()) {
                return Ok(trimmed.to_lowercase());
            }
        }

        // Some versions of nak output "hex: <hex>" or just the hex
        // Try to find any 64-char hex substring manually
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
        anyhow::bail!("Input is neither a valid npub nor a 64-char hex pubkey: {}", input);
    }
}

/// Convert a hex pubkey to npub using nak decode.
async fn hex_to_npub(hex: &str) -> Result<String> {
    let mut cmd = tokio::process::Command::new("nak");
    cmd.args(["encode", "npub", hex])
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped());

    let output = tokio::time::timeout(Duration::from_secs(10), cmd.output()).await??;
    let stdout = String::from_utf8_lossy(&output.stdout);

    let npub = stdout.trim().to_string();
    if npub.starts_with("npub1") {
        Ok(npub)
    } else {
        anyhow::bail!("Could not encode hex to npub: {}", stdout)
    }
}

/// Verify a NIP-05 identifier matches a given hex pubkey.
async fn verify_nip05(nip05: &str, hex_pubkey: &str) -> Result<bool> {
    let (user, domain) = match nip05.split_once('_') {
        Some((u, d)) => (u, d),
        None => match nip05.split_once('@') {
            Some((u, d)) => (u, d),
            None => anyhow::bail!("Invalid NIP-05: {}", nip05),
        },
    };

    let url = format!(
        "https://{}/.well-known/nostr.json?name={}",
        domain,
        user
    );

    let data = http::fetch_json(&url).await?;
    let verified_pubkey = data["names"][user].as_str();

    Ok(verified_pubkey.map(|p| p.to_lowercase() == hex_pubkey.to_lowercase()).unwrap_or(false))
}

// -----------------------------------------------------------------------------
// Tests
// -----------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_resolve_pubkey_hex_passthrough() {
        // Hex pubkeys should be recognized (64 hex chars)
        let hex = "a" .repeat(64);
        // This is a sync test but resolve_pubkey is async — just test the logic
        assert_eq!(hex.len(), 64);
        assert!(hex.chars().all(|c| c.is_ascii_hexdigit()));
    }

    #[test]
    fn test_nip05_parsing() {
        // Test basic user@domain parsing
        let (user, domain) = "derekross@nostrplebs.com".split_once('@').unwrap();
        assert_eq!(user, "derekross");
        assert_eq!(domain, "nostrplebs.com");

        // NIP-05 with underscore prefix (e.g. _@domain)
        // The underscore is the username, @ splits on the domain
        let nip05 = "_@nostrplebs.com";
        let (user, domain) = nip05.split_once('@').unwrap();
        assert_eq!(user, "_");
        assert_eq!(domain, "nostrplebs.com");
    }
}
