//! FerroxLabs/wayland-core#238 c4 — **the measurement the #238 guard rests on,
//! executable.**
//!
//! `is_windows_null_device_name` refuses exactly one spelling, `NUL`, and the
//! justification for refusing that one and no other is a table pasted into a
//! doc comment from a hand-run 2026-08-18 probe on Windows 11 build
//! 10.0.26200.0. A table in a comment cannot notice that the build changed. Two
//! facts in it are load-bearing and neither was ever asserted by anything:
//!
//! 1. **Is bare `NUL` still a device on the build under test?** If a future
//!    build makes it an ordinary file, the guard refuses an addressable file
//!    holding real user data — the precise failure the narrow scope was chosen
//!    to avoid, now caused by the guard instead of by the textbook blocklist.
//! 2. **Does `fs::metadata` report `is_file()` true for it?** This is the half
//!    that decides whether the OTHER guard in `validate_user_path` — the
//!    `NonRegularFile` check, which refuses an existing char device — can see
//!    `NUL` at all. If `is_file()` is true for a character device, that guard
//!    is structurally blind to it and the NAME guard is the only thing standing
//!    between a `Write` and silently discarded bytes. #238's ledger note calls
//!    this out as unmeasured, and it is the reason c4 exists.
//!
//! The probe RECORDS both, prints them as a block that can be pasted straight
//! into the ledger, and then asserts the consequences. It is deliberately not a
//! silent green: run it with `--no-capture` and the measurement is the output.
//!
//! Windows-only by construction. `NUL` is an ordinary, legal file name on Unix,
//! so there is nothing here a Linux or macOS host could answer — and a probe
//! that "passed" on a host that cannot exhibit the property would be the
//! green-from-the-wrong-host mistake this repo has been bitten by before.
//!
//! Run: `cargo nextest run -p wcore-tools --no-capture -E
//! 'binary(windows_nul_device_probe)'` on a Windows host.

#![cfg(windows)]

use std::fs::{self, File};
use std::os::windows::io::AsRawHandle;
use std::path::Path;

use wcore_tools::path_validation::{PathValidationError, validate_user_path};
use windows_sys::Win32::Storage::FileSystem::{FILE_TYPE_CHAR, FILE_TYPE_DISK, GetFileType};

/// What the kernel says a successfully-opened path is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Kind {
    /// `FILE_TYPE_CHAR` — a character device. `NUL`, `CON`, a console handle.
    CharDevice,
    /// `FILE_TYPE_DISK` — an ordinary file on a filesystem.
    DiskFile,
    /// Something else (pipe, unknown), or the raw value if neither constant.
    Other(u32),
    /// The open itself failed; the path names nothing openable.
    Unopenable(std::io::ErrorKind),
}

/// Ask the KERNEL, not the name. `GetFileType` on a live handle is the only
/// answer that cannot be wrong about what the path resolved to — a name-shaped
/// test would just be re-asserting the predicate under test.
fn kernel_kind(path: &Path) -> Kind {
    match File::open(path) {
        Ok(file) => {
            // SAFETY: `file` is alive for the duration of the call, so the raw
            // handle is valid; `GetFileType` neither stores nor closes it.
            let ty = unsafe { GetFileType(file.as_raw_handle() as _) };
            match ty {
                FILE_TYPE_CHAR => Kind::CharDevice,
                FILE_TYPE_DISK => Kind::DiskFile,
                other => Kind::Other(other),
            }
        }
        Err(err) => Kind::Unopenable(err.kind()),
    }
}

/// `is_file()` for a path, or `None` when it cannot be stat-ed at all.
fn stat_is_file(path: &Path) -> Option<bool> {
    fs::metadata(path).ok().map(|m| m.file_type().is_file())
}

/// Best-effort OS build string, so the recorded measurement names the build it
/// was taken on. `cmd /C ver` through the central argv helper — no shell
/// interpretation, no LLM-supplied bytes, and no new dependency feature just to
/// print a version. An unavailable answer is recorded as unknown rather than
/// failing the probe: the two measurements below are the point.
async fn os_build() -> String {
    let out = wcore_config::shell::shell_command_argv("cmd", &["/C", "ver"])
        .output()
        .await;
    match out {
        Ok(o) => {
            let text = String::from_utf8_lossy(&o.stdout);
            let line = text.lines().find(|l| !l.trim().is_empty()).unwrap_or("");
            let trimmed = line.trim();
            if trimmed.is_empty() {
                "unknown".to_string()
            } else {
                trimmed.to_string()
            }
        }
        Err(err) => format!("unknown ({err})"),
    }
}

