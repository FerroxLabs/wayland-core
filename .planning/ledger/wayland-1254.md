---
issue: 1254
repo: FerroxLabs/wayland
kind: defect
title: "preflight.sh prints PRE-FLIGHT PASSED on a tree CI reds: a gate's self-disclosed downgrade is discarded on the success path"
status: open
last_verified_commit: a63defc18
criteria:
  - id: c1
    text: "On a shallow clone of a tree whose full-clone python3 scripts/check-criteria-ledger.py --offline is EXIT=1, bash scripts/preflight.sh does NOT exit 0 and does NOT print PRE-FLIGHT PASSED"
    state: not-met
    owner: core
    note: "Filed 2026-08-30 by lane f13-w2-provider-url while answering its own refutation. Nothing has been done. MEASURED on 388de5d70 with three ledgers carrying a non-ancestor last_verified_commit: full clone -> ledger EXIT=1 (3 problems) and preflight EXIT=1 'PRE-FLIGHT FAILED'; shallow clone of the SAME commit, same working tree, same bad anchors -> ledger EXIT=0 and preflight EXIT=0 'PRE-FLIGHT PASSED'. CI is not fooled: ci.yml sets fetch-depth: 0 on the ledger job at .github/workflows/ci.yml:1387-1399 precisely to arm this. So preflight, whose stated purpose is to predict CI's host-side gates, predicts PASS where CI gives FAIL."
  - id: c2
    text: "On any tree where check-criteria-ledger.py --offline exits 0, the string THIS IS NOT A PASS appears in bash scripts/preflight.sh's own stdout"
    state: not-met
    owner: core
    note: "Filed 2026-08-30 by lane f13-w2-provider-url. Nothing has been done. This one is live on EVERY green run, not only under a shallow clone, and it is the cheapest possible demonstration of the shape. check-criteria-ledger.py --offline always prints 'OFFLINE: tracker coverage and ledger/GitHub divergence were NOT checked. THIS IS NOT A PASS for coverage'. preflight.sh captures gate stdout into $out and prints it ONLY on the failure branch, so that sentence has never once reached an operator reading a green preflight; they are shown the bare line 'ok python3 scripts/check-criteria-ledger.py --offline'. The gate says it is not a pass and the operator is shown ok."
  - id: c3
    text: "Every entry in preflight's GATES is rendered from a three-valued status whose degraded value is produced BY THE GATE (exit code or machine-readable marker), not inferred by preflight from a substring search over free-form stdout"
    state: not-met
    owner: core
    note: "Filed 2026-08-30 by lane f13-w2-provider-url. Nothing has been done. THIS IS THE SHAPE CRITERION and it is written to forbid the tempting fix. Grepping preflight's captured output for 'NOTE:' or 'THIS IS NOT A PASS' reproduces the defect: 'does this free-form text disclose a downgrade?' is undecidable over an open alphabet of future wordings, and the next gate to phrase a downgrade differently is silently ok again. The decidable, total form inverts it -- a gate signals its own degraded state out of band (reserved exit code, e.g. 0 armed / 3 ran-but-degraded / other non-zero fail) and preflight renders three values, never collapsing degraded into ok. Then no future gate can add a silent downgrade, because a gate that does not signal degraded is not degraded."
  - id: c4
    text: "A self-test carries both directions -- a fully-armed gate still renders ok and preflight still exits 0, AND a degraded gate is rendered distinguishably from ok -- shown RED against today's scripts/preflight.sh"
    state: not-met
    owner: core
    note: "Filed 2026-08-30 by lane f13-w2-provider-url. Nothing has been done. Both directions are named for the reason this repo keeps re-learning: a change that renders EVERYTHING as degraded, or that reds preflight on any NOTE at all, satisfies the positive half and destroys the gate. preflight.sh already carries a self-testing precedent in its inline DRIFT GUARD; the sibling scripts (check-criteria-ledger.py --self-test) exit 0 with 'self-test: both directions proven' and are the pattern to copy."
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
