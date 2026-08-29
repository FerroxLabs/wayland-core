//! FerroxLabs/wayland#174 c2-c5 — the enforcement side, graded at the CALL
//! SITES rather than at the guard function.
//!
//! A guard that is fully unit-tested and wired into only some of its call
//! sites has shipped in this repository before, so this file leads with a
//! census: it enumerates every place in the workspace that can hand an
//! `LlmRequest` to a provider, and fails if a new one appears without being
//! named and assigned a guard. The behavioural tests below then drive the real
//! `AgentEngine` through the two that the criteria turn on.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use async_trait::async_trait;
use tempfile::TempDir;
use tokio::sync::mpsc;
use wcore_agent::engine::AgentEngine;
use wcore_agent::output::OutputSink;
use wcore_agent::output::null_sink::NullSink;
use wcore_budget::{BudgetConfig, SpendMode};
use wcore_config::compat::ProviderCompat;
use wcore_config::config::Config;
use wcore_providers::{LlmProvider, ProviderError};
use wcore_tools::registry::ToolRegistry;
use wcore_types::llm::{LlmEvent, LlmRequest};
use wcore_types::message::{FinishReason, StopReason, TokenUsage};

// ── The census ──────────────────────────────────────────────────────────────

/// Every production `LlmProvider::stream` call site in the workspace, as
/// `<crate>/<crate-relative path>::<receiver expression>`, grouped by the guard
/// that covers it. A site that appears here without being in one of these three
/// groups is a hole, and a site that appears in the tree without being here at
/// all fails this test.
///
/// **Group 1 — covered by the `SpendGuardProvider` decorator.** All three
/// dispatch through a handle cloned from `AgentEngine::provider`, which
/// `install_spend_guard` makes a `SpendGuardProvider`: the conversation turn,
/// the autocompact summarization call, and the online-evolution paraphrase.
///
/// **Group 2 — decorators and transports that cannot change provider OR
/// model.** A journal wrapper, the guard's own pass-through, and the sixteen
/// concrete providers that delegate their wire work to
/// `OpenAICompatibleProvider`. Each forwards the request it was handed, so it
/// inherits whatever admitted that request. Nothing here needs its own gate,
/// and if one of them ever starts rewriting `request.model` this list is where
/// that becomes visible.
///
/// **Group 3 — the sites that DO change provider and model, inside their own
/// `stream()`, below the engine's guarded handle.** `ResilientProvider`'s
/// configured fallback and `ProviderChain`'s next slot. The decorator cannot
/// see either, so both are gated where they both funnel:
/// `retry::admit_configured_fallback`, whose admitter the engine installs with
/// a `SpendGuard::admit` call.
const EXPECTED_DISPATCH_SITES: &[&str] = &[
    // Group 1
    "wcore-agent/src/compact/auto.rs::provider",
    "wcore-agent/src/engine.rs::attempt_provider",
    "wcore-agent/src/engine.rs::provider",
    "wcore-evolve/src/mutator/llm_paraphrase_provider.rs::self.provider",
    // Group 2
    "wcore-agent/src/journal_provider.rs::self.inner",
    "wcore-agent/src/spend_guard.rs::self.inner",
    "wcore-providers/src/cerebras.rs::self.inner",
    "wcore-providers/src/deepseek.rs::self.inner",
    "wcore-providers/src/fireworks.rs::self.inner",
    "wcore-providers/src/flux_router.rs::self.inner",
    "wcore-providers/src/groq.rs::self.inner",
    "wcore-providers/src/mistral.rs::self.inner",
    "wcore-providers/src/moonshot.rs::self.inner",
    "wcore-providers/src/nvidia.rs::self.inner",
    "wcore-providers/src/openai_compatible.rs::self.inner",
    "wcore-providers/src/openrouter.rs::self.inner",
    "wcore-providers/src/perplexity.rs::self.inner",
    "wcore-providers/src/qwen.rs::self.inner",
    "wcore-providers/src/sakana.rs::self.inner",
    "wcore-providers/src/together.rs::self.inner",
    "wcore-providers/src/xai.rs::self.inner",
    "wcore-providers/src/resilient.rs::self.primary",
    // Group 3
    "wcore-providers/src/chain.rs::slot.provider",
    "wcore-providers/src/resilient.rs::fallback.provider",
];

fn workspace_root() -> PathBuf {
    // CARGO_MANIFEST_DIR is `<root>/crates/wcore-agent`.
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .expect("workspace root above crates/wcore-agent")
        .to_path_buf()
}

