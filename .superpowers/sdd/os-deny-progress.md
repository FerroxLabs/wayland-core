# OS-Sandbox Secret-Read-Deny — SDD progress (separate from spine ledger)

Plan: docs/superpowers/plans/2026-06-21-os-secret-read-deny.md (v3, 10 tasks).
Base: feat/workspace-policy HEAD e90d638a. Branch: feat/os-secret-deny.
PR placement (stacked vs extend #59) DEFERRED to Sean — DO NOT push, DO NOT touch PR.

- Task 1: b3e65085 — DONE — fs_read_deny field (#[serde(default)]) + SandboxBackend::enforces_read_deny() (default false) + 5 tests (37 pass, 0 warnings)
- Task 2: 7e446c4a — DONE — macOS SBPL read-deny after allows (last-match-wins) + enforces_read_deny()=true + 4 tests incl. live denial (41 pass, 0 code warnings)
- Task 3: cb7ba75b — DONE — Linux bwrap stat-at-bind overlay deny (/dev/null for files, --tmpfs for dirs) + fs_read_deny path validation + enforces_read_deny()=true + live test (bwrap-gated, Linux-only) (41 pass, 8 ignored, 0 warnings)
