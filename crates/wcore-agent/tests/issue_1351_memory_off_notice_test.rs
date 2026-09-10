//! FerroxLabs/wayland#1351 c2 — a session that ends on `NullMemory` must tell
//! its user, on a channel that reaches them with `RUST_LOG` unset.
//!
//! THE DEFECT. When `Memory::open` fails, `bootstrap.rs` substitutes
//! `NullMemory` and carries on. The only announcement was a `tracing::warn!`.
//! With `RUST_LOG` unset — the default for every ordinary user — `wcore-cli`
//! builds its stderr writer as `with_max_level(Level::ERROR)`, so that line
//! goes to a rotating log file and nowhere else. `NullMemory` then accepts
//! every write and returns a fresh id, so nothing downstream looks wrong
//! either: the session remembers nothing, recalls nothing, and says so to
//! no one.
//!
//! WHY A LOG-LEVEL BUMP IS NOT THE FIX, and why this test does not assert on
//! one. `warn!` -> `error!` would put a log record on stderr for a CLI run and
//! still reach neither the TUI (which owns the terminal) nor a JSON-stream
//! host (which reads frames, not stderr). The channel that reaches all three is
//! the session's own `OutputSink` — the same one the retry notices, the #1130
//! capability narrowings and the local-shell notice already use. So this test
//! captures the SINK, which is the user-facing stream, and asserts on what the
//! user was told.
//!
//! HOW THE DEGRADATION IS PLANTED, without breaking anything real: the memory
//! root is stated at the call site via `with_memory_root` (no env write, so no
//! sibling test observes it), and the global store under it is stamped with a
//! schema version from the future. `apply_migrations` fails closed on that by
//! design — `SchemaTooNew` — which is a deterministic `Err` from
//! `Memory::open` on every platform. The route into `NullMemory` is what this
//! criterion is about; #1351 c1's own race is one way in, not the only one.
//!
//! RED ARM. Delete `self.output.emit_info(&memory_unavailable_notice(&e))`
//! from the `Err` arm in `bootstrap.rs` and leave the `tracing::warn!` beside
//! it. That is the defect verbatim, it compiles clean, and it turns
//! `a_session_that_lost_memory_says_so` red while every other memory test
//! stays green.

use std::sync::Arc;

use tempfile::tempdir;
use wcore_agent::bootstrap::AgentBootstrap;
use wcore_agent::output::OutputSink;
use wcore_agent::output::null_sink::NullSink;
use wcore_config::compat::ProviderCompat;
use wcore_config::config::{Config, ProviderType};

/// Keyed off the exact words `memory_unavailable_notice` renders.
const NOTICE_MARK: &str = "long-term memory is OFF for this session";

/// Records what the USER is told. Every other surface delegates to `NullSink`
/// so the test asserts on the notice channel alone. Same shape as
/// `issue_1130_narrowing_notice_test.rs`'s `NoticeSink`.
#[derive(Default)]
struct NoticeSink {
    infos: std::sync::Mutex<Vec<String>>,
}

impl OutputSink for NoticeSink {
    fn emit_text_delta(&self, text: &str, msg_id: &str) {
        NullSink.emit_text_delta(text, msg_id);
    }
    fn emit_thinking(&self, text: &str, msg_id: &str) {
        NullSink.emit_thinking(text, msg_id);
    }
    fn emit_tool_call(&self, name: &str, input: &str) {
        NullSink.emit_tool_call(name, input);
    }
    fn emit_tool_result(&self, name: &str, is_error: bool, content: &str) {
        NullSink.emit_tool_result(name, is_error, content);
    }
    fn emit_stream_start(&self, msg_id: &str) {
        NullSink.emit_stream_start(msg_id);
    }
    fn emit_stream_end(
        &self,
        msg_id: &str,
        turns: usize,
        input: u64,
        output: u64,
        cache_creation: u64,
        cache_read: u64,
        finish: wcore_types::message::FinishReason,
    ) {
        NullSink.emit_stream_end(
            msg_id,
            turns,
            input,
            output,
            cache_creation,
            cache_read,
            finish,
        );
    }
    fn emit_error(
        &self,
        msg: &str,
        retryable: bool,
        category: wcore_protocol::events::FailureCategory,
    ) {
        NullSink.emit_error(msg, retryable, category);
    }
    fn emit_info(&self, msg: &str) {
        self.infos.lock().unwrap().push(msg.to_string());
    }
}

