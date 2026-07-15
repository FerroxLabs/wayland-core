//! Per-client observation of normalized outbound HTTP attempts.
//!
//! Observation is deliberately separate from policy: policy decides whether a
//! request may leave the process, while an observer receives one terminal,
//! secret-minimized event for each successfully-built request. The default is
//! a no-op, so callers opt in explicitly and no process-global recorder is
//! introduced.

use std::collections::VecDeque;
use std::fmt::Write as _;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::{Arc, Mutex, OnceLock};

use sha2::{Digest, Sha256};

/// Normalized logical destination of an outbound HTTP request.
///
/// Userinfo, path, query, fragment, headers, and body are never retained. The
/// path and query are represented only by a length-framed SHA-256 digest.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct EgressDestination {
    pub scheme: String,
    pub host: String,
    pub effective_port: Option<u16>,
    pub path_query_sha256: String,
}

/// Stable transport-error classes suitable for deterministic evidence.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EgressTransportErrorClass {
    Timeout,
    Connect,
    Redirect,
    Request,
    Body,
    Decode,
    Unknown,
}

/// Terminal outcome of one successfully-built outbound request.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum EgressOutcome {
    /// Policy refused the request before network I/O.
    Denied,
    /// The network returned HTTP response headers. This does not assert that a
    /// later streaming response body completed successfully.
    HttpResponse { status: u16 },
    /// Request execution failed before response headers arrived.
    TransportError { class: EgressTransportErrorClass },
    /// The send future was dropped while awaiting the async policy decision.
    AbandonedBeforeDecision,
    /// Policy allowed the request, but the send future was dropped before
    /// response headers or a transport error arrived.
    AbandonedAfterAllow,
    /// Policy allowed the request, but its pre-dispatch hook failed. The
    /// request did not reach the network.
    BeforeDispatchFailed,
}

/// One normalized terminal event for an outbound HTTP attempt.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct EgressEvent {
    pub attempt_id: u64,
    pub method: String,
    pub destination: EgressDestination,
    pub outcome: EgressOutcome,
}

/// Synchronous, non-blocking sink for egress events.
///
/// Implementations must return quickly. The send path catches observer panics
/// so telemetry can never change the request's policy or network result.
pub trait EgressObserver: Send + Sync {
    fn observe(&self, event: EgressEvent);
}

/// Shared observer handle carried cheaply by clients and request builders.
pub type SharedEgressObserver = Arc<dyn EgressObserver>;

static GLOBAL_OBSERVER: OnceLock<SharedEgressObserver> = OnceLock::new();

/// Install the process-wide observer used by clients that do not carry an
/// explicit per-client observer. Installation is one-shot.
pub fn install_global_observer(observer: SharedEgressObserver) -> Result<(), SharedEgressObserver> {
    GLOBAL_OBSERVER.set(observer)
}

/// True when a process-wide observer has been installed.
pub fn global_observer_installed() -> bool {
    GLOBAL_OBSERVER.get().is_some()
}

/// Default observer that intentionally records nothing.
#[derive(Debug, Default, Clone, Copy)]
pub struct NoopEgressObserver;

impl EgressObserver for NoopEgressObserver {
    fn observe(&self, _event: EgressEvent) {}
}

/// Proxy used by default clients so an observer installed after client
/// construction still receives subsequent request events.
#[derive(Debug, Default, Clone, Copy)]
pub struct GlobalDefaultObserver;

impl EgressObserver for GlobalDefaultObserver {
    fn observe(&self, event: EgressEvent) {
        if let Some(observer) = GLOBAL_OBSERVER.get() {
            notify_fail_open(observer, event);
        }
    }
}

pub(crate) fn default_observer() -> SharedEgressObserver {
    Arc::new(GlobalDefaultObserver)
}

/// Immutable snapshot of a bounded recorder.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EgressRecorderSnapshot {
    pub events: Vec<EgressEvent>,
    /// Number of terminal events rejected because the recorder was full.
    /// Any non-zero value means the evidence is incomplete.
    pub dropped_events: u64,
}

#[derive(Debug)]
struct RecorderState {
    events: VecDeque<EgressEvent>,
    dropped_events: u64,
}

/// Bounded in-memory observer for deterministic tests.
///
/// Once full, it preserves the earliest events and counts every later event as
/// dropped. It never allocates beyond `capacity` events and never blocks on
/// external I/O.
#[derive(Debug)]
pub struct BoundedEgressRecorder {
    capacity: usize,
    state: Mutex<RecorderState>,
}

impl BoundedEgressRecorder {
    pub fn new(capacity: usize) -> Self {
        Self {
            capacity,
            state: Mutex::new(RecorderState {
                events: VecDeque::with_capacity(capacity),
                dropped_events: 0,
            }),
        }
    }

    pub fn snapshot(&self) -> EgressRecorderSnapshot {
        match self.state.lock() {
            Ok(state) => EgressRecorderSnapshot {
                events: state.events.iter().cloned().collect(),
                dropped_events: state.dropped_events,
            },
            // A poisoned recorder must not panic the request path. Mark the
            // snapshot incomplete rather than presenting empty evidence as
            // trustworthy.
            Err(_) => EgressRecorderSnapshot {
                events: Vec::new(),
                dropped_events: u64::MAX,
            },
        }
    }
}

impl EgressObserver for BoundedEgressRecorder {
    fn observe(&self, event: EgressEvent) {
        let Ok(mut state) = self.state.lock() else {
            return;
        };
        if state.events.len() < self.capacity {
            state.events.push_back(event);
        } else {
            state.dropped_events = state.dropped_events.saturating_add(1);
        }
    }
}

