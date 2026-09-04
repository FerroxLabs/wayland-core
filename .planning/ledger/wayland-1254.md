---
issue: 1254
repo: FerroxLabs/wayland
kind: defect
title: "preflight.sh prints PRE-FLIGHT PASSED on a tree CI reds: a gate's self-disclosed downgrade is discarded on the success path"
status: closed
last_verified_commit: 6d5be540b
criteria:
  - id: c1
    text: "On a shallow clone of a tree whose full-clone python3 scripts/check-criteria-ledger.py --offline is EXIT=1, bash scripts/preflight.sh does NOT exit 0 and does NOT print PRE-FLIGHT PASSED"
    state: met
    evidence: "file:scripts/preflight.sh:303:PREFLIGHT CLONE-DEPTH GUARD: this worktree is a SHALLOW clone"
    owner: core
    note: "MET 2026-09-04 on bc9288da5. THE PRECONDITION WAS BUILT, NOT ASSUMED: a clone of this lane branch with `.planning/ledger/wayland-1088.md`'s last_verified_commit rewritten to an unreachable sha, committed, then re-cloned at --depth 1. Full clone -> `check-criteria-ledger.py --offline` EXIT=1 ('last_verified_commit 0123... is not a commit in this tree'). Shallow clone of that same commit, same working tree -> EXIT=0, 'OK: every ledger file parses'. RED ARM, today's preflight on that shallow clone: every line `ok`, banner `PRE-FLIGHT PASSED`, EXIT=0 -- the defect reproduced on demand. GREEN ARM, this commit's preflight on the identical shallow clone: 'PREFLIGHT CLONE-DEPTH GUARD: this worktree is a SHALLOW clone, and ci.yml gives the same gates a `fetch-depth: 0` checkout', EXIT=2, no banner. The guard is DERIVED, not hard-coded: it reads the same ci-linux region the DRIFT GUARD parses and arms only because `fetch-depth: 0` is set there. It refuses rather than downgrading, because preflight's whole claim is that it predicts CI, and from a shallower checkout than CI's it cannot -- a prediction that cannot be made must not be printed. NOTE FOR THE FOLLOW-UP: the durable form is for check-criteria-ledger.py to return the reserved degraded exit code when it skips sha resolution; that is a change to the gate and is not made here (this lane owns preflight.sh only). Until then the shallow path is closed by refusal, not by rendering."
  - id: c2
    text: "On any tree where check-criteria-ledger.py --offline exits 0, the string THIS IS NOT A PASS appears in bash scripts/preflight.sh's own stdout"
    state: met
    evidence: "file:scripts/preflight.sh:159:printf '%s\n' "$gate_out" | sed 's/^/        | /'"
    owner: core
    note: "MET 2026-09-04 on bc9288da5. BOTH ARMS ON THE SAME TREE, /root/L2-1254 at this commit, where `check-criteria-ledger.py --offline` EXIT=0. RED: today's preflight.sh -> `grep -c 'THIS IS NOT A PASS'` over its full stdout = 0; the operator is shown the bare line `ok    python3 scripts/check-criteria-ledger.py --offline` and the banner PRE-FLIGHT PASSED. GREEN: this commit -> count = 1, the entry renders `DEGRADED`, and the gate's OWN six lines are echoed under it, including 'OFFLINE: tracker coverage and ledger/GitHub divergence were NOT checked. THIS IS NOT A PASS for coverage'. The words are the gate's, quoted, not preflight's paraphrase -- which matters, because a paraphrase is a second implementation of the disclosure and drifts from the first. The degraded rendering is the only place a gate's stdout now reaches stdout on a non-failing run; that is the whole of the fix for this criterion."
  - id: c3
    text: "Every entry in preflight's GATES is rendered from a three-valued status whose degraded value is produced BY THE GATE (exit code or machine-readable marker), not inferred by preflight from a substring search over free-form stdout"
    state: met
    evidence: "file:scripts/preflight.sh:349:render_gate "${entry%%|*}" "${entry#*|}""
    owner: core
    note: "MET 2026-09-04 on bc9288da5. Every GATES entry, and the corpus gate, goes through one `render_gate` -> `run_gate`, and `run_gate`'s ONLY inputs to the status are the exit code and the entry's declared mode. It never reads the captured output; `$gate_out` is display-only. THREE VALUES, THREE RENDERINGS: `ok`, `DEGRADED` (with the gate's own words), `FAIL`. The degraded value has two total sources, neither of them prose. (1) RESERVED EXIT CODE 3 = ran-but-disarmed. This is the general mechanism: any gate that adopts it is rendered DEGRADED forever after with no edit here, and a gate that does not signal degraded is not degraded, which is what makes the rule decidable. (2) THE GATE'S OWN CLI. One entry is invoked by preflight in a mode the gate itself defines as disarming -- `check-criteria-ledger.py --offline`, whose documented meaning is 'tracker coverage and ledger/GitHub divergence were NOT checked'. The marker is a flag on the gate's declared interface, read from the invocation preflight itself wrote; it is not an open alphabet of English. That declaration cannot rot into a lie: `run_gate` re-verifies on every run that the declared flag is still present in the invocation and renders FAIL, not ok, if it is not (self-test arm 'disarmed declaration without its flag -> fail'). MUTATION C, the arm that matters for this criterion: re-introducing a substring search (`case $gate_out in *THIS IS NOT A PASS*|*NOTE:*) gate_status=degraded`) makes the self-test go RED at 'armed gate printing THIS IS NOT A PASS, exit 0 -> ok', EXIT=1. The forbidden fix is now itself gated."
  - id: c4
    text: "A self-test carries both directions -- a fully-armed gate still renders ok and preflight still exits 0, AND a degraded gate is rendered distinguishably from ok -- shown RED against today's scripts/preflight.sh"
    state: met
    evidence: "file:scripts/preflight.sh:202:armed gate printing THIS IS NOT A PASS, exit 0 -> ok"
    owner: core
    note: "MET 2026-09-04 on bc9288da5. `bash scripts/preflight.sh --self-test` -> 9 arms, all ok, 'self-test: both directions proven', EXIT=0. It drives the REAL `run_gate`/`render_gate` against synthetic gates, not a reimplementation of the rule. Positive half: 'armed gate, exit 0 -> ok'; the real run also still exits 0 (green arm on this tree: EXIT=0). Negative half: reserved exit 3 -> degraded; exit 1 -> fail; ok and degraded render differently; a degraded rendering carries the gate's own words; an ok gate is never labelled DEGRADED. RED AGAINST TODAY: `bash <origin/main preflight.sh> --self-test` ignores the flag entirely, runs the real gates and prints PRE-FLIGHT PASSED -- `grep -c 'both directions proven'` = 0. NOT VACUOUS, three mutations of this commit's own script, each run through the same self-test: (A) collapse the reserved degraded code into ok -> RED on 2 arms; (B) render everything degraded, the failure mode this criterion is written to catch -> RED on 3 arms including 'armed gate, exit 0 -> ok'; (C) decide status by substring search over gate stdout -> RED on the prose control. All three EXIT=1 with 'self-test: BROKEN -- the pre-flight cannot be trusted'. Mutations were made on scratch copies under /root/scratch-L2-1254, never in the worktree; `git status --porcelain` empty throughout."
  - id: c5
    text: "just push fails on a tree where bash scripts/preflight.sh fails -- verified by making a ledger last_verified_commit a non-ancestor and confirming just push refuses (today push chains neither preflight nor ledger-check, and there are no git hooks)"
    state: met
    evidence: "file:justfile:326:push *ARGS: lint-fix fmt _auto-commit-fixes preflight test"
    owner: core
    note: "MET 2026-09-04 on bc9288da5. `justfile:326` -- `push *ARGS: lint-fix fmt _auto-commit-fixes preflight test`, plus a `preflight` recipe at :313. Placed AFTER `_auto-commit-fixes` on purpose: the gate has to see the tree actually being pushed, and a lint/fmt auto-commit changes that tree. MEASURED END TO END, not argued from just semantics. A full (non-shallow) clone of this lane branch with `.planning/ledger/wayland-1088.md`'s last_verified_commit rewritten to an unreachable sha -- exactly the condition c5 names -- had its remote removed, then `just push` was run for real (`vx` shimmed to exec its arguments, since vx is not installed on this host; that swaps the toolchain pin, not the recipe graph). It ran lint-fix, fmt and _auto-commit-fixes (nothing to commit), then: `PRE-FLIGHT FAILED -- CI would red on this tree. Fix before pushing.` followed by `error: recipe `preflight` failed on line 314 with exit code 1`. `test` never ran and `git push` never ran -- zero occurrences of any push output in the 813-line log. RED-ARM HYGIENE: run against the COMMITTED tree, in a throwaway clone under /root/scratch-L2-1254, with the origin remote removed so a hypothetical pass could not have pushed anywhere. THE NEGATIVE HALF is the everyday green run in this worktree: preflight EXIT=0 on the same commit, so this is not a chain that refuses every push. BEFORE: `push` was `lint-fix fmt _auto-commit-fixes test`, the string `preflight` appeared nowhere in the justfile, and there are still no git hooks (`ls .git/hooks | grep -v sample` and `git config core.hooksPath` both empty)."
---

`scripts/preflight.sh` exists so a lane can predict CI's host-side gates in 2-3
minutes instead of 67. Measured 2026-08-30, it can report `PRE-FLIGHT PASSED`
on a commit where CI's own ledger step is red.

Two parts. The shallow-clone downgrade inside `check-criteria-ledger.py` is
DELIBERATE and correct in isolation -- without it the check produces one
guaranteed problem per ledger file on a `fetch-depth: 1` checkout and can never
pass on any tree, and a gate with no reachable pass state is worth exactly as
much as one that cannot fail. That script says so out loud, on purpose: "Say
it. A check that quietly stops running is indistinguishable from one that ran
and passed, and that is how a gate rots between releases."

The defect is the second part: `preflight.sh` captures each gate's output into
`$out` and prints it only on the failure branch, so the disclosure is
discarded. The script that insists on saying it and the script that decides
what the operator sees disagree, and preflight wins. That converts "I did not
check" into "I checked and it was fine" -- the precise inversion preflight's
own header was written to prevent, arriving through a different door than the
stale-list drift the DRIFT GUARD already covers.

Filed by the lane that was refuted for quoting EXIT=0 on these very two gates.
The lane's numbers were most likely truthful when taken (run before a squash
that orphaned the commit its ledgers cited), but chasing the shape question
found a path where the gate genuinely runs and genuinely appears to pass, on
the shipped SHA, with the defect present.

