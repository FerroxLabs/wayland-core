//! SMTP outbound. Wraps `lettre::AsyncSmtpTransport` behind a
//! `MailSender` trait so tests can stand in a recording mock without
//! booting a real SMTP server.
//!
//! Retry policy mirrors `wcore-channel-slack` / `wcore-channel-telegram`:
//! up to `SEND_MAX_ATTEMPTS` tries, exponential backoff on transient
//! errors, permanent short-circuit on auth failure.

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use async_trait::async_trait;
use lettre::message::header::{HeaderName, HeaderValue};
use lettre::message::{Attachment, Message, MultiPart, SinglePart, header::ContentType};
use lettre::transport::smtp::AsyncSmtpTransport;
use lettre::transport::smtp::authentication::Credentials;
use lettre::transport::smtp::response::Response;
use lettre::{AsyncTransport, Tokio1Executor};

use crate::error::EmailError;

/// Number of retry attempts (including the first one) for outbound sends.
pub(crate) const SEND_MAX_ATTEMPTS: u32 = 5;
/// Base backoff for transient retries.
pub(crate) const SEND_BASE_BACKOFF_MS: u64 = 200;
/// Cap any single sleep between retries so a misbehaving server can't
/// park us indefinitely.
pub(crate) const SEND_MAX_BACKOFF_MS: u64 = 30_000;

/// Outbound abstraction. Production binds this to lettre's async
/// transport; tests provide an in-memory recorder.
#[async_trait]
pub trait MailSender: Send + Sync {
    async fn send(&self, msg: Message) -> Result<Response, SendError>;
}

/// Internal send error returned by `MailSender::send`. Carries enough
/// context for the retry loop to decide transient vs permanent.
#[derive(Debug)]
pub enum SendError {
    /// Connection / DNS / TLS / 5xx — retry-eligible.
    Transient(String),
    /// Auth failure (5xx 535 etc.) — do not retry.
    Auth(String),
    /// Permanent 5xx envelope rejection — do not retry.
    Permanent(String),
}

impl std::fmt::Display for SendError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Transient(m) => write!(f, "transient: {m}"),
            Self::Auth(m) => write!(f, "auth: {m}"),
            Self::Permanent(m) => write!(f, "permanent: {m}"),
        }
    }
}

impl std::error::Error for SendError {}

/// Production sender — wraps an `AsyncSmtpTransport`.
pub struct LettreSender {
    inner: AsyncSmtpTransport<Tokio1Executor>,
}

impl LettreSender {
    /// Build a STARTTLS sender for `host:port` with username/password
    /// SASL PLAIN auth. Returns Err if the relay builder rejects the
    /// host (e.g. malformed name).
    pub fn new(
        host: &str,
        port: u16,
        username: String,
        password: String,
    ) -> Result<Self, EmailError> {
        let creds = Credentials::new(username, password);
        let transport = AsyncSmtpTransport::<Tokio1Executor>::starttls_relay(host)
            .map_err(|e| EmailError::Smtp(format!("build relay {host}: {e}")))?
            .port(port)
            .credentials(creds)
            .build();
        Ok(Self { inner: transport })
    }
}

#[async_trait]
impl MailSender for LettreSender {
    async fn send(&self, msg: Message) -> Result<Response, SendError> {
        match self.inner.send(msg).await {
            Ok(r) => Ok(r),
            Err(e) => Err(classify_lettre_error(&e)),
        }
    }
}

fn classify_lettre_error(e: &lettre::transport::smtp::Error) -> SendError {
    // lettre's smtp::Error doesn't expose a stable enum; we sniff the
    // Display string. Auth failures contain "auth" / "535" / "credentials".
    let s = e.to_string();
    let low = s.to_lowercase();
    if low.contains("auth") || low.contains("535") || low.contains("credentials") {
        SendError::Auth(s)
    } else if e.is_permanent() {
        SendError::Permanent(s)
    } else {
        SendError::Transient(s)
    }
}

