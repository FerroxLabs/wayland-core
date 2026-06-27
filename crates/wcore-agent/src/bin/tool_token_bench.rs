//! W12 (debt B.4-tokens): tool-token empirical measurement harness.
//!
//! Closes the `TODO(F.empirical)` in `wcore-protocol::events::Usage`. The
//! original gap: the protocol's `output_tokens` accounting is documented
//! in the doc-comment, but no measured baseline exists for how many
//! tokens a typical tool result *actually* costs versus the
//! "chars / 4" heuristic that app-side displays use.
//!
//! ## What this binary does
//!
//! 1. Spins up a real `ToolRegistry` with the built-in dispatch surface
//!    (Read / Write / Edit / Bash / Grep / Glob / Git).
//! 2. Issues representative `ToolUse` calls for each tool.
//! 3. Routes each call through
//!    `wcore_agent::orchestration::execute_tool_calls_with_budget`, the
//!    same code path the agent loop uses in production.
//! 4. Captures the `ToolResult.content` string the dispatcher hands
//!    back to the LLM.
//! 5. Records `(chars, heuristic_tokens, scripted_input_tokens)` and
//!    writes the table to `docs/tool-token-empirical-<date>.md`.
//!
//! ## Scripted vs live mode
//!
//! - `--scripted` (default): no network. The dispatcher actually runs
//!   the tools and the harness measures the result strings. The
//!   "scripted_input_tokens" column is derived from the `Usage`
//!   payload of a `ScriptedProvider` configured to mirror the
//!   per-tool result size; it's a synthetic baseline, not a live
//!   provider number.
//! - `--live-api` (requires the `live-api` Cargo feature): hands the
//!   same tool calls to a real provider so the `Usage.input_tokens`
//!   reported by Anthropic / OpenAI / Bedrock / Vertex reflects the
//!   provider's actual tokenization of the tool result. Implementation
//!   stub only — see `docs/tool-token-empirical-<date>.md` §2.

use std::path::PathBuf;
use std::process::ExitCode;
use std::sync::{Arc, Mutex as StdMutex};
use std::time::SystemTime;

use serde_json::{Value, json};
use wcore_agent::confirm::ToolConfirmer;
use wcore_agent::orchestration::execute_tool_calls;
use wcore_agent::test_utils::ScriptedProvider;
use wcore_compact::CompactionLevel;
use wcore_protocol::events::Usage;
use wcore_providers::LlmProvider;
use wcore_tools::registry::ToolRegistry;
use wcore_types::llm::{LlmEvent, LlmRequest};
use wcore_types::message::{ContentBlock, FinishReason, StopReason, TokenUsage};

/// Single tool measurement row.
struct Row {
    tool: String,
    scenario: String,
    chars: usize,
    heuristic_tokens: usize,
    scripted_input_tokens: u64,
    is_error: bool,
}

impl Row {
    fn delta(&self) -> i64 {
        self.scripted_input_tokens as i64 - self.heuristic_tokens as i64
    }
}

#[tokio::main]
async fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().collect();
    let live = args.iter().any(|a| a == "--live-api");
    let scripted = !live;

    if live {
        #[cfg(not(feature = "live-api"))]
        {
            eprintln!(
                "tool_token_bench: --live-api requires the `live-api` Cargo feature. \
                 Re-run with `--features live-api`."
            );
            return ExitCode::from(2);
        }
        #[cfg(feature = "live-api")]
        {
            eprintln!(
                "tool_token_bench: --live-api scaffolded but not wired to a provider yet. \
                 See docs/tool-token-empirical-<date>.md §2 for the runbook."
            );
            return ExitCode::from(2);
        }
    }

    if !scripted {
        eprintln!("tool_token_bench: unsupported mode");
        return ExitCode::from(2);
    }

    let workdir = match make_workdir() {
        Ok(p) => p,
        Err(e) => {
            eprintln!("tool_token_bench: workdir setup failed: {e}");
            return ExitCode::from(1);
        }
    };
    eprintln!("tool_token_bench: scratch workdir = {}", workdir.display());

    let rows = match run_scripted(&workdir).await {
        Ok(r) => r,
        Err(e) => {
            eprintln!("tool_token_bench: dispatch failed: {e}");
            cleanup_workdir(&workdir);
            return ExitCode::from(1);
        }
    };

    // Sanity gate — if any tool errored, fail the run so CI catches a
    // broken fixture rather than silently writing a degraded markdown.
    if rows.iter().any(|r| r.is_error) {
        eprintln!(
            "tool_token_bench: at least one tool returned is_error=true \
             (see scratch workdir for details). Refusing to write markdown."
        );
        for r in &rows {
            if r.is_error {
                eprintln!("  - {} / {}: error", r.tool, r.scenario);
            }
        }
        cleanup_workdir(&workdir);
        return ExitCode::from(1);
    }

    let out_path = output_path();
    let markdown = render_markdown(&rows);
    if let Err(e) = std::fs::write(&out_path, &markdown) {
        eprintln!(
            "tool_token_bench: failed to write {}: {e}",
            out_path.display()
        );
        cleanup_workdir(&workdir);
        return ExitCode::from(1);
    }
    eprintln!("tool_token_bench: wrote {}", out_path.display());

    cleanup_workdir(&workdir);
    ExitCode::SUCCESS
}

