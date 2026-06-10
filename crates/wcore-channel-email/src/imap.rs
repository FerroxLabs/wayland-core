//! IMAP inbound. The `imap` crate is synchronous, so we run the poll
//! loop on `tokio::task::spawn_blocking`. New messages land in the
//! shared `inbox` queue that `poll_events` drains.

use std::collections::VecDeque;
use std::sync::{Arc, Mutex as StdMutex};
use std::time::Duration;

use tokio::sync::{Mutex, watch};
use wcore_channels::event::{Attachment, ChannelEvent, ChatType, IncomingMessage};

use crate::error::EmailError;

/// Arguments for the blocking poll task. Cloneable plain data so the
/// `spawn_blocking` closure owns its own copy.
pub(crate) struct ImapPollArgs {
    pub host: String,
    pub port: u16,
    pub user: String,
    pub pass: String,
    pub mailbox: String,
    pub poll_interval_secs: u32,
    /// Case-insensitive allow-list of bare sender addresses. When
    /// non-empty, inbound messages whose `From:` addr-spec is not on this
    /// list are dropped before enqueueing. See `ImapConfig::allowed_senders`
    /// for the (lack of) authentication guarantees.
    pub allowed_senders: Vec<String>,
    pub inbox: Arc<Mutex<VecDeque<ChannelEvent>>>,
    pub last_seen_uid: Arc<StdMutex<u32>>,
    pub shutdown: watch::Receiver<bool>,
    /// Tokio handle used so the sync task can enqueue events via
    /// `block_on(inbox.lock())`. Falls back to constructing one if `None`.
    pub runtime_handle: tokio::runtime::Handle,
}

/// Drive an IMAP UID-search loop until `shutdown` flips. Runs on the
/// blocking pool.
pub(crate) fn imap_poll_blocking(args: ImapPollArgs) {
    let ImapPollArgs {
        host,
        port,
        user,
        pass,
        mailbox,
        poll_interval_secs,
        allowed_senders,
        inbox,
        last_seen_uid,
        mut shutdown,
        runtime_handle,
    } = args;

    let interval = Duration::from_secs(u64::from(poll_interval_secs.max(1)));

    // Pre-normalize the allow-list once: bare addr-spec, lowercased.
    let allow_set: Option<std::collections::HashSet<String>> = if allowed_senders.is_empty() {
        None
    } else {
        Some(
            allowed_senders
                .iter()
                .map(|s| normalize_from_addr(s))
                .collect(),
        )
    };

    while !*shutdown.borrow() {
        match poll_once(
            &host,
            port,
            &user,
            &pass,
            &mailbox,
            allow_set.as_ref(),
            &inbox,
            &last_seen_uid,
            &runtime_handle,
        ) {
            Ok(_) => {}
            Err(e) => {
                tracing::warn!(
                    target: "wcore_channel_email::imap",
                    error = %e,
                    "imap poll iteration failed; will retry"
                );
            }
        }
        // Sleep in small increments so shutdown propagates quickly.
        let mut elapsed = Duration::ZERO;
        let step = Duration::from_millis(100);
        while elapsed < interval {
            if *shutdown.borrow() {
                return;
            }
            std::thread::sleep(step);
            elapsed += step;
            // Non-blocking peek for an update on the watch channel.
            if shutdown.has_changed().unwrap_or(false) {
                // Refresh the borrow.
                let _ = shutdown.borrow_and_update();
            }
        }
    }
}

