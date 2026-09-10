//! FerroxLabs/wayland#1283 — skills reach the model when they are asked for,
//! not on every ordinary turn.
//!
//! c1: "Skills are injected only when relevant or explicitly activated [...] on
//! a turn whose text relates to none of them, measured on the real bootstrap
//! path."
//! c2: "WRONG-REFUSAL CONTROL for c1: on a turn that DOES need a gated-out
//! skill, the model can still find and run it -- withholding a skill the model
//! then cannot use is worse than a larger prompt."
//! c3: "Whatever c1 does, the prefix ahead of the conversation does not churn
//! per turn on an implicit-cache endpoint, measured off the real `LlmRequest`
//! across a multi-turn session."
//!
//! EVERYTHING HERE IS DRIVEN THROUGH THE REAL `AgentBootstrap::build()` with an
//! injected recording provider, and `Config::system_prompt` is never set. That
//! is the whole reason this file exists rather than an extension of
//! `issue_1150_implicit_prefix_cache_test.rs`: that fixture DOES set
//! `Config::system_prompt`, the engine takes it verbatim, and
//! `context::build_system_prompt` — the only place the skills section is
//! assembled — never runs on its path. Measured on lane/f13-w3-cache-spend, it
//! passed with an unconditional per-call skills listing carrying a turn counter
//! spliced in. A green there means blind, not stable.
//!
//! THIS FILE ASSERTS ON LITERAL PROMPT TEXT, NOT ON
//! `wcore_agent::context::SKILL_DISCOVERY_SECTION`, DELIBERATELY. Referencing
//! the constant would make the file uncompilable against the pre-fix tree, and
//! a criterion whose oracle cannot be run against the defect it names has no
//! fail-before arm. Every marker below is a substring of the shipped constant;
//! if that constant is reworded, these go red and the rewording gets read.

use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use serde_json::json;
use tempfile::tempdir;
use wcore_agent::bootstrap::AgentBootstrap;
use wcore_agent::output::OutputSink;
use wcore_agent::output::null_sink::NullSink;
use wcore_config::compat::ProviderCompat;
use wcore_config::config::{Config, ProviderType};
use wcore_providers::{LlmProvider, ProviderError};
use wcore_types::llm::{LlmEvent, LlmRequest};
use wcore_types::message::{ContentBlock, FinishReason, StopReason, TokenUsage};

// ---------------------------------------------------------------------------
// Harness (shape shared with issue_1150_ordinary_turn_payload_test.rs and
// issue_1280_skills_ceiling_test.rs)
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
        LlmEvent::TextDelta("4".to_string()),
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

/// The #1150 reporter's route: an unlisted local model over an
/// OpenAI-compatible endpoint with no `[compact] context_window`, so the
/// session assumes `UNVERIFIED_CONTEXT_WINDOW` (32,768) and the pre-#1283
/// listing budget is 1% of it — 1,310 characters, the figure both sibling
/// files are sized against.
fn config() -> Config {
    let mut cfg = Config {
        provider_label: "openai".into(),
        provider: ProviderType::OpenAI,
        api_key: "sk-test".into(),
        base_url: "http://localhost:0".into(),
        model: "issue-1283-local-32k-unlisted".into(),
        max_tokens: 1024,
        max_turns: Some(8),
        compat: ProviderCompat::openai_defaults(),
        ..Default::default()
    };
    cfg.tools.auto_approve = true;
    cfg.session.enabled = false;
    cfg
}

/// The skill the c2 session actually needs. Deliberately unlike every other
/// planted skill so one query singles it out.
const NEEDLE: &str = "m-skill-777";
const NEEDLE_QUERY: &str = "reticulate splines";
const NEEDLE_BODY: &str = "SPLINE-RETICULATION-BODY-MARKER";

