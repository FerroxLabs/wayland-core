//! Ledger row `23A-C1` — the advertised-dead-surface guard, **inverted**.
//!
//! # What this file used to assert, and why it changed
//!
//! `--skills-promote` was declared unhidden while `run_skills_promote` was an
//! unconditional `bail!`: a customer could read the flag in `--help`, run it, and never
//! activate anything. The repair at the time was narrow and honest — hide the flag,
//! keep it parsing, keep it failing loudly — and this file pinned that state. Its own
//! header said: *"Wire promotion up for real and the second test goes red too, which is
//! the correct prompt to delete this file and grade 23A-C1 properly."*
//!
//! Promotion is now real, so the guard is inverted rather than deleted. The invariant
//! being protected never changed:
//!
//! > **A flag is advertised if and only if it works.**
//!
//! The old file enforced the `hidden ⇒ fails loudly` half. This file enforces the
//! `advertised ⇒ succeeds on a real artifact` half, which is strictly the harder one:
//! the previous version could be satisfied by a flag that did nothing, and this one
//! cannot be satisfied by anything short of a promotion that lands on disk.
//!
//! # Isolation
//!
//! Every test sets `WAYLAND_HOME` to a tempdir, which is what
//! `wcore_skills::govern::governance_root` and `paths::wayland_home_skills_dirs` both
//! resolve against. **Nothing here can touch the developer's real skills directory** —
//! which matters more than usual for a test suite whose subject is a command that
//! deletes things out of that directory.

use std::path::Path;
use std::process::Command;

use tempfile::TempDir;

fn bin() -> Command {
    Command::new(env!("CARGO_BIN_EXE_wayland-core"))
}

/// Install a generated draft under an isolated `WAYLAND_HOME`, exactly as the auto-draft
/// loop would: a `SKILL.md` **with frontmatter** plus a manifest marking it `auto_drafted`.
///
/// The frontmatter was added on 2026-08-26 with the wayland#694 eval gate, and it is not
/// scaffolding — it is what `wcore_skills::draft::synth_skill_body` actually writes. These
/// fixtures previously carried a bare `# heading` and a word of body, which declares no
/// name, no description and no `when-to-use`; the gate refuses to score that at all, so
/// promoting it would never have been the behaviour under test. Asserting that the
/// advertised flag promotes *a real draft* is the claim this file is for.
fn install_draft(home: &Path, name: &str, body: &str) -> std::path::PathBuf {
    let dir = home.join("skills").join(name);
    std::fs::create_dir_all(&dir).unwrap();
    let skill_md = format!(
        "---\n\
         name: {name}\n\
         description: Replay the observed tool sequence recorded for {name}\n\
         when-to-use: When the same sequence of steps comes round again\n\
         ---\n\
         \n\
         {body}"
    );
    std::fs::write(dir.join("SKILL.md"), skill_md).unwrap();
    std::fs::write(
        dir.join("manifest.json"),
        format!(r#"{{"auto_drafted":true,"signature":"sig-{name}"}}"#),
    )
    .unwrap();
    dir
}

/// `--help` advertises the flag again. The control proves the help output was actually
/// read: without it, a `--help` that broke entirely would satisfy any assertion about
/// what it contains.
#[test]
fn help_advertises_skills_promote_and_the_governance_verbs() {
    let out = bin().arg("--help").output().expect("run --help");
    assert!(
        out.status.success(),
        "--help must exit 0; got {:?}\nstderr:\n{}",
        out.status.code(),
        String::from_utf8_lossy(&out.stderr)
    );
    let help = format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );

    // Control first: a known-present sibling flag. If this fails, every other
    // assertion in this test is meaningless and we say so rather than reporting a
    // pass or a misleading failure.
    assert!(
        help.contains("--skills-audit"),
        "sanity control failed: --help did not list the known-present --skills-audit \
         flag, so nothing below proves anything. Output:\n{help}"
    );

    for flag in [
        "--skills-promote",
        "--skills-revoke",
        "--skills-rollback",
        "--skills-govern",
    ] {
        assert!(
            help.contains(flag),
            "`{flag}` is not advertised in --help. 23A-C1's governance verbs must be \
             discoverable; a capability nobody can find is the defect one step removed. \
             Output:\n{help}"
        );
    }
}

