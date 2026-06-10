//! IMAP inbound. The `imap` crate is synchronous, so we run the poll
//! loop on `tokio::task::spawn_blocking`. New messages land in the
//! shared `inbox` queue that `poll_events` drains.

use std::collections::{HashMap, VecDeque};
use std::sync::{Arc, Mutex as StdMutex};
use std::time::Duration;

use tokio::sync::{Mutex, watch};
use wcore_channels::event::{Attachment, ChannelEvent, ChatType, IncomingMessage, MediaKind};

use crate::error::EmailError;
use crate::smtp::ReplyContext;

/// Shared index mapping an inbound RFC `Message-ID` (== `IncomingMessage.id`)
/// to the threading context needed to reply to it. Populated by the IMAP
/// poll task on every accepted inbound message and read by the SMTP send
/// path when an `OutgoingMessage.reply_to` names a known id. `std::Mutex`
/// because the poll task is synchronous.
pub(crate) type ReplyIndex = Arc<StdMutex<HashMap<String, ReplyContext>>>;

/// Upper bound on the reply index so a long-lived channel can't grow it
/// without bound. When exceeded we clear it wholesale; a dropped entry just
/// means a later reply falls back to a single-id `References` chain (still
/// correctly threaded via `In-Reply-To`), never an error.
pub(crate) const REPLY_INDEX_CAP: usize = 4096;

