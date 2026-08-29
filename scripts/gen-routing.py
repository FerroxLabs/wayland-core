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
 "telegram-topic":     ("Telegram forum-topic target sent as reply_to_message_id, never message_thread_id", "hetzner", "1-build"),
 "container-latch":    ("Container backend latches on a leftover name, and attests a run that never happened", "hetzner", "1-build"),
 # ── added 2026-08-29 by the 0.13.12 close-sweep (SWEEP-0.13.12) ──────────────
 # Every lane below owns criteria that were either newly filed by the sweep or
 # re-graded not-met by it. An unrouted criterion is how work goes missing, so
 # each one is assigned here even though no lane instance has picked it up yet.
 "vfs-store-reach":    ("VFS/policy: store reach through GrepTool, the weaker resolver, and a dead predicate", "hetzner", "1-build"),
 "atref-walk":         ("@-ref directory walk: silent empty payloads and a FIFO wedge", "hetzner", "1-build"),
 "plugin-quarantine":  ("Plugin quarantine: teardown after setsid, and the Windows primitive", "hetzner", "1-build"),
 "small-window":       ("Small served/configured windows: notice truth, reserves, ceilings, budgets", "hetzner", "1-build"),
 "cache-truth":        ("Cache and spend ledgers: legacy decode, false invalidations, session keys", "hetzner", "1-build"),
 "ci-evidence":        ("CI evidence: the wrapper that cannot write, the floor that cannot see, the anchors that cannot rot", "hetzner", "1-build"),
 "provider-urls":      ("Provider endpoints: Anthropic /v1 doubling and the self-hosted locality predicate", "hetzner", "1-build"),
 "reasoning-history":  ("Reasoning filter writes the durable record: truncation and the missing flush", "hetzner", "1-build"),
 "tool-wire-order":    ("Tool wire order and the frozen system prefix", "hetzner", "1-build"),
 "mcp-transports":     ("tools/list_changed on the SSE and Streamable-HTTP transports", "hetzner", "1-build"),
 "model-limits":       ("Model limits: host-variable open-weights ids and the provider-blind ceiling", "hetzner", "1-build"),
 "test-instruments":   ("Instruments that cannot fail: the wrapping ratchet and the 60s test", "hetzner", "1-build"),
 "cli-messages":       ("User-facing message shape: collapsed whitespace in three crates", "hetzner", "1-build"),
 "doc-truth":          ("Shipped operator docs that overclaim what the code does", "hetzner", "1-build"),
 "win-quarantine":     ("Windows: what a quarantine child can still do to the console", "SeanDesktop", "2-platform"),
 "atomic-write":       ("Atomic publish: a rollback that exchanged nothing is reported as success", "hetzner", "1-build"),
 "json-stream-consent":("json-stream: an egress consent the host is never shown", "hetzner", "1-build"),
 "instrument-integrity-2":("reserved -- kept so an empty lane is visible rather than silently dropped", "hetzner", "1-build"),
 # ── added 2026-08-29: lanes for the open wayland-core issues that had no ledger
 # file at all (SEED-UNCOVERED). Routed so the render can see them; no lane
 # instance has picked any of them up.
 "bwrap-race":         ("bwrap ownership race with ENOENT, and a containment test that retries into a pass", "hetzner", "1-build"),
 "gepa-selection":     ("GEPA online evolution: close the selection loop before defaulting it on", "hetzner", "1-build"),
 "container-latch-2":  ("Container orphan scan is nonce-scoped and can never see an earlier run's leftover", "hetzner", "1-build"),
 "test-instruments-2": ("Harness ownership on Unix, and the guard against merging a red arm", "hetzner", "1-build"),
 "instrument-integrity-3": ("Scoped-subscriber ERROR visibility: name the mechanism, measure at n>=100", "hetzner", "1-build"),
 "win-runs-2":         ("Windows measurement arms from the 0.13.12 sweep - serialize, ONE box", "SeanDesktop", "2-platform"),
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
 "telegram-topic":      [("core#363", "c1 c2 c3 c4 c5 c6")],
 "container-latch":     [("core#365", "c1 c2 c3 c4 c5 c6")],
 # ── added 2026-08-29 by the 0.13.12 close-sweep (SWEEP-0.13.12) ──────────────
 "vfs-store-reach":     [("core#375", "c1 c2 c3 c4"), ("core#376", "c1 c2 c3 c4"),
                         ("core#383", "c1 c2 c3"), ("core#384", "c1 c2 c3"),
                         ("core#244", "c3"), ("core#323", "c4"), ("core#339", "c2"), ("core#356", "c4")],
 "atref-walk":          [("core#377", "c1 c2 c3 c4"), ("core#381", "c1 c2 c3 c4"),
                         ("core#335", "c3")],
 "plugin-quarantine":   [("core#379", "c1 c2 c3"), ("core#338", "c2")],
 "win-quarantine":      [("core#380", "c1 c2 c3")],
 "small-window":        [("core#382", "c1 c2 c3"), ("wl#1199", "c1 c2 c3"),
                         ("wl#1200", "c1 c2 c3"), ("wl#1210", "c1 c2 c3"),
                         ("wl#1218", "c1 c2 c3"), ("wl#1172", "c3"),
                         ("wl#1179", "c2"), ("wl#1150", "c5")],
 "cache-truth":         [("wl#1203", "c1 c2 c3"), ("wl#1205", "c1 c2 c3 c4"),
                         ("wl#1206", "c1 c2 c3 c4"), ("wl#1207", "c1 c2"),
                         ("wl#559", "c6")],
 "ci-evidence":         [("wl#1197", "c1 c2 c3"), ("wl#1198", "c1 c2 c3"),
                         ("wl#1215", "c1 c2 c3"), ("wl#1216", "c1 c2 c3"),
                         ("wl#1220", "c1 c2 c3"), ("wl#1134", "c3"),
                         ("wl#1177", "c1 c2"), ("wl#1182", "c3"), ("core#325", "c2")],
 "provider-urls":       [("wl#1211", "c1 c2 c3 c4"), ("wl#1212", "c1 c2 c3"),
                         ("wl#1217", "c1 c2 c3 c4")],
 "reasoning-history":   [("wl#1221", "c1 c2 c3 c4"), ("wl#1222", "c1 c2 c3 c4"),
                         ("wl#908", "c3")],
 "tool-wire-order":     [("wl#1208", "c1 c2 c3"), ("wl#1209", "c1 c2 c3")],
 "mcp-transports":      [("wl#1213", "c1 c2 c3 c4"), ("wl#1175", "c1")],
 "model-limits":        [("wl#1214", "c1 c2 c3 c4"), ("wl#1176", "c5")],
 "test-instruments":    [("core#378", "c1 c2 c3"), ("core#385", "c1 c2 c3"),
                         ("core#336", "c2"), ("wl#1155", "c2")],
 "cli-messages":        [("wl#1204", "c1 c2 c3")],
 "doc-truth":           [("core#340", "c1")],
 "instrument-integrity-2": [],
 "atomic-write":        [("wl#1202", "c1 c2 c3 c4")],
 "json-stream-consent": [("wl#1219", "c1 c2 c3 c4")],
 # ── added 2026-08-29 (SEED-UNCOVERED) ──────────────────────────────────────
 "bwrap-race":          [("core#362", "c1 c2 c3 c4 c5")],
 "gepa-selection":      [("core#372", "c1 c2 c3 c4 c5 c6")],
 "container-latch-2":   [("core#366", "c1 c2 c3 c4 c5")],
 "test-instruments-2":  [("core#367", "c2 c3"), ("core#371", "c1 c2")],
 "instrument-integrity-3": [("core#373", "c1 c2 c3 c4 c5")],
 "win-runs-2":          [("core#368", "c1 c2"), ("core#369", "c1 c2 c3 c4"),
                         ("core#370", "c1 c2"), ("core#374", "c1 c2 c3")],
 "maintainer":          [("wl#1186", "c5"), ("wl#934", "c5"), ("core#364", "c1 c2")],
}
NOTE = {
 "core#238 c4": "probe AUTHORED by bookkeeping lane; this criterion is the RUN",
 "core#358 c3": "red arm must be quoted verbatim from a real Windows run",
 "core#350 c5": "issue's own close condition: a green nightly-windows-soak against this tree",
 "core#253 c4": "issue is a feature, but c4 is defect hygiene - extract the Telegram defect to its own ticket",
 "wl#559 c4":   "ticket's own close condition: ONE real 26-turn Desktop team run showing non-zero cache_read",
 "core#380 c3": "Windows-only: the run must be quoted verbatim from SeanDesktop",
 "wl#1215 c2":  "an observed-outcome criterion: a real CI run URL, not a code reading",
 "wl#1203 c3":  "an observed-outcome criterion: a live spend-audit.jsonl, not a call-site reading",
 "wl#1176 c5":  "re-graded not-met 2026-08-29: the pointer names another change's code and the property is false on passthrough.rs's own contents",
 "wl#1177 c1":  "the mechanism has never once worked on a real runner; wl#1215 is the live instance",
 "core#340 c1": "the shipped operator doc still carries the guarantee the ticket exists to remove",
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
