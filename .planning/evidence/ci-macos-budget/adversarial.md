# Internal adversarial pass — arguing AGAINST the 3-0 consensus

**A1. The token creates a silent-absence gate — the exact self-passing class this program hunts.**
A lane push with no token goes green having never touched macOS, and a green lane CI looks
identical to a fully-covered one. LANE-BRIEF §3.2 lists "a suite that exits 0 having run zero
tests" as a known self-passing class; this manufactures a workflow-level instance of it.
*Rebuttal:* the failure mode is not identical. A skipped-but-green **check** is invisible; an
**absent job** is visible in the checks list — `CI (macos-latest)` simply is not there. The
mitigation is to make absence loud rather than merely visible: the `budget` job always runs and
always writes `DARWIN=…` plus the reason to `$GITHUB_STEP_SUMMARY`. All three panellists
converged on this independently. **Survives, mitigated — this is the real residual risk and
must be stated as such.**

**A2. Does this actually fix the integration branch, or just make it less bad?**
Strongest objection. Even at 0.83 macOS jobs/hr the integration branch still serialises through
its per-ref concurrency group, and a full run takes ~40-50 min (critical path is
`CI (linux-containerized)` 40 min and `CI (Array)` 32 min — not macOS). Integration pushes land
~93/24h ≈ one per 15 min. So the pending slot still churns and roughly 2 of every 3 SHAs are
still evicted unverified. **This objection is CORRECT and is not fully answered by the fix.**
What changes is the cadence: from **0 verdicts in 9 h** to **~1 verdict per 45 min on a
≤45-min-old SHA**. That must be reported as the actual claim, not "every push gets a verdict".

**A3. Lanes will just always add the token, and we are back to square one.**
Plausible — a lane agent that reads "add `[ci-darwin]` to get macOS" may add it reflexively.
*Rebuttal:* it needs a deliberate act per push, and the headroom is large (capacity 11.25/hr vs
integration's 0.83/hr leaves ~10/hr ≈ 3 opt-in lane runs per hour before saturation). But there
is no enforcement, so this is a genuine residual risk, not a closed one. Report it.

**A4. Detection of macOS-only breakage moves later.**
True. arm64-specific behaviour, case-insensitive APFS paths, BSD-vs-GNU userland, keychain and
fsevents deps now fail at integration rather than at the lane. *Rebuttal:* they still fail
before `main`, lanes merge serially so attribution is preserved, and today the lane-level macOS
check does not complete at all — a check that never runs detects nothing. Converting a
theoretical per-lane check into an actual per-integration one is strictly better.

**A5. kimi's simpler alternative (job-level `if:`) would avoid the matrix plumbing.**
*Checked and REJECTED on a factual ground:* `jobs.<id>.if` cannot access the `matrix` context
(only `github`, `needs`, `vars`, `inputs`), so a job-level `if` can only skip the *entire* `ci`
or `build` job — which would also drop the self-hosted Windows leg and all four non-Darwin
build targets. Per-cell selection requires a dynamic matrix. kimi is wrong on this detail.

**A6. An empty matrix vector is a hard workflow error.**
`strategy.matrix.os: []` fails with "Matrix vector 'os' does not contain any values". *Checked:*
the narrowed `ci` matrix always retains the self-hosted Windows entry (1 cell) and the narrowed
`build` matrix always retains 4 non-Darwin targets. Neither can be empty under any branch of the
decision logic. Not a live hazard, but asserted in the gate rather than assumed.

**A7. Adding `needs: budget` means a `budget` failure silently drops all CI.**
*Checked:* when a `needs` dependency fails, dependents are `skipped` **and the run conclusion is
`failure`** — the run goes red, loudly. Fails closed, not open.

**A8. Should the arm64 `Build` job be dropped to save a third of macOS cost? (panel Q4)**
Panel split 2-1 against (kimi NO, codex NO, gemini YES-with-artifact-transfer). **Taking the
majority.** gemini's variant — move `upload-artifact` into `CI (macos-latest)` — is worse for the
stated use case: the `CI` job gates on fmt, clippy and the full test suite *before* it would
reach an upload step, so a lane whose tests are red would lose the binary. That is precisely
when a lane most needs a binary to live-test. And the saving is one third of 3.7% ≈ **1.2% of
pool load** — noise against a 2x oversubscription. Dropping it would trade a measured 1.2% for
re-breaking the artifact capability added deliberately at ci.yml:600-605. That is this program's
most-repeated failure mode. **NO.**

## Net
Consensus survives on the mechanism. Two claims must be weakened in the write-up:
(i) A2 — the fix restores a ~45-minute verdict cadence, NOT coverage of every integration push;
(ii) A1/A3 — silent absence and reflexive opt-in are mitigated but not eliminated.