/// Byte ranges of every `#[cfg(test)]` item in `src`, by brace matching from
/// the first `{` after the attribute.
fn cfg_test_ranges(src: &str) -> Vec<(usize, usize)> {
    let bytes = src.as_bytes();
    let mut ranges = Vec::new();
    let mut search = 0usize;
    while let Some(found) = src[search..].find("#[cfg(test)]") {
        let at = search + found;
        search = at + "#[cfg(test)]".len();
        let Some(open_offset) = src[search..].find('{') else {
            break;
        };
        let open = search + open_offset;
        let mut depth = 0i32;
        let mut idx = open;
        while idx < bytes.len() {
            match bytes[idx] {
                b'{' => depth += 1,
                b'}' => {
                    depth -= 1;
                    if depth == 0 {
                        break;
                    }
                }
                _ => {}
            }
            idx += 1;
        }
        ranges.push((at, idx.min(bytes.len())));
        search = idx.min(bytes.len());
    }
    ranges
}

fn collect_rs_files(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            collect_rs_files(&path, out);
        } else if path.extension().is_some_and(|e| e == "rs") {
            out.push(path);
        }
    }
}

/// Receiver expression of a `<recv>.stream(` call ending at `dot`.
fn receiver_before(src: &str, dot: usize) -> Option<String> {
    let bytes = src.as_bytes();
    let mut start = dot;
    while start > 0 {
        let c = bytes[start - 1];
        if c.is_ascii_alphanumeric() || c == b'_' || c == b'.' {
            start -= 1;
        } else {
            break;
        }
    }
    if start == dot {
        return None;
    }
    Some(src[start..dot].to_string())
}

#[test]
fn every_production_provider_dispatch_site_is_named_and_assigned_a_guard() {
    let root = workspace_root();
    let crates_dir = root.join("crates");
    let mut found: BTreeSet<String> = BTreeSet::new();

    let mut crate_dirs: Vec<PathBuf> = std::fs::read_dir(&crates_dir)
        .expect("crates/ is readable")
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.is_dir())
        .collect();
    crate_dirs.sort();
    assert!(
        crate_dirs.len() > 20,
        "the census must actually walk the workspace; found {} crates",
        crate_dirs.len()
    );

    for crate_dir in crate_dirs {
        let crate_name = crate_dir
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or_default()
            .to_string();
        let src = crate_dir.join("src");
        if !src.is_dir() {
            continue;
        }
        let mut files = Vec::new();
        collect_rs_files(&src, &mut files);
        files.sort();
        for file in files {
            let text = std::fs::read_to_string(&file).expect("source file is readable");
            // A file named `tests.rs` or living under a `tests` module dir is
            // test code even without the attribute.
            let rel = file
                .strip_prefix(&crate_dir)
                .expect("file under its crate")
                .to_string_lossy()
                .replace('\\', "/");
            if rel.ends_with("/tests.rs") || rel.contains("/test_utils/") || rel.contains("/bin/") {
                continue;
            }
            let skip = cfg_test_ranges(&text);
            let mut search = 0usize;
            while let Some(offset) = text[search..].find(".stream(") {
                let dot = search + offset;
                search = dot + ".stream(".len();
                if skip.iter().any(|(lo, hi)| dot >= *lo && dot <= *hi) {
                    continue;
                }
                let Some(receiver) = receiver_before(&text, dot) else {
                    continue;
                };
                found.insert(format!("{crate_name}/{rel}::{receiver}"));
            }
        }
    }

    let expected: BTreeSet<String> = EXPECTED_DISPATCH_SITES
        .iter()
        .map(|s| (*s).to_string())
        .collect();

    // Positive control: the census must actually be finding things. A silently
    // broken walk would produce an empty set that matched nothing and passed
    // by vacuity if `expected` were ever emptied too.
    assert!(
        found.contains("wcore-agent/src/engine.rs::attempt_provider"),
        "census found nothing at the engine's own dispatch site; the walk is broken. \
         found = {found:#?}"
    );

    let unexpected: Vec<&String> = found.difference(&expected).collect();
    assert!(
        unexpected.is_empty(),
        "a NEW provider dispatch site appeared and has not been assigned a spend guard. \
         Gate it (or wrap its provider handle) and add it to EXPECTED_DISPATCH_SITES with \
         the guard that covers it: {unexpected:#?}"
    );
    let vanished: Vec<&String> = expected.difference(&found).collect();
    assert!(
        vanished.is_empty(),
        "a listed dispatch site no longer exists; drop it from EXPECTED_DISPATCH_SITES so \
         the list keeps meaning something: {vanished:#?}"
    );
}

// ── Behavioural: the engine really refuses ──────────────────────────────────

struct CountingProvider {
    calls: Arc<AtomicUsize>,
}

