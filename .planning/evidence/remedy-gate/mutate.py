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

# (id, EXPECTED verdict, description, file, old, new)
#
# The STILL-GREEN rows are not filler. A coverage claim asserted in prose is
# worth nothing; these two MEASURE the boundary by re-introducing the defect and
# recording that the gate does not see it. Stating the limit and proving the
# limit are different things, and only the second one survives review.
MUTATIONS = [
    (
        "case1-27-C2a",
        "RED",
        "browser remediation hint reverted to the bare [browser] section",
        "crates/wcore-browser/src/config_hint.rs",
        "[browser.policy]",
        "[browser]",
    ),
    (
        "case5-headless-keyring",
        "RED",
        'keyring remedy reverted to `credentials.backend` = "encrypted-file"',
        "crates/wcore-agent/src/recovery_confidential.rs",
        'or set [storage.credentials] backend = \\"keyring\\"',
        'or set `credentials.backend` to \\"encrypted-file\\"',
    ),
    (
        "case6-init-model",
        "RED",
        "init template reverted to a root-level model key (found by this gate)",
        "crates/wcore-cli/src/init.rs",
        "         [default]\\n\\\n         model = \\\"{model}\\\"\\n\\\n",
        "         model = \\\"{model}\\\"\\n\\\n",
    ),
    (
        "case2-23A-C1",
        "RED",
        "--skills-promote un-hidden, i.e. advertised in --help again",
        "crates/wcore-cli/src/main.rs",
        '#[arg(long, value_name = "PROCEDURE_ID", hide = true)]',
        '#[arg(long, value_name = "PROCEDURE_ID")]',
    ),
    # ---- measured coverage LIMITS: the gate is expected NOT to see these ----
    (
        "case3-24-C2-NOT-COVERED",
        "STILL-GREEN",
        "webhook:/poll: re-advertised as --trigger values (a flag VALUE, which "
        "has no single type to re-parse through)",
        "crates/wcore-cli/src/cron.rs",
        "        /// `webhook:` and `poll:` are NOT accepted: nothing in this build can",
        "        ///   --trigger webhook:https://example.test/hook\n"
        "        ///   --trigger poll:https://example.test/health:300\n"
        "        ///\n"
        "        /// `webhook:` and `poll:` are NOT accepted: nothing in this build can",
    ),
    (
        "case4-ollama-NOT-COVERED",
        "STILL-GREEN",
        "the ollama local-model escape hatch removed again, making the hint's "
        "advertised route unreachable (an ORDERING defect, not a naming one)",
        "crates/wcore-cli/src/cron.rs",  # placeholder, replaced below
        "@@never-matches@@",
        "@@never-matches@@",
    ),
]

# case 4 lives in wcore-config; spelled separately because the anchor is long.
MUTATIONS[-1] = (
    "case4-ollama-NOT-COVERED",
    "STILL-GREEN",
    "the ollama local-model escape hatch removed again, making the hint's "
    "advertised route unreachable (an ORDERING defect, not a naming one)",
    "crates/wcore-config/src/config.rs",
    "None if wcore_types::model_aliases::is_local_model(&model) => String::new(),\n",
    "",
)


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
    for mid, expect, desc, rel, old, new in MUTATIONS:
        path = os.path.join(ROOT, rel)
        backup = path + ".rg-backup"
        shutil.copy2(path, backup)
        try:
            src = open(path, encoding="utf-8").read()
            if old not in src:
                results.append((mid, expect, "SKIPPED-NOT-APPLIED", "pattern not found"))
                print(f"!! {mid}: pattern NOT FOUND -- mutation not applied.", flush=True)
                print(f"   looked for: {old!r}", flush=True)
                continue
            open(path, "w", encoding="utf-8").write(src.replace(old, new))
            rc, lines, out = run_gate()
            verdict = "RED" if rc != 0 else "STILL-GREEN"
            results.append((mid, expect, verdict, "; ".join(lines)))
            mark = "as expected" if verdict == expect else "*** UNEXPECTED ***"
            print(f"== {mid}: {verdict} ({mark})  {desc}", flush=True)
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
    for mid, expect, verdict, detail in results:
        mark = "OK" if verdict == expect else "MISMATCH"
        print(f"{mid:28s} expect={expect:12s} got={verdict:14s} {mark}", flush=True)
    return 1 if any(v != e for _, e, v, _ in results) else 0


if __name__ == "__main__":
    sys.exit(main())
