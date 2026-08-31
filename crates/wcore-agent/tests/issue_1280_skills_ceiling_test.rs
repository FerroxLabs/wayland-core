//! wayland#1280 c1/c2 — the skills listing has a ceiling, and what the ceiling
//! trims is still reachable.
//!
//! c1: "The skills listing respects a ceiling derived from the resolved context
//! window, with no term that grows without bound in the skill count.
//! Specifically: bundled entries are inside the budget rather than subtracted
//! from it, and the names-only fallback is itself bounded. Graded [...] at 100
//! bundled / 1,000 project skills."
//!
//! c2: "WRONG-REFUSAL CONTROL for c1: a skill trimmed out of the listing is
//! still reachable. The model can discover and invoke it — measured on a
//! session where it actually needs one — or the trimming is refused."
//!
//! Both are measured on the PRODUCTION path. The end-to-end arms drive the real
//! `AgentBootstrap::build()` with an injected recording provider, so the
//! `<system-reminder>` measured is the one the engine hands a provider and the
//! `Skill` dispatches are real engine tool calls against the real catalog. The
//! 100-bundled arm cannot be driven that way — bundled skills are compiled into
//! the binary (`wcore_skills::bundled`, `include`-time entries) and cannot be
//! planted on disk — so it is measured one level down, on
//! `wcore_agent::context::format_skills_section`, which is the function
//! `build_system_prompt` (context.rs) and the late-MCP bind path
//! (late_mcp.rs) both call to render this block. That is the product renderer,
//! not a re-composition of it.
//!
//! Sibling `issue_1150_ordinary_turn_payload_test.rs` carries the tool half of
//! #1150 c5 and the same wrong-refusal shape for tools; this file is the skills
//! half's ceiling. Neither grades a relevance gate: #1280 c3/c4/c5 own that and
//! are superseded to their own issue.

use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use serde_json::json;
use tempfile::tempdir;
use unicode_width::UnicodeWidthStr;
use wcore_agent::bootstrap::AgentBootstrap;
use wcore_agent::output::OutputSink;
use wcore_agent::output::null_sink::NullSink;
use wcore_config::compat::ProviderCompat;
use wcore_config::config::{Config, ProviderType};
use wcore_providers::{LlmProvider, ProviderError};
use wcore_skills::prompt::{SKILL_OVERFLOW_HINT, get_char_budget};
use wcore_skills::refs::SkillRef;
use wcore_skills::types::{LoadedFrom, SkillSource};
use wcore_types::llm::{LlmEvent, LlmRequest};
use wcore_types::message::{ContentBlock, FinishReason, StopReason, TokenUsage};

// ---------------------------------------------------------------------------
// Harness (shape shared with issue_1150_ordinary_turn_payload_test.rs)
// ---------------------------------------------------------------------------

struct RecordingProvider {
    scripts: Mutex<Vec<Vec<LlmEvent>>>,
    requests: Arc<Mutex<Vec<LlmRequest>>>,
}

#[async_trait]
impl LlmProvider for RecordingProvider {
    async fn stream(
        &self,
        request: &LlmRequest,
    ) -> Result<tokio::sync::mpsc::Receiver<LlmEvent>, ProviderError> {
        self.requests.lock().unwrap().push(request.clone());
        let mut scripts = self.scripts.lock().unwrap();
        let events = if scripts.len() > 1 {
            scripts.remove(0)
        } else {
            scripts[0].clone()
        };
        drop(scripts);
        let (tx, rx) = tokio::sync::mpsc::channel(16);
        tokio::spawn(async move {
            for event in events {
                let _ = tx.send(event).await;
            }
        });
        Ok(rx)
    }
}

fn plain_answer() -> Vec<LlmEvent> {
    vec![
        LlmEvent::TextDelta("done".to_string()),
        LlmEvent::Done {
            stop_reason: StopReason::EndTurn,
            finish_reason: FinishReason::Stop,
            usage: TokenUsage::default(),
        },
    ]
}

