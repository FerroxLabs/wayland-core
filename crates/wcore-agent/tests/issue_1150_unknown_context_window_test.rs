//! #1150 — a model whose context window we do not know must not be measured
//! against a fabricated one, and the user must be told.
//!
//! Two production defects, both driven here through the REAL
//! `AgentBootstrap::build()` with a capturing sink:
//!
//! 1. **The fabricated window.** Every kernel call site fed
//!    `CompactConfig::fallback_context_window()` — `context_window` or a flat
//!    200,000 — into `ContextWindow::resolve`, so `resolve` returned
//!    `Some(200_000)` for an unlisted model instead of the `None` its own doc
//!    comment promises. A user on a 32k local model got a pre-flight shed
//!    ceiling of 177,000 and a `% full` gauge computed against a window five
//!    times the real one, and nothing anywhere said the window was a guess.
//!
//! 2. **The dead skills prompt budget.** Both production call sites passed
//!    `None` for `context_window_tokens`, so the real 1%-of-window formula in
//!    `wcore_skills::prompt::get_char_budget` was reachable only from tests
//!    and every real session got the flat `DEFAULT_CHAR_BUDGET` of 8,000.
//!
//! `tracing::warn!` cannot close the first one: with `RUST_LOG` unset only
//! `ERROR` reaches stderr, so the line goes to a file nobody has open (the
//! same trap `issue_1130_narrowing_notice_test.rs` was written for). These
//! tests assert on `OutputSink::emit_info`, the channel the user reads.

use std::sync::Arc;

use tempfile::tempdir;
use wcore_agent::bootstrap::AgentBootstrap;
use wcore_agent::output::OutputSink;
use wcore_agent::output::null_sink::NullSink;
use wcore_config::compat::ProviderCompat;
use wcore_config::config::{Config, ProviderType};

/// A model id no arm of `wcore_config::limits::model_output_ceiling` matches
/// and no Flux tier alias covers — the reporter's case (a local 32k model
/// served over an OpenAI-compatible endpoint).
const UNLISTED_MODEL: &str = "issue-1150-local-32k-unlisted";

/// The phrase the notice is keyed off.
const NOTICE_MARK: &str = "context window";

/// Records what the user is told; every other surface is inert.
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
    fn emit_error(&self, msg: &str, retryable: bool) {
        NullSink.emit_error(msg, retryable);
    }
    fn emit_info(&self, msg: &str) {
        self.infos.lock().unwrap().push(msg.to_string());
    }
}

fn config(model: &str) -> Config {
    Config {
        provider_label: "openai".into(),
        provider: ProviderType::OpenAI,
        api_key: "sk-test".into(),
        base_url: "http://localhost:0".into(),
        model: model.into(),
        max_tokens: 1024,
        max_turns: Some(1),
        compat: ProviderCompat::openai_defaults(),
        ..Default::default()
    }
}

/// One planted project skill, with a description long enough that dropping it
/// is unmistakable in the rendered listing.
const SKILL_DESC: &str = "ISSUE_1150_DESCRIPTION_MARKER a description long enough that a \
tight character budget must drop it entirely rather than merely trim a word";

fn plant_skill(root: &std::path::Path) {
    let dir = root.join(".wayland-core").join("skills").join("issue-1150");
    std::fs::create_dir_all(&dir).expect("skill dir");
    std::fs::write(
        dir.join("SKILL.md"),
        format!("---\nname: issue-1150-skill\ndescription: {SKILL_DESC}\n---\n\nbody\n"),
    )
    .expect("write SKILL.md");
}

/// Boot one session through the production path. Returns everything the user
/// was told plus the system prompt the model was given.
async fn boot(model: &str, context_window: Option<usize>) -> (Vec<String>, String) {
    let tmp = tempdir().expect("tempdir");
    let root = std::fs::canonicalize(tmp.path()).expect("canonicalize workspace");
    plant_skill(&root);

    let mut cfg = config(model);
    cfg.compact.context_window = context_window;

    let notices = Arc::new(NoticeSink::default());
    let sink: Arc<dyn OutputSink> = notices.clone();
    let result = AgentBootstrap::new(
        cfg,
        root.to_str().expect("utf-8 workspace").to_string(),
        sink,
    )
    .without_channels(true)
    .extra_skill_dirs(vec![root.clone()])
    .build()
    .await
    .expect("bootstrap");
    let prompt = result.engine.system_prompt().to_string();
    drop(result);
    let infos = notices.infos.lock().unwrap().clone();
    (infos, prompt)
}

// -- Bug 1: the fabricated window -------------------------------------------

