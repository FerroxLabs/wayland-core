//! F23A-C1-H3 kill harness: is a skill rollback atomic under a real SIGKILL?
//!
//! An `examples/` binary, not a test and not a `src/bin`, because it must be killed from the
//! outside mid-syscall. `cargo test` cannot express that, and shipping another dev binary out
//! of `src/bin` would repeat the mistake this phase is closing — a governance capability whose
//! only surface is a bin nobody installs.
//!
//! # The claim under test
//!
//! After a SIGKILL landing anywhere inside a restore, the user's skill directory is either
//! **wholly absent** (the pre-rollback state) or **wholly restored**. Never partial.
//!
//! # Why this harness can fail
//!
//! Three separate ways, all load-bearing (LANE-BRIEF §3b-i — a known-negative on a dead
//! instrument passes for free):
//!
//! 1. **`--mode legacy` is a reddening control.** It performs the *pre-fix* restore: a bare
//!    recursive copy straight into the live directory. Same payload, same kill schedule. If
//!    the harness cannot produce `PARTIAL` there, it cannot detect `PARTIAL` anywhere, and
//!    the driver treats a legacy run with zero partials as an INVALID MEASUREMENT rather
//!    than as a pass.
//! 2. **Markers are files, not stdout.** `marks/BEGIN` and `marks/DONE` are created with
//!    syscalls, so a SIGKILL cannot lose them the way it loses a buffered `println!`. Exit
//!    status is never consulted: a killed process's status tells you nothing about where in
//!    the restore it died.
//! 3. **Kills outside the window are excluded, not counted as passes.** A trial that never
//!    reached `BEGIN`, or that reached `DONE` before the kill, proves nothing about
//!    atomicity. Only `BEGIN && !DONE` trials count, and the driver requires a non-zero
//!    number of them.
//!
//! # Grading
//!
//! `grade` compares the live directory against a manifest captured before the revoke: every
//! expected path, at its exact byte length, and no extras.
//!
//! - `ABSENT`  — the directory is not there. The pre-rollback state. Recoverable by retry.
//! - `WHOLE`   — every file present at its exact size, nothing extra.
//! - `PARTIAL` — present but not whole. **This is the defect.**
//!
//! A staging tree left behind by a kill is reported separately as `STAGED=1`, and whether it
//! holds a discoverable `SKILL.md` as `STAGEDSKILL=1`. That second field is the F23A-C1-H4
//! measurement: for the namespaced layout this harness uses, staging lands INSIDE the skills
//! root, so a leftover tree with a `SKILL.md` in it is a half-built skill sitting where the
//! loader walks. It is not a partial *target* directory -- `GRADE` covers that -- and it is
//! fenced by name in `loader::collect_skill_md`, but the harness reports it rather than
//! assuming the fence holds.

use std::io::Write;
use std::path::{Path, PathBuf};

use wcore_skills::govern::GovernanceStore;

/// ~20 MB across enough files that the copy is comfortably longer than the kill jitter, and
/// safely under `MAX_SNAPSHOT_BYTES` (32 MB) so `revoke` retains the whole draft.
const FILES: usize = 320;
const FILE_BYTES: usize = 64 * 1024;
const SKILL_NAME: &str = "auto-killtest";
/// `promote::STAGING`, which is crate-private. Duplicated as a literal so a rename breaks
/// this harness loudly instead of silently making it measure nothing.
const STAGING_DIR: &str = ".promote-staging";

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let usage = "usage: f23a_c1_kill_restore <prepare|restore|grade> <root> [id] [atomic|legacy]";
    let cmd = args.get(1).map(String::as_str).unwrap_or("");
    let root = PathBuf::from(args.get(2).unwrap_or_else(|| panic!("{usage}")));

    match cmd {
        "prepare" => prepare(&root),
        "restore" => restore(
            &root,
            args.get(3).unwrap_or_else(|| panic!("{usage}")),
            args.get(4).map(String::as_str).unwrap_or("atomic"),
        ),
        "grade" => grade(&root),
        other => {
            eprintln!("unknown sub-command '{other}'. {usage}");
            std::process::exit(64);
        }
    }
}

fn skills_dir(root: &Path) -> PathBuf {
    root.join("skills")
}
/// The **namespaced** layout the auto-skill drafter actually writes
/// (`$WAYLAND_HOME/skills/auto/auto-<sig>/`). Deliberately not a flat `<root>/<name>`: for a
/// flat skill `promote::staging_root_for` puts staging outside the skills tree, and this
/// harness would then never exercise the F23A-C1-H4 path at all.
fn target_dir(root: &Path) -> PathBuf {
    skills_dir(root).join("auto").join(SKILL_NAME)
}

/// Where `rollback` stages a restore of a namespaced skill: `<skills_root>/.promote-staging`.
fn staging_root(root: &Path) -> PathBuf {
    skills_dir(root).join(STAGING_DIR)
}
fn store(root: &Path) -> GovernanceStore {
    GovernanceStore::new(root.join("governance"))
}

