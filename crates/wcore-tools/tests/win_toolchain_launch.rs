//! Windows stage 2 — can the DEFAULT sandbox posture actually launch an
//! external toolchain?
//!
//! The product's default for an untrusted workspace is
//! [`WorkspacePolicy::contained`], whose read grant set is the workspace root,
//! the scratch dirs, and `minimal_toolchain_read_dirs()` (`~/.rustup`,
//! `~/.cargo/bin`). Under that posture the measured behaviour on SeanDesktop
//! was: `cmd` builtins run, and every external toolchain fails —
//! git / cargo / rustc with `0xC0000142 STATUS_DLL_INIT_FAILED`, node / python
//! with "not recognized" because their install directories are in no grant set.
//!
//! This binary is the measurement. It drives the REAL product surface —
//! `BashTool::execute_with_ctx` with a `ToolContext` carrying the contained
//! policy and the real `AppContainerBackend` — and grades from world state
//! (did the file appear on disk) as well as exit code.
//!
//! Gating follows `wcore-sandbox/tests/live_cwd_verbatim.rs`: `#![cfg(windows)]`,
//! `#[ignore]`, and an assert (not an early return) on
//! `WAYLAND_SANDBOX_LIVE_WINDOWS=1`, so an unset variable fails rather than
//! silently passing.
#![cfg(windows)]

use std::path::{Path, PathBuf};
use std::sync::Arc;

use serde_json::json;
use wcore_sandbox::SandboxRegistry;
use wcore_sandbox::backends::SandboxBackend;
use wcore_sandbox::backends::appcontainer::AppContainerBackend;
use wcore_tools::Tool;
use wcore_tools::bash::BashTool;
use wcore_tools::context::ToolContext;
use wcore_tools::workspace_policy::WorkspacePolicy;

fn require_live() {
    assert_eq!(
        std::env::var("WAYLAND_SANDBOX_LIVE_WINDOWS").as_deref(),
        Ok("1"),
        "native toolchain acceptance requires WAYLAND_SANDBOX_LIVE_WINDOWS=1"
    );
    assert!(
        AppContainerBackend::new().is_available(),
        "native toolchain acceptance requires an available AppContainer backend"
    );
}

/// Quietness guard. A SECOND wayland-core-family process makes every sandboxed
/// call fail on the machine-wide 15 s ACL mutex, which voided an entire earlier
/// measurement. This binary is itself the only process that should hold it.
fn assert_quiet() {
    let out = std::process::Command::new("tasklist")
        .args(["/FI", "IMAGENAME eq wayland-core.exe", "/NH"])
        .output()
        .expect("tasklist");
    let text = String::from_utf8_lossy(&out.stdout);
    assert!(
        !text.to_ascii_lowercase().contains("wayland-core.exe"),
        "a wayland-core process is running; the machine-wide AppContainer ACL \
         mutex makes every measurement void. tasklist said: {text}"
    );
}

struct Probe {
    exit_code: i32,
    stdout: String,
    stderr: String,
    raw: String,
}

fn run(ctx: &ToolContext, command: &str) -> Probe {
    let result = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("rt")
        .block_on(BashTool.execute_with_ctx(json!({ "command": command }), ctx));
    parse(&result.content)
}

fn parse(content: &str) -> Probe {
    // `output_to_result` renders "Exit code: N\nSTDOUT:\n…\nSTDERR:\n…".
    // A refusal or an execution failure has no such prefix; encode that as
    // a distinguishable sentinel rather than pretending it is an exit code.
    let Some(rest) = content.strip_prefix("Exit code: ") else {
        return Probe {
            exit_code: i32::MIN,
            stdout: String::new(),
            stderr: String::new(),
            raw: content.to_owned(),
        };
    };
    let (code, rest) = rest.split_once('\n').unwrap_or((rest, ""));
    let exit_code = code.trim().parse::<i32>().unwrap_or(i32::MIN);
    let body = rest.strip_prefix("STDOUT:\n").unwrap_or(rest);
    let (stdout, stderr) = body.split_once("\nSTDERR:\n").unwrap_or((body, ""));
    Probe {
        exit_code,
        stdout: stdout.to_owned(),
        stderr: stderr.to_owned(),
        raw: content.to_owned(),
    }
}