/// Threading context for an outbound reply, captured from the inbound
/// message it replies to. Populated on inbound (keyed by the inbound RFC
/// `Message-ID`, which is `IncomingMessage.id`) and looked up on outbound
/// when `OutgoingMessage.reply_to` names a known id.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct ReplyContext {
    /// The inbound message's RFC `Message-ID` (without angle brackets).
    pub message_id: String,
    /// The inbound message's `Subject`, if any. Used to build `Re: <subj>`.
    pub subject: Option<String>,
    /// The inbound message's `References` header value (space-separated
    /// `<id>` tokens, angle brackets retained), if present. Carried so the
    /// reply's `References` chain stays well-formed.
    pub references: Option<String>,
}

/// Build the `Subject` for a reply: `Re: <original>`, without
/// double-prefixing when the original already starts with `Re:`
/// (case-insensitive, optional surrounding whitespace). Falls back to a
/// bare `Re:` when the original subject is unknown or empty.
pub(crate) fn build_reply_subject(original: Option<&str>) -> String {
    match original.map(str::trim).filter(|s| !s.is_empty()) {
        Some(s) => {
            // Detect an existing "Re:" prefix (case-insensitive). Compare the
            // leading bytes so a multibyte subject can't panic on a non-char
            // boundary slice.
            let b = s.as_bytes();
            let is_re = b.len() >= 3
                && b[0].eq_ignore_ascii_case(&b'r')
                && b[1].eq_ignore_ascii_case(&b'e')
                && b[2] == b':';
            if is_re {
                s.to_string()
            } else {
                format!("Re: {s}")
            }
        }
        None => "Re:".to_string(),
    }
}

/// Build the `References` value for a reply, per RFC 5322 §3.6.4: append
/// the parent's `Message-ID` to the parent's existing `References` chain
/// (or start a fresh chain with just the parent id). Each id is wrapped in
/// angle brackets. Returns `None` only when the parent id is empty.
fn build_references(reply: &ReplyContext) -> Option<String> {
    if reply.message_id.is_empty() {
        return None;
    }
    let parent = format!("<{}>", reply.message_id);
    match reply.references.as_deref().map(str::trim) {
        Some(prev) if !prev.is_empty() => Some(format!("{prev} {parent}")),
        _ => Some(parent),
    }
}

/// Build the threading headers (`In-Reply-To`, `References`) for a reply.
/// Returns an empty vec when the reply context carries no usable parent
/// id (so the caller can unconditionally extend the builder). Header
/// values are plain-ASCII `<message-id>` tokens; `HeaderValue::new` passes
/// them through unencoded.
fn reply_headers(reply: &ReplyContext) -> Vec<HeaderValue> {
    if reply.message_id.is_empty() {
        return Vec::new();
    }
    let mut out = Vec::with_capacity(2);
    let in_reply_to = format!("<{}>", reply.message_id);
    out.push(HeaderValue::new(
        HeaderName::new_from_ascii_str("In-Reply-To"),
        in_reply_to,
    ));
    if let Some(refs) = build_references(reply) {
        out.push(HeaderValue::new(
            HeaderName::new_from_ascii_str("References"),
            refs,
        ));
    }
    out
}

/// Build a minimal text/plain `lettre::Message` from the outbound
/// envelope. `from` is the channel's configured From: address; `to` is
/// the outbound conversation_id (one recipient per send — multi-recipient
/// support lives behind a future enhancement).
///
/// When `reply` is set, the message is threaded: `Subject` becomes
/// `Re: <original>` and `In-Reply-To` / `References` headers are attached
/// so MUAs group the reply with the original. When `reply` is `None` the
/// subject falls back to `subject_override` (if any) or a bare default,
/// preserving the prior outbound-only behavior.
/// Monotonic per-process sequence so two sends in the same nanosecond (or on
/// a coarse clock) still get distinct Message-IDs.
static OUTBOUND_ID_SEQ: AtomicU64 = AtomicU64::new(0);

/// Generate a bare (no angle brackets) RFC 5322 `Message-ID` for an outbound
/// message: `wl-<epoch-nanos>-<seq>@<from-domain>`. The domain comes from the
/// configured From address so the id is well-formed for strict MTAs; falls
/// back to a fixed label when the address has no `@` (already rejected later
/// by the address parse, but never panic here).
pub(crate) fn make_outbound_message_id(from: &str) -> String {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let seq = OUTBOUND_ID_SEQ.fetch_add(1, Ordering::Relaxed);
    let domain = from
        .rsplit_once('@')
        .map(|(_, d)| d.trim().trim_end_matches('>'))
        .filter(|d| !d.is_empty())
        .unwrap_or("wayland.invalid");
    format!("wl-{nanos:x}-{seq:x}@{domain}")
}

