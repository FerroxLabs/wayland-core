---
issue: 1298
repo: FerroxLabs/wayland
kind: defect
title: "Signing seeds are published with a non-atomic write: a torn read refuses permanently, and #1250's recorded root cause is refuted by reproduction"
status: open
last_verified_commit: 509f4426b
criteria:
  - id: c1
    text: "Both seed writers publish through one helper that stages to a per-call temporary file and links it into place, so no reader can observe a partial seed. Measured by a concurrency test that FAILS against the pre-fix body and passes after, not by inspection."
    state: met
    evidence: "test:crates/wcore-exec-backend/src/backends/mod.rs::concurrent_first_use_never_observes_a_partial_seed"
    owner: core
    note: "MET at 509f4426b -- POST-MERGE SYNC. The anchor was 6e4eca07, a tree that does NOT carry the fix, so every criterion here was not-met BY CONSTRUCTION; 509f4426b carries all three fix commits (919ecf117, 6d45bb623, 75cc3682b), each verified an ancestor. BOTH WRITERS go through ONE helper: backends::load_or_create_seed delegates to load_or_create_seed_at, and node::pairing::load_or_create_node_seed (pairing.rs:280) now calls the same helper instead of the byte-identical copy it carried; those are the only two seed writers in the crate. BOTH ARMS RUN 2026-09-04 on hetzner, with a control. RED ARM: a detached worktree at 509f4426b with the publish mutated back to the verbatim pre-fix body (bare fs::write onto the TARGET name, chmod afterwards). The fix was confirmed ABSENT before running -- grep for std::fs::hard_link returned zero call sites -- and the file was touched after mutating so cargo could not serve a stale object. Result: FAILED on round 0 of 24 with the production signature, verbatim: 'round 0: concurrent first use refused: receipt is invalid: backend signing seed at /tmp/.tmpImRoPk/container.key is not 32 bytes'. GREEN ARM at 509f4426b unmutated: 5 passed / 0 failed. CONTROL: an_existing_seed_is_returned_unchanged PASSES in BOTH arms, so the red arm discriminates the torn write rather than being a tree that simply fails."
  - id: c2
    text: "Publication is exclusive: N concurrent first-use callers all return the seed that was actually persisted. Measured by the same test asserting every returned seed equals the file's contents."
    state: met
    evidence: "file:crates/wcore-exec-backend/src/backends/mod.rs:469:let published = std::fs::hard_link(&staging, path);"
    owner: core
    note: "MET at 509f4426b, and NOT redundant with c1. The first attempt at the fix used write-then-rename, which is atomic but not EXCLUSIVE: last writer wins the file, so an earlier racer returns a seed the disk does not have and signs with an identity that changes on its next start. SEPARATE RED ARM RUN 2026-09-04, independent of c1's: the single publish call site was mutated hard_link -> rename in a detached worktree (grep confirmed zero remaining std::fs::hard_link call sites, file touched afterwards), and the same test then FAILED on round 0 on the exclusivity assertion alone, verbatim 'round 0: caller 0 returned a seed that is not the persisted one', left [14, 108, 183, ...] right [38, 242, 67, ...]. CONTROL: the other four tests in the module PASSED in that same arm, so the persisted-equality assertion is the only thing the mutation moved. hard_link publishes a complete file like rename does and additionally fails with AlreadyExists rather than overwriting, so exactly one racer creates the identity and every loser falls through and reads it back."
  - id: c3
    text: "The seed is never reachable under its real name at a mode other than 0600. Measured on unix by reading the published file's mode."
    state: met
    evidence: "file:crates/wcore-exec-backend/src/backends/mod.rs:460:set_permissions(&staging"
    owner: core
    note: "MET at 509f4426b, but NOT by the test alone, and that is recorded rather than glossed. the_seed_is_never_published_world_readable reads the published file's mode and asserts 0600 -- and it PASSES against the pre-fix create-write-chmod body too: measured, it passed inside c1's red arm. It grades the END STATE, so it cannot see the window the criterion is actually about. The window was measured directly instead, by strace of the fail_closed_matrix binary in both arms on 2026-09-04, tracing openat/link/rename/chmod. RED (pre-fix ordering, mutated worktree): openat(AT_FDCWD, '/tmp/.tmpBM3mGP/keys/container.key', O_WRONLY|O_CREAT|O_TRUNC|O_CLOEXEC, 0666) on the REAL NAME, then chmod('/tmp/.tmpBM3mGP/keys/container.key', 0600) -- 16 chmod calls on real *.key names over the run. GREEN at 509f4426b: ZERO chmod on any real *.key name; the real name appears only as linkat(staging -> container.key) and as O_RDONLY reads, while every chmod lands on a '.key.tmp.<pid>.<n>' staging name. Mode is an inode property and hard_link creates no new inode, so there is no instant at which the published name resolves to an inode at another mode. The two test binaries were confirmed distinct by sha256 (b9a960cc... red, 16fe519c... green)."
  - id: c4
    text: "A corrupt (non-32-byte) seed is still refused rather than silently regenerated, and the refusal names the recovery. Measured by a test asserting the message and that the file survives the refusal."
    state: met
    evidence: "test:crates/wcore-exec-backend/src/backends/mod.rs::a_corrupt_seed_is_refused_with_a_recovery_instruction"
    owner: core
    note: "MET at 509f4426b. This is the EXISTING-DAMAGE half -- a seed already torn on disk, not merely a future write. Deliberately NOT self-healing: regenerating would rotate an identity behind the operator's back. The test writes a 31-byte file, asserts the message contains 'is not 32 bytes' AND 'Delete it', and asserts the file still reads 31 bytes after the refusal, so refusing does not destroy what the operator may want to inspect. RED ARM RUN 2026-09-04: the recovery clause was mutated out of the one emitter, leaving the verbatim pre-fix message; the test then FAILED on the actionability assertion, verbatim 'the refusal must be actionable: receipt is invalid: backend signing seed at /tmp/.tmpIOa9Yz/ssh.key is not 32 bytes', while the other four tests PASSED. The assertion is live, not decorative. RESIDUE, not owed by any criterion here but recorded so it is not lost: a crash BETWEEN the staging write and the hard_link leaves an orphan '<name>.key.tmp.<pid>.<n>' that nothing ever collects. It cannot brick the backend -- only the target name is ever read -- and no_staging_file_survives_a_successful_publish covers the success path only, so the crash path is untested."
  - id: c5
    text: "The refuted \"state dir removed by a sibling\" attribution is corrected in every file that asserts it, each naming the torn write instead. Graded by a grep returning zero occurrences of the refuted claim, with a control proving the query matches."
    state: met
    evidence: "absent:crates/wcore-exec-backend/tests/conformance_matrix.rs::had just been removed by a"
    owner: core
    note: "MET at 509f4426b, graded by grep WITH the control the criterion demands -- and the first control query written for this sync returned ZERO and would have read as proof. It was wrong: the refuted clause wraps a line ('removed by a' / '/// sibling finishing first'), so a fragment spanning the break can never match. The query that works is the unbroken fragment 'had just been removed by a'. GREEN: git grep -c over crates/ at 509f4426b returns ZERO. CONTROL A (polarity): the identical query at 75cc3682b^ returns exactly 1 hit in each of the FOUR files -- tests/conformance_matrix.rs, tests/container_orphan_scan.rs, tests/container_wedge.rs, tests/live_equivalence.rs -- so the query does match the claim when the claim is present. CONTROL B (edited, not deleted): 'deleted out from under them', the neighbouring sentence that is CORRECT and was deliberately kept, still returns 1 hit in each of the same four files at 509f4426b. SCOPE: FOUR files, not the six the issue body said. src/registry.rs and tests/fail_closed_matrix.rs were re-read and are NOT wrong -- they describe the env var redirecting a sibling's record/load/list calls, a real and separate hazard, and neither attributes the seed failure. The grep is scoped to crates/: this ledger's own c5 text quotes the refuted phrase, and a repo-wide query would hit that quotation and the unrelated wayland-1308 entry."
  - id: c6
    text: "The three unguarded fail_closed_matrix.rs tests stop writing into the operator's real config directory during a test run. Graded by a test-env-globals check rather than by inspection."
    state: not-met
    owner: core
    note: "NOT MET at 509f4426b, and deliberately left so after the behaviour was proven fixed. THE BEHAVIOUR IS FIXED, and that was MEASURED rather than inspected: the fail_closed_matrix binary was run under a sentinel WAYLAND_HOME in both arms on 2026-09-04. RED at 75cc3682b^ (= 6d45bb623: atomic publish already in, guards not yet): the sentinel gained exec-backend/keys/cloud.key, container.key, local.key, ssh.key and exec-backend/instance-id -- while all 13 tests still reported ok, which is exactly why this was invisible. GREEN at 509f4426b: the sentinel tree is EMPTY, 13 tests ok. The two binaries differ by sha256. WHAT IS NOT MET is the criterion's own grading clause. The only test-env-globals check in this tree is scripts/check-test-env-globals.py, and it returns RC=0 IDENTICALLY at the defective commit 6d45bb623 and at 509f4426b: it scans for a test writing a process-global ENV VAR that its own binary's production code reads, and this hazard was never an env write -- registry::state_dir() simply fell through to wcore_config::wayland_config_dir(). A gate that returns the same answer on both sides of the defect cannot grade it, and grading c6 met on it would be certifying against a check that cannot fail. Concretely: nothing in this repo will catch a FOURTH unguarded reference_backends caller added tomorrow, which is the durable guarantee 'graded by a check rather than by inspection' was written to buy. TO CLOSE: extend check-test-env-globals.py (or add a sibling check) to fail a test that constructs backends with no StateDirGuard, or assert the sentinel-empty property from a test."
