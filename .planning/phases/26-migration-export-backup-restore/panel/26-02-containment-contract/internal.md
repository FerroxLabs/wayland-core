PANEL-MEMBER: internal
---

# Internal adversarial pass — argued AGAINST `contract-accept`

I walked into this measurement expecting `contract-accept`, and the three
external captures all landed there. So this pass presses the two ids that would
overturn it, as hard as the evidence allows, and then says exactly where each
one breaks.

## The case for `contract-reject`

The strongest version is not about the arithmetic — that closes at 554 — it is
about criterion 2 read strictly. `CEILING-REFUSES-REALISTIC: yes` with
`SCALE-STORE-ADMITTED: 512` against `SCALE-QUARANTINED: 541` means the
headline claim "Wayland Core can import a real peer install" is **false as
stated** for a real 540-skill Hermes home. 29 skills do not arrive. A plan
whose objective is "make import selective, provenance-preserving, and safe
against the executable content that dominates a real peer install" has, on the
one corpus that reproduces the real install's scale, failed to import 5% of it.
`contract-reject`'s own pro says exactly this: "each of these three means a
headline claim of this plan is not true as stated."

I tested the obvious defence — that a second pass picks up the remainder — and
**it is false.** Measured directly: after `promote --all`, a plain re-import
reports `discovered=554 imported=0 quarantined=554` and the store fills to 512
again with the *same* first 512, because the scan order is stable. The naive
recovery does not work, and I would have recorded that as HIGH had it been the
end of the story.

**Where it breaks.** It is not the end of the story. Every one of the 29 is
named individually in the apply report, with its reason, and re-importing with
`--select` on exactly those printed identities admits all 29:
`discovered=554 imported=0 quarantined=29 excluded=525`, balancing, followed by
one `promote --all`. End state measured: **541 skills on the load path — the
complete corpus.** Four operator invocations in total, every one of them driven
by identities the tool itself printed. That is friction to record, not a
containment failure and not a false headline: nothing was lost, nothing was
silently dropped, and nothing arrived live. `contract-reject`'s trigger
conditions are counts that do not balance, a positive control that did not
fire, or a moved ceiling. None of the three holds.

## The case for `contract-amend-ergonomics`

Four invocations and a `--select` list of 29 identities is not a path a casual
operator finds unaided. The option's pro is the real risk: "an operator who has
to promote items one at a time will route around the contract entirely." One
could argue the recovery is discoverable only by reading a 541-line report
closely, and that an operator who does not will conclude the migration silently
lost 29 skills — which is the *perception* of data loss even where there is
none, and perception is what drives people to disable safety features.

**Where it breaks.** The binding rule is not a matter of taste: this option
requires `PROMOTE-SCALING: linear`, and the final measurement says `bounded`
(`items=1 invocations=1`, `items=256 invocations=1`). Choosing it now would be
arguing to a defect the measurement contradicts — the precise failure the gate
exists to prevent. It is also worth being honest that this option was *already
taken and executed* earlier in this task: the first run measured `items=256
invocations=256`, linear, the cause was reproduced (256 items sharing 46
distinct directory names), fixed inside the plan's own files, regression-tested,
and re-measured. The ergonomics amendment is not being declined; it is done.

## What I would still record rather than wave through

1. The 512-item store ceiling refuses 29 of a realistic 541-item executable
   surface, and the naive recovery does not work. A documented `--select`
   recovery exists and completes the import. **MEDIUM**, matching the threat
   register's own T-26-02-09 rating, and non-blocking per the phase rules.
2. The 540-payload scale point was **materialised** by the measurement script
   into 26-01's structural corpus, which ships those directories as markers
   with no `SKILL.md`. The structure is the real install's and the payload is
   the committed fixture, but the number is not one the corpus shipped, and the
   script prints `SCALE-CORPUS-MATERIALISED-SKILLS: 540` so it can never be
   mistaken for one that was. **LOW**, recorded.
3. `CLASSIFY-EXEC-UNCONTAINED: 0` is only as strong as what it looks at — MCP
   definitions carrying a command in `config.toml`, and directive-carrying
   skill bodies on the load path. An executable surface neither of those covers
   would read as zero. I could not name one for the four kinds this plan
   classifies, but the metric is a floor, not a proof of exhaustiveness.

None of the three overturns the six criteria as measured.

PANEL-VERDICT: contract-accept
PANEL-BASIS: Both overturning options fail against the final measurement — reject's three triggers are all absent and the import is completable, and the ergonomics amendment was already made and re-measured bounded — leaving acceptance with two recorded findings.