// imap poll loop accepts host/port/user/pass/mailbox/inbox/uid/runtime;
// refactoring into a struct is needless ceremony for a sub-driver helper.
#[allow(clippy::too_many_arguments)]
fn poll_once(
    host: &str,
    port: u16,
    user: &str,
    pass: &str,
    mailbox: &str,
    allow_set: Option<&std::collections::HashSet<String>>,
    inbox: &Arc<Mutex<VecDeque<ChannelEvent>>>,
    last_seen_uid: &Arc<StdMutex<u32>>,
    runtime_handle: &tokio::runtime::Handle,
) -> Result<(), EmailError> {
    let tls =
        native_tls::TlsConnector::new().map_err(|e| EmailError::Imap(format!("tls init: {e}")))?;
    let client = imap::connect((host, port), host, &tls)
        .map_err(|e| EmailError::Imap(format!("connect {host}:{port}: {e}")))?;
    let mut session = client
        .login(user, pass)
        .map_err(|(e, _)| EmailError::Auth(format!("imap login: {e}")))?;
    session
        .select(mailbox)
        .map_err(|e| EmailError::Imap(format!("select {mailbox}: {e}")))?;

    let start_uid = {
        let g = last_seen_uid.lock().unwrap();
        (*g).saturating_add(1)
    };
    let query = format!("{start_uid}:*");
    let uids = session
        .uid_search(&query)
        .map_err(|e| EmailError::Imap(format!("uid_search {query}: {e}")))?;

    let mut new_events: Vec<ChannelEvent> = Vec::new();
    let mut high_water = *last_seen_uid.lock().unwrap();

    for uid in uids {
        if uid <= high_water {
            // `UID N:*` returns at least one result even when nothing
            // new — server semantics. Skip anything we've already seen.
            continue;
        }
        let fetches = match session.uid_fetch(uid.to_string(), "RFC822") {
            Ok(f) => f,
            Err(e) => {
                tracing::warn!(
                    target: "wcore_channel_email::imap",
                    uid = uid,
                    error = %e,
                    "uid_fetch failed; skipping",
                );
                continue;
            }
        };
        for fetch in fetches.iter() {
            let body = match fetch.body() {
                Some(b) => b,
                None => continue,
            };
            match parse_basic_rfc5322(uid, body) {
                Ok(msg) => {
                    // Sender allow-list. `msg.author` is the raw `From:`
                    // header value (display name + addr-spec); compare its
                    // normalized addr-spec against the configured set.
                    // NOTE: `From:` is spoofable — this is a delivery-side
                    // filter, not authentication (see ImapConfig docs).
                    if !is_sender_allowed(allow_set, &msg.author) {
                        tracing::info!(
                            target: "wcore_channel_email::imap",
                            uid = uid,
                            "dropping inbound message: From: not in allowed_senders",
                        );
                        continue;
                    }
                    new_events.push(ChannelEvent::MessageReceived { msg });
                }
                Err(e) => {
                    tracing::warn!(
                        target: "wcore_channel_email::imap",
                        uid = uid,
                        error = %e,
                        "rfc5322 parse failed; dropping message",
                    );
                }
            }
        }
        high_water = high_water.max(uid);
    }

    // Bump watermark even if parses failed — otherwise we'd loop on the
    // same UID forever.
    {
        let mut g = last_seen_uid.lock().unwrap();
        if high_water > *g {
            *g = high_water;
        }
    }

    if !new_events.is_empty() {
        // Bridge sync → async via the runtime handle.
        runtime_handle.block_on(async {
            let mut guard = inbox.lock().await;
            for e in new_events {
                guard.push_back(e);
            }
        });
    }

    // Best-effort logout; ignore errors.
    let _ = session.logout();
    Ok(())
}

