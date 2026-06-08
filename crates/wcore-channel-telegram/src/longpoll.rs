//! Background long-poll task. Spawned by `TelegramChannel::start()`,
//! signaled to exit by the watch channel in `TelegramChannel`.

use std::collections::{HashSet, VecDeque};
use std::sync::Arc;
use std::time::Duration;

use tokio::sync::{Mutex, watch};
use wcore_channels::event::{ChannelEvent, IncomingMessage};

use crate::api::{Update, get_updates};

/// Constructor arguments — flatter than a struct, easier to spawn.
pub(crate) struct LongPollArgs {
    pub http: wcore_egress::EgressClient,
    pub api_base: String,
    pub bot_token: String,
    pub timeout_secs: u32,
    pub allowed_chat_ids: HashSet<String>,
    pub inbox: Arc<Mutex<VecDeque<ChannelEvent>>>,
    pub shutdown: watch::Receiver<bool>,
}

/// Drive `getUpdates` in a loop until the shutdown signal flips.
///
/// Backoff on transient failures stays small (2s + jitter-free) — the
/// caller's poll cadence is the load-bearing knob, not this loop's.
pub(crate) async fn longpoll_loop(args: LongPollArgs) {
    let LongPollArgs {
        http,
        api_base,
        bot_token,
        timeout_secs,
        allowed_chat_ids,
        inbox,
        mut shutdown,
    } = args;

    let mut offset: i64 = 0;
    let mut consecutive_failures: u32 = 0;

    loop {
        if *shutdown.borrow() {
            break;
        }

        // Race the next API call against a shutdown signal so we don't
        // get stuck for ~timeout_secs after stop() flips the flag.
        let updates = tokio::select! {
            biased;
            _ = shutdown.changed() => {
                if *shutdown.borrow() { break; }
                continue;
            }
            r = get_updates(&http, &api_base, &bot_token, offset, timeout_secs) => r,
        };

        match updates {
            Ok(updates) => {
                consecutive_failures = 0;
                ingest_updates(updates, &allowed_chat_ids, &inbox, &mut offset).await;
            }
            Err(e) => {
                tracing::warn!(
                    target: "wcore_channel_telegram::longpoll",
                    error = %e,
                    "getUpdates failed; backing off"
                );
                consecutive_failures = consecutive_failures.saturating_add(1);
                // Linear cap at 30s — same family as the send retry cap
                // but without the exponential bias (the poll loop is
                // self-correcting; tight failure loops here are usually
                // a transient outage, not a coding error).
                let sleep_secs = (2_u64.saturating_mul(consecutive_failures as u64)).min(30);
                tokio::select! {
                    biased;
                    _ = shutdown.changed() => {
                        if *shutdown.borrow() { break; }
                    }
                    _ = tokio::time::sleep(Duration::from_secs(sleep_secs)) => {}
                }
            }
        }
    }
}

async fn ingest_updates(
    updates: Vec<Update>,
    allowed_chat_ids: &HashSet<String>,
    inbox: &Arc<Mutex<VecDeque<ChannelEvent>>>,
    offset: &mut i64,
) {
    if updates.is_empty() {
        return;
    }
    let mut events = Vec::with_capacity(updates.len());
    for u in updates {
        // Advance offset past every Update we see, even ones we drop —
        // otherwise we'd loop on the same filtered-out message forever.
        *offset = (*offset).max(u.update_id + 1);

        let Some(msg) = u.message else { continue };
        let chat_id_str = msg.chat.id.to_string();
        if !allowed_chat_ids.is_empty() && !allowed_chat_ids.contains(&chat_id_str) {
            continue;
        }
        let author = msg
            .from
            .as_ref()
            .and_then(|f| {
                f.username
                    .clone()
                    .or_else(|| f.first_name.clone())
                    .or_else(|| Some(f.id.to_string()))
            })
            .unwrap_or_else(|| "unknown".to_string());
        let text = msg.text.unwrap_or_default();
        events.push(ChannelEvent::MessageReceived {
            msg: IncomingMessage {
                id: msg.message_id.to_string(),
                conversation_id: chat_id_str,
                author,
                text,
                ts_secs: msg.date,
                attachments: Vec::new(),
            },
        });
    }
    if !events.is_empty() {
        let mut guard = inbox.lock().await;
        for e in events {
            guard.push_back(e);
        }
    }
}
