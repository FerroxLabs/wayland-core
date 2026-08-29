---
issue: 379
repo: FerroxLabs/wayland-core
kind: defect
title: "The #338 setsid hardening turns a quarantine git timeout into an unreaped detached process tree"
status: open
last_verified_commit: 9de21aa1
criteria:
  - id: c1
    text: "The quarantine timeout path kills the whole session/process group it created, not the direct child alone"
    state: not-met
    owner: core
    note: "Filed 2026-08-29 by the 0.13.12 close-sweep (worklist defect D5, found while verifying FerroxLabs/wayland-core#338). Nothing has been done. The measured finding, verbatim: The setsid hardening converts the quarantine git timeout path into an unreaped detached process tree. `harden_against_credential_prompt` (crates/wcore-cli/src/plugin/quarantine.rs:344-356) puts every quarantine git child in a NEW SESSION, but the timeout path (`run_git`, quarantine.rs:415-418) still does only `let _ = child.kill(); let _ = child.wait();` -- a single-pid SIGKILL. There is no process_group, no killpg, no OwnedTree anywhere in quarantine.rs (grep for `process_group|OwnedTree|killpg|libc::kill` returns nothing). Descendants git spawned -- exactly the credential/askpass/transport helpers whose backgrounded workers DRAIN_GRACE was written for -- are now in a session the CLI does not own, so no group signal reaches them and terminal hangup no longer reaps them either. I demonstrated the mechanism on hetzner: `setsid sh -c 'sleep 300 & ...; sleep 300'`, then SIGKILL the direct child; `ps -o pid,sid,pgid` showed the grandchild pid 2438901 still alive in SID 2438889, the detached session, not the shell's."
  - id: c2
    text: "A test spawns a quarantine git child that backgrounds a descendant, trips the timeout, and asserts no descendant survives; shown RED against today's kill-the-leaf code"
    state: not-met
    owner: core
    note: "Filed 2026-08-29 by the 0.13.12 close-sweep (worklist defect D5). Nothing has been done; the measurement is on c1 and the file:line anchors are in the prose below."
  - id: c3
    text: "The teardown decision is written down beside the setsid decision (DECISIONS.md Q-338c4 or its successor), as MASTER-PLAN.md:202 required"
    state: not-met
    owner: core
    note: "Filed 2026-08-29 by the 0.13.12 close-sweep (worklist defect D5). Nothing has been done; the measurement is on c1 and the file:line anchors are in the prose below."
---

The setsid hardening converts the quarantine git timeout path into an unreaped detached process tree. `harden_against_credential_prompt` (crates/wcore-cli/src/plugin/quarantine.rs:344-356) puts every quarantine git child in a NEW SESSION, but the timeout path (`run_git`, quarantine.rs:415-418) still does only `let _ = child.kill(); let _ = child.wait();` -- a single-pid SIGKILL. There is no process_group, no killpg, no OwnedTree anywhere in quarantine.rs (grep for `process_group|OwnedTree|killpg|libc::kill` returns nothing). Descendants git spawned -- exactly the credential/askpass/transport helpers whose backgrounded workers DRAIN_GRACE was written for -- are now in a session the CLI does not own, so no group signal reaches them and terminal hangup no longer reaps them either. I demonstrated the mechanism on hetzner: `setsid sh -c 'sleep 300 & ...; sleep 300'`, then SIGKILL the direct child; `ps -o pid,sid,pgid` showed the grandchild pid 2438901 still alive in SID 2438889, the detached session, not the shell's.

**Where.** crates/wcore-cli/src/plugin/quarantine.rs:344-356 (setsid) vs :415-418 (timeout kill); the leak-shaped case is already exercised by the passing test plugin::quarantine::tests::a_helper_holding_a_pipe_is_reported_instead_of_hanging_the_install

**Why it matters.** The project's own plan predicted this and the change shipped without it. .planning/MASTER-PLAN.md:144 says verbatim: 'setsid/process_group(0) detaches git but does not close the pipes or extend the kill (quarantine.rs:29-44 DRAIN_GRACE; timeout path ~:325 kills the direct child only). Split, it converts a bounded wedge into an unreaped detached tree.' MASTER-PLAN.md:202 obliged 'Layers 1+2 as ONE change, teardown decided in the same change'. Layers 1+2 did land together; the teardown was neither decided nor implemented, and Q-338c4 in DECISIONS.md does not mention it. Net effect on a user: a hung or malicious plugin source now leaves helper processes running with no owner after the install reports an error, and they are strictly less reachable than before the fix.

Criteria are taken verbatim from the issue's Acceptance section. Nothing has been done: this entry exists so the release gate counts the work rather than anyone having to remember it.
