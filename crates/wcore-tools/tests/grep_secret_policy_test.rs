//! SR-04 / SR-05 — Grep must own its ignore policy and its secret policy.
//!
//! MEASURED DEFECT (foundation W2/W3 gate, commit d6f76c67): Grep tried
//! `rg` and fell back to a bare `grep -rn`. Neither the fallback nor the
//! Windows `findstr` arm had ANY ignore or secret handling, so on a host
//! without ripgrep the `.env` canary `ZORVAX-7731` reached the provider
//! VERBATIM — the recorded request body carried the literal tool result
//! `.env:1:SECRET=ZORVAX-7731`. `which rg` on the Linux build host: NOT
//! FOUND. On the Mac: `/opt/homebrew/bin/rg`. So the macOS SR-04/SR-05
//! passes were Homebrew ripgrep's default hidden-file skipping, not a
//! product guarantee.
//!
//! Note the second mechanism, which is why the engine's central
//! `redact_tool_output` (wcore-agent) did not save us: the
//! `SECRET_ASSIGNMENT` rule in `wcore-safety` is `(?im)^\s*...`-anchored,
//! and Grep prefixes every hit with `path:lineno:`. The prefix pushes the
//! assignment off the start of the line, so the line-anchored rule cannot
//! match and the secret sails through the one guard that was supposed to
//! catch it.
//!
//! Every refusal assertion below is paired with a POSITIVE CONTROL that must
//! still be returned, so a Grep that broke entirely cannot pass this file.

#![cfg(unix)]

use std::path::{Path, PathBuf};

use serde_json::json;
use serial_test::serial;
use wcore_tools::Tool;
use wcore_tools::context::ToolContext;
use wcore_tools::grep::GrepTool;

const CANARY: &str = "ZORVAX-7731";
/// Present in an ORDINARY file. Must survive every filter below.
const CONTROL: &str = "ZORVAX-CONTROL-OK";

/// A PATH that contains the system `grep` but provably NOT `rg`, so the
/// fallback backend is the one under test. Asserting the absence is the point:
/// if a host ever put `rg` in `/usr/bin`, this test must fail loudly rather
/// than quietly grade the ripgrep path instead.
fn path_without_ripgrep() -> String {
    let path = "/usr/bin:/bin".to_string();
    assert!(
        !resolves_on(&path, "rg"),
        "this test requires ripgrep to be ABSENT from {path}; it is present, \
         so the grep fallback would not be exercised"
    );
    assert!(
        resolves_on(&path, "grep"),
        "this test requires the system grep on {path}"
    );
    path
}

fn resolves_on(path: &str, program: &str) -> bool {
    path.split(':')
        .any(|dir| Path::new(dir).join(program).exists())
}

/// RAII PATH override. Process-global, hence `#[serial]` on every user.
struct PathGuard(Option<String>);

impl PathGuard {
    fn set(value: &str) -> Self {
        let prev = std::env::var("PATH").ok();
        // SAFETY: single-threaded section — every caller is `#[serial]`, so no
        // other test in this binary is running while the variable is swapped.
        unsafe { std::env::set_var("PATH", value) };
        Self(prev)
    }
}

impl Drop for PathGuard {
    fn drop(&mut self) {
        // SAFETY: see `set`.
        match self.0.take() {
            Some(prev) => unsafe { std::env::set_var("PATH", prev) },
            None => unsafe { std::env::remove_var("PATH") },
        }
    }
}

/// A directory holding a scripted `rg` that reports EVERYTHING, including the
/// dotfile a real ripgrep would skip. The product policy — not the backend's
/// defaults — is what must withhold the secret, so the backend is pinned to a
/// known-permissive one rather than to whatever ripgrep the host happens to
/// have.
fn fake_ripgrep_dir(lines: &[String]) -> tempfile::TempDir {
    use std::os::unix::fs::PermissionsExt;

    let dir = tempfile::tempdir().expect("tempdir");
    let rg = dir.path().join("rg");
    let body = lines
        .iter()
        .map(|l| format!("printf '%s\\n' {}\n", shell_single_quote(l)))
        .collect::<String>();
    std::fs::write(&rg, format!("#!/bin/sh\n{body}exit 0\n")).expect("write fake rg");
    std::fs::set_permissions(&rg, std::fs::Permissions::from_mode(0o755)).expect("chmod fake rg");
    dir
}

fn shell_single_quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', r"'\''"))
}

/// A workspace with a secret dotfile and an ordinary file, both matching the
/// same pattern.
fn seed_workspace() -> tempfile::TempDir {
    let dir = tempfile::tempdir().expect("tempdir");
    std::fs::write(dir.path().join(".env"), format!("SECRET={CANARY}\n")).expect("write .env");
    std::fs::write(
        dir.path().join("notes.txt"),
        format!("plain line {CONTROL}\n"),
    )
    .expect("write notes.txt");
    dir
}