/// Build a draft, capture its exact shape, then revoke it through the real product path so
/// the payload under test is one `revoke` actually wrote.
fn prepare(root: &Path) {
    let dir = target_dir(root);
    std::fs::create_dir_all(dir.join("refs")).unwrap();
    std::fs::write(
        dir.join("SKILL.md"),
        "---\nname: auto-killtest\n---\nbody\n",
    )
    .unwrap();
    std::fs::write(
        dir.join("manifest.json"),
        br#"{"auto_drafted":true,"signature":"kill-sig"}"#,
    )
    .unwrap();
    for i in 0..FILES {
        // Deterministic, incompressible-enough filler. Content is not compared byte-wise --
        // length is the cheap invariant and a truncated copy is exactly what a kill produces.
        let buf = vec![(i % 251) as u8; FILE_BYTES];
        std::fs::write(dir.join("refs").join(format!("r{i:04}.bin")), &buf).unwrap();
    }

    let manifest = walk(&dir, &dir);
    let mut f = std::fs::File::create(root.join("expected.tsv")).unwrap();
    for (rel, len) in &manifest {
        writeln!(f, "{}\t{}", rel.display(), len).unwrap();
    }
    f.sync_all().unwrap();

    let rec = store(root).revoke(&dir).expect("revoke failed in prepare");
    assert!(!dir.exists(), "prepare: revoke left the directory behind");
    std::fs::write(root.join("id.txt"), &rec.revocation_id).unwrap();
    println!("PREPARED id={} files={}", rec.revocation_id, manifest.len());
}

fn restore(root: &Path, id: &str, mode: &str) {
    let marks = root.join("marks");
    std::fs::create_dir_all(&marks).unwrap();
    // Created immediately before the first byte moves, and fsync'd, so its presence is a
    // fact about the kernel rather than about a userspace buffer.
    let begin = std::fs::File::create(marks.join("BEGIN")).unwrap();
    begin.sync_all().unwrap();

    match mode {
        "atomic" => {
            store(root).rollback(id).expect("rollback failed");
        }
        "legacy" => {
            // The pre-fix implementation, reproduced verbatim in shape: create the live
            // directory, then copy into it file by file. This is the control -- it exists to
            // prove the grader can see a partial state at all.
            let payload = root
                .join("governance")
                .join("generations")
                .join(id)
                .join("payload");
            legacy_copy(&payload, &target_dir(root));
        }
        other => {
            eprintln!("unknown mode '{other}'");
            std::process::exit(64);
        }
    }

    let done = std::fs::File::create(marks.join("DONE")).unwrap();
    done.sync_all().unwrap();
    println!("RESTORED mode={mode}");
}

fn legacy_copy(from: &Path, to: &Path) {
    std::fs::create_dir_all(to).unwrap();
    for entry in std::fs::read_dir(from).unwrap() {
        let entry = entry.unwrap();
        let src = entry.path();
        let dst = to.join(entry.file_name());
        if src.is_dir() {
            legacy_copy(&src, &dst);
        } else {
            std::fs::copy(&src, &dst).unwrap();
        }
    }
}

fn grade(root: &Path) {
    let expected: Vec<(PathBuf, u64)> = std::fs::read_to_string(root.join("expected.tsv"))
        .expect("no expected.tsv -- prepare did not run")
        .lines()
        .filter(|l| !l.trim().is_empty())
        .map(|l| {
            let (p, n) = l.split_once('\t').expect("malformed expected.tsv line");
            (PathBuf::from(p), n.parse::<u64>().unwrap())
        })
        .collect();
    assert!(
        !expected.is_empty(),
        "expected.tsv is empty -- the grader would call any state WHOLE"
    );

    let marks = root.join("marks");
    let began = marks.join("BEGIN").exists();
    let done = marks.join("DONE").exists();

    // Is a staging tree left behind, and does it hold a discoverable `SKILL.md`? The second
    // question is the F23A-C1-H4 one: a leftover empty directory is litter, a leftover tree
    // with a SKILL.md in it is a half-built skill sitting inside the loader's walk.
    let staged = staging_root(root).is_dir();
    let staged_skill = staging_root(root)
        .read_dir()
        .map(|rd| rd.flatten().any(|e| e.path().join("SKILL.md").is_file()))
        .unwrap_or(false);

    let dir = target_dir(root);
    let state = if !dir.exists() {
        "ABSENT"
    } else {
        let actual = walk(&dir, &dir);
        let mut whole = actual.len() == expected.len();
        if whole {
            for (rel, len) in &expected {
                match std::fs::metadata(dir.join(rel)) {
                    Ok(m) if m.len() == *len => {}
                    _ => {
                        whole = false;
                        break;
                    }
                }
            }
        }
        if whole { "WHOLE" } else { "PARTIAL" }
    };

    // One line, all fields, parsed by the driver. Written to stdout by a process that is not
    // being killed, so it is safe here in a way it is not inside `restore`.
    println!(
        "GRADE={state} BEGAN={} DONE={} STAGED={} STAGEDSKILL={}",
        began as u8, done as u8, staged as u8, staged_skill as u8
    );
}

/// Relative path + byte length of every regular file under `dir`.
fn walk(base: &Path, dir: &Path) -> Vec<(PathBuf, u64)> {
    let mut out = Vec::new();
    let Ok(rd) = std::fs::read_dir(dir) else {
        return out;
    };
    for entry in rd.flatten() {
        let p = entry.path();
        let Ok(meta) = std::fs::symlink_metadata(&p) else {
            continue;
        };
        if meta.is_dir() {
            out.extend(walk(base, &p));
        } else {
            out.push((p.strip_prefix(base).unwrap_or(&p).to_path_buf(), meta.len()));
        }
    }
    out.sort();
    out
}
