# Decision — the F26-02 containment contract at real 540-item scale

CHOSEN: contract-accept
BASIS: majority
RATIONALE: All four members answered one bundle carrying the same measurement and all four landed on contract-accept. The choice is bound to what the run measured rather than to the tally: the four counts balance at 554 (13 imported + 541 quarantined + 0 excluded), the inertness positive control fired when Task 3's paired legs were RE-RUN at this commit, promotion cost measured bounded at two distinct subset sizes (1 item -> 1 invocation, 256 items -> 1 invocation), no data kind sits in quarantine and no executable kind sits outside it (0 and 0), and all three workspace-trust ceiling constants stand exactly where they were at 512 / 4 MiB / 32 MiB. Those six are precisely the conditions under which the plan's binding rule makes contract-accept available, and every one of them was measured rather than argued.

## What the measurement decided, not the argument

`contract-amend-ergonomics` was not declined — it was **taken and executed**
earlier in this same task. The first 540-scale run measured
`PROMOTE-COST: items=256 invocations=256` and `PROMOTE-SCALING: linear`. The
cause was reproduced directly rather than guessed: a real peer install reuses
one skill name across profiles, so 256 quarantined items shared only 46
distinct directory names, and `promote` aborted the whole set on the first
`PromotionTargetExists`. That is the "operator routes around containment"
failure mode the option exists to catch. It was fixed inside this plan's own
declared files (collision resolved by digest-disambiguated naming, mapping
returned and reported, nothing overwritten and nothing dropped), guarded by a
regression test, and re-measured at `invocations=1`. `promotion-scale.txt` is
the re-measurement.

So the ergonomics amendment is in the record as work done, and
`contract-accept` describes the state after it.

## Findings recorded, not waved through

FINDING: medium — the 512-item store ceiling refuses 29 of a realistic 541-item
executable surface, and the NAIVE recovery does not work. Measured: after
`promote --all`, a plain re-import reports `discovered=554 imported=0
quarantined=554` and refills the store with the same first 512, because the
scan order is stable. A documented recovery DOES exist and completes the
import: every refusal is named individually in the apply report, and re-running
with `--select` on exactly those printed identities yields
`discovered=554 imported=0 quarantined=29 excluded=525`, balancing, after which
one `promote --all` leaves **541 skills on the load path — the complete
corpus**, in four operator invocations total. Raising `MAX_EXECUTABLE_FILES` to
avoid this is expressly forbidden and was not done. Non-blocking per the phase
rules; carried to BACKLOG.

FINDING: low — the 540-payload scale point was MATERIALISED by the measurement
script into 26-01's structural corpus, which ships those 540 skill directories
as markers with no `SKILL.md`. The structure is the real install's and the body
is the committed fixture, but the number is not one the corpus shipped. The
script prints `SCALE-CORPUS-MATERIALISED-SKILLS: 540` so it cannot be mistaken
for one that was.

EVIDENCE: promotion-scale.txt

## DISSENT

**Unanimous — all four captured verdicts are `contract-accept`** (codex,
gemini, kimi, internal). There is no differing verdict to record.

Recording what the unanimity does NOT mean, since a 4/4 that merely ratifies is
worth less than a split:

- The internal pass was written AGAINST the emerging consensus and pressed both
  overturning ids by name. Its `contract-reject` case produced a genuinely new
  measurement — that the naive second-pass recovery does **not** work — which
  had it stood alone would have been a HIGH finding. It was only demoted to
  MEDIUM after the `--select` recovery was measured end to end and shown to
  complete the corpus.
- kimi's capture independently identified criterion 2 as "the criterion that
  could have sunk this", and noted the record shows it nearly did.
- gemini and codex both explicitly refused the tempting repair (widening
  `MAX_EXECUTABLE_FILES`), which is the answer the constraint required; neither
  treated the 29 refusals as a containment failure.
- The residual each member left standing is the same one: `CLASSIFY-EXEC-UNCONTAINED: 0`
  is a floor over the surfaces it inspects (MCP definitions carrying a command
  in `config.toml`; directive-carrying skill bodies on the load path), not a
  proof of exhaustiveness over surfaces nobody has named.