fn work(dir: &tempfile::TempDir) -> PathBuf {
    dir.path().to_path_buf()
}

// ---------------------------------------------------------------------------
// SR-05 — the secret policy
// ---------------------------------------------------------------------------

/// THE MEASURED DEFECT. Ripgrep forcibly absent, so the `grep -rn` fallback
/// runs — the path that has no ignore or secret handling at all.
#[tokio::test]
#[serial]
async fn grep_fallback_without_ripgrep_never_returns_dot_env_contents() {
    let dir = seed_workspace();
    let _path = PathGuard::set(&path_without_ripgrep());

    let ctx = ToolContext::test_default();
    let result = GrepTool
        .execute_with_ctx(
            json!({ "pattern": "ZORVAX", "path": work(&dir).to_string_lossy() }),
            &ctx,
        )
        .await;

    // POSITIVE CONTROL — an ordinary file's match must still come back, so a
    // Grep that returned nothing at all cannot satisfy the assertion below.
    assert!(
        result.content.contains(CONTROL),
        "positive control missing: an ordinary file's match must still be \
         returned, got: {}",
        result.content
    );
    assert!(
        !result.content.contains(CANARY),
        "the .env canary reached the tool result via the grep fallback: {}",
        result.content
    );
}

/// The same property with a ripgrep ON PATH. The backend here is scripted to
/// report the dotfile, because the guarantee must be the product's, not
/// whichever ripgrep the host installed.
#[tokio::test]
#[serial]
async fn grep_with_ripgrep_on_path_never_returns_dot_env_contents() {
    let dir = seed_workspace();
    let env_line = format!("{}:1:SECRET={CANARY}", dir.path().join(".env").display());
    let ok_line = format!(
        "{}:1:plain line {CONTROL}",
        dir.path().join("notes.txt").display()
    );
    let bin = fake_ripgrep_dir(&[env_line, ok_line]);
    let _path = PathGuard::set(&format!(
        "{}:{}",
        bin.path().display(),
        path_without_ripgrep()
    ));

    let ctx = ToolContext::test_default();
    let result = GrepTool
        .execute_with_ctx(
            json!({ "pattern": "ZORVAX", "path": work(&dir).to_string_lossy() }),
            &ctx,
        )
        .await;

    assert!(
        result.content.contains(CONTROL),
        "positive control missing on the ripgrep path, got: {}",
        result.content
    );
    assert!(
        !result.content.contains(CANARY),
        "the .env canary reached the tool result via the ripgrep path: {}",
        result.content
    );
}

/// Naming the secret file outright must be refused, not served. Grep returns
/// matched line CONTENT, so an explicit `.env` target is the shortest exfil
/// path there is.
///
/// The CASE VARIANTS are the point of the second and third rows. Grep resolves
/// its target with `validate_search_root`, which normalizes LEXICALLY and does
/// NOT `canonicalize`, so the spelling the caller typed is the spelling the
/// denylist sees. On macOS and Windows the filesystem is case-insensitive, so
/// `.ENV` opens the same bytes as `.env` while a case-SENSITIVE denylist waves
/// it through — an agent that picks the spelling gets the secret with no grant
/// and no card. The lowercase row is the known-positive control: if it ever
/// stops refusing, this test is broken rather than the product.
#[tokio::test]
#[serial]
async fn grep_refuses_an_explicitly_named_secret_file() {
    let dir = seed_workspace();
    // On a case-insensitive filesystem these are all the same file; on Linux
    // they are three, each seeded with the canary so a leak is visible either
    // way. The body is deliberately NOT `KEY=value` shaped: when this was
    // first run against the unfixed guard, `.ENV` WAS searched and the only
    // reason the canary did not appear verbatim was the engine's downstream
    // `SECRET_ASSIGNMENT` redactor. That second layer catches one shape; a PEM
    // body or a bare token in a `.PEM`/`ID_RSA` spelled the same way is not an
    // assignment and comes back whole. Seeding a plain line puts the refusal
    // itself under test instead of its backstop.
    for spelling in [".ENV", ".Env"] {
        std::fs::write(dir.path().join(spelling), format!("plain line {CANARY}\n"))
            .expect("write case variant");
    }
    let _path = PathGuard::set(&path_without_ripgrep());

    for spelling in [".env", ".ENV", ".Env"] {
        let ctx = ToolContext::test_default();
        let result = GrepTool
            .execute_with_ctx(
                json!({
                    "pattern": "ZORVAX",
                    "path": dir.path().join(spelling).to_string_lossy(),
                }),
                &ctx,
            )
            .await;

        assert!(
            !result.content.contains(CANARY),
            "an explicitly named {spelling} must never return its contents, \
             got: {}",
            result.content
        );
        assert!(
            result.is_error,
            "an explicitly named secret file ({spelling}) must be refused \
             loudly, got: {}",
            result.content
        );
    }
}

