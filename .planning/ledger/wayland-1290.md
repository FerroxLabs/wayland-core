---
issue: 1290
repo: FerroxLabs/wayland
kind: defect
title: "f14_sigkill_recovery: expected exactly one provider-dispatch recovery checkpoint fails on first attempt - possible real exactly-once defect"
status: open
last_verified_commit: 67fa14db6
criteria:
  - id: c1
    text: "The two tests are run at --retries 0 at n>=20 on a macOS host and the failing runs are inspected to establish whether the journal genuinely contains two provider-dispatch checkpoints (a product defect) or the assertion races the journal fsync (a test defect)."
    state: not-met
    evidence: "file:crates/wcore-cli/tests/f14_sigkill_recovery.rs"
    owner: core
    note: "Filed 2026-09-01. NOT FIXED IN 0.13.12. Milestoned 0.13.13. DELIBERATELY NOT ALLOWLISTED, and that is the point of the ticket: an exactly-once assertion failing under a simulated crash is a known-real defect class in this codebase, so recording it as flaky would silence exactly the signal that matters. The payload at f14_sigkill_recovery.rs:806 is 'expected exactly one provider-dispatch recovery checkpoint', which is a DIFFERENT disease from the keyring timeout in gh#1289 despite the same file and the same run -- the distinction that separated a real product bug from a timing flake earlier in the same cycle. While these stay unlisted the retry-flake gate reds on them, which is the gate working."
---

# Not allowlisted, on purpose

An exactly-once assertion failing under SIGKILL is the signal, not the noise.
