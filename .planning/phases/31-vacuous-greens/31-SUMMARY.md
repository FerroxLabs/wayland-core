# 31-vacuous-greens — SUMMARY

Lane `vacuous-greens`. Finding `BL-F28-VACUOUS-GREENS`.
Branch `lane/vacuous-greens`, base `plan/f20-unified-audit-repair` @ `8420ee94`.
All builds/tests on `hetzner-dsm` (`/root/wayland-vacuous-greens`), cargo 1.96.0,
cargo-nextest 0.9.137. Nothing was compiled on the Mac.

**Verdict: goal ACHIEVED, and the lane found a HIGH defect larger than the one it
was sent for — the repo's existing anti-vacuity control was itself a no-op.**

---

## 1. The verified count

The "~44" in the finding is **correct**, but for a different quantity than the
finding states. Measured (unproxied `find` / `/usr/bin/grep`):

| Quantity | Count |
|---|---|
| Integration-test binaries reachable by bare `cargo test` | **488** (481 in `crates/*/tests/`, 7 in `tests/`) |
| Workspace members (each also yielding `--lib`/`--bins` unittest binaries) | 56 |
| **Binaries with a file-level `#![cfg(...)]` — the vacuity-prone population** | **44** (17 feature-gated, 27 platform-gated) |
| Binaries with every case `#[ignore]`d | 23 |

So: ~488 are *reachable*; **44** can compile to EMPTY and print `test result: ok`.
The finding's number is right about the exposed population, not the reachable one.

Two corrections to counts already in the repo, both stale in the unsafe direction:
- `.config/nextest.toml` claimed **22** file-level `#![cfg(...)]` binaries. True: **44**.
- LANE-BRIEF §3.2 says **15** all-`#[ignore]`d binaries. Measured: **23**.

My own first scan said 39 — it used a `head -40` window and missed
`harness_failure_injection.rs:42`. Caught and re-measured full-file; the 44 is the
full-file number.

## 2. The invocation paths (file and line, at base `8420ee94`)

**Real holes — bare `cargo test`, no executed-count assertion (all now closed):**

| Path | What it gated |
|---|---|
| `.github/workflows/ci.yml:266` | F01 packaged wayland-eval driver gate (native matrix) |
| `.github/workflows/ci.yml:561` | same, containerized job |
| `.github/workflows/supply-chain.yml:131` | SBOM byte-determinism contract |
| `justfile:177`, `justfile:178` | `just harness` layers 1-3 |
| `justfile:204`, `justfile:209` | `f01-packaged-driver-gate` (unix + windows) |

**NOT holes — already guarded. Left as `cargo test` deliberately, now marked
`vacuity-checked:` so the guard records the decision rather than suppressing it:**

- `.github/workflows/macos-native-suites.yml:105,114` — its own "Assert both
  suites executed" step pins `running 1 test` and `1 passed; 0 failed` and
  catches an early-return `^skip:`. **Stronger** than a non-zero-test check;
  converting it would have been a downgrade.
- `scripts/f24-c3-tests.sh:24,33,39` — `PIPESTATUS[0]` plus explicit count read-back.
- `scripts/f24-c3-mutations.sh:24` — grades "ran nothing" as a distinct state 3.
- `scripts/f23-clock-probe.sh:97`, `f23-multi-day-journey.{sh:111,ps1:85}` —
  `--no-run`, build-only, cannot execute.

The sharp part: **every real hole was a release-integrity gate, and each was
`--test <name>` scoped at a binary that is feature- or platform-gated.** Drop the
feature and the gate certifies nothing while printing `ok`.

## 3. HIGH — the existing guard was itself vacuous

`.config/nextest.toml` carried `no-tests = "fail"` under `[profile.default]`.
**nextest ignores it.** Printed on every `run` and `list` this repo has executed:

```
warning: in config file .config/nextest.toml, ignoring unknown
         configuration key: profile.default.no-tests
```

`no-tests` is a CLI option only; the key is absent from nextest's config schema.
So the fail-closed behaviour came entirely from the **CLI default of whatever
nextest happened to be installed** — precisely the dependency the key's own
comment said it existed to remove. `vx.toml` has no nextest pin; CI installs are
`tool: nextest` (unpinned) or `^0.9`.

**Fixed, not merely written up** (§6b-ii). Two mechanisms, both measured to bite:
- `nextest-version = { required = "0.9.137" }` — verified honoured
  (`required = "0.9.200"` against installed 0.9.137 → `error: this repository
  requires nextest version 0.9.200`). Floor is the oldest version whose zero-test
  behaviour was actually measured here, not the oldest that might work.
- Explicit `--no-tests=fail` at every release-integrity call site, so those gates
  depend on no default at all.

## 4. The mutation proof — both directions, on real gates

Not a synthetic fixture: the actual `packaged_driver_gate` binary, empty because
its feature is off.