/// Insert one reply-threading entry, enforcing `REPLY_INDEX_CAP`. A
/// synthesized `uid:N` id (no real Message-ID) is still recorded so a
/// reply can at least carry `In-Reply-To`. Clears the map when it would
/// exceed the cap (see `REPLY_INDEX_CAP`).
pub(crate) fn record_reply_context(index: &ReplyIndex, id: String, ctx: ReplyContext) {
    if id.is_empty() {
        return;
    }
    let mut guard = match index.lock() {
        Ok(g) => g,
        // A poisoned lock would only happen if a holder panicked; threading
        // metadata is non-critical, so we skip rather than propagate.
        Err(_) => return,
    };
    if guard.len() >= REPLY_INDEX_CAP && !guard.contains_key(&id) {
        guard.clear();
    }
    guard.insert(id, ctx);
}

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
    /// Shared reply-threading index; the poll task records one entry per
    /// accepted inbound message so the send path can thread replies.
    pub reply_index: ReplyIndex,
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
        reply_index,
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
            &reply_index,
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
    reply_index: &ReplyIndex,
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
            match parse_message(uid, body) {
                Ok((msg, reply_ctx)) => {
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
                    // Record threading context so a later reply can set
                    // In-Reply-To / References / Re: subject. Keyed by the
                    // RFC Message-ID (== msg.id).
                    record_reply_context(reply_index, msg.id.clone(), reply_ctx);
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

/// RFC 5322 / MIME parser — surfaces From, Subject, Message-ID,
/// In-Reply-To, References, a non-empty text body (walking the MIME tree
/// and converting HTML-only mail to text), and typed attachments.
///
/// We operate on a lossy-UTF-8 view of the raw message: RFC822 headers and
/// base64/quoted-printable bodies are ASCII, so the only bytes lost are
/// raw-8bit body octets we surface as metadata anyway. This keeps the
/// parser robust against the odd non-UTF-8 byte instead of erroring out.
/// Test-only convenience wrapper that drops the `ReplyContext` returned by
/// [`parse_message`]; production code uses `parse_message` directly.
#[cfg(test)]
pub(crate) fn parse_basic_rfc5322(uid: u32, body: &[u8]) -> Result<IncomingMessage, EmailError> {
    parse_message(uid, body).map(|(msg, _)| msg)
}

/// Parse a raw RFC822 message into the inbound `IncomingMessage` plus the
/// `ReplyContext` (Message-ID + Subject + References) that the SMTP send
/// path needs to thread a reply. The reply context is keyed downstream by
/// the inbound message id (`IncomingMessage.id`, which is the RFC
/// Message-ID when present).
pub(crate) fn parse_message(
    uid: u32,
    body: &[u8],
) -> Result<(IncomingMessage, ReplyContext), EmailError> {
    let text = String::from_utf8_lossy(body).into_owned();
    let (head, body_part) = split_headers_body(&text);
    let headers = unfold_headers(head);

    let mut from: Option<String> = None;
    let mut subject: Option<String> = None;
    let mut date: Option<String> = None;
    let mut message_id: Option<String> = None;
    let mut in_reply_to: Option<String> = None;
    let mut references: Option<String> = None;

    for h in &headers {
        if let Some(rest) = header_value(h, "From") {
            from = Some(rest.to_string());
        } else if let Some(rest) = header_value(h, "Subject") {
            subject = Some(rest.to_string());
        } else if let Some(rest) = header_value(h, "Date") {
            date = Some(rest.to_string());
        } else if let Some(rest) = header_value(h, "Message-ID") {
            message_id = Some(rest.trim_matches(|c| c == '<' || c == '>').to_string());
        } else if let Some(rest) = header_value(h, "In-Reply-To") {
            let stripped = rest.trim_matches(|c| c == '<' || c == '>').to_string();
            if !stripped.is_empty() {
                in_reply_to = Some(stripped);
            }
        } else if let Some(rest) = header_value(h, "References")
            && !rest.is_empty()
        {
            references = Some(rest.to_string());
        }
    }

    let author = from.clone().unwrap_or_else(|| format!("unknown@uid-{uid}"));

    // Walk the MIME tree for a plain-text body + typed attachments. The
    // walk prefers text/plain, falls back to HTML→text, and pulls any
    // attachment parts out as metadata.
    let MimeResult {
        text: body_text,
        attachments,
    } = walk_mime(&headers, body_part, 0);

    // Prepend the subject so consumers can use it as a thread hint, mirroring
    // the prior behavior (and Slack's subject-in-text convention).
    let body_trimmed = body_text.trim_end_matches(['\n', '\r']);
    let combined_text = match &subject {
        Some(s) if !s.is_empty() => {
            if body_trimmed.is_empty() {
                s.clone()
            } else {
                format!("{s}\n\n{body_trimmed}")
            }
        }
        _ => body_trimmed.to_string(),
    };

    let ts_secs = date.and_then(parse_rfc2822_to_epoch).unwrap_or(0);
    let id = message_id.unwrap_or_else(|| format!("uid:{uid}"));

    // Threading context for any reply to this message: its Message-ID,
    // Subject (for `Re: <subj>`), and References chain (for `References`).
    let reply_ctx = ReplyContext {
        message_id: id.clone(),
        subject: subject.clone(),
        references,
    };

    // Stable sender identity: the normalized addr-spec from the From header.
    let sender_id = normalize_from_addr(&author);
    let sender_display = from.as_deref().and_then(extract_display_name);
    let conversation_id = sender_id.clone();

    let msg = IncomingMessage {
        id,
        conversation_id,
        author,
        text: combined_text,
        ts_secs,
        attachments,
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
    };
    Ok((msg, reply_ctx))
}

/// Split a raw message into (headers, body) on the first blank line.
/// Accepts `CRLFCRLF` (per RFC) or bare `LFLF` (real-world MTAs are
/// sloppy). When no blank line exists, the whole input is treated as
/// headers with an empty body.
fn split_headers_body(text: &str) -> (&str, &str) {
    match text.find("\r\n\r\n") {
        Some(i) => (&text[..i], &text[i + 4..]),
        None => match text.find("\n\n") {
            Some(i) => (&text[..i], &text[i + 2..]),
            None => (text, ""),
        },
    }
}

/// Unfold a header block: a line starting with whitespace continues the
/// previous header (RFC 5322 §2.2.3). Returns one logical header per
/// entry, each still in `Name: value` form.
fn unfold_headers(head: &str) -> Vec<String> {
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
    headers
}

/// Case-insensitively match a single unfolded header line against `name`
/// and return its trimmed value, or `None` if it's a different header.
fn header_value<'a>(line: &'a str, name: &str) -> Option<&'a str> {
    let colon = line.find(':')?;
    if line[..colon].trim().eq_ignore_ascii_case(name) {
        Some(line[colon + 1..].trim())
    } else {
        None
    }
}

/// Result of walking a message's MIME tree: the best plain-text body we
/// could extract plus any attachment parts surfaced as metadata.
struct MimeResult {
    text: String,
    attachments: Vec<Attachment>,
}