/// The second mechanism: a secret in an ORDINARY file. The engine's central
/// `redact_tool_output` cannot catch it because Grep's `path:lineno:` prefix
/// defeats the `^`-anchored `SECRET_ASSIGNMENT` rule, so Grep must scrub the
/// match content itself.
#[tokio::test]
#[serial]
async fn grep_redacts_a_secret_assignment_found_in_an_ordinary_file() {
    let dir = tempfile::tempdir().expect("tempdir");
    std::fs::write(
        dir.path().join("config.txt"),
        format!("API_KEY={CANARY}\nplain line {CONTROL}\n"),
    )
    .expect("write config.txt");
    let _path = PathGuard::set(&path_without_ripgrep());

    let ctx = ToolContext::test_default();
    let result = GrepTool
        .execute_with_ctx(
            json!({ "pattern": "ZORVAX", "path": work(&dir).to_string_lossy() }),
            &ctx,
        )
        .await;

    assert!(
        result.content.contains(CONTROL),
        "positive control missing: the non-secret match in the same file must \
         still be returned, got: {}",
        result.content
    );
    assert!(
        !result.content.contains(CANARY),
        "a secret assignment in an ordinary file must be redacted, got: {}",
        result.content
    );
}

// ---------------------------------------------------------------------------
// SR-04 — the ignore policy
// ---------------------------------------------------------------------------

/// A tree with a vendored, `.gitignore`d copy of the needle and a real one.
/// `repo` plants a `.git` directory, which is what turns `.gitignore` on.
fn seed_ignore_tree(repo: bool) -> tempfile::TempDir {
    let dir = tempfile::tempdir().expect("tempdir");
    std::fs::create_dir_all(dir.path().join("node_modules")).expect("mkdir node_modules");
    std::fs::create_dir_all(dir.path().join("src")).expect("mkdir src");
    std::fs::write(
        dir.path().join("node_modules").join("x.js"),
        "var ZORVAXNEEDLE = 1;\n",
    )
    .expect("write vendored");
    std::fs::write(
        dir.path().join("src").join("app.js"),
        "// ZORVAXNEEDLE real\n",
    )
    .expect("write src");
    std::fs::write(dir.path().join(".gitignore"), "node_modules/\n").expect("write .gitignore");
    if repo {
        std::fs::create_dir_all(dir.path().join(".git")).expect("mkdir .git");
    }
    dir
}

async fn grep_for_needle(dir: &tempfile::TempDir) -> String {
    let ctx = ToolContext::test_default();
    GrepTool
        .execute_with_ctx(
            json!({ "pattern": "ZORVAXNEEDLE", "path": work(dir).to_string_lossy() }),
            &ctx,
        )
        .await
        .content
}

/// Ripgrep skips `.gitignore`d paths; `grep -rn` and `findstr` do not. Inside a
/// repository the product must give ripgrep's answer on every backend.
#[tokio::test]
#[serial]
async fn grep_fallback_honours_gitignore_inside_a_repo_like_ripgrep_does() {
    let dir = seed_ignore_tree(true);
    let _path = PathGuard::set(&path_without_ripgrep());
    let content = grep_for_needle(&dir).await;

    assert!(
        content.contains("app.js"),
        "positive control missing: the non-ignored hit must be returned, got: {content}"
    );
    assert!(
        !content.contains("node_modules/x.js"),
        "a .gitignore'd path must be absent from the results, got: {content}"
    );
}

/// The ONE deliberate divergence from ripgrep, pinned so it cannot drift back
/// by accident. MEASURED on ripgrep 14 (`/opt/homebrew/bin/rg`): outside a git
/// repository `rg` does not read `.gitignore` at all and returns
/// `node_modules/x.js`. The product honours the file anyway — it is an explicit
/// statement of intent that should not wait on `git init` — and says so in the
/// footer rather than dropping the matches silently.
#[tokio::test]
#[serial]
async fn grep_honours_a_gitignore_outside_a_repo_and_says_that_it_did() {
    let dir = seed_ignore_tree(false);
    let _path = PathGuard::set(&path_without_ripgrep());
    let content = grep_for_needle(&dir).await;

    assert!(
        content.contains("app.js"),
        "positive control missing: the ordinary hit must be returned, got: {content}"
    );
    assert!(
        !content.contains("node_modules/x.js"),
        "an explicit .gitignore must be honoured even outside a repo, got: {content}"
    );
    assert!(
        content.contains("[Grep policy:") && content.contains("ignored paths"),
        "withholding must be reported, never silent — a caller told \
         'no matches' would act on it. got: {content}"
    );
}
