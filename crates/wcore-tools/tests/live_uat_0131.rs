//! LIVE Windows UAT for the two v0.13.1 claims that shipped without ever
//! being run against a real `cmd.exe`.
//!
//! * **CLAIM A** — 845446a6 / PR #272: the Windows Bash argv now carries `/S`,
//!   so a nested `cmd /c echo ...` payload no longer returns a stray trailing
//!   `0x22`. Graded on the EXACT BYTES, with a RED ARM (the pre-fix `cmd /C`
//!   argv, same backend, same payload) beside every green one. A green arm on
//!   its own cannot tell "fixed" from "never broken on this machine".
//! * **CLAIM B** — PR #278: the per-exec workspace read-deny walk is skipped
//!   on a backend that discards the list. Timed through the real
//!   `BashTool::execute_with_ctx` surface in a large real checkout and in an
//!   empty directory (the control), with the pre-fix walk timed beside it.
//!
//! This binary is DELIBERATELY not part of any gate. It is an evidence
//! generator: it prints measurements and asserts only the claims themselves.
#![cfg(windows)]

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Instant;

use serde_json::json;
use wcore_config::shell::bash_shell_argv_prefix;
use wcore_sandbox::backends::SandboxBackend;
use wcore_sandbox::backends::no_sandbox::NoSandboxBackend;
use wcore_sandbox::{SandboxCommand, SandboxManifest, SandboxRegistry, default_for_platform};
use wcore_tools::Tool;
use wcore_tools::bash::BashTool;
use wcore_tools::context::ToolContext;
use wcore_tools::workspace_policy::WorkspacePolicy;

/// Fail rather than silently skip when the operator forgot the opt-in: a
/// vacuous pass is the failure mode this whole binary exists to avoid.
fn require_live() {
    assert_eq!(
        std::env::var("WAYLAND_LIVE_UAT").as_deref(),
        Ok("1"),
        "this binary runs real cmd.exe processes; set WAYLAND_LIVE_UAT=1"
    );
}

fn rt() -> tokio::runtime::Runtime {
    tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .expect("tokio runtime")
}

// ---------------------------------------------------------------------------
// byte reporting
// ---------------------------------------------------------------------------

fn hex(bytes: &[u8]) -> String {
    bytes
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect::<Vec<_>>()
        .join(" ")
}

fn printable(bytes: &[u8]) -> String {
    bytes
        .iter()
        .map(|b| match b {
            b'\r' => "\\r".to_string(),
            b'\n' => "\\n".to_string(),
            0x20..=0x7e => (*b as char).to_string(),
            other => format!("\\x{other:02x}"),
        })
        .collect()
}

/// stdout with only the trailing CR/LF removed. A trailing `"` survives this,
/// which is the entire point.
fn strip_eol(bytes: &[u8]) -> &[u8] {
    let mut end = bytes.len();
    while end > 0 && (bytes[end - 1] == b'\n' || bytes[end - 1] == b'\r') {
        end -= 1;
    }
    &bytes[..end]
}

fn report(arm: &str, argv: &[String], payload: &str, stdout: &[u8], exit: i32) -> bool {
    let body = strip_eol(stdout);
    let stray = body.last() == Some(&0x22);
    println!("  [{arm}] argv          = {argv:?}");
    println!("  [{arm}] payload       = {payload:?}");
    println!("  [{arm}] exit          = {exit}");
    println!("  [{arm}] stdout hex    = {}", hex(stdout));
    println!("  [{arm}] stdout text   = {}", printable(stdout));
    println!("  [{arm}] eol-stripped  = {}", hex(body));
    println!("  [{arm}] TRAILING 0x22 = {stray}");
    stray
}

// ---------------------------------------------------------------------------
// CLAIM A
// ---------------------------------------------------------------------------

/// The minimum scrubbed env `cmd.exe` needs to start and to resolve a program
/// on disk. The executable-file test in cmd's quote-PRESERVING branch is a
/// filesystem question, so PATH/PATHEXT/COMSPEC must be present or the RED arm
/// cannot reproduce for the wrong reason.
fn manifest_env() -> SandboxManifest {
    let mut manifest = SandboxManifest::default();
    for key in [
        "PATH",
        "SYSTEMROOT",
        "COMSPEC",
        "PATHEXT",
        "TEMP",
        "TMP",
        "WINDIR",
    ] {
        if let Some(value) = std::env::var_os(key) {
            manifest
                .env
                .push((key.to_string(), value.to_string_lossy().into_owned()));
        }
    }
    manifest
}

