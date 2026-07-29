# 31-vacuous-greens — NOTES (append-only, committed continuously)

Lane `vacuous-greens`. Finding `BL-F28-VACUOUS-GREENS`.
Base: `plan/f20-unified-audit-repair` @ `8420ee94`.
All measurements below via unproxied `/usr/bin/grep`, `/usr/bin/git`, `find` (LANE-BRIEF §3b).

## Instrument-liveness check (LANE-BRIEF §3b-i)

Before reporting any absence, the grep instrument was proven alive on a known-positive in
the same session: `/usr/bin/grep -rn 'nextest' justfile` → 19 hits (non-zero). Good.

---

## M1 — The claimed number (~44 test binaries) is WRONG, in the safe direction

Verified counts, `find`/`grep`, unproxied:

| Quantity | Count | How measured |
|---|---|---|
| Integration test binaries (`crates/*/tests/*.rs`, depth 3) | **481** | `find crates -mindepth 3 -maxdepth 3 -path '*/tests/*.rs' \| wc -l` |
| Nested helper modules (`tests/*/**.rs`, NOT binaries) | 33 | `find crates -mindepth 4 -path '*/tests/*.rs'` |
| Root `tests/*.rs` | 7 | `ls tests/` |
| Workspace members | 56 | `grep -c '^\s*"crates/' Cargo.toml` |
| Explicit `[[test]]` declarations | 4 (2 crates) | `grep -c '\[\[test\]\]' crates/*/Cargo.toml` |

So the reachable-by-`cargo test` integration-binary surface is **~488**, not ~44 — an order
of magnitude larger — before counting per-crate `--lib`/`--bins` unittest binaries (up to 56
more). **The finding understated its own blast radius by ~10x.** Correcting this UP, not down.

## M2 — `no-tests = "fail"` is ALREADY set, and inherited

`.config/nextest.toml:37` — `no-tests = "fail"` under `[profile.default]`, explicitly
documented as inherited by `ci` / `e2e` / `eval` which do not override it. So the finding's
premise is right: **nextest is already closed; the hole is exclusively the bare-`cargo test`
paths.** This is why the earlier lane found the guard "redundant rather than missing" — it was
redundant *for nextest*, and that conclusion was over-generalised to the whole repo.

## M3 — cfg-gated binary count in nextest.toml is STALE

`.config/nextest.toml:22-24` claims "22 file-level `#![cfg(...)]` test binaries in this
workspace (15 feature-gated, 22 counting platform gates)".

Measured today: **39** file-level `#![cfg(...)]` test binaries
(scan: `head -40 $f | grep '^#!\[cfg('` over all 481). Breakdown:
- feature-gated: 18
- platform-gated (`unix` / `windows` / `target_os`): 21

These are exactly the binaries that compile to **empty** on the wrong platform/feature and
print `running 0 tests ... ok`. This is the false-positive population I must handle
deliberately (brief item 4) — a `#![cfg(windows)]` binary on Linux is *legitimately* empty.

## M4 — Bare `cargo test` invocation sites (executable, not prose)

Sweep: `/usr/bin/git grep -n 'cargo test' -- . ':!.planning'` → 112 files; the vast majority
are doc-comments/prose. The **executable** sites:

**justfile**
- `justfile:177` `vx cargo test -p wcore-cli --test harness_cli_surface --test harness_tui_flow`
- `justfile:178` `vx cargo test -p wcore-cli --features harness-failure-injection --test harness_failure_injection`
- `justfile:204` `vx cargo test --locked -p wcore-eval-scenarios --features packaged-driver-gate --test packaged_driver_gate`
- `justfile:209` same, `[windows]` PowerShell recipe

**.github/workflows**
- `ci.yml:266` `vx cargo test --locked -p wcore-eval-scenarios --features packaged-driver-gate --test packaged_driver_gate`
- `ci.yml:561` same, inside `$DOCKER_RUN`
- `supply-chain.yml:131` `cargo test --locked -p wcore-eval-scenarios --test sbom_contract`
- `macos-native-suites.yml:105` `vx cargo test -p wcore-sandbox --test hard_process_containment_macos`
- `macos-native-suites.yml:114` `vx cargo test -p wcore-sandbox --test live_integrity_macos`

**scripts/** (worst offenders — these also PIPE, stealing exit status per §3.2)
- `f24-c3-tests.sh:24,33,39` — `cargo test ... 2>&1 | tee -a "$OUT" | grep -E ...`
- `f24-c3-mutations.sh:24`
- `f23-clock-probe.sh:97`, `f23-multi-day-journey.sh:111`, `.ps1:85` — `--no-run` (build-only, NOT test-executing → out of scope)

### The sharpest observation so far

**Every one of the CI/justfile bare-`cargo test` sites is `--test <name>` scoped AND
feature-or-platform gated.** `packaged_driver_gate.rs` is `#![cfg(feature = "packaged-driver-gate")]`;
`hard_process_containment_macos.rs` / `live_integrity_macos.rs` are `#![cfg(target_os = "macos")]`.
So if the feature flag is ever dropped, renamed, or fails to activate, the binary compiles
**empty** and the gate prints `ok` having certified nothing — on the **supply-chain** and
**packaged-driver** gates, which are exactly the ones you cannot afford to be vacuous.

That is the real finding, and it is worse than "44 binaries could be vacuous": it is
"the release-integrity gates are the ones structurally most exposed to it."

---

## Still to establish
- [ ] Count of all-`#[ignore]`d binaries (LANE-BRIEF claims 15)
- [ ] Reproduce vacuous pass BEFORE the fix (mutation, both directions)
- [ ] Pick + implement the closure mechanism
- [ ] Reproduce failure AFTER the fix
- [ ] Third assertion: old shape would have missed it
- [ ] False-positive handling for legitimately-empty platform-gated binaries