/// The state of any fresh clone. The dead base URL is never dialled —
/// `build()` only constructs.
fn config() -> Config {
    Config {
        provider_label: "openai".into(),
        provider: ProviderType::OpenAI,
        api_key: "sk-test".into(),
        base_url: "http://localhost:0".into(),
        model: "gpt-test-model".into(),
        max_tokens: 1024,
        max_turns: Some(1),
        compat: ProviderCompat::openai_defaults(),
        ..Default::default()
    }
}

/// Stamp the global store under `base` with a schema version this build
/// refuses, so `Memory::open` returns `Err` deterministically.
fn plant_unopenable_store(base: &std::path::Path) {
    let global = wcore_memory::paths::global_db_path_in(Some(base))
        .expect("a stated base always resolves a global db path");
    let tier = wcore_memory::db::TierConn::open(global).expect("plant: first open migrates");
    let conn = tier.conn.lock();
    conn.execute(
        "INSERT OR REPLACE INTO schema_version (version) VALUES (99)",
        [],
    )
    .expect("plant: stamp a version from the future");
}

/// Boot one session through the production path and return everything the user
/// was told on the notice channel.
async fn user_notices(memory_root: &std::path::Path) -> Vec<String> {
    let tmp = tempdir().expect("tempdir");
    let root = std::fs::canonicalize(tmp.path()).expect("canonicalize workspace");
    let notices = Arc::new(NoticeSink::default());
    let sink: Arc<dyn OutputSink> = notices.clone();
    let result = AgentBootstrap::new(
        config(),
        root.to_str().expect("utf-8 workspace").to_string(),
        sink,
    )
    .with_memory_root(memory_root)
    .without_channels(true)
    .build()
    .await
    .expect("bootstrap must still succeed — memory degrades, it does not abort the session");
    drop(result);
    let infos = notices.infos.lock().unwrap().clone();
    infos
}

/// THE #1351 c2 GUARD.
#[tokio::test]
async fn a_session_that_lost_memory_says_so() {
    let mem = tempdir().expect("memory root");
    let base = mem.path();
    plant_unopenable_store(base);

    // Precondition, asserted against the SAME base the bootstrap below uses so
    // the oracle and the session are one experiment: memory really is
    // unopenable here. Without this the test could pass vacuously on a build
    // whose store opened fine and simply announced nothing.
    let opened = wcore_memory::Memory::open_with_config_in(
        Some(base),
        std::path::Path::new("."),
        "precondition",
        &Default::default(),
    )
    .await;
    assert!(
        opened.is_err(),
        "precondition: the planted store opened successfully, so this session would keep \
         real memory and there would be no degradation to announce"
    );

    let infos = user_notices(base).await;
    let hits: Vec<&String> = infos.iter().filter(|m| m.contains(NOTICE_MARK)).collect();
    assert_eq!(
        hits.len(),
        1,
        "the session fell back to NullMemory and told the user nothing on the channel they \
         actually read — #1351 c2. A `tracing::warn!` does not count: with RUST_LOG unset \
         stderr takes ERROR only. Everything the user WAS told: {infos:?}"
    );

    let notice = hits[0];
    assert!(
        notice.contains("could not be opened"),
        "the announcement does not say the store failed to open: {notice}"
    );
    assert!(
        notice.contains("schema"),
        "the announcement drops the store's own cause, so the user cannot tell what is \
         wrong or whether it is fixable: {notice}"
    );
    assert!(
        notice.contains("not deleted"),
        "the announcement does not say existing memories survive, so it reads as data \
         loss: {notice}"
    );
}

/// The other direction, and the reason the assertion above is `== 1` rather
/// than `>= 1`: a session whose memory opened normally must not be told it did
/// not. Without this half, a bootstrap that announced the sentence on every
/// boot would satisfy the guard above while being a new defect — and a louder
/// one, since it would tell every healthy user their memory is off.
#[tokio::test]
async fn a_session_with_working_memory_is_told_nothing() {
    let mem = tempdir().expect("memory root");
    let base = mem.path();

    let opened = wcore_memory::Memory::open_with_config_in(
        Some(base),
        std::path::Path::new("."),
        "precondition",
        &Default::default(),
    )
    .await;
    assert!(
        opened.is_ok(),
        "precondition: an empty memory root must open cleanly, got {:?}",
        opened.err()
    );
    drop(opened);

    let infos = user_notices(base).await;
    let hits: Vec<&String> = infos.iter().filter(|m| m.contains(NOTICE_MARK)).collect();
    assert!(
        hits.is_empty(),
        "memory opened normally yet the user was told it is off: {hits:?}"
    );
}
