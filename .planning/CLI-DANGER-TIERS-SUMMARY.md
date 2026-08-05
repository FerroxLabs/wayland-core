# CLI danger tiers — LANE SUMMARY

Lane branch: `lane/cli-danger-tiers`
Merge-base (captured once, quoted everywhere): `a3e68a31e9e63767c505345eb996f5eeab2341f9`
Verdict: **LANDED.** No existing invocation changes tier, and a test now makes
that impossible to break silently.

---

## What shipped

| Tier | Canonical | Aliases | Approvals | OS sandbox |
|---|---|---|---|---|
| 1 | `--dangerously-skip-permissions` | `--force`, `--yolo` (visible) | bypass | **stays ON** |
| 2 | `--dangerously-skip-permissions-and-sandbox` | `--dangerous` (deprecated, visible) | bypass | **off, leased** |

Tier 1 is now ONE clap field with `visible_aliases = ["force", "yolo"]`, so the
three spellings are the same tier **by construction** and cannot drift apart.
Previously `--force` and `--dangerously-skip-permissions` were two separate
fields OR'd together on one line — identical by convention, not by structure.

`danger_tiers(&Cli) -> (approval_bypass, dangerous_launch)` is the single wiring
point from parsed argv to the two tiers. `run()` and the regression test both
read it, so the test observes the real wiring rather than a copy of it.

## Base semantics, measured before changing anything

The sandbox decision has exactly one chokepoint:
`BaselineExecutionPolicy::smart()` hardcodes `SandboxPolicy::Required`
(`wcore-types/src/execution_policy.rs:98`, again at `:156`). **No approval value
can turn the sandbox off.** It only becomes `Bypass` via
`EffectiveExecutionPolicy::dangerous(grant)` (`:303`), and a grant exists only
when the `dangerous` argument is true (`packaged_runtime.rs:89`).

| Spelling (at base) | feeds | approvals | sandbox | lease |
|---|---|---|---|---|
| `--force` | `approval_bypass` (`main.rs:1091`) | Bypass | Required | none |
| `--yolo` | same clap field as `--force` | Bypass | Required | none |
| `--dangerously-skip-permissions` | `approval_bypass` + stderr notice | Bypass | Required | none |
| `--dangerous` | `dangerous` arg (`main.rs:1930`) | Bypass | **Bypass** | yes |
| `--auto-approve` | `CliArgs` → `tools.auto_approve` (`config.rs:2270`) | Bypass | Required | none |

## `--auto-approve` — VERDICT: not tier 1. Left exactly as it was.

The brief's own uncertainty here was correct. Four measured differences:

1. **Decisive, parse-level.** `--dangerous --force` is REJECTED by clap today;
   `--dangerous --auto-approve` is ACCEPTED — `auto_approve` is in no
   `conflicts_with` list. Aliasing it would start rejecting an invocation that
   works today, which is precisely the class of change this lane exists to
   prevent.
2. **It is a config key**, not just a flag: the CLI face of `[tools]
   auto_approve` (`config.rs:1002`), subject to GHSA-8r7g project clamping
   (`config.rs:4140-4153`). The tier-1 flags have no config equivalent.
3. **Provenance differs.** `has_cli_overrides` (`config.rs:3473-3480`) counts
   `cli.auto_approve` but not `cli.force` / `cli.dangerously_skip_permissions`.
4. **They diverge on a reachable path.** The onboarding early-return
   (`main.rs:1828-1850`) passes `approval_bypass` into
   `resolve_local_execution` but builds `Config::default()`, so `--force` is
   honoured there and `--auto-approve` is silently dropped.

They DO converge later — `bootstrap.rs:681` normalises `tools.auto_approve` for
a `--force` run too, so children inherit the same posture either way. Runtime
convergence is not flag identity; points 1-4 stand.

`auto_approve_is_not_a_danger_tier_alias` pins this so a later lane cannot fold
it in without reddening a test.

## The regression guard (the deliverable)

`tests::danger_spellings_never_change_tier` walks every accepted spelling,
derives both tier arguments from the parsed `Cli` through `danger_tiers`, and
asserts the **effective** sandbox posture per spelling.

