---
phase: 25-remote-reach-nodes-plugin-lifecycle
plan: "02"
subsystem: plugin-lifecycle
tags: [f25-04, plugin, lifecycle, approval-gate, signing, generations, rollback, recover]
status: complete
termination_state: 1
requires:
  - wcore-agent sig_verifier (the one Ed25519 trust root)
  - wcore-cli marketplace resolver / quarantine / lockfile
  - templates/plugin-static, templates/plugin-wasm
provides:
  - the twelve-verb `wayland-core plugin` operator surface
  - wcore-config::plugin_governance — one shared approval verdict for CLI and engine
  - retained content-addressed generations behind update / rollback / recover
  - a native (non-lowered) install path for Wayland plugins, which did not exist
affects:
  - crates/wcore-cli (ten new verbs, one new install branch)
  - crates/wcore-agent (loader approval gate; one bootstrap log line)
  - crates/wcore-config (new plugin_governance module)
  - templates/ (both shipped scaffolds were unusable and are repaired)
tech-stack:
  added: []
  patterns: [single-trust-root, fail-closed-gate, retained-prior-state, live-product-exercise, digest-bound-consent]
key-files:
  created:
    - crates/wcore-config/src/plugin_governance.rs
    - crates/wcore-cli/src/plugin/{scaffold,verify,sign,publish,inspect,approve,generations,lifecycle,recover}.rs
    - crates/wcore-cli/tests/plugin_lifecycle_cli.rs
    - .planning/phases/25-remote-reach-nodes-plugin-lifecycle/25-02-LIFECYCLE-TRANSCRIPT.md
    - .planning/phases/25-remote-reach-nodes-plugin-lifecycle/25-02-CLI-GATE-DECISION.md
  modified:
    - crates/wcore-cli/src/plugin/{mod,marketplace}.rs
    - crates/wcore-cli/Cargo.toml (rand dev-dep → dep; ZERO new packages)
    - crates/wcore-agent/src/plugins/loader.rs, crates/wcore-agent/src/bootstrap.rs
    - crates/wcore-config/src/lib.rs
    - templates/plugin-static/cargo-generate.toml, templates/plugin-wasm/cargo-generate.toml
    - crates/wcore-plugin-api/tests/template_smoke.rs, templates/plugin-wasm/tests/template_smoke.rs
decisions:
  - "Approval enforcement is ROOT-SCOPED; panel split 2-1 (A/A/C), basis=majority, dissent converted into a binding condition."
  - "Approval binds to the plugin directory's SHA-256, so an update invalidates consent rather than inheriting it."
  - "A plugin declaring no entry artifact is REFUSED by `plugin sign` rather than signed under a second scheme."
  - "A Wayland-native plugin is installed byte-for-byte; the lowering pipeline is for foreign formats only."
metrics:
  tests_added: 64
  new_third_party_crates: 0
  defects_found_live: 4
  panel_members: 4
completed: 2026-07-27
---

# Phase 25 Plan 02: Twelve-Verb Plugin Lifecycle — Summary

Ten missing verbs landed on the real `wayland-core plugin` surface, approval became a load-time
gate the engine actually enforces, and all twelve verbs plus all four negative cases were driven
through the shipped release binary on Linux and on Windows.

**Success Criterion 3 is MET on Linux and MET-with-one-recorded-divergence on Windows.**
**Termination state 1.**

---

## 1. What landed

| Verb | Where | What makes it real rather than printed |
|---|---|---|
| `new` | `scaffold.rs` | drives the existing `templates/` scaffolds; refuses with the install command when `cargo generate` is absent, leaving nothing behind |
| `test` | `scaffold.rs` | returns the plugin suite's own exit status — proven non-zero against a deliberately red fixture |
| `verify` | `verify.rs` | non-zero exit on an incompatible declared API version, so it is scriptable as a gate |
| `sign` | `sign.rs` | detached Ed25519 over the entry artifact, written where `sig_verifier` reads it |
| `publish` | `publish.rs` | digest-addressed bundle into a local marketplace directory; refuses unsigned material; never pushes |
| `inspect` | `inspect.rs` | reports the LOADER's verdict via the same `evaluate()` call the engine makes |
| `approve` | `approve.rs` | durable record bound to the content digest; `--revoke` retains the revocation |
| `update` | `lifecycle.rs` | plan-then-commit over retained generations |
| `rollback` | `lifecycle.rs` | restores the retained generation and proves it by digest equality |
| `recover` | `recover.rs` | repairs induced damage; refuses to alter approval state |
| `install` | `mod.rs`, `lifecycle.rs` | gained a native branch and a bundle-integrity gate |
| `remove` | `mod.rs` | now actually removes a marketplace install and its lifecycle state |

