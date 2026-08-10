//! Windows — WHY can a sandboxed child not resolve its own current directory?
//!
//! This is a MEASUREMENT binary, not a gate. It exists because two named
//! repair directions for that defect were both wrong, and the only way to stop
//! a third wave re-spending the same hours is to keep the instrument that
//! refuted them.
//!
//! # The defect, as the product shows it
//!
//! Under the default (`WorkspacePolicy::contained`) posture on
//! SEANDESKTOP / Windows 10.0.26200, with the window-station repair in place
//! so every image starts:
//!
//! ```text
//! git   exit=128  fatal: unable to get current working directory: Permission denied
//! cargo exit=101  warn: could not canonicalize path <workspace>
//!                 failed to resolve path '<workspace>/hello/.git/': Access is denied.
//! rustc exit=1    warn: could not canonicalize path <workspace>
//! ```
//!
//! All three are ONE defect — path resolution of the working directory — not
//! the two ("git cwd" and "cargo needs DELETE") they were filed as. `cargo`
//! and `rustc` emit the identical `could not canonicalize path <cwd>` warning
//! that precedes git's failure, and libgit2's message is `failed to RESOLVE
//! path`, a read-side realpath. Nothing on this path deletes anything, so the
//! missing-`DELETE`-in-`ACL_WRITE_MASK` theory does not touch it.
//!
//! # What is measured, and what it refutes
//!
//! The child CAN, inside its granted workspace root: create a file, read a
//! file back by absolute path, and read the directory's own DACL (`icacls .`
//! from inside the sandbox prints the live per-execution package-SID ace,
//! `(OI)(CI)(RX,W)`). It CANNOT enumerate that same directory. Three
//! independent enumerators agree, so this is not one broken tool:
//!
//! ```text
//! dir /b .                 exit=1  Access is denied.
//! where /R <workspace> *   exit=2  ERROR: Access is denied.
//! attrib *                 exit=0  Path not found - <workspace>
//! ```
//!
//! Enumeration also fails on `%SystemRoot%\System32`, which carries an
//! explicit `ALL APPLICATION PACKAGES` read+execute ace — so the failure is
//! not "this directory was not granted".
//!
//! **REFUTED — do not re-spend these.**
//!
//! 1. *"The workspace's ANCESTOR directories are in no grant set."* True but
//!    causally irrelevant. Measured serially, one variant at a time, with the
//!    positive control asserted before and after each: granting the parent
//!    `FILE_TRAVERSE` changed nothing, and granting it
//!    `FILE_LIST_DIRECTORY | FILE_TRAVERSE | FILE_READ_ATTRIBUTES |
//!    SYNCHRONIZE` also changed nothing. Every row stayed byte-identical to
//!    the ungranted baseline.
//! 2. *"Traverse is bypassed anyway, so it must be traverse."* The
//!    AppContainer token does keep `SeChangeNotifyPrivilege` — the child reads
//!    `<workspace>\file` through ancestors it holds no ace on — but adding
//!    traverse fixes nothing, see (1).
//! 3. *"Grant the ancestor chain up to the drive root."* Closed on cost, not
//!    on principle: a DACL write on a drive root walks the volume. Measured on
//!    the same box, `icacls D:\ /grant` with NO `/T` burned 473 s of CPU and
//!    `Set-Acl D:\` another 143 s, both killed unfinished, against 3 ms for
//!    the workspace's own parent. It can never be a per-command mutation, and
//!    an unelevated engine cannot write it at all.
//!
//! # What is still open
//!
//! Why a directory-enumeration open is refused while a file open on the same
//! DACL succeeds. Answering it needs the NTSTATUS from an `NtOpenFile` with
//! `FILE_LIST_DIRECTORY` inside the container, which is a native probe this
//! binary does not have.
//!
//! Gating matches the other live Windows binaries: `#![cfg(windows)]`,
//! `#[ignore]`, and an ASSERT (not an early return) on
//! `WAYLAND_SANDBOX_LIVE_WINDOWS=1`, so an unset variable fails loudly rather
//! than passing silently.
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
        "ancestor probe requires WAYLAND_SANDBOX_LIVE_WINDOWS=1"
    );
    assert!(
        AppContainerBackend::new().is_available(),
        "ancestor probe requires an available AppContainer backend"
    );
}

/// A SECOND wayland-core-family process makes every sandboxed call fail on the
/// machine-wide 15 s ACL mutex, which has already voided one whole
/// measurement. This binary must be the only one holding it.
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

