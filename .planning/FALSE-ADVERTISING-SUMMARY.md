# Lane `false-advertising` — SUMMARY

**Defect class closed:** the product advertises capabilities that cannot work.

Branch `lane/false-advertising`, head **`b7e16e29`**, pushed to `gh`.
Merge-base captured once at start: **`d53fd54a9976cf71407c70a21ea22d89a5ae6a1e`**.
All diffs below are against that SHA, never the branch name.
Every build, test and live run on `hetzner-dsm` (`/root/wayland-false-advertising`).
The Mac ran only `cargo fmt --all -- --check`.

---

## Verdict per item

| Item | Landed | Criterion |
|---|---|---|
| 1. Browser config hint names the wrong TOML section | **YES** | `27-C2(a)` **CLOSED** |
| 2. `--skills-promote` advertised, always fails | **YES** (de-advertised) | `23A-C1` **STILL OPEN** |
| 3. Capabilities advertised on linkage, not liveness | **YES, wired + live-proven** | `27-C2(b)` **STILL OPEN** — needs the fenced regeneration in SR-FA-1 |

---

## Item 1 — `27-C2(a)`: the remediation text sent every operator in a circle

**Fixed.** `crates/wcore-browser/src/tool.rs:499` told a default-denied operator to
paste `[browser] allowed_origins = [...]`. The loader reads
`browser.policy.allowed_origins`. `BrowserConfig` and `BrowserPolicyConfig` are both
`#[serde(default)]` with no `deny_unknown_fields`, so the misplaced key parsed
cleanly and was **silently discarded** — no error, tool disabled forever.

**`default_action` had the same defect** (also on `BrowserPolicyConfig`), and so did
**`README.md:300`**, which shipped the identical wrong header. Both fixed. Grep for
`[browser]` across `--include=*.rs --include=*.md --include=*.toml --include=*.json`
now returns no remaining instruction to a customer; the only survivors are
`.planning/` ledger prose and one internal rustdoc at `config.rs:392` describing the
`[browser]` *table* (correct as written, not paste-able instruction).

### The regression guard, and proof it can fail

`crates/wcore-browser/src/config_hint.rs` is now the single source of truth for what
the hint prints. **Nothing hardcodes the section name.**
`crates/wcore-agent/tests/browser_config_hint_roundtrip.rs` takes those exact
constants and drives the real production chain:

`toml::from_str::<ConfigFile>` (the real serde types; `Config::resolve` assigns
`browser: merged.browser` verbatim) → `browser_adapter::apply_config_policy` (the
real copy `AgentBootstrap` performs) → `browser_adapter::spec_to_core` (the real
mirror→core conversion) → `BrowserPolicy::check_url` → **asserts `Allow`**.

Bootstrap's three-line policy copy was extracted to `apply_config_policy` so the
guard exercises the engine's own code, not a second copy of the mapping.

**Mutation proof (hetzner, uncommitted, reverted):** `sed`-reverted
`[browser.policy]` → `[browser]` in the constants. Result: **2 of 4 FAILED**, with
the failure text naming the defect exactly —

```
the remediation snippet the product prints does NOT enable the browser tool.
https://example.com/ was refused: policy violation: default_action=Deny and no rules
matched origin example.com
```

File restored; re-ran **4/4 pass**.

A negative control (`the_wrong_section_parses_silently_and_leaves_the_tool_disabled`)
pins the silent-drop behaviour itself, so a future reader can see why the guard
asserts an `Allow` decision rather than comparing strings: **a string comparison
would have passed against the buggy input too.**

**Gate results:** `wcore-browser --lib config_hint` 3/3 · roundtrip 4/4 ·
`wcore-browser` whole crate 124/124 · clippy clean.

---

## Item 2 — `23A-C1`: I took repair (a), and the criterion stays open

**Repair taken: de-advertise.** `#[arg(long, value_name = "PROCEDURE_ID", hide = true)]`
on `skills_promote` in `wcore-cli/src/main.rs`. The flag still parses and still fails
loudly, so anyone who already scripted it keeps the deliberate explanation instead of
a clap parse error.

### I measured (b) before declining it, as instructed

(b) is **not** small. Measured, not estimated:

* `ProcedureStatus` (`wcore-memory/src/v2_types.rs:359`) has **no `Revoked` variant**;
  adding one changes the enum, `as_str`, `FromStr`, `can_transition_to`, the DB
  serialisation and every exhaustive match.