/// Parsed `Content-Type` of a MIME part.
struct ContentType {
    /// Lowercased `type/subtype` (e.g. `text/plain`, `multipart/mixed`).
    mime: String,
    /// `boundary` parameter for multipart types, if present.
    boundary: Option<String>,
}

/// Walk a MIME body given its (unfolded) headers and raw body text.
///
/// Dispatch:
/// - `multipart/*` — split on the boundary, recurse into each sub-part.
///   For `multipart/alternative` prefer the richest text we can render
///   (plain over html-converted-to-text); for `mixed`/`related` collect
///   the first non-empty text part and accumulate attachments.
/// - `text/plain` — decoded body as-is.
/// - `text/html` — tags stripped to a readable plain body.
/// - anything with a filename / `attachment` disposition — an Attachment.
///
/// Guarantees a non-empty `text` whenever any renderable text part exists
/// anywhere in the tree.
/// Maximum MIME nesting depth walked. A hostile sender can craft a message
/// with thousands of nested `multipart/*` levels; without a cap the
/// recursion would overflow the stack and crash the poll thread. Past this
/// depth a part is surfaced as raw text rather than recursed into.
const MAX_MIME_DEPTH: usize = 20;

fn walk_mime(headers: &[String], body: &str, depth: usize) -> MimeResult {
    let ct = parse_content_type(headers);
    let disposition = headers
        .iter()
        .find_map(|h| header_value(h, "Content-Disposition"));
    let cte = headers
        .iter()
        .find_map(|h| header_value(h, "Content-Transfer-Encoding"))
        .map(|s| s.to_ascii_lowercase());

    // Attachment parts are surfaced as metadata, not body text. Treat a
    // part as an attachment when it declares `Content-Disposition:
    // attachment`, or when it carries a filename, or when it's a non-text
    // part that isn't multipart.
    let filename = disposition
        .and_then(parse_filename)
        .or_else(|| ct_param(headers, "name"));
    let is_attachment = disposition
        .is_some_and(|d| d.trim().to_ascii_lowercase().starts_with("attachment"))
        || (filename.is_some() && !ct.mime.starts_with("text/"));

    if ct.mime.starts_with("multipart/") {
        if depth >= MAX_MIME_DEPTH {
            // Pathologically nested multipart — stop recursing and surface the
            // raw body as text rather than risk a stack overflow.
            return MimeResult {
                text: body.trim().to_string(),
                attachments: Vec::new(),
            };
        }
        return walk_multipart(&ct, body, depth);
    }

    if is_attachment {
        let name = filename.unwrap_or_else(|| "attachment".to_string());
        return MimeResult {
            text: String::new(),
            attachments: vec![attachment_from(&ct.mime, &name)],
        };
    }

    // Leaf text part.
    let decoded = decode_transfer(body, cte.as_deref());
    let text = if ct.mime == "text/html" || (ct.mime.is_empty() && looks_like_html(&decoded)) {
        html_to_text(&decoded)
    } else {
        // text/plain (the default per RFC 2045 when no Content-Type).
        decoded
    };
    MimeResult {
        text,
        attachments: Vec::new(),
    }
}

/// Split a multipart body on its boundary and fold the sub-parts into a
/// single `MimeResult`.
fn walk_multipart(ct: &ContentType, body: &str, depth: usize) -> MimeResult {
    let boundary = match &ct.boundary {
        Some(b) => b,
        // Malformed multipart with no boundary: best-effort treat the raw
        // body as text so we don't silently drop content.
        None => {
            return MimeResult {
                text: body.trim().to_string(),
                attachments: Vec::new(),
            };
        }
    };

    let alternative = ct.mime == "multipart/alternative";
    let mut plain: Option<String> = None;
    let mut html: Option<String> = None;
    let mut other_text: Option<String> = None;
    let mut attachments: Vec<Attachment> = Vec::new();

    for raw_part in split_parts(body, boundary) {
        let (phead, pbody) = split_headers_body(raw_part);
        let pheaders = unfold_headers(phead);
        let sub = walk_mime(&pheaders, pbody, depth + 1);
        attachments.extend(sub.attachments);

        if sub.text.trim().is_empty() {
            continue;
        }
        let part_ct = parse_content_type(&pheaders);
        match part_ct.mime.as_str() {
            "text/plain" => {
                if plain.is_none() {
                    plain = Some(sub.text);
                }
            }
            "text/html" => {
                if html.is_none() {
                    html = Some(sub.text);
                }
            }
            _ => {
                // Nested multipart already rendered to text, or an
                // untyped text leaf — keep the first non-empty.
                if other_text.is_none() {
                    other_text = Some(sub.text);
                }
            }
        }
    }

    // multipart/alternative: prefer plain, then html. mixed/related/etc:
    // concatenation isn't meaningful for a chat surface, so we take the
    // first renderable text in priority order. Either way a non-empty text
    // part anywhere yields a non-empty body.
    let text = if alternative {
        plain.or(html).or(other_text).unwrap_or_default()
    } else {
        plain.or(other_text).or(html).unwrap_or_default()
    };

    MimeResult { text, attachments }
}

