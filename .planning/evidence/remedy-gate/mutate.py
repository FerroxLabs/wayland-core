#!/usr/bin/env python3
"""Red-before-green harness for the remedy gate.

The gate's only real acceptance test is whether it goes red on the defects it
claims to catch. Reading the code proves nothing; a gate that looks thorough and
fires on nothing is the failure mode this program has recorded ten times.

So: revert one historical defect at a time IN THE WORKING TREE, run the gate,
record the verdict, restore from a byte-exact backup.

A mutation whose pattern does not match is reported SKIPPED-NOT-APPLIED, never
as a pass. "The sed matched nothing so the suite stayed green" is itself a
self-passing gate.

Usage (on the build host, inside the lane worktree):
    export PATH=/root/.cargo/bin:$PATH
    python3 .planning/evidence/remedy-gate/mutate.py
"""

import os
import shutil
import subprocess
import sys

ROOT = os.environ.get("RG_ROOT", "/root/wayland-remedy-gate")
TARGET = "remedy_advertisements"

# (id, description, file, old, new, extra test target or None)
MUTATIONS = [
    (
        "case1-27-C2a",
        "browser remediation hint reverted to the bare [browser] section",
        "crates/wcore-browser/src/config_hint.rs",
        "[browser.policy]",
        "[browser]",
    ),
    (
        "case5-headless-keyring",
        'keyring remedy reverted to `credentials.backend` = "encrypted-file"',
        "crates/wcore-agent/src/recovery_confidential.rs",
        'or set [storage.credentials] backend = \\"keyring\\"',
        'or set `credentials.backend` to \\"encrypted-file\\"',
    ),
    (
        "case6-init-model",
        "init template reverted to a root-level model key (found by this gate)",
        "crates/wcore-cli/src/init.rs",
        "         [default]\\n\\\n         model = \\\"{model}\\\"\\n\\\n",
        "         model = \\\"{model}\\\"\\n\\\n",
    ),
    (
        "case2-23A-C1",
        "--skills-promote un-hidden, i.e. advertised in --help again",
        "crates/wcore-cli/src/main.rs",
        '#[arg(long, value_name = "PROCEDURE_ID", hide = true)]',
        '#[arg(long, value_name = "PROCEDURE_ID")]',
    ),
]


def run_gate():
    p = subprocess.run(
        ["cargo", "test", "-p", "wcore-cli", "--test", TARGET, "--", "--nocapture"],
        cwd=ROOT,
        capture_output=True,
        text=True,
    )
    out = p.stdout + p.stderr
    lines = [l for l in out.splitlines() if l.startswith("test result:")]
    return p.returncode, lines, out


def main():
    print("=== baseline (no mutation) ===", flush=True)
    rc, lines, _ = run_gate()
    for l in lines:
        print("   " + l, flush=True)
    if rc != 0:
        print("!! baseline is ALREADY RED -- every mutation below would be "
              "meaningless. Stopping.", flush=True)
        return 2

    results = []
    for mid, desc, rel, old, new in MUTATIONS:
        path = os.path.join(ROOT, rel)
        backup = path + ".rg-backup"
        shutil.copy2(path, backup)
        try:
            src = open(path, encoding="utf-8").read()
            if old not in src:
                results.append((mid, "SKIPPED-NOT-APPLIED", "pattern not found"))
                print(f"!! {mid}: pattern NOT FOUND -- mutation not applied.", flush=True)
                print(f"   looked for: {old!r}", flush=True)
                continue
            open(path, "w", encoding="utf-8").write(src.replace(old, new))
            rc, lines, out = run_gate()
            verdict = "RED" if rc != 0 else "STILL-GREEN"
            results.append((mid, verdict, "; ".join(lines)))
            print(f"== {mid}: {verdict}  ({desc})", flush=True)
            for l in lines:
                print("   " + l, flush=True)
            if verdict == "RED":
                for l in out.splitlines():
                    if "advertises" in l or "advertised in --help" in l:
                        print("   > " + l[:220], flush=True)
        finally:
            shutil.copy2(backup, path)
            os.remove(backup)

    print("\n=== SUMMARY ===", flush=True)
    for mid, verdict, detail in results:
        print(f"{mid:26s} {verdict:20s} {detail}", flush=True)
    return 1 if any(v != "RED" for _, v, _ in results) else 0


if __name__ == "__main__":
    sys.exit(main())