/// The user running an unlisted model must be told the window is a guess, and
/// told what to do about it. Red arm: nothing on `emit_info` at all — the
/// session silently measured a 32k model against 200,000.
#[tokio::test]
async fn an_unknown_context_window_is_announced_where_the_user_is_looking() {
    let (infos, _prompt) = boot(UNLISTED_MODEL, None).await;

    let hits: Vec<&String> = infos
        .iter()
        .filter(|m| m.to_ascii_lowercase().contains(NOTICE_MARK))
        .collect();
    assert_eq!(
        hits.len(),
        1,
        "the session could not size {UNLISTED_MODEL}'s context window and told the user \
         nothing on the channel they read. Everything they WERE told: {infos:?}"
    );
    let notice = hits[0];
    assert!(
        notice.contains(UNLISTED_MODEL),
        "the notice does not name the model whose window is unknown: {notice}"
    );
    assert!(
        notice.contains("context_window"),
        "the notice does not name the setting that fixes it: {notice}"
    );
}

/// The other direction, and the reason the assertion above is `== 1`: a
/// session whose window IS known must not be told it is unknown.
#[tokio::test]
async fn a_known_model_is_not_told_its_window_is_unknown() {
    let (infos, _prompt) = boot("gpt-4o", None).await;
    let hits: Vec<&String> = infos
        .iter()
        .filter(|m| m.to_ascii_lowercase().contains(NOTICE_MARK))
        .collect();
    assert!(
        hits.is_empty(),
        "gpt-4o's 128k window is in the built-in table, yet the user was told it is \
         unknown: {hits:?}"
    );
}

/// An operator who set `[compact] context_window` has answered the question;
/// they must not be nagged about it.
#[tokio::test]
async fn an_explicit_operator_window_silences_the_notice() {
    let (infos, _prompt) = boot(UNLISTED_MODEL, Some(32_768)).await;
    let hits: Vec<&String> = infos
        .iter()
        .filter(|m| m.to_ascii_lowercase().contains(NOTICE_MARK))
        .collect();
    assert!(
        hits.is_empty(),
        "the operator pinned context_window = 32768 and was still told the window is \
         unknown: {hits:?}"
    );
}

// -- #1179 c2: the window is KNOWN, and too small to compact inside ---------
//
// Same channel, same reason, adjacent branch in the same bootstrap `if`. It
// lives in this file because `boot()` is the production path both notices are
// emitted from and duplicating that harness would give the two notices two
// different definitions of "the user was told".

/// The phrase the #1179 c2 notice is keyed off.
const TOO_SMALL_MARK: &str = "too small for automatic compaction";

/// An operator who sets `[compact] context_window` below the window core can
/// compact inside gets a SILENT refusal everywhere else — `should_autocompact_at`
/// simply returns `false`, forever. A silent refusal on a window the operator
/// chose is indistinguishable from compaction being broken, so the session says
/// so once, at boot, on the channel the user reads.
///
/// 6,000 is not a made-up number: #1150's own notice tells operators to set
/// `[compact] context_window`, and a local Ollama `num_ctx` of 6,144 lands in
/// the same band.
#[tokio::test]
async fn a_configured_window_too_small_to_compact_in_is_announced() {
    let (infos, _prompt) = boot(UNLISTED_MODEL, Some(6_000)).await;
    let hits: Vec<&String> = infos
        .iter()
        .filter(|m| m.contains(TOO_SMALL_MARK))
        .collect();
    assert_eq!(
        hits.len(),
        1,
        "compaction is off for the whole session at a 6,000-token window and the user \
         was told nothing. Everything they WERE told: {infos:?}"
    );
    let notice = hits[0];
    assert!(
        notice.contains("6000"),
        "the notice does not name the window it is refusing: {notice}"
    );
    assert!(
        notice.contains("context_window"),
        "the notice does not name the setting that fixes it: {notice}"
    );
    assert!(
        notice.contains("emergency"),
        "the notice must say which boundary DOES still apply, or it reads as \
         'you are now unbounded': {notice}"
    );
}

/// The other direction, and the reason the assertion above is `== 1`: a window
/// compaction CAN work in must not be announced as too small. Without this,
/// the notice would also pass by firing on every session.
#[tokio::test]
async fn a_workable_configured_window_is_not_announced_as_too_small() {
    let (infos, _prompt) = boot(UNLISTED_MODEL, Some(32_768)).await;
    let hits: Vec<&String> = infos
        .iter()
        .filter(|m| m.contains(TOO_SMALL_MARK))
        .collect();
    assert!(
        hits.is_empty(),
        "32,768 is workable - threshold 14,748 against a 3,118-token baseline turn - \
         yet the session announced compaction as off: {hits:?}"
    );
}

// -- Bug 2: the dead skills prompt budget -----------------------------------