/// Split a multipart body into raw sub-part slices delimited by
/// `--<boundary>`. The preamble (before the first boundary) and the
/// closing `--<boundary>--` epilogue are discarded per RFC 2046.
fn split_parts<'a>(body: &'a str, boundary: &str) -> Vec<&'a str> {
    let delim = format!("--{boundary}");
    let mut parts = Vec::new();
    // Skip everything up to (and including) the first delimiter line.
    let mut rest = match body.find(&delim) {
        Some(i) => &body[i + delim.len()..],
        None => return parts,
    };
    loop {
        // Each part starts after the CRLF following the delimiter.
        let part_start = rest.find('\n').map_or(rest.len(), |i| i + 1);
        let after = &rest[part_start..];
        match after.find(&delim) {
            Some(next) => {
                let part = &after[..next];
                // Closing delimiter is `--boundary--`; trailing chars after
                // the part are trimmed by split_headers_body downstream.
                parts.push(part.trim_end_matches(['\r', '\n']));
                rest = &after[next + delim.len()..];
                // `--` immediately after the delimiter marks the end.
                if rest.starts_with("--") {
                    break;
                }
            }
            None => break,
        }
    }
    parts
}

/// Parse a part's `Content-Type` header into a normalized struct. When the
/// header is absent, `mime` is left empty; the leaf handler then treats it
/// as `text/plain` per RFC 2045 (unless the body sniffs as HTML).
fn parse_content_type(headers: &[String]) -> ContentType {
    let raw = headers.iter().find_map(|h| header_value(h, "Content-Type"));
    let raw = match raw {
        Some(r) => r,
        None => {
            return ContentType {
                mime: String::new(),
                boundary: None,
            };
        }
    };
    let mime = raw
        .split(';')
        .next()
        .unwrap_or("")
        .trim()
        .to_ascii_lowercase();
    let boundary = ct_param_in(raw, "boundary");
    ContentType { mime, boundary }
}

/// Extract a `Content-Type` parameter (e.g. `boundary`, `name`) from the
/// full header value list.
fn ct_param(headers: &[String], param: &str) -> Option<String> {
    let raw = headers.iter().find_map(|h| header_value(h, "Content-Type"))?;
    ct_param_in(raw, param)
}

/// Extract a parameter value from a raw `Content-Type` / disposition
/// value, handling both quoted and bare forms (`name="a.png"` / `name=a`).
fn ct_param_in(raw: &str, param: &str) -> Option<String> {
    for seg in raw.split(';').skip(1) {
        let seg = seg.trim();
        let Some(eq) = seg.find('=') else { continue };
        if seg[..eq].trim().eq_ignore_ascii_case(param) {
            let val = seg[eq + 1..].trim().trim_matches('"');
            if !val.is_empty() {
                return Some(val.to_string());
            }
        }
    }
    None
}

/// Pull the `filename` out of a `Content-Disposition` value.
fn parse_filename(disposition: &str) -> Option<String> {
    ct_param_in(disposition, "filename")
}

/// Map a leaf part to a typed `Attachment`. For email there is no fetch
/// URL, so `url` stays empty and the filename is surfaced via `path` so
/// downstream consumers have a human-facing name. The raw bytes are not
/// inlined (v1 surfaces metadata only); `content_type` + `kind` let a
/// consumer decide whether to fetch later.
fn attachment_from(mime: &str, filename: &str) -> Attachment {
    let kind = if mime.starts_with("image/") {
        MediaKind::Image
    } else if mime.starts_with("audio/") {
        MediaKind::Audio
    } else if mime.starts_with("video/") {
        MediaKind::Video
    } else {
        MediaKind::Document
    };
    Attachment {
        url: String::new(),
        path: Some(filename.to_string()),
        content_type: if mime.is_empty() {
            None
        } else {
            Some(mime.to_string())
        },
        kind,
        transcribed: None,
    }
}

