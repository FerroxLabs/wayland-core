# CLI danger tiers — lane NOTES

Lane: `lane/cli-danger-tiers`
Base (merge-base, captured once): `a3e68a31e9e63767c505345eb996f5eeab2341f9`
Worktree: `/Users/seandonahoe/dev/waylandcore-frontier-worktrees/lane-cli-danger-tiers`

Goal: rename the two danger flags so the superset relationship is visible in the
spelling, WITHOUT changing any existing invocation's effective privileges.

---

## 1. Measured semantics at base — the five spellings, BEFORE any change

All file:line refs are at `a3e68a31`. Every claim below was read out of source,
not inferred from the brief (LANE-BRIEF "your MEASUREMENTS are probably stale").

### The two things that actually get decided

`crates/wcore-cli/src/packaged_runtime.rs:67 resolve_local_execution(config, approval_bypass, dangerous, ttl, desktop_launch)`

- **approvals**: `approval_bypass` → `ApprovalPolicy::Bypass`, else `config.smart_approval_policy()`.
- **sandbox**: `BaselineExecutionPolicy::smart()` hardcodes `sandbox: SandboxPolicy::Required`
  (`crates/wcore-types/src/execution_policy.rs:98`, and again at `:156` in
  `with_requested_approvals`). **No approval value can ever turn the sandbox off.**
  The sandbox goes to `Bypass` ONLY via `EffectiveExecutionPolicy::dangerous(grant)`
  (`execution_policy.rs:303`), and a grant exists only when `dangerous == true`
  (`packaged_runtime.rs:89`).

So the tier boundary is exactly the `dangerous` parameter. Nothing else reaches it.

### Table (measured, at base)

| Spelling | clap field | feeds | approvals | sandbox | lease |
|---|---|---|---|---|---|
| `--force` | `cli.force` (`main.rs:337`) | `approval_bypass` (`main.rs:1091`) | Bypass | **Required** | none |
| `--yolo` | same field, `alias = "yolo"` (`main.rs:337`) | identical | Bypass | **Required** | none |
| `--dangerously-skip-permissions` | `cli.dangerously_skip_permissions` (`main.rs:343`) | `approval_bypass` (`main.rs:1091`) + stderr notice (`main.rs:1095`) | Bypass | **Required** | none |
| `--dangerous` | `cli.dangerous` (`main.rs:349`) | `dangerous` arg (`main.rs:1930`) | Bypass | **BYPASS** | yes, TTL-bounded |
| `--auto-approve` | `cli.auto_approve` (`main.rs:329`) | `CliArgs.auto_approve` (`main.rs:1765`) → `tools.auto_approve` (`config.rs:2270`) | Bypass | **Required** | none |

`--force` and `--dangerously-skip-permissions` are provably identical: they are
OR'd into one local bool on `main.rs:1091` and never read separately again except
for the stderr notice. `--yolo` is a clap alias of the same field, so it is the
same bool by construction, not by convention.

Existing `conflicts_with` at base:
- `main.rs:343`: `dangerously_skip_permissions` conflicts_with `dangerous`
- `main.rs:349`: `dangerous` conflicts_with_all `["force", "dangerously_skip_permissions"]`
- `main.rs:353`: `dangerous_ttl_secs` requires `dangerous`
- **`auto_approve` has NO conflict relationship with anything.**

## 2. `--auto-approve` — VERDICT: do NOT alias it. It is not tier 1.

The orchestrator brief flagged its own uncertainty here and was right to. Four
measured differences, any one of which is disqualifying:

1. **Parse-level conflict difference (decisive).** `--dangerous --force` is
   REJECTED by clap today (`main.rs:349`). `--dangerous --auto-approve` is
   ACCEPTED — `auto_approve` is in no `conflicts_with` list. Aliasing
   `--auto-approve` into tier 1 would start rejecting an invocation that works
   today. That is a behaviour change to existing scripts, which is the exact
   thing this lane exists to prevent.
2. **It is a config key, not just a flag.** `--auto-approve` is the CLI face of
   `[tools] auto_approve` (`config.rs:1002`, documented `config.rs:4789`) and is
   subject to GHSA-8r7g project-layer clamping (`config.rs:4140-4153`). The tier-1
   flags have no config equivalent at all. Same posture, different *kind* of thing.
3. **Provenance differs.** `has_cli_overrides` (`config.rs:3473-3480`) counts
   `cli.auto_approve` and does NOT count `cli.force` / `cli.dangerously_skip_permissions`.
   `--force` alone reports the CliOverrides row `Absent`; `--auto-approve` reports
   `Loaded`. Observable in `Config::resolve_with_provenance` output.
4. **They diverge on a reachable path.** The onboarding early-return
   (`main.rs:1828-1850`) builds `Config::default()` and calls
   `resolve_local_execution(&onboarding_config, approval_bypass, ...)`.
   `approval_bypass` carries `--force`/`--dsp` onto that path; `cli.auto_approve`
   is NOT consulted there and `Config::default()` has `auto_approve: false`, so
   `--auto-approve` is silently dropped on the onboarding branch while tier 1 is
   honoured.

Where they DO converge: `bootstrap.rs:681` calls
`set_smart_approval_policy(baseline_policy.approvals())`, which writes
`tools.auto_approve = true` (`config.rs:1357`) for a `--force` run too. So child
agents (`spawner.rs:1743`) inherit the same posture either way. Convergence at
runtime posture is NOT identity of the flag, and points 1-4 stand.

**Action: leave `--auto-approve` exactly as it is. Do not alias, do not
conflicts_with, do not touch its doc comment beyond what already exists.**

## 3. The hazard this lane must make impossible

If `--force`/`--yolo` ever become aliases of the tier-2 flag, every existing
script and CI job using them silently loses its OS sandbox on upgrade. The base
test `foreign_dangerous_alias_is_approval_only` (`main.rs:7642`) does NOT catch
this: it hardcodes `resolve_local_execution(&cfg, /*approval_bypass*/ true, /*dangerous*/ false, ...)`
rather than deriving those two arguments from the parsed `Cli`. Rewiring
`main.rs:1091` would leave that test green. The replacement test must derive the
tier from the parsed CLI.

## 4. Instrument controls run so far

- `/usr/bin/grep` known-positive: `smart_approval_policy` → 35 hits (alive);
  narrow needle `set_smart_approval_policy` → 8 hits. Neither result is an
  absence resting on a dead grep.
- zsh ate an unquoted `--include=*.rs` on the first attempt (LANE-BRIEF §3b-i);
  all subsequent greps quote the glob.

## 5. Status

- [x] Semantics measured, table above
- [x] `--auto-approve` verdict reached with evidence
- [ ] Rename + aliases wired
- [ ] Regression test authored, proven able to fail AND able to pass
- [ ] Docs updated
- [ ] hetzner build/test
- [ ] live binary drive
