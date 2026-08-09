//! Windows — can the DEFAULT sandbox posture actually launch and run an
//! external toolchain?
//!
//! The product's default for an untrusted workspace is
//! [`WorkspacePolicy::contained`], whose read grant set is the workspace root,
//! the scratch dirs, and `minimal_toolchain_read_dirs()` (`~/.rustup`,
//! `~/.cargo/bin`). Before the window-station repair the measured behaviour on
//! SEANDESKTOP was: `cmd` builtins run, and every USER32-linked image —
//! `where`, `git`, `cargo`, `rustc`, `node` — dies at load with
//! `0xC0000142 STATUS_DLL_INIT_FAILED`, because the child inherited the
//! engine's window station and a non-interactive station carries no
//! `ALL APPLICATION PACKAGES` ACE.
//!
//! This binary drives the REAL product surface — `BashTool::execute_with_ctx`
//! with a `ToolContext` carrying the contained policy and the real
//! `AppContainerBackend` — and grades from world state (did the object appear
//! on disk) as well as exit code.
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

/// Spawn a System32 image DIRECTLY through the backend, with no `cmd.exe` in
/// the middle, and report the raw exit status.
fn spawn_direct(backend: &AppContainerBackend, root: &Path, argv: Vec<String>) -> (i32, String) {
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("rt");
    let manifest = wcore_sandbox::SandboxManifest {
        timeout: Some(std::time::Duration::from_secs(30)),
        ..Default::default()
    };
    let outcome = rt.block_on(backend.execute(
        &manifest,
        wcore_sandbox::SandboxCommand {
            argv,
            cwd: Some(root.to_path_buf()),
        },
    ));
    match outcome {
        Ok(o) => (
            o.exit_code,
            format!(
                "stdout={:?} stderr={:?}",
                String::from_utf8_lossy(&o.stdout).trim(),
                String::from_utf8_lossy(&o.stderr).trim()
            ),
        ),
        Err(e) => (i32::MIN, format!("ERR={e}")),
    }
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
/// ALL APPLICATION PACKAGES, so its 0xC0000142 cannot be a grant-set or
/// DLL-location problem. These cases vary ONE thing at a time to locate the
/// boundary the failure sits on.
#[test]
#[ignore = "explicit native Windows toolchain measurement"]
fn discriminate_direct_spawn_versus_shell_child() {
    require_live();
    assert_quiet();
    let root = workspace("discriminate");
    let (_policy, ctx) = contained_ctx(&root);
    control(&ctx, &root, "start");

    let backend = AppContainerBackend::new();
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
        let (exit, detail) = spawn_direct(&backend, &root, argv);
        println!("DISC name={name} exit={exit} {detail}");
    }

    control(&ctx, &root, "end");
    let _ = std::fs::remove_dir_all(&root);
}

/// **W-C, the acceptance for the window-station repair.**
///
/// A byte scan of the import directory says `cmd.exe`, `hostname.exe`,
/// `attrib.exe` and `find.exe` do NOT link `USER32.dll`, while `where.exe`,
/// `git.exe`, `cargo.exe` and `node.exe` do. Before the repair, exactly the
/// second group returned `0xC0000142` at image init — USER32's process-attach
/// could not open the window station the child inherited from the engine.
///
/// This case is a REGRESSION GATE, not a measurement, and it discriminates in
/// both directions:
///
/// * The non-USER32 rows are the **positive control**. If `cmd` /
///   `hostname` / `attrib` cannot run, the sandbox is dead for reasons that
///   have nothing to do with USER32 and this case is VOID — so it fails
///   loudly instead of reporting a green that means nothing.
/// * The USER32 rows are the **claim**. They must start. Delete
///   `lpDesktop` from `windows_impl::process` and this assertion returns to
///   `exit=-1073741502 (0xC0000142)`.
///
/// Each row is spawned DIRECTLY, so `cmd.exe` and PATH resolution are not in
/// the picture; the only variable is the image's USER32 linkage.
#[test]
#[ignore = "explicit native Windows toolchain acceptance"]
fn user32_linked_images_start_under_the_sandbox() {
    require_live();
    assert_quiet();
    let root = workspace("user32");
    let backend = AppContainerBackend::new();
    let system32 =
        std::path::Path::new(&std::env::var("SYSTEMROOT").expect("SYSTEMROOT")).join("System32");

    let cases: [(&str, bool, &[&str]); 5] = [
        ("cmd", false, &["cmd.exe", "/c", "echo ok"]),
        ("hostname", false, &["hostname.exe"]),
        ("attrib", false, &["attrib.exe"]),
        ("where", true, &["where.exe", "cmd"]),
        ("find", false, &["find.exe", "/?"]),
    ];

    let mut control_failures = Vec::new();
    let mut claim_failures = Vec::new();
    for (name, links_user32, argv) in cases {
        let mut argv: Vec<String> = argv.iter().map(|s| (*s).to_owned()).collect();
        argv[0] = system32.join(&argv[0]).to_string_lossy().into_owned();
        let (exit, detail) = spawn_direct(&backend, &root, argv);
        println!("U32 name={name} links_user32={links_user32} exit={exit} {detail}");
        // `find /?` exits 0; every other row exits 0 on success. The failure
        // being gated is image init, which surfaces as the NTSTATUS itself
        // (0xC0000142 as i32 = -1073741502), never as a program exit code.
        let started = exit != i32::MIN && exit != -1_073_741_502;
        if !started {
            if links_user32 {
                claim_failures.push(format!("{name}: exit={exit} {detail}"));
            } else {
                control_failures.push(format!("{name}: exit={exit} {detail}"));
            }
        }
    }
    let _ = std::fs::remove_dir_all(&root);

    assert!(
        control_failures.is_empty(),
        "POSITIVE CONTROL FAILED — images that do NOT link USER32 could not \
         start either, so this case tests nothing about USER32 and its verdict \
         is VOID: {control_failures:#?}"
    );
    assert!(
        claim_failures.is_empty(),
        "USER32-linked images cannot start under the sandbox while non-USER32 \
         images can — the child is on a window station its AppContainer token \
         cannot open: {claim_failures:#?}"
    );
}