| # | Invocation (same empty binary) | Result |
|---|---|---|
| BEFORE | `cargo test -p wcore-eval-scenarios --test packaged_driver_gate` | `test result: ok. 0 passed; 0 failed; 0 ignored; 0 filtered out` — **rc=0** |
| BEFORE | `cargo test --test sbom_contract -- zzz_no_such_test_name` | `0 passed; 12 filtered out` — **rc=0** |
| AFTER | `cargo nextest run --no-tests=fail --test packaged_driver_gate` | `error: no tests to run` — **rc=4** |
| AFTER | `cargo nextest run -E 'test(zzz_no_such_test_name)'` | `error: no tests to run` — **rc=4** |
| KNOWN-POSITIVE | `cargo nextest run --no-tests=fail --test sbom_contract` | **12 tests run, 12 passed** — rc=0 |
| KNOWN-POSITIVE | `cargo nextest run --no-tests=fail --test harness_cli_surface --test harness_tui_flow` | **28 tests run, 28 passed** — rc=0 |
| FLAG-IS-HONOURED | `cargo nextest run --no-tests=pass --test packaged_driver_gate` | `0 tests run` — **rc=0** |

The last row is load-bearing: it proves the CLI flag is honoured, which by
contrast proves the ignored config key was doing nothing.

Raw logs: `.planning/phases/31-vacuous-greens/evidence/{before,after}-*.log`.

## 5. The durable guard

`scripts/check-no-vacuous-cargo-test.py` — fails if a new executable `cargo test`
appears in the justfile, a workflow or a script without `--no-run` or a
`vacuity-checked:` annotation. Wired into `ci.yml` (Linux only, ~50ms) and
`just check-no-vacuous-cargo-test`.

**End-to-end mutation on real content, not a fixture:**
- against the **base tree** `8420ee94` → **13 violations, rc=1**
- against **HEAD** → **0 violations, rc=0**

**Self-test, 6 assertions** (`--self-test`), all green on hetzner:
1. known-positive: a real bare `cargo test` is detected;
2. known-negative: nextest / `--no-run` / marker / prose all pass;
3. **A3a — the old shape would have missed it**: fires on the literal
   `ci.yml:266` line as it stood at base (there was no guard at all, so it shipped);
4. **A3b** — a naive substring matcher false-fires 5x on the clean fixture,
   proving the filtering does real work rather than passing trivially;
5-6. **A4 regression** — YAML step names and backticked prose do not false-fire.

## 6. Two false positives found in my own instruments, both repaired in-lane

Per §6b-ii, an instrument defect written up but not fixed is a defect kept.

1. **The guard false-fired on its own CI wiring step** — the YAML label
   `- name: No vacuous \`cargo test\` invocations`. Fixed by stripping
   backtick-quoted spans before matching plus a YAML-label rule; assertion A4
   locks it.
2. **`justfile`/`Justfile` double-counted** every violation on a case-insensitive
   filesystem (one file, two path strings). Fixed by deduping on `(st_dev, st_ino)`.

And the inverse discipline (brief item 4): a `#![cfg(windows)]` binary is
*legitimately* empty on Linux. `just harness` is therefore **split
`[unix]`/`[windows]`** — the pattern this justfile already uses for
`f01-packaged-driver-gate` — so the emptiness is **declared, not suppressed**.
Nobody has to add a suppression. `just --list` parses both recipes.

**A measurement artifact I nearly published:** my first post-fix check reported
the config warning still present. It came from `/tmp/final-types.log` — **another
lane's file**, caught by my `/tmp/final-*.log` glob on shared `/tmp`. Re-scoped to
my four logs (all clean) and re-proved the grep alive on the foreign file
(returns 1). `/tmp` on `hetzner-dsm` is shared across lanes; glob evidence there.

## 7. What I did NOT do

- **No full-workspace test run.** Other lanes were building concurrently, so per
  §6 that would not be a measurement. Targeted runs at the new config all pass.
- **Did not convert `macos-native-suites.yml`** — its existing gate is stronger,
  and I cannot execute macOS CI to verify a change.
- **Did not touch runner configuration or `sean-mac-arm64`.** No merge, PR, tag,
  release, issue close, or `wcore-contract generate`.
- **No `cargo fmt`** — zero `.rs` files changed.

## 8. Open / for the orchestrator

- **Behavioural change to disclose:** `nextest-version = { required = "0.9.137" }`
  makes **every** nextest invocation hard-error for anyone on an older nextest.
  That is the intended fail-closed posture, but it is a real upgrade requirement.
  CI is unaffected (installs latest / `^0.9`).
- No protocol seams, no contract requests.
- **Shared-file fence: not touched.** `git diff $BASE -- crates/wcore-cli/src/lib.rs
  crates/wcore-cli/src/main.rs` is empty.
