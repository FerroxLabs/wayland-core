//! USER-FLOW HARNESS — Layer 1: CLI surface.
//!
//! Every other test in the workspace drives `AgentEngine` in-process at
//! the library level. This layer drives the **compiled `wayland-core`
//! binary** as a subprocess the way a user's shell does: it asserts on
//! exit codes, stdout, and stderr for the non-interactive subcommands.
//! No PTY is needed — these subcommands never open the TUI.
//!
//! Reference pattern: `release_binary_smoke.rs` (spawns the binary,
//! captures output). Unlike that test, Layer 1 targets the **debug**
//! binary Cargo wires through `CARGO_BIN_EXE_wayland-core`, so it runs in
//! the default `cargo test` pass without a pre-built release artifact.
//!
//! Every test that touches config points `HOME` at a fresh tempdir, so
//! the real `~/Library/Application Support/wayland-core/config.toml`
//! (macOS) / `~/.config/wayland-core/config.toml` (Linux) is never read
//! or written. `wcore-config`'s `app_config_dir()` derives the path from
//! `dirs::config_dir()`, which is rooted at `$HOME` on both platforms.

use std::path::Path;
use std::process::{Command, Output};

use tempfile::TempDir;

/// Path to the debug binary under test. Cargo guarantees this is built
/// before the integration test runs.
fn binary() -> &'static str {
    env!("CARGO_BIN_EXE_wayland-core")
}

/// Run the binary with `args` and a tempdir `HOME`, so config CRUD is
/// fully isolated from the developer's real environment. Returns the
/// completed `Output` (status + stdout + stderr).
///
/// `WAYLAND_HOME` is the F-010 hermetic-sandbox env in `wcore-config`'s
/// `wayland_config_dir()` resolver — it overrides every config/auth path
/// on all three platforms in one shot. `HOME` alone is *not* enough on
/// Windows, where `dirs::config_dir()` resolves via `%APPDATA%`, not
/// `HOME` — so tests would write to the real `%APPDATA%\wayland-core\`
/// and pick up leaked state across nextest retries (round 11 caught
/// `auth add` reporting "Updated" instead of "Added" on retry, because
/// the first try left the key behind in the real APPDATA).
fn run_isolated(args: &[&str], home: &Path) -> Output {
    Command::new(binary())
        .args(args)
        // A scratch cwd too: `wayland-core` reads `.wayland-core.toml`
        // from cwd, and the repo root has none — but pointing cwd at the
        // tempdir removes any doubt.
        .current_dir(home)
        .env("HOME", home)
        .env("WAYLAND_HOME", home)
        // Keep the run hermetic: never pick up an ambient provider key.
        .env_remove("API_KEY")
        .env_remove("ANTHROPIC_API_KEY")
        .env_remove("OPENAI_API_KEY")
        .output()
        .unwrap_or_else(|e| panic!("failed to spawn {} {:?}: {e}", binary(), args))
}

/// Convenience: run with `args`, no HOME isolation needed (pure
/// argument-parsing subcommands like `--version` / `--help`).
fn run(args: &[&str]) -> Output {
    Command::new(binary())
        .args(args)
        .env_remove("API_KEY")
        .env_remove("ANTHROPIC_API_KEY")
        .env_remove("OPENAI_API_KEY")
        .output()
        .unwrap_or_else(|e| panic!("failed to spawn {} {:?}: {e}", binary(), args))
}

fn stdout_of(out: &Output) -> String {
    String::from_utf8_lossy(&out.stdout).into_owned()
}

fn stderr_of(out: &Output) -> String {
    String::from_utf8_lossy(&out.stderr).into_owned()
}

#[test]
fn version_reports_the_pinned_release() {
    let out = run(&["--version"]);
    assert!(
        out.status.success(),
        "--version must exit 0; status {:?}, stderr: {}",
        out.status,
        stderr_of(&out)
    );
    let stdout = stdout_of(&out);
    // The version string is the product's release identity — a regression
    // here means the published build claims the wrong version. Pin against
    // the crate's own `CARGO_PKG_VERSION` (workspace-inherited) rather than a
    // hardcoded literal, so a routine version bump can't silently rot this
    // test the way `0.8.1` did long after the workspace moved to 0.9.x.
    let expected = format!("wayland-core {}", env!("CARGO_PKG_VERSION"));
    assert!(
        stdout.contains(&expected),
        "--version must contain `{expected}`; got: {stdout:?}"
    );
}

#[test]
fn help_lists_the_setup_and_auth_subcommands() {
    let out = run(&["--help"]);
    assert!(
        out.status.success(),
        "--help must exit 0; status {:?}, stderr: {}",
        out.status,
        stderr_of(&out)
    );
    let stdout = stdout_of(&out);
    // The two CLI-surface subcommands a user reaches for first must be
    // discoverable from `--help`.
    assert!(
        stdout.contains("setup"),
        "--help must list the `setup` subcommand; got: {stdout}"
    );
    assert!(
        stdout.contains("auth"),
        "--help must list the `auth` subcommand; got: {stdout}"
    );
    // Sanity: the usage line is present so a user knows the invocation.
    assert!(
        stdout.contains("Usage: wayland-core"),
        "--help must print a usage line; got: {stdout}"
    );
}

#[test]
fn auth_list_on_a_fresh_config_reports_no_providers() {
    let home = TempDir::new().expect("create tempdir HOME");
    let out = run_isolated(&["auth", "list"], home.path());
    assert!(
        out.status.success(),
        "`auth list` on a fresh config must exit 0; status {:?}, stderr: {}",
        out.status,
        stderr_of(&out)
    );
    let stdout = stdout_of(&out);
    assert!(
        stdout.contains("No providers configured"),
        "`auth list` on a fresh config must say so; got: {stdout:?}"
    );
}