/// Run `payload` through the PRODUCTION backend with the supplied argv prefix.
///
/// `NoSandboxBackend::append_args` is what applies `quote_cmd_payload` (the one
/// outer pair) and it keys off `cmd_payload_index`, which finds the payload for
/// both `cmd /S /C` and the pre-fix `cmd /C`. So the ONLY difference between
/// the two arms below is the `/S` switch — every other byte of the spawn path
/// is shared production code.
fn run_backend(prefix: &[String], payload: &str) -> (Vec<u8>, Vec<u8>, i32) {
    let mut argv = prefix.to_vec();
    argv.push(payload.to_string());
    let manifest = manifest_env();
    let out = rt()
        .block_on(
            NoSandboxBackend::new().execute(&manifest, SandboxCommand { argv, cwd: None }),
        )
        .expect("cmd.exe must run");
    (out.stdout, out.stderr, out.exit_code)
}

fn pre_fix_prefix() -> Vec<String> {
    vec!["cmd".to_string(), "/C".to_string()]
}

#[test]
fn claim_a_nested_cmd_payload_bytes_green_and_red() {
    require_live();
    println!("=== CLAIM A (#272 / 845446a6): /S strips the wrapper's outer pair ===");
    let green_prefix = bash_shell_argv_prefix();
    println!("bash_shell_argv_prefix() = {green_prefix:?}");
    assert_eq!(
        green_prefix,
        vec!["cmd".to_string(), "/S".to_string(), "/C".to_string()],
        "the shipping tree must build the Windows Bash argv with /S"
    );
    let red_prefix = pre_fix_prefix();

    // (payload, expected text after the eol strip)
    let cases: [(&str, &str); 3] = [
        // The #943 shape: a nested cmd whose leading token names an executable
        // on disk, which is what enables cmd's quote-PRESERVING branch.
        ("cmd /c echo NESTED", "NESTED"),
        // Control 1: `echo` is internal, nothing on disk answers the executable
        // test, so this shape was ALREADY clean before the fix.
        ("echo NOQUOTE", "NOQUOTE"),
        // Control 2: `&` disqualifies the preserving branch, so this shape was
        // ALREADY clean too. A fix that merely trimmed trailing quotes would
        // move one of these two controls.
        ("cmd /c echo A && cmd /c echo B", ""),
    ];

    let mut green_dirty: Vec<&str> = Vec::new();
    let mut red_dirty: Vec<&str> = Vec::new();

    for (payload, expected) in cases {
        println!("\n-- payload {payload:?}");
        let (g_out, g_err, g_code) = run_backend(&green_prefix, payload);
        if report("GREEN /S", &green_prefix, payload, &g_out, g_code) {
            green_dirty.push(payload);
        }
        if !g_err.is_empty() {
            println!("  [GREEN /S] stderr      = {}", printable(&g_err));
        }
        let (r_out, r_err, r_code) = run_backend(&red_prefix, payload);
        if report("RED  no/S", &red_prefix, payload, &r_out, r_code) {
            red_dirty.push(payload);
        }
        if !r_err.is_empty() {
            println!("  [RED  no/S] stderr      = {}", printable(&r_err));
        }

        if !expected.is_empty() {
            let got = String::from_utf8_lossy(strip_eol(&g_out)).to_string();
            let got = got.trim_end().to_string();
            assert_eq!(
                got, expected,
                "GREEN arm for {payload:?} produced {:x?}; a trailing 0x22 is #943",
                g_out
            );
        }
    }

    println!("\n-- summary");
    println!("GREEN arms with a stray trailing 0x22: {green_dirty:?}");
    println!("RED   arms with a stray trailing 0x22: {red_dirty:?}");
    println!(
        "RED_ARM_REPRODUCED={}",
        red_dirty.contains(&"cmd /c echo NESTED")
    );

    assert!(
        green_dirty.is_empty(),
        "the shipping argv must never deliver a stray quote: {green_dirty:?}"
    );
}