The load-bearing pieces are `wcore-config::plugin_governance` (one shared verdict) and
`generations.rs` (content-addressed retained state). **Zero new third-party crates**; `rand` moved
from `[dev-dependencies]` to `[dependencies]` in `crates/wcore-cli/Cargo.toml` for
`sign --new-key`, which adds no packages to `Cargo.lock`.

## 2. Approval is a gate, not a prompt — proven negatively AND positively

The plan called this its most important test. It is enforced in
`wcore-agent/src/plugins/loader.rs`, **before any runtime dispatch**, so an unapproved plugin
never reaches a spawn, a WASM compile or a hook registration.

Driven through the real engine on `hetzner-dsm`:

```
# unapproved
WARN on-disk plugin load failed (continuing) plugin=lifecycle-demo
  error=plugin approval required: lifecycle-demo: installed at digest c564afd22804
        but never approved — run `wayland-core plugin approve lifecycle-demo`

# after `plugin approve` (and installing the author key as a trust anchor)
INFO on-disk plugin loaded plugin=lifecycle-demo
```

Revoking re-arms the refusal. Updating the plugin changes its digest and therefore its verdict:
`approved digest c564afd22804 does not match installed digest a818b717bde6`. Consent does not
travel across a change of bytes.

The verdict is computed by ONE function that both the CLI and the engine call. `plugin inspect`
cannot disagree with the loader, because it is not a second opinion.

## 3. The cross-audited decision

Enforcement scope was a real judgement call and went to the 4-way panel with `(Recommended)`
stripped. **Gemini: A. Kimi: A. Codex: C.** Decision: **A (root-scoped governance),
basis = majority.** Full record with the dissent's reasoning quoted:
`25-02-CLI-GATE-DECISION.md`; verbatim captures in `evidence/25-02-panel-*.txt`.

The internal adversarial pass sustained Codex's attack — under A, deleting `generations.json`
un-governs a whole root in one file operation, which neither majority member raised. Converted
into a binding condition rather than discarded: `is_governed()` now returns true if **either**
marker file exists, so un-governing requires destroying the approval record itself. Pinned by a
test that fails against the single-marker implementation.

## 4. Live evidence

Full detail: `25-02-LIFECYCLE-TRANSCRIPT.md`. Ledgers:
`evidence/25-02-lifecycle-ledger.txt` (Linux, 475-line transcript) and
`evidence/25-02-lifecycle-windows-ledger.txt` (Windows, 369-line transcript).

- **Linux (`hetzner-dsm`): 12/12 verbs PASS, 4/4 negative cases PASS, plus the positive
  approved-loads case PASS.**
- **Windows (`SeanDesktop`): 11/12 verbs PASS** (`new` NOT-RUN — `cargo-generate` is not
  installed there; the verb refused correctly), **4/4 negative cases PASS**, one divergence
  recorded (§7).

The generation swap and rollback survived Windows, which is the leg most likely to have broken:
those are rename-and-replace operations over directories, and Phase 20A's Windows defects were
overwhelmingly handle and path semantics. Rollback produced byte-identical content verified by
per-file SHA-256 against a snapshot taken before the update.

## 5. Gate results

