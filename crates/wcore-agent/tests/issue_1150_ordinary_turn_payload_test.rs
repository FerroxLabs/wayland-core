//! wayland#1150 c5 — what an ORDINARY chat turn actually injects.
//!
//! The reporter's Expected Behavior, verbatim: "Tool schemas and skills should
//! be injected only when relevant or explicitly activated, rather than on every
//! ordinary chat turn." Two halves, and until this file only the tool half had
//! any machinery and NEITHER half had an instrument that measured what a real
//! boot sends.
//!
//! Everything here is driven through the REAL `AgentBootstrap::build()` with an
//! injected recording provider, so the `LlmRequest` measured is the one the
//! engine hands a provider: the real built-in registry, the real cold-deferral
//! and catalog-fold passes, the real skill discovery, the real system prompt.
//! Re-composing those helpers by hand would grade a pipeline the product does
//! not run — how three vacuous guards shipped on this issue already.
//!
//! Sibling `issue_1150_implicit_prefix_cache_test.rs` cannot stand in for this:
//! its fixture sets `Config::system_prompt` directly, so `build_system_prompt`
//! — the ONLY place the skills listing is assembled — is never called on that
//! path, and its segment-0 assertion is blind to skills by construction.

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
use wcore_types::message::{FinishReason, StopReason, TokenUsage};

// ---------------------------------------------------------------------------
// Harness
// ---------------------------------------------------------------------------

/// Replays one script per dispatch, in order, and keeps every request.
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

/// The reporter's route: an unlisted local model over an OpenAI-compatible
/// endpoint, with no `[compact] context_window` — so the session assumes
/// `UNVERIFIED_CONTEXT_WINDOW` (32,768) and the skills budget is 1% of it.
fn config() -> Config {
    let mut cfg = Config {
        provider_label: "openai".into(),
        provider: ProviderType::OpenAI,
        api_key: "sk-test".into(),
        base_url: "http://localhost:0".into(),
        model: "issue-1150-local-32k-unlisted".into(),
        max_tokens: 1024,
        max_turns: Some(6),
        compat: ProviderCompat::openai_defaults(),
        ..Default::default()
    };
    cfg.tools.auto_approve = true;
    cfg.session.enabled = false;
    cfg
}

/// Plant `n` project skills of a realistic shape, none of which has anything
/// to do with the questions asked below.
fn plant_skills(root: &std::path::Path, n: usize) {
    let skills = root.join(".wayland-core").join("skills");
    for i in 0..n {
        let dir = skills.join(format!("m-skill-{i:03}"));
        std::fs::create_dir_all(&dir).expect("skill dir");
        let desc = format!("skill {i:03} ")
            + &"does a distinct thing worth describing at some length so the listing is realistic "
                .repeat(3);
        std::fs::write(
            dir.join("SKILL.md"),
            format!("---\nname: m-skill-{i:03}\ndescription: {desc}\n---\n\nbody\n"),
        )
        .expect("write SKILL.md");
    }
}

/// Drive `prompts` user turns on ONE engine and return every `LlmRequest` the
/// provider was handed.
async fn session(
    skill_count: usize,
    cfg: Config,
    scripts: Vec<Vec<LlmEvent>>,
    prompts: &[&str],
) -> Vec<LlmRequest> {
    let tmp = tempdir().expect("tempdir");
    let root = std::fs::canonicalize(tmp.path()).expect("canonicalize");
    plant_skills(&root, skill_count);

    let requests: Arc<Mutex<Vec<LlmRequest>>> = Arc::new(Mutex::new(Vec::new()));
    let provider = Arc::new(RecordingProvider {
        scripts: Mutex::new(scripts),
        requests: requests.clone(),
    });
    let sink: Arc<dyn OutputSink> = Arc::new(NullSink);
    let mut result =
        AgentBootstrap::new(cfg, root.to_str().expect("utf-8").to_string(), sink)
            .without_channels(true)
            .extra_skill_dirs(vec![root.clone()])
            .provider(provider)
            .build()
            .await
            .expect("bootstrap");

    for (i, p) in prompts.iter().enumerate() {
        result
            .engine
            .run(p, &format!("m{i}"))
            .await
            .expect("turn");
    }
    drop(result);
    let reqs = requests.lock().unwrap().clone();
    assert!(!reqs.is_empty(), "the engine dispatched nothing");
    reqs
}

async fn ordinary_turn(skill_count: usize, cfg: Config) -> LlmRequest {
    session(skill_count, cfg, vec![plain_answer()], &["What is 2 + 2?"])
        .await
        .remove(0)
}

/// Bytes a def costs when its FULL schema is serialized.
fn schema_bytes(t: &wcore_types::tool::ToolDef) -> usize {
    t.name.len()
        + t.description.len()
        + serde_json::to_string(&t.input_schema)
            .map(|s| s.len())
            .unwrap_or(0)
}