fn workspace(tag: &str) -> PathBuf {
    let base = std::env::var_os("WAYLAND_WIN_LANE_SCRATCH")
        .map(PathBuf::from)
        .unwrap_or_else(|| std::env::temp_dir().join("wayland-win-launch"));
    let dir = base.join(format!("{tag}-{}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("workspace");
    std::fs::canonicalize(&dir).expect("canonicalize workspace")
}

fn contained_ctx(root: &Path) -> ToolContext {
    ToolContext::test_default()
        .with_workspace(Arc::new(WorkspacePolicy::contained(root)))
        .with_sandbox(Arc::new(SandboxRegistry::new(Arc::new(
            AppContainerBackend::new(),
        ))))
}

struct Probe {
    exit_code: i32,
    stdout: String,
    stderr: String,
    raw: String,
}

fn run(ctx: &ToolContext, command: &str) -> Probe {
    let content = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("rt")
        .block_on(BashTool.execute_with_ctx(json!({ "command": command }), ctx))
        .content;
    let Some(rest) = content.strip_prefix("Exit code: ") else {
        return Probe {
            exit_code: i32::MIN,
            stdout: String::new(),
            stderr: String::new(),
            raw: content,
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
        raw: content.clone(),
    }
}

/// The positive control: a `cmd` builtin writing INTO the granted workspace
/// root, graded from world state. It asserts, so a run where the sandbox was
/// simply wedged reports a wedged sandbox instead of an access finding.
fn control(ctx: &ToolContext, root: &Path, tag: &str) {
    let marker = format!("anc-control-{tag}.txt");
    let p = run(ctx, &format!("echo hello> {marker}"));
    assert_eq!(
        p.exit_code, 0,
        "POSITIVE CONTROL FAILED at {tag} - every measurement after the last \
         passing control is VOID. raw: {}",
        p.raw
    );
    assert!(
        root.join(&marker).is_file(),
        "control claimed exit 0 but wrote nothing at {tag}"
    );
}

/// One row per access shape, all against the child's OWN granted workspace
/// root. Nothing about the rows is asserted — the file's job is to produce the
/// table — but the control before and after is, so a void run is visible.
#[test]
#[ignore = "explicit native Windows directory-resolution measurement"]
fn sandboxed_child_cannot_enumerate_its_own_granted_workspace() {
    require_live();
    assert_quiet();
    let root = workspace("anc");
    let ctx = contained_ctx(&root);
    println!("ANC ROOT {}", root.display());

    control(&ctx, &root, "start");

    // The child sees a de-prefixed cwd (`resolve_cwd` strips the verbatim
    // `\\?\`, because cmd.exe reads a leading `\\` as UNC), so every row uses
    // the plain form the child itself would type.
    let root_plain = root
        .display()
        .to_string()
        .trim_start_matches("\\\\?\\")
        .to_owned();
    let system32 = Path::new(&std::env::var("SYSTEMROOT").expect("SYSTEMROOT"))
        .join("System32")
        .display()
        .to_string();

    let cases: Vec<(&str, String)> = vec![
        // Works today: the cwd is inherited, no resolution needed.
        ("cwd", "cd".to_owned()),
        // Works today: a file open + read through an ancestor holding no ace,
        // which is what proves traverse is already available.
        (
            "read-file-by-absolute-path",
            format!("type \"{root_plain}\\anc-control-start.txt\""),
        ),
        // Works today: reads the live per-execution package-SID ace, so the
        // grant is provably present on this very directory.
        ("dacl-of-self", format!("icacls \"{root_plain}\"")),
        // Fails: three independent enumerators, so no single broken tool can
        // explain it. `dir` is the suspect one (it also queries volume
        // information, which an AppContainer cannot do); `where /R` and
        // `attrib` use FindFirstFileW and touch no volume device.
        ("enumerate-dir", "dir /b .".to_owned()),
        ("enumerate-where", format!("where /R \"{root_plain}\" *")),
        ("enumerate-attrib", "attrib *".to_owned()),
        // Fails: and System32 carries an explicit ALL APPLICATION PACKAGES
        // read+execute ace, so "not granted" cannot be the explanation.
        (
            "enumerate-system32",
            format!("dir /b \"{system32}\\*.exe\""),
        ),
        // The defect itself, at the product level.
        ("git-init", "git init -q probe-repo".to_owned()),
    ];
    for (name, command) in cases {
        let p = run(&ctx, &command);
        println!(
            "ANC name={name} cmd={command:?} exit={} stdout={:?} stderr={:?}",
            p.exit_code,
            p.stdout.trim(),
            p.stderr.trim()
        );
    }

    control(&ctx, &root, "end");
    let _ = std::fs::remove_dir_all(&root);
}
