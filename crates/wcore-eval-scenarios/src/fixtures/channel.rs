//! The hermetic channel sink — an INDEPENDENT destination for outbound
//! deliveries.
//!
//! Phase 24 plan 24-03, Task 3, pulled forward because it is the one artefact
//! Phase 24 Success Criterion 1 cannot be closed without.
//!
//! # Why this exists at all
//!
//! Lane 24-B proved delivery CONTINUITY across a `kill -9`: twelve deliveries
//! were carried, counted and named. But it counted them by reading the
//! gateway's own delivery ledger. Reading it out-of-process rules out a
//! runtime grading its own in-memory state; it does not rule out the runtime
//! being wrong about the world. **The ledger is the sender's record of what it
//! believes it did.** A gateway whose sends never leave the process writes
//! exactly the same ledger as one whose sends all land.
//!
//! So `delivered` is not a fact this workspace could observe until something
//! other than the sender wrote it down. That is this module.
//!
//! # What makes it independent, concretely
//!
//! Three properties, and all three are load-bearing:
//!
//! 1. **It is a different process.** The sink binary
//!    (`src/bin/wayland-channel-sink.rs`) is started before the gateway and
//!    outlives it. The gateway cannot append to the arrivals journal except
//!    by completing a real TCP round trip to it.
//! 2. **The sink assigns the message identity.** The `ts` returned to the
//!    sender comes from the sink's own monotonic counter, so a receipt the
//!    sender holds is proof the sink saw the request, not proof the sender
//!    formatted one.
//! 3. **The arrival is journalled BEFORE the response is written.** An arrival
//!    the sender never learns about is still recorded. That asymmetry is the
//!    whole point: it is the only way to distinguish "did not arrive" from
//!    "arrived, and the sender does not know it".
//!
//! # The stall mode, and why a clean sink cannot find the defect
//!
//! AGENTS.md §11: *a live test proves nothing if its scenario is too clean to
//! reach the defect.* A sink that always answers immediately can only ever
//! produce deliveries in the `Settled` state, so a kill lands between
//! deliveries and every carried delivery is of the `Accepted` class. The
//! interesting class — `Attempted`, outcome UNKNOWN — is reachable only if the
//! destination can be made to accept a request and then never answer.
//!
//! [`SinkMode::StallAfter`] does exactly that: it journals arrival number
//! `n + 1` and then holds the connection open forever. A gateway killed in
//! that window has a delivery that genuinely LANDED and that its own ledger
//! can only describe as unknown. Retrying it duplicates at the destination;
//! dropping it is safe. Which of those the product does is a fact, and this
//! module is the instrument that reads it.
//!
//! # Shape
//!
//! Slack's Web API, because `wcore-channel-slack` already carries a
//! `api_base_url` override in its on-disk TOML schema (`#[serde(default)]`).
//! That means the REAL production adapter — unmodified, auto-registered by
//! `wcore-channels-registry` from `$WAYLAND_HOME/channels/*.toml` — can be
//! pointed here. No fixture adapter, no eleventh platform, no vendor
//! credential, and the code path under test is the shipped one.
//!
//! # Secrets
//!
//! The bearer presented by the adapter is NEVER journalled. Only a truncated
//! SHA-256 fingerprint is, which is enough to prove the sender authenticated
//! as one stable identity and not enough to be a credential.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use axum::Router;
use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use serde::{Deserialize, Serialize};
use serde_json::json;
use sha2::{Digest, Sha256};
use tokio::sync::oneshot;
use tokio::task::JoinHandle;

/// One recorded arrival at the sink.
///
/// `seq` and `ts` are the SINK's, never the sender's. `text` is the
/// discriminator the tally is computed over: two records with the same `text`
/// are the same logical delivery, seen twice.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Arrival {
    /// The sink's own monotonic arrival number, from 1.
    pub seq: u64,
    /// Sink-assigned message identity, handed back to the sender as Slack `ts`.
    pub ts: String,
    /// Which endpoint took it.
    pub endpoint: String,
    /// Destination conversation the sender addressed.
    pub conversation_id: String,
    /// The delivery body. The uniqueness tally is computed over this.
    pub text: String,
    /// Truncated SHA-256 of the presented bearer. NEVER the bearer itself.
    pub auth_fingerprint: String,
    /// Whether the sink answered this request, or accepted it and stalled.
    /// A stalled arrival is a delivery that LANDED and that the sender
    /// cannot know landed.
    pub answered: bool,
    /// Sink wall-clock, RFC3339.
    pub at: String,
}