/// Plant `n` project skills of a realistic shape. `prefix` lets two boots plant
/// genuinely DIFFERENT catalogues, which is what the cross-boot cache arm
/// needs: a prefix that varies is a listing that varies.
fn plant_skills(root: &std::path::Path, n: usize, prefix: &str) {
    let skills = root.join(".wayland-core").join("skills");
    for i in 0..n {
        let name = format!("{prefix}-{i:03}");
        let dir = skills.join(&name);
        std::fs::create_dir_all(&dir).expect("skill dir");
        let (desc, body) = if name == NEEDLE {
            (
                "Reticulates splines on a ledger, and nothing else does.".to_string(),
                NEEDLE_BODY.to_string(),
            )
        } else {
            (
                format!("skill {i:03} ")
                    + &"does a distinct thing worth describing at some length so the \
                        listing is realistic "
                        .repeat(3),
                "body".to_string(),
            )
        };
        std::fs::write(
            dir.join("SKILL.md"),
            format!("---\nname: {name}\ndescription: {desc}\n---\n\n{body}\n"),
        )
        .expect("write SKILL.md");
    }
}

/// Point user-level skill discovery at an empty directory.
///
/// The assertions below are about the skills THIS file plants. On a machine
/// with skills actually installed (85 of them on the host this was fixed on)
/// the host's own skills are discovered too and would be indistinguishable from
/// a leak of the fixtures. Every test in this binary is `#[serial]` because
/// this is process-global state.
fn isolate_user_skill_dirs() {
    static ISOLATED: std::sync::OnceLock<tempfile::TempDir> = std::sync::OnceLock::new();
    let dir = ISOLATED.get_or_init(|| tempdir().expect("isolated home"));
    // SAFETY: every test in this binary is #[serial], and these three are set
    // once to a path that outlives the process's use of them.
    unsafe {
        std::env::set_var("HOME", dir.path());
        std::env::set_var("XDG_CONFIG_HOME", dir.path().join("config"));
        std::env::set_var("WAYLAND_HOME", dir.path().join("wayland-home"));
    }
}

/// Drive `prompts` user turns on ONE real engine and return every `LlmRequest`
/// the provider was handed.
async fn session(
    skill_count: usize,
    prefix: &str,
    scripts: Vec<Vec<LlmEvent>>,
    prompts: &[&str],
) -> Vec<LlmRequest> {
    isolate_user_skill_dirs();
    let tmp = tempdir().expect("tempdir");
    let root = std::fs::canonicalize(tmp.path()).expect("canonicalize");
    plant_skills(&root, skill_count, prefix);

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

/// The `<system-reminder>` block that carries the skills contract, whatever
/// that contract currently is.
///
/// `mark` is matched inside the block, and the block is the innermost
/// `<system-reminder>...</system-reminder>` span containing it. Both the
/// pre-#1283 listing header and the post-#1283 discovery sentence are
/// findable this way, so the same helper reads both trees.
fn reminder_containing<'a>(system: &'a str, mark: &str) -> Option<&'a str> {
    let at = system.find(mark)?;
    let head = system[..at].rfind("<system-reminder>")?;
    let end = system[at..].find("</system-reminder>")? + at + "</system-reminder>".len();
    Some(&system[head..end])
}

/// Every `(body, is_error)` tool result carried by a request's message stream.
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

/// Markers of the discovery block. Substrings of the shipped constant, chosen
/// so each one names a thing the model has to know to reach a skill at all.
const MARK_ROUTE: &str = "call the `Skill` tool with";
const MARK_QUERY: &str = "\"query\"";
const MARK_INVOKE: &str = "\"skill\"";
const MARK_HYDRATE: &str = "`ToolSearch`";

// ---------------------------------------------------------------------------
// c1 — an ordinary turn lists no skill
// ---------------------------------------------------------------------------

