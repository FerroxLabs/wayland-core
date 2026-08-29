---
issue: 383
repo: FerroxLabs/wayland-core
kind: defect
title: "is_project_secret still uses the weaker resolver, so a dangling symlink to a not-yet-existing project secret is not refused"
status: open
last_verified_commit: 9de21aa1
criteria:
  - id: c1
    text: "policy.is_project_secret(<in-root dangling symlink to a missing .env>) returns true"
    state: not-met
    owner: core
    note: "Filed 2026-08-29 by the 0.13.12 close-sweep (worklist defect D11, found while verifying wayland-core#356). Nothing has been done. The measured finding, verbatim: The same dangling-symlink escape #356 closed on `is_skill_source_path` and `is_repo_control_path` is still live on `is_project_secret`, because that predicate still uses the weaker `canon_for_scope` resolver. Measured on hetzner at origin/integ/f13 with a probe test: with `<root>/.env` absent and `<root>/notes.txt` a dangling symlink to it, `policy.is_project_secret(&notes.txt)` returns **false**, while the same `.env` named directly returns **true** and a link to an EXISTING `.env` returns **true**. Probe output verbatim: `PROBE live-link-to-existing-env: true` / `PROBE dangling-link-to-missing-env: false` / `PROBE same-under-canon_existing_ancestor-shape: true`. The read direction is closed (a secret that does not exist has nothing to leak, and a link to one that does exist canonicalizes and is caught); the residual is the WRITE direction — a Full-posture channel/remote session's SecretDenyFs would not refuse a write that lands as a not-yet-existing project secret through a dangling link. `is_project_secret`'s own doc says the Full deployment has no SandboxedFs wrapper to pre-canonicalize, so the raw path is what reaches the predicate there."
  - id: c2
    text: "A test asserts all three arms measured here -- live link to an existing .env, dangling link to a missing .env, and the direct name -- shown RED against today's canon_for_scope"
    state: not-met
    owner: core
    note: "Filed 2026-08-29 by the 0.13.12 close-sweep (worklist defect D11). Nothing has been done; the measurement is on c1 and the file:line anchors are in the prose below."
  - id: c3
    text: "workspace_policy.rs no longer carries two resolvers with different escape properties, or every remaining site names which resolver it uses and why"
    state: not-met
    owner: core
    note: "Filed 2026-08-29 by the 0.13.12 close-sweep (worklist defect D11). Nothing has been done; the measurement is on c1 and the file:line anchors are in the prose below."
---

The same dangling-symlink escape #356 closed on `is_skill_source_path` and `is_repo_control_path` is still live on `is_project_secret`, because that predicate still uses the weaker `canon_for_scope` resolver. Measured on hetzner at origin/integ/f13 with a probe test: with `<root>/.env` absent and `<root>/notes.txt` a dangling symlink to it, `policy.is_project_secret(&notes.txt)` returns **false**, while the same `.env` named directly returns **true** and a link to an EXISTING `.env` returns **true**. Probe output verbatim: `PROBE live-link-to-existing-env: true` / `PROBE dangling-link-to-missing-env: false` / `PROBE same-under-canon_existing_ancestor-shape: true`. The read direction is closed (a secret that does not exist has nothing to leak, and a link to one that does exist canonicalizes and is caught); the residual is the WRITE direction — a Full-posture channel/remote session's SecretDenyFs would not refuse a write that lands as a not-yet-existing project secret through a dangling link. `is_project_secret`'s own doc says the Full deployment has no SandboxedFs wrapper to pre-canonicalize, so the raw path is what reaches the predicate there.

**Where.** crates/wcore-tools/src/workspace_policy.rs:903 (`is_project_secret`, `let canon = canon_for_scope(path);`), resolver at :2935; same resolver at :936 (`is_vcs_content_store`), :1688, :1694, :852. Guard call site: crates/wcore-tools/src/vfs.rs:2043 (`SecretDenyFs::guard`).

**Why it matters.** #356 was filed precisely because two resolvers with different escape properties in one file invite the next author to pick whichever is nearer, and the fix's own ledger note asserts the file now holds only one. It does not. The weaker one still guards a security refusal, and I measured the identical escape class still passing through it — so the class the ticket describes is narrowed to two predicates, not closed.

Criteria are taken verbatim from the issue's Acceptance section. Nothing has been done: this entry exists so the release gate counts the work rather than anyone having to remember it.
