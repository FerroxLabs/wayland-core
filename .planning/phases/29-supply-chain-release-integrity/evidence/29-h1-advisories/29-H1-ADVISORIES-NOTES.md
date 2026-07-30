# Lane `f29-h1-advisories` — running notes

Base: integration `b2ddf113681647221dc9e5bbfc7de79b1da90b54`, branch `lane/f29-h1-advisories`.
Worktree `/Users/seandonahoe/dev/waylandcore-frontier-worktrees/lane-f29-h1-advisories`.

## Minute 0-10 — the brief's central premise is STALE

My brief says `F29-02-H1` is "an OPEN and UNFIXED HIGH". At base `b2ddf113` it is not.

- `.cargo/audit.toml` at base has `ignore = []` and a header reading
  "CLOSED AT SOURCE 2026-07-29 (F29-02-H1)".
- `.planning/phases/29-supply-chain-release-integrity/29-H1-SUMMARY.md` exists and
  documents a prior lane `lane/29-h1` @ `0ea36bcc06a6fa9b10b90223f2057bec2f8ca02d`
  (based at `12fc794f`) that took the source fix: `quick-xml 0.39 -> 0.41`,
  `calamine 0.26 -> 0.36`, `plist 1.9.0 -> 1.10.0`.
- That lane INDEPENDENTLY confirmed the three-path claim in my brief and went further:
  it found FOUR `cargo audit` findings (2 advisories x 2 resolved versions), not two.

So the brief's ledger row is describing the pre-fix world. The ledger row itself
(`COMPETITIVE-LEDGER.md:159`) still asserts the HIGH is open — the ledger was not
updated when the fix landed.

### What this does NOT excuse me from

Per LANE-BRIEF §3b-i, "the vulnerable version is absent from Cargo.lock" is an
ABSENCE CLAIM, and it is the single easiest thing to pass without doing work. The
prior lane measured it at ITS base `12fc794f`, not at `b2ddf113`. Cargo.lock is the
most merge-conflict-prone file in the repo and the orchestrator already had to repair
lock drift at `b2ddf113` (`serial_test` missing). A merge could have partially
reverted the pin. So I re-verify at MY base, with known-positives.

## Work queue (what is actually left)

1. VERIFY the fix survived into `b2ddf113` — advisory count, lock contents, both
   directions, known-positive alive.
2. The prior lane pinned the fix with a **prose comment** ("Rotation policy: do NOT
   float quick-xml below 0.41"). A comment is not a gate. My brief asks for "an
   executable gate so the false-premise shape cannot return" — that is unbuilt.
3. `cargo deny` verdict RED and deliberately unchained from `check-all` — decide and
   implement an honest disposition.
4. `grep -rn 'environment:' .github/workflows/` returns zero — verify myself.

## Instrument posture for this lane

Every number I report: redirect to a file, read with the Read tool, never through
Bash stdout. `rtk` has been measured corrupting `git diff --numstat`, `grep -c`,
`cargo` ignored/filtered counts, `ls` sizes, and `git status --porcelain`.
Note macOS has no `/usr/bin/ls` or `/usr/bin/cat` — they are in `/bin`.