/// How the sink behaves.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SinkMode {
    /// Journal and answer every request.
    Answer,
    /// Journal and answer the first `n` arrivals, then journal arrival
    /// `n + 1` and never answer it — holding the connection open.
    ///
    /// This is the only way to place a delivery in the ledger's `Attempted`
    /// (outcome-unknown) class from outside the process.
    StallAfter(u64),
}

/// A tally over an arrivals journal, computed by the READER of the journal.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ArrivalTally {
    /// Every record in the journal, including duplicates.
    pub total: usize,
    /// Distinct `text` values.
    pub unique: usize,
    /// `text` values that appear more than once — a duplicate delivery,
    /// which is what Success Criterion 1 forbids.
    pub duplicated: Vec<String>,
    /// Arrivals the sink accepted but never answered.
    pub stalled: usize,
}

impl ArrivalTally {
    /// Compute the tally over a journal's records.
    pub fn of(arrivals: &[Arrival]) -> Self {
        let mut counts: std::collections::BTreeMap<&str, usize> = std::collections::BTreeMap::new();
        for a in arrivals {
            *counts.entry(a.text.as_str()).or_default() += 1;
        }
        let duplicated: Vec<String> = counts
            .iter()
            .filter(|(_, n)| **n > 1)
            .map(|(t, _)| (*t).to_string())
            .collect();
        Self {
            total: arrivals.len(),
            unique: counts.len(),
            duplicated,
            stalled: arrivals.iter().filter(|a| !a.answered).count(),
        }
    }

    /// Whether this tally satisfies Criterion 1's delivery clause for an
    /// expected set of delivery bodies: every expected body arrived, and none
    /// arrived twice.
    ///
    /// Returns the losses so a failure names what was lost rather than only
    /// reporting a count mismatch.
    pub fn losses(&self, arrivals: &[Arrival], expected: &[String]) -> Vec<String> {
        let seen: std::collections::BTreeSet<&str> =
            arrivals.iter().map(|a| a.text.as_str()).collect();
        expected
            .iter()
            .filter(|e| !seen.contains(e.as_str()))
            .cloned()
            .collect()
    }
}

/// Read an arrivals journal written by a sink process.
///
/// A torn tail is an ERROR here rather than a silent skip: this file is the
/// evidence, and evidence that quietly discards its last line can turn a lost
/// delivery into a pass.
pub fn read_arrivals(path: impl AsRef<Path>) -> std::io::Result<Vec<Arrival>> {
    let raw = std::fs::read_to_string(path.as_ref())?;
    let mut out = Vec::new();
    for (i, line) in raw.lines().enumerate() {
        if line.trim().is_empty() {
            continue;
        }
        let a: Arrival = serde_json::from_str(line).map_err(|e| {
            std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                format!(
                    "{}: arrivals journal line {} is not a record: {e}",
                    path.as_ref().display(),
                    i + 1
                ),
            )
        })?;
        out.push(a);
    }
    Ok(out)
}

struct SinkState {
    mode: SinkMode,
    seq: AtomicU64,
    journal: Mutex<std::fs::File>,
    journal_path: PathBuf,
}

impl SinkState {
    /// Journal an arrival and flush it to a durable point BEFORE the caller
    /// gets an answer. A buffered arrival lost in the sink's own page cache
    /// would be indistinguishable from a delivery that never happened.
    fn record(
        &self,
        endpoint: &str,
        conversation_id: &str,
        text: &str,
        auth: &str,
        answered: bool,
    ) {
        let seq = self.seq.fetch_add(1, Ordering::SeqCst) + 1;
        let a = Arrival {
            seq,
            ts: format!("{seq}.000000"),
            endpoint: endpoint.to_string(),
            conversation_id: conversation_id.to_string(),
            text: text.to_string(),
            auth_fingerprint: fingerprint(auth),
            answered,
            at: chrono_now(),
        };
        let line = serde_json::to_string(&a).unwrap_or_else(|e| {
            format!("{{\"seq\":{seq},\"error\":\"arrival could not be encoded: {e}\"}}")
        });
        if let Ok(mut f) = self.journal.lock() {
            use std::io::Write as _;
            let _ = writeln!(f, "{line}");
            let _ = f.flush();
            let _ = f.sync_all();
        }
    }