/// Decode a leaf part body according to its `Content-Transfer-Encoding`.
/// Handles `quoted-printable` and `base64`; `7bit`/`8bit`/`binary`/none
/// pass through unchanged.
fn decode_transfer(body: &str, cte: Option<&str>) -> String {
    match cte {
        Some("quoted-printable") => decode_quoted_printable(body),
        Some("base64") => decode_base64_text(body),
        _ => body.to_string(),
    }
}

/// Decode a single ASCII hex digit to its nibble value.
fn hex_val(b: u8) -> Option<u8> {
    match b {
        b'0'..=b'9' => Some(b - b'0'),
        b'a'..=b'f' => Some(b - b'a' + 10),
        b'A'..=b'F' => Some(b - b'A' + 10),
        _ => None,
    }
}

/// Minimal quoted-printable decoder (RFC 2045 §6.7): `=XX` hex escapes and
/// `=`-at-end-of-line soft breaks. Unrecognized `=` sequences pass through.
fn decode_quoted_printable(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    for line in input.split_inclusive('\n') {
        let trimmed = line.trim_end_matches(['\r', '\n']);
        // Soft line break: a trailing `=` joins to the next line.
        let (content, soft_break) = match trimmed.strip_suffix('=') {
            Some(rest) => (rest, true),
            None => (trimmed, false),
        };
        let bytes = content.as_bytes();
        let mut i = 0;
        let mut buf: Vec<u8> = Vec::with_capacity(bytes.len());
        while i < bytes.len() {
            // `=XX` hex escape, decoded directly from bytes (no str slicing,
            // so a malformed multibyte sequence can never panic).
            if bytes[i] == b'='
                && i + 2 < bytes.len()
                && let (Some(hi), Some(lo)) = (hex_val(bytes[i + 1]), hex_val(bytes[i + 2]))
            {
                buf.push((hi << 4) | lo);
                i += 3;
                continue;
            }
            buf.push(bytes[i]);
            i += 1;
        }
        out.push_str(&String::from_utf8_lossy(&buf));
        if !soft_break {
            // Preserve hard line breaks (normalize to \n).
            if line.ends_with('\n') {
                out.push('\n');
            }
        }
    }
    out
}