#[async_trait]
impl LlmProvider for CountingProvider {
    async fn stream(&self, _: &LlmRequest) -> Result<mpsc::Receiver<LlmEvent>, ProviderError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        let (tx, rx) = mpsc::channel(2);
        tokio::spawn(async move {
            let _ = tx.send(LlmEvent::TextDelta("ok".into())).await;
            let _ = tx
                .send(LlmEvent::Done {
                    stop_reason: StopReason::EndTurn,
                    finish_reason: FinishReason::Stop,
                    usage: TokenUsage {
                        input_tokens: 12,
                        output_tokens: 3,
                        ..Default::default()
                    },
                })
                .await;
        });
        Ok(rx)
    }
}

fn null_output() -> Arc<dyn OutputSink> {
    Arc::new(NullSink)
}

fn config_with_mode(model: &str, mode: Option<SpendMode>) -> Config {
    Config {
        model: model.into(),
        max_tokens: 64,
        max_turns: Some(1),
        compat: ProviderCompat::anthropic_defaults(),
        budget: BudgetConfig {
            mode,
            ..Default::default()
        },
        ..Default::default()
    }
}

/// Point `wayland_config_dir()` at a temp root so the audit log this test
/// reads back is this test's own.
struct HomeGuard {
    previous: Option<String>,
    _dir: TempDir,
    root: PathBuf,
}

impl HomeGuard {
    fn new() -> Self {
        let dir = TempDir::new().expect("temp wayland home");
        let root = dir.path().to_path_buf();
        let previous = std::env::var("WAYLAND_HOME").ok();
        // SAFETY: every test in this binary that touches WAYLAND_HOME is
        // `#[serial]`, and Drop restores the previous value on panic too.
        unsafe { std::env::set_var("WAYLAND_HOME", &root) };
        Self {
            previous,
            _dir: dir,
            root,
        }
    }

    fn audit_lines(&self) -> Vec<serde_json::Value> {
        let path = self.root.join("budget").join("spend-audit.jsonl");
        let Ok(raw) = std::fs::read_to_string(&path) else {
            return Vec::new();
        };
        raw.lines()
            .filter(|l| !l.trim().is_empty())
            .map(|l| serde_json::from_str(l).expect("each audit line is JSON"))
            .collect()
    }
}

impl Drop for HomeGuard {
    fn drop(&mut self) {
        // SAFETY: see `HomeGuard::new`.
        match &self.previous {
            Some(value) => unsafe { std::env::set_var("WAYLAND_HOME", value) },
            None => unsafe { std::env::remove_var("WAYLAND_HOME") },
        }
    }
}

