//! 23A-C1 live drive: exercises the real `wcore-skill-govern` binary end to end.
//!
//! Not a unit test of the library — this spawns the shipped executable and reads its actual
//! stdout, because a phase is not done because its suite is green (LANE-BRIEF §3.1). The
//! whole journey a user takes is driven here: see the draft, revoke it, confirm it is gone
//! and suppressed, roll it back, confirm it returned byte-identical.
//!
//! Hermetic by construction: `WAYLAND_HOME` is pinned to a tempdir for every child, so the
//! child resolves both its skills directory and its governance root inside that tempdir and
//! can never read or mutate the developer's real profile.

use std::path::{Path, PathBuf};
use std::process::Command;

const BIN: &str = env!("CARGO_BIN_EXE_wcore-skill-govern");

// ---------------------------------------------------------------------------
// The matcher, and the reason it is shaped this way
// ---------------------------------------------------------------------------

/// Read one `key=value` field from the rendered line belonging to `name`.
///
/// **Bound to a single line, and to that line's own fields.** The tempting form is
/// `out.contains(name) && out.contains("status=revoked")`: two UNBOUND substring searches
/// over the whole listing. That pair is true whenever *any* row is revoked, so it cannot
/// distinguish the row it names from a neighbour — it reports "this skill is revoked" while
/// holding a skill that is present. That exact defect was measured in this subsystem's
/// `/skill list` quarantine check (`wcore-eval-scenarios/tests/f23a_boundary_drive.rs`),
/// where the unbound form reported a hidden tag while holding a model-visible skill.
///
/// Returns `None` when no line names `name`, which is itself a meaningful answer (absent).
fn field(out: &str, name: &str, key: &str) -> Option<String> {
    let prefix = format!("{key}=");
    out.lines()
        .map(str::trim)
        .find(|line| line.split_whitespace().next() == Some(name))
        .and_then(|line| {
            line.split_whitespace()
                .find_map(|tok| tok.strip_prefix(&prefix).map(str::to_string))
        })
}

/// Self-test for the matcher. THREE assertions: two would pass on the broken form too, so
/// the third — "the old matcher would have missed it" — is the only one proving the shape
/// does any work.
#[test]
fn field_matcher_selftest() {
    // A listing where the two rows carry OPPOSITE statuses. This is the shape that breaks
    // an unbound matcher and the only shape that can prove a bound one.
    let rendered = "\
governance root: /tmp/x

ON DISK (1)
  auto-present  status=present  kind=auto-drafted  path=/tmp/x/skills/auto-present

REVOKED (1)
  auto-gone  status=revoked  id=abc-123  files=2  bytes=99
";

    // 1. Known-positive: the revoked row reports its own status.
    assert_eq!(
        field(rendered, "auto-gone", "status").as_deref(),
        Some("revoked"),
        "must read the revoked row's own status"
    );

    // 2. Known-negative: the present row reports "present", even though the string
    //    "status=revoked" appears elsewhere in the very same output.
    assert_eq!(
        field(rendered, "auto-present", "status").as_deref(),
        Some("present"),
        "must NOT borrow the other row's status"
    );

    // 3. The old, unbound matcher WOULD have missed it. Without this assertion the
    //    self-test passes on the broken matcher too and proves nothing.
    let old_matcher = |out: &str, name: &str| out.contains(name) && out.contains("status=revoked");
    assert!(
        old_matcher(rendered, "auto-present"),
        "the unbound form reports 'auto-present is revoked' on this input -- which is FALSE, \
         and is precisely the defect the bound form removes"
    );
    assert_ne!(
        field(rendered, "auto-present", "status").as_deref(),
        Some("revoked"),
        "the bound form must reach the opposite (correct) conclusion on the same bytes"
    );
}

// ---------------------------------------------------------------------------
// Harness
// ---------------------------------------------------------------------------

struct Drive {
    _tmp: tempfile::TempDir,
    home: PathBuf,
    skills: PathBuf,
}

fn drive() -> Drive {
    let tmp = tempfile::tempdir().unwrap();
    let home = tmp.path().to_path_buf();
    let skills = home.join("skills");
    std::fs::create_dir_all(&skills).unwrap();
    Drive {
        _tmp: tmp,
        home,
        skills,
    }
}

impl Drive {
    /// Run the real binary with a pinned `WAYLAND_HOME`. Returns `(stdout, stderr, code)`.
    fn run(&self, args: &[&str]) -> (String, String, i32) {
        let out = Command::new(BIN)
            .args(args)
            .env("WAYLAND_HOME", &self.home)
            // Remove the overrides that would otherwise let a developer's environment
            // redirect the child out of the tempdir and make the run non-hermetic.
            .env_remove("WAYLAND_SKILLS_GOVERNANCE_DIR")
            .env_remove("XDG_DATA_HOME")
            .output()
            .expect("failed to spawn wcore-skill-govern");
        (
            String::from_utf8_lossy(&out.stdout).into_owned(),
            String::from_utf8_lossy(&out.stderr).into_owned(),
            out.status.code().unwrap_or(-1),
        )
    }

