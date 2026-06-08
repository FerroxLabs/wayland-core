//! SMTP outbound. Wraps `lettre::AsyncSmtpTransport` behind a
//! `MailSender` trait so tests can stand in a recording mock without
//! booting a real SMTP server.
//!
//! Retry policy mirrors `wcore-channel-slack` / `wcore-channel-telegram`:
//! up to `SEND_MAX_ATTEMPTS` tries, exponential backoff on transient
//! errors, permanent short-circuit on auth failure.

use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use lettre::message::{Message, header::ContentType};
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

/// Build a minimal text/plain `lettre::Message` from the outbound
/// envelope. `from` is the channel's configured From: address; `to` is
/// the outbound conversation_id (one recipient per send — multi-recipient
/// support lives behind a future enhancement).
pub(crate) fn build_message(from: &str, to: &str, text: &str) -> Result<Message, EmailError> {
    let from_addr = from
        .parse()
        .map_err(|e| EmailError::Envelope(format!("from {from}: {e}")))?;
    let to_addr = to
        .parse()
        .map_err(|e| EmailError::Envelope(format!("to {to}: {e}")))?;
    Message::builder()
        .from(from_addr)
        .to(to_addr)
        .subject("")
        .header(ContentType::TEXT_PLAIN)
        .body(text.to_string())
        .map_err(|e| EmailError::Envelope(format!("body: {e}")))
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
        let msg = build_message("bot@acme.com", "ops@acme.com", "hello body").unwrap();
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
        let msg = build_message("bot@acme.com", "ops@acme.com", "after retry").unwrap();
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
        let msg = build_message("bot@acme.com", "ops@acme.com", "should not retry").unwrap();
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
        let msg = build_message("bot@acme.com", "ops@acme.com", "nope").unwrap();
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
        let err = build_message("not-an-email", "ops@acme.com", "x")
            .expect_err("expected envelope error");
        match err {
            EmailError::Envelope(_) => {}
            other => panic!("expected Envelope, got {other:?}"),
        }
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
}