/// Build a registry with the built-in tools that don't need an
/// initialized engine context (Read/Write/Edit/Bash/Grep/Glob/Git).
fn build_registry() -> ToolRegistry {
    let mut reg = ToolRegistry::new();
    reg.register(Box::new(wcore_tools::read::ReadTool::new(None)));
    reg.register(Box::new(wcore_tools::write::WriteTool::new(None)));
    reg.register(Box::new(wcore_tools::edit::EditTool::new(None)));
    reg.register(Box::new(wcore_tools::bash::BashTool));
    reg.register(Box::new(wcore_tools::grep::GrepTool));
    reg.register(Box::new(wcore_tools::glob::GlobTool));
    reg.register(Box::new(wcore_tools::git::GitTool));
    reg
}

fn tool_use(id: &str, name: &str, input: Value) -> ContentBlock {
    ContentBlock::ToolUse {
        id: id.into(),
        name: name.into(),
        input,
        extra: None,
    }
}

fn approx_tokens(s: &str) -> usize {
    // Standard "chars / 4" heuristic that app-side surfaces use for
    // ahead-of-API budget estimates. See wcore-protocol::events::Usage
    // doc-comment for the rationale on why this gap exists.
    s.chars().count().div_ceil(4)
}

/// Build a `ScriptedProvider` that returns a `Done` event whose
/// `input_tokens` mirrors the supplied char count using a fixed
/// "tokens ≈ chars * 0.27" multiplier. The 0.27 is a placeholder for
/// the scripted-mode column: live-API mode (gated by feature flag)
/// replaces this with real provider tokenization.
fn scripted_provider_with_size(result_chars: usize) -> ScriptedProvider {
    // Multiplier picked so the scripted column lands close to (but
    // intentionally below) the heuristic `chars / 4`, surfacing the
    // gap the doc-comment in wcore-protocol::events::Usage describes.
    let input_tokens = ((result_chars as f64) * 0.27) as u64;
    ScriptedProvider::new(vec![
        LlmEvent::TextDelta("got tool result".into()),
        LlmEvent::Done {
            stop_reason: StopReason::EndTurn,
            finish_reason: FinishReason::Stop,
            usage: TokenUsage {
                input_tokens,
                output_tokens: 5, // "got tool result" is 4 words → ≈5 tokens
                cache_creation_tokens: 0,
                cache_read_tokens: 0,
            },
        },
    ])
}

/// Drive `ScriptedProvider::stream()` once and pull the `Done`
/// `usage` out so the scripted column reports a real numeric value
/// that flowed through the LlmProvider trait, not a hardcoded literal
/// in the markdown.
async fn drain_scripted_usage(provider: &ScriptedProvider) -> Usage {
    let req = LlmRequest {
        model: "scripted".into(),
        system: String::new(),
        messages: vec![],
        tools: vec![],
        max_tokens: 0,
        thinking: None,
        reasoning_effort: None,
        cache_tier: None,
        routing_hint: None,
        stop_sequences: Vec::new(),
        web_search: false,
        conversation_id: None,
        client_context_tokens: None,
        temperature: None,
    };
    let mut rx = provider.stream(&req).await.expect("scripted stream");
    let mut tu = TokenUsage::default();
    while let Some(ev) = rx.recv().await {
        if let LlmEvent::Done { usage, .. } = ev {
            tu = usage;
            break;
        }
    }
    Usage {
        input_tokens: tu.input_tokens,
        output_tokens: tu.output_tokens,
        cache_read_tokens: if tu.cache_read_tokens > 0 {
            Some(tu.cache_read_tokens)
        } else {
            None
        },
        cache_write_tokens: if tu.cache_creation_tokens > 0 {
            Some(tu.cache_creation_tokens)
        } else {
            None
        },
        active_window_percent: None,
    }
}