/// The same claim driven through the tool surface the model actually calls,
/// so the result cannot be an artefact of hand-assembling the argv.
#[test]
fn claim_a_through_the_bash_tool_surface() {
    require_live();
    println!("=== CLAIM A — end to end via BashTool::execute_with_ctx ===");
    let backend = default_for_platform();
    println!("default_for_platform() = {}", backend.name());
    println!("  enforces_read_deny    = {}", backend.enforces_read_deny());
    let ctx = ToolContext::test_default();

    for payload in ["cmd /c echo NESTED", "echo NOQUOTE"] {
        let result = rt().block_on(BashTool.execute_with_ctx(json!({ "command": payload }), &ctx));
        println!("\n-- BashTool {payload:?}");
        println!("  raw ToolResult = {:?}", result.content);
        let stdout = extract_stdout(&result.content);
        println!("  stdout hex     = {}", hex(stdout.as_bytes()));
        println!("  stdout text    = {}", printable(stdout.as_bytes()));
        let body = stdout.trim_end_matches(['\r', '\n']);
        assert!(
            !body.ends_with('"'),
            "BashTool returned a stray trailing quote for {payload:?}: {:?}",
            result.content
        );
    }
}

/// `output_to_result` renders `Exit code: N\nSTDOUT:\n…\nSTDERR:\n…`.
fn extract_stdout(content: &str) -> String {
    let Some(rest) = content.strip_prefix("Exit code: ") else {
        return content.to_string();
    };
    let (_code, rest) = rest.split_once('\n').unwrap_or((rest, ""));
    let body = rest.strip_prefix("STDOUT:\n").unwrap_or(rest);
    body.split_once("\nSTDERR:\n")
        .map(|(o, _)| o.to_string())
        .unwrap_or_else(|| body.to_string())
}

// ---------------------------------------------------------------------------
// CLAIM B
// ---------------------------------------------------------------------------

/// The posture the shipped CLI installs for a non-channel session on an
/// untrusted workspace (`bootstrap.rs`): `contained` + the local-operator
/// shell principal. That combination is what #922 measured — it keeps
/// `secret_read_deny_required` (so the walk is requested) while
/// `shell_requires_os_read_deny()` is false (so the shell is not refused on
/// the Windows job-object backend).
fn shipping_policy(root: &Path) -> Arc<WorkspacePolicy> {
    Arc::new(WorkspacePolicy::contained(root).with_shell_principal(false, false))
}

fn ctx_for(policy: Arc<WorkspacePolicy>) -> ToolContext {
    ToolContext::test_default()
        .with_workspace(policy)
        .with_sandbox(Arc::new(SandboxRegistry::new(Arc::from(
            default_for_platform(),
        ))))
}

fn timed_echo(rt: &tokio::runtime::Runtime, ctx: &ToolContext, tag: &str) -> u128 {
    // The runtime is built by the caller: constructing one costs milliseconds
    // and this measurement is reported in milliseconds.
    let started = Instant::now();
    let result = rt.block_on(BashTool.execute_with_ctx(json!({ "command": "echo uat" }), ctx));
    let ms = started.elapsed().as_millis();
    let ok = result.content.contains("uat");
    println!("  {tag:<22} {ms:>8} ms   ok={ok}");
    if !ok {
        println!("    !! no output — raw: {:?}", result.content);
    }
    ms
}

