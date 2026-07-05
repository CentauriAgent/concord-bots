//! NIP-98 HTTP Authentication — build `Authorization: Nostr <token>` headers.
//!
//! Spec: https://github.com/nostr-protocol/nips/blob/master/98.md

use anyhow::{Context, Result};
use base64::Engine;
use nostr_sdk::prelude::*;
use sha2::{Digest, Sha256};

pub async fn build_auth_header(
    url: &str,
    method: &str,
    payload: Option<&[u8]>,
    nsec: &str,
) -> Result<String> {
    let keys = Keys::parse(nsec).context("Failed to parse nsec")?;

    let mut tags: Vec<Tag> = vec![
        Tag::custom(TagKind::custom("u"), vec![url.to_string()]),
        Tag::custom(TagKind::custom("method"), vec![method.to_string()]),
    ];

    if let Some(body) = payload {
        let hash = Sha256::digest(body);
        tags.push(Tag::custom(
            TagKind::custom("payload"),
            vec![hex::encode(hash)],
        ));
    }

    let event = EventBuilder::new(Kind::Custom(27235), "")
        .tags(tags)
        .sign(&keys)
        .await
        .context("Failed to sign NIP-98 event")?;;

    let json = serde_json::to_string(&event).context("Failed to serialize NIP-98 event")?;
    let b64 = base64::engine::general_purpose::STANDARD.encode(json);

    Ok(format!("Nostr {}", b64))
}