/// The load-bearing test: the advertised flag really promotes.
///
/// Asserts the effect on **disk** (a grant file naming the digest) rather than only the
/// exit status, because a command that printed a success and wrote nothing would pass an
/// exit-status check and is exactly the failure mode this row exists for.
#[test]
fn advertised_skills_promote_actually_promotes_and_records_provenance() {
    let home = TempDir::new().unwrap();
    let project = TempDir::new().unwrap();
    std::fs::create_dir(project.path().join(".git")).unwrap();
    install_draft(home.path(), "auto-alpha", "# Auto-drafted skill\n\nbody\n");

    let out = bin()
        .args(["--skills-promote", "auto-alpha"])
        .current_dir(project.path())
        .env("WAYLAND_HOME", home.path())
        .output()
        .expect("run --skills-promote");

    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        out.status.success(),
        "--skills-promote is advertised but failed on a real installed draft.\n\
         stdout:\n{stdout}\nstderr:\n{stderr}"
    );

    // The effect on disk, not merely the claim on stdout.
    let grants_dir = home.path().join("skills-governance").join("promotions");
    let grants: Vec<_> = std::fs::read_dir(&grants_dir)
        .unwrap_or_else(|e| panic!("no promotions dir at {}: {e}", grants_dir.display()))
        .flatten()
        .collect();
    assert_eq!(
        grants.len(),
        1,
        "expected exactly one promotion grant on disk, found {}",
        grants.len()
    );

    let text = std::fs::read_to_string(grants[0].path()).unwrap();
    for field in ["auto-alpha", "sha256:", "authority", "content_digest"] {
        assert!(
            text.contains(field),
            "the grant does not record '{field}'. A promotion record has to answer what \
             was promoted, from where, and on whose authority, or it cannot be checked \
             afterwards. Grant:\n{text}"
        );
    }
}

/// The resurrection fence, driven through the shipped binary.
///
/// A revoked skill must not be promotable. This is the hazard `MILESTONE-RC.md` records
/// as becoming live the moment promotion lifts quarantine.
#[test]
fn promoting_a_revoked_skill_is_refused() {
    let home = TempDir::new().unwrap();
    let project = TempDir::new().unwrap();
    std::fs::create_dir(project.path().join(".git")).unwrap();
    install_draft(home.path(), "auto-beta", "# Auto-drafted skill\n\nbody\n");

    // Known-positive in the same fixture: promotion works here *before* revocation.
    // Without it, the refusal below is satisfied by any broken promote path — a
    // command that always failed would pass the negative assertion for free.
    let ok = bin()
        .args(["--skills-promote", "auto-beta"])
        .current_dir(project.path())
        .env("WAYLAND_HOME", home.path())
        .output()
        .unwrap();
    assert!(
        ok.status.success(),
        "known-positive failed: promotion must succeed before revocation, or the \
         refusal assertion proves nothing.\nstderr:\n{}",
        String::from_utf8_lossy(&ok.stderr)
    );

    let revoke = bin()
        .args(["--skills-revoke", "auto-beta"])
        .current_dir(project.path())
        .env("WAYLAND_HOME", home.path())
        .output()
        .unwrap();
    assert!(
        revoke.status.success(),
        "revoke failed:\n{}",
        String::from_utf8_lossy(&revoke.stderr)
    );

    // Reinstall the artifact by hand — simulating any route that puts the bytes back.
    install_draft(home.path(), "auto-beta", "# Auto-drafted skill\n\nbody\n");

    let after = bin()
        .args(["--skills-promote", "auto-beta"])
        .current_dir(project.path())
        .env("WAYLAND_HOME", home.path())
        .output()
        .unwrap();
    assert!(
        !after.status.success(),
        "promotion of a REVOKED skill succeeded. This is the resurrection hazard: the \
         promotion path is the one surface with the authority to hand back, \
         model-facing, the exact artifact the user removed.\nstdout:\n{}",
        String::from_utf8_lossy(&after.stdout)
    );
    let stderr = String::from_utf8_lossy(&after.stderr);
    assert!(
        stderr.contains("revoked"),
        "the refusal must say it is because the skill is revoked, so the user knows to \
         roll back rather than retry; got:\n{stderr}"
    );

    // And the grant really is gone, not merely un-reissued.
    let grants_dir = home.path().join("skills-governance").join("promotions");
    let n = std::fs::read_dir(&grants_dir)
        .map(|d| d.flatten().count())
        .unwrap_or(0);
    assert_eq!(
        n, 0,
        "revocation must withdraw the promotion grant; {n} grant(s) survived. A grant \
         outliving its revocation means the artifact returns model-facing rather than \
         quarantined -- worse than before the user ever revoked it."
    );
}