#[test]
fn claim_b_read_deny_walk_timing() {
    require_live();
    println!("=== CLAIM B (#278): the per-exec read-deny walk ===");

    let real_root = PathBuf::from(
        std::env::var("WAYLAND_UAT_REAL_TREE").expect("WAYLAND_UAT_REAL_TREE must be set"),
    );
    let empty_root = PathBuf::from(
        std::env::var("WAYLAND_UAT_EMPTY_DIR").expect("WAYLAND_UAT_EMPTY_DIR must be set"),
    );
    assert!(real_root.is_dir(), "{} is not a dir", real_root.display());
    assert!(empty_root.is_dir(), "{} is not a dir", empty_root.display());
    println!("real tree  = {}", real_root.display());
    println!("empty dir  = {}", empty_root.display());
    println!(
        "NOTE: this tree was just checked out and built on this runner, so the \
         NTFS metadata cache is WARM. A true cold-cache number is not \
         obtainable inside a CI job without a reboot."
    );

    let backend = default_for_platform();
    println!(
        "backend={} enforces_read_deny={}",
        backend.name(),
        backend.enforces_read_deny()
    );

    // ---- the four requested numbers. NOTHING above this point has walked
    // either tree, and the arms are interleaved real/empty so a drift in
    // machine load cannot masquerade as a difference between them.
    let real_policy = shipping_policy(&real_root);
    let empty_policy = shipping_policy(&empty_root);
    println!(
        "policy(real): shell_requires_os_read_deny={} secret_read_deny_required={}",
        real_policy.shell_requires_os_read_deny(),
        real_policy.secret_read_deny_required()
    );
    let real_ctx = ctx_for(Arc::clone(&real_policy));
    let empty_ctx = ctx_for(Arc::clone(&empty_policy));

    println!("\n-- BashTool `echo uat` (shipping path, PR #278 applied)");
    let runtime = rt();
    let real_cold = timed_echo(&runtime, &real_ctx, "real-tree COLD");
    let empty_cold = timed_echo(&runtime, &empty_ctx, "empty-dir COLD");
    let mut real_warm = Vec::new();
    let mut empty_warm = Vec::new();
    for _ in 0..3 {
        real_warm.push(timed_echo(&runtime, &real_ctx, "real-tree warm"));
        empty_warm.push(timed_echo(&runtime, &empty_ctx, "empty-dir warm"));
    }
    let real_warm_best = *real_warm.iter().min().expect("warm samples");
    let empty_warm_best = *empty_warm.iter().min().expect("warm samples");

    // ---- the cause. `secret_deny_paths_for_backend(true)` IS the pre-#278
    // code path byte for byte (A3 pins it identical to the old list); `(false)`
    // is what the shipping Windows default now does. Same policy object, same
    // tree, back to back.
    println!("\n-- the walk itself (red arm = the pre-#278 behaviour)");
    let t = Instant::now();
    let denied = real_policy.secret_deny_paths_for_backend(true);
    let real_walk = t.elapsed().as_millis();
    println!("  real-tree WALK (pre-fix, enforcing)   {real_walk:>8} ms  entries={}", denied.len());

    let t = Instant::now();
    let skipped = real_policy.secret_deny_paths_for_backend(false);
    let real_skip = t.elapsed().as_millis();
    println!("  real-tree SKIP (shipping, job object) {real_skip:>8} ms  entries={}", skipped.len());

    let t = Instant::now();
    let e_denied = empty_policy.secret_deny_paths_for_backend(true);
    let empty_walk = t.elapsed().as_millis();
    println!("  empty-dir WALK (pre-fix, enforcing)   {empty_walk:>8} ms  entries={}", e_denied.len());

    let t = Instant::now();
    let e_skipped = empty_policy.secret_deny_paths_for_backend(false);
    let empty_skip = t.elapsed().as_millis();
    println!("  empty-dir SKIP (shipping, job object) {empty_skip:>8} ms  entries={}", e_skipped.len());

    // ---- size of the tree, measured LAST so the walk above was not warmed by
    // this enumeration.
    let (files, bytes) = count_tree(&real_root);
    println!("\nreal tree size: {files} files, {bytes} bytes");
    let (efiles, ebytes) = count_tree(&empty_root);
    println!("empty dir size: {efiles} files, {ebytes} bytes");

    println!("\n=== CLAIM B RESULT TABLE (ms) ===");
    println!("real-tree cold echo   = {real_cold}");
    println!("real-tree warm echo   = {real_warm_best}   (samples {real_warm:?})");
    println!("empty-dir cold echo   = {empty_cold}");
    println!("empty-dir warm echo   = {empty_warm_best}   (samples {empty_warm:?})");
    println!("real-tree pre-fix walk= {real_walk}");
    println!("real-tree shipping    = {real_skip}");
    println!("empty-dir pre-fix walk= {empty_walk}");
    println!("empty-dir shipping    = {empty_skip}");

    assert!(
        skipped.is_empty(),
        "the shipping Windows backend must not compute a list it discards"
    );
}

/// Plain recursive count. Deliberately called only AFTER every timing above.
fn count_tree(root: &Path) -> (u64, u64) {
    fn walk(dir: &Path, files: &mut u64, bytes: &mut u64) {
        let Ok(entries) = std::fs::read_dir(dir) else {
            return;
        };
        for entry in entries.flatten() {
            let Ok(meta) = entry.metadata() else { continue };
            if meta.is_dir() {
                walk(&entry.path(), files, bytes);
            } else {
                *files += 1;
                *bytes += meta.len();
            }
        }
    }
    let mut files = 0;
    let mut bytes = 0;
    walk(root, &mut files, &mut bytes);
    (files, bytes)
}
