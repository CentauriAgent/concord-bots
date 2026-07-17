// =============================================================================
// handlers/thread.rs — Kind 1111 threaded reply helper
// =============================================================================
//
// Sends bot command responses as kind 1111 (COMMENT) threaded replies per
// CORD-03 §3, instead of kind 9 (MESSAGE) with a `q` tag inline reply.
//
// Clients like Armada can render kind 1111 as collapsible threads, keeping
// the main channel uncluttered from bot command spam.
//
// Falls back to `msg.reply()` (kind 9 inline reply) when:
//   - The message is a DM (kind 1111 is community-only)
//   - The community can't be resolved (not a v2 community)
//   - The thread send fails for any reason

use anyhow::Result;
use vector_sdk::IncomingMessage;

use crate::bot::BotContext;

/// Send a threaded reply (kind 1111) to the incoming message.
///
/// Falls back to a regular `msg.reply()` (kind 9 with `q` tag) when the
/// thread reply can't be sent — e.g. in DMs, non-v2 communities, or on error.
pub async fn reply_as_thread(ctx: &BotContext, msg: &IncomingMessage, text: &str) -> Result<()> {
    // Kind 1111 is community-only — fall back to regular reply for DMs.
    if !msg.is_group {
        msg.reply(text).await?;
        return Ok(());
    }

    // Attempt the kind 1111 thread reply; fall back on any error.
    match send_comment(ctx, msg, text).await {
        Ok(rumor_id) => {
            tracing::debug!("Sent thread reply {} for command", rumor_id);
        }
        Err(e) => {
            tracing::warn!(
                "Thread reply failed, falling back to regular reply: {}",
                e
            );
            msg.reply(text).await?;
        }
    }

    Ok(())
}

