//! #1130 — a narrowed capability must reach a channel a person actually reads.
//!
//! `PluginCapabilitySet::narrowed_to_live` clears `browser_suite` /
//! `computer_use` when no backend on this host can start. That fact used to
//! exist ONLY as a `tracing::warn!`: with `RUST_LOG` unset — the default for
//! every ordinary user — `wcore-cli` builds the stderr writer as
//! `with_max_level(Level::ERROR)`, so the single line explaining why a
//! capability vanished went to a rotating log file nobody was reading. From the
//! user's side the feature simply did not work.
//!
//! `capability_liveness_narrowing.rs` already pins that the narrowing is
//! RETURNED and that the returned sentence carries the words a person needs.
//! Neither of those is the ticket's ask. The ask is that bootstrap puts the
//! sentence ON THE SINK, and that step had no test behind it: deleting
//! `self.output.emit_info(&notice)` from `bootstrap.rs` while leaving the
//! `tracing::warn!` beside it reproduces #1130 verbatim and leaves all four of
//! those tests green.
//!
//! So this file drives the PRODUCTION `AgentBootstrap::build()` with a
//! capturing sink and a planted narrowing, and asserts on what the operator was
//! told. It fails if — and only if — the emission is removed.
//!
//! **How the narrowing is planted.** `browser_suite` is flipped on by
//! `from_verified` when a plugin named `wayland-browser` is discovered through
//! `plugin_inventory`. This file `inventory::submit!`s a fixture factory under
//! that name, which registers into THIS test binary only, so the real
//! `wayland-browser` crate does not have to become a dev-dependency of every
//! `wcore-agent` test binary. The liveness half is then forced negative by
//! pointing the probe at a program that cannot exist and a loopback port
//! nothing serves — the same two facts `capability_liveness_narrowing.rs`
//! plants, and for the same reason: on a host running a Camoufox sidecar on
//! the default port (the standing state of the Linux build box) the probe would
//! correctly answer `Ready` and this test would fail against a product telling
//! the truth.

use std::sync::Arc;

use tempfile::tempdir;
use wcore_agent::bootstrap::AgentBootstrap;
use wcore_agent::output::OutputSink;
use wcore_agent::output::null_sink::NullSink;
use wcore_config::compat::ProviderCompat;
use wcore_config::config::{Config, ProviderType};
use wcore_plugin_api::{Plugin, PluginContext, PluginFactory, PluginManifest, PluginResult};

/// The sentence the ticket asks for, keyed off the exact words
/// `CapabilityNarrowing::notice` renders.
const NOTICE_MARK: &str = "capability is not available in this session";

/// A loopback port that is reserved and never served, so the healthcheck arm of
/// the probe is provably dead.
const DEAD_SIDECAR_URL: &str = "http://127.0.0.1:1";

// ── the fixture plugin that flips `browser_suite` on ────────────────────────

static MANIFEST_TOML: &str = r#"
[plugin]
name = "wayland-browser"
version = "0.0.0"
description = "issue 1130 fixture — occupies the wayland-browser name so from_verified flips browser_suite"
entry = "builtin:issue-1130-fixture"
authors = ["t"]
license = "MIT"
[permissions]
"#;

fn fixture_manifest() -> &'static PluginManifest {
    static M: std::sync::OnceLock<PluginManifest> = std::sync::OnceLock::new();
    M.get_or_init(|| PluginManifest::from_toml_str(MANIFEST_TOML).expect("manifest parses"))
}

struct BrowserNameFixture;

#[async_trait::async_trait]
impl Plugin for BrowserNameFixture {
    fn manifest(&self) -> &PluginManifest {
        fixture_manifest()
    }

    /// Registers nothing. The capability flag rides on the plugin's NAME, which
    /// is the whole of what this fixture has to supply.
    async fn initialize(&self, _ctx: &mut PluginContext<'_>) -> PluginResult<()> {
        Ok(())
    }
}