/// Minimal RFC 5322 parser — enough to surface From, Subject, and a
/// text/plain body. We treat anything that doesn't look like text/plain
/// as "body = empty string" rather than decoding MIME; the channel is a
/// message-passing surface, not an email client.
pub(crate) fn parse_basic_rfc5322(uid: u32, body: &[u8]) -> Result<IncomingMessage, EmailError> {
    let text = std::str::from_utf8(body)
        .map_err(|e| EmailError::Decode(format!("utf-8: {e}")))?
        .to_string();
    // Split headers / body on the first blank line. Accept CRLFCRLF
    // (per RFC) or bare LFLF (real-world MTAs are sloppy).
    let (head, body_part) = match text.find("\r\n\r\n") {
        Some(i) => (&text[..i], &text[i + 4..]),
        None => match text.find("\n\n") {
            Some(i) => (&text[..i], &text[i + 2..]),
            None => (text.as_str(), ""),
        },
    };

    let mut from: Option<String> = None;
    let mut subject: Option<String> = None;
    let mut date: Option<String> = None;
    let mut message_id: Option<String> = None;
    let mut in_reply_to: Option<String> = None;

    // Header unfolding: a line starting with whitespace continues the
    // previous header.
    let mut current: Option<String> = None;
    let mut headers: Vec<String> = Vec::new();
    for line in head.lines() {
        if line.starts_with(' ') || line.starts_with('\t') {
            if let Some(ref mut acc) = current {
                acc.push(' ');
                acc.push_str(line.trim());
            }
        } else {
            if let Some(prev) = current.take() {
                headers.push(prev);
            }
            current = Some(line.to_string());
        }
    }
    if let Some(prev) = current.take() {
        headers.push(prev);
    }

    for h in &headers {
        if let Some(rest) = h.strip_prefix("From:").or_else(|| h.strip_prefix("from:")) {
            from = Some(rest.trim().to_string());
        } else if let Some(rest) = h
            .strip_prefix("Subject:")
            .or_else(|| h.strip_prefix("subject:"))
        {
            subject = Some(rest.trim().to_string());
        } else if let Some(rest) = h.strip_prefix("Date:").or_else(|| h.strip_prefix("date:")) {
            date = Some(rest.trim().to_string());
        } else if let Some(rest) = h
            .strip_prefix("Message-ID:")
            .or_else(|| h.strip_prefix("Message-Id:"))
            .or_else(|| h.strip_prefix("message-id:"))
        {
            message_id = Some(
                rest.trim()
                    .trim_matches(|c| c == '<' || c == '>')
                    .to_string(),
            );
        } else if let Some(rest) = h
            .strip_prefix("In-Reply-To:")
            .or_else(|| h.strip_prefix("in-reply-to:"))
        {
            let stripped = rest
                .trim()
                .trim_matches(|c| c == '<' || c == '>')
                .to_string();
            if !stripped.is_empty() {
                in_reply_to = Some(stripped);
            }
        }
    }

    let author = from.clone().unwrap_or_else(|| format!("unknown@uid-{uid}"));

    // Prepend the subject so consumers can use it as a thread hint. The
    // explicit text body still carries the message content; we mirror
    // Slack's pattern of putting the subject into `text` with a separator
    // when present.
    let text_body = body_part.trim_end_matches('\n').trim_end_matches('\r');
    let combined_text = match &subject {
        Some(s) if !s.is_empty() => {
            if text_body.is_empty() {
                s.clone()
            } else {
                format!("{s}\n\n{text_body}")
            }
        }
        _ => text_body.to_string(),
    };

    let ts_secs = date.and_then(parse_rfc2822_to_epoch).unwrap_or(0);
    let id = message_id.unwrap_or_else(|| format!("uid:{uid}"));

    // Stable sender identity: the normalized addr-spec from the From header.
    // `normalize_from_addr` strips the display name and lowercases, giving a
    // consistent key that survives name changes and quoting variations.
    let sender_id = normalize_from_addr(&author);

    // Display name: present only when the From header is in "Name <addr>" form.
    // We derive it by taking the text before the angle-addr, trimmed of quotes.
    let sender_display = from.as_deref().and_then(extract_display_name);

    // conversation_id: the stable addr-spec so all messages from a given
    // sender map to one conversation, regardless of display-name drift.
    let conversation_id = sender_id.clone();

    Ok(IncomingMessage {
        id,
        conversation_id,
        author,
        text: combined_text,
        ts_secs,
        // No attachment extraction today — the parser discards non-text/plain
        // MIME parts; Vec::new() is correct (not a stub, just reflects reality).
        attachments: Vec::<Attachment>::new(),
        sender_id,
        sender_display,
        // sender_handle, sender_alt_id: no handle/alt-id concept in email.
        // is_bot, is_self: not determinable without knowing our own address here.
        chat_type: ChatType::Direct,
        chat_name: subject,
        // space_id, parent_chat_id: no enclosing workspace in email.
        // thread_id: References-based root is not parsed; would require scanning
        //   the full References chain. Leave None until thread extraction lands.
        // account_id: receiving mailbox not passed into this fn; caller sets it.
        platform: Some("email".into()),
        // was_mentioned, mention_kind: N/A for email.
        reply_to_message_id: in_reply_to,
        // reply_to_text: we don't inline quoted-reply bodies; leave None.
        ..Default::default()
    })
}

/// Extract the bare `addr-spec` from a `From:`-style header value and
/// lowercase it for case-insensitive comparison.
///
/// Handles the two real-world shapes:
///   `Alice <alice@acme.com>`  -> `alice@acme.com`
///   `bob@acme.com`            -> `bob@acme.com`
///
/// If an angle-addr is present we take its inner text; otherwise we take
/// the whole trimmed value. We do not attempt full RFC 5322 group/comment
/// parsing — the goal is a robust normalized key, and any leftover
/// surrounding text simply won't match a clean allow-list entry (fail
/// closed: an unparsable sender is dropped when an allow-list is set).
/// Decide whether an inbound message's raw `From:` value passes the
/// sender allow-list. `None` allow-set means "no filtering" (allow all).
/// A non-empty allow-set requires the normalized addr-spec to be present;
/// anything else — including an unparsable/empty sender — is dropped
/// (fail closed).
pub(crate) fn is_sender_allowed(
    allow_set: Option<&std::collections::HashSet<String>>,
    raw_from: &str,
) -> bool {
    match allow_set {
        None => true,
        Some(set) => set.contains(&normalize_from_addr(raw_from)),
    }
}

/// Extract the display name from a `From:`-style header value, returning
/// `None` when no display name is present (bare addr-spec form).
///
/// `Alice <alice@acme.com>`        -> `Some("Alice")`
/// `"Carol D" <carol@acme.com>`    -> `Some("Carol D")`
/// `bob@acme.com`                  -> `None`
fn extract_display_name(raw: &str) -> Option<String> {
    let trimmed = raw.trim();
    let angle = trimmed.find('<')?;
    let name = trimmed[..angle]
        .trim()
        .trim_matches(|c| c == '"' || c == '\'');
    if name.is_empty() {
        None
    } else {
        Some(name.to_string())
    }
}