fn total_schema_bytes(req: &LlmRequest) -> usize {
    req.tools.iter().map(schema_bytes).sum()
}

/// The `<system-reminder>` skills block, if the turn carried one.
fn skills_block(system: &str) -> Option<&str> {
    let mark = "The following skills are available for use with the Skill tool:";
    let start = system.find(mark)?;
    let head = system[..start].rfind("<system-reminder>")?;
    let end = system[start..].find("</system-reminder>")? + start + "</system-reminder>".len();
    Some(&system[head..end])
}

// ---------------------------------------------------------------------------
// The TOOL half of c5
// ---------------------------------------------------------------------------

/// Measured on a real boot, with the pre-machinery arm as the CONTROL in the
/// same test so an empty result cannot read as absence.
///
/// Numbers at the time of writing (hetzner, clean HOME): deferral OFF ships 48
/// tools and 52,110 bytes of schema on EVERY turn; deferral ON ships 8 and
/// 8,902, with the other 40 folded out of `tools[]` entirely and named in a
/// 566-byte catalog line inside ToolSearch's description.
#[tokio::test]
async fn most_tool_schemas_are_not_shipped_on_an_ordinary_turn() {
    let on = ordinary_turn(10, config()).await;

    let mut off_cfg = config();
    off_cfg.builtin_tools.defer_cold.enabled = false;
    off_cfg.builtin_tools.defer_cold.catalog = false;
    let off = ordinary_turn(10, off_cfg).await;

    // CONTROL: the arm the machinery is measured against must actually carry
    // the whole registry, or the comparison below grades nothing.
    assert!(
        off.tools.len() >= 40,
        "the deferral-OFF control shipped only {} tools, so it is not the \
         whole-registry arm this test compares against",
        off.tools.len()
    );

    let hot: Vec<&str> = on
        .tools
        .iter()
        .filter(|t| !t.deferred)
        .map(|t| t.name.as_str())
        .collect();

    assert_eq!(
        on.tools.len(),
        hot.len(),
        "a deferred STUB entry reached the wire; the catalog fold is supposed to \
         remove every deferred def from tools[] outright. Shipped: {:?}",
        on.tools.iter().map(|t| &t.name).collect::<Vec<_>>()
    );

    assert!(
        on.tools.len() * 3 <= off.tools.len(),
        "an ordinary turn still ships {} of the registry's {} tools; the whole \
         point of deferral is that most of them are not there",
        on.tools.len(),
        off.tools.len()
    );

    let on_bytes = total_schema_bytes(&on);
    let off_bytes = total_schema_bytes(&off);
    assert!(
        on_bytes * 3 <= off_bytes,
        "tool schemas on an ordinary turn cost {on_bytes} bytes against the \
         deferral-off control's {off_bytes} — less than the 3x the machinery is \
         supposed to buy"
    );

    // Everything folded out must still be NAMED, or the model cannot ask for it.
    let catalog = on
        .tools
        .iter()
        .find(|t| t.name == "ToolSearch")
        .map(|t| t.description.clone())
        .expect("ToolSearch is the hydration path and is never deferred");
    let missing: Vec<&str> = off
        .tools
        .iter()
        .map(|t| t.name.as_str())
        .filter(|n| !hot.contains(n) && !catalog.contains(*n))
        .collect();
    assert!(
        missing.is_empty(),
        "these tools were folded out of tools[] AND left out of the catalog, so \
         the model can neither call them nor discover them: {missing:?}"
    );
}

/// WRONG-REFUSAL CONTROL. Withholding a schema the model then cannot call is
/// worse than a large prompt, so the conditional half is measured on a session
/// where the model DOES need a folded-out tool: it asks ToolSearch for it, and
/// the next dispatch must carry that tool's real schema.
#[tokio::test]
async fn a_folded_out_tool_becomes_callable_on_explicit_activation() {
    const WANTED: &str = "WebFetch";

    let reqs = session(
        10,
        config(),
        vec![
            tool_round("c1", "ToolSearch", json!({ "query": WANTED })),
            plain_answer(),
        ],
        &["Fetch a page for me."],
    )
    .await;

    assert!(
        reqs.len() >= 2,
        "the session did not reach a second dispatch, so nothing about \
         post-activation availability was measured"
    );

    // PRECONDITION: it really was absent before the model asked for it,
    // otherwise the assertion below passes for the wrong reason.
    assert!(
        !reqs[0].tools.iter().any(|t| t.name == WANTED),
        "{WANTED} was already on the wire before any ToolSearch call, so this \
         test cannot show activation did anything"
    );

    let admitted = reqs[1]
        .tools
        .iter()
        .find(|t| t.name == WANTED)
        .unwrap_or_else(|| {
            panic!(
                "the model explicitly asked ToolSearch for {WANTED} and the very \
                 next dispatch still does not carry it — a tool it cannot call. \
                 Shipped: {:?}",
                reqs[1].tools.iter().map(|t| &t.name).collect::<Vec<_>>()
            )
        });
    assert!(
        !admitted.deferred,
        "{WANTED} was admitted as a DEFERRED stub, so the provider still has no \
         schema to validate a call against"
    );
    assert!(
        !admitted.input_schema.is_null(),
        "{WANTED} was admitted without an input schema"
    );

    // And activation is PER TOOL, not a blanket un-deferral: something the
    // model never asked for must not ride in on the back of the one it did.
    let after = reqs[1].tools.len();
    let before = reqs[0].tools.len();
    assert_eq!(
        after,
        before + 1,
        "one explicit activation changed the tools[] array by {} entries; \
         hydration is supposed to admit exactly the tool that was asked for",
        after as i64 - before as i64
    );
}