fn tool_round(id: &str, name: &str, input: serde_json::Value) -> Vec<LlmEvent> {
    vec![
        LlmEvent::ToolUse {
            id: id.to_string(),
            name: name.to_string(),
            input,
            extra: None,
        },
        LlmEvent::Done {
            stop_reason: StopReason::ToolUse,
            finish_reason: FinishReason::from_stop_reason(StopReason::ToolUse),
            usage: TokenUsage::default(),
        },
    ]
}

/// The #1150 reporter's route, which is what makes 1,310 chars the budget: an
/// unlisted local model over an OpenAI-compatible endpoint with no
/// `[compact] context_window`, so the session assumes `UNVERIFIED_CONTEXT_WINDOW`
/// (32,768) and the listing gets 1% of it in characters.
fn config() -> Config {
    let mut cfg = Config {
        provider_label: "openai".into(),
        provider: ProviderType::OpenAI,
        api_key: "sk-test".into(),
        base_url: "http://localhost:0".into(),
        model: "issue-1280-local-32k-unlisted".into(),
        max_tokens: 1024,
        max_turns: Some(8),
        compat: ProviderCompat::openai_defaults(),
        ..Default::default()
    };
    cfg.tools.auto_approve = true;
    cfg.session.enabled = false;
    cfg
}

/// The skill the c2 session actually needs. Its description is deliberately
/// unlike every other planted skill so one query can single it out.
const NEEDLE: &str = "m-skill-777";
const NEEDLE_QUERY: &str = "reticulate splines";
const NEEDLE_BODY: &str = "SPLINE-RETICULATION-BODY-MARKER";

fn plant_skills(root: &std::path::Path, n: usize) {
    let skills = root.join(".wayland-core").join("skills");
    for i in 0..n {
        let name = format!("m-skill-{i:03}");
        let dir = skills.join(&name);
        std::fs::create_dir_all(&dir).expect("skill dir");
        let (desc, body) = if name == NEEDLE {
            (
                "reticulate splines for the quarterly ledger".to_string(),
                NEEDLE_BODY,
            )
        } else {
            (
                format!("skill {i:03} ")
                    + &"does a distinct thing worth describing at some length so the listing is realistic "
                        .repeat(3),
                "body",
            )
        };
        std::fs::write(
            dir.join("SKILL.md"),
            format!("---\nname: {name}\ndescription: {desc}\n---\n\n{body}\n"),
        )
        .expect("write SKILL.md");
    }
}

/// Point user-level skill discovery at an empty directory for the whole test
/// binary.
///
/// Without this the arms below measure the HOST'S installed skills, not the
/// planted ones: on the machine this was written on, `<config>/wayland-core/
/// skills/` holds 85 real skills, they sort ahead of anything planted in a
/// tempdir, and the first run of `a_thousand_project_skills_still_fit_the_
/// window_budget` bounded a listing that named none of its own fixtures. The
/// ceiling held — but a green there would have been a green about someone
/// else's skills. Every test in this file is `#[serial]` because this is
/// process-global state.
fn isolate_user_skill_dirs() -> &'static std::path::Path {
    static ISOLATED: std::sync::OnceLock<tempfile::TempDir> = std::sync::OnceLock::new();
    let dir = ISOLATED.get_or_init(|| tempdir().expect("isolated home"));
    // SAFETY: every test in this binary is #[serial], and these three are set
    // once to a path that outlives the process's use of them.
    unsafe {
        std::env::set_var("HOME", dir.path());
        std::env::set_var("XDG_CONFIG_HOME", dir.path().join("config"));
        std::env::set_var("WAYLAND_HOME", dir.path().join("wayland-home"));
    }
    dir.path()
}