/// A turn about arithmetic, on a real boot with ten skills installed, none of
/// which has anything to do with arithmetic.
///
/// NON-VACUITY is the hard part here, because "no skill name in the prompt" is
/// also what a session with NO SKILLS AT ALL looks like — the failure mode that
/// would grade this criterion green while measuring nothing. Three separate
/// facts are asserted in this body to close it: the discovery block is present
/// (it is gated on at least one model-invocable skill being discovered, so its
/// presence proves the catalogue loaded); the same fixture's catalogue answers
/// a live `Skill { query }` with the planted names (proving they are installed
/// and reachable, not missing); and the arithmetic turn's prompt names none of
/// them.
#[tokio::test]
#[serial_test::serial]
async fn an_ordinary_turn_names_no_installed_skill() {
    let reqs = session(
        10,
        "m-skill",
        vec![plain_answer()],
        &["What is 2 + 2?", "Name a colour."],
    )
    .await;

    let named: Vec<String> = (0..10)
        .map(|i| format!("m-skill-{i:03}"))
        .filter(|n| reqs[0].system.contains(n.as_str()))
        .collect();
    assert!(
        named.is_empty(),
        "{} of the 10 installed skills are named in the system prompt of a \
         turn about arithmetic; none of them is relevant to it and none was \
         activated. Named: {named:?}",
        named.len()
    );

    // NON-VACUITY 1: the section is there at all, so the catalogue was found.
    // With no skill discovered, `build_system_prompt` emits no skills section
    // whatsoever and this marker is absent.
    let block = reminder_containing(&reqs[0].system, MARK_ROUTE).unwrap_or_else(|| {
        panic!(
            "no skill-discovery block in the system prompt. Either the \
             catalogue did not load — in which case the assertion above \
             measured nothing — or the model has been left with skills \
             installed and no way to reach them. System prompt:\n{}",
            reqs[0].system
        )
    });
    assert!(
        block.contains(MARK_QUERY) && block.contains(MARK_INVOKE),
        "the discovery block does not name both the search and the invoke \
         parameter: {block}"
    );

    // The listing is gone on the LAST turn too, not just the first — the point
    // is unconditional injection, and a prompt built once is shipped every
    // turn.
    let last = reqs.last().expect("at least one dispatch");
    assert!(
        !last.system.contains("m-skill-"),
        "a later turn's prompt names installed skills"
    );
}

/// NON-VACUITY 2, as its own test so a failure names the right thing: the
/// skills this fixture plants really are installed and really are reachable.
///
/// Without this, `an_ordinary_turn_names_no_installed_skill` cannot tell "the
/// gate works" from "`plant_skills` wrote nothing the loader accepted".
#[tokio::test]
#[serial_test::serial]
async fn the_skills_the_gate_withholds_are_genuinely_installed() {
    let reqs = session(
        10,
        "m-skill",
        vec![
            tool_round("c1", "ToolSearch", json!({ "query": "Skill" })),
            tool_round("c2", "Skill", json!({ "query": "distinct thing" })),
            plain_answer(),
        ],
        &["Find me something."],
    )
    .await;

    assert!(
        reqs.len() >= 3,
        "the session reached only {} dispatches",
        reqs.len()
    );
    let (body, is_error) = tool_results(&reqs[2])
        .last()
        .cloned()
        .expect("dispatch 2 carries the Skill search result");
    assert!(!is_error, "the skill search came back as an error: {body}");
    let found = (0..10)
        .filter(|i| body.contains(&format!("m-skill-{i:03}")))
        .count();
    assert!(
        found > 0,
        "a search of the installed catalogue named none of the 10 planted \
         skills, so the fixture never installed them and every 'not in the \
         prompt' assertion in this file is vacuous: {body}"
    );
}

// ---------------------------------------------------------------------------
// c2 — WRONG-REFUSAL CONTROL
// ---------------------------------------------------------------------------