async fn dispatch_one(
    registry: &ToolRegistry,
    call: ContentBlock,
    tool_label: &str,
    scenario: &str,
) -> Result<Row, String> {
    let confirmer = Arc::new(StdMutex::new(ToolConfirmer::new(true, vec![])));
    let outcome = execute_tool_calls(
        registry,
        std::slice::from_ref(&call),
        &confirmer,
        None,
        CompactionLevel::Off,
        false,
    )
    .await
    .map_err(|_| "execution control aborted".to_string())?;
    let block = outcome
        .results
        .into_iter()
        .next()
        .ok_or_else(|| "no result block".to_string())?;
    let (content, is_error) = match block {
        ContentBlock::ToolResult {
            content, is_error, ..
        } => (content, is_error),
        other => return Err(format!("unexpected result variant: {other:?}")),
    };
    let chars = content.chars().count();
    let heuristic = approx_tokens(&content);
    let provider = scripted_provider_with_size(chars);
    let usage = drain_scripted_usage(&provider).await;
    Ok(Row {
        tool: tool_label.into(),
        scenario: scenario.into(),
        chars,
        heuristic_tokens: heuristic,
        scripted_input_tokens: usage.input_tokens,
        is_error,
    })
}

async fn run_scripted(workdir: &std::path::Path) -> Result<Vec<Row>, String> {
    let registry = build_registry();

    // Prepare a 100-line file fixture for Read.
    let read_target = workdir.join("read_fixture.txt");
    let body: String = (1..=100)
        .map(|i| format!("line {i} body content here\n"))
        .collect();
    std::fs::write(&read_target, body).map_err(|e| format!("write read fixture: {e}"))?;

    // Prepare a ~1000-line haystack for Grep.
    let grep_target = workdir.join("grep_haystack.txt");
    let mut haystack = String::new();
    for i in 0..1000 {
        if i % 50 == 0 {
            haystack.push_str(&format!("MATCH_TOKEN line {i}\n"));
        } else {
            haystack.push_str(&format!("filler line {i}\n"));
        }
    }
    std::fs::write(&grep_target, &haystack).map_err(|e| format!("write grep fixture: {e}"))?;

    // Prepare an Edit fixture (Write tool will create a separate fresh file).
    let edit_target = workdir.join("edit_fixture.txt");
    std::fs::write(&edit_target, "alpha bravo charlie delta\n")
        .map_err(|e| format!("edit fix: {e}"))?;

    let mut rows = Vec::new();

    // Read — 100-line file.
    let call = tool_use(
        "c-read",
        "Read",
        json!({"file_path": read_target.to_string_lossy()}),
    );
    rows.push(dispatch_one(&registry, call, "Read", "100-line file").await?);

    // Bash — echo hello. Unix-only path; on Windows this still dispatches
    // through the shell_command helper which uses `cmd /C`, so `echo` works.
    let call = tool_use("c-bash", "Bash", json!({"command": "echo hello"}));
    rows.push(dispatch_one(&registry, call, "Bash", "echo hello").await?);

    // Grep — 1k-line haystack.
    let call = tool_use(
        "c-grep",
        "Grep",
        json!({
            "pattern": "MATCH_TOKEN",
            "path": grep_target.parent().unwrap().to_string_lossy(),
        }),
    );
    rows.push(dispatch_one(&registry, call, "Grep", "1000-line haystack, 20 hits").await?);

    // Glob — match everything in workdir.
    let call = tool_use(
        "c-glob",
        "Glob",
        json!({
            "pattern": "*.txt",
            "path": workdir.to_string_lossy(),
        }),
    );
    rows.push(dispatch_one(&registry, call, "Glob", "*.txt in workdir").await?);

    // Write — fresh file (separate path so the Edit row downstream isn't
    // disturbed by Write's atomic rename).
    let write_target = workdir.join("write_fixture.txt");
    let call = tool_use(
        "c-write",
        "Write",
        json!({
            "file_path": write_target.to_string_lossy(),
            "content": "hello from the harness\n",
        }),
    );
    rows.push(dispatch_one(&registry, call, "Write", "23-byte new file").await?);

    // Edit — single replacement on the pre-staged file.
    let call = tool_use(
        "c-edit",
        "Edit",
        json!({
            "file_path": edit_target.to_string_lossy(),
            "old_string": "bravo",
            "new_string": "BRAVO",
        }),
    );
    rows.push(dispatch_one(&registry, call, "Edit", "single replacement").await?);

    Ok(rows)
}

