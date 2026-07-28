//! Regression guard for ledger row `23A-C1` — an advertised flag that always
//! fails.
//!
//! `--skills-promote <PROCEDURE_ID>` was declared unhidden, so it appeared in
//! the shipped binary's `--help` with a docstring describing exactly what it
//! would do, while `run_skills_promote` is an unconditional `bail!`. A customer
//! could draft a skill and never activate it.
//!
//! **This does not close `23A-C1`.** The criterion requires governed promotion
//! plus observe/revoke/rollback; none of that exists. The repair here is
//! narrower and honest: stop *advertising* a surface the product cannot
//! deliver. The criterion stays open.
//!
//! Both halves are pinned, and each can fail independently:
//!
//! * Drop `hide = true` and `help_does_not_advertise_skills_promote` goes red.
//! * Delete the flag outright (rather than hiding it) and
//!   `skills_promote_still_fails_loudly_when_invoked` goes red, because clap
//!   would answer with `unexpected argument` instead of the deliberate
//!   governed-promotion message — a silent change of contract for anyone who
//!   already scripted the flag.
//! * Wire promotion up for real and the second test goes red too, which is the
//!   correct prompt to delete this file and grade `23A-C1` properly.
//!
//! These drive the REAL built binary (`CARGO_BIN_EXE_wayland-core`), not a
//! reconstructed `clap::Command`, so what is asserted is what ships.

use std::process::Command;

fn bin() -> Command {
    Command::new(env!("CARGO_BIN_EXE_wayland-core"))
}

#[test]
fn help_does_not_advertise_skills_promote() {
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

    assert!(
        !help.contains("--skills-promote"),
        "`--skills-promote` is advertised in --help but its implementation is a \
         bail!(). Either it now works (delete this guard and grade 23A-C1) or the \
         `hide = true` was dropped. Help output:\n{help}"
    );

    // Control: the surrounding, genuinely-implemented sibling flag IS still
    // advertised. Without this, a --help that broke entirely (or an output
    // stream we read from the wrong pipe) would pass the assertion above for
    // the wrong reason.
    assert!(
        help.contains("--skills-audit"),
        "sanity control failed: --help did not list the working `--skills-audit` \
         flag, so the absence of --skills-promote proves nothing. Output:\n{help}"
    );
}

#[test]
fn skills_promote_still_fails_loudly_when_invoked() {
    // Hidden must mean hidden, not removed: anyone who already scripted the
    // flag keeps getting the deliberate, specific explanation rather than a
    // generic clap parse error.
    let out = bin()
        .arg("--skills-promote")
        .arg("00000000-0000-0000-0000-000000000000")
        .output()
        .expect("run --skills-promote");

    assert!(
        !out.status.success(),
        "--skills-promote reported success; governed promotion is not implemented, \
         so a success here means something silently no-ops"
    );

    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("governed promotion"),
        "--skills-promote did not explain itself. Expected the governed-promotion \
         message; got:\n{stderr}"
    );
    assert!(
        !stderr.contains("unexpected argument"),
        "the flag was removed rather than hidden — scripted callers now get a clap \
         parse error instead of the deliberate explanation:\n{stderr}"
    );
}