#[test]
fn auth_add_list_remove_is_a_full_crud_round_trip() {
    let home = TempDir::new().expect("create tempdir HOME");

    // --- ADD ---------------------------------------------------------
    // `--no-validate` skips the live endpoint check so the test never
    // touches the network. The key is long enough that `mask_key` keeps
    // a head + tail (short keys are fully masked).
    let api_key = "sk-ant-harness-abcdefghijklmnop";
    let add = run_isolated(
        &["auth", "add", "anthropic", api_key, "--no-validate"],
        home.path(),
    );
    assert!(
        add.status.success(),
        "`auth add` must exit 0; status {:?}, stderr: {}",
        add.status,
        stderr_of(&add)
    );
    assert!(
        stdout_of(&add).contains("Added API key for Anthropic"),
        "`auth add` must confirm the write; got: {}",
        stdout_of(&add)
    );

    // --- LIST shows the masked key -----------------------------------
    let list = run_isolated(&["auth", "list"], home.path());
    assert!(list.status.success(), "`auth list` after add must exit 0");
    let list_out = stdout_of(&list);
    assert!(
        list_out.contains("anthropic"),
        "`auth list` must show the configured provider; got: {list_out}"
    );
    // The key is masked: the head + tail survive, the middle is bullets,
    // and the secret middle ("harness") must NOT leak.
    assert!(
        list_out.contains("sk-a") && list_out.contains("mnop"),
        "`auth list` must show the masked head + tail; got: {list_out}"
    );
    assert!(
        !list_out.contains("harness"),
        "`auth list` leaked the key middle into plaintext: {list_out}"
    );

    // --- REMOVE ------------------------------------------------------
    let remove = run_isolated(&["auth", "remove", "anthropic"], home.path());
    assert!(
        remove.status.success(),
        "`auth remove` must exit 0; status {:?}, stderr: {}",
        remove.status,
        stderr_of(&remove)
    );
    assert!(
        stdout_of(&remove).contains("Removed API key for Anthropic"),
        "`auth remove` must confirm the deletion; got: {}",
        stdout_of(&remove)
    );

    // --- LIST is empty again — the round trip closed -----------------
    let list_again = run_isolated(&["auth", "list"], home.path());
    assert!(
        list_again.status.success(),
        "`auth list` after remove must exit 0"
    );
    assert!(
        stdout_of(&list_again).contains("No providers configured"),
        "`auth list` after remove must be empty again; got: {}",
        stdout_of(&list_again)
    );
}

#[test]
fn list_agents_prints_a_non_empty_roster() {
    let out = run(&["--list-agents"]);
    assert!(
        out.status.success(),
        "--list-agents must exit 0; status {:?}, stderr: {}",
        out.status,
        stderr_of(&out)
    );
    let stdout = stdout_of(&out);
    assert!(
        !stdout.trim().is_empty(),
        "--list-agents must print at least one built-in persona; got empty"
    );
    // The bundled agent pack ships these read-only personas; at least one
    // must appear so the test fails if the pack stops linking.
    assert!(
        stdout.contains("architect") || stdout.contains("debugger"),
        "--list-agents must name a known built-in persona; got: {stdout}"
    );
}

#[test]
fn an_unrecognized_argument_fails_with_a_usage_message() {
    // NOTE: a bare unknown WORD (e.g. `wayland-core notacommand`) is NOT
    // a clap error — the top-level `prompt` field is `trailing_var_arg`,
    // so an unknown word is swallowed as a prompt and the agent path is
    // entered. The genuine "unrecognized input" surface is an unknown
    // FLAG, which clap rejects with exit code 2 + a usage message. That
    // is what this test asserts.
    let out = run(&["--definitely-not-a-real-flag"]);
    assert!(
        !out.status.success(),
        "an unknown flag must fail; got success. stdout: {}",
        stdout_of(&out)
    );
    // clap exits with code 2 on an argument error.
    assert_eq!(
        out.status.code(),
        Some(2),
        "an unknown flag must exit with clap's code 2; got {:?}",
        out.status.code()
    );
    let stderr = stderr_of(&out);
    assert!(
        stderr.contains("error:") && stderr.contains("Usage: wayland-core"),
        "an unknown flag must print an error + usage line; got stderr: {stderr}"
    );
}

#[test]
fn an_unrecognized_subcommand_under_auth_fails_with_usage() {
    // `auth` IS a real `#[command(subcommand)]`, so an unknown word after
    // it is a true clap subcommand error — exit 2 + usage. This is the
    // closest the CLI gets to "unknown subcommand" with a non-zero exit.
    let home = TempDir::new().expect("create tempdir HOME");
    let out = run_isolated(&["auth", "not-a-subcommand"], home.path());
    assert!(
        !out.status.success(),
        "an unknown `auth` subcommand must fail; got success"
    );
    assert_eq!(
        out.status.code(),
        Some(2),
        "an unknown subcommand must exit with clap's code 2; got {:?}",
        out.status.code()
    );
    let stderr = stderr_of(&out);
    // Windows runs the binary as `wayland-core.exe`, so clap's usage
    // line reads `Usage: wayland-core.exe auth ...` — the exact-substring
    // `Usage: wayland-core auth` doesn't match (CI run 26405564483 job
    // 77727955376 — caught the only red on a 10-green ship). Check
    // each invariant separately to stay platform-agnostic.
    assert!(
        stderr.contains("unrecognized subcommand"),
        "stderr must surface the unknown subcommand error; got: {stderr}"
    );
    assert!(
        stderr.contains("Usage:") && stderr.contains("auth"),
        "stderr must include the auth-subcommand usage line; got: {stderr}"
    );
}