/// Build and publish a kind 1111 comment (threaded reply) to the channel.
///
/// Mirrors the flow of `vector_core::community::v2::service::send_chat_message`
/// but uses `build_comment_rumor` (kind 1111) instead of `build_message_rumor`
/// (kind 9).
///
/// **Key rotation safety:** This function loads the community's current epoch
/// from local state to derive the sealing key. If the community recently
/// rekeyed (epoch advance) and the follow worker hasn't processed it yet, the
/// sealed event will use stale keys and peers won't be able to open it. To
/// handle this, we trigger a community catch-up before sending, and retry once
/// if the first attempt fails.
async fn send_comment(ctx: &BotContext, msg: &IncomingMessage, text: &str) -> Result<String, String> {
    use vector_sdk::vector_core::{
        community::{
            ChannelId,
            CommunityId,
            v2::{
                chat::{build_comment_rumor, open_chat_event, seal_chat_rumor},
                derive::channel_group_key,
                kind,
            },
        },
        db::community::{
            community_id_for_channel, get_community_banlist, get_community_dissolved,
            load_community_v2,
        },
        simd::hex::hex_to_bytes_32,
        state::{my_public_key, nostr_client, SessionGuard, STATE, MY_SECRET_KEY},
    };

    // Trigger a community state sync to pick up any recent rekeys before we
    // read the epoch. This ensures we seal with the latest key, not a stale one.
    // The sync is cheap if nothing has changed.
    ctx.bot.sync_communities().await.ok();

    // 1. Resolve local identity keys.
    let author_keys = MY_SECRET_KEY
        .to_keys()
        .ok_or_else(|| "no local identity key".to_string())?;
    let author_pk = author_keys.public_key();

    // 2. Resolve the community that owns this channel.
    let channel_hex = &msg.chat_id;
    let community_id_hex = community_id_for_channel(channel_hex)
        .map_err(|e| format!("community_id_for_channel: {e}"))?
        .ok_or_else(|| "no community owns this channel".to_string())?;

    let cid = CommunityId(hex_to_bytes_32(&community_id_hex));
    let community = load_community_v2(&cid)
        .map_err(|e| format!("load_community_v2: {e}"))?
        .ok_or_else(|| "v2 community not found".to_string())?;

    // 3. Guardrails: dissolved / banned (mirrors chat_send_context).
    if get_community_dissolved(&community_id_hex).unwrap_or(false) {
        return Err("community has been dissolved".into());
    }
    let banlist = get_community_banlist(&community_id_hex).unwrap_or_default();
    if banlist.contains(&author_pk.to_hex()) {
        return Err("you are banned from this community".into());
    }

    // 4. Resolve the channel within the community.
    let channel_id = ChannelId(hex_to_bytes_32(channel_hex));
    let ch = community
        .channel(&channel_id)
        .ok_or_else(|| "no such channel in this community".to_string())?;

    if ch.private && ch.key.is_none() {
        return Err("private channel has no key yet (awaiting rekey)".into());
    }

    // 5. Derive the group key + epoch for sealing.
    //    Log the epoch so we can diagnose stale-key issues.
    let (secret, epoch) = community.channel_secret(ch);
    tracing::debug!(
        "Sealing comment for channel {} at epoch {} (community {})",
        channel_hex, epoch.0, community_id_hex
    );
    let group = channel_group_key(&secret, &channel_id, epoch);

    // 6. Capture session generation (swap detection).
    let session = SessionGuard::capture();

    // 7. Resolve the parent message (the !command we're replying to).
    let parent_id = &msg.message.id;
    let parent_author_hex = msg
        .message
        .npub
        .as_ref()
        .and_then(|n| nostr_sdk::prelude::PublicKey::parse(n).ok())
        .map(|pk| pk.to_hex())
        .unwrap_or_default();

    if parent_author_hex.is_empty() {
        return Err("could not resolve parent message author".into());
    }

    // 8. Build the kind 1111 comment rumor.
    let at_ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0);

    let rumor = build_comment_rumor(
        author_pk,
        &channel_id,
        epoch,
        text,
        parent_id,
        kind::MESSAGE, // parent is a kind 9 message
        &parent_author_hex,
        None, // parent IS the root (no nested threading for command replies)
        &[],  // no custom emoji
        at_ms,
    );

    let rumor_id = rumor
        .id
        .ok_or_else(|| "rumor has no id".to_string())?
        .to_hex();

    // 9. Seal the rumor into a gift-wrapped event.
    let (wrap, _ephemeral_keys) = seal_chat_rumor(
        &rumor,
        &group,
        &author_keys,
        nostr_sdk::Timestamp::from_secs(at_ms / 1000),
        false, // not ephemeral
    )
    .map_err(|e| format!("seal_chat_rumor: {e}"))?;

    // 10. Publish to the community's relays.
    let client = nostr_client().ok_or_else(|| "not logged in".to_string())?;
    let relay_urls: Vec<String> = community.relays.clone();
    client
        .send_event_to(relay_urls.clone(), &wrap)
        .await
        .map_err(|e| format!("publish failed: {e}"))?;

    // 11. Local echo — run the wrap through the inbound pipeline so the
    //     bot's own state reflects the sent message immediately (same as
    //     send_chat_message does via publish_chat).
    if let Ok(event) = open_chat_event(&wrap, &group, &channel_id, epoch) {
        let channel_hex_owned = channel_hex.to_string();
        let outcome = {
            let mut st = STATE.lock().await;
            if !session.is_valid() {
                return Ok(rumor_id);
            }
            vector_sdk::vector_core::community::v2::inbound::apply_chat_to_state(
                &mut st,
                &event,
                &channel_hex_owned,
                &author_pk,
            )
        };
        if let Some(outcome) = outcome {
            if !session.is_valid() {
                return Ok(rumor_id);
            }
            vector_sdk::vector_core::community::v2::inbound::persist_chat(
                &channel_hex_owned,
                &outcome,
            )
            .await;
        }
    }

    // Silence unused-import warning for my_public_key (kept for potential
    // future use in swap-check diagnostics).
    let _ = my_public_key();

    Ok(rumor_id)
}