FIXED 2026-09-04 in bc9288da5. The status is now three-valued and is never read
out of a gate's prose: exit code 3 is reserved for "ran, but disarmed part of
itself", and one entry is declared disarmed by the gate's own CLI flag, with
that declaration re-verified against the invocation on every run. DEGRADED
carries the gate's own sentence into preflight's stdout and downgrades the
summary line to `PRE-FLIGHT INCOMPLETE`, which is why an everyday green run now
reads INCOMPLETE rather than PASSED: preflight always runs the ledger gate
`--offline`, so on any real tree something genuinely was not checked. Exit 0 is
unchanged for that case, so the machine verdict a caller gates on is preserved
while the human-facing line stops lying.

TWO THINGS FOUND ON THE WAY, both recorded rather than left implied:

  * `scripts/check-ci-step-suppression.py` has been a ci.yml host-side gate
    since 0.13.12 and was not in `GATES`, so the DRIFT GUARD was already
    refusing to run (`EXIT=2`) on untouched `origin/main` -- measured on
    509f4426b before any change here. That is the guard working exactly as
    designed; the remedy it prints is the two lines now added. Any lane that
    ran the pre-flight since 0.13.12 got a refusal, not a verdict.

  * `.planning/ledger/wayland-1256.md` c2 anchored `file:scripts/preflight.sh:101`,
    and this change moved that content to line 345. Re-anchored mechanically in
    the same commit; the claim is untouched, only its recorded position moved.
    An anchor with a +/-20 window into a script that is under active repair is
    a standing cost, not a defect of either file.