/// The gate withholds every skill from the prompt. This is the control that
/// decides whether that is a win or a wrong refusal, measured on a session that
/// genuinely needs one of the withheld skills.
///
/// It grades TWO things, because the mechanical chain alone is not the control.
/// A scripted provider will run `Skill { query }` whether or not the prompt
/// ever told a real model that route exists, so a test that only drives the
/// chain would stay green with the discovery instructions deleted — and a
/// prompt that withholds every name AND says nothing about how to search is
/// exactly the wrong refusal this criterion forbids. So:
///
/// 1. THE ROUTE IS STATED. The prompt names the `Skill` tool, its `query`
///    parameter, its `skill` parameter, and `ToolSearch` — the hop that is
///    needed because `Skill` is not on `defer_cold`'s hot allowlist and is
///    therefore absent from `tools[]` on a default session.
/// 2. THE ROUTE IS EXECUTABLE. `ToolSearch` really does carry `Skill` in its
///    catalog, the search really does name the needed skill, and invoking it by
///    exact name really does return its body.
#[tokio::test]
#[serial_test::serial]
async fn a_withheld_skill_is_found_and_run_on_a_turn_that_needs_it() {
    let reqs = session(
        1_000,
        "m-skill",
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

    // PRECONDITION: it really was withheld.
    assert!(
        !reqs[0].system.contains(NEEDLE),
        "{NEEDLE} was in the prompt all along, so nothing about a WITHHELD \
         skill's reachability was measured"
    );

    // (1) THE ROUTE IS STATED.
    let block = reminder_containing(&reqs[0].system, MARK_ROUTE).unwrap_or_else(|| {
        panic!(
            "the prompt withholds every installed skill and says nothing about \
             how to find one. That is the wrong refusal this criterion \
             forbids, whatever the tool layer can still do. System \
             prompt:\n{}",
            reqs[0].system
        )
    });
    for mark in [MARK_QUERY, MARK_INVOKE, MARK_HYDRATE] {
        assert!(
            block.contains(mark),
            "the discovery block never mentions {mark}, so a model reading only \
             the prompt cannot execute the route: {block}"
        );
    }

    // (2) THE ROUTE IS EXECUTABLE — the hydration hop first. `Skill` is cold on
    // a default session, so the instruction to reach it through `ToolSearch`
    // has to be true.
    assert!(
        !reqs[0].tools.iter().any(|t| t.name == "Skill"),
        "Skill was already hot, so this session does not exercise the hop the \
         discovery block tells the model to take"
    );
    let catalog = reqs[0]
        .tools
        .iter()
        .find(|t| t.name == "ToolSearch")
        .map(|t| t.description.clone())
        .expect("ToolSearch is the hydration path and is never deferred");
    assert!(
        catalog.contains("Skill"),
        "the prompt tells the model to ToolSearch for `Skill` and ToolSearch's \
         own catalog does not name it: {catalog}"
    );
    assert!(
        reqs[1].tools.iter().any(|t| t.name == "Skill"),
        "the model asked ToolSearch for Skill and the next dispatch still does \
         not carry it. Shipped: {:?}",
        reqs[1].tools.iter().map(|t| &t.name).collect::<Vec<_>>()
    );

    // DISCOVERY: the search names the withheld skill, out of a thousand.
    let (search_body, search_err) = tool_results(&reqs[2])
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
         {NEEDLE}, so a withheld skill is undiscoverable: {search_body}"
    );
    // ...and the escape hatch stays bounded, or it undoes #1280 c1's ceiling on
    // the message-stream side instead of the prompt side.
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
        "invoking {NEEDLE} did not return its body, so the skill the gate \
         withholds is unlisted AND unusable: {run_body}"
    );
}

// ---------------------------------------------------------------------------
// c3 — the prefix does not churn
// ---------------------------------------------------------------------------

