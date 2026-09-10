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

/// How many FILLER skills the fixture plants beside the marker one, and how
/// long each description is.
///
/// These are not decoration. Both budget arms under test render a listing that
/// is CLIPPED to a character budget, so a test that compares two budgets can
/// only measure anything if the catalogue overflows the LARGER of them. The
/// budget is 1% of the window in characters (`window x CHARS_PER_TOKEN / 100`),
/// so the largest arm here — the old fabricated 200,000-token window — buys
/// 8,000 characters. `FILLER_SKILLS x FILLER_DESC_LEN` is comfortably over
/// that, and `the_fixture_overflows_every_budget_under_test` asserts it rather
/// than trusting the arithmetic to stay true.
///
/// Before this, the fixture planted ONE skill and the overflow came from
/// whatever skills happened to be installed on the host. On a developer box or
/// hetzner there are plenty and the test passed; in a clean CI container there
/// are none, both arms rendered the identical 4,890 bytes, and the
/// non-vacuity precondition in
/// `an_unknown_window_sizes_the_skill_listing_like_the_window_it_assumes`
/// fired 3/3 — so wayland#1199 c2 and c3 were graded `met` on evidence that was
/// RED in CI. The sibling test above already carries a comment diagnosing
/// exactly this ("that composition differs between a developer box with skills
/// installed and a clean CI container"); the lesson was written down and then
/// not applied one test over. The fixture now supplies its own overflow, so
/// what the host has installed cannot decide whether this file grades anything.
const FILLER_SKILLS: usize = 30;
const FILLER_DESC_LEN: usize = 400;

fn plant_skill(root: &std::path::Path) {
    let skills = root.join(".wayland-core").join("skills");
    let dir = skills.join("issue-1150");
    std::fs::create_dir_all(&dir).expect("skill dir");
    std::fs::write(
        dir.join("SKILL.md"),
        format!("---\nname: issue-1150-skill\ndescription: {SKILL_DESC}\n---\n\nbody\n"),
    )
    .expect("write SKILL.md");

    for i in 0..FILLER_SKILLS {
        let dir = skills.join(format!("issue-1150-filler-{i:03}"));
        std::fs::create_dir_all(&dir).expect("filler skill dir");
        // Unique per skill so nothing can dedupe them, and long enough that the
        // catalogue overflows the largest budget under test.
        let desc = format!("filler {i:03} ")
            + &"describes a distinct capability so the listing cannot dedupe it "
                .repeat(FILLER_DESC_LEN / 63 + 1)[..FILLER_DESC_LEN];
        std::fs::write(
            dir.join("SKILL.md"),
            format!("---\nname: issue-1150-filler-{i:03}\ndescription: {desc}\n---\n\nbody\n"),
        )
        .expect("write filler SKILL.md");
    }
}

/// Boot one session through the production path. Returns everything the user
/// was told, the system prompt the model was given, and the skills listing the
/// session's own resolved window buys.
///
/// WHY THE LISTING IS RENDERED HERE RATHER THAN READ OUT OF THE PROMPT.
/// FerroxLabs/wayland#1283 c1 took the per-skill listing out of the boot
/// prompt: segment 0 now carries fixed Skill discovery instructions and nothing
/// about which skills are installed, so a budget assertion read off
/// `engine.system_prompt()` would be an assertion about a string that no longer
/// varies with the window at all — it would pass on any budget, including the
/// 8,000-character fabrication #1199 exists to keep out.
///
/// What #1199 c2/c3 actually claim is that the session's RESOLVED window
/// reaches the skills budget instead of a hardcoded `None`. That chain is
/// intact and is what this returns: `engine.known_context_window()` is the
/// value the session resolved, `late_mcp.catalog()` is the real catalogue boot
/// discovered, and `format_skills_section` is the renderer the two surviving
/// production listing call sites use (`late_mcp.rs`, and the engine's transient
/// inventory-change block). The hop this no longer covers is boot-prompt
/// assembly, because there is no listing there to cover.
async fn boot(model: &str, context_window: Option<usize>) -> (Vec<String>, String, String) {
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
    let listing = wcore_agent::context::format_skills_section(
        &result.late_mcp.catalog().visible(),
        result.engine.known_context_window(),
    );
    drop(result);
    let infos = notices.infos.lock().unwrap().clone();
    (infos, prompt, listing)
}

