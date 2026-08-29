---
issue: 365
repo: FerroxLabs/wayland-core
kind: defect
title: "Container backend latches on a leftover container name, and attests a run that never happened"
status: open
last_verified_commit: 2a5aa7e9
criteria:
  - id: c1
    text: "A container left in Created under the name a new task would take does not fail that task -- either the name carries the nonce, or the submit path clears a conflicting name before docker run"
    state: not-met
    evidence: ""
    owner: core
    note: "MEASURED, both directions. crates/wcore-exec-backend/src/backends/container.rs:80-82 derives the name deterministically as wayland-f25-{task_id} and relies on `docker run --rm`, which removes on EXIT -- so a container that reaches Created but never starts is never removed, and every later run of that task id conflicts with it forever. `docker rm -f` exists only in cancel() at container.rs:256, never in the submit path. Two leftovers from 2026-08-28 16:36-16:37 UTC (wayland-f25-conf-container-ok, wayland-f25-conf-container-budget, both `Created`, both busybox:1.36) turned conformance_matrix red on the build host; removing them with no code change turned it green 2/2. The daemon error reproduced directly: >>> docker: Error response from daemon: Conflict. The container name \"/wayland-f25-conf-container-ok\" is already in use by container \"9e0bb9943941e2308fa2cf57db1bbb105a685036800e07cfa920b8b392ce14f5\". You have to remove (or rename) that container to be able to reuse that name. <<< The same argv with a fresh name exits 0."
  - id: c2
    text: "A daemon-level refusal is not reported as a task exit: exit-125 from docker run produces a distinct outcome, never a receipt asserting the task ran and returned 125. 126 and 127 considered in the same pass"
    state: not-met
    evidence: ""
    owner: core
    note: "This is the half that matters. Docker reserves 125 for `the daemon itself failed`, as distinct from 126 (not executable) and 127 (not found), which are the container's own. Nothing in container.rs distinguishes them -- grep for 125/126/127 in that file returns nothing. container.rs:213-247 takes output.status.code() and hands it to outcome_receipt as RunOutcome.exit_code whatever it is, so a task the daemon REFUSED TO CREATE yields a signed receipt asserting it ran and exited 125. For a subsystem whose whole purpose is signed attestation of what executed, conflating `did not run` with `ran and failed` is a correctness defect in the attestation itself, not a cosmetic one."
  - id: c3
    text: "The daemon's stderr reaches the operator on a daemon-level failure rather than being captured into a receipt nobody reads"
    state: not-met
    evidence: ""
    owner: core
    note: "The stderr IS captured -- container.rs:238 passes output.stderr into RunOutcome -- so this is not a data-loss bug in the product; the bytes exist. Nothing consults them on the failure path, and the conformance report renders only `Failure { code: \"exit-125\" }`. The message docker returns names the exact problem and the exact remedy. Note the polarity before fixing: warn! is not a fix -- RUST_LOG is unset for ordinary users, so only ERROR reaches stderr, and a log-level bump can never make the user told."
  - id: c4
    text: "A red arm is quoted verbatim: the new guard reverted, the test failing, restored and green, with the mutation shown to have landed on code"
    state: not-met
    evidence: ""
    owner: core
  - id: c5
    text: "conformance_matrix passes on a host that has run it before with a leftover container present -- the regression test creates the wedged container itself rather than assuming a clean daemon"
    state: not-met
    evidence: ""
    owner: core
    note: "This is the criterion that keeps the class closed. CI never sees the defect because hosted runners are fresh, so a test that assumes a clean daemon is structurally incapable of catching it -- it reproduces only where the backend has run before, which is every operator's machine and none of our gates. A test that only passes on a fresh host is the same shape of blind spot as the one that hid this."
  - id: c6
    text: "The orphan-scan path is checked for the same latch: state whether docker ps -a --filter label=wayland.task.nonce= would have found these two, and if not, why the label was absent"
    state: not-met
    evidence: ""
    owner: core
    note: "container.rs:297 already enumerates by nonce label, and the submit path sets --label wayland.task.nonce=<nonce> at container.rs:191-192, so on the face of it the scan SHOULD have found them. Establish whether it did and was ignored, or whether a container that never started carries no label. Do not assume; the answer decides whether this is one defect or two."
---

Found while auditing why `integ/f13` reported 2 nextest failures during the 0.13.12
integration. Those two had been recorded in a handoff as "the known `conformance_matrix`
exclusion". They were not. That framing was wrong, and this is what was actually
happening -- the test was doing exactly its job and reporting a real product defect on
the only kind of host that can exhibit it.

`conformance_matrix`'s own module docs state the contract it was failing under: "fails
only on a backend that was exercised and failed -- an honestly reported unavailable
surface is a result, not a red." The container backend WAS exercised (the daemon answered
a version ping) and it DID fail. Reading the failure as an unexercised-surface skip is
what let it sit for a day.