---

# The error string is the evidence

`backend signing seed at <path> is not 32 bytes` has exactly one emitter, and reading that function
settles the root cause without running anything: the error is reachable only when the file EXISTS
and reads at a length other than 32. A deleted state dir makes `fs::read` fail, and control falls
through to create-and-write. So the recorded cause -- a sibling removing the state dir -- cannot
produce the signature it was written to explain. A torn write can, and does: 16 threads reproduced
it on the first round against the pre-fix body.

The crate already had the right pattern in `registry::record`, commented "a cancel racing a run must
never read a half file". The two seed writers were the sites that did not get it.

# What the post-merge sync found that the fix PR did not

c1-c5 are met at 509f4426b, each with a red arm that fails on the specific assertion it names and a
control that passes in both arms. c6 is the exception, and it is instructive: the fix it asks for
LANDED and was measured landing -- three tests that used to write four signing seeds into the
operator's real config directory now write none -- but the criterion also asked to be graded by a
test-env-globals check, and the only such check in the tree answers RC=0 on both sides of the
defect. The hazard was never an environment variable; it was `registry::state_dir()` falling through
to `wayland_config_dir()`. So the behaviour is fixed and the guarantee is not: nothing here fails
when a fourth unguarded caller is added. That is the difference between a defect being closed and a
class being closed, and c6 stays open on the second.
