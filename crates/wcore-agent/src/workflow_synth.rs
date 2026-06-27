//! B7 — natural-language → RON workflow synthesis.
//!
//! Turns a free-text task into a *validated* [`WorkflowPlan`] by asking the
//! LLM to emit a RON `Workflow(...)` document, extracting that block from the
//! response, and validating it through A1's parser ([`WorkflowPlan::parse`]).
//!
//! ## Contract (PLAN B7)
//!
//! - **Reuse, don't re-classify.** The synthesis prompt folds in
//!   [`workflow_candidate`]'s rationale as context when the heuristic fired,
//!   instead of running a second classifier. The signal is advisory only.
//! - **Validate via the real parser.** The synthesised RON is only accepted
//!   if [`WorkflowPlan::parse`] succeeds — the same path the explicit tier and
//!   the runner use, so a synthesised plan is runnable by construction.
//! - **Retry on EITHER failure mode, up to [`MAX_SYNTH_ATTEMPTS`].** A real
//!   model frequently answers the first time with prose, a tool call, or an
//!   exploration step — producing NO parseable `Workflow(...)` block at all —
//!   and only sometimes with an extractable-but-invalid block. The synthesis
//!   loop re-prompts on BOTH a missing/unparseable block (`extract_ron` →
//!   `None`, corrected with a terse "you did not output a RON block") AND an
//!   extracted block that [`WorkflowPlan::parse`] rejects (corrected with the
//!   parse error fed back verbatim). After the last attempt a typed
//!   [`SynthError`] aborts the call — **no silent fallback, no fabricated
//!   workflow.**
//!
//! ## Why an [`AgentSpawner`], not a bare provider
//!
//! Every other workflow call in this crate makes its one-shot LLM call through
//! [`AgentSpawner::spawn_one`] (the runner's per-stage dispatch, the schema
//! retry path). Synthesis reuses that seam rather than hand-rolling an
//! `AgentEngine`: `spawn_one` already builds a read-only child engine, runs one
//! turn loop, and returns the text. The spawner owns the `Arc<dyn LlmProvider>`,
//! so passing the spawner is equivalent to passing the provider plus the wiring.
//!
//! This module is a standalone capability: the live confirm gate (B6) that will
//! invoke it is deferred, so nothing here adds a UI or trigger.

use thiserror::Error;

use crate::orchestration::intent::workflow_candidate;
use crate::orchestration::workflow::error::WorkflowParseError;
use crate::orchestration::workflow::runner::WorkflowPlan;
use crate::spawner::{AgentSpawner, SubAgentConfig};

/// Turn budget for the synthesis sub-agent. Synthesis is a *compiler*: one
/// turn that emits the RON document. A budget of 1 denies the model the extra
/// turns it would otherwise spend exploring the codebase via its read-only
/// tools (Read/Grep/Glob) instead of emitting RON directly.
const SYNTH_MAX_TURNS: usize = 1;

/// Maximum number of synthesis attempts (the initial call plus re-prompts).
/// Each attempt is one [`AgentSpawner::spawn_one`] call. The loop retries on
/// BOTH a missing/unparseable block and a parse failure, so a model that opens
/// with prose still gets corrective tries before the call aborts.
const MAX_SYNTH_ATTEMPTS: u8 = 3;

/// System prompt pinned onto the synthesis sub-agent. It frames the agent as a
/// pure RON compiler and forbids tool use so the model spends its single turn
/// emitting the `Workflow(...)` document rather than exploring files.
const SYNTH_SYSTEM_PROMPT: &str = "You are a workflow compiler. Your ONLY job is to \
translate the user's task into a single RON `Workflow(...)` document. You MUST NOT call \
any tools, read or search files, or explore the codebase. You MUST NOT write prose, \
explanations, or markdown fences. Respond with the `Workflow(...)` document and nothing \
else.";

/// Token budget for the synthesis sub-agent. A workflow document is small;
/// 4096 leaves ample headroom for the grammar echo plus the emitted RON.
const SYNTH_MAX_TOKENS: u32 = 4096;

/// Failure modes of [`synthesize_workflow`]. Every variant is terminal — the
/// caller must surface the error, never silently substitute a workflow.
#[derive(Debug, Error)]
pub enum SynthError {
    /// Every attempt's response lacked a recognisable `Workflow(...)` RON block
    /// (the model answered with prose, a tool call, or an empty extraction on
    /// the LAST attempt). Carries the final attempt count and the raw response
    /// for context.
    #[error("no `Workflow(...)` RON block found in the model response (attempt {attempt})")]
    NoRonBlock { attempt: u8, response: String },