/// The body the budget actually bounds, with `format_skills_section`'s
/// `<system-reminder>` wrapper removed.
///
/// `format_skills_within_budget` guarantees `listing_width(entries) <= budget`;
/// the wrapper is added afterwards and is a constant that does not scale with
/// the catalogue, so a ceiling assertion has to measure what the renderer
/// measured rather than the wrapped string.
fn listing_body(listing: &str) -> &str {
    const HEAD: &str =
        "<system-reminder>\nThe following skills are available for use with the Skill tool:\n\n";
    const TAIL: &str = "\n</system-reminder>";
    listing
        .strip_prefix(HEAD)
        .and_then(|s| s.strip_suffix(TAIL))
        .unwrap_or(listing)
}

/// The character budget a window buys, named once so the assertions below
/// compare against the product's own formula instead of a copied constant.
fn budget_for(window: usize) -> usize {
    wcore_skills::prompt::get_char_budget(Some(window))
}

// -- Bug 1: the fabricated window -------------------------------------------

/// The user running an unlisted model must be told the window is a guess, and
/// told what to do about it. Red arm: nothing on `emit_info` at all — the
/// session silently measured a 32k model against 200,000.
#[tokio::test]
async fn an_unknown_context_window_is_announced_where_the_user_is_looking() {
    let (infos, _prompt, _listing) = boot(UNLISTED_MODEL, None).await;

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
    let (infos, _prompt, _listing) = boot("gpt-4o", None).await;
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
    let (infos, _prompt, _listing) = boot(UNLISTED_MODEL, Some(32_768)).await;
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
    let (infos, _prompt, _listing) = boot(UNLISTED_MODEL, Some(6_000)).await;
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
    let (infos, _prompt, _listing) = boot(UNLISTED_MODEL, Some(32_768)).await;
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
/// `AgentBootstrap::build()` session.
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
    let (_infos, roomy_prompt, roomy) = boot(UNLISTED_MODEL, Some(1_000_000)).await;
    let (_infos, tight_prompt, tight) = boot(UNLISTED_MODEL, Some(2_000)).await;

    // FerroxLabs/wayland#1283 c1, asserted here because this is the file that
    // used to read the budget off the boot prompt: the boot prompt itself is
    // now window-INDEPENDENT. Two sessions differing only in the window produce
    // the same segment 0, and neither names a skill. Without this the port
    // below would be a silent downgrade rather than a stated one.
    // Matched on the skill NAMES and the description marker, not on the bare
    // "issue-1150" prefix: that also spells UNLISTED_MODEL, which the intro
    // legitimately states, and the first cut of this assertion fired on the
    // model name.
    for (label, prompt) in [("1_000_000", &roomy_prompt), ("2_000", &tight_prompt)] {
        assert!(
            !prompt.contains("issue-1150-filler-")
                && !prompt.contains("issue-1150-skill")
                && !prompt.contains("ISSUE_1150_DESCRIPTION_MARKER"),
            "the {label}-token boot prompt still names installed skills"
        );
    }
    assert_eq!(
        roomy_prompt.len(),
        tight_prompt.len(),
        "the boot prompt's length still moves with the context window, so \
         something in segment 0 is still sized against it"
    );

    // Precondition: there is a real listing whose size the budget acts on.
    //
    // THIS USED TO SAY "the planted skill reached the listing in BOTH arms",
    // read off the system prompt, and it was VACUOUS on the tight arm: the
    // marker it matched, "issue-1150", is also the first two segments of
    // UNLISTED_MODEL, which the prompt's intro states on every session. An
    // 80-character budget cannot hold a single entry, so the tight arm never
    // contained the planted skill and the precondition passed on the model
    // name. Reading the listing instead of the prompt exposed that, and the
    // honest form of the same claim is asymmetric: the roomy arm HOLDS the
    // skill, the tight arm DECLARES that it dropped it.
    // "issue-1150" is safe as a marker HERE and was not safe in the prompt:
    // the listing cannot contain the model id, the prompt's intro always does.
    assert!(
        roomy.contains("issue-1150"),
        "precondition: the planted skill never reached the 1,000,000-token \
         listing: {roomy}"
    );
    assert!(
        roomy.contains("ISSUE_1150_DESCRIPTION_MARKER"),
        "precondition: a 1,000,000-token window gives a 40,000-char budget, so the planted \
         description must survive in full"
    );
    assert!(
        tight.contains(wcore_skills::prompt::SKILL_OVERFLOW_HINT),
        "precondition: an 80-char budget must render a TRIMMED listing that says \
         so; a listing with nothing to trim would make the comparison below \
         meaningless: {tight}"
    );

    assert!(
        tight.len() < roomy.len(),
        "an 80-char skills budget must render a SHORTER listing than a 40,000-char one; \
         identical lengths mean the session is still handing the renderer `None` and every \
         session is on the flat default (tight = {} bytes, roomy = {} bytes)",
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
    let (_infos, _unknown_prompt, unknown) = boot(UNLISTED_MODEL, None).await;
    let (_infos, _assumed_prompt, assumed) = boot(
        UNLISTED_MODEL,
        Some(wcore_config::compact::UNVERIFIED_CONTEXT_WINDOW),
    )
    .await;
    let (_infos, _fab_prompt, old_fabrication) = boot(
        UNLISTED_MODEL,
        Some(wcore_config::compact::DEFAULT_CONTEXT_WINDOW),
    )
    .await;

    // PRECONDITION. This used to read `unknown.contains("issue-1150")` -- the
    // fixture's own skill must reach the 1,310-character arm -- and that is not
    // a property of the product, it is an accident of how much the HOST
    // installed. `loader.rs` documents the priority order (bundled -> MCP ->
    // user -> project -> additional -> legacy) and the fixture binds its
    // catalogue through `extra_skill_dirs`, which is `additional`: fifth of six.
    // `clamp_to_budget` keeps entries in that order and stops at the budget, so
    // a host carrying more user-tier NAMES than the budget holds pushes the
    // fixture out -- correctly, by the documented model. Measured on hetzner's
    // real catalogue (84 skills, 3,397 characters of names against a 1,310
    // -character budget) the old form fired, while every relation this test
    // actually grades was intact. The proxy was fragile; the property was not.
    //
    // Replaced by the two halves that survive competition and still say
    // something the product could get wrong:
    //
    //   (a) the fixture is genuinely in play, asserted where the fixture has
    //       room to land -- the 8,000-character arm, six times the tight one;
    //   (b) each arm renders INSIDE the budget its own window buys, which is
    //       the derivation claim this file exists for and the one a renderer
    //       that ignores the window breaks immediately.
    //
    // Neither replaces the NON-VACUITY guard: that is the third assertion
    // below, and it is the one that reds when the catalogue renders nothing
    // (FerroxLabs/wayland-core#401 c2).
    assert!(
        old_fabrication.contains("issue-1150"),
        "precondition: the fixture's own skill did not reach even the 8,000 \
         -character arm, so this comparison is grading the host's catalogue \
         rather than the planted one: {old_fabrication}"
    );
    assert!(
        listing_body(&unknown).chars().count()
            <= budget_for(wcore_config::compact::UNVERIFIED_CONTEXT_WINDOW),
        "the unknown-window listing overran the budget its own assumed window \
         buys ({} chars rendered against a {}-char budget)",
        listing_body(&unknown).chars().count(),
        budget_for(wcore_config::compact::UNVERIFIED_CONTEXT_WINDOW),
    );
    assert!(
        listing_body(&old_fabrication).chars().count()
            <= budget_for(wcore_config::compact::DEFAULT_CONTEXT_WINDOW),
        "the 200,000-token listing overran its own budget ({} chars against \
         {})",
        listing_body(&old_fabrication).chars().count(),
        budget_for(wcore_config::compact::DEFAULT_CONTEXT_WINDOW),
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
         (200k = {} bytes, unknown = {} bytes). The fixture plants \
         {FILLER_SKILLS} filler skills precisely so this cannot depend on what \
         the host happens to have installed -- see \
         the_fixture_overflows_every_budget_under_test",
        old_fabrication.len(),
        unknown.len(),
    );
}

/// The guard on the guard: the FIXTURE, not the host, must be what overflows
/// the budgets the tests above compare.
///
/// Both arms of `an_unknown_window_sizes_the_skill_listing_like_the_window_it_assumes`
/// render a listing clipped to `window x CHARS_PER_TOKEN / 100` characters, so
/// the comparison is only meaningful when the catalogue is bigger than the
/// LARGER budget. That used to be supplied by whatever skills were installed on
/// the machine, which is why the file passed on hetzner and failed 3/3 in the
/// clean CI container. This asserts the planted skills alone exceed the largest
/// budget any test in this file uses, so the property is decided here rather
/// than by the host.
#[tokio::test]
async fn the_fixture_overflows_every_budget_under_test() {
    use wcore_config::compact::CHARS_PER_TOKEN;

    // The window whose budget MUST be overflowed for
    // `an_unknown_window_sizes_the_skill_listing_like_the_window_it_assumes`
    // to grade anything: its larger arm, the old fabricated 200,000. The
    // 1,000,000 arm below is deliberately NOT the bound — it exists to show the
    // clipping is monotone, and its 40,000-character budget is meant to hold the
    // whole fixture.
    const HEADROOM_WINDOW: usize = 1_000_000;
    let must_overflow = wcore_config::compact::DEFAULT_CONTEXT_WINDOW * CHARS_PER_TOKEN / 100;

    let planted = FILLER_SKILLS * FILLER_DESC_LEN;
    assert!(
        planted > must_overflow,
        "the fixture plants {planted} characters of skill description and the \
         budget it must overflow is {must_overflow}; if the fixture does not \
         overflow it, the length comparisons in this file measure the host's \
         installed skills instead of the product"
    );

    // And the clipping is real, measured through the production bootstrap
    // rather than asserted from the arithmetic: a 1,000,000-token window must
    // render strictly more than the fabricated 200,000-token one, which must in
    // turn render strictly more than the 32,768 the session assumes.
    let (_i, _huge_prompt, huge) = boot(UNLISTED_MODEL, Some(HEADROOM_WINDOW)).await;
    let (_i, _fab_prompt, fabricated) = boot(
        UNLISTED_MODEL,
        Some(wcore_config::compact::DEFAULT_CONTEXT_WINDOW),
    )
    .await;
    let (_i, _assumed_prompt, assumed) = boot(
        UNLISTED_MODEL,
        Some(wcore_config::compact::UNVERIFIED_CONTEXT_WINDOW),
    )
    .await;
    assert!(
        huge.len() > fabricated.len() && fabricated.len() > assumed.len(),
        "the three budgets must render three strictly decreasing listings, or \
         the catalogue is not overflowing them: 1M = {} bytes, 200k = {} bytes, \
         {} = {} bytes",
        huge.len(),
        fabricated.len(),
        wcore_config::compact::UNVERIFIED_CONTEXT_WINDOW,
        assumed.len()
    );
}

// -- #401: the fixture's independence, graded rather than asserted -----------

/// How many skills stand in for "a host with a populated skills catalogue" in
/// `the_verdict_does_not_move_when_the_host_catalogue_grows`. Distinct names
/// from the fixture's own, so the loader's name dedup cannot collapse them into
/// it and leave the arm measuring nothing.
const HOST_SKILLS: usize = 30;

/// Plant a second catalogue standing in for what a developer box happens to
/// have installed. Deliberately the same SHAPE as `plant_skill`'s filler --
/// non-bundled, unique descriptions, individually large -- because a host
/// catalogue that could not compete for the budget would make the arm vacuous.
fn plant_host_catalogue(root: &std::path::Path) {
    let skills = root.join(".wayland-core").join("skills");
    for i in 0..HOST_SKILLS {
        let dir = skills.join(format!("host-installed-{i:03}"));
        std::fs::create_dir_all(&dir).expect("host skill dir");
        let desc = format!("host {i:03} ")
            + &"an installed capability that competes for the same budget "
                .repeat(FILLER_DESC_LEN / 57 + 1)[..FILLER_DESC_LEN];
        std::fs::write(
            dir.join("SKILL.md"),
            format!("---\nname: host-installed-{i:03}\ndescription: {desc}\n---\n\nbody\n"),
        )
        .expect("write host SKILL.md");
    }
}

/// Boot exactly like `boot`, with one more catalogue bound beside the fixture's
/// own. Returns the skills listing the session's resolved window buys — see
/// `boot` for why that, and not the system prompt.
async fn boot_with_host_catalogue(model: &str, context_window: Option<usize>) -> String {
    let tmp = tempdir().expect("tempdir");
    let root = std::fs::canonicalize(tmp.path()).expect("canonicalize workspace");
    plant_skill(&root);
    let host = tempdir().expect("host tempdir");
    let host_root = std::fs::canonicalize(host.path()).expect("canonicalize host");
    plant_host_catalogue(&host_root);

    let mut cfg = config(model);
    cfg.compact.context_window = context_window;

    let sink: Arc<dyn OutputSink> = Arc::new(NoticeSink::default());
    let result = AgentBootstrap::new(
        cfg,
        root.to_str().expect("utf-8 workspace").to_string(),
        sink,
    )
    .without_channels(true)
    .extra_skill_dirs(vec![root.clone(), host_root.clone()])
    .build()
    .await
    .expect("bootstrap");
    wcore_agent::context::format_skills_section(
        &result.late_mcp.catalog().visible(),
        result.engine.known_context_window(),
    )
}

/// #401 c1, the half that is decidable without two machines: the verdict of
/// `an_unknown_window_sizes_the_skill_listing_like_the_window_it_assumes` must
/// not MOVE when the catalogue grows by a host's worth of installed skills.
///
/// The defect this file was opened for is the mirror image -- the verdict moved
/// when the catalogue SHRANK, because the discriminating power came from
/// whatever `$HOME` happened to hold, so the file passed on hetzner and failed
/// 3/3 in a clean CI container. The fixture fix supplies the overflow itself,
/// and `the_fixture_overflows_every_budget_under_test` grades that from below.
/// This grades it from ABOVE: adding 30 more non-bundled skills of the same
/// size, all competing for the same budget, must leave every relation the test
/// asserts exactly as it found them.
///
/// It is NOT a substitute for running the file on a populated host. It is the
/// property that makes such a run predictable, held permanently and on every
/// platform, rather than re-argued from one machine's `$HOME`.
#[tokio::test]
async fn the_verdict_does_not_move_when_the_host_catalogue_grows() {
    let unknown = boot_with_host_catalogue(UNLISTED_MODEL, None).await;
    let assumed = boot_with_host_catalogue(
        UNLISTED_MODEL,
        Some(wcore_config::compact::UNVERIFIED_CONTEXT_WINDOW),
    )
    .await;
    let old_fabrication = boot_with_host_catalogue(
        UNLISTED_MODEL,
        Some(wcore_config::compact::DEFAULT_CONTEXT_WINDOW),
    )
    .await;

    // PRECONDITION -- same repair as the sibling test, and it matters more
    // here: this arm exists precisely to hold under competition, so a
    // precondition that assumes the fixture outranks whatever the host
    // installed is the one thing it cannot afford. The fixture's tier is
    // `additional`, below `user`, by the documented order in `loader.rs`; on a
    // host with more user-tier names than the tight budget holds it is dropped
    // by design. Asserted where the fixture has room instead, plus the budget
    // ceiling that is the actual claim.
    assert!(
        old_fabrication.contains("issue-1150"),
        "precondition: the fixture's own skill did not reach the 8,000-character \
         arm even there, so this arm is grading the host's catalogue rather than \
         the planted one: {old_fabrication}"
    );
    assert!(
        listing_body(&unknown).chars().count()
            <= budget_for(wcore_config::compact::UNVERIFIED_CONTEXT_WINDOW),
        "with a populated catalogue in play the unknown-window listing overran \
         the budget its assumed window buys ({} chars against {})",
        listing_body(&unknown).chars().count(),
        budget_for(wcore_config::compact::UNVERIFIED_CONTEXT_WINDOW),
    );
    assert_eq!(
        unknown.len(),
        assumed.len(),
        "the unknown-window listing must still be budgeted against {} tokens with a \
         populated catalogue in play (unknown = {} bytes, assumed = {} bytes)",
        wcore_config::compact::UNVERIFIED_CONTEXT_WINDOW,
        unknown.len(),
        assumed.len(),
    );
    assert!(
        old_fabrication.len() > unknown.len(),
        "the 200,000-token arm must still buy a strictly longer listing with a \
         populated catalogue (200k = {} bytes, unknown = {} bytes)",
        old_fabrication.len(),
        unknown.len(),
    );
}

/// #401 c4 -- the ambient-catalogue dependence is a SHAPE, not one site.
///
/// The sibling comment inside `the_bootstrap_prompt_uses_the_real_window_derived_skill_budget`
/// diagnosed this class ("that composition differs between a developer box with
/// skills installed and a clean CI container") and the very next test in the
/// same file was written with the defect anyway. A comment is not a guard, so
/// this is the guard: every `wcore-agent` test that makes a claim about the
/// SIZE or CONTENT of a rendered skills listing must supply the catalogue it
/// grades, instead of taking its discriminating power from `$HOME`.
///
/// Three ways to supply it are recognised, all three of them already in the
/// tree: plant a directory and bind it with `.extra_skill_dirs`, build the
/// catalogue in memory with `SkillCatalog::from_refs`, or drive the prompt
/// through `build_for_test` with an explicit catalogue.
///
/// A STATIC SWEEP IS ONLY WORTH ITS CONTROLS, so the classifier is driven in
/// BOTH directions on synthetic sources before it is pointed at the tree, and
/// the walk asserts a floor on what it actually read -- a scan that silently
/// found no files would otherwise report "no violations" and read as a pass.
#[test]
fn no_agent_test_grades_the_skills_budget_against_the_ambient_catalogue() {
    // A file mentioning any of these makes a claim about the rendered listing's
    // size or content. Deliberately broad: a mention in a doc comment still
    // means the file is reasoning about this budget.
    const BUDGET_MARKERS: [&str; 4] = [
        "get_char_budget",
        "format_skills_section",
        "format_skills_within_budget",
        "SKILL_OVERFLOW_HINT",
    ];
    // ...and any of these means it brought its own catalogue.
    const ISOLATION_MARKERS: [&str; 3] = [
        "extra_skill_dirs",
        "SkillCatalog::from_refs",
        "build_for_test",
    ];

    fn grades_budget(src: &str) -> bool {
        BUDGET_MARKERS.iter().any(|m| src.contains(m))
    }
    fn owns_catalogue(src: &str) -> bool {
        ISOLATION_MARKERS.iter().any(|m| src.contains(m))
    }

    // ── CONTROLS, BEFORE THE SWEEP ──────────────────────────────────────────
    // The violating shape: it grades the budget and never says whose catalogue.
    let ambient = "let budget = get_char_budget(None);\nassert!(block.len() <= budget);";
    assert!(
        grades_budget(ambient) && !owns_catalogue(ambient),
        "the classifier cannot recognise the shape it exists to find"
    );
    // The compliant shape, over the same marker, so the arm above is the
    // ISOLATION and not the budget marker being unmatchable.
    let isolated =
        "let catalog = SkillCatalog::from_refs(vec![]);\nlet budget = get_char_budget(None);";
    assert!(
        grades_budget(isolated) && owns_catalogue(isolated),
        "the classifier does not recognise an in-memory catalogue as isolation"
    );
    // ...and a file that grades nothing here is out of scope entirely, so the
    // sweep cannot be satisfied by flagging everything.
    let unrelated = "assert_eq!(engine.turns(), 1);";
    assert!(
        !grades_budget(unrelated),
        "the classifier claims an unrelated test grades the skills budget"
    );

    // ── THE SWEEP ───────────────────────────────────────────────────────────
    let tests_dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests");
    let mut scanned = 0usize;
    let mut graded: Vec<String> = Vec::new();
    let mut violations: Vec<String> = Vec::new();
    let mut stack = vec![tests_dir.clone()];
    while let Some(dir) = stack.pop() {
        for entry in std::fs::read_dir(&dir).expect("read wcore-agent tests directory") {
            let path = entry.expect("directory entry").path();
            if path.is_dir() {
                stack.push(path);
                continue;
            }
            if path.extension().and_then(|e| e.to_str()) != Some("rs") {
                continue;
            }
            scanned += 1;
            let src = std::fs::read_to_string(&path).expect("read a test source");
            if !grades_budget(&src) {
                continue;
            }
            let rel = path
                .strip_prefix(&tests_dir)
                .unwrap_or(&path)
                .display()
                .to_string();
            if !owns_catalogue(&src) {
                violations.push(rel.clone());
            }
            graded.push(rel);
        }
    }

    // ANTI-VACUITY. A walk that found nothing reports "no violations", which is
    // indistinguishable from a clean tree — the exact failure this file's own
    // defect took two days to notice one level down.
    assert!(
        scanned >= 100,
        "the sweep read only {scanned} sources under {}; it is not walking the \
         tests directory and its silence means nothing",
        tests_dir.display()
    );
    assert!(
        graded.len() >= 4,
        "the sweep classified only {} file(s) as grading the skills budget; at \
         least four do. The markers have stopped matching: {graded:?}",
        graded.len()
    );
    assert!(
        graded
            .iter()
            .any(|f| f == "issue_1150_unknown_context_window_test.rs"),
        "the sweep did not classify THIS file as grading the skills budget, so \
         it is not reading what it thinks it is reading: {graded:?}"
    );
    assert!(
        violations.is_empty(),
        "these wcore-agent tests reason about the skills budget and never supply \
         the catalogue they grade, so what they measure is whatever $HOME holds \
         — a clean container and a developer box give different answers and the \
         green one is the vacuous one: {violations:?}"
    );
}