/// Read the bare `Message-ID` (angle brackets stripped) off a built message.
/// `build_message` always stamps one, so `None` only happens for messages
/// built elsewhere.
pub(crate) fn outbound_message_id(msg: &Message) -> Option<String> {
    msg.headers()
        .get_raw("Message-ID")
        .map(|v| v.trim().trim_matches(|c| c == '<' || c == '>').to_string())
        .filter(|v| !v.is_empty())
}

pub(crate) fn build_message(
    from: &str,
    to: &str,
    text: &str,
    reply: Option<&ReplyContext>,
    subject_override: Option<&str>,
    attachments: &[OutboundAttachment],
) -> Result<Message, EmailError> {
    let from_addr = from
        .parse()
        .map_err(|e| EmailError::Envelope(format!("from {from}: {e}")))?;
    let to_addr = to
        .parse()
        .map_err(|e| EmailError::Envelope(format!("to {to}: {e}")))?;

    // Subject: reply threading wins, then an explicit override, then a
    // sensible non-empty default (a blank subject reads as a malformed
    // orphan in most MUAs).
    let subject = match reply {
        Some(r) => build_reply_subject(r.subject.as_deref()),
        None => subject_override
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_string)
            .unwrap_or_else(|| "(no subject)".to_string()),
    };

    let mut builder = Message::builder()
        .from(from_addr)
        .to(to_addr)
        .subject(subject)
        // Stamp an explicit Message-ID (lettre only generates one when asked;
        // otherwise the relay assigns it and we never learn the value). The
        // send path records this id so the IMAP poll task can recognize the
        // channel's own mail if it echoes back into the monitored mailbox and
        // mark it `is_self` — breaking the wayland#547 self-reply loop.
        .message_id(Some(format!("<{}>", make_outbound_message_id(from))));

    if let Some(r) = reply {
        for hv in reply_headers(r) {
            builder = builder.raw_header(hv);
        }
    }

    // Text-only: a single text/plain body (the prior behavior). With
    // attachments: a multipart/mixed body — the text part first, then one
    // octet-typed part per attachment with Content-Disposition: attachment so
    // a file/image reply isn't silently dropped.
    if attachments.is_empty() {
        return builder
            .header(ContentType::TEXT_PLAIN)
            .body(text.to_string())
            .map_err(|e| EmailError::Envelope(format!("body: {e}")));
    }

    let mut multipart = MultiPart::mixed().singlepart(SinglePart::plain(text.to_string()));
    for att in attachments {
        let ct = ContentType::parse(&att.content_type)
            .unwrap_or(ContentType::parse("application/octet-stream").expect("static mime"));
        multipart =
            multipart.singlepart(Attachment::new(att.filename.clone()).body(att.bytes.clone(), ct));
    }
    builder
        .multipart(multipart)
        .map_err(|e| EmailError::Envelope(format!("multipart body: {e}")))
}

/// A resolved outbound attachment, ready to embed as a MIME part.
#[derive(Debug)]
pub(crate) struct OutboundAttachment {
    pub(crate) filename: String,
    pub(crate) content_type: String,
    pub(crate) bytes: Vec<u8>,
}