struct BrowserNameFixtureFactory;
impl PluginFactory for BrowserNameFixtureFactory {
    fn name(&self) -> &'static str {
        "wayland-browser"
    }
    fn build(&self) -> Box<dyn Plugin> {
        Box::new(BrowserNameFixture)
    }
}
inventory::submit! { &BrowserNameFixtureFactory as &dyn wcore_plugin_api::PluginFactory }

// ── the world in which no browser backend can start ─────────────────────────

/// Point the probe at a program that cannot exist **and** at a sidecar URL
/// nothing answers. Both have to be planted because the probe mirrors
/// `BrowserSupervisor::ensure_ready`'s two real startup paths.
struct NoBackend {
    prior_bin: Option<std::ffi::OsString>,
    prior_url: Option<std::ffi::OsString>,
}

impl NoBackend {
    fn install() -> Self {
        Self::with_program("wcore-agent-issue-1130-no-such-program")
    }

    /// The other half of the same instrument: plant a program that DOES
    /// resolve, so the probe answers `Ready` and nothing narrows. The planted
    /// program is this test binary's own path — an absolute path `which`
    /// resolves as given on every platform, and one that is never executed
    /// (rule 3 of the probe: it resolves only). Planting it rather than reading
    /// the host's browser state is what keeps
    /// `a_session_that_narrowed_nothing_is_told_nothing` from silently skipping
    /// on any box without Camoufox installed — which is every CI runner.
    fn resolvable() -> Self {
        let exe = std::env::current_exe().expect("test binary has a path");
        Self::with_program(exe.to_str().expect("utf-8 test binary path"))
    }

    fn with_program(program: &str) -> Self {
        let prior_bin = std::env::var_os("WAYLAND_CAMOUFOX_BIN");
        let prior_url = std::env::var_os("WAYLAND_CAMOUFOX_URL");
        unsafe {
            std::env::set_var("WAYLAND_CAMOUFOX_BIN", program);
            std::env::set_var("WAYLAND_CAMOUFOX_URL", DEAD_SIDECAR_URL);
        };
        Self {
            prior_bin,
            prior_url,
        }
    }
}

impl Drop for NoBackend {
    fn drop(&mut self) {
        unsafe {
            match self.prior_bin.take() {
                Some(v) => std::env::set_var("WAYLAND_CAMOUFOX_BIN", v),
                None => std::env::remove_var("WAYLAND_CAMOUFOX_BIN"),
            }
            match self.prior_url.take() {
                Some(v) => std::env::set_var("WAYLAND_CAMOUFOX_URL", v),
                None => std::env::remove_var("WAYLAND_CAMOUFOX_URL"),
            }
        }
    }
}

// ── the capturing sink ──────────────────────────────────────────────────────

/// Records what the OPERATOR is told. Every other surface delegates to
/// `NullSink` so the test asserts on the notice channel alone. Same shape as
/// `local_shell_principal_test.rs`'s `NoticeSink` and `wcore-cli`'s
/// `CapturingOutputSink`.
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
        _category: wcore_protocol::events::FailureCategory,
    ) {
        NullSink.emit_error(
            msg,
            retryable,
            wcore_protocol::events::FailureCategory::Unknown,
        );
    }
    fn emit_info(&self, msg: &str) {
        self.infos.lock().unwrap().push(msg.to_string());
    }
}

/// The state of any fresh clone. The dead base URL is never dialled — `build()`
/// only constructs.
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

/// Boot one session through the production path and return everything the
/// operator was told.
async fn operator_notices() -> Vec<String> {
    let tmp = tempdir().expect("tempdir");
    let root = std::fs::canonicalize(tmp.path()).expect("canonicalize workspace");
    let notices = Arc::new(NoticeSink::default());
    let sink: Arc<dyn OutputSink> = notices.clone();
    let result = AgentBootstrap::new(
        config(),
        root.to_str().expect("utf-8 workspace").to_string(),
        sink,
    )
    .without_channels(true)
    .build()
    .await
    .expect("bootstrap");
    drop(result);
    notices.infos.lock().unwrap().clone()
}