/// A workspace on a real disk (NOT `%TEMP%`), because the grant set and the
/// verbatim-prefix path both behave differently for a canonicalized root.
fn workspace(tag: &str) -> PathBuf {
    let base = std::env::var_os("WAYLAND_WIN_LANE_SCRATCH")
        .map(PathBuf::from)
        .unwrap_or_else(|| std::env::temp_dir().join("wayland-win-launch"));
    let dir = base.join(format!("{tag}-{}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("workspace");
    std::fs::canonicalize(&dir).expect("canonicalize workspace")
}

fn contained_ctx(root: &Path) -> (Arc<WorkspacePolicy>, ToolContext) {
    let policy = Arc::new(WorkspacePolicy::contained(root));
    let ctx = ToolContext::test_default()
        .with_workspace(Arc::clone(&policy))
        .with_sandbox(Arc::new(SandboxRegistry::new(Arc::new(
            AppContainerBackend::new(),
        ))));
    (policy, ctx)
}

/// The positive control: a `cmd` builtin under the default posture. Every other
/// case in this file is void if this one fails, so it runs first and is also
/// re-run by each acceptance case.
fn control(ctx: &ToolContext, root: &Path, tag: &str) {
    let marker = format!("control-{tag}.txt");
    let p = run(ctx, &format!("echo hello> {marker}"));
    assert_eq!(
        p.exit_code, 0,
        "POSITIVE CONTROL FAILED at {tag} — every measurement after the last \
         passing control is VOID. raw: {}",
        p.raw
    );
    let landed = root.join(&marker);
    assert!(
        landed.is_file(),
        "control claimed exit 0 but {} is not on disk",
        landed.display()
    );
}

/// THE MEASUREMENT. Prints one row per toolchain and asserts nothing about the
/// toolchains themselves — it exists to produce the table, with the control
/// asserted before, between and after so a wedge point is visible.
#[test]
#[ignore = "explicit native Windows toolchain measurement"]
fn measure_default_posture_toolchain_launch() {
    require_live();
    assert_quiet();
    let root = workspace("measure");
    let (policy, ctx) = contained_ctx(&root);

    println!("WORKSPACE {}", root.display());
    println!("TRUST {:?}", policy.trust());
    for r in policy.readable_roots() {
        println!("READ-GRANT {}", r.display());
    }
    for w in policy.writable_roots() {
        println!("WRITE-GRANT {}", w.display());
    }

    control(&ctx, &root, "start");

    for (name, command) in [
        ("where-git", "where git"),
        ("git", "git --version"),
        ("where-node", "where node"),
        ("node", "node --version"),
        ("where-python", "where python"),
        ("python", "python --version"),
        ("where-cargo", "where cargo"),
        ("cargo", "cargo --version"),
        ("where-rustc", "where rustc"),
        ("rustc", "rustc --version"),
    ] {
        let p = run(&ctx, command);
        println!(
            "ROW name={name} cmd={command:?} exit={} stdout={:?} stderr={:?}",
            p.exit_code,
            p.stdout.trim(),
            p.stderr.trim()
        );
        control(&ctx, &root, name);
    }

    control(&ctx, &root, "end");
    let _ = std::fs::remove_dir_all(&root);
}

/// Discriminator. `where.exe` lives in System32 and is granted read+execute to
/// ALL APPLICATION PACKAGES, so its 0xC0000142 in the measurement above cannot
/// be a grant-set or DLL-location problem. These cases vary ONE thing at a time
/// to locate the boundary the failure sits on:
///
/// * `direct` spawns the external image as the sandbox's OWN program — no
///   `cmd.exe` in the middle.
/// * `via-cmd` spawns the same image as a child of the sandboxed `cmd.exe`.
/// * `nested-cmd` spawns `cmd.exe` itself as a child of `cmd.exe`, so the child
///   image is the one program already proven to start under this sandbox.
///
/// If `direct` succeeds and `via-cmd` fails, the defect is in what a child of
/// the sandboxed shell inherits, not in the token or the ACLs.
#[test]
#[ignore = "explicit native Windows toolchain measurement"]
fn discriminate_direct_spawn_versus_shell_child() {
    require_live();
    assert_quiet();
    let root = workspace("discriminate");
    let (_policy, ctx) = contained_ctx(&root);
    control(&ctx, &root, "start");

    let backend = AppContainerBackend::new();
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("rt");
    let system32 = std::path::Path::new(&std::env::var("SYSTEMROOT").expect("SYSTEMROOT"))
        .join("System32")
        .to_string_lossy()
        .into_owned();

    for (name, argv) in [
        (
            "direct-where",
            vec![format!("{system32}\\where.exe"), "cmd".to_owned()],
        ),
        (
            "via-cmd-where",
            vec![
                "cmd".to_owned(),
                "/c".to_owned(),
                format!("\"{system32}\\where.exe\" cmd"),
            ],
        ),
        (
            "nested-cmd",
            vec![
                "cmd".to_owned(),
                "/c".to_owned(),
                format!("\"{system32}\\cmd.exe\" /c echo nested"),
            ],
        ),
        (
            "direct-cmd",
            vec![
                format!("{system32}\\cmd.exe"),
                "/c".to_owned(),
                "echo plain".to_owned(),
            ],
        ),
    ] {
        let manifest = wcore_sandbox::SandboxManifest {
            timeout: Some(std::time::Duration::from_secs(30)),
            ..Default::default()
        };
        let outcome = rt.block_on(backend.execute(
            &manifest,
            wcore_sandbox::SandboxCommand {
                argv,
                cwd: Some(root.clone()),
            },
        ));
        match outcome {
            Ok(o) => println!(
                "DISC name={name} exit={} stdout={:?} stderr={:?}",
                o.exit_code,
                String::from_utf8_lossy(&o.stdout).trim(),
                String::from_utf8_lossy(&o.stderr).trim()
            ),
            Err(e) => println!("DISC name={name} ERR={e}"),
        }
    }

    control(&ctx, &root, "end");
    let _ = std::fs::remove_dir_all(&root);
}

/// The user32 hypothesis, stated so it can be falsified.
///
/// A byte scan of the import directory says `cmd.exe`, `hostname.exe`,
/// `attrib.exe` and `find.exe` do NOT link `USER32.dll`, while `where.exe`,
/// `git.exe`, `cargo.exe` and `node.exe` do. If user32's `DllMain` is what
/// fails under this sandbox, the first group runs and the second returns
/// 0xC0000142 — and no other property (System32 vs Program Files, granted vs
/// AAP-inherited, Microsoft-signed vs not) separates the two groups the same
/// way. Each row is spawned DIRECTLY, so `cmd.exe` is not in the picture.
///
/// # Why this asserts, and did not
///
/// This test printed a table and asserted NOTHING, so it could not fail.
/// Measured on SeanDesktop 2026-08-10 with `SYSTEMROOT` pointed at an empty
/// directory: all five rows returned `ERR=… does not exist`, nothing was
/// spawned, no property was tested, and the binary still reported
/// `test result: ok. 1 passed`. A hypothesis test that is falsifiable only by a
/// human reading stdout is not a gate. Three checks now stand between the table
/// and a green:
///
/// 1. **Decidedness.** A row whose backend call errored measured nothing, so an
///    undecided row is a failure, not a data point. This is the check the
///    `SYSTEMROOT` sabotage trips.
/// 2. **Positive control.** The `cmd` row must exit 0 AND return its own
///    `ok` on stdout. A sandbox that starts nothing, or whose capture is dead,
///    cannot satisfy the split below honestly.
/// 3. **The hypothesis itself.** Every `links_user32` row must die at image
///    initialization with `STATUS_DLL_INIT_FAILED`, and no row that does not
///    link user32 may die that way. It is deliberately NOT "the other rows exit
///    0": `find /?` legitimately exits 1 under this sandbox, and an exit code
///    from a program that ran is exactly the evidence that it was spawnable.
#[test]
#[ignore = "explicit native Windows toolchain measurement"]
fn user32_linkage_splits_the_spawnable_from_the_dead() {
    require_live();
    assert_quiet();
    let root = workspace("user32");
    let backend = AppContainerBackend::new();
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("rt");
    let system32 =
        std::path::Path::new(&std::env::var("SYSTEMROOT").expect("SYSTEMROOT")).join("System32");

    println!(
        "UI-LIMITS-OVERRIDE {:?}",
        std::env::var("WAYLAND_SANDBOX_DIAG_UI_LIMITS").ok()
    );

    // The backend refused to produce an outcome: this row measured nothing.
    // Same sentinel convention as `parse()` above.
    const UNDECIDED: i32 = i32::MIN;
    // The image never finished initializing — the "dead" side of the split.
    const STATUS_DLL_INIT_FAILED: i32 = 0xC000_0142_u32 as i32;

    let mut rows: Vec<(&str, bool, i32, String)> = Vec::new();
    for (name, links_user32, argv) in [
        (
            "cmd",
            false,
            vec!["cmd.exe".to_owned(), "/c".to_owned(), "echo ok".to_owned()],
        ),
        ("hostname", false, vec!["hostname.exe".to_owned()]),
        ("attrib", false, vec!["attrib.exe".to_owned()]),
        ("find", false, vec!["find.exe".to_owned(), "/?".to_owned()]),
        (
            "where",
            true,
            vec!["where.exe".to_owned(), "cmd".to_owned()],
        ),
    ] {
        let mut argv = argv;
        argv[0] = system32.join(&argv[0]).to_string_lossy().into_owned();
        let manifest = wcore_sandbox::SandboxManifest {
            timeout: Some(std::time::Duration::from_secs(30)),
            ..Default::default()
        };
        let outcome = rt.block_on(backend.execute(
            &manifest,
            wcore_sandbox::SandboxCommand {
                argv,
                cwd: Some(root.clone()),
            },
        ));
        let row = match outcome {
            Ok(o) => {
                let stdout = String::from_utf8_lossy(&o.stdout).trim().to_owned();
                println!(
                    "U32 name={name} links_user32={links_user32} exit={} stdout={stdout:?} stderr={:?}",
                    o.exit_code,
                    String::from_utf8_lossy(&o.stderr).trim()
                );
                (name, links_user32, o.exit_code, stdout)
            }
            Err(e) => {
                println!("U32 name={name} links_user32={links_user32} ERR={e}");
                (name, links_user32, UNDECIDED, format!("ERR={e}"))
            }
        };
        rows.push(row);
    }
    let _ = std::fs::remove_dir_all(&root);

    let undecided: Vec<&str> = rows
        .iter()
        .filter(|(_, _, exit, _)| *exit == UNDECIDED)
        .map(|(name, ..)| *name)
        .collect();
    assert!(
        undecided.is_empty(),
        "the sandbox returned no outcome for {undecided:?}, so the user32 split was never \
         measured and this run certifies nothing. rows: {rows:?}"
    );

    let (_, _, cmd_exit, cmd_stdout) = rows
        .iter()
        .find(|(name, ..)| *name == "cmd")
        .expect("the cmd row is the positive control and must be in the table");
    assert_eq!(
        *cmd_exit, 0,
        "POSITIVE CONTROL FAILED: the sandbox cannot run `cmd /c echo ok`, so every other row \
         is void. rows: {rows:?}"
    );
    assert!(
        cmd_stdout.contains("ok"),
        "POSITIVE CONTROL FAILED: `cmd /c echo ok` exited 0 but its output never arrived, so \
         stdout capture is dead and no row below can be read. rows: {rows:?}"
    );

    for (name, links_user32, exit, detail) in &rows {
        if *links_user32 {
            assert_eq!(
                *exit, STATUS_DLL_INIT_FAILED,
                "{name} links USER32.dll, so the hypothesis predicts it dies at image \
                 initialization with {STATUS_DLL_INIT_FAILED}; it returned {exit} \
                 ({detail:?}). The split is REFUTED. rows: {rows:?}"
            );
        } else {
            assert_ne!(
                *exit, STATUS_DLL_INIT_FAILED,
                "{name} does not link USER32.dll, so the hypothesis predicts it is spawnable; \
                 it died at image initialization instead ({detail:?}). The split is REFUTED \
                 and something other than user32 linkage is killing these images. rows: {rows:?}"
            );
        }
    }
}

/// W-D acceptance. Under the DEFAULT (`contained`) posture, `node` and `python`
/// must run. They are the two toolchains whose install directories carry no
/// `ALL APPLICATION PACKAGES` ACE, so before the grant-set change cmd cannot
/// even stat them and reports "is not recognized as an internal or external
/// command" — that is the RED this test reproduces.
///
/// Graded from WORLD STATE, not stdout: each interpreter is asked to create a
/// file, and the assertion is that the file is on disk afterwards.
///
/// STATUS: this test is RED against HEAD and the defect is OPEN. The obvious
/// repair — putting the two install directories in
/// `minimal_toolchain_read_dirs()` — was written, run, and REVERTED, because a
/// grant is a DACL write and a normal (non-elevated) user does not hold
/// WRITE_DAC on `C:\Program Files\nodejs`. Measured under a `schtasks /IT`
/// task running as the interactive user: every sandboxed command, including
/// `echo`, then failed with
/// `SetNamedSecurityInfoW for \\?\C:\Program Files\nodejs: 0x5`. The same
/// grant succeeds from an elevated shell, which is why an admin ssh session
/// would have reported it as working. The real repair therefore has to make a
/// discovery-derived grant OPTIONAL — skipped with a warning when the DACL
/// cannot be written — instead of aborting the execution.
#[test]
#[ignore = "explicit native Windows toolchain acceptance"]
fn node_and_python_run_under_the_default_posture() {
    require_live();
    assert_quiet();
    let root = workspace("wd");
    let (policy, ctx) = contained_ctx(&root);
    for r in policy.readable_roots() {
        println!("WD READ-GRANT {}", r.display());
    }
    control(&ctx, &root, "start");

    let cases: [(&str, &str, &str); 2] = [
        (
            "node",
            "node-ok.txt",
            "node -e \"require('fs').writeFileSync('node-ok.txt','1')\"",
        ),
        (
            "python",
            "python-ok.txt",
            "python -c \"open('python-ok.txt','w').write('1')\"",
        ),
    ];
    let mut failures = Vec::new();
    for (name, marker, command) in cases {
        let started = std::time::Instant::now();
        let p = run(&ctx, command);
        let landed = root.join(marker);
        println!(
            "WD name={name} exit={} elapsed_ms={} landed={} stderr={:?}",
            p.exit_code,
            started.elapsed().as_millis(),
            landed.is_file(),
            p.stderr.trim()
        );
        if p.exit_code != 0 || !landed.is_file() {
            failures.push(format!(
                "{name}: exit={} landed={} raw={}",
                p.exit_code,
                landed.is_file(),
                p.raw
            ));
        }
        control(&ctx, &root, name);
    }
    control(&ctx, &root, "end");
    let _ = std::fs::remove_dir_all(&root);
    assert!(
        failures.is_empty(),
        "the default posture cannot run these toolchains: {failures:#?}"
    );
}

/// The acceptance the brief asked for, beyond `--version`: does each toolchain
/// actually DO something, graded from world state?
///
/// * git — `init` + `add` + `commit` in a scratch repo, graded by asking git
///   for the commit's own object id and by the presence of `.git/HEAD`.
/// * cargo — build a hello-world, graded by the binary appearing on disk.
///
/// This asserts nothing; it prints, because on this tree node and python cannot
/// start at all (see `node_and_python_run_under_the_default_posture`) and a
/// hard assertion here would just be a second copy of that RED.
#[test]
#[ignore = "explicit native Windows toolchain measurement"]
fn measure_toolchain_work_not_just_version() {
    require_live();
    assert_quiet();
    let root = workspace("work");
    let (_policy, ctx) = contained_ctx(&root);
    control(&ctx, &root, "start");

    // git: a real repository with a real commit.
    let p = run(
        &ctx,
        "git init -q repo && cd repo && git -c user.name=t -c user.email=t@e \
         -c commit.gpgsign=false init -q . && echo hi> a.txt && git add a.txt && \
         git -c user.name=t -c user.email=t@e -c commit.gpgsign=false commit -q -m w && \
         git rev-parse HEAD",
    );
    println!(
        "WORK name=git exit={} head_on_disk={} stdout={:?} stderr={:?}",
        p.exit_code,
        root.join("repo").join(".git").join("HEAD").is_file(),
        p.stdout.trim(),
        p.stderr.trim()
    );
    control(&ctx, &root, "git");

    // cargo: a hello-world that has to compile and link.
    let p = run(
        &ctx,
        "cargo new --offline --bin hello -q && cd hello && cargo build --offline -q",
    );
    let binary = root
        .join("hello")
        .join("target")
        .join("debug")
        .join("hello.exe");
    println!(
        "WORK name=cargo exit={} binary_on_disk={} stdout={:?} stderr={:?}",
        p.exit_code,
        binary.is_file(),
        p.stdout.trim(),
        p.stderr.trim()
    );
    control(&ctx, &root, "cargo");

    control(&ctx, &root, "end");
    let _ = std::fs::remove_dir_all(&root);
}

/// Cost of the grant-set change, measured rather than assumed.
///
/// Every extra directory in the read grant set is an inherited-ACE rewrite over
/// its whole subtree, applied on grant and again on revoke, inside the
/// machine-wide mutation lock (stage 1 measured ~100 µs per file per
/// direction). `C:\Program Files\nodejs` and a Python install are large trees,
/// so adding them could make EVERY Bash call slower. This prints the wall clock
/// of the same trivial `echo` under the default posture so the before/after
/// numbers are comparable.
#[test]
#[ignore = "explicit native Windows toolchain measurement"]
fn measure_trivial_command_wall_clock() {
    require_live();
    assert_quiet();
    let root = workspace("cost");
    let (policy, ctx) = contained_ctx(&root);
    println!("COST grants={}", policy.readable_roots().len());
    for r in policy.readable_roots() {
        println!("COST READ-GRANT {}", r.display());
    }
    for i in 0..5 {
        let started = std::time::Instant::now();
        let p = run(&ctx, &format!("echo cost{i}> cost{i}.txt"));
        println!(
            "COST iter={i} exit={} elapsed_ms={}",
            p.exit_code,
            started.elapsed().as_millis()
        );
        assert_eq!(p.exit_code, 0, "cost iteration {i} failed: {}", p.raw);
    }
    let _ = std::fs::remove_dir_all(&root);
}