/// Resolve an [`OutgoingMessage`] attachment reference to embeddable bytes.
///
/// **Only `data:<mime>;base64,<payload>` references are accepted** — inline
/// bytes, no I/O and no network. Everything else (local filesystem paths,
/// `http(s)://` URLs, anything unrecognized) is rejected with an explicit
/// error so the attachment fails loudly rather than being silently dropped.
///
/// Why no local paths and no remote fetch: `OutgoingMessage.attachments` is
/// produced by the agent, and inbound channel content can steer the agent via
/// prompt injection. Reading an arbitrary local path here (e.g.
/// `/etc/passwd`, `~/.ssh/id_rsa`) would let an attacker exfiltrate host files
/// by email; fetching an arbitrary URL (email has no media-host allowlist)
/// would be an SSRF surface. Requiring inline `data:` bytes removes both: the
/// caller (host integration / a deliberate tool) must commit the exact bytes,
/// and the inbound side already emits `data:` URLs for attachments, so
/// receive-then-reply round-trips unchanged.
///
/// [`OutgoingMessage`]: wcore_channels::outgoing::OutgoingMessage
pub(crate) fn resolve_attachment(reference: &str) -> Result<OutboundAttachment, EmailError> {
    let trimmed = reference.trim();

    let Some(rest) = trimmed.strip_prefix("data:") else {
        return Err(EmailError::Envelope(format!(
            "unsupported email attachment reference (only data: URLs are accepted; \
             local paths and remote URLs are rejected to avoid file-exfiltration / SSRF): {reference}"
        )));
    };
    let (meta, b64) = rest.split_once(";base64,").ok_or_else(|| {
        EmailError::Envelope(format!("unsupported data URL attachment: {reference}"))
    })?;
    let mime = if meta.is_empty() {
        "application/octet-stream"
    } else {
        meta
    };
    Ok(OutboundAttachment {
        filename: format!("attachment{}", ext_for_mime(mime)),
        content_type: mime.to_string(),
        bytes: crate::imap::decode_base64_bytes(b64),
    })
}

/// A filename extension (including the dot) for a data-URL mime, so inline
/// attachments land with a sensible name. Empty when unknown.
fn ext_for_mime(mime: &str) -> &'static str {
    match mime {
        "image/png" => ".png",
        "image/jpeg" => ".jpg",
        "image/gif" => ".gif",
        "image/webp" => ".webp",
        "application/pdf" => ".pdf",
        "text/plain" => ".txt",
        "text/csv" => ".csv",
        "application/json" => ".json",
        "application/zip" => ".zip",
        "audio/mpeg" => ".mp3",
        "video/mp4" => ".mp4",
        _ => "",
    }
}

/// Send one message with retry. `Arc<dyn MailSender>` so the same
/// sender instance can be shared (cheap clone) between the channel and
/// any tests.
pub(crate) async fn send_with_retry(
    sender: Arc<dyn MailSender>,
    msg: Message,
) -> Result<Response, EmailError> {
    let mut last_err = EmailError::Smtp("no attempts made".to_string());

    for attempt in 0..SEND_MAX_ATTEMPTS {
        if attempt > 0 {
            let sleep_ms = exp_backoff_ms(attempt);
            tokio::time::sleep(Duration::from_millis(sleep_ms)).await;
        }
        // Each retry re-builds the message — lettre's Message is consumed
        // by send(). Both first-try and retry paths clone the same input
        // message; the conditional is kept for parity with the retry shape
        // even though both arms currently produce identical clones.
        let attempt_msg = msg.clone();
        match sender.send(attempt_msg).await {
            Ok(r) => return Ok(r),
            Err(SendError::Auth(m)) => {
                return Err(EmailError::Auth(m));
            }
            Err(SendError::Permanent(m)) => {
                return Err(EmailError::Rejected(m));
            }
            Err(SendError::Transient(m)) => {
                last_err = EmailError::Smtp(m);
                continue;
            }
        }
    }
    Err(last_err)
}

fn exp_backoff_ms(attempt: u32) -> u64 {
    // attempt=1 -> 200ms, attempt=2 -> 400ms, attempt=3 -> 800ms, ...
    let shift = attempt.saturating_sub(1).min(10);
    SEND_BASE_BACKOFF_MS
        .saturating_mul(1u64 << shift)
        .min(SEND_MAX_BACKOFF_MS)
}

