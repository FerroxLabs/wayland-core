# Phase 26 acceptance decision

CHOSEN: accept-with-named-open
BASIS: majority
RATIONALE: Three of four members reached accept-with-named-open and every one of the seven CLOSED keys was RE-EXECUTED at the certified SHA 9b2ed829 and reproduced, including the macOS leg re-derived from GitHub and the Windows leg re-run on SeanDesktop; the two OPEN keys name the larger unmet clause rather than the convenient one, and no severity fell from critical or high anywhere in the phase.

## The tally, from the captured verdicts

| Member | Verdict |
|---|---|
| codex | `send-back-rounded-up` |
| gemini | `accept-with-named-open` |
| kimi | `accept-with-named-open` |
| internal | `accept-with-named-open` |

Three to one. The chosen option is the plurality verdict, so the basis is
`majority` rather than `minority-with-evidence`.

## Why the replay, not the argument, is what this rests on

`accept-as-certified` was arithmetically unavailable from the start: it requires
all nine keys CLOSED and two are OPEN. Both send-back options were bound to
facts rather than to preference — `send-back-rounded-up` requires at least one
CLOSED claim that failed to reproduce, and `send-back-finding-reclassified`
requires at least one severity that fell from critical or high. Neither trigger
fired:

```
REPLAY-SUMMARY: closed_keys=7 failed=0 not_replayable=0
```

Seven closed keys, seven `result=reproduced`, on three different hosts —
`github` for the macOS provenance, `hetzner-dsm` for the Linux evidence, and
`SeanD@seandesktop` for the Windows evidence. And the findings reconciliation,
built from each earlier plan's own SUMMARY rather than from the list
reclassification would have produced, shows the phase's single critical-or-high
finding (F26-03-D) still recorded HIGH and closed by a fix at the product with
the fixture left deeper than when it was red.

SEND-BACK-ACTED: codex's two named objections were both correct and were both
acted on inside this task before the decision was recorded, rather than argued
with. (1) F26-SC1 and F26-01 now state IN THE STATUS LINE that the mandatory
macOS leg ran at ancestor commit `b671f9ad` and not at this certified tree, so a
reader checking only the machine-parseable form cannot infer otherwise. (2)
F26-04's platform claim fell from `linux+windows` to `linux`, because its named
evidence `scripts/portability-remap-capture.sh` covers the remap half on Linux
and nothing on Windows; the Windows rollback half is real, is 26-03's, and is now
described rather than counted. A third objection, raised by the internal
adversarial pass against the position it walked in holding, was also acted on:
F26-SC4's evidence moved from the Linux matrix script to
`scripts/portability-native-matrix.ps1`, and a new `replay_windows_script`
driver RE-EXECUTES it on real Windows with the checkout SHA proven from an
isolated capture before and after. Without that change, the phase's headline
Windows claim would have rested on a Linux re-execution.

## What acting on the send-back cost, stated plainly

Three of the nine lines (`F26-SC2`, `F26-02`, `F26-04`) now read `platform=linux`
even though a real Windows run corroborates each of them. That UNDER-claims. It
is the deliberate price of a single consistent standard — the platform list must
be covered by the named, replayed evidence — and under-claiming is the error this
program would rather make.

## DISSENT

**codex — `send-back-rounded-up`.** Recorded in its own terms, not summarised
away:

> F26-SC1/F26-01's macOS replay validates `b671f9ad`, not certified SHA
> `9b2ed829`, while the Windows-bearing CLOSED keys replay only on Hetzner. In
> particular, F26-04's named remap evidence does not prove its claimed Windows
> interruption rollback.

**Both halves of that objection were correct.** The second half was fixed by
demoting F26-04 to `platform=linux`, and the generalised form of it — a
Windows-bearing key replaying only on Linux — was fixed by moving F26-SC4 to the
PowerShell script and replaying it on SeanDesktop. The first half is real and
is NOT fully fixable inside this phase: the macOS leg cannot be re-executed
because macOS is not a gate host here, so the strongest obtainable evidence is
the GitHub re-derivation of that run's head SHA, its `Build
(aarch64-apple-darwin)` conclusion and its live non-empty artifact. The
certification now says so on the line rather than only in prose.

`send-back-rounded-up` was nonetheless not selectable as the outcome, and not
because three members outvoted one: the plan binds that option to at least one
CLOSED claim failing to reproduce, and after the corrections every closed claim
reproduces. A send-back is work this plan does rather than a message it emits —
the work was done, and the resulting state is `accept-with-named-open`.

**A residual codex raised that nobody has closed.** Its framing implies that a
criterion whose evidence lives at an ancestor commit should not read `CLOSED` at
all. That is a defensible standard and this certification does not adopt it; it
discloses instead. A reader in six months may reasonably disagree, and this
paragraph exists so that they can find the disagreement rather than reconstruct
it.

**kimi's nit, disposed of rather than dropped:** the certification referenced
`26-04-SUMMARY.md`, which did not exist when the bundle was built. It exists now,
alongside this record, and carries the full per-case results.