#[tokio::test]
async fn the_bare_nul_device_is_measured_on_the_build_under_test() {
    let build = os_build().await;

    let dir = tempfile::tempdir().expect("tempdir");
    let root = dir.path();

    // LIVENESS CONTROL. An ordinary file must measure as a disk file and stat
    // as `is_file() == true`. Without it, a probe that reported "NUL is a
    // device" could equally be a probe that calls everything a device.
    let control = root.join("ordinary.txt");
    fs::write(&control, b"real user data\n").expect("write control file");
    let control_kind = kernel_kind(&control);
    let control_stat = stat_is_file(&control);

    // The two spellings that matter. `<dir>\NUL` is the shape a Write actually
    // takes (a leaf inside the workspace); bare `NUL` is the shape the doc
    // table recorded. Win32 resolves the reserved name in ANY directory, so the
    // two are expected to agree — and if they ever stop agreeing, the guard,
    // which looks only at the FINAL COMPONENT, needs to know.
    let in_dir = root.join("NUL");
    let in_dir_kind = kernel_kind(&in_dir);
    let in_dir_stat = stat_is_file(&in_dir);

    let bare = Path::new("NUL");
    let bare_kind = kernel_kind(bare);
    let bare_stat = stat_is_file(bare);

    // BEHAVIOURAL half, the one the 2026-08-18 table recorded: bytes written to
    // a device go nowhere and read back as nothing.
    let write_ok = fs::write(&in_dir, b"bytes-that-must-not-vanish").is_ok();
    let read_back = fs::read(&in_dir).map(|b| b.len());

    println!("----- #238 c4 NUL DEVICE PROBE — RECORD BEGIN -----");
    println!("os build                     : {build}");
    println!("control ordinary.txt kind    : {control_kind:?}");
    println!("control ordinary.txt is_file : {control_stat:?}");
    println!("<dir>\\NUL kernel kind        : {in_dir_kind:?}");
    println!("<dir>\\NUL metadata is_file   : {in_dir_stat:?}");
    println!("bare NUL kernel kind         : {bare_kind:?}");
    println!("bare NUL metadata is_file    : {bare_stat:?}");
    println!(
        "write to <dir>\\NUL reported  : {}",
        if write_ok { "Ok" } else { "Err" }
    );
    println!("read back from <dir>\\NUL     : {read_back:?}");
    println!("----- #238 c4 NUL DEVICE PROBE — RECORD END -------");

    assert_eq!(
        control_kind,
        Kind::DiskFile,
        "instrument is dead: an ordinary file did not measure as FILE_TYPE_DISK"
    );
    assert_eq!(
        control_stat,
        Some(true),
        "instrument is dead: an ordinary file did not stat as is_file()"
    );

    // FACT 1. If this ever flips, the narrow guard has started refusing an
    // addressable name — the data-destroying direction, and the whole reason
    // the textbook blocklist was rejected.
    assert_eq!(
        in_dir_kind,
        Kind::CharDevice,
        "bare NUL is NOT a character device on {build}. \
         The #238 guard refuses this name, so on this build it refuses a file \
         that can hold real user data. Re-scope the guard against a fresh \
         measurement table before shipping."
    );

    // FACT 2. Recorded unconditionally above; asserted here only in the
    // direction that changes what must protect the path. `is_file() == true`
    // for a character device means `validate_user_path`'s `NonRegularFile` arm
    // cannot see it, so the NAME guard is the only thing left.
    if in_dir_stat == Some(true) {
        let err = validate_user_path(&in_dir).expect_err(
            "metadata reports is_file() for the NUL device, so the NonRegularFile \
             guard is blind to it — the name guard MUST refuse the path, and it \
             did not",
        );
        assert!(
            matches!(err, PathValidationError::WindowsNullDevice(_)),
            "the NUL device must be refused BY NAME (NonRegularFile is blind to \
             it on this build), got: {err:?}"
        );
    }

    // The control path must still validate Ok, so the refusal above is the
    // device and not the probe refusing everything it is handed.
    validate_user_path(&control).expect("control failed: an ordinary file must validate Ok");
}
