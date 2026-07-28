PANEL-MEMBER: internal
---

# The adversarial pass, written AGAINST the position I walked in holding

I walked into this holding `accept-with-named-open`. A fourth voice that ratifies
the certification is not a panel member, and on an acceptance decision that is the
single easiest place to lose one. So this pass is written to break the
certification, not to bless it, and it presses `send-back-rounded-up` and
`accept-as-certified` by name.

## The case for `send-back-rounded-up` — three attacks, in descending strength

**Attack 1: `F26-SC4` claims Windows on evidence that only ever ran on Linux.**
This is the strongest one, and it is the same defect the certification itself
just used to demote `F26-SC2` and `F26-04`. The certification's stated standard
is that a platform list must be covered by the NAMED, REPLAYED evidence. As
originally written, `F26-SC4: CLOSED … platform=linux+windows
evidence=scripts/portability-native-matrix.sh` named the LINUX script, and the
replay driver re-ran it on `hetzner-dsm`. So the Windows half of the phase's
headline criterion rested on a Linux re-execution plus a Windows report that no
replay ever regenerated. A certification that demotes two other lines for
precisely this and then does it itself on the line that matters most is not
applying a standard, it is applying it selectively.

**This attack landed and it changed the artifact.** `F26-SC4`'s evidence is now
`scripts/portability-native-matrix.ps1`, a new `replay_windows_script` driver
re-executes it ON `SeanD@seandesktop` with the checkout SHA proven from an
isolated capture, and the replay verdict for that key now names
`host=SeanD@seandesktop`. If that driver had failed, the binding gate would have
forced this send-back regardless of what anyone here preferred. That is the
difference between a panel and a formality.

**Attack 2: `F26-SC3` is marked OPEN, but is the certification's stated unmet
clause the REAL one, or a smaller one chosen because it is easy to name?**
The clause named is "profile migration and reciprocal portability were never
interrupted". Press harder: is `backup restore`'s interruption proof itself
sound on Windows? F26-03-E says the handler probe there does not fire, so the
*uncatchability* of `TerminateProcess` is corroborated by documented Win32
semantics plus a delivered-but-unrecorded event rather than by an instrumented
probe. A hostile reading is that the Windows interruption leg is weaker than
"proven" and the certification leans on it in the OPEN line's first sentence.

**This one does NOT land, and the reason matters.** The rollback-exactness claim
does not depend on the probe: `DIGEST-EQUAL: yes` is measured from the product's
own `backup digest` before and after, and the negative control fired at exit 9,
so the mid-flight check is proven able to fire. The probe would only strengthen
the separate claim that the kill was uncatchable. 26-03 recorded exactly that
distinction as a control gap rather than a vacuous leg, and the certification
carries it. So the OPEN clause is the right clause, and it is the larger of the
two rather than the convenient one.

**Attack 3: the portable/platform report split is a loosened comparison wearing a
design rationale.** Seven of nineteen cases are excluded from the byte
comparison. Anyone can make two reports byte-identical by removing the rows that
differ, and "the filesystem, not the product" is exactly what a person doing
that would say.

**This does not land either, and it is checkable rather than arguable.** The
excluded cases are excluded by a property declared IN THE SPEC before any run —
`scope: platform` is data in `corpus-spec.json`, whose SHA-256 appears in both
reports, and the Linux leg refuses to run at all if that committed spec has
drifted from the generator. The split cannot have been chosen after seeing a
diff. And the twelve portable rows are not thin: each carries the case's
`corpus_digest`, so byte equality additionally proves that two INDEPENDENTLY
written materialisers — Python on Linux, native PowerShell on Windows, because
that box has no Python — built identical corpora. A report trimmed to force a
match would not have that property. The platform-variant cases are separately
asserted on each platform, and the measurement they produced is the most
interesting result in the phase: NTFS collapses case-only names and does NOT
collapse Unicode normal-form names, while APFS collapses both.

## The case for `accept-as-certified`, argued honestly and rejected

It is arithmetically unavailable: the option requires all nine keys CLOSED and
two are OPEN. But the interesting version of the argument is that the two OPEN
keys are over-scrupulous — that `F26-03`'s F23-envelope clause is a requirement
drafted before the phase's real shape was known, and that SC3's migration-
interruption clause is pedantry over a path with an atomic writer.

Reject both. The F23 envelope clause is not a technicality: it is the FIRST
clause of the requirement, no code in `crates/wcore-cli/src/backup/` references a
session or evidence envelope, and no plan's SUMMARY mentions one. That is
unstarted work, and Phase 28 will plan around it either way — the only question
is whether it does so knowingly. And "the writer is atomic" is an argument, not a
measurement; this program's own standing lesson is that arguments of that shape
are where the false greens live.

## What I would still not sign

Three things stay uncomfortable and belong in the record rather than in a
rebuttal. First, `F26-SC1`'s macOS evidence is at an ancestor commit and always
will be until macOS becomes a gate host; the line now says so, but "CLOSED" and
"proven at this tree" are not the same thing and a reader in six months will read
them as the same. Second, `platform=linux` on three lines UNDER-claims real
Windows corroboration, which is the opposite error and is also a distortion, just
a safe one. Third, this phase never pointed backup or restore at a real home on
any host, so every recovery claim is against synthetic input — deliberate, stated,
and still a limit on what "proven" means here.

## Verdict

Two of my three attacks failed on evidence rather than on argument, and the one
that landed was FIXED inside this task rather than argued away. Every CLOSED key
re-executed and reproduced at the certified SHA, no severity fell from critical
or high, and the two OPEN keys name the larger unmet clause rather than the
convenient one. That is `accept-with-named-open`, and it gets there by surviving
the attack rather than by nobody making it.

PANEL-VERDICT: accept-with-named-open
PANEL-BASIS: My strongest attack — that F26-SC4 claimed Windows on Linux-only evidence — was correct and was ACTED ON, so its Windows claim is now re-executed on SeanDesktop; the other two attacks failed against checkable properties rather than against argument, and the two OPEN keys name the larger unmet clause rather than the easier one.