async fn session(
    skill_count: usize,
    scripts: Vec<Vec<LlmEvent>>,
    prompts: &[&str],
) -> Vec<LlmRequest> {
    isolate_user_skill_dirs();
    let tmp = tempdir().expect("tempdir");
    let root = std::fs::canonicalize(tmp.path()).expect("canonicalize");
    plant_skills(&root, skill_count);

    let requests: Arc<Mutex<Vec<LlmRequest>>> = Arc::new(Mutex::new(Vec::new()));
    let provider = Arc::new(RecordingProvider {
        scripts: Mutex::new(scripts),
        requests: requests.clone(),
    });
    let sink: Arc<dyn OutputSink> = Arc::new(NullSink);
    let mut result = AgentBootstrap::new(config(), root.to_str().expect("utf-8").to_string(), sink)
        .without_channels(true)
        .extra_skill_dirs(vec![root.clone()])
        .provider(provider)
        .build()
        .await
        .expect("bootstrap");

    for (i, p) in prompts.iter().enumerate() {
        result.engine.run(p, &format!("m{i}")).await.expect("turn");
    }
    drop(result);
    let reqs = requests.lock().unwrap().clone();
    assert!(!reqs.is_empty(), "the engine dispatched nothing");
    reqs
}

const HEADER: &str = "The following skills are available for use with the Skill tool:";

/// The `<system-reminder>` skills block a turn carried.
fn skills_block(system: &str) -> Option<&str> {
    let start = system.find(HEADER)?;
    let head = system[..start].rfind("<system-reminder>")?;
    let end = system[start..].find("</system-reminder>")? + start + "</system-reminder>".len();
    Some(&system[head..end])
}

/// The listing itself — the part whose size is a function of the skill count.
/// The wrapper around it is a fixed string and is not what c1 bounds.
fn listing_of(block: &str) -> &str {
    let start = block.find(HEADER).expect("the block carries its header") + HEADER.len();
    let end = block.find("</system-reminder>").expect("closing tag");
    block[start..end].trim()
}

fn width(s: &str) -> usize {
    UnicodeWidthStr::width(s)
}

// ---------------------------------------------------------------------------
// c1 — the ceiling, end to end
// ---------------------------------------------------------------------------

/// 1,000 project skills, on the real bootstrap path.
///
/// Before the fix this rendered 19,999 chars against the 1,310-char budget the
/// session's own resolved window implies — 15.3x — because level 3 emitted
/// every non-bundled NAME with nothing capping the total.
#[tokio::test]
#[serial_test::serial]
async fn a_thousand_project_skills_still_fit_the_window_budget() {
    let budget = get_char_budget(None);

    let reqs = session(1_000, vec![plain_answer()], &["What is 2 + 2?"]).await;
    let block = skills_block(&reqs[0].system).expect("a skills listing was rendered");
    let listing = listing_of(block);

    // NON-VACUITY: the arm has to be over budget BEFORE the ceiling, or a pass
    // here means only that 1,000 skills happen to be small. One name-only entry
    // per planted skill is the cheapest shape the old code could have emitted,
    // and even that is far over.
    let cheapest_unbounded = 1_000 * "- m-skill-000".len() + 999;
    assert!(
        cheapest_unbounded > budget * 10,
        "this arm does not exercise the defect: the cheapest unbounded rendering \
         is {cheapest_unbounded} chars against a {budget}-char budget"
    );

    assert!(
        width(listing) <= budget,
        "the skills listing is {} columns against the {budget}-column budget the \
         session's own resolved context window implies",
        width(listing)
    );

    // ...and it is not bounded by being empty. Something real is listed, and
    // it is one of THIS test's fixtures — see `isolate_user_skill_dirs`.
    assert!(
        listing.contains("- m-skill-"),
        "the listing named no planted skill at all, so the bound above measures \
         nothing: {listing}"
    );

    // WRONG-REFUSAL CONTROL, prompt half: what was trimmed is counted and the
    // route back to it is named in the listing itself.
    assert!(
        listing.contains(SKILL_OVERFLOW_HINT),
        "990-odd skills were dropped from the listing with no statement that \
         they exist or how to reach them: {listing}"
    );
}