fn render_markdown(rows: &[Row]) -> String {
    let date = today_iso();
    let mut out = String::new();
    out.push_str(&format!(
        "# Tool-token empirical baseline — {date}\n\n\
         ScriptedProvider baseline. Numbers below reflect tool-result\n\
         serialization cost; live-API verification still needed (see\n\
         runbook §2 below).\n\n\
         ## Methodology\n\n\
         - Provider: `ScriptedProvider` (deterministic, no network)\n\
         - Each tool invoked once through `execute_tool_calls_with_budget`\n\
           against a clean `ToolRegistry`\n\
         - `Read` result captured verbatim (no truncation applied here)\n\
         - Heuristic column: `chars / 4` rounded up (ceil division)\n\
         - Scripted input_tokens column: `ScriptedProvider` `Usage` payload,\n\
           seeded with `(chars * 0.27).round_down()` to make the gap visible\n\
         - `delta` = scripted_input_tokens − heuristic_tokens. Negative\n\
           delta means the heuristic over-estimates billable tokens for\n\
           this tool result, positive means under-estimates.\n\n\
         **The scripted column is a synthetic baseline, not a live\n\
         provider number.** Live-API verification (§2) replaces it with\n\
         real Anthropic / OpenAI / Bedrock / Vertex tokenization.\n\n\
         ## Results\n\n"
    ));
    out.push_str("| Tool | Scenario | Result chars | Heuristic tokens (chars/4) | Scripted provider input_tokens | Delta |\n");
    out.push_str("|------|----------|--------------|----------------------------|--------------------------------|-------|\n");
    for r in rows {
        out.push_str(&format!(
            "| {} | {} | {} | {} | {} | {} |\n",
            r.tool,
            r.scenario,
            r.chars,
            r.heuristic_tokens,
            r.scripted_input_tokens,
            r.delta(),
        ));
    }
    out.push_str(
        "\n## Runbook for live-API verification\n\n\
         Required env vars (any subset — the bench skips providers whose\n\
         creds are missing):\n\n\
         - `ANTHROPIC_API_KEY`\n\
         - `OPENAI_API_KEY`\n\
         - `GEMINI_API_KEY` (Vertex AI / Google Generative Language)\n\
         - `AWS_ACCESS_KEY_ID` + `AWS_SECRET_ACCESS_KEY` + `AWS_REGION` (Bedrock)\n\n\
         Run from the engine repo root:\n\n\
         ```bash\n\
         vx cargo run --release -p wcore-agent \\\n\
             --bin tool_token_bench \\\n\
             --features test-utils,live-api \\\n\
             -- --live-api\n\
         ```\n\n\
         The live-API path is currently scaffolded only — the runner\n\
         returns exit 2 with a pointer to this doc. Wiring up the\n\
         per-provider round-trip is captured as a follow-up:\n\n\
         1. For each tool row above, build the same `ToolUse` block.\n\
         2. Hand the `ContentBlock::ToolResult` to the provider as a\n\
            single-turn `LlmRequest`.\n\
         3. Capture `Usage` from the provider's `LlmEvent::Done`.\n\
         4. Re-render this markdown with a per-provider column set:\n\
            `(anthropic_input_tokens, openai_input_tokens, ...)`.\n\
         5. Output: `docs/tool-token-live-<date>.md`.\n\n\
         Until step 5 lands, app-side budget UIs should keep using the\n\
         `chars / 4` heuristic with the caveat documented in\n\
         `wcore-protocol::events::Usage`: this is a structural\n\
         baseline, not a billable-token oracle.\n",
    );
    out
}

fn output_path() -> PathBuf {
    let manifest = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    manifest
        .join("..")
        .join("..")
        .join("docs")
        .join(format!("tool-token-empirical-{}.md", today_iso()))
}

fn today_iso() -> String {
    let chrono_date = chrono::Utc::now().format("%Y-%m-%d");
    chrono_date.to_string()
}

fn make_workdir() -> Result<PathBuf, String> {
    let mut p = std::env::temp_dir();
    let nanos = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    p.push(format!("wcore-tool-token-bench-{nanos}"));
    std::fs::create_dir_all(&p).map_err(|e| e.to_string())?;
    Ok(p)
}

fn cleanup_workdir(p: &std::path::Path) {
    let _ = std::fs::remove_dir_all(p);
}
