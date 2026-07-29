#!/usr/bin/env python3
"""Prove f24-reconnect-selftest.mjs can fail, by mutating the REPAIRED fixture.

R3 already proves the pre-repair fixture reddens, but R3 is one assertion. These
mutations attack the other assertions individually, so a reader can see the
self-test discriminates rather than blanket-failing.

Each replacement is asserted to apply EXACTLY ONCE. A replacement that silently
matched nothing would make the whole sweep meaningless — the sweep would report
"mutated, still green" for a mutation that never happened.

Restores in a `finally:`, then verifies the tree is byte-identical.

usage: python3 mutate-instrument.py
"""

import hashlib
import json
import pathlib
import subprocess
import sys

REPO = pathlib.Path(__file__).resolve().parents[4]
FIXTURE = REPO / "scripts" / "f24-discord-fixture.mjs"
SELFTEST = REPO / "scripts" / "f24-reconnect-selftest.mjs"

MUTATIONS = {
    # The drop is announced but never performed. The socket stays up, so the
    # "disconnect window" contains a connected client and DURING-1 is delivered
    # live rather than replayed. A probe that could not detect this would grade
    # a reconnect that never happened.
    "MI1-drop-is-a-lie": (
        "    const n = this.conns.size;\n    for (const c of this.conns) {\n"
        "      try {\n        c.socket.destroy();\n      } catch {\n"
        "        /* already gone */\n      }\n    }\n    this.conns.clear();\n",
        "    const n = this.conns.size;\n",
    ),
    # RESUME replays the entire journal instead of the tail. Every already-
    # delivered message arrives a second time. This is the DUPLICATE half of the
    # clause, and a probe that only asked "did the gap arrive?" would pass.
    "MI2-replay-everything": (
        "const replayed = this.dispatched.filter((x) => x.s > after);",
        "const replayed = this.dispatched.filter((x) => x.s > 0);",
    ),
    # The replay happens but is not journalled and not counted as a delivery.
    # The message arrives, so a naive probe is green, while the fixture's own
    # ledger under-reports deliveries — which is what the duplicate detector
    # keys on.
    "MI3-replay-not-journalled": (
        "        d.sockets += 1;\n",
        "",
    ),
}


def run_selftest():
    p = subprocess.run(
        ["node", str(SELFTEST)],
        cwd=str(REPO),
        capture_output=True,
        text=True,
    )
    tail = [ln for ln in p.stdout.splitlines() if ln.startswith("F24RECONNECT")]
    failing = [ln.split("\n")[0][5:] for ln in p.stdout.splitlines() if ln.startswith("FAIL ")]
    return {
        "rc": p.returncode,
        "verdict": tail[0] if tail else "NO VERDICT LINE",
        "failed_assertions": failing,
    }


def main():
    original = FIXTURE.read_text()
    original_sha = hashlib.sha256(original.encode()).hexdigest()
    results = {"baseline": run_selftest(), "mutations": {}}

    if results["baseline"]["rc"] != 0:
        print(json.dumps(results, indent=2))
        print("BASELINE IS NOT GREEN — a mutation sweep from a red baseline proves nothing")
        return 2

    try:
        for name, (old, new) in MUTATIONS.items():
            count = original.count(old)
            if count != 1:
                results["mutations"][name] = {
                    "applied": False,
                    "reason": f"pattern occurs {count} times, expected exactly 1",
                }
                continue
            FIXTURE.write_text(original.replace(old, new, 1))
            r = run_selftest()
            r["applied"] = True
            r["REDDENED"] = r["rc"] != 0
            results["mutations"][name] = r
            FIXTURE.write_text(original)
    finally:
        FIXTURE.write_text(original)

    restored_sha = hashlib.sha256(FIXTURE.read_text().encode()).hexdigest()
    results["restored_byte_identical"] = restored_sha == original_sha
    results["sha256_before"] = original_sha
    results["sha256_after"] = restored_sha

    all_red = all(m.get("REDDENED") for m in results["mutations"].values())
    results["EVERY_MUTATION_REDDENED"] = all_red
    print(json.dumps(results, indent=2))
    return 0 if all_red and results["restored_byte_identical"] else 1


if __name__ == "__main__":
    sys.exit(main())