/// The CONTROL for the test above: a session whose skills fit is not trimmed.
///
/// A ceiling that always fires is indistinguishable from a listing that always
/// says "search for it", and would be its own wrong refusal.
#[tokio::test]
#[serial_test::serial]
async fn a_small_skill_set_is_listed_in_full_and_not_trimmed() {
    let reqs = session(6, vec![plain_answer()], &["What is 2 + 2?"]).await;
    let listing = listing_of(skills_block(&reqs[0].system).expect("a skills listing"));

    for i in 0..6 {
        assert!(
            listing.contains(&format!("m-skill-{i:03}")),
            "skill {i} is missing from a listing that had room for it: {listing}"
        );
    }
    assert!(
        !listing.contains(SKILL_OVERFLOW_HINT),
        "a listing that fits its budget was trimmed anyway: {listing}"
    );
}

// ---------------------------------------------------------------------------
// c1 — the 100-bundled / 1,000-project arm
// ---------------------------------------------------------------------------

fn skill_ref(name: &str, description: &str, source: SkillSource) -> SkillRef {
    SkillRef {
        name: name.to_string(),
        display_name: None,
        description: description.to_string(),
        when_to_use: None,
        paths: vec![],
        source,
        loaded_from: match source {
            SkillSource::Bundled => LoadedFrom::Bundled,
            _ => LoadedFrom::Skills,
        },
        file_path: std::path::PathBuf::from(format!("/tmp/{name}/SKILL.md")),
        skill_root: None,
        content_length_hint: 0,
        user_invocable: true,
        disable_model_invocation: false,
        has_artifacts: false,
        inline_content: None,
    }
}

/// The arm c1 names: 100 bundled + 1,000 project skills.
///
/// This is the case the old code was worst on, and for a reason worth stating:
/// `remaining_budget = budget.saturating_sub(bundled_chars)` SUBTRACTED the
/// bundled block from the budget instead of charging it against it, so the
/// bundled entries were emitted at full description no matter how many there
/// were. 100 bundled skills alone measured 22,399 chars against 1,310 — 17.1x.
#[test]
fn a_hundred_bundled_and_a_thousand_project_skills_fit_the_budget() {
    let budget = get_char_budget(None);

    let mut skills: Vec<SkillRef> = (0..100)
        .map(|i| {
            skill_ref(
                &format!("bundled-{i:03}"),
                &("a bundled skill with a description of realistic length ".repeat(4)),
                SkillSource::Bundled,
            )
        })
        .collect();
    skills.extend((0..1_000).map(|i| {
        skill_ref(
            &format!("proj-{i:04}"),
            &("a project skill with a description of realistic length ".repeat(4)),
            SkillSource::User,
        )
    }));

    // NON-VACUITY: the bundled block ALONE is the term the old formula exempted.
    // If it is not already far over budget, this arm is not the one c1 names.
    let bundled_alone: usize = skills
        .iter()
        .filter(|s| s.source == SkillSource::Bundled)
        .map(|s| width(&wcore_skills::prompt::format_skill_entry(s)) + 1)
        .sum();
    assert!(
        bundled_alone > budget * 10,
        "the bundled block is only {bundled_alone} chars against a {budget}-char \
         budget, so this arm does not exercise the subtraction defect"
    );

    // The PRODUCT renderer — the one build_system_prompt and the late-MCP bind
    // path both call.
    let block = wcore_agent::context::format_skills_section(&skills, None);
    let listing = listing_of(&block);

    assert!(
        width(listing) <= budget,
        "100 bundled + 1,000 project skills render {} columns against the \
         {budget}-column budget",
        width(listing)
    );
    assert!(
        listing.contains(SKILL_OVERFLOW_HINT),
        "1,100 skills were cut to {} columns with no route back to them",
        width(listing)
    );
    assert!(
        listing.contains("- bundled-000") || listing.contains("- proj-0000"),
        "the listing named nothing at all: {listing}"
    );
}