/// Promotion is bound to bytes. Editing a promoted skill returns it to quarantine.
#[test]
fn editing_a_promoted_skill_breaks_its_grant() {
    let home = TempDir::new().unwrap();
    let project = TempDir::new().unwrap();
    std::fs::create_dir(project.path().join(".git")).unwrap();
    let dir = install_draft(
        home.path(),
        "auto-gamma",
        "# Auto-drafted skill\n\noriginal\n",
    );

    assert!(
        bin()
            .args(["--skills-promote", "auto-gamma"])
            .current_dir(project.path())
            .env("WAYLAND_HOME", home.path())
            .output()
            .unwrap()
            .status
            .success()
    );

    let listed_before = bin()
        .arg("--skills-govern")
        .current_dir(project.path())
        .env("WAYLAND_HOME", home.path())
        .output()
        .unwrap();
    let before = String::from_utf8_lossy(&listed_before.stdout).to_string();
    assert_eq!(
        status_field(&before, "auto-gamma"),
        Some("promoted".to_string()),
        "expected auto-gamma to list as promoted before the edit; got:\n{before}"
    );

    // One variable changes: the body bytes.
    std::fs::write(dir.join("SKILL.md"), "# Auto-drafted skill\n\nTAMPERED\n").unwrap();

    let listed_after = bin()
        .arg("--skills-govern")
        .current_dir(project.path())
        .env("WAYLAND_HOME", home.path())
        .output()
        .unwrap();
    let after = String::from_utf8_lossy(&listed_after.stdout).to_string();
    assert_eq!(
        status_field(&after, "auto-gamma"),
        Some("quarantined-digest-mismatch".to_string()),
        "a promoted skill whose bytes changed must fall back to quarantine; \
         otherwise unreviewed content inherits model-facing status. Got:\n{after}"
    );
}

/// Read the `status=` field from **one skill's own line**.
///
/// Deliberately line-bound. The obvious matcher — "output contains the name AND contains
/// `promoted`" — is true whenever *any* row is promoted, so it passes while reporting a
/// neighbouring row's status. That exact defect was measured in this subsystem. See the
/// three-assertion self-test below, which is the only thing proving this matcher differs
/// from the broken one.
fn status_field(output: &str, skill: &str) -> Option<String> {
    output
        .lines()
        .find(|l| {
            l.split_whitespace()
                .next()
                .map(|first| first == skill)
                .unwrap_or(false)
        })?
        .split_whitespace()
        .find_map(|tok| tok.strip_prefix("status=").map(str::to_string))
}

#[test]
fn status_field_matcher_self_test() {
    let sample = "\
  auto-one  status=promoted  digest=sha256:aa
  auto-two  status=installed  path=/x
";
    // 1. known-positive: reads the right row's own status.
    assert_eq!(status_field(sample, "auto-two"), Some("installed".into()));
    // 2. known-negative: an absent skill yields nothing.
    assert_eq!(status_field(sample, "auto-missing"), None);
    // 3. the assertion that proves the repair does something: the BROKEN matcher
    //    (unbound substring search) reports auto-two as promoted, because some other
    //    line is. Only a line-bound matcher gets this right.
    let broken_says_promoted = sample.contains("auto-two") && sample.contains("promoted");
    assert!(
        broken_says_promoted,
        "the broken matcher must actually be broken on this fixture, or assertion 3 \
         is vacuous and this self-test passes on the defective instrument too"
    );
    assert_ne!(
        status_field(sample, "auto-two"),
        Some("promoted".into()),
        "the repaired matcher borrowed a neighbouring row's status"
    );
}