#[tokio::test]
#[serial_test::serial]
async fn local_only_stops_a_hosted_model_before_the_provider_is_reached() {
    let home = HomeGuard::new();
    let calls = Arc::new(AtomicUsize::new(0));
    let provider: Arc<dyn LlmProvider> = Arc::new(CountingProvider {
        calls: Arc::clone(&calls),
    });
    let config = config_with_mode(
        wcore_types::model_aliases::ANTHROPIC_SONNET,
        Some(SpendMode::LocalOnly),
    );
    let mut engine =
        AgentEngine::new_with_provider(provider, config, ToolRegistry::new(), null_output());

    let _ = engine.run("hello", "m1").await;

    assert_eq!(
        calls.load(Ordering::SeqCst),
        0,
        "local-only must refuse a hosted model BEFORE the provider is reached; \
         an advisory mode would have let this through"
    );

    let records: Vec<_> = home
        .audit_lines()
        .into_iter()
        .filter(|v| v["kind"] == "task_spend_audit")
        .collect();
    assert_eq!(records.len(), 1, "one task, one audit record: {records:#?}");
    let refusals = records[0]["payload"]["refusals"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    assert_eq!(
        refusals.len(),
        1,
        "the refusal must be on the record: {:#?}",
        records[0]
    );
    assert_eq!(refusals[0]["kind"], "remote_model_refused");
    assert_eq!(records[0]["payload"]["mode"], "local-only");
}

#[tokio::test]
#[serial_test::serial]
async fn a_local_model_still_runs_under_local_only_and_is_audited() {
    let home = HomeGuard::new();
    let calls = Arc::new(AtomicUsize::new(0));
    let provider: Arc<dyn LlmProvider> = Arc::new(CountingProvider {
        calls: Arc::clone(&calls),
    });
    let config = config_with_mode("ollama:qwen3-coder:30b", Some(SpendMode::LocalOnly));
    let mut engine =
        AgentEngine::new_with_provider(provider, config, ToolRegistry::new(), null_output());

    let _ = engine.run("hello", "m1").await;

    assert_eq!(
        calls.load(Ordering::SeqCst),
        1,
        "local-only forbids SPEND, not work: a local model must still dispatch"
    );
    let records: Vec<_> = home
        .audit_lines()
        .into_iter()
        .filter(|v| v["kind"] == "task_spend_audit")
        .collect();
    assert_eq!(records.len(), 1);
    assert!(
        records[0]["payload"]["refusals"]
            .as_array()
            .is_some_and(|r| r.is_empty()),
        "an admitted dispatch records no refusal: {:#?}",
        records[0]
    );
}

#[tokio::test]
#[serial_test::serial]
async fn every_task_writes_its_own_audit_record_even_with_no_mode_configured() {
    let home = HomeGuard::new();
    let calls = Arc::new(AtomicUsize::new(0));
    let provider: Arc<dyn LlmProvider> = Arc::new(CountingProvider {
        calls: Arc::clone(&calls),
    });
    // No `[budget] mode` at all — the audit is unconditional, the modes are not.
    let config = config_with_mode(wcore_types::model_aliases::ANTHROPIC_SONNET, None);
    let mut engine =
        AgentEngine::new_with_provider(provider, config, ToolRegistry::new(), null_output());

    let _ = engine.run("first", "m1").await;
    let _ = engine.run("second", "m2").await;

    let records: Vec<_> = home
        .audit_lines()
        .into_iter()
        .filter(|v| v["kind"] == "task_spend_audit")
        .collect();
    assert_eq!(
        records.len(),
        2,
        "two tasks must produce two records, not one rolling total: {records:#?}"
    );
    let first_id = records[0]["payload"]["task_id"]
        .as_str()
        .unwrap_or_default();
    let second_id = records[1]["payload"]["task_id"]
        .as_str()
        .unwrap_or_default();
    assert_ne!(first_id, second_id, "each task gets its own id");
    assert_eq!(records[0]["payload"]["mode"], "unrestricted");
    // The dispatch actually happened, so the record must carry it.
    assert_eq!(
        records[0]["payload"]["dispatches"]
            .as_array()
            .map(Vec::len)
            .unwrap_or(0),
        1,
        "the settled provider dispatch must be charged to the task record: {:#?}",
        records[0]
    );
}

#[tokio::test]
#[serial_test::serial]
async fn an_operator_model_change_is_recorded_and_a_forbidden_one_is_refused() {
    let home = HomeGuard::new();
    let calls = Arc::new(AtomicUsize::new(0));
    let provider: Arc<dyn LlmProvider> = Arc::new(CountingProvider {
        calls: Arc::clone(&calls),
    });
    let config = config_with_mode(wcore_types::model_aliases::ANTHROPIC_HAIKU, None);
    let mut engine =
        AgentEngine::new_with_provider(provider, config, ToolRegistry::new(), null_output());

    engine.set_model(wcore_types::model_aliases::ANTHROPIC_OPUS);
    assert_eq!(engine.model(), wcore_types::model_aliases::ANTHROPIC_OPUS);

    let escalations: Vec<_> = home
        .audit_lines()
        .into_iter()
        .filter(|v| v["kind"] == "model_escalation")
        .collect();
    assert_eq!(
        escalations.len(),
        1,
        "an operator escalation is not silent, but it must still be recorded: {escalations:#?}"
    );
    assert_eq!(escalations[0]["payload"]["source"], "operator");
    assert!(
        escalations[0]["payload"]["reason"]
            .as_str()
            .is_some_and(|r| !r.trim().is_empty()),
        "the recorded reason must not be blank"
    );
    let _ = calls;
}

#[tokio::test]
#[serial_test::serial]
async fn no_paid_mode_refuses_a_metered_model_at_the_engine() {
    let home = HomeGuard::new();
    let calls = Arc::new(AtomicUsize::new(0));
    let provider: Arc<dyn LlmProvider> = Arc::new(CountingProvider {
        calls: Arc::clone(&calls),
    });
    let config = config_with_mode(
        wcore_types::model_aliases::ANTHROPIC_SONNET,
        Some(SpendMode::NoPaid),
    );
    let mut engine =
        AgentEngine::new_with_provider(provider, config, ToolRegistry::new(), null_output());

    let _ = engine.run("hello", "m1").await;

    assert_eq!(calls.load(Ordering::SeqCst), 0);
    let records: Vec<_> = home
        .audit_lines()
        .into_iter()
        .filter(|v| v["kind"] == "task_spend_audit")
        .collect();
    assert_eq!(records.len(), 1);
    assert_eq!(
        records[0]["payload"]["refusals"][0]["kind"],
        "paid_model_refused"
    );
}