* **No procedure generation store exists**, so there is nothing to roll back *to*.
  `grep -rni "fn.*rollback"` across `wcore-memory`/`wcore-skills`/`wcore-cli` returns
  only the unrelated plugin-lifecycle and backup-journal paths.
* **No artifact-provenance binding exists.** The `bail!`'s own docstring states the
  requirement: "a governed transaction that binds one reviewed procedure id to one
  canonical skill artifact."
* The mechanical `Staged → Active` hop is a five-line call to the existing
  `transition_procedure` — **and that is exactly the ungoverned promotion that was
  deliberately removed.** Re-adding it would make a drafted skill executable without
  review: a security regression wearing the costume of a fix.

The ledger's 3–4 sessions across three crates is correct. I did not take it.

### This does NOT close `23A-C1`

Governed promotion, revoke, rollback and append-only history are all still
unimplemented. **Only the false promise was retired.** Recording that plainly because
narrowing a criterion until the evidence satisfies it is the failure mode this
program was burned by.

### Class check, not instance check

Scanned all of `wcore-cli/src` for advertised-but-dead surfaces
(`temporarily unavailable|not yet implemented|unimplemented!|todo!(|coming soon`):
**`--skills-promote` is the only member of the class.** `--skills-archive`, its
nearest W9.1 sibling, is genuinely implemented via `transition_procedure` and is
correctly left advertised.

### Guard + live evidence

`crates/wcore-cli/tests/skills_promote_not_advertised.rs` drives the **real built
binary** (`CARGO_BIN_EXE_wayland-core`), not a reconstructed `clap::Command`, and
pins both halves — absent from `--help`, still loud when reached. It carries a
**sanity control** asserting the working `--skills-audit` IS still listed, so a
`--help` that broke entirely could not pass for the wrong reason.

**Live transcript, real binary, hetzner:**

```
$ wayland-core --help | grep -c -- "--skills-promote"
0
$ wayland-core --help | grep -- "--skills"
      --skills-path
      --skills-audit
      --skills-audit-stale-days <SKILLS_AUDIT_STALE_DAYS>
      --skills-archive <PROCEDURE_ID>
$ wayland-core --skills-promote 00000000-0000-0000-0000-000000000000
Error: skill promotion is temporarily unavailable while governed promotion is being implemented
EXIT=1
```

**Mutation proof:** removed `hide = true` on hetzner → `--skills-promote` reappears in
`--help` and `help_does_not_advertise_skills_promote` **FAILED**. Restored, 2/2 pass.
That mutation also confirms the pre-fix state directly: the flag *was* advertised.

---

## Item 3 — `27-C2(b)`: probe-based readiness, wired, live-proven, fenced at the last step

`capabilities.browser_suite` / `.computer_use` were derived from **linkage** — whether
the plugin crate was discovered and identity-verified.

`PluginCapabilitySet::narrowed_to_live` layers each backend crate's own probe on top
(`wcore-browser/src/liveness.rs`, `wcore-cua/src/liveness.rs`). Three invariants, each
pinned by a test that can fail independently:

1. **Only clears a flag, never sets one.** The Wave SC plugin-identity guarantee (a
   malicious crate named `wayland-browser` must not flip a UI badge) is preserved
   exactly — a probe that could widen would silently undo a SECURITY MAJOR fix.
2. **Narrows only on positive proof** that every compiled-in backend cannot start.
   Anything undecidable without launching a backend returns `Indeterminate` and
   **keeps** the capability.
3. **Never executes anything.** `which` (PATHEXT-aware) for binary presence, honouring
   `bootstrap.rs`'s standing note that `<command> --help` preflights run third-party
   code with the ambient environment and leak secrets. The only other probe is the
   same loopback healthcheck `BrowserSupervisor::ensure_ready` already makes.

Probes mirror the engine's real startup paths: browser = sidecar binary resolves **OR**
an externally managed sidecar answers `/health`; CUA = the `DISPLAY`/`WAYLAND_DISPLAY`
precondition the X11 and Wayland backends themselves enforce. macOS/Windows return
`Indeterminate` — no honest non-executing probe exists for a window-server session.

Every narrowing logs reason **and remedy** at WARN.

### Where the cross-audit panel changed the design

Panel per LANE-BRIEF §4, unique per-lane prompt dir (`panel-false-advertising-$$`),
`< /dev/null` on every member, unanchored vote extraction, byte-counted, and each
response verified to answer *this* lane's question (all three name camoufox and the
contract fence — no contamination).

