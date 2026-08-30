---
issue: 1207
repo: FerroxLabs/wayland
kind: defect
title: "#1166's ticket Defect 5 -- cache diagnostics off by default -- has no ledger criterion and was never graded"
status: open
last_verified_commit: f45e5bd83
criteria:
  - id: c1
    text: "A decision is recorded for compact.cache_diagnostics defaulting to false: on, off, or off with the reason stated"
    state: met
    evidence: "test:crates/wcore-agent/src/cache_ledger.rs::a_default_install_still_detects_and_records_a_cache_break"
    owner: core
    note: "Filed 2026-08-29 by the 0.13.12 close-sweep (worklist defect D25, found while verifying wayland#1166). NOW CLOSED -- the disposition is at the end of this note. The measured finding as filed, verbatim: Ticket Defect 5 ('off by default', `compact.cache_diagnostics = false` at crates/wcore-config/src/compact.rs:643) is neither fixed nor graded. The ledger has five criteria but they cover only four of the ticket's five numbered defects — c5 is a control, not a defect. compact.rs:825 `cache_diagnostics_defaults_to_false` actively pins the flag off, so the three user-facing `emit_info` lines at engine.rs:15836/15842/15850 stay silent for a default install. CLOSED 2026-08-30 by lane/f13-misc. THE DECISION IS: off, with the reason stated, and the ticket's premise is narrower than it reads. `compact.cache_diagnostics` gates exactly three `emit_info` lines that print a cache verdict INTO THE CONVERSATION, and it stays off deliberately -- #101 filed those as alarming users over normal behaviour (a TtlExpiry after a pause is expected). The two DETECTING surfaces are live without it: the `cache_health_warn` tracing event (the CLI's default EnvFilter is `info`, so a warn! is emitted) and the ledger record (`recording_enabled` is on unless WAYLAND_CACHE_LEDGER is set). A default install therefore detects, attributes and persists; it just does not interrupt the chat. #1166 worded this defect as `Consider`, and the answer is a documented no with a guard that makes the no stick: a source lint brace-matching every `if self.compact_config.cache_diagnostics {` block in engine.rs and requiring that neither cache_health_warn nor recording_enabled sits inside one, with four controls against a vacuous pass (the gate must still exist, every extracted block must be 20-1000 bytes so a runaway brace match cannot swallow the file, every block must contain emit_info, and each probe string must still exist in engine.rs at all). RED ARM: `let _ = crate::cache_ledger::recording_enabled();` inserted inside the first gated block -- `recording_enabled moved inside `if self.compact_config.cache_diagnostics {`. A default install would stop detecting the break, which is ticket Defect 5 of #1166`. `cache_diagnostics_defaults_to_false` (wcore-config/src/compact.rs:825) still pins the flag off and is untouched: the flag's value is the decision, the lint is what it may not gate."
  - id: c2
    text: "Whichever way it goes, the ledger entry for wayland#1166 carries a criterion covering it, so the ticket's fifth numbered defect is visible in the 'all criteria met' reading rather than absent from it"
    state: met
    evidence: "file:.planning/ledger/wayland-1166.md"
    owner: core
    note: "Filed 2026-08-29 by the 0.13.12 close-sweep (worklist defect D25). NOW CLOSED -- the disposition is at the end of this note. The original measurement is on c1 and the file:line anchors are in the prose below. CLOSED 2026-08-30 by lane/f13-misc. .planning/ledger/wayland-1166.md now carries c6, `Ticket Defect 5 -- the detection is not silent on a default install: only the chat-visible half is behind compact.cache_diagnostics`, graded with its own evidence and its own red arm. The ticket's fifth numbered defect is therefore inside the `all criteria met` reading of that file instead of absent from it, which is the whole of this criterion."
---

Ticket Defect 5 ('off by default', `compact.cache_diagnostics = false` at crates/wcore-config/src/compact.rs:643) is neither fixed nor graded. The ledger has five criteria but they cover only four of the ticket's five numbered defects — c5 is a control, not a defect. compact.rs:825 `cache_diagnostics_defaults_to_false` actively pins the flag off, so the three user-facing `emit_info` lines at engine.rs:15836/15842/15850 stay silent for a default install.

**Where.** crates/wcore-config/src/compact.rs:643 and :825; .planning/ledger/wayland-1166.md (no criterion for ticket Defect 5)

**Why it matters.** Lower severity than it first reads — I checked the other two surfaces and both ARE live by default: the `cache_health_warn` tracing::warn! at engine.rs:15877 is not gated by the flag and the CLI default filter is EnvFilter::new('info') (wcore-cli/src/main.rs:1360), and the ledger records by default (recording_enabled() → true). The ticket also worded this one as 'Consider'. But it is a ticket-listed defect with no criterion at all, which is exactly the ledger-vs-ticket drift that lets a partial grade as done; a closer reading the ledger alone would never see it.

Criteria are taken verbatim from the issue's Acceptance section.

All of them are now met, closed by lane/f13-misc on 2026-08-30 alongside the
ticket they were found under. Each criterion's note carries its own evidence
and its own red arm. The GitHub issue is left OPEN deliberately: closing an
issue is not this lane's action.