    fn next_ts(&self) -> String {
        format!("{}.000000", self.seq.load(Ordering::SeqCst))
    }
}

/// Truncated SHA-256 of the presented bearer. Enough to prove one stable
/// identity authenticated; not a credential.
fn fingerprint(auth: &str) -> String {
    let token = auth.strip_prefix("Bearer ").unwrap_or(auth);
    if token.is_empty() {
        return "none".to_string();
    }
    let d = Sha256::digest(token.as_bytes());
    format!("sha256:{:x}", d)[..19].to_string()
}

/// RFC3339 now, without pulling `chrono` into this crate's dependency set.
fn chrono_now() -> String {
    let d = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default();
    format!("{}.{:09}Z", d.as_secs(), d.subsec_nanos())
}

/// A running hermetic sink.
pub struct ChannelSink {
    base_url: String,
    journal_path: PathBuf,
    shutdown: Option<oneshot::Sender<()>>,
    server: JoinHandle<std::io::Result<()>>,
}

impl ChannelSink {
    /// Bind on loopback (ephemeral port unless `port` is given) and start
    /// serving. `journal` is the arrivals file this sink owns.
    pub async fn start(
        journal: impl AsRef<Path>,
        mode: SinkMode,
        port: u16,
    ) -> std::io::Result<Self> {
        let journal_path = journal.as_ref().to_path_buf();
        if let Some(parent) = journal_path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let file = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&journal_path)?;

        let state = Arc::new(SinkState {
            mode,
            seq: AtomicU64::new(0),
            journal: Mutex::new(file),
            journal_path: journal_path.clone(),
        });

        let app = Router::new()
            .route("/api/chat.postMessage", post(post_message))
            .route("/api/reactions.add", post(reactions_add))
            .route("/api/auth.test", post(auth_test))
            .route("/api/auth.test", get(auth_test))
            .route("/_sink/arrivals", get(arrivals))
            .route("/_sink/health", get(health))
            .with_state(Arc::clone(&state));

        // Loopback only. A sink reachable off the host would make the
        // evidence depend on who else could reach it.
        let listener = tokio::net::TcpListener::bind(("127.0.0.1", port)).await?;
        let addr = listener.local_addr()?;
        let (tx, rx) = oneshot::channel();
        let server = tokio::spawn(async move {
            axum::serve(listener, app)
                .with_graceful_shutdown(async {
                    let _ = rx.await;
                })
                .await
        });

        Ok(Self {
            base_url: format!("http://127.0.0.1:{}", addr.port()),
            journal_path,
            shutdown: Some(tx),
            server,
        })
    }

    /// The value to write into a Slack adapter's `api_base_url`.
    pub fn base_url(&self) -> &str {
        &self.base_url
    }

    pub fn journal_path(&self) -> &Path {
        &self.journal_path
    }

    /// Everything journalled so far, read back off disk — not out of memory,
    /// so the test reads the same bytes an operator would.
    pub fn arrivals(&self) -> std::io::Result<Vec<Arrival>> {
        read_arrivals(&self.journal_path)
    }

    pub async fn shutdown(mut self) {
        if let Some(tx) = self.shutdown.take() {
            let _ = tx.send(());
        }
        let _ = self.server.await;
    }
}