/// Pull a synthetic platform-id from the SMTP response. Many servers
/// embed a queue id in the response message ("250 2.0.0 Ok: queued as
/// ABC123"); we extract the trailing token after "queued as" when
/// present, else fall back to a hash of the bytes so callers always
/// have a stable correlation id.
pub(crate) fn response_message_id(r: &Response) -> String {
    let joined = r.message().collect::<Vec<_>>().join(" ");
    if let Some(idx) = joined.to_lowercase().find("queued as") {
        let tail = &joined[idx + "queued as".len()..];
        let id: String = tail
            .trim()
            .chars()
            .take_while(|c| !c.is_whitespace())
            .collect();
        if !id.is_empty() {
            return id;
        }
    }
    // Fallback: hash of the body bytes for a stable id.
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    use std::hash::{Hash, Hasher};
    joined.hash(&mut hasher);
    format!("smtp-{:x}", hasher.finish())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    /// In-memory MailSender for tests. Records every send + a programmable
    /// outcome script — pop one outcome per call.
    pub struct RecordingSender {
        pub sent: Mutex<Vec<Message>>,
        pub outcomes: Mutex<Vec<Result<Response, SendError>>>,
    }

    impl RecordingSender {
        pub fn new(outcomes: Vec<Result<Response, SendError>>) -> Arc<Self> {
            Arc::new(Self {
                sent: Mutex::new(Vec::new()),
                outcomes: Mutex::new(outcomes),
            })
        }

        fn make_response(body: &str) -> Response {
            // lettre's Response constructor is not pub; round-trip parse one.
            // Format: code + at least one info line.
            use std::str::FromStr;
            // The lettre `Response` type implements FromStr in 0.11.
            Response::from_str(body).expect("hand-crafted ok response parses")
        }

        /// Helper to make an `Ok(Response)` outcome that embeds a queue id.
        pub fn ok_with_queue_id(id: &str) -> Result<Response, SendError> {
            Ok(Self::make_response(&format!(
                "250 2.0.0 Ok: queued as {id}\r\n"
            )))
        }
    }

    #[async_trait]
    impl MailSender for RecordingSender {
        async fn send(&self, msg: Message) -> Result<Response, SendError> {
            self.sent.lock().unwrap().push(msg);
            let mut outcomes = self.outcomes.lock().unwrap();
            if outcomes.is_empty() {
                return Err(SendError::Transient("no more outcomes scripted".into()));
            }
            outcomes.remove(0)
        }
    }

    #[tokio::test]
    async fn send_records_envelope_from_to_body() {
        let sender = RecordingSender::new(vec![RecordingSender::ok_with_queue_id("Q1")]);
        let msg = build_message(
            "bot@acme.com",
            "ops@acme.com",
            "hello body",
            None,
            None,
            &[],
        )
        .unwrap();
        let resp = send_with_retry(sender.clone(), msg).await.unwrap();
        assert_eq!(response_message_id(&resp), "Q1");
        let sent = sender.sent.lock().unwrap();
        assert_eq!(sent.len(), 1);
        let rfc = String::from_utf8_lossy(&sent[0].formatted()).to_string();
        assert!(rfc.contains("From: bot@acme.com"), "rfc = {rfc}");
        assert!(rfc.contains("To: ops@acme.com"), "rfc = {rfc}");
        assert!(rfc.contains("hello body"), "rfc = {rfc}");
    }

    #[tokio::test]
    async fn send_retries_transient_then_succeeds() {
        let sender = RecordingSender::new(vec![
            Err(SendError::Transient("conn reset".into())),
            Err(SendError::Transient("conn reset".into())),
            RecordingSender::ok_with_queue_id("Q2"),
        ]);
        let msg = build_message(
            "bot@acme.com",
            "ops@acme.com",
            "after retry",
            None,
            None,
            &[],
        )
        .unwrap();
        let resp = send_with_retry(sender.clone(), msg).await.unwrap();
        assert_eq!(response_message_id(&resp), "Q2");
        assert_eq!(sender.sent.lock().unwrap().len(), 3);
    }

    #[tokio::test]
    async fn send_auth_failure_is_permanent_short_circuit() {
        let sender = RecordingSender::new(vec![
            Err(SendError::Auth("535 5.7.8 bad creds".into())),
            // Outcome below must NOT be consumed.
            RecordingSender::ok_with_queue_id("Q3"),
        ]);
        let msg = build_message(
            "bot@acme.com",
            "ops@acme.com",
            "should not retry",
            None,
            None,
            &[],
        )
        .unwrap();
        let err = send_with_retry(sender.clone(), msg)
            .await
            .expect_err("auth");
        match err {
            EmailError::Auth(_) => {}
            other => panic!("expected EmailError::Auth, got {other:?}"),
        }
        // Only the first outcome consumed.
        assert_eq!(sender.sent.lock().unwrap().len(), 1);
    }

    #[tokio::test]
    async fn send_permanent_rejection_short_circuits() {
        let sender = RecordingSender::new(vec![
            Err(SendError::Permanent("550 user unknown".into())),
            RecordingSender::ok_with_queue_id("nope"),
        ]);
        let msg = build_message("bot@acme.com", "ops@acme.com", "nope", None, None, &[]).unwrap();
        let err = send_with_retry(sender.clone(), msg)
            .await
            .expect_err("permanent");
        match err {
            EmailError::Rejected(_) => {}
            other => panic!("expected EmailError::Rejected, got {other:?}"),
        }
        assert_eq!(sender.sent.lock().unwrap().len(), 1);
    }

    #[test]
    fn build_message_rejects_bad_from() {
        let err = build_message("not-an-email", "ops@acme.com", "x", None, None, &[])
            .expect_err("expected envelope error");
        match err {
            EmailError::Envelope(_) => {}
            other => panic!("expected Envelope, got {other:?}"),
        }
    }

    // ----- wayland#547 loop guard: outbound Message-ID stamping -----

    #[test]
    fn build_message_stamps_extractable_message_id() {
        let msg =
            build_message("Bot <bot@acme.com>", "ops@acme.com", "hi", None, None, &[]).unwrap();
        let id = outbound_message_id(&msg).expect("Message-ID stamped");
        assert!(id.starts_with("wl-"), "id = {id}");
        // Domain comes from the From addr-spec even in display-name form.
        assert!(id.ends_with("@acme.com"), "id = {id}");
        // The bracketed form is what actually hits the wire.
        let rfc = String::from_utf8_lossy(&msg.formatted()).to_string();
        assert!(
            rfc.contains(&format!("Message-ID: <{id}>")),
            "wire bytes missing stamped Message-ID; rfc = {rfc}"
        );
    }

    #[test]
    fn outbound_message_ids_are_unique_per_send() {
        let a = make_outbound_message_id("bot@acme.com");
        let b = make_outbound_message_id("bot@acme.com");
        assert_ne!(a, b);
    }

    #[test]
    fn outbound_message_id_domain_falls_back_when_from_has_no_at() {
        let id = make_outbound_message_id("garbage");
        assert!(id.ends_with("@wayland.invalid"), "id = {id}");
    }

    #[test]
    fn build_message_with_attachments_is_multipart() {
        let att = OutboundAttachment {
            filename: "report.pdf".to_string(),
            content_type: "application/pdf".to_string(),
            bytes: b"%PDF-1.4 hello".to_vec(),
        };
        let msg = build_message(
            "bot@acme.com",
            "ops@acme.com",
            "see attached",
            None,
            Some("Daily report"),
            std::slice::from_ref(&att),
        )
        .expect("multipart build");
        let raw = String::from_utf8_lossy(&msg.formatted()).to_string();
        assert!(raw.contains("multipart/mixed"), "not multipart: {raw}");
        assert!(
            raw.contains("Content-Disposition: attachment"),
            "no attachment disposition: {raw}"
        );
        assert!(raw.contains("report.pdf"), "filename missing: {raw}");
        // Text part still present.
        assert!(raw.contains("see attached"), "body text missing");
    }

    #[test]
    fn resolve_attachment_decodes_data_url() {
        // base64 of "hello" is aGVsbG8=
        let a = resolve_attachment("data:image/png;base64,aGVsbG8=").expect("data url");
        assert_eq!(a.bytes, b"hello");
        assert_eq!(a.content_type, "image/png");
        assert_eq!(a.filename, "attachment.png");
    }

    #[test]
    fn resolve_attachment_rejects_remote_url() {
        let err = resolve_attachment("https://evil.example/x.png").expect_err("remote rejected");
        match err {
            EmailError::Envelope(m) => assert!(m.contains("unsupported"), "msg: {m}"),
            other => panic!("expected Envelope, got {other:?}"),
        }
    }

    #[test]
    fn resolve_attachment_rejects_local_path_no_file_read() {
        // A local filesystem path must NOT be read — otherwise a prompt-injected
        // agent could exfiltrate host files by email. Even an existing, readable
        // file is rejected: only data: URLs are accepted.
        let dir = std::env::temp_dir().join(format!("wcore_email_att_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("secret.txt");
        std::fs::write(&path, b"top secret").unwrap();
        let err = resolve_attachment(path.to_str().unwrap()).expect_err("local path rejected");
        match err {
            EmailError::Envelope(m) => assert!(m.contains("unsupported"), "msg: {m}"),
            other => panic!("expected Envelope, got {other:?}"),
        }
        // And a classic traversal target is rejected without touching the FS.
        assert!(resolve_attachment("/etc/passwd").is_err());
        assert!(resolve_attachment("../../etc/passwd").is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn response_message_id_extracts_queue_id() {
        use std::str::FromStr;
        let r = Response::from_str("250 2.0.0 Ok: queued as DEAD-BEEF\r\n").unwrap();
        assert_eq!(response_message_id(&r), "DEAD-BEEF");
    }

    #[test]
    fn response_message_id_falls_back_to_hash() {
        use std::str::FromStr;
        let r = Response::from_str("250 2.0.0 fine and dandy\r\n").unwrap();
        let id = response_message_id(&r);
        assert!(id.starts_with("smtp-"), "id = {id}");
    }

    #[test]
    fn reply_subject_prefixes_re_once() {
        assert_eq!(build_reply_subject(Some("Hello there")), "Re: Hello there");
        // Already-Re subject is not double-prefixed (case-insensitive).
        assert_eq!(
            build_reply_subject(Some("Re: Hello there")),
            "Re: Hello there"
        );
        assert_eq!(build_reply_subject(Some("RE: shouting")), "RE: shouting");
        assert_eq!(build_reply_subject(Some("re: lower")), "re: lower");
        // Whitespace-only / empty / unknown fall back to a bare "Re:".
        assert_eq!(build_reply_subject(Some("   ")), "Re:");
        assert_eq!(build_reply_subject(None), "Re:");
    }

    #[test]
    fn references_appends_parent_to_existing_chain() {
        let r = ReplyContext {
            message_id: "msg-2@x".into(),
            subject: None,
            references: Some("<msg-0@x> <msg-1@x>".into()),
        };
        assert_eq!(
            build_references(&r).unwrap(),
            "<msg-0@x> <msg-1@x> <msg-2@x>"
        );
        // Fresh chain (no prior References) starts with just the parent.
        let r2 = ReplyContext {
            message_id: "root@x".into(),
            subject: None,
            references: None,
        };
        assert_eq!(build_references(&r2).unwrap(), "<root@x>");
        // Empty parent id -> no References.
        let r3 = ReplyContext::default();
        assert!(build_references(&r3).is_none());
    }

    #[test]
    fn reply_message_carries_threading_headers_and_subject() {
        let reply = ReplyContext {
            message_id: "orig-123@acme.com".into(),
            subject: Some("Status update".into()),
            references: Some("<root@acme.com>".into()),
        };
        let msg = build_message(
            "bot@acme.com",
            "ops@acme.com",
            "thread reply body",
            Some(&reply),
            None,
            &[],
        )
        .unwrap();
        let rfc = String::from_utf8_lossy(&msg.formatted()).to_string();
        assert!(
            rfc.contains("In-Reply-To: <orig-123@acme.com>"),
            "rfc = {rfc}"
        );
        assert!(
            rfc.contains("References: <root@acme.com> <orig-123@acme.com>"),
            "rfc = {rfc}"
        );
        assert!(rfc.contains("Subject: Re: Status update"), "rfc = {rfc}");
        assert!(rfc.contains("thread reply body"), "rfc = {rfc}");
    }

    #[test]
    fn non_reply_message_uses_default_subject_not_blank() {
        let msg = build_message("bot@acme.com", "ops@acme.com", "fresh", None, None, &[]).unwrap();
        let rfc = String::from_utf8_lossy(&msg.formatted()).to_string();
        // A blank Subject reads as a malformed orphan; we emit a default.
        assert!(rfc.contains("Subject: (no subject)"), "rfc = {rfc}");
    }

    #[test]
    fn non_reply_message_honours_subject_override() {
        let msg = build_message(
            "bot@acme.com",
            "ops@acme.com",
            "fresh",
            None,
            Some("Weekly report"),
            &[],
        )
        .unwrap();
        let rfc = String::from_utf8_lossy(&msg.formatted()).to_string();
        assert!(rfc.contains("Subject: Weekly report"), "rfc = {rfc}");
    }
}