    /// The synthesised RON still failed to parse on the final attempt. Carries
    /// the last parse error — this is the abort path (no fallback workflow is
    /// fabricated). The `attempt` field records the attempt that produced it.
    #[error("synthesised workflow did not parse after {attempt} attempts: {source}")]
    InvalidAfterReprompt {
        attempt: u8,
        #[source]
        source: WorkflowParseError,
    },

    /// The synthesis sub-agent itself reported an LLM-layer error (the engine
    /// returned an error result rather than text). Carries the message.
    #[error("synthesis sub-agent failed: {0}")]
    AgentError(String),
}

/// Synthesise a validated [`WorkflowPlan`] from a natural-language `task`.
///
/// Makes up to [`MAX_SYNTH_ATTEMPTS`] LLM calls (via `spawner`). The first
/// embeds the RON grammar plus a worked example so the model emits a valid
/// `Workflow(...)` document. Each response is extracted with [`extract_ron`]
/// and validated with [`WorkflowPlan::parse`]; on EITHER failure mode the loop
/// re-prompts — a missing/unparseable block gets a terse "you did not output a
/// RON block" correction (the model likely answered with prose or a tool call),
/// while an extracted-but-invalid block gets the parse error fed back verbatim.
/// After the final attempt a typed [`SynthError`] aborts — no silent fallback.
///
/// `task` is the user's free-text request. The function reuses
/// [`workflow_candidate`]'s rationale as prompt context when the heuristic
/// fired; it never re-runs classification as a gate.
pub async fn synthesize_workflow(
    task: &str,
    spawner: &AgentSpawner,
) -> Result<WorkflowPlan, SynthError> {
    // Reuse B3's intent reading as advisory context (not a re-classification).
    let rationale = workflow_candidate(task).map(|c| c.rationale);

    // Attempt 1: the full synthesis prompt (grammar + example + task + signal).
    let mut prompt = build_synth_prompt(task, rationale.as_deref());
    // Tracks the most recent failure so the FINAL attempt's failure surfaces a
    // precise typed error (parse error vs. missing block).
    let mut last_failure: SynthFailure = SynthFailure::Missing;

    for attempt in 1..=MAX_SYNTH_ATTEMPTS {
        let name = if attempt == 1 {
            "workflow-synth"
        } else {
            "workflow-synth-retry"
        };
        let response = dispatch(spawner, name, prompt).await?;

        let Some(ron) = extract_ron(&response) else {
            // No parseable block: the model gave prose / a tool call / nothing.
            last_failure = SynthFailure::Missing;
            if attempt == MAX_SYNTH_ATTEMPTS {
                return Err(SynthError::NoRonBlock { attempt, response });
            }
            prompt = build_missing_block_reprompt(task);
            continue;
        };

        match WorkflowPlan::parse(&ron) {
            Ok(plan) => return Ok(plan),
            Err(err) => {
                if attempt == MAX_SYNTH_ATTEMPTS {
                    return Err(SynthError::InvalidAfterReprompt {
                        attempt,
                        source: err,
                    });
                }
                // Re-prompt with the prior RON + parse error appended.
                prompt = build_reprompt(task, &ron, &err);
                last_failure = SynthFailure::Parse;
            }
        }
    }

    // Unreachable: every loop iteration either returns or, on the last attempt,
    // returns inside the branches above. Guard with the last failure rather
    // than panic, so a future edit to the loop bound can't reach a bad state.
    Err(match last_failure {
        SynthFailure::Missing => SynthError::NoRonBlock {
            attempt: MAX_SYNTH_ATTEMPTS,
            response: String::new(),
        },
        SynthFailure::Parse => SynthError::InvalidAfterReprompt {
            attempt: MAX_SYNTH_ATTEMPTS,
            source: WorkflowParseError::Ron("synthesis loop exhausted".to_string()),
        },
    })
}

/// Which failure the most recent attempt hit — used only to pick the typed
/// error in the (logically unreachable) post-loop guard.
enum SynthFailure {
    /// `extract_ron` returned `None`.
    Missing,
    /// The block extracted but `WorkflowPlan::parse` rejected it.
    Parse,
}

