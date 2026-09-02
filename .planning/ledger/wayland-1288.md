---
issue: 1288
repo: FerroxLabs/wayland
kind: defect
title: "Three Linux retry-flakes surfaced in the run that overran its 120-min timeout; rate unmeasured, entries expire 2026-09-20"
status: open
last_verified_commit: 67fa14db6
criteria:
  - id: c1
    text: "Each of the three entries is either deleted at expiry because a normal-duration ci-linux run does not reproduce it, or carries a measured rate from a run that did not overrun its timeout."
    state: not-met
    evidence: "file:.config/flaky-allowlist.txt"
    owner: core
    note: "Filed 2026-09-01. NOT FIXED IN 0.13.12. Milestoned 0.13.13. These were observed ONCE, on Linux, in the run whose ci-linux job overran its 120-minute timeout (120.3 min against a 99-102 min norm), i.e. under pathological load; the same tests graded 0 unlisted on Linux in two normal runs. The rate is therefore NOT measured and the entries say so. Short 2026-09-20 expiry for exactly that reason: DELETE rather than renew if a normal-duration run does not reproduce them. Note core#352 does NOT explain the two f14_sigkill_recovery members -- its fix is already in this tree, carrying its 10 OwnedTree call sites -- and the macOS occurrence of the same test names was later traced to a keyring timeout (gh#1289), a different disease."
---

# Three Linux flakes seen once, under a degraded run

Ledgered for coverage; the honest disposition is deletion at expiry unless reproduced.
