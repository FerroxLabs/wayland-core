---
issue: 1286
repo: FerroxLabs/wayland
kind: defect
title: "macOS retry-flake cluster: redundant_walk_root_is_not_walked_twice is the 5th member; discovery is one per CI cycle"
status: open
last_verified_commit: 93ede3424
criteria:
  - id: c1
    text: "The cluster is characterised as a population rather than discovered one member per CI cycle, so the allowlist stops being written reactively."
    state: not-met
    evidence: "file:.config/flaky-allowlist.txt"
    owner: core
    note: "Filed 2026-09-01. NOT FIXED IN 0.13.12. This is the ticket that names the METHOD defect behind the other four: grading only the leg expected to be guilty discovers one flake per cycle and reads absence as health. Partially answered already -- grading every leg immediately surfaced three Linux flakes (gh#1288) that four consecutive investigations had missed -- but the characterisation run itself is 0.13.13 work. redundant_walk_root_is_not_walked_twice is a measured ratio flake (1.44x, baseline 107.470ms vs subject 154.373ms), distinct from the containment bug in gh#1287 that shared its symptom."
---

# The discovery method, not just the member

Ledgered for coverage. Characterisation is 0.13.13 work.
