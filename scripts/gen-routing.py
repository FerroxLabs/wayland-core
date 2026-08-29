#!/usr/bin/env python3
"""Emit .planning/PLAN-ROUTING.json. Every outstanding criterion gets a lane and a host."""
import json, os, sys

LANE = {
 "mcp-gate-mode":      ("MCP malware gate: explicit permissive/strict operator choice", "hetzner", "1-build"),
 "floor-disclosure":   ("Command-floor refusal reaches the user, not model improvisation", "hetzner", "1-build"),
 "atref-residuals":    ("@-ref secret guard: the residuals #339 shipped past", "hetzner", "1-build"),
 "win-bash":           ("Windows: resolve a real bash, never System32\\\\bash.exe", "hetzner", "1-build"),
 "win-owned-tree":     ("OwnedTree kills the process tree on Windows, not the leaf", "hetzner", "1-build"),
 "instrument-integrity":("Prove the instruments can fail: mutation + measurement arms", "hetzner", "1-build"),
 "prompt-cache":       ("Prompt-cache collapse and re-billed context", "hetzner", "1-build"),
 "channel-caps":       ("Message caps: matrix/msteams probe shape, Telegram UTF-16, WhatsApp", "hetzner", "1-build"),
 "acp-mcp":            ("ACP MCP surface and alias resolution", "hetzner", "1-build"),
 "bookkeeping":        ("Orphan branch outcomes, VFS shell reach, misfiled defect", "hetzner", "1-build"),
 "budget-guards":      ("Runaway token spend guards  [FEATURE -> 0.13.13]", "hetzner", "1-build"),
 "approvals-rest":     ("REST approvals allowlist + timeout  [FEATURE -> 0.13.13]", "hetzner", "1-build"),
 "win-runs":           ("Windows measurement arms - serialize, ONE box", "SeanDesktop", "2-platform"),
 "macos-ci":           ("macOS arms via the lane/** CI wildcard", "macOS CI", "2-platform"),
 "desktop-run":        ("Live Desktop session measurement", "Desktop app", "2-platform"),
 "browser-revive":     ("Browser tool non-functional by default", "hetzner", "3-unrouted-pickup"),
 "flux-contract":      ("Anvil/Elevation loop-ownership contract", "hetzner", "3-unrouted-pickup"),
 "desktop-contract":   ("Published desktop contract schema gaps", "hetzner", "3-unrouted-pickup"),
 "decompose":          ("File each cross-team remainder as its OWN ticket with a contract", "gh", "2-decompose"),
 "flake-584":          ("Shared-process lib suite: the #584 fixture misses its truncation boundary under load", "hetzner", "1-build"),
 "maintainer":         ("Sean-only: credentials and platform accounts", "Sean", "2-maintainer"),
}

M = {
 "mcp-gate-mode":       [("core#354", "c1 c2 c3 c4 c5 c6 c7")],
 "floor-disclosure":    [("core#355", "c1 c2 c3 c4")],
 "atref-residuals":     [("core#339", "c3 c6"), ("core#322", "c4")],
 "win-bash":            [("wl#1164", "c1 c2 c3 c4"), ("wl#1151", "c2")],
 "win-owned-tree":      [("core#358", "c1 c4 c6")],
 "instrument-integrity":[("core#336", "c3 c4"), ("core#337", "c2 c3"), ("core#350", "c3"),
                         ("wl#1155", "c4"), ("core#352", "c5"), ("core#353", "c5")],
 "prompt-cache":        [("wl#559", "c3 c5 c6"), ("wl#1150", "c4")],
 "channel-caps":        [("wl#934", "c7 c8"), ("core#360", "c1 c2 c4 c5")],
 "acp-mcp":             [("wl#998", "c6"), ("wl#1165", "c1"), ("wl#434", "c2")],
 "bookkeeping":         [("wl#1181", "c1 c2 c3 c4"), ("core#244", "c3"), ("core#253", "c4")],
 "budget-guards":       [("wl#174", "c2 c3 c4 c5")],
 "approvals-rest":      [("wl#305", "c2 c3")],
 "win-runs":            [("core#358", "c2 c3 c5"), ("core#324", "c1 c2 c3"),
                         ("core#342", "c3 c5"), ("core#350", "c5"), ("core#238", "c4"),
                         ("wl#1164", "c5")],
 "macos-ci":            [("core#352", "c4")],
 "desktop-run":         [("wl#559", "c4")],
 "decompose":           [("wl#1088", "c2"), ("wl#1151", "c3"), ("wl#305", "c4"),
                         ("wl#388", "c4"), ("wl#434", "c3"), ("wl#998", "c5"),
                         ("core#314", "c5")],
 "flux-contract":       [("wl#863", "c3 c4 c5")],
 "browser-revive":      [("core#113", "c5")],
 "flake-584":           [("core#361", "c1 c2 c3 c4 c5 c6")],
 "maintainer":          [("wl#1186", "c5"), ("wl#934", "c5")],
}
NOTE = {
 "core#238 c4": "probe AUTHORED by bookkeeping lane; this criterion is the RUN",
 "core#358 c3": "red arm must be quoted verbatim from a real Windows run",
 "core#350 c5": "issue's own close condition: a green nightly-windows-soak against this tree",
 "core#253 c4": "issue is a feature, but c4 is defect hygiene - extract the Telegram defect to its own ticket",
 "wl#559 c4":   "ticket's own close condition: ONE real 26-turn Desktop team run showing non-zero cache_read",
 "core#352 c4": "the only prior macOS run FAILED and its branch was deleted - do not cite it",
}

FEATURE = ["wl#174", "wl#305", "wl#1165", "core#253"]

crit = {}
for lane, pairs in M.items():
    t, host, phase = LANE[lane]
    for issue, ids in pairs:
        for cid in ids.split():
            k = "%s %s" % (issue, cid)
            crit[k] = {"lane": lane, "host": host, "phase": phase}
            if k in NOTE:
                crit[k]["note"] = NOTE[k]

doc = {
  "_comment": [
    "Assignment for every outstanding criterion: which lane owns it and which machine it needs.",
    "The plan render FAILS if an outstanding criterion is missing here. That is deliberate:",
    "core#113 and wayland#863 sat outside every lane this cycle purely because nothing forced",
    "them to be assigned. An unrouted criterion is how work goes missing.",
    "kind_overrides marks feature-request issues, which do not gate a defect release."
  ],
  "lanes": {k: {"title": v[0], "host": v[1], "phase": v[2]} for k, v in LANE.items()},
  "kind_overrides": {k: "feature" for k in FEATURE},
  "criteria": crit,
}
json.dump(doc, open(sys.argv[1], "w"), indent=2, sort_keys=True)
print("routed %d criteria across %d lanes" % (len(crit), len(LANE)))
