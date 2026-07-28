# Control — the builder refactor is behaviour-preserving, measured A/B

The superseding receipt is only trustworthy if the tool that issues it still behaves exactly as
the tool that issued the original. Asserting that from a code review is not evidence. This is
the A/B, run on identical inputs in a pristine checkout at the commit where the original receipt
was signed.

## Method

```bash
git worktree add --detach .../lane-28-receipt-ctl 3f85026a     # the commit that wrote the receipt
git show 3f85026a:.planning/phases/28-*/28-04-CERTIFICATION-RECEIPT.json > committed-receipt.json
```

At `3f85026a` the on-disk ledger is the PRE-adjudication one
(`findings.tsv` sha256 `511e19dd1954630eb533446f33f84880e6d80b79523d6cd6fa00f863b3547a03`), so
both tools see exactly the inputs the original signature was made over.

## Result

| Leg | Command | vs committed receipt | rc |
|---|---|---|---|
| **A** — original tool, unmodified | `python3 f28-build-receipt.py` (defaults) | `cmp` | **0 — byte-identical** |
| **B** — refactored tool | `python3 f28-build-receipt.py --out 28-04-AB-NEWTOOL.json` | `cmp` | **0 — byte-identical** |

Both legs reproduce the signed artifact bit for bit, including `body_sha256` and
`signature_base64`. The refactor changed no output on the default path.

Leg A also independently re-establishes that the receipt is *reproducible* — the deterministic
phase-scoped key means a later reader can re-derive and re-verify it without the minting machine,
which is the property the builder's own docstring claims.

**`git diff --stat` was NOT used to grade this.** It exits 0 unconditionally (lane brief §3.2),
so an empty diff-stat proves nothing about status. Every comparison above is `cmp` with its own
exit code read back.

## The new overwrite guard, proven in both directions

A tool that will silently replace a signed receipt when the evidence moves makes its own
signatures worthless. The guard refuses that, and a guard nobody has seen refuse anything is not
a guard:

| Case | Expectation | Observed |
|---|---|---|
| write to the path of an existing phase-scoped-signed receipt | REFUSE | `rc=2`, refusal names `--supersede`; the receipt `cmp`s identical afterwards (`rc=0`) |
| write to a fresh path (negative control) | ALLOW | `rc=0` |

The negative control matters: a guard that refused everything would also show `rc=2` on the first
row and would be useless.

## Second-order note — why the in-tree control was discarded

An earlier, weaker control rebuilt at HEAD with `--ledger` pinned to the historical TSV. It came
out non-identical in exactly three hunks, and the cause is worth recording because it is not a
defect:

```
"sha256": "511e19dd…", "bytes": 55568     <- original: the artifact binding for findings.tsv
"sha256": "51ddac03…", "bytes": 58035     <- rebuild:  the ledger ON DISK has since moved
```

`--ledger` changes which file the FINDINGS are read from; it does not change which files the
ARTIFACT BINDING digests off disk, and `evidence/28-04/findings.tsv` is itself a bound artifact.
So that control could never have been byte-identical at HEAD, and reading its failure as a
refactor regression would have been wrong. The pristine-checkout A/B above is the correct
instrument. The two stray files it produced were deleted rather than committed: an unsigned
receipt-shaped JSON sitting in the phase directory is a thing a later reader can mistake for a
real receipt.