It reads `EffectiveExecutionPolicy::sandbox()` — the projection the protocol
emits to hosts — **not** `baseline().sandbox()`, which is hardcoded `Required`
for every baseline and therefore a permanently-green assertion (§3b-iii). The
baseline assertion is retained but explicitly labelled as such in-code.

### Three-assertion self-test

1. **Known-positive passes:** `6 passed; 0 failed; 0 ignored; 0 measured; 47 filtered out`.
2. **Known-negative genuinely fails.** Sabotage = move `force` out of tier 1's
   `visible_aliases` into tier 2's, i.e. exactly the hazard. Verbatim:

```
test tests::danger_spellings_never_change_tier ... FAILED
thread 'tests::danger_spellings_never_change_tier' panicked at crates/wcore-cli/src/main.rs:7761:13:
assertion `left == right` failed: --force CHANGED TIER: sandbox_bypassed=true, expected false. A tier-1 spelling that gains a dangerous grant strips the OS sandbox from every existing script that uses it.
  left: true
 right: false
test result: FAILED. 5 passed; 1 failed; 0 ignored; 0 measured; 47 filtered out
```

3. **The old shape would have missed it.** Under the IDENTICAL sabotage:

```
test tests::foreign_dangerous_alias_is_approval_only ... ok
```

because it hardcodes `resolve_local_execution(&cfg, true, false, ..)` instead of
deriving the tier from the parsed `Cli`.

**Near-miss worth recording:** under the same sabotage
`the_two_tiers_refuse_to_stack_in_every_spelling` ALSO stays green — once
`--force` is a tier-2 alias, `--force --dangerous` is the same bool flag twice,
which clap rejects for an unrelated reason. So the stacking test would not have
caught the escalation either. `danger_spellings_never_change_tier` is the only
gate in the suite that does.

## Live drive — posture read out of the binary's own `ready` frame

Per §3b-ii the posture is read from the product's output, never inferred from
what was exported. hetzner, debug binary, `env -i`, isolated `HOME`, fake key
`sk-ant-lane-fake-000`. **No real credential was used anywhere in this lane.**

| argv | posture | approvals | sandbox | lease expiry (unix ms) |
|---|---|---|---|---|
| `--force` | smart | bypass | **required** | — |
| `--yolo` | smart | bypass | **required** | — |
| `--dangerously-skip-permissions` | smart | bypass | **required** | — |
| `--auto-approve` | smart | bypass | **required** | — |
| `--dangerously-skip-permissions-and-sandbox` | dangerous | bypass | **bypass** | 1785374431528 |
| `--dangerous` | dangerous | bypass | **bypass** | 1785374432051 |
| *(no flag — CONTROL)* | smart | **prompt** | required | — |

**Controls in both directions on the live instrument (§3b-iii):** the no-flag
row proves it CAN report `approvals=prompt`, and the tier-2 rows prove it CAN
report `sandbox=bypass`. So `sandbox=required` for tier 1 is a measurement, not
a field that is always that value.

Lease bound measured live: `now=1785373558s expires=1785374458s delta=900s` =
exactly the 15-minute `DEFAULT_DANGEROUS_SESSION_TTL_SECS`.

`--help` renders `[aliases: --force, --yolo]` and `[aliases: --dangerous]`, with
the tier-2 doc naming `--dangerous` a DEPRECATED alias.

Notice scoping — `--force` stderr is unchanged, with a known-positive control so
the zero is not a dead grep:

```
--force                            compat_notice=0   known_positive(any_stderr)=2
--yolo                             compat_notice=0   known_positive(any_stderr)=2
--dangerously-skip-permissions     compat_notice=1   known_positive(any_stderr)=2
```

## Gates

| Gate | Result |
|---|---|
| `cargo fmt --all -- --check` (Mac, permitted) | rc=0, 0-line diff |
| `cargo check --workspace --all-targets` (hetzner, absolute-path cargo) | **WLRC=0**, 0 errors, 132-line log (non-empty control) |
| `cargo test -p wcore-cli --bin wayland-core danger` | `6 passed; 0 failed; 0 ignored; 0 measured; 47 filtered out` |
| `cargo test -p wcore-cli --test yolo_flag_smoke` | `10 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out` |
| `cargo test -p wcore-cli` bin unittests | `52 passed; 0 failed; 1 ignored; 0 measured; 0 filtered out` |

