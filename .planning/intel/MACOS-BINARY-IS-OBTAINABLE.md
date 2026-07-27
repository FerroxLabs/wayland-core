# A macOS binary IS obtainable, without Cargo on the Mac

**Resolved 2026-07-27 by measurement.** This closes an escalation two separate lanes raised as
a blocker needing Sean's ruling. It is not a blocker and never needed a ruling.

## The apparent conflict

Phase plans require macOS evidence from a current binary. The standing rule forbids running
Cargo on the Mac. Lanes concluded these were irreconcilable — 23B reported "no macOS leg
(instruction conflict unchanged: plans require Cargo on the Mac, the brief forbids it)", and
lane 24 reported "no macOS evidence, and it is not obtainable in this lane… the only macOS
binary present is `/opt/homebrew/bin/wayland-core` **v0.12.12**, predating this work."

Both were reasoning from the assumption that the only way to get a macOS binary is to build one.

## The resolution

**CI already builds and uploads a macOS binary for every target** (`.github/workflows/ci.yml`
`Upload release binary`, added in `d9c7683b` — before that, the `build` job compiled release
binaries for all six targets and *discarded* them). Download it. No Cargo, no Mac build, no
rule bent.

```bash
gh run list --branch <branch> --limit 15 --json databaseId --jq '.[].databaseId'
# find a run with artifacts:
gh api repos/FerroxLabs/wayland-core/actions/runs/<id>/artifacts --jq '.artifacts[].name'
gh run download <id> -R FerroxLabs/wayland-core -n wayland-core-aarch64-apple-darwin
chmod +x wayland-core
```

**Verified end to end on the Mac, 2026-07-27**, from run `30228836800`:

```
file        -> Mach-O 64-bit executable arm64
--version   -> wayland-core 0.12.25
--build-info-> wayland-core 0.12.25 (source b75e640c589ef4dcaa8f557260354e7aa90851aa)
```

It runs, and `--build-info` binds it to an exact source SHA — so it is usable as *evidence*, not
just as a binary. All six targets were present and unexpired: both Apple targets, both Windows
targets, both Linux targets.

## Three traps that make this look unavailable when it isn't

1. **A run can be `conclusion: failure` and still have good artifacts.** Run `30228836800`
   failed overall, but its `build` job succeeded and uploaded all six. Filtering to
   `--status success` returns nothing and makes it look like there is no binary. Query
   artifacts directly instead of filtering runs by conclusion.
2. **Frequent pushes cancel QUEUED runs.** `cancel-in-progress: false` protects a *started* run,
   not a queued one. During heavy pushing most runs on the branch showed `cancelled`. If you
   need a fresh artifact, stop pushing and let one finish.
3. **Artifacts expire after 14 days** (`retention-days: 14`). Re-check `expired` before relying
   on an older run.

## What this unblocks

Every macOS row previously written off as unobtainable — in 23B, in 24, and in the Phase 28
certification matrix. Phase 28's plan set already forbids the phrase "no macOS binary is
obtainable" by name; this is the mechanism that makes that prohibition satisfiable rather than
merely aspirational.

**It does not** give you a macOS *build* — you still cannot compile on the Mac, so anything
requiring a locally-patched binary still needs CI to build it first. Push, wait for the run,
download.