/// Extract the bare `addr-spec` from a `From:`-style header value and
/// lowercase it for case-insensitive comparison.
fn normalize_from_addr(raw: &str) -> String {
    let trimmed = raw.trim();
    let inner = match (trimmed.find('<'), trimmed.find('>')) {
        (Some(open), Some(close)) if close > open + 1 => &trimmed[open + 1..close],
        _ => trimmed,
    };
    inner
        .trim()
        .trim_matches(|c| c == '"' || c == '\'')
        .to_lowercase()
}

fn parse_rfc2822_to_epoch(s: String) -> Option<i64> {
    chrono::DateTime::parse_from_rfc2822(&s)
        .ok()
        .map(|dt| dt.timestamp())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_basic_message_extracts_from_subject_body() {
        let raw = b"From: Alice <alice@acme.com>\r\nSubject: Hello\r\nDate: Mon, 1 Jan 2024 12:00:00 +0000\r\nMessage-ID: <abc@x>\r\n\r\nThe body line.\r\n";
        let m = parse_basic_rfc5322(42, raw).unwrap();
        assert_eq!(m.id, "abc@x");
        assert_eq!(m.author, "Alice <alice@acme.com>");
        assert!(m.text.starts_with("Hello"), "text = {}", m.text);
        assert!(m.text.contains("The body line."), "text = {}", m.text);
        assert_eq!(m.ts_secs, 1_704_110_400);
    }

    #[test]
    fn parse_handles_bare_lflf_body_separator() {
        let raw = b"From: bob@acme.com\nSubject: s\n\nbody";
        let m = parse_basic_rfc5322(7, raw).unwrap();
        assert_eq!(m.author, "bob@acme.com");
        assert!(m.text.contains("body"));
    }

    #[test]
    fn parse_synthesises_id_when_no_message_id() {
        let raw = b"From: x@y\r\n\r\nhi";
        let m = parse_basic_rfc5322(99, raw).unwrap();
        assert_eq!(m.id, "uid:99");
    }

    #[test]
    fn parse_unfolds_multi_line_subject() {
        let raw = b"From: a@b\r\nSubject: line one\r\n  line two\r\n\r\nbody";
        let m = parse_basic_rfc5322(1, raw).unwrap();
        assert!(m.text.starts_with("line one line two"), "text = {}", m.text);
    }

    #[test]
    fn normalize_extracts_addr_spec_from_display_name() {
        assert_eq!(
            normalize_from_addr("Alice <alice@acme.com>"),
            "alice@acme.com"
        );
        assert_eq!(normalize_from_addr("bob@acme.com"), "bob@acme.com");
        // Case-insensitive.
        assert_eq!(normalize_from_addr("OPS@ACME.COM"), "ops@acme.com");
        // Quoted display name with angle-addr.
        assert_eq!(
            normalize_from_addr("\"Carol D\" <carol@acme.com>"),
            "carol@acme.com"
        );
    }

    fn allow(senders: &[&str]) -> std::collections::HashSet<String> {
        senders.iter().map(|s| normalize_from_addr(s)).collect()
    }

    #[test]
    fn no_allowlist_allows_everything() {
        // None = filtering disabled (preserves prior behavior).
        assert!(is_sender_allowed(None, "anyone@anywhere.com"));
        assert!(is_sender_allowed(None, "attacker <evil@phisher.test>"));
    }

    #[test]
    fn allowlist_drops_forged_from_outside_list() {
        let set = allow(&["ops@acme.com", "Alice@Acme.com"]);
        // Legit, case- and display-name-insensitive: allowed.
        assert!(is_sender_allowed(Some(&set), "Alice <alice@acme.com>"));
        assert!(is_sender_allowed(Some(&set), "OPS@acme.com"));
        // Forged From: impersonating a trusted admin not on the list: dropped.
        assert!(!is_sender_allowed(
            Some(&set),
            "Trusted Admin <trusted-admin@company.com>"
        ));
        // Empty / unparsable sender with an allow-list set: fail closed.
        assert!(!is_sender_allowed(Some(&set), ""));
        assert!(!is_sender_allowed(Some(&set), "unknown@uid-7"));
    }

    #[test]
    fn allowlist_filters_parsed_message_author() {
        // End-to-end through the real parser: a forged From outside the
        // allow-list must not pass the gate.
        let set = allow(&["alice@acme.com"]);
        let forged = b"From: Trusted Admin <trusted-admin@company.com>\r\nSubject: x\r\n\r\nbody";
        let msg = parse_basic_rfc5322(1, forged).unwrap();
        assert!(!is_sender_allowed(Some(&set), &msg.author));

        let legit = b"From: Alice <alice@acme.com>\r\nSubject: x\r\n\r\nbody";
        let msg = parse_basic_rfc5322(2, legit).unwrap();
        assert!(is_sender_allowed(Some(&set), &msg.author));
    }
}