// ---------------------------------------------------------------------------
// The SKILLS half of c5
// ---------------------------------------------------------------------------

/// The gap, pinned so it is falsifiable rather than asserted in a ledger note.
///
/// c5 asks for skills "injected only when relevant or explicitly activated".
/// There is no relevance gate and no activation gate on that path: two turns
/// whose text has nothing to do with any planted skill get byte-identical
/// listings naming every one of them.
///
/// This test deliberately does NOT assert that the listing fits the 1%-of-window
/// character budget in `wcore_skills::prompt`. An earlier cut of it did, on the
/// assumption that made the skills half look like a small lever, and the
/// assertion FAILED the first time it ran on a host with skills installed:
/// 2,359 bytes against a 1,310-char budget. The budget is not a ceiling —
/// `format_skills_within_budget` subtracts the bundled entries from it
/// (`remaining_budget = budget.saturating_sub(bundled_chars)`) and never caps
/// them, and its minimal mode still emits every non-bundled NAME. Both terms
/// grow linearly with the skill count and neither is bounded by the window.
/// Measured: 100 bundled + 10 project skills render 22,399 chars, 17.1x the
/// budget, about 5,600 tokens of a 32,768-token window, on every ordinary turn.
/// That is FerroxLabs/wayland#1274, filed rather than fixed here, and it is why
/// this file asserts only what is true today.
///
/// When a gate is built, the first assertion below is the one that must go red.
#[tokio::test]
async fn the_skills_listing_is_unconditional_on_an_ordinary_turn() {
    let reqs = session(
        10,
        config(),
        vec![plain_answer()],
        &["What is 2 + 2?", "Name a colour."],
    )
    .await;

    let first = skills_block(&reqs[0].system).expect("a skills listing was rendered");
    let second = skills_block(&reqs[reqs.len() - 1].system)
        .expect("a skills listing was rendered on the later turn too");

    assert_eq!(
        first, second,
        "the two turns got different skill listings; if that is now a relevance \
         gate, this test is measuring the OLD contract and must be rewritten \
         around the new one"
    );

    let named = (0..10)
        .filter(|i| first.contains(&format!("m-skill-{i:03}")))
        .count();
    assert_eq!(
        named, 10,
        "every planted skill is listed on a turn about arithmetic; none of them \
         is relevant to it and none was activated. This is c5's open half"
    );

    // NON-VACUITY: the listing must actually be carrying something, or the
    // equality above is the equality of two empty strings.
    assert!(
        first.len() > 200,
        "the skills listing is only {} bytes; this test would pass on a session \
         with no skills at all, which measures nothing",
        first.len()
    );
}

/// The constraint any future relevance gate has to satisfy, measured on the
/// path that actually assembles the listing.
///
/// The skills block lives in the system prompt, which is segment 0 of an
/// OpenAI-shaped body — ahead of the tool schemas and the entire conversation.
/// On the reporter's own provider shape (LM Studio, implicit prefix cache) a
/// system prompt that varies per turn is a total loss of reuse at token 0 on
/// every request. A naive per-turn relevance gate would therefore trade ~330
/// tokens of listing for re-billing the whole prompt uncached every turn, which
/// makes #1150's reported symptom WORSE.
#[tokio::test]
async fn the_system_prompt_is_byte_identical_across_the_turns_of_a_session() {
    let reqs = session(
        10,
        config(),
        vec![plain_answer()],
        &["What is 2 + 2?", "Name a colour.", "And another."],
    )
    .await;

    assert!(
        reqs.len() >= 3,
        "only {} dispatches; this test needs several turns to measure stability",
        reqs.len()
    );
    // PRECONDITION: there is a skills listing in there at all, or this asserts
    // that a constant is constant.
    assert!(
        skills_block(&reqs[0].system).is_some(),
        "no skills listing in the system prompt, so its stability is vacuous"
    );

    for (i, r) in reqs.iter().enumerate().skip(1) {
        assert_eq!(
            r.system, reqs[0].system,
            "dispatch {i} changed the system prompt, so on an implicit-cache \
             endpoint every dispatch from here on re-bills its entire context at \
             full price"
        );
    }
}
