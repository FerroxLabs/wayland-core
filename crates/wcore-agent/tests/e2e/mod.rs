/// End-to-end tests that hit real LLM provider APIs.
///
/// These tests are skipped when the required environment variable is absent,
/// making them safe to compile and run in any environment while still providing
/// full coverage in CI when secrets are available.
///
/// Required env vars (at least one):
///   ANTHROPIC_API_KEY — runs Anthropic provider tests
///   OPENAI_API_KEY    — runs OpenAI provider tests
///
/// Run manually:
///   ANTHROPIC_API_KEY=sk-ant-... cargo test -p wcore-agent --test e2e -- --nocapture
// The e2e and acceptance test binaries each have their own crate root
// (Cargo.toml `[[test]] path = "tests/e2e/mod.rs"`), so the standard
// `mod common;` resolves to `tests/e2e/common.rs`. Re-host the shared
// common module via `#[path]` so the same source compiles into both
// binaries without duplication.
#[path = "../common/mod.rs"]
mod common;

mod anthropic;
mod compaction;
mod openai;