    fn write_draft(&self, name: &str, signature: &str, body: &str) -> PathBuf {
        let dir = self.skills.join(name);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("SKILL.md"), body).unwrap();
        std::fs::write(
            dir.join("manifest.json"),
            serde_json::to_vec_pretty(&serde_json::json!({
                "name": name, "auto_drafted": true, "signature": signature,
                "evidence_count": 3, "needs_review": true,
            }))
            .unwrap(),
        )
        .unwrap();
        dir
    }

    fn write_user_skill(&self, name: &str, body: &str) -> PathBuf {
        let dir = self.skills.join(name);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("SKILL.md"), body).unwrap();
        dir
    }
}

/// Pull a revocation id out of `revoke`'s own output.
fn revocation_id(stdout: &str) -> String {
    stdout
        .lines()
        .find_map(|l| l.trim().strip_prefix("revocation id: "))
        .map(str::trim)
        .expect("revoke must print a revocation id")
        .to_string()
}

fn read(p: &Path) -> String {
    std::fs::read_to_string(p).unwrap()
}

// ---------------------------------------------------------------------------
// The drive
// ---------------------------------------------------------------------------

#[test]
fn full_revoke_and_rollback_journey_through_the_real_binary() {
    let d = drive();
    let body = "# Auto-drafted skill: auto-live\n\nthese exact bytes must survive\n";
    let draft = d.write_draft("auto-live", "live-sig", body);
    // A user-authored control that must be untouched throughout. Without it, a binary that
    // revoked everything would pass every assertion below.
    let control = d.write_user_skill("my-own-skill", "hand written\n");

    // --- 1. observe -------------------------------------------------------
    let (out, err, code) = d.run(&["list"]);
    assert_eq!(code, 0, "list failed: {err}");
    assert_eq!(
        field(&out, "auto-live", "status").as_deref(),
        Some("present"),
        "the draft must be observable before revocation.\n{out}"
    );
    assert_eq!(
        field(&out, "auto-live", "kind").as_deref(),
        Some("auto-drafted")
    );
    assert_eq!(
        field(&out, "auto-live", "signature").as_deref(),
        Some("live-sig")
    );
    assert_eq!(
        field(&out, "my-own-skill", "kind").as_deref(),
        Some("user-authored"),
        "the control must be distinguished from a draft.\n{out}"
    );

    // --- 2. revoke --------------------------------------------------------
    let (out, err, code) = d.run(&["revoke", "auto-live"]);
    assert_eq!(code, 0, "revoke failed: {err}");
    let id = revocation_id(&out);
    assert!(
        !draft.exists(),
        "revoke must remove it from the user's directory"
    );
    assert!(
        control.exists(),
        "revoke must not touch the user's own skill"
    );

    // --- 3. the revocation is visible and bound to its own row ------------
    let (out, _, code) = d.run(&["list"]);
    assert_eq!(code, 0);
    assert_eq!(
        field(&out, "auto-live", "status").as_deref(),
        Some("revoked"),
        "the draft's own row must report revoked.\n{out}"
    );
    assert_eq!(
        field(&out, "my-own-skill", "status").as_deref(),
        Some("present"),
        "the control's own row must still report present -- this is the assertion an \
         unbound matcher gets wrong.\n{out}"
    );
    assert_eq!(
        field(&out, "auto-live", "files").as_deref(),
        Some("2"),
        "both files must be retained.\n{out}"
    );

    // --- 4. history ------------------------------------------------------
    let (out, _, code) = d.run(&["history"]);
    assert_eq!(code, 0);
    assert_eq!(
        out.lines().filter(|l| l.contains("REVOKED")).count(),
        1,
        "exactly one revocation must be journalled.\n{out}"
    );

    // --- 5. rollback ------------------------------------------------------
    let (_, err, code) = d.run(&["rollback", &id]);
    assert_eq!(code, 0, "rollback failed: {err}");
    assert!(draft.exists(), "rollback must restore the directory");
    assert_eq!(
        read(&draft.join("SKILL.md")),
        body,
        "rollback must restore the exact prior bytes"
    );

    // --- 6. the revocation is gone from the live set ----------------------
    let (out, _, _) = d.run(&["list"]);
    assert_eq!(
        field(&out, "auto-live", "status").as_deref(),
        Some("present"),
        "after rollback the draft must be present again, not revoked.\n{out}"
    );
}

#[test]
fn revoking_an_unknown_skill_fails_loudly_rather_than_reporting_success() {
    let d = drive();
    let (_, err, code) = d.run(&["revoke", "no-such-skill"]);
    assert_ne!(code, 0, "a revoke that did nothing must not exit 0");
    assert!(
        err.contains("no skill named"),
        "the user must be told why: {err}"
    );
}

#[test]
fn rollback_of_an_unknown_id_fails_loudly() {
    let d = drive();
    let (_, err, code) = d.run(&["rollback", "not-a-real-id"]);
    assert_ne!(code, 0);
    assert!(err.contains("no revocation"), "{err}");
}

#[test]
fn an_empty_profile_lists_cleanly_and_exits_zero() {
    let d = drive();
    let (out, err, code) = d.run(&["list"]);
    assert_eq!(code, 0, "{err}");
    assert!(out.contains("ON DISK (0)"), "{out}");
    assert!(out.contains("REVOKED (0)"), "{out}");
}

#[test]
fn an_unknown_verb_is_rejected() {
    let d = drive();
    let (_, err, code) = d.run(&["frobnicate"]);
    assert_ne!(code, 0, "an unknown verb must not exit 0");
    assert!(err.contains("unknown command"), "{err}");
}