/// One-shot LLM dispatch through the spawner, mapping an agent-layer error to a
/// typed [`SynthError::AgentError`].
async fn dispatch(
    spawner: &AgentSpawner,
    name: &str,
    prompt: String,
) -> Result<String, SynthError> {
    let result = spawner
        .spawn_one(SubAgentConfig {
            name: name.to_string(),
            prompt,
            max_turns: SYNTH_MAX_TURNS,
            max_tokens: SYNTH_MAX_TOKENS,
            // Pin the compiler framing so the model emits RON in its single
            // turn instead of exploring the codebase with its read-only tools.
            system_prompt: Some(SYNTH_SYSTEM_PROMPT.to_string()),
            provider: None,
            model: None,
            temperature: None,
        })
        .await;
    if result.is_error {
        return Err(SynthError::AgentError(result.text));
    }
    Ok(result.text)
}

/// The RON grammar reference embedded in every synthesis prompt. Kept in lock-
/// step with the `Workflow`/`Phase`/`Step`/`AgentSpec` shape documented in
/// `orchestration/workflow/dsl.rs` so the model emits RON the real parser
/// accepts.
const RON_GRAMMAR: &str = r#"A Workflow is a RON document with this exact shape:

Workflow(
    meta: (name: "short-id", description: "one line", est_agents: 4),
    // Optional named JSON Schemas a step may reference by name.
    schemas: { "findings": "{ \"type\": \"object\" }" },
    phases: [
        Phase(
            title: "label",
            steps: [
                // A single sub-agent call.
                Agent((id: "scan", prompt: "scan the diff")),
                // An ordered chain; each stage may read a prior node's output
                // via `input: Some("<prior node id>")`.
                Pipeline(id: "review", stages: [
                    (id: "lint",   prompt: "lint it"),
                    (id: "verify", prompt: "verify", schema: Some("findings"), input: Some("lint")),
                ]),
                // 2+ sibling agents that fan into a join (Collect | Merge | Concat).
                Parallel(id: "vote", branches: [
                    (id: "judge_a", prompt: "judge a"),
                    (id: "judge_b", prompt: "judge b"),
                ], join: Collect),
            ],
        ),
    ],
)

Rules you MUST follow:
- Every node `id` is unique across the whole workflow.
- A step's `input` (and a stage's) may only reference an EARLIER node's id.
- A `schema: Some("name")` must name a key declared in the top-level `schemas`.
- `Parallel` needs at least two branches.
- A `Pipeline` may add `over: Some("<state key>")` to stream each element of a
  state array through its stages independently (a per-item map).
- The initial state is pre-seeded with two keys you may reference directly from
  `over:` or `input:`: `changed_files` (an array of the repo's changed file
  paths) and `cwd` (the working-directory string). To fan work across the
  changed files, use `over: Some("changed_files")`. Only reference `over:` keys
  that are either one of these seeded keys or an EARLIER node's id — referencing
  an unset key fans over nothing and the workflow does no work.
- `meta.name` is required; `description`/`est_agents`/`schemas` are optional."#;

/// A concrete, minimal, VALID worked example embedded in the first-attempt
/// prompt. A real model anchors far better on one example than on grammar
/// prose alone. Shape: a no-barrier `Pipeline` mapping `over` a state array
/// (`changed_files`) feeding a `Parallel` verify that fans into a `Collect`
/// join — exercising the two most useful step kinds in ~10 lines.
///
/// This MUST stay parseable: `synth_example_parses` locks it to
/// [`WorkflowPlan::parse`] so it can never drift invalid.
const EXAMPLE_WORKFLOW: &str = r#"Workflow(
    meta: (name: "review-each-file", description: "review every changed file", est_agents: 3),
    phases: [
        Phase(title: "review", steps: [
            Pipeline(id: "per_file", over: Some("changed_files"), stages: [
                (id: "read_file", prompt: "read the file"),
                (id: "review_file", prompt: "review it for bugs", input: Some("read_file")),
            ]),
            Parallel(id: "verify", branches: [
                (id: "verify_a", prompt: "double-check the findings", input: Some("per_file")),
                (id: "verify_b", prompt: "rate severity", input: Some("per_file")),
            ], join: Collect),
        ]),
    ],
)"#;