/// The other two arms from the issue's table, on the same renderer, so the
/// ceiling is shown to hold across the skill counts it was measured failing at.
#[test]
fn the_measured_arms_from_the_issue_all_fit_the_budget() {
    let budget = get_char_budget(None);
    for (bundled, project) in [
        (0usize, 300usize),
        (0, 1_000),
        (40, 10),
        (100, 10),
        (40, 300),
    ] {
        let mut skills: Vec<SkillRef> = (0..bundled)
            .map(|i| {
                skill_ref(
                    &format!("bundled-{i:03}"),
                    &("a bundled skill description of realistic length ".repeat(4)),
                    SkillSource::Bundled,
                )
            })
            .collect();
        skills.extend((0..project).map(|i| {
            skill_ref(
                &format!("proj-{i:04}"),
                &("a project skill description of realistic length ".repeat(4)),
                SkillSource::User,
            )
        }));
        let block = wcore_agent::context::format_skills_section(&skills, None);
        let listing = listing_of(&block);
        assert!(
            width(listing) <= budget,
            "{bundled} bundled / {project} project renders {} columns against a \
             {budget}-column budget",
            width(listing)
        );
    }
}

/// The ceiling tracks the RESOLVED window, not a constant. A larger window buys
/// a larger listing; that is what "derived from the resolved context window"
/// means and it is the half a hard-coded cap would silently fail.
#[test]
fn the_ceiling_is_derived_from_the_window_not_fixed() {
    let skills: Vec<SkillRef> = (0..1_000)
        .map(|i| {
            skill_ref(
                &format!("proj-{i:04}"),
                &("a project skill description of realistic length ".repeat(4)),
                SkillSource::User,
            )
        })
        .collect();

    let small = listing_of(&wcore_agent::context::format_skills_section(&skills, None)).to_string();
    let large = listing_of(&wcore_agent::context::format_skills_section(
        &skills,
        Some(1_000_000),
    ))
    .to_string();

    assert!(width(&small) <= get_char_budget(None));
    assert!(width(&large) <= get_char_budget(Some(1_000_000)));
    assert!(
        width(&large) > width(&small) * 4,
        "a 1,000,000-token window bought a listing of {} columns against the \
         32,768-token window's {} — the ceiling is not tracking the window",
        width(&large),
        width(&small)
    );
}

// ---------------------------------------------------------------------------
// c2 — WRONG-REFUSAL CONTROL: a trimmed skill is still reachable
// ---------------------------------------------------------------------------

/// Everything text-shaped the model was handed on dispatch `i`.
fn tool_results(req: &LlmRequest) -> Vec<(String, bool)> {
    req.messages
        .iter()
        .flat_map(|m| m.content.iter())
        .filter_map(|b| match b {
            ContentBlock::ToolResult {
                content, is_error, ..
            } => Some((content.clone(), *is_error)),
            _ => None,
        })
        .collect()
}

