# f24-c3-h5-reload — NOTES (append-only, committed as I go)

Lane branch: `lane/f24-c3-h5-reload`
Base: `gh/plan/f20-unified-audit-repair` @ `d622cb09de01329cef6f20d6f9183df171462daf`
(SHA asserted against `/usr/bin/git ls-remote gh plan/f20-unified-audit-repair` — match).

## Minute 0-15 — the brief's central premise is FALSE at HEAD

My brief states `F24-C3-H5` is **"open and unfixed at HEAD"** and that my job is "the repair,
not the discovery". **Measured: the repair already landed and is an ancestor of HEAD.**

```
$ /usr/bin/git merge-base --is-ancestor 5d4bf4b9 HEAD  -> ANCESTOR-OF-HEAD
$ /usr/bin/git merge-base --is-ancestor 44a7cc16 HEAD  -> ANCESTOR-OF-HEAD
$ /usr/bin/git merge-base --is-ancestor 7c512fe2 HEAD  -> ANCESTOR-OF-HEAD
control (must be false): HEAD ancestor-of 5d4bf4b9 -> NOT ancestor  [instrument alive]
```

- `5d4bf4b9 fix(24-h5): a reloaded channel carries its access policy AND its tool posture`
- `44a7cc16 fix(24-h5): restore InboundPolicy import for the channel_inbound test module`
- `7c512fe2 docs(24-h5): SUMMARY and live evidence — reload now admits, with the configured posture`

`.planning/.../24-H5-SUMMARY.md` exists at HEAD with `status: complete`, verdict
"FIXED and live-proven, both facets", and a pre-fix/post-fix one-variable table
(pre-fix `8/11` legs FAIL, fixed `11/11` PASS) taken against binaries built from
`d34b2fe1` (pre) and the lane head (post).

**Why the ledger says otherwise:** the `2026-07-30` re-grade block in
`CRITERIA-GAP-LEDGER.md:818-832` was written by `lane/ledger-regrade` (`71acfd19`) and
reads the *finding* lane's `24-C3-FINISH.md`, which was accurate when written. The
`24-h5` repair lane merged after. The ledger row is stale, not wrong-at-the-time —
exactly the decay LANE-BRIEF §"Your brief's MEASUREMENTS are probably stale" predicts.

**So this lane is NOT the repair lane.** Remaining honest work, in priority order:

1. Re-verify the fix independently at HEAD (do not take the prior summary's word).
2. Answer the adjacent question my brief asks and the prior lane may not have:
   **is the access policy the only state `reload` fails to reload?** Sweep for siblings.
3. Answer: **can `reload`'s health report fail when the path is dead?**
4. Whatever siblings turn up: fix or name.