#[derive(Debug)]
struct EgressEventBase {
    attempt_id: u64,
    method: String,
    destination: EgressDestination,
}

#[derive(Debug, Clone, Copy)]
enum AttemptStage {
    BeforeDecision,
    AfterAllow,
}

/// Drop guard that guarantees one terminal event even when the async send
/// future is cancelled between policy evaluation and request completion.
pub(crate) struct EgressAttemptGuard {
    observer: SharedEgressObserver,
    base: Option<EgressEventBase>,
    stage: AttemptStage,
}

impl EgressAttemptGuard {
    pub(crate) fn new(
        observer: SharedEgressObserver,
        attempt_id: u64,
        request: &reqwest::Request,
    ) -> Self {
        Self {
            observer,
            base: Some(EgressEventBase {
                attempt_id,
                method: request.method().as_str().to_string(),
                destination: normalize_destination(request.url()),
            }),
            stage: AttemptStage::BeforeDecision,
        }
    }

    pub(crate) fn mark_allowed(&mut self) {
        self.stage = AttemptStage::AfterAllow;
    }

    pub(crate) fn finish(&mut self, outcome: EgressOutcome) {
        self.emit(outcome);
    }

    fn emit(&mut self, outcome: EgressOutcome) {
        let Some(base) = self.base.take() else {
            return;
        };
        notify_fail_open(
            &self.observer,
            EgressEvent {
                attempt_id: base.attempt_id,
                method: base.method,
                destination: base.destination,
                outcome,
            },
        );
    }
}

impl Drop for EgressAttemptGuard {
    fn drop(&mut self) {
        let outcome = match self.stage {
            AttemptStage::BeforeDecision => EgressOutcome::AbandonedBeforeDecision,
            AttemptStage::AfterAllow => EgressOutcome::AbandonedAfterAllow,
        };
        self.emit(outcome);
    }
}

pub(crate) fn classify_transport_error(error: &reqwest::Error) -> EgressTransportErrorClass {
    if error.is_timeout() {
        EgressTransportErrorClass::Timeout
    } else if error.is_connect() {
        EgressTransportErrorClass::Connect
    } else if error.is_redirect() {
        EgressTransportErrorClass::Redirect
    } else if error.is_request() {
        EgressTransportErrorClass::Request
    } else if error.is_body() {
        EgressTransportErrorClass::Body
    } else if error.is_decode() {
        EgressTransportErrorClass::Decode
    } else {
        EgressTransportErrorClass::Unknown
    }
}

fn notify_fail_open(observer: &SharedEgressObserver, event: EgressEvent) {
    let _ = catch_unwind(AssertUnwindSafe(|| observer.observe(event)));
}

fn normalize_destination(url: &reqwest::Url) -> EgressDestination {
    EgressDestination {
        scheme: url.scheme().to_string(),
        host: url.host_str().unwrap_or_default().to_string(),
        effective_port: url.port_or_known_default(),
        path_query_sha256: path_query_sha256(url.path(), url.query()),
    }
}

fn path_query_sha256(path: &str, query: Option<&str>) -> String {
    let mut hasher = Sha256::new();
    hash_framed(&mut hasher, path.as_bytes());
    match query {
        Some(query) => {
            hasher.update([1]);
            hash_framed(&mut hasher, query.as_bytes());
        }
        None => hasher.update([0]),
    }
    let digest = hasher.finalize();
    let mut encoded = String::with_capacity(digest.len() * 2);
    for byte in digest {
        let _ = write!(&mut encoded, "{byte:02x}");
    }
    encoded
}

fn hash_framed(hasher: &mut Sha256, value: &[u8]) {
    hasher.update((value.len() as u64).to_be_bytes());
    hasher.update(value);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn event(id: u64) -> EgressEvent {
        EgressEvent {
            attempt_id: id,
            method: "GET".to_string(),
            destination: EgressDestination {
                scheme: "https".to_string(),
                host: "example.test".to_string(),
                effective_port: Some(443),
                path_query_sha256: "0".repeat(64),
            },
            outcome: EgressOutcome::HttpResponse { status: 200 },
        }
    }

    #[test]
    fn normalization_excludes_raw_url_secrets() {
        const SECRET: &str = "WCORE-CANARY-3f91";
        let url = reqwest::Url::parse(&format!(
            "https://user:{SECRET}@Example.TEST/private/{SECRET}?token={SECRET}#{SECRET}"
        ))
        .unwrap();
        let destination = normalize_destination(&url);

        assert_eq!(destination.scheme, "https");
        assert_eq!(destination.host, "example.test");
        assert_eq!(destination.effective_port, Some(443));
        assert_eq!(destination.path_query_sha256.len(), 64);
        assert!(!format!("{destination:?}").contains(SECRET));
    }

    #[test]
    fn path_query_digest_uses_unambiguous_framing() {
        assert_ne!(
            path_query_sha256("/a", Some("bc")),
            path_query_sha256("/ab", Some("c"))
        );
        assert_ne!(
            path_query_sha256("/a", None),
            path_query_sha256("/a", Some(""))
        );
    }

    #[test]
    fn bounded_recorder_reports_overflow_without_growing() {
        let recorder = BoundedEgressRecorder::new(1);
        recorder.observe(event(1));
        recorder.observe(event(2));

        let snapshot = recorder.snapshot();
        assert_eq!(snapshot.events, vec![event(1)]);
        assert_eq!(snapshot.dropped_events, 1);
    }
}