/// The control that decides whether c1's ceiling is a win or a regression.
///
/// A session with 1,000 skills installed, on a turn that genuinely needs one
/// the ceiling trimmed away. The model finds it and runs it, through real
/// engine dispatches: ToolSearch hydrates the `Skill` tool (it is not on the
/// hot allowlist, so it is folded out of `tools[]` like any other cold tool),
/// `Skill { query }` searches every installed skill and names the one that
/// matches, and `Skill { skill }` executes it. Nothing here reads the listing;
/// that is the point.
#[tokio::test]
#[serial_test::serial]
async fn a_trimmed_skill_is_found_and_run_on_a_turn_that_needs_it() {
    let reqs = session(
        1_000,
        vec![
            tool_round("c1", "ToolSearch", json!({ "query": "Skill" })),
            tool_round("c2", "Skill", json!({ "query": NEEDLE_QUERY })),
            tool_round("c3", "Skill", json!({ "skill": NEEDLE })),
            plain_answer(),
        ],
        &["Reticulate the splines on the quarterly ledger."],
    )
    .await;

    assert!(
        reqs.len() >= 4,
        "the session reached only {} dispatches, so the discover-then-invoke \
         chain was not measured",
        reqs.len()
    );

    // PRECONDITION: the skill really is absent from the listing, or this test
    // proves nothing about reachability after trimming.
    let listing = listing_of(skills_block(&reqs[0].system).expect("a skills listing"));
    assert!(
        !listing.contains(NEEDLE),
        "{NEEDLE} was in the listing all along, so nothing about a TRIMMED \
         skill's reachability was measured"
    );
    assert!(
        listing.contains(SKILL_OVERFLOW_HINT),
        "the listing did not even declare that skills were withheld: {listing}"
    );

    // The Skill tool is cold; the model has to ask for it first. That is the
    // same explicit-activation path the tool half of #1150 c5 grades.
    assert!(
        !reqs[0].tools.iter().any(|t| t.name == "Skill"),
        "Skill was already hot, so this session does not exercise activation"
    );
    assert!(
        reqs[1].tools.iter().any(|t| t.name == "Skill"),
        "the model asked ToolSearch for Skill and the next dispatch still does \
         not carry it. Shipped: {:?}",
        reqs[1].tools.iter().map(|t| &t.name).collect::<Vec<_>>()
    );

    // DISCOVERY: the search names the trimmed skill.
    let search = tool_results(&reqs[2]);
    let (search_body, search_err) = search
        .last()
        .cloned()
        .expect("dispatch 2 carries the Skill search result");
    assert!(
        !search_err,
        "the skill search came back as an error: {search_body}"
    );
    assert!(
        search_body.contains(NEEDLE),
        "searching 1,000 installed skills for {NEEDLE_QUERY:?} did not name \
         {NEEDLE}, so a trimmed skill is undiscoverable: {search_body}"
    );
    // ...and the search is itself bounded — the escape hatch must not undo the
    // ceiling by pouring the whole catalogue into the message stream.
    assert!(
        search_body.lines().count() <= wcore_skills::prompt::SKILL_SEARCH_MAX_RESULTS * 2 + 4,
        "the skill search returned {} lines; it is supposed to be bounded at \
         {} hits",
        search_body.lines().count(),
        wcore_skills::prompt::SKILL_SEARCH_MAX_RESULTS
    );

    // INVOCATION: and it actually runs.
    let (run_body, run_err) = tool_results(&reqs[3])
        .last()
        .cloned()
        .expect("dispatch 3 carries the Skill invocation result");
    assert!(
        !run_err,
        "the model found {NEEDLE} and invoking it failed: {run_body}"
    );
    assert!(
        run_body.contains(NEEDLE_BODY),
        "invoking {NEEDLE} did not return its body, so the skill the ceiling \
         trimmed is listed-out AND unusable: {run_body}"
    );
}

/// The miss path is bounded too.
///
/// `available_names()` used to join EVERY visible name into the not-found
/// message, which is the same unbounded-in-the-skill-count term the listing
/// carried, relocated to the message stream — one typo on a 1,000-skill machine
/// answered with 1,000 names.
#[tokio::test]
#[serial_test::serial]
async fn a_mistyped_skill_name_does_not_dump_the_whole_catalogue() {
    let reqs = session(
        1_000,
        vec![
            tool_round("c1", "ToolSearch", json!({ "query": "Skill" })),
            tool_round("c2", "Skill", json!({ "skill": "m-skill-does-not-exist" })),
            plain_answer(),
        ],
        &["Run the skill that does not exist."],
    )
    .await;

    let (body, is_error) = tool_results(&reqs[2])
        .last()
        .cloned()
        .expect("dispatch 2 carries the not-found result");
    assert!(is_error, "a missing skill should be an error: {body}");

    let named = (0..1_000)
        .filter(|i| body.contains(&format!("m-skill-{i:03}")))
        .count();
    assert!(
        named <= wcore_skills::prompt::SKILL_SEARCH_MAX_RESULTS,
        "the not-found message named {named} of the 1,000 installed skills"
    );
    // ...and it still points somewhere useful.
    assert!(
        body.contains("query"),
        "the not-found message named no way to find the right skill: {body}"
    );
}