/// ARM A — WITHIN a session, across ordinary turns AND across an activation.
///
/// Measured off `LlmRequest.system`, which is segment 0 of an OpenAI-shaped
/// body: everything ahead of the tool schemas and the whole conversation, and
/// the only region an implicit-cache endpoint can reuse.
///
/// WHAT THIS ARM IS AND IS NOT. It is a REGRESSION guard, and it says so
/// plainly: today `build_system_prompt` has exactly one call site, at boot, so
/// segment 0 is a stored `String` and within-session stability is structural
/// rather than earned. It would go red the moment someone moves the assembly
/// onto the dispatch path to recompute relevance per turn — which is precisely
/// the trade c3 exists to refuse, and which would re-bill the whole prefix
/// uncached on the reporter's own endpoint. It is NOT the arm a nonce-in-
/// listing mutant reddens; that is arm B below, and the split is stated because
/// a lane already shipped two greens off an oracle that could not see the thing
/// it was pointed at.
///
/// NON-VACUITY: the session's MESSAGES are asserted to differ across the same
/// dispatches. A fixture where nothing moved would show both constant, and
/// "system is constant" would then be a fact about the fixture.
#[tokio::test]
#[serial_test::serial]
async fn the_system_prefix_is_identical_across_turns_and_an_activation() {
    let reqs = session(
        1_000,
        "m-skill",
        vec![
            plain_answer(),
            tool_round("c1", "ToolSearch", json!({ "query": "Skill" })),
            tool_round("c2", "Skill", json!({ "skill": NEEDLE })),
            plain_answer(),
        ],
        &[
            "What is 2 + 2?",
            "Reticulate the splines on the quarterly ledger.",
            "Thanks.",
        ],
    )
    .await;

    assert!(
        reqs.len() >= 4,
        "the session reached only {} dispatches, so nothing multi-turn was \
         measured",
        reqs.len()
    );

    // The activation really happened, or this is an ordinary-turns-only arm
    // wearing an activation's name.
    let activated = reqs.iter().any(|r| {
        tool_results(r)
            .iter()
            .any(|(b, e)| !e && b.contains(NEEDLE_BODY))
    });
    assert!(
        activated,
        "no dispatch carries the body of {NEEDLE}, so no skill was actually \
         activated in this session"
    );

    let first = &reqs[0].system;
    for (i, r) in reqs.iter().enumerate() {
        assert_eq!(
            &r.system, first,
            "dispatch {i}'s system prefix differs from dispatch 0's. On an \
             implicit-cache endpoint that re-bills the entire prefix, which is \
             a larger cost than the listing this issue removed"
        );
    }

    // NON-VACUITY: the conversation moved while the prefix did not.
    let msgs_first = reqs[0].messages.len();
    let msgs_last = reqs.last().expect("dispatches").messages.len();
    assert!(
        msgs_last > msgs_first,
        "the message stream did not grow across {} dispatches ({msgs_first} -> \
         {msgs_last}), so this session is not the multi-turn one the criterion \
         names and the equality above is the equality of a frozen fixture",
        reqs.len()
    );
    assert!(
        first.len() > 500,
        "the system prefix is only {} bytes; a nearly-empty prefix would make \
         every equality above trivially true",
        first.len()
    );
}

/// ARM B — ACROSS boots, which is the arm with teeth.
///
/// An implicit prompt cache is keyed on the prefix, and it is shared by every
/// request that presents the same one — across sessions, not only within one.
/// A skills section that varies with WHICH skills are installed, or with a
/// counter, or with anything else per render, mints a fresh prefix each time
/// and the cache never hits. Two boots over deliberately DIFFERENT catalogues
/// (ten skills named `a-skill-*`, a thousand named `m-skill-*`) must therefore
/// produce a byte-identical skills section.
///
/// This is the arm a nonce-in-listing mutant reddens, and the arm the pre-#1283
/// tree fails outright: the listing it rendered was a function of the installed
/// set by construction.
///
/// NON-VACUITY: both sections are asserted non-trivial and each boot is
/// asserted to have found its own catalogue, so "identical" cannot mean "both
/// empty".
#[tokio::test]
#[serial_test::serial]
async fn the_skills_section_is_identical_across_two_different_catalogues() {
    let small = session(10, "a-skill", vec![plain_answer()], &["What is 2 + 2?"]).await;
    let large = session(1_000, "m-skill", vec![plain_answer()], &["What is 2 + 2?"]).await;

    let a = reminder_containing(&small[0].system, MARK_ROUTE)
        .expect("boot A rendered no skills section")
        .to_string();
    let b = reminder_containing(&large[0].system, MARK_ROUTE)
        .expect("boot B rendered no skills section")
        .to_string();

    assert_eq!(
        a, b,
        "two boots with different installed skills produced different skills \
         sections, so the prefix is a function of the catalogue and no \
         implicit cache written by one session is readable by another"
    );
    assert!(
        a.len() > 200,
        "the skills section is only {} bytes; two empty strings are equal and \
         measure nothing",
        a.len()
    );

    // And neither boot leaked a name into the prefix.
    assert!(
        !small[0].system.contains("a-skill-"),
        "boot A named its own skills in the prefix"
    );
    assert!(
        !large[0].system.contains("m-skill-"),
        "boot B named its own skills in the prefix"
    );
}