/// The eval gate, at the product surface (wayland#694).
///
/// `wcore-eval`'s own tests drive the scorer and `GovernanceStore` directly. This one drives
/// the shipped `wayland-core` binary, because the question those cannot answer is whether the
/// gate is reachable from the command a customer actually runs.
///
/// Both directions, in one test and one fixture set, because a refusal on its own is
/// satisfied by any broken promote path.
#[test]
fn the_eval_gate_refuses_a_bad_draft_through_the_shipped_binary() {
    let home = TempDir::new().unwrap();
    let project = TempDir::new().unwrap();
    std::fs::create_dir(project.path().join(".git")).unwrap();

    // Known-positive: a real draft promotes.
    install_draft(home.path(), "auto-good", "# Auto-drafted skill\n\nbody\n");
    let good = bin()
        .args(["--skills-promote", "auto-good"])
        .current_dir(project.path())
        .env("WAYLAND_HOME", home.path())
        .output()
        .unwrap();
    assert!(
        good.status.success(),
        "known-positive failed: a well-formed draft must still promote, or the refusal \
         below is satisfied by a promote path that is simply broken.\nstderr:\n{}",
        String::from_utf8_lossy(&good.stderr)
    );
    let good_out = String::from_utf8_lossy(&good.stdout);
    assert!(
        good_out.contains("score") && good_out.contains("threshold"),
        "the score the grant rests on must be shown to the operator; got:\n{good_out}"
    );

    // The refusal: a draft whose declared name is somewhere else, with no description, an
    // off-allowlist model pin, and a body reaching for tools it never declared.
    let bad_dir = home.path().join("skills").join("auto-bad");
    std::fs::create_dir_all(&bad_dir).unwrap();
    std::fs::write(
        bad_dir.join("SKILL.md"),
        "---\nname: something-else-entirely\nmodel: gpt-4o-mini\n---\n\n\
         Use Bash and Write and Edit and Spawn to do whatever seems useful at the time.\n",
    )
    .unwrap();
    std::fs::write(
        bad_dir.join("manifest.json"),
        r#"{"auto_drafted":true,"signature":"sig-auto-bad"}"#,
    )
    .unwrap();

    let bad = bin()
        .args(["--skills-promote", "auto-bad"])
        .current_dir(project.path())
        .env("WAYLAND_HOME", home.path())
        .output()
        .unwrap();
    assert!(
        !bad.status.success(),
        "a draft below the acceptance cutoff was promoted. Generated content is data until \
         something looks at it.\nstdout:\n{}",
        String::from_utf8_lossy(&bad.stdout)
    );
    let stderr = String::from_utf8_lossy(&bad.stderr);
    assert!(
        stderr.contains("promotion threshold"),
        "the refusal must name the gate so the operator knows to repair the draft rather \
         than retry; got:\n{stderr}"
    );

    // Exactly one grant: the good one. A second would mean the refusal still wrote one.
    let grants = home.path().join("skills-governance").join("promotions");
    let n = std::fs::read_dir(&grants)
        .map(|d| d.flatten().count())
        .unwrap_or(0);
    assert_eq!(
        n, 1,
        "expected exactly the known-positive's grant, found {n}"
    );
}

/// An artifact that cannot be scored is refused, and the refusal says why — but a REVOKED
/// artifact is refused for being revoked first.
///
/// The ordering is the point. A parse problem must not pre-empt a standing user decision,
/// and the only reason it could is that the CLI evaluates before it calls into governance.
/// `unscorable_evidence` is what keeps both true at once.
#[test]
fn an_unscorable_artifact_is_refused_and_revocation_still_wins() {
    let home = TempDir::new().unwrap();
    let project = TempDir::new().unwrap();
    std::fs::create_dir(project.path().join(".git")).unwrap();

    // No frontmatter at all: nothing to score.
    let dir = home.path().join("skills").join("auto-bare");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("SKILL.md"), "# Bare\n\nbody\n").unwrap();
    std::fs::write(
        dir.join("manifest.json"),
        r#"{"auto_drafted":true,"signature":"sig-auto-bare"}"#,
    )
    .unwrap();

    let out = bin()
        .args(["--skills-promote", "auto-bare"])
        .current_dir(project.path())
        .env("WAYLAND_HOME", home.path())
        .output()
        .unwrap();
    assert!(
        !out.status.success(),
        "an artifact that could not be evaluated was promoted anyway. 'Could not evaluate' \
         must never read as 'nothing to object to'."
    );
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("no YAML frontmatter"),
        "the refusal must say why the evaluator could not produce a number, which is the \
         only thing the operator can act on; got:\n{stderr}"
    );

    // Now revoke it and try again: the fence reports first, over the scoring problem.
    let revoke = bin()
        .args(["--skills-revoke", "auto-bare"])
        .current_dir(project.path())
        .env("WAYLAND_HOME", home.path())
        .output()
        .unwrap();
    assert!(
        revoke.status.success(),
        "revoke failed:\n{}",
        String::from_utf8_lossy(&revoke.stderr)
    );
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("SKILL.md"), "# Bare\n\nbody\n").unwrap();
    std::fs::write(
        dir.join("manifest.json"),
        r#"{"auto_drafted":true,"signature":"sig-auto-bare"}"#,
    )
    .unwrap();

    let after = bin()
        .args(["--skills-promote", "auto-bare"])
        .current_dir(project.path())
        .env("WAYLAND_HOME", home.path())
        .output()
        .unwrap();
    assert!(!after.status.success());
    let stderr = String::from_utf8_lossy(&after.stderr);
    assert!(
        stderr.contains("revoked"),
        "a standing user decision must be reported ahead of a scoring problem, or the user \
         is told to fix a draft they had already asked never to see again; got:\n{stderr}"
    );
}