// ── the guard ───────────────────────────────────────────────────────────────

/// THE #1130 guard: a capability the session silently dropped must be announced
/// on the session's own `OutputSink`.
///
/// Red arm: delete `self.output.emit_info(&notice)` from `bootstrap.rs`'s
/// narrowing loop and leave the `tracing::warn!` in place. That is the defect
/// exactly, it compiles without a warning, and it turns this test red while
/// every test in `capability_liveness_narrowing.rs` stays green.
#[serial_test::serial]
#[tokio::test]
async fn a_narrowed_capability_is_announced_where_the_user_is_looking() {
    let _guard = NoBackend::install();

    // Precondition, asserted against the SAME url the guard planted so the
    // oracle and the bootstrap below are one experiment: on this host, in this
    // planted world, the browser probe really does reach a definite negative.
    // Without this the test could pass vacuously on a build whose probe is
    // `Indeterminate` (feature `chromium`/`browserbase`) — there would be no
    // narrowing to announce and no notice to miss.
    let verdict = wcore_browser::liveness::probe(DEAD_SIDECAR_URL).await;
    assert!(
        verdict.should_narrow(),
        "precondition: this build's browser probe returned {verdict:?} rather than a \
         definite Unavailable, so no narrowing would occur and this run would assert \
         nothing about #1130"
    );

    let infos = operator_notices().await;

    let hits: Vec<&String> = infos.iter().filter(|m| m.contains(NOTICE_MARK)).collect();
    assert_eq!(
        hits.len(),
        1,
        "the session dropped `browser_suite` and told the operator nothing on the channel \
         they actually read — #1130. A `tracing::warn!` does not count: with RUST_LOG unset \
         stderr takes ERROR only. Everything the operator WAS told: {infos:?}"
    );

    let notice = hits[0];
    assert!(
        notice.contains("browser_suite"),
        "the announcement does not name the capability that went away: {notice}"
    );
    assert!(
        notice.contains("does not resolve on PATH"),
        "the announcement drops the probe's reason, so the operator cannot tell what is \
         wrong: {notice}"
    );
    assert!(
        notice.contains(&wcore_browser::install::CAMOUFOX.remedy()),
        "the announcement drops the probe's remedy, so the operator cannot tell what to \
         do: {notice}"
    );
}

/// The other direction, and the reason the assertion above is `== 1` rather
/// than `>= 1`: a session whose capability was NOT narrowed must not be told
/// that one was. Without this half, a bootstrap that announced the sentence
/// unconditionally — or on every boot regardless of the probe — would satisfy
/// the guard above while being a new defect.
///
/// The `Ready` verdict is PLANTED (a resolvable program), not read off the
/// host, so this arm asserts on every runner rather than skipping wherever
/// Camoufox happens not to be installed.
#[serial_test::serial]
#[tokio::test]
async fn a_session_that_narrowed_nothing_is_told_nothing() {
    let _guard = NoBackend::resolvable();

    // Same planted world the bootstrap below boots into: one experiment, not
    // two. The dead sidecar URL is still in force, so a `Ready` here can only
    // have come from the resolvable program — the arm this test is about.
    let verdict = wcore_browser::liveness::probe(DEAD_SIDECAR_URL).await;
    assert!(
        !verdict.should_narrow(),
        "precondition: a resolvable sidecar program must keep the capability, got {verdict:?}"
    );

    let infos = operator_notices().await;
    let hits: Vec<&String> = infos.iter().filter(|m| m.contains(NOTICE_MARK)).collect();
    assert!(
        hits.is_empty(),
        "nothing was narrowed ({verdict:?}) yet the operator was told a capability is \
         unavailable: {hits:?}"
    );
}