| Gate | Result |
|---|---|
| `cargo fmt --all -- --check` (Mac) | clean |
| `cargo clippy -p wcore-cli -p wcore-agent -p wcore-config --all-targets --all-features -- -D warnings` | clean (one real finding fixed: a redundant `trim_start`) |
| `cargo nextest run -p wcore-cli --test plugin_lifecycle_cli` | **14/14 pass** |
| `cargo nextest run -p wcore-cli` (whole crate) | **2081/2081 pass**, 9 skipped |
| `cargo nextest run -p wcore-agent -E "test(/plugin/)"` | **116/116 pass** — the loader gate is backwards compatible |
| `cargo nextest run -p wcore-config -E "test(/plugin_governance/)"` | **10/10 pass** |
| `cargo nextest run -p wcore-plugin-api --test template_smoke` | **1/1 pass** — and it now actually RUNS |
| `cargo build --release --locked -p wcore-cli` (Linux + Windows) | both exit 0; both list twelve verbs |
| Fenced files untouched | `Cargo.toml`, `wcore-cli/src/{main,lib}.rs`, `wcore-config/src/config.rs` all unchanged vs base |

`Cargo.lock` carries exactly one commit, the coordinator's base fix `9a86b287`, cherry-picked as
instructed. Nothing in this plan regenerated it.

## 6. Four defects the live exercise found

Not one was a crash. All four were **false answers or dead functionality** — the class a green
suite cannot catch, and every one was found by driving the real binary rather than by running
tests.

1. **`plugin sign` wrote the signature where the verifier never looks.** `sig_verifier` reads
   `<entry_artifact.parent()>/wayland-plugin.sig`, not `<plugin_dir>/wayland-plugin.sig`. For a
   manifest declaring `binary_path = "bin/run"` the sig belongs in `bin/`. The verb reported
   success and the loader then refused the plugin with `SignatureMissing`. Found by calling the
   ENGINE's own verifier from the test rather than re-checking with local crypto.

2. **`plugin install` had NO path for a Wayland-native plugin at all.** The marketplace path
   runs everything through `wcore-pluginsrc`, which only detects the foreign Claude Code format
   and whose commit step *generates* a `plugin.toml` from a canonical draft. Pointed at a native
   plugin it failed outright with `unrecognized plugin format`; had it lowered one, it would have
   discarded the author's manifest, the entry artifact and the signature — installing something
   that is not what was signed. A native branch now installs verbatim, because a
   digest-addressed signed bundle has to arrive byte for byte or it is not the signed thing.

3. **`plugin remove` could not remove a marketplace-installed plugin.** The verb only knew about
   legacy `<name>.json` records, so it reported "not installed" while the directory stayed on
   disk and kept loading. `remove_marketplace_plugin` existed and had no caller.

4. **Both shipped plugin templates were unusable.** Each declares an `authors` placeholder;
   cargo-generate reserves `project-name`, `crate_name`, `crate_type`, `authors` and `os-arch`
   and refuses the *entire run* when a template declares one. Both templates' own smoke tests
   pass the same rejected flag and would have caught it — but both **skip when cargo-generate is
   absent**, and it has never been installed in CI, so a scaffold nobody could use looked
   permanently green. Installing cargo-generate on `hetzner-dsm` made
   `wcore-plugin-api::template_smoke` execute for the first time.

Defect 4 is the plan's own "a gate that was already green proves nothing" failure mode, found in
the wild.

## 7. Deviations from the plan

**[Rule 3 — blocking] `rand` promoted from dev-dependency to dependency in
`crates/wcore-cli/Cargo.toml`.** `plugin sign --new-key` mints the keypair an author signs with,
so key generation is a production path. `rand` was already a dev-dep of this crate and is already
pinned at 0.8 in the lockfile: **zero packages added**. `crates/wcore-cli/Cargo.toml` is not one
of the five fenced files, and the root `Cargo.toml` and `Cargo.lock` are untouched by this plan.

**[Rule 3 — blocking] `crates/wcore-cli/src/plugin/marketplace.rs` was edited.** Not in
`files_modified`. One extraction, no behaviour change: the catalog lookup, traversal check and
quarantine clone at the head of `resolve_and_plan` became `resolve_source`, so the native install
branch shares them instead of carrying a second copy. AGENTS.md forbids duplicating that logic.