| Member | Bytes | Vote |
|---|---|---|
| `codex exec -m gpt-5.6-sol` | 1,097 | SHIP_WITHOUT_CONTRACT_BUMP |
| `gemini -m gemini-3.1-pro-preview` | 1,200 | **DO_NOT_SHIP_PROBE** |
| `/Users/seandonahoe/.kimi-code/bin/kimi` | 3,895 | SHIP_WITHOUT_CONTRACT_BUMP |

**All three found a false-negative class I had missed, and they were right.** My first
draft probed only local Camoufox; `select_provider` has three backends, so a Chromium
or Browserbase deployment would have had a *working* capability stripped from its UI.
Kimi read `selection.rs` to establish it. **`Indeterminate` exists because of that
finding** — under-advertising is the same defect as over-advertising, pointed the other
way. Measured follow-up: `release.yml:144` builds `-p wcore-cli` with default features,
and `wcore-browser`'s `default = []`, so **chromium and browserbase are compiled out of
the shipped binary** — the carve-out is inert for the RC but correct if anyone builds
with those features.

**Recorded dissent (Gemini, 1 of 3):** silently dropping a capability replaces an
actionable runtime error with an un-debuggable missing feature. **Partly taken.** Its
severity rested largely on the same backend gap, which `Indeterminate` closes — but the
diagnostic point survives on its own, and is why every narrowing logs a reason *and* a
remedy. The reason survives even though the flag does not.

**Internal adversarial pass, arguing against the emerging consensus:** three of three
found a false-negative class on my first draft, so my base rate of missing another is
not low; the safest version narrows only on positive proof of unavailability. **That
pass survived and is invariant 2.**

### Live evidence — A/B on ONE binary, one headless host

`hetzner-dsm`: `DISPLAY` unset, `WAYLAND_DISPLAY` unset, `camofox-browser` not on
PATH, `localhost:9377/health` unreachable. Plugins **are** loaded (`plugins: true`), so
pre-fix both flags read `true` on this box — the defect exactly.

**Probes unsatisfied** — `ready` event omits both flags (they are skip-if-false; absent
*is* `false`, and is already what a host sees when a plugin is absent), plus:

```
WARN not advertising browser_suite: the plugin is loaded but no backend can start
  reason=no browser backend can start: `camofox-browser` does not resolve on PATH
         and no sidecar answered http://localhost:9377/health
  remedy=install @askjo/camofox-browser, or set WAYLAND_CAMOUFOX_BIN ...
WARN not advertising computer_use: the plugin is loaded but no backend can start
  reason=neither DISPLAY nor WAYLAND_DISPLAY is set, so no display server is
         reachable and the X11 backend cannot connect
  remedy=run inside a graphical session, or export DISPLAY ...
```

**Same binary, probes satisfied** (`WAYLAND_CAMOUFOX_BIN=/bin/sh DISPLAY=:0`):

```
browser_suite = True
computer_use  = True
narrowing WARNs this run: 0
```

That A/B is the proof the absence is *the narrowing* and not a broken pipeline or a
stuck-off probe.

**Mutation proof:** made `narrowed_to_live` a no-op on hetzner →
`narrows_when_no_backend_can_start` **FAILED** ("27-C2(b) is not fixed"). Restored,
3/3 pass. The guard also refuses to pass vacuously: if both probes returned
`Indeterminate` it asserts nothing, so it fails rather than self-passing.

### Why `27-C2(b)` is still OPEN

The code is landed, tested and live-proven, but the branch is **red on
`wcore-protocol --test desktop_contract_corpus`**, and clearing that requires a
regeneration reserved to Sean. See SR-FA-1. I did not run `wcore-contract generate`.

---

## The red, and what it is not

**`cargo test -p wcore-protocol` → 177 pass, 1 FAILED**
(`checked_corpus_matches_real_serializers_byte_for_byte`). Reported red, not worked
around. No test was weakened, ignored, re-gated or deleted.

*Caveat on that 177:* `cargo test` stops after the first failing test target, so this
is a **partial** run — the targets after `desktop_contract_corpus` never executed. It
is not comparable to the full-suite 302 in `HANDOFF-2026-07-28.md` §2, and I am not
claiming it is. Once SR-FA-1 regenerates, the full count should be re-taken.

**It is NOT a wire-shape change.** `git diff <base> -- crates/wcore-protocol` is
**empty**. Read-only `wcore-contract digest` (the `digest` subcommand, not `generate`):