fn bearer(headers: &HeaderMap) -> String {
    headers
        .get(axum::http::header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
        .to_string()
}

#[derive(Deserialize)]
struct PostMessageBody {
    #[serde(default)]
    channel: String,
    #[serde(default)]
    text: String,
}

async fn post_message(
    State(state): State<Arc<SinkState>>,
    headers: HeaderMap,
    body: String,
) -> Response {
    let parsed: PostMessageBody = serde_json::from_str(&body).unwrap_or(PostMessageBody {
        channel: String::new(),
        text: body.clone(),
    });
    let auth = bearer(&headers);

    // Decide BEFORE journalling, so the record states truthfully whether this
    // arrival was ever going to be answered.
    let will_answer = match state.mode {
        SinkMode::Answer => true,
        SinkMode::StallAfter(n) => state.seq.load(Ordering::SeqCst) < n,
    };

    state.record(
        "chat.postMessage",
        &parsed.channel,
        &parsed.text,
        &auth,
        will_answer,
    );

    if !will_answer {
        // Accept and never answer. The delivery has LANDED — it is in the
        // journal above — and the sender will never learn that it did. This
        // is the only shape that puts the gateway's ledger into its
        // outcome-unknown state from the outside.
        std::future::pending::<()>().await;
        unreachable!("the stall future never resolves");
    }

    let ts = state.next_ts();
    (
        StatusCode::OK,
        [(axum::http::header::CONTENT_TYPE, "application/json")],
        json!({ "ok": true, "ts": ts, "channel": parsed.channel }).to_string(),
    )
        .into_response()
}

#[derive(Deserialize)]
struct ReactionBody {
    #[serde(default)]
    channel: String,
    #[serde(default)]
    name: String,
}

async fn reactions_add(
    State(state): State<Arc<SinkState>>,
    headers: HeaderMap,
    body: String,
) -> Response {
    let parsed: ReactionBody = serde_json::from_str(&body).unwrap_or(ReactionBody {
        channel: String::new(),
        name: String::new(),
    });
    state.record(
        "reactions.add",
        &parsed.channel,
        &format!("reaction:{}", parsed.name),
        &bearer(&headers),
        true,
    );
    (
        StatusCode::OK,
        [(axum::http::header::CONTENT_TYPE, "application/json")],
        json!({ "ok": true }).to_string(),
    )
        .into_response()
}

/// The setup/authentication probe endpoint. Reports the identity the caller
/// authenticated AS — never the credential it presented.
async fn auth_test(State(_state): State<Arc<SinkState>>, headers: HeaderMap) -> Response {
    let auth = bearer(&headers);
    if auth.is_empty() {
        return (
            StatusCode::OK,
            [(axum::http::header::CONTENT_TYPE, "application/json")],
            json!({ "ok": false, "error": "not_authed" }).to_string(),
        )
            .into_response();
    }
    (
        StatusCode::OK,
        [(axum::http::header::CONTENT_TYPE, "application/json")],
        json!({
            "ok": true,
            "team": "f24c-fixture",
            "user": "wayland-core",
            "user_id": fingerprint(&auth),
        })
        .to_string(),
    )
        .into_response()
}

/// Control surface: the journal, served by the process that owns it.
async fn arrivals(State(state): State<Arc<SinkState>>) -> Response {
    match read_arrivals(&state.journal_path) {
        Ok(a) => (
            StatusCode::OK,
            [(axum::http::header::CONTENT_TYPE, "application/json")],
            serde_json::to_string(&json!({ "arrivals": a, "tally": ArrivalTally::of(&a) }))
                .unwrap_or_else(|e| format!("{{\"error\":\"{e}\"}}")),
        )
            .into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    }
}

async fn health(State(state): State<Arc<SinkState>>) -> Response {
    let mode = match state.mode {
        SinkMode::Answer => "answer".to_string(),
        SinkMode::StallAfter(n) => format!("stall_after:{n}"),
    };
    (
        StatusCode::OK,
        [(axum::http::header::CONTENT_TYPE, "application/json")],
        json!({ "ok": true, "mode": mode, "seq": state.seq.load(Ordering::SeqCst) }).to_string(),
    )
        .into_response()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn arr(seq: u64, text: &str, answered: bool) -> Arrival {
        Arrival {
            seq,
            ts: format!("{seq}.000000"),
            endpoint: "chat.postMessage".into(),
            conversation_id: "c".into(),
            text: text.into(),
            auth_fingerprint: "sha256:deadbeef".into(),
            answered,
            at: "0.0Z".into(),
        }
    }

    #[test]
    fn a_duplicate_at_the_sink_is_named_not_merely_counted() {
        // The failure Criterion 1 forbids. A tally that only reported
        // `total != unique` would make a duplicate visible but not
        // diagnosable; the caller needs to know WHICH delivery landed twice.
        let a = vec![
            arr(1, "d-1", true),
            arr(2, "d-2", true),
            arr(3, "d-1", true),
        ];
        let t = ArrivalTally::of(&a);
        assert_eq!(t.total, 3);
        assert_eq!(t.unique, 2);
        assert_eq!(t.duplicated, vec!["d-1".to_string()]);
    }

    #[test]
    fn a_stalled_arrival_still_counts_as_arrived() {
        // The whole reason the sink exists. This delivery LANDED; the sender
        // cannot know it did. A tally that dropped unanswered arrivals would
        // report a loss that did not happen — and would let a retry that
        // duplicated it look correct.
        let a = vec![arr(1, "d-1", true), arr(2, "d-2", false)];
        let t = ArrivalTally::of(&a);
        assert_eq!(t.total, 2, "an unanswered arrival is still an arrival");
        assert_eq!(t.stalled, 1);
        assert!(t.losses(&a, &["d-1".into(), "d-2".into()]).is_empty());
    }

    #[test]
    fn a_loss_is_named_by_the_body_that_never_arrived() {
        let a = vec![arr(1, "d-1", true)];
        let t = ArrivalTally::of(&a);
        assert_eq!(
            t.losses(&a, &["d-1".into(), "d-2".into(), "d-3".into()]),
            vec!["d-2".to_string(), "d-3".to_string()]
        );
    }

    #[test]
    fn the_bearer_is_never_journalled_only_a_fingerprint() {
        // A sink that recorded the token would turn every evidence file in
        // this phase into a credential leak.
        let f = fingerprint("Bearer xoxb-super-secret-value");
        assert!(f.starts_with("sha256:"));
        assert!(!f.contains("xoxb"));
        assert!(!f.contains("secret"));
        assert_eq!(fingerprint(""), "none");
    }

    #[tokio::test]
    async fn the_sink_assigns_the_message_identity_and_journals_before_answering() {
        let dir = tempfile::tempdir().unwrap();
        let j = dir.path().join("arrivals.jsonl");
        let sink = ChannelSink::start(&j, SinkMode::Answer, 0).await.unwrap();

        let c = reqwest::Client::new();
        let r = c
            .post(format!("{}/api/chat.postMessage", sink.base_url()))
            .header("Authorization", "Bearer xoxb-fixture")
            .json(&json!({ "channel": "room", "text": "f24c-delivery-0001" }))
            .send()
            .await
            .unwrap();
        let v: serde_json::Value = r.json().await.unwrap();

        // The identity in the receipt is the SINK's counter. A sender cannot
        // manufacture this without the round trip actually happening.
        assert_eq!(v["ok"], true);
        assert_eq!(v["ts"], "1.000000");

        let a = sink.arrivals().unwrap();
        assert_eq!(a.len(), 1);
        assert_eq!(a[0].text, "f24c-delivery-0001");
        assert_eq!(a[0].seq, 1);
        assert!(a[0].answered);
        assert!(!a[0].auth_fingerprint.contains("xoxb"));
        sink.shutdown().await;
    }

    #[tokio::test]
    async fn a_stalling_sink_journals_the_arrival_it_never_answers() {
        // This is the measurement that makes the outcome-unknown class
        // reachable from outside the gateway. If the arrival were journalled
        // only on the way out, this record would not exist and the delivery
        // would look lost.
        let dir = tempfile::tempdir().unwrap();
        let j = dir.path().join("arrivals.jsonl");
        let sink = ChannelSink::start(&j, SinkMode::StallAfter(1), 0)
            .await
            .unwrap();
        let base = sink.base_url().to_string();

        let c = reqwest::Client::new();
        c.post(format!("{base}/api/chat.postMessage"))
            .header("Authorization", "Bearer t")
            .json(&json!({ "channel": "room", "text": "first" }))
            .send()
            .await
            .unwrap();

        // The second never answers. Time it out from the client side, which
        // is what a killed gateway looks like to the sink.
        let stalled = c
            .post(format!("{base}/api/chat.postMessage"))
            .header("Authorization", "Bearer t")
            .json(&json!({ "channel": "room", "text": "second" }))
            .timeout(std::time::Duration::from_millis(600))
            .send()
            .await;
        assert!(stalled.is_err(), "the stalled request must not answer");

        let a = read_arrivals(&j).unwrap();
        assert_eq!(a.len(), 2, "the stalled arrival is journalled too");
        assert_eq!(a[1].text, "second");
        assert!(!a[1].answered);
        assert_eq!(ArrivalTally::of(&a).stalled, 1);
    }
}
