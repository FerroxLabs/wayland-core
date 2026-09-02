---
issue: 1287
repo: FerroxLabs/wayland
kind: defect
title: "macOS process-tree containment intermittently refuses to attach - root NOT in its own group at recheck (fails closed)"
status: open
last_verified_commit: 93ede3424
criteria:
  - id: c1
    text: "process_tree.rs stops refusing containment when the root process is spawned with process_group(0) but is observed outside its own group at recheck, or the recheck is corrected to tolerate the macOS reassignment it is actually observing."
    state: not-met
    evidence: "file:crates/wcore-sandbox/src/backends/process_tree.rs"
    owner: core
    note: "Filed 2026-09-01. NOT FIXED IN 0.13.12 and NOT INTRODUCED BY IT -- pre-existing, and absent from the 0.13.12 diff. Milestoned 0.13.13. Found by reading the PANIC PAYLOAD rather than the test name: the visible symptom was a 15s timeout in a swarm capacity test, and it was one allowlist entry away from being recorded as a sixth timing flake. The production refusal is raised at process_tree.rs:683 with 'macOS process-group authority changed while containment was attached (sentinel in group: true, root: same generation, but NOT in its own group)'. Root is spawned with _command.process_group(0) at line 30, so it should be its own group leader, and at recheck it is not. FAILS CLOSED (PermissionDenied), so this is a reliability defect and not a containment escape -- which is why it is 0.13.13 and not a release blocker."
---

# A product bug that was nearly filed as a flake

The symptom is a timeout; the cause is a production containment refusal. Fails closed.