Every count above was read back from an **unproxied absolute-path** cargo
(`/root/.cargo/bin/cargo`), including the `0 ignored` / `0 filtered out` fields
that the `rtk` proxy strips (§3b).

### The one red, and why it is not mine

`plugin_discovery_e2e::ready_event_withdraws_plugin_capabilities_when_backends_cannot_start`
fails: `1 passed; 1 failed`. **Measured at BASE `a3e68a31` in a separate
worktree: identical test, identical message, identical `1 passed; 1 failed`.**
Pre-existing and environment-dependent — the test's own message says the leg
needs the `chromium`/`browserbase` backends disabled to be meaningful, and this
build enables them. Named, not fixed: out of scope.

`always_fails` in the log is the **deliberate** child of
`plugin::scaffold::tests::plugin_test_propagates_a_failing_suite`, which itself
reports `ok` — the scaffold test asserts a failing suite propagates, and the
child cargo's output interleaves into the parent log.

## Fence exposure — `crates/wcore-cli/src/main.rs`

Diffed against the merge-base SHA captured once, never against the branch name.

- `crates/wcore-cli/src/lib.rs`: **0 lines — not touched.**
- `crates/wcore-cli/src/main.rs`: **+253 / -23. NOT contiguous** — 12 hunks at
  `-U0`, in 5 regions:
  1. `331-358` — the two danger args in `struct Cli` (the rename)
  2. `1089-1128` — `danger_tiers()`, the wiring line, the scoped notice
  3. `1859` — one token: `!cli.dangerous` → `!dangerous_launch`
  4. `1960` — one token: `cli.dangerous` → `dangerous_launch`
  5. `7672-7903` — inside `#[cfg(test)] mod tests`
- **200 of 253 added lines are test code.** Production exposure is **53 added /
  23 removed** across regions 1-4, two of which are single-token renames. All 23
  removals are production; the test hunks are pure additions.

A rename cannot be additive-only, so the fence's "additive, one contiguous
block" could not be met literally. Met in spirit: no reformatting, no
reordering, no drive-by cleanup, no touched registrations.

## Instrument defect found in my own work, and repaired

`the_binary_refuses_to_stack_the_two_tiers` as first written appended `--help`
to the conflicting pair. **clap handles `--help` BEFORE validating
`conflicts_with`**, so the process exited 0 and the assertion never fired — the
gate would have stayed green with the conflict deleted outright. Caught by
running it (`9 passed, 1 failed`), then **repaired in this lane rather than
documented** (§6b-ii): it now uses `--list-agents`, asserts the refusal is
clap's own `cannot be used with` error rather than any nonzero exit, and adds a
pass-direction control running each flag solo.

## Corrections to the brief and to my own process

- The coordinator reported my worktree "otherwise clean" after a reload. It was
  **not** — `main.rs` carried 76 uncommitted lines from the pre-reload session.
  Measured before acting, per the brief's absence-claim rule.
- I then made the same error myself: I read a `git diff` render as complete and
  **missed a sixth hunk**, which is how commit `51f1ddba` silently deleted
  `foreign_dangerous_alias_is_approval_only`. Caught by grepping for the test by
  name with a known-positive control. Restored verbatim in `652f5be1`. Deleting
  a passing test is forbidden by §5, and that test turned out to be the third
  assertion of the self-test.

## Out of scope, named not fixed

- **Egress is NOT folded into tier 2.** The seam is clean: tier 2 is decided
  solely by the `dangerous` argument reaching `resolve_local_execution`, so
  adding egress later means changing what a grant implies, not the flag surface.
- `plugin_discovery_e2e` red (above) — pre-existing, proven at BASE.
- `docs/design/2026-07-13-*.md` still spell `--dangerous` in historical
  narrative prose. Left alone deliberately: they are dated design records, not
  operator docs, and rewriting history would misrepresent what was decided then.

## Not done

- No PR, no merge to integration, no tag, no issue closed, no
  `wcore-contract generate`. Pushed only to `gh lane/cli-danger-tiers`.
- No clippy run (workspace clippy was not requested and full-workspace clippy
  under lane contention is not a measurement).
- No Windows or macOS leg — the change is platform-neutral CLI parsing.