/// Minimal base64 decoder for text parts. Ignores whitespace/newlines and
/// stops at padding. Returns a lossy-UTF-8 rendering of the decoded bytes.
fn decode_base64_text(input: &str) -> String {
    fn val(c: u8) -> Option<u8> {
        match c {
            b'A'..=b'Z' => Some(c - b'A'),
            b'a'..=b'z' => Some(c - b'a' + 26),
            b'0'..=b'9' => Some(c - b'0' + 52),
            b'+' => Some(62),
            b'/' => Some(63),
            _ => None,
        }
    }
    let mut out: Vec<u8> = Vec::with_capacity(input.len() * 3 / 4);
    let mut acc: u32 = 0;
    let mut bits = 0u32;
    for &c in input.as_bytes() {
        if c == b'=' {
            break;
        }
        let Some(v) = val(c) else { continue };
        acc = (acc << 6) | u32::from(v);
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push((acc >> bits) as u8);
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// Heuristic: does this body look like HTML even without a Content-Type?
fn looks_like_html(s: &str) -> bool {
    let low = s.trim_start().to_ascii_lowercase();
    low.starts_with("<!doctype html") || low.starts_with("<html") || low.contains("<body")
}

/// Strip HTML to a readable plain-text body (v1: a minimal tag stripper,
/// not a full renderer). Drops `<script>`/`<style>` contents entirely,
/// removes all tags, decodes the handful of common entities, and collapses
/// runs of blank lines. Adequate for surfacing the human-readable text of
/// an HTML-only email on a chat surface.
fn html_to_text(html: &str) -> String {
    let mut out = String::with_capacity(html.len());
    let lower = html.to_ascii_lowercase();
    // Work on byte offsets but only ever slice at char boundaries: tag
    // delimiters (`<`, `>`) are ASCII, so byte offsets at those points are
    // always valid boundaries. Text between tags is copied char-by-char.
    let mut i = 0;
    while i < html.len() {
        if html.as_bytes()[i] == b'<' {
            // Drop the entire contents of <script>/<style> blocks.
            let mut skipped = false;
            for tag in ["script", "style"] {
                if lower[i..].starts_with(&format!("<{tag}")) {
                    let close = format!("</{tag}>");
                    i = match lower[i..].find(&close) {
                        Some(end) => i + end + close.len(),
                        None => html.len(),
                    };
                    skipped = true;
                    break;
                }
            }
            if skipped {
                continue;
            }
            // Map block-level tags to newlines so structure survives.
            if lower[i..].starts_with("<br")
                || lower[i..].starts_with("</p")
                || lower[i..].starts_with("</div")
                || lower[i..].starts_with("</tr")
                || lower[i..].starts_with("</li")
                || lower[i..].starts_with("</h")
            {
                out.push('\n');
            }
            // Skip to the end of the tag.
            match html[i..].find('>') {
                Some(end) => i += end + 1,
                None => break,
            }
        } else {
            // Copy one full UTF-8 char (handles multibyte safely).
            let ch = html[i..].chars().next().unwrap_or('\u{FFFD}');
            out.push(ch);
            i += ch.len_utf8();
        }
    }
    let decoded = decode_html_entities(&out);
    collapse_blank_lines(&decoded)
}

/// Decode the small set of HTML entities that show up in real mail.
fn decode_html_entities(s: &str) -> String {
    s.replace("&nbsp;", " ")
        .replace("&amp;", "&")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&#39;", "'")
        .replace("&apos;", "'")
}

/// Trim each line and collapse runs of 2+ blank lines into one.
fn collapse_blank_lines(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut blank_run = 0;
    for line in s.lines() {
        let t = line.trim();
        if t.is_empty() {
            blank_run += 1;
            if blank_run <= 1 {
                out.push('\n');
            }
        } else {
            blank_run = 0;
            out.push_str(t);
            out.push('\n');
        }
    }
    out.trim().to_string()
}

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

    // -----------------------------------------------------------------
    // MIME walk (FIX 2)
    // -----------------------------------------------------------------

    #[test]
    fn multipart_alternative_html_only_yields_nonempty_text() {
        // multipart/alternative whose only renderable part is text/html.
        let raw = b"From: a@b\r\n\
Subject: Hi\r\n\
Content-Type: multipart/alternative; boundary=\"BD\"\r\n\
\r\n\
--BD\r\n\
Content-Type: text/html; charset=utf-8\r\n\
\r\n\
<html><body><p>Hello <b>world</b></p></body></html>\r\n\
--BD--\r\n";
        let m = parse_basic_rfc5322(1, raw).unwrap();
        assert!(m.text.contains("Hello"), "text = {:?}", m.text);
        assert!(m.text.contains("world"), "text = {:?}", m.text);
        // Subject still prepended.
        assert!(m.text.starts_with("Hi"), "text = {:?}", m.text);
        // Tags stripped.
        assert!(!m.text.contains('<'), "text should have no tags: {:?}", m.text);
    }

    #[test]
    fn multipart_alternative_prefers_plain_over_html() {
        let raw = b"From: a@b\r\n\
Content-Type: multipart/alternative; boundary=\"X\"\r\n\
\r\n\
--X\r\n\
Content-Type: text/plain\r\n\
\r\n\
plain body wins\r\n\
--X\r\n\
Content-Type: text/html\r\n\
\r\n\
<p>html body</p>\r\n\
--X--\r\n";
        let m = parse_basic_rfc5322(2, raw).unwrap();
        assert!(m.text.contains("plain body wins"), "text = {:?}", m.text);
        assert!(!m.text.contains("html body"), "text = {:?}", m.text);
    }

    #[test]
    fn multipart_mixed_text_plus_attachment_yields_text_and_attachment() {
        // multipart/mixed: a text/plain part + an attachment part.
        let raw = b"From: a@b\r\n\
Subject: report\r\n\
Content-Type: multipart/mixed; boundary=\"MIX\"\r\n\
\r\n\
--MIX\r\n\
Content-Type: text/plain\r\n\
\r\n\
see attached\r\n\
--MIX\r\n\
Content-Type: image/png; name=\"chart.png\"\r\n\
Content-Disposition: attachment; filename=\"chart.png\"\r\n\
Content-Transfer-Encoding: base64\r\n\
\r\n\
aGVsbG8=\r\n\
--MIX--\r\n";
        let m = parse_basic_rfc5322(3, raw).unwrap();
        assert!(m.text.contains("see attached"), "text = {:?}", m.text);
        assert_eq!(m.attachments.len(), 1, "attachments = {:?}", m.attachments);
        let att = &m.attachments[0];
        assert_eq!(att.kind, MediaKind::Image);
        assert_eq!(att.content_type.as_deref(), Some("image/png"));
        assert_eq!(att.path.as_deref(), Some("chart.png"));
        assert!(att.url.is_empty(), "email attachments have no fetch url");
    }

    #[test]
    fn attachment_kind_maps_from_content_type() {
        assert_eq!(attachment_from("image/jpeg", "a.jpg").kind, MediaKind::Image);
        assert_eq!(attachment_from("audio/mpeg", "a.mp3").kind, MediaKind::Audio);
        assert_eq!(attachment_from("video/mp4", "a.mp4").kind, MediaKind::Video);
        assert_eq!(
            attachment_from("application/pdf", "a.pdf").kind,
            MediaKind::Document
        );
    }

    #[test]
    fn quoted_printable_html_part_decodes_and_strips() {
        let raw = b"From: a@b\r\n\
Content-Type: text/html\r\n\
Content-Transfer-Encoding: quoted-printable\r\n\
\r\n\
<p>caf=C3=A9 =26 more</p>\r\n";
        let m = parse_basic_rfc5322(4, raw).unwrap();
        // =C3=A9 -> é ; =26 -> & ; tags stripped.
        assert!(m.text.contains("café"), "text = {:?}", m.text);
        assert!(m.text.contains('&'), "text = {:?}", m.text);
        assert!(!m.text.contains("<p>"), "text = {:?}", m.text);
    }

    #[test]
    fn plain_text_single_part_unchanged() {
        // Regression: a bare text/plain message still surfaces its body.
        let raw = b"From: a@b\r\nSubject: s\r\n\r\njust text\r\n";
        let m = parse_basic_rfc5322(5, raw).unwrap();
        assert!(m.text.contains("just text"), "text = {:?}", m.text);
        assert!(m.attachments.is_empty());
    }

    #[test]
    fn html_strip_drops_script_and_decodes_entities() {
        let html = "<html><head><style>p{color:red}</style></head>\
<body><script>evil()</script><p>A &amp; B</p><p>line two</p></body></html>";
        let text = html_to_text(html);
        assert!(text.contains("A & B"), "text = {:?}", text);
        assert!(text.contains("line two"), "text = {:?}", text);
        assert!(!text.contains("evil"), "script body must be dropped: {:?}", text);
        assert!(!text.contains("color:red"), "style body must be dropped: {:?}", text);
    }

    #[test]
    fn parse_message_returns_reply_context() {
        let raw = b"From: a@b\r\n\
Subject: Original subject\r\n\
Message-ID: <orig@host>\r\n\
References: <root@host>\r\n\
\r\n\
hello";
        let (msg, ctx) = parse_message(7, raw).unwrap();
        assert_eq!(msg.id, "orig@host");
        assert_eq!(ctx.message_id, "orig@host");
        assert_eq!(ctx.subject.as_deref(), Some("Original subject"));
        assert_eq!(ctx.references.as_deref(), Some("<root@host>"));
    }

    #[test]
    fn record_reply_context_skips_empty_id_and_enforces_cap() {
        let index: ReplyIndex = Arc::new(StdMutex::new(HashMap::new()));
        // Empty id is ignored.
        record_reply_context(&index, String::new(), ReplyContext::default());
        assert_eq!(index.lock().unwrap().len(), 0);
        // Normal insert.
        record_reply_context(
            &index,
            "a@x".into(),
            ReplyContext {
                message_id: "a@x".into(),
                subject: Some("s".into()),
                references: None,
            },
        );
        assert_eq!(index.lock().unwrap().len(), 1);
        assert!(index.lock().unwrap().contains_key("a@x"));
    }
}