**[Rule 3 — blocking] `crates/wcore-agent/src/bootstrap.rs` gained one `tracing::info!`.** The
negative half of the gate was observable (an existing warn); the positive half was not, and "no
error line" is not evidence that a plugin loaded.

**[Rule 2 — missing critical functionality] `templates/plugin-wasm/` and both template smoke
tests were repaired.** Only `templates/plugin-static/` is in `files_modified`. The identical
one-line defect broke the wasm template, and leaving it would have left half the scaffold surface
dead for the sake of a file list.

**[Plan gate defect — reported, not worked around]** Task 1 and Task 2's dispatcher gates use
`grep -cE "^[[:space:]]*$V([(,{]|\$)"`, which cannot match rustfmt's output `    New {` — there is
a space before the brace. The gate fails against a correct tree. I ran its **intent** with
`^[[:space:]]*${V}[[:space:]]*[({,]` plus a fallback for bare variants, and included a negative
control (a nonexistent `Teleport` variant, correctly reported absent) so the corrected gate is
shown able to go red. The code was NOT reformatted to satisfy a broken regex.

## 8. Known gaps, stated plainly

- **`F25-SC3-WIN-NEG-APPROVED-LOADS: PARTIAL`.** On Windows the approval gate opens correctly —
  the refusal message disappears after `plugin approve` — but the demo plugin does not complete
  its load, failing at `SubprocessPluginRunner::load: subprocess spawn failed: %1 is not a valid
  Win32 application`. The fixture's entry artifact is a Python script and Windows
  `CreateProcess` cannot execute a `.py` directly. **This is a fixture limitation, not a product
  defect**, and it is recorded PARTIAL rather than PASS because the completed-load line a PASS
  requires is genuinely absent. Closing it needs a compiled Windows entry artifact.
- **`plugin new` is unexercised on Windows.** `cargo-generate` is not installed on `SeanDesktop`.
  The verb refused with the exact install command and left nothing behind, which is the designed
  behaviour, but the generate path itself did not run there.
- **No TUI observation on either platform.** No part of this lifecycle surfaces in the TUI, and
  the repo's PTY harness is `#![cfg(unix)]` regardless. Nothing is claimed.
- **Only a local-directory marketplace was driven.** Git and GitHub sources use the same
  (unchanged) `resolve_source` path but were not exercised end to end.
- **No WASM-runtime plugin was driven end to end.** `sign` and `verify` resolve a WASM
  `component_path` through the same code, untested against a real component.
- **The A-scope residual.** An attacker with arbitrary write access to the plugins root can
  delete both governance markers and revert the root to ungoverned. Named in
  `25-02-CLI-GATE-DECISION.md` with the two ways to close it. Not closed here and not claimed as
  closed.

## 9. Backlog candidates (MEDIUM and below, non-blocking)

- `[MED]` Neither template smoke test runs in CI, because `cargo-generate` is not installed
  there. A skip that has never once executed is indistinguishable from a pass. Either install the
  tool in CI or make the skip loud.
- `[MED]` `plugin publish` writes a `bundle.json` that provides integrity, not authenticity — an
  attacker editing both the tree and the sidecar defeats it. The authenticity anchor is
  `wayland-plugin.sig` against the trust root. Signing the bundle manifest itself would close the
  gap without minting a second scheme.
- `[LOW]` The Windows lifecycle fixture needs a compiled entry artifact to close
  `WIN-NEG-APPROVED-LOADS`.
- `[LOW]` `/root/f25-02-lab`, `/root/f25-02-evidence`, `/root/wayland-25` (branch `hz/25`) on
  `hetzner-dsm`, and `C:\f25-02-lab`, `C:\f25-02-evidence` on `SeanDesktop`. Named for this plan
  and safe to remove.

## 10. Requirements

- **F25-04 — COMPLETE on Linux.** All twelve verbs ran against the shipped binary with an
  independently observed state change after each, and all four negative cases held.
- **Success Criterion 3 — MET on Linux**, met on Windows for eleven verbs with one verb unrun for
  an environment reason and one divergence recorded above.

## Self-Check: PASSED

All named files exist in the worktree; all commits are present on `lane/25`.
