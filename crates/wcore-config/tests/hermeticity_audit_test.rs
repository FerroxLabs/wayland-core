//! Hermeticity audit test (#270, F-010).
//!
//! Locks the gate behind `wayland_config_dir()`: no source file in the
//! workspace may call `dirs::config_dir()` directly, with the sole
//! exception of the canonical helper itself in
//! `crates/wcore-config/src/config.rs`. Without this test, the three
//! bypass sites we just fixed (`tui/frecency.rs`, `auto_memorize.rs`,
//! `debug_helpers.rs`) could silently re-appear via copy-paste of an
//! older pattern, breaking `WAYLAND_HOME` hermeticity for tests,
//! sandboxed runs, and second-user accounts.
//!
//! Scope: this test gates ONLY `dirs::config_dir()`. The
//! `dirs::home_dir()` landscape has many legitimate callers
//! (`~/.wayland/trusted-keys` for plugin signing, ADC credential paths,
//! cron sentinel fallbacks, etc.) and would require a much larger
//! allowlist; a separate audit is the right place to tackle it if/when
//! a similar gate is desired.
//!
//! Implementation: shells out to `git grep` (already on the dev-tool
//! path) so the test stays dependency-free and the regex stays simple.
//! If `git` is unavailable the test is skipped — no false negative, no
//! flake.

use std::path::PathBuf;
use std::process::Command;

/// Files allowed to call `dirs::config_dir()` directly. Everything else
/// must route through `wcore_config::config::wayland_config_dir()` so
/// `WAYLAND_HOME` hermetically sandboxes on-disk state (F-010).
const ALLOWLIST: &[&str] = &[
    // The canonical helper — this is the one legitimate call site, the
    // platform-native fallback inside `wayland_config_dir()` itself.
    "crates/wcore-config/src/config.rs",
    // S11 self-configure discovery (`/doctor` DISCOVERED): counts MCP servers
    // in the REAL machine's Claude-Desktop config at
    // `<config_dir>/Claude/claude_desktop_config.json`. This deliberately
    // probes the actual OS config dir (not `WAYLAND_HOME`) — routing it through
    // `wayland_config_dir()` would make it look in the sandbox and never find
    // the user's real Claude install. Read-only; writes no Wayland state.
    "crates/wcore-cli/src/tui/surfaces/diagnostics.rs",
    // Forge cross-app MCP discovery: the `<config_dir>/forge/mcp-servers.json`
    // file is a convention written by *other* Forge-suite apps (Agent Vault,
    // future Foundry tools) about the REAL machine — deliberately NOT
    // `WAYLAND_HOME`-scoped, exactly like the Claude-Desktop MCP discovery. It
    // is read-only and never writes Wayland state, so it does not break
    // hermeticity. See the module doc in `forge_discovery.rs` for the rationale.
    "crates/wcore-config/src/forge_discovery.rs",
];

fn workspace_root() -> PathBuf {
    let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    // `crates/wcore-config` → `<workspace>`
    manifest_dir
        .parent()
        .and_then(|p| p.parent())
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            panic!(
                "CARGO_MANIFEST_DIR ({}) has fewer than two ancestors; cannot locate workspace root",
                manifest_dir.display()
            )
        })
}

/// Lines from `git grep` that look like *call sites* of
/// `dirs::config_dir()` — i.e. the bare token followed by `(`, on a line
/// whose trimmed prefix is NOT a comment marker (`//`, `*`, `///`). This
/// filters out doc comments + module-comment cross-references that name
/// the function without actually invoking it.
fn call_site_hits(root: &PathBuf) -> Option<Vec<String>> {
    // `git grep -nE` — line numbers + extended regex. The pattern matches
    // the call form `dirs::config_dir(`, which is what we want to ban.
    // We restrict to `crates/` so this test ignores top-level docs.
    let output = Command::new("git")
        .arg("-C")
        .arg(root)
        .arg("grep")
        .arg("-nE")
        .arg(r"dirs::config_dir\(")
        .arg("--")
        .arg("crates/")
        .output()
        .ok()?;

    // `git grep` exits 1 when there are zero matches; that's success for
    // us. Any other non-zero exit means git itself failed — skip.
    if !output.status.success() && output.status.code() != Some(1) {
        eprintln!(
            "hermeticity_audit: `git grep` failed (status {:?}); skipping",
            output.status.code()
        );
        return Some(Vec::new());
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    let mut hits = Vec::new();
    for raw_line in stdout.lines() {
        // Line shape: `<path>:<lineno>:<content>`.
        let mut parts = raw_line.splitn(3, ':');
        let path = match parts.next() {
            Some(p) => p,
            None => continue,
        };
        let _lineno = parts.next();
        let content = match parts.next() {
            Some(c) => c,
            None => continue,
        };

        let trimmed = content.trim_start();
        // Skip comment-only mentions: doc comments (`///`, `//!`),
        // line comments (`//`), and block-comment-internals (`*`).
        if trimmed.starts_with("//") || trimmed.starts_with("*") {
            continue;
        }

        if ALLOWLIST.contains(&path) {
            continue;
        }

        hits.push(raw_line.to_string());
    }
    Some(hits)
}

#[test]
fn no_dirs_config_dir_bypasses_outside_canonical_helper() {
    let root = workspace_root();

    // If `.git` isn't present (unusual; happens in some packaging tests),
    // skip rather than fail — the audit is a regression gate, not a hard
    // build precondition.
    if !root.join(".git").exists() {
        eprintln!("hermeticity_audit: no .git at {}; skipping", root.display());
        return;
    }

    let hits = match call_site_hits(&root) {
        Some(h) => h,
        None => {
            eprintln!("hermeticity_audit: `git` unavailable; skipping");
            return;
        }
    };

    assert!(
        hits.is_empty(),
        "Found {} bypass(es) of `wayland_config_dir()` — every call site \
         must route through `wcore_config::config::wayland_config_dir()` so \
         `WAYLAND_HOME` hermetically sandboxes on-disk state (F-010, #270). \
         If a new legitimate caller exists, add its path to ALLOWLIST in \
         this test with an explanation.\n\nOffending lines:\n{}",
        hits.len(),
        hits.join("\n")
    );
}