/// THE #1150 Bug-2 guard, driven through the production
/// `AgentBootstrap::build()` prompt.
///
/// `get_char_budget` is 1% of the window in characters, so a 2,000-token
/// window buys 80 characters of skill listing and a 1,000,000-token window
/// buys 40,000. Two sessions differing ONLY in that number must therefore
/// produce different prompts. Under the defect the bootstrap call site passed a
/// hardcoded `None`, both sessions got the identical flat 8,000-character
/// default, and the two prompts came out byte-identical.
///
/// Asserting on the DIFFERENCE rather than on a particular truncation keeps
/// this independent of whatever skills the host running the test happens to
/// have installed — the two arms share that ambient catalog exactly.
#[tokio::test]
async fn the_bootstrap_prompt_uses_the_real_window_derived_skill_budget() {
    let (_infos, roomy) = boot(UNLISTED_MODEL, Some(1_000_000)).await;
    let (_infos, tight) = boot(UNLISTED_MODEL, Some(2_000)).await;

    // Precondition: the planted skill reached the listing in BOTH arms, so
    // there is a listing whose size the budget could act on. Without this a
    // bootstrap that rendered no skills section at all would make the
    // comparison below pass by rendering two empty listings.
    for (label, prompt) in [("1_000_000", &roomy), ("2_000", &tight)] {
        assert!(
            prompt.contains("issue-1150"),
            "precondition: the planted skill never reached the {label}-token prompt"
        );
    }
    assert!(
        roomy.contains("ISSUE_1150_DESCRIPTION_MARKER"),
        "precondition: a 1,000,000-token window gives a 40,000-char budget, so the planted \
         description must survive in full"
    );

    assert!(
        tight.len() < roomy.len(),
        "an 80-char skills budget must render a SHORTER listing than a 40,000-char one; \
         identical lengths mean the bootstrap call site is still passing `None` and every \
         session is on the flat 8,000-char default (tight = {} bytes, roomy = {} bytes)",
        tight.len(),
        roomy.len()
    );

    // Deliberately NOT asserted here: that the tight listing dropped the
    // description specifically. `format_skills_within_budget` picks between
    // truncated and name-only degradation from the BUNDLED/non-bundled split of
    // whatever is in the catalogue, and it has a C-5 escape hatch that returns
    // full entries when every skill is bundled. That composition differs between
    // a developer box with skills installed and a clean CI container, and the
    // assertion duly passed on the build host and failed in CI. The length
    // comparison above is the portable form of the same claim, and it is the one
    // the defect actually breaks.
}

/// #1150 D16 — the UNKNOWN-window path, which the guard above never touches.
///
/// `the_bootstrap_prompt_uses_the_real_window_derived_skill_budget` boots BOTH
/// arms with an explicit window (`Some(1_000_000)` / `Some(2_000)`), so the
/// `None` arm — the reporter's exact configuration, an unlisted model with no
/// `[compact] context_window` — is the one case it cannot see. And that is the
/// case where the fabricated 200,000 survived: `get_char_budget`'s `None` arm
/// returned `DEFAULT_CHAR_BUDGET = 8_000`, whose own source comment read
/// "1% of 200k x 4", while every other boundary in the same session was sized
/// against `UNVERIFIED_CONTEXT_WINDOW` = 32,768.
///
/// Asserted as an IDENTITY against the window the session actually assumes, so
/// it cannot be satisfied by any other 1,310-character coincidence, and so it
/// keeps tracking `UNVERIFIED_CONTEXT_WINDOW` if that constant ever moves.
#[tokio::test]
async fn an_unknown_window_sizes_the_skill_listing_like_the_window_it_assumes() {
    let (_infos, unknown) = boot(UNLISTED_MODEL, None).await;
    let (_infos, assumed) = boot(
        UNLISTED_MODEL,
        Some(wcore_config::compact::UNVERIFIED_CONTEXT_WINDOW),
    )
    .await;
    let (_infos, old_fabrication) = boot(
        UNLISTED_MODEL,
        Some(wcore_config::compact::DEFAULT_CONTEXT_WINDOW),
    )
    .await;

    assert!(
        unknown.contains("issue-1150"),
        "precondition: the planted skill never reached the unknown-window prompt"
    );
    assert_eq!(
        unknown.len(),
        assumed.len(),
        "an unknown window must budget the skills listing against the same \
         {} tokens the rest of the session is sized against, not against a \
         200,000-token window nothing else believes in",
        wcore_config::compact::UNVERIFIED_CONTEXT_WINDOW,
    );
    assert!(
        old_fabrication.len() > unknown.len(),
        "precondition: a 200,000-token window really does buy a longer listing \
         here, or this test could pass on a catalogue with no skills in it \
         (200k = {} bytes, unknown = {} bytes)",
        old_fabrication.len(),
        unknown.len(),
    );
}