/// Build the first-attempt synthesis prompt: grammar + task + optional signal.
fn build_synth_prompt(task: &str, rationale: Option<&str>) -> String {
    let mut prompt = String::new();
    prompt.push_str(
        "You design multi-agent workflows. Convert the user's task into a workflow \
         expressed in the RON grammar below.\n\n",
    );
    prompt.push_str(RON_GRAMMAR);
    prompt.push_str("\n\n--- worked example (valid Workflow) ---\n");
    prompt.push_str(EXAMPLE_WORKFLOW);
    prompt.push_str("\n\n--- task ---\n");
    prompt.push_str(task);
    if let Some(rationale) = rationale {
        // Advisory context from B3's heuristic — NOT a re-classification gate.
        prompt.push_str("\n\n--- detector context (advisory) ---\n");
        prompt.push_str(rationale);
    }
    prompt.push_str(CLOSING_INSTRUCTION);
    prompt
}

/// The forceful closing line appended to every synthesis prompt. Demands a bare
/// RON document and explicitly forbids the failure modes a live model exhibits:
/// tool use, file exploration, prose, and markdown fences.
const CLOSING_INSTRUCTION: &str = "\n\nOutput ONLY the RON document. It MUST start with \
`Workflow(`. Do NOT use tools, do NOT explore files, do NOT write any prose or markdown fences.";

/// Re-prompt for the case where the model produced NO parseable `Workflow(...)`
/// block (prose, a tool call, or an empty response). Distinct from
/// [`build_reprompt`], which handles an extracted-but-invalid block.
fn build_missing_block_reprompt(task: &str) -> String {
    let mut prompt = String::new();
    prompt.push_str(
        "You did not output a RON block. Respond with ONLY a `Workflow(...)` document and \
         nothing else — no prose, no tool calls, no markdown.\n\n",
    );
    prompt.push_str(RON_GRAMMAR);
    prompt.push_str("\n\n--- worked example (valid Workflow) ---\n");
    prompt.push_str(EXAMPLE_WORKFLOW);
    prompt.push_str("\n\n--- task ---\n");
    prompt.push_str(task);
    prompt.push_str(CLOSING_INSTRUCTION);
    prompt
}

/// Build the single re-prompt: the prior (invalid) RON plus the parse error,
/// demanding a corrected `Workflow(...)` block. Mirrors the schema-retry phrasing
/// in `runner.rs` so the correction request reads consistently.
fn build_reprompt(task: &str, prior: &str, err: &WorkflowParseError) -> String {
    let mut prompt = String::new();
    prompt.push_str(RON_GRAMMAR);
    prompt.push_str("\n\n--- task ---\n");
    prompt.push_str(task);
    prompt.push_str("\n\n--- your previous RON ---\n");
    prompt.push_str(prior);
    prompt.push_str("\n\nYour RON did not parse: ");
    prompt.push_str(&err.to_string());
    prompt.push_str(". Emit ONLY a valid Workflow(...) RON block.");
    prompt
}

/// Extract a `Workflow(...)` RON block from a model response.
///
/// Tolerant of the common ways a model wraps code: it first strips a fenced
/// block (``` / ```ron) if present, then locates the `Workflow(` keyword and
/// returns the balanced-parenthesis span starting there. Returns `None` if no
/// `Workflow(` opener exists or its parentheses never balance.
///
/// The prompt forbids fences and prose, but a model may ignore that; extraction
/// recovers the block regardless so a single stray fence does not waste the lone
/// re-prompt.
fn extract_ron(response: &str) -> Option<String> {
    // Prefer the contents of a fenced block when one is present — it isolates
    // the code from any surrounding prose. If there is no fence, scan the whole
    // response.
    let haystack = strip_fence(response).unwrap_or(response);

    let start = haystack.find("Workflow(")?;
    let after_kw = start + "Workflow".len(); // points at the '('
    let mut depth = 0usize;
    // Walk by char index so the returned slice is valid UTF-8 boundaries.
    for (offset, ch) in haystack[after_kw..].char_indices() {
        match ch {
            '(' => depth += 1,
            ')' => {
                depth -= 1;
                if depth == 0 {
                    let end = after_kw + offset + ch.len_utf8();
                    return Some(haystack[start..end].to_string());
                }
            }
            _ => {}
        }
    }
    None
}

