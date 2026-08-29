---
issue: 1207
repo: FerroxLabs/wayland
kind: defect
title: "#1166's ticket Defect 5 -- cache diagnostics off by default -- has no ledger criterion and was never graded"
status: open
last_verified_commit: 9de21aa1
criteria:
  - id: c1
    text: "A decision is recorded for compact.cache_diagnostics defaulting to false: on, off, or off with the reason stated"
    state: not-met
    owner: core
    note: "Filed 2026-08-29 by the 0.13.12 close-sweep (worklist defect D25, found while verifying wayland#1166). Nothing has been done. The measured finding, verbatim: Ticket Defect 5 ('off by default', `compact.cache_diagnostics = false` at crates/wcore-config/src/compact.rs:643) is neither fixed nor graded. The ledger has five criteria but they cover only four of the ticket's five numbered defects — c5 is a control, not a defect. compact.rs:825 `cache_diagnostics_defaults_to_false` actively pins the flag off, so the three user-facing `emit_info` lines at engine.rs:15836/15842/15850 stay silent for a default install."
  - id: c2
    text: "Whichever way it goes, the ledger entry for wayland#1166 carries a criterion covering it, so the ticket's fifth numbered defect is visible in the 'all criteria met' reading rather than absent from it"
    state: not-met
    owner: core
    note: "Filed 2026-08-29 by the 0.13.12 close-sweep (worklist defect D25). Nothing has been done; the measurement is on c1 and the file:line anchors are in the prose below."
---

Ticket Defect 5 ('off by default', `compact.cache_diagnostics = false` at crates/wcore-config/src/compact.rs:643) is neither fixed nor graded. The ledger has five criteria but they cover only four of the ticket's five numbered defects — c5 is a control, not a defect. compact.rs:825 `cache_diagnostics_defaults_to_false` actively pins the flag off, so the three user-facing `emit_info` lines at engine.rs:15836/15842/15850 stay silent for a default install.

**Where.** crates/wcore-config/src/compact.rs:643 and :825; .planning/ledger/wayland-1166.md (no criterion for ticket Defect 5)

**Why it matters.** Lower severity than it first reads — I checked the other two surfaces and both ARE live by default: the `cache_health_warn` tracing::warn! at engine.rs:15877 is not gated by the flag and the CLI default filter is EnvFilter::new('info') (wcore-cli/src/main.rs:1360), and the ledger records by default (recording_enabled() → true). The ticket also worded this one as 'Consider'. But it is a ticket-listed defect with no criterion at all, which is exactly the ledger-vs-ticket drift that lets a partial grade as done; a closer reading the ledger alone would never see it.

Criteria are taken verbatim from the issue's Acceptance section. Nothing has been done: this entry exists so the release gate counts the work rather than anyone having to remember it.
