#!/usr/bin/env python3
"""27-C2(c) macOS leg — the negative controls.

A gate that cannot go red proves nothing. Each mutation below breaks exactly one
mechanism, the suite is rebuilt and re-run, and the source is restored from a
byte-for-byte backup taken first. The baseline is re-verified green at the end.

Two of the four mutations hit the PRODUCT (confinement, reaper discrimination,
approval gate) and one hits this lane's new macOS INSTRUMENT (`ps_snapshot`) —
because a ported instrument that silently returns an empty process table would
make every "the process is gone" assertion in baseline 3 free.

  python3 .planning/scripts/macos-legs-m3-mutations.py
"""

import os
import pathlib
import shutil
import subprocess
import sys

ROOT = pathlib.Path(__file__).resolve().parents[2]
CARGO = "/Users/seandonahoe/.cargo/bin/cargo"

MUTATIONS = [
    {
        "id": "MUT-1",
        "what": "baseline 1 / PRODUCT: replace the symlink-aware root confinement "
        "with a naive lexical starts_with on the UNRESOLVED path",
        "file": "crates/wcore-browser/src/tool.rs",
        "old": """        let (target_canon, target_suffix) = canonicalize_existing_prefix(&normalized)
            .ok_or_else(|| format!("local path {raw:?} has no real prefix"))?;
        let resolved = target_canon.join(&target_suffix);
        if !resolved.starts_with(&root_canon) {""",
        "new": """        if !normalized.starts_with(&root_canon) {""",
        "crate": "wcore-browser",
        "test": "downloads_root_baseline_test",
        "expect": "B4-symlink escape is NOT refused",
    },
    {
        "id": "MUT-2",
        "what": "baseline 3 / PRODUCT: make the orphan reaper terminate every "
        "registered session regardless of whether its parent is alive",
        "file": "crates/wcore-browser/src/supervisor.rs",
        "old": "                            if !process_alive(h.parent_pid) {",
        "new": "                            if true {",
        "crate": "wcore-browser",
        "test": "process_count_reaper_baseline_test",
        "expect": "ARM 2 (live-parent control) fails — the reaper is indiscriminate",
    },
    {
        "id": "MUT-3",
        "what": "baseline 3 / INSTRUMENT: make the macOS `ps` reader return an "
        "empty process table (the dead-instrument case)",
        "file": "crates/wcore-browser/tests/process_count_reaper_baseline_test.rs",
        "old": """    let out = match std::process::Command::new("/bin/ps")""",
        "new": """    if std::env::var("WL_MUT3").is_err() { return Vec::new(); }
    let out = match std::process::Command::new("/bin/ps")""",
        "crate": "wcore-browser",
        "test": "process_count_reaper_baseline_test",
        "expect": "ps_instrument_is_live fails — a dead instrument is caught, "
        "so baseline 3's zeros are not free",
    },
    {
        "id": "MUT-4",
        "what": "baseline 2 / PRODUCT: drop the Suspend arm so a withheld "
        "approval falls through to backend dispatch",
        "file": "crates/wcore-cua/src/tool.rs",
        "old": """            CuaPolicyOutcome::Suspend { reason } => {
                return Err(CuaError::PolicySuspended { reason });
            }""",
        "new": """            CuaPolicyOutcome::Suspend { reason: _ } => {}""",
        "crate": "wcore-cua",
        "test": "approval_gate_baseline_test",
        "expect": "withheld arms dispatch to the backend — approval is advisory",
    },
]


def run(args, **kw):
    return subprocess.run(args, cwd=ROOT, capture_output=True, text=True, **kw)


def run_suite(crate, test):
    """Build + run one test binary. Returns (rc, combined output)."""
    b = run([CARGO, "build", "-p", crate, "--test", test])
    if b.returncode != 0:
        return 101, "BUILD FAILED\n" + b.stdout + b.stderr
    r = run([CARGO, "test", "-p", crate, "--test", test, "--", "--test-threads=1"])
    return r.returncode, r.stdout + r.stderr


def main():
    print("### 27-C2(c) macOS leg — negative controls (mutation runs)")
    print(f"repo: {ROOT}")
    head = run(["/usr/bin/git", "rev-parse", "HEAD"]).stdout.strip()
    print(f"HEAD: {head}")
    print(f"uname: {subprocess.run(['uname', '-a'], capture_output=True, text=True).stdout.strip()}")
    print()

    overall_ok = True
    for m in MUTATIONS:
        src = ROOT / m["file"]
        backup = src.with_suffix(src.suffix + ".mutbak")
        shutil.copy2(src, backup)
        text = src.read_text()
        if m["old"] not in text:
            print(f"--- {m['id']}: FATAL — anchor text not found in {m['file']}")
            print("    The mutation did not apply, so a green suite below would be")
            print("    meaningless. Refusing to report this control.")
            backup.unlink()
            overall_ok = False
            continue
        occurrences = text.count(m["old"])
        src.write_text(text.replace(m["old"], m["new"], 1))
        print(f"--- {m['id']} — {m['what']}")
        print(f"    file: {m['file']} (anchor occurrences: {occurrences})")
        print(f"    expected consequence: {m['expect']}")
        rc, out = run_suite(m["crate"], m["test"])
        # `copy2` would restore the ORIGINAL mtime, which is OLDER than the
        # artifact the mutated build just produced — cargo would then skip the
        # rebuild and the next run would exercise the STALE MUTATED BINARY.
        # (AGENTS.md §11: "an artifact newer than its source is a build that did
        # not happen". The first version of this script did exactly that and the
        # restore-verification came back falsely RED; the same mechanism yields a
        # false GREEN whenever the stale binary is the permissive one.)
        shutil.copyfile(backup, src)
        os.utime(src, None)
        backup.unlink()
        tail = [
            ln
            for ln in out.splitlines()
            if "test result:" in ln
            or ln.startswith("test ")
            and ("FAILED" in ln or "ok" in ln)
            or "panicked at" in ln
            or "BUILD FAILED" in ln
        ]
        print(f"    MUTATED_RC={rc}")
        for ln in tail[:25]:
            print(f"      {ln}")
        verdict = "RED (control valid)" if rc != 0 else "STILL GREEN — CONTROL INVALID"
        if rc == 0:
            overall_ok = False
        print(f"    VERDICT: {verdict}")
        print()

    print("=== baseline restored — re-verifying green ===")
    for crate, test in [
        ("wcore-browser", "downloads_root_baseline_test"),
        ("wcore-browser", "process_count_reaper_baseline_test"),
        ("wcore-cua", "approval_gate_baseline_test"),
    ]:
        rc, out = run_suite(crate, test)
        line = next((l for l in out.splitlines() if "test result:" in l), "(no result line)")
        print(f"  {test}: rc={rc} | {line.strip()}")
        if rc != 0:
            overall_ok = False

    print()
    print(f"CONTROLS_ALL_VALID={overall_ok}")
    return 0 if overall_ok else 1


if __name__ == "__main__":
    sys.exit(main())