| Digest | base | lane head | moved? |
|---|---|---|---|
| `schema_digest` | `e5d1744a…2e54` | `e5d1744a…2e54` | **NO** |
| `source_inputs_digest` | `25170996…9336` | `2ec10eab…1aa18` | yes |
| `fixture_digest` | `634bbbe9…30fa` | `0a496996…a010` | yes |

Cause, measured: `contract/spec.rs:833` `SOURCE_INPUTS` digests **40 engine source
files**, including `wcore-agent/src/bootstrap.rs`, `output/protocol_sink.rs` and
**`wcore-cli/src/main.rs`** — the very file LANE-BRIEF §6 designates as the shared
file every lane edits. Drift set is `missing=[], extra=[]` and exactly the five
descriptor-carrying artifacts; no `schema/*.json` drifted.

**Verified green at base** (`d53fd54a`, separate worktree, same command: 15/15 pass) and
**red at my first commit** `793bead9` — so it is mine, and it is caused purely by
editing a digested engine file, not by any protocol change. I did not assume either
direction; I measured both.

**Orchestrator action:** ONE regeneration over the merged tree after all lanes land.
Not one per lane — the artifact is byte-exact and N regenerations would conflict.
Same shape Sean authorized at `c743f398`. Desktop must re-pin in the same train.

---

## Gate results — real numbers, isolated runs

Every figure below is from a **per-crate** run on `hetzner-dsm`, never a
full-workspace run under lane contention.

| Suite | Result |
|---|---|
| `wcore-browser` (whole crate) | **124 pass, 0 fail** |
| `wcore-cua` (whole crate) | **86 pass, 0 fail** |
| `wcore-agent` browser/capability tests | **18 pass, 0 fail** |
| `wcore-cli` promote/lockfile/harness | **12 pass, 0 fail** |
| `wcore-protocol` | **177 pass, 1 FAILED** (contract corpus — SR-FA-1; partial, cargo aborts after the failing target) |
| `cargo clippy -p wcore-browser -p wcore-cua -p wcore-agent -p wcore-cli --all-targets` | **0 errors** |
| `cargo fmt --all -- --check` (Mac) | clean |

Pre-existing `capability_advertising_test` (6/6) and `plugin_single_reification_call_site`
(3/3) still pass — the identity-check layer is unregressed.

---

## Fence compliance

* `git diff <BASE-SHA> -- crates/wcore-cli/src/lib.rs crates/wcore-cli/src/main.rs`
  → **`main.rs` only, +11 −1, one contiguous block** (the `hide = true` attribute and
  its explanatory docstring). `lib.rs` untouched. No reformatting, no reordering, no
  drive-by cleanup. Diffed against the captured SHA, never the branch name.
* `git diff <BASE-SHA> -- crates/wcore-protocol` → **empty**.
* `cron.rs` untouched. No `git add -A`, no `checkout`/`reset`/`stash`/`rebase` on shared
  refs, no `Co-Authored-By`.
* Did NOT run `wcore-contract generate`. Did NOT merge into
  `plan/f20-unified-audit-repair`. Did NOT open a PR, tag, release or close an issue.

Full change set: 17 files, +1052 −17.

---

## What I did NOT do

* Did **not** close `23A-C1`. Only stopped advertising it.
* Did **not** close `27-C2(b)`. Code is in and live-proven; the last step is fenced.
* Did **not** implement governed promotion, revoke, rollback or append-only history.
* Did **not** implement SR-27-1..3 (`CapabilityId` variants, activation ladder, reason
  codes, `CONTRACT_MINOR` bump). Those remain open and unstarted.
* Did **not** address `27-C2`'s third leg — the three missing policy baselines
  (downloads-root confinement, the approval gate on a computer-use op, process count
  before/during/after plus one reaper interval). Untouched, still unmeasured.
* Did **not** narrow `PluginRunner::with_computer_use_advertised(true)`. That is the
  *reification* gate, not the wire flag: clearing it would unregister the CUA tool
  entirely. Leaving it registered preserves a loud typed error at first use, which is
  the honest failure. Deliberate, and flagged here rather than left implicit.
* Did **not** run a full-workspace test run, by policy.

---

## Cleanup

Hetzner worktrees `/root/wayland-false-advertising` and `/root/wayland-fa-base` were
created by this lane and removed on completion, with their `target/` dirs. No other
lane's worktree was touched.