/// If `text` contains a Markdown code fence, return the slice between the first
/// pair of ``` lines (dropping an optional language tag on the opening fence).
/// Returns `None` when there is no fenced block.
fn strip_fence(text: &str) -> Option<&str> {
    let open = text.find("```")?;
    // Skip the opening fence and any language tag up to the end of that line.
    let after_open = open + 3;
    let body_start = match text[after_open..].find('\n') {
        Some(nl) => after_open + nl + 1,
        // Opening fence with no newline after it — nothing usable inside.
        None => return None,
    };
    let close_rel = text[body_start..].find("```")?;
    Some(&text[body_start..body_start + close_rel])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extract_finds_bare_workflow_block() {
        let resp = r#"Workflow(meta: (name: "x"), phases: [Phase(steps: [Agent((id: "a", prompt: "p"))])])"#;
        let got = extract_ron(resp).expect("should extract");
        assert!(got.starts_with("Workflow("));
        assert!(got.ends_with(')'));
    }

    #[test]
    fn extract_strips_markdown_fences_and_prose() {
        let resp = "Sure, here you go:\n\n```ron\nWorkflow(meta: (name: \"x\"), phases: [Phase(steps: [Agent((id: \"a\", prompt: \"p\"))])])\n```\n\nHope that helps!";
        let got = extract_ron(resp).expect("should extract from inside fence");
        assert!(got.starts_with("Workflow("));
        // The trailing prose and the fence are gone.
        assert!(!got.contains("```"));
        assert!(!got.contains("Hope that helps"));
    }

    #[test]
    fn extract_handles_nested_parens_via_balance() {
        // The grammar nests parens deeply (Phase(...), Agent((...)) ); the
        // extractor must return the OUTERMOST balanced span, not the first ')'.
        let resp = r#"prose Workflow(meta: (name: "x", est_agents: 2), phases: [Phase(steps: [Agent((id: "a", prompt: "p"))])]) trailing"#;
        let got = extract_ron(resp).expect("should extract");
        assert!(got.ends_with(')'));
        assert!(!got.contains("trailing"));
        // Round-trips through the real parser.
        assert!(WorkflowPlan::parse(&got).is_ok());
    }

    #[test]
    fn extract_returns_none_without_workflow_keyword() {
        assert!(extract_ron("no workflow here, just text").is_none());
    }

    #[test]
    fn extract_returns_none_on_unbalanced_parens() {
        assert!(extract_ron("Workflow(meta: (name:").is_none());
    }

    #[test]
    fn synth_prompt_embeds_grammar_and_task() {
        let p = build_synth_prompt("review every file", Some("workflow signals: every file"));
        assert!(p.contains("Workflow("));
        assert!(p.contains("review every file"));
        // The advisory detector context is folded in when present.
        assert!(p.contains("workflow signals: every file"));
        assert!(p.contains("detector context"));
        // The worked example is embedded.
        assert!(p.contains("worked example"));
        assert!(p.contains(EXAMPLE_WORKFLOW));
        // The forceful closing forbids tools / exploration / prose / fences.
        assert!(p.contains("Do NOT use tools"));
        assert!(p.contains("MUST start with `Workflow(`"));
    }

    #[test]
    fn synth_example_parses() {
        // Lock the few-shot example so it can never drift invalid: the real
        // parser must accept it. A parse failure here means a model shown the
        // example would be anchored to broken RON.
        let plan = WorkflowPlan::parse(EXAMPLE_WORKFLOW)
            .expect("the embedded worked example must parse via the real parser");
        assert_eq!(plan.meta.name, "review-each-file");
    }

    #[test]
    fn missing_block_reprompt_demands_bare_ron() {
        let p = build_missing_block_reprompt("do the thing");
        // The distinctive correction the loop emits when extraction returns None.
        assert!(p.contains("You did not output a RON block"));
        assert!(p.contains("no prose, no tool calls, no markdown"));
        // Still carries the task + grammar + example so the model can recover.
        assert!(p.contains("do the thing"));
        assert!(p.contains("Workflow("));
        assert!(p.contains(EXAMPLE_WORKFLOW));
    }

    #[test]
    fn synth_prompt_omits_detector_block_when_absent() {
        let p = build_synth_prompt("do a thing", None);
        assert!(!p.contains("detector context"));
    }

    #[test]
    fn reprompt_carries_the_parse_error_verbatim() {
        let err = WorkflowParseError::Ron("expected `Workflow`".to_string());
        let p = build_reprompt("a task", "bad ron", &err);
        assert!(p.contains("Your RON did not parse"));
        assert!(p.contains("expected `Workflow`"));
        assert!(p.contains("bad ron"));
        assert!(p.contains("Emit ONLY a valid Workflow(...) RON block"));
    }
}