/// W-D acceptance. Under the DEFAULT (`contained`) posture, `node` and `python`
/// must run. Their install directories carry no `ALL APPLICATION PACKAGES` ACE
/// and are in no grant set, so before the grant-set change `cmd` cannot even
/// stat them and reports "is not recognized as an internal or external
/// command" — that is the RED this test reproduces.
///
/// Graded from WORLD STATE, not stdout: each interpreter is asked to create a
/// file, and the assertion is that the file is on disk afterwards.
///
/// STATUS: RED against HEAD, defect OPEN. The obvious repair — putting the two
/// install directories in `minimal_toolchain_read_dirs()` — was written, run,
/// and REVERTED, because a grant is a DACL write and a normal (non-elevated)
/// user does not hold WRITE_DAC on `C:\Program Files\nodejs`. Measured under a
/// `schtasks /IT` task running as the interactive user: every sandboxed
/// command, including `echo`, then failed with
/// `SetNamedSecurityInfoW for \\?\C:\Program Files\nodejs: 0x5`. The same grant
/// succeeds from an elevated shell, which is why an admin ssh session would
/// have reported it as working. The real repair has to make a
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
/// * **git** — `init` + `add` + `commit`, graded by `.git/HEAD` existing on
///   disk AND by `git cat-file -t <rev-parse HEAD>` answering `commit`, so a
///   git that printed a plausible id without writing an object cannot pass.
/// * **rustc** — compile a source file directly, graded by the `.exe`
///   appearing on disk.
/// * **cargo** — build a hello-world, graded by the binary appearing on disk.
///
/// Every row asserts. The control runs before and after each one, so a
/// failure that is really a wedged sandbox is reported as a wedged sandbox.
#[test]
#[ignore = "explicit native Windows toolchain acceptance"]
fn git_rustc_and_cargo_do_real_work_under_the_default_posture() {
    require_live();
    assert_quiet();
    let root = workspace("work");
    let (_policy, ctx) = contained_ctx(&root);
    control(&ctx, &root, "start");
    let mut failures: Vec<String> = Vec::new();

    // ---- git: a real repository with a real commit object ----
    let p = run(
        &ctx,
        "git -c init.defaultBranch=main init -q repo && cd repo && \
         echo hi> a.txt && git add a.txt && \
         git -c user.name=t -c user.email=t@e -c commit.gpgsign=false commit -q -m w && \
         git rev-parse HEAD && git cat-file -t HEAD",
    );
    let head_on_disk = root.join("repo").join(".git").join("HEAD").is_file();
    let names_a_commit = p.stdout.lines().any(|l| l.trim() == "commit");
    let rev = p
        .stdout
        .lines()
        .find(|l| l.trim().len() == 40 && l.trim().chars().all(|c| c.is_ascii_hexdigit()))
        .map(str::trim)
        .unwrap_or("");
    println!(
        "WORK name=git exit={} head_on_disk={head_on_disk} rev={rev:?} \
         cat_file_says_commit={names_a_commit} stderr={:?}",
        p.exit_code,
        p.stderr.trim()
    );
    if p.exit_code != 0 || !head_on_disk || rev.is_empty() || !names_a_commit {
        failures.push(format!("git: exit={} raw={}", p.exit_code, p.raw));
    }
    control(&ctx, &root, "git");

    // ---- rustc: compile one file, straight to an exe ----
    let p = run(
        &ctx,
        "echo fn main(){println!(\"rc\");}> m.rs && rustc -O -o m.exe m.rs",
    );
    let rustc_binary = root.join("m.exe");
    println!(
        "WORK name=rustc exit={} binary_on_disk={} stderr={:?}",
        p.exit_code,
        rustc_binary.is_file(),
        p.stderr.trim()
    );
    if p.exit_code != 0 || !rustc_binary.is_file() {
        failures.push(format!("rustc: exit={} raw={}", p.exit_code, p.raw));
    }
    control(&ctx, &root, "rustc");

    // ---- cargo: a hello-world that has to compile and link ----
    let p = run(
        &ctx,
        "cargo new --offline --bin hello -q && cd hello && cargo build --offline -q",
    );
    let cargo_binary = root
        .join("hello")
        .join("target")
        .join("debug")
        .join("hello.exe");
    println!(
        "WORK name=cargo exit={} binary_on_disk={} stderr={:?}",
        p.exit_code,
        cargo_binary.is_file(),
        p.stderr.trim()
    );
    if p.exit_code != 0 || !cargo_binary.is_file() {
        failures.push(format!("cargo: exit={} raw={}", p.exit_code, p.raw));
    }
    control(&ctx, &root, "cargo");

    control(&ctx, &root, "end");
    let _ = std::fs::remove_dir_all(&root);
    assert!(
        failures.is_empty(),
        "the default posture cannot do real work with these toolchains: {failures:#?}"
    );
}

/// Cost of the sandbox per trivial command, measured rather than assumed.
///
/// Every directory in the read grant set is an inherited-ACE rewrite over its
/// whole subtree, applied on grant and again on revoke, inside the machine-wide
/// mutation lock (~100 µs per file per direction). This prints the wall clock
/// of a trivial `echo` under the default posture so any grant-set change can be
/// costed against a comparable before/after number.
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
