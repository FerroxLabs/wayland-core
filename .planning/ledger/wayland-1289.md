---
issue: 1289
repo: FerroxLabs/wayland
kind: defect
title: "macOS keyring does not answer within the 5s credential-store timeout; f14_sigkill_recovery hard-fails 3/3"
status: open
last_verified_commit: 67fa14db6
criteria:
  - id: c1
    text: "The 5s credential-store wait is either shown to be adequate under parallel nextest on macOS, or the timeout is raised, retried, or the affected tests are serialised so a contended keychain does not hard-fail them."
    state: not-met
    evidence: "file:crates/wcore-cli/tests/f14_sigkill_recovery.rs"
    owner: core
    note: "Filed 2026-09-01. NOT FIXED IN 0.13.12 and NOT CAUSED BY IT. Milestoned 0.13.13. In run 33499226403 three tests failed ALL THREE attempts -- deterministic within that run, so no allowlist entry could or should have rescued them -- with SessionAuthority('the configured credential store did not answer within 5s') raised at f14_sigkill_recovery.rs:593 and surfaced at :641 as a non-zero seeder exit. That string appears ZERO times across all five previously stored macos-latest artifacts, and no credential, session or keyring code is in the 0.13.12 diff. DID NOT REPRODUCE on the next run (17,672/17,672 passed, 0 retries), and that clean run was 37% faster (1,332s vs 2,098s), which is consistent with the contention hypothesis -- nextest runs at num-cpus and macOS serialises keychain access -- but one datapoint does not measure a rate, so this stays open rather than being closed as transient."
---

# A 5s ceiling on the OS keyring

Also a real user-facing failure mode on a locked login keychain, not only a CI artifact.
