# Lane `27-c2b-readiness` — SUMMARY

**Verdict: `27-C2(b)` is CLOSED. It was already substantially closed before I started, and my
brief's central premise was false. I found and fixed the one re-drift vector the earlier fix
left open, and live-proved the whole thing at integration head.**

Branch `lane/27-c2b-readiness`, head **`73a9f997`**, pushed to `gh`.
Merge-base captured once at start: **`d622cb09de01329cef6f20d6f9183df171462daf`**, asserted
against `git ls-remote gh plan/f20-unified-audit-repair` before creating the worktree, and
re-asserted on `hetzner-dsm` after the worktree add. All diffs are against that SHA, never a
branch name.

Every build, test and live run on `hetzner-dsm` (`/root/wayland-27c2b`). The Mac ran only
`cargo fmt --all`. Every number below was read back from `/root/.cargo/bin/cargo` invoked by
**absolute path** and every result line still carries `0 ignored` and its `filtered out`
count — the two fields §3b says the `rtk` proxy strips, so their presence is the evidence the
proxy was not in the path.

---

## 1. Which premises held

Per §"Your brief's MEASUREMENTS are probably stale", this report is part of the deliverable.

| Claim | Verdict |
|---|---|
| `bootstrap.rs:754` is `PluginRunner::new().with_computer_use_advertised(true)`, unconditional | **HELD**, verbatim, at exactly `:754` |
| the in-source justification is per-OS reify-time self-gating | **HELD** (`bootstrap.rs:745-753`) |
| ledger's older `:696` has moved | **HELD** |
| `CONTRACT_MINOR = 10`, corpus pinned at 23 commands / 52 events | **HELD** (`generate.rs:24`, `desktop_contract_corpus.rs:217-218`) |
| SR-27-1's `CapabilityId::{Browser, ComputerUse, Web}` are unimplemented | **HELD** — the enum still ends at `LegacyAutoSkillDrafting` |
| **"readiness is advertised on the basis of whether a plugin crate is linked"** | **FALSE at my base** |
| **the ledger's 2026-07-30 re-grade "(b) is UNCHANGED and still open"** | **FALSE when it was written** |
| **"on a headless box the operation fails with `spawn camoufox: No such file or directory`"** | **FALSE at my base** — it is wrapped; see §4 |

### The ledger row cites a real line that does not support its conclusion

`:754` is a **reify-time registry-admission gate**. When false, `cua_adapter.rs:80` refuses
every captured `CuaToolSpec` with `CapabilityDisabled` and no CUA tool reaches the registry at
all. It does not touch the wire.

The **wire capability flags** are produced 187 lines later, at `bootstrap.rs:939-942`:

```rust
let plugin_capabilities =
    crate::output::protocol_sink::PluginCapabilitySet::from_verified(&verified_plugins)
        .narrowed_to_live()
        .await;
```

`narrowed_to_live()` (`output/protocol_sink.rs:186`) runs `wcore_browser::liveness::probe()` and
`wcore_cua::liveness::probe()` and clears each flag on positive proof of unavailability. Two
different code paths were conflated: **tool admission** vs **readiness publication**.

This landed at **`85b60a2f` on 2026-07-28 16:44**. The re-grade that calls it unchanged is
**`71acfd19`, 2026-07-30 02:28** — 34 hours later, with `85b60a2f` already in its own ancestry
(`git merge-base --is-ancestor` → true). So this was not a stale row; it was **graded off an
instrument with no reachable pass state**, which is precisely the `§3b-iii` defect the same
re-grade added to the brief. `:754` reads `true` forever no matter what anyone builds, so citing
it can only ever return "still open".

Instrument liveness for the greps above: `with_computer_use_advertised|computer_use_advertised`
returned **31 hits across 11 files**, `narrowed_to_live` **5 hits in 3 files**. Non-zero, so the
greps were alive; unproxied `/usr/bin/grep` throughout.

---

## 2. What I actually fixed — the last drift vector

**The probe re-derived the sidecar program in a second copy of the supervisor's own logic.**

* `supervisor.rs:70-73` — `SupervisorConfig::local_camoufox` sets
  `sidecar_program = WAYLAND_CAMOUFOX_BIN` or `"camofox-browser"`.
* `liveness.rs:88` (pre-fix) — `camoufox_program()` computed **the identical expression again**,
  under a docstring *claiming* it performed "the same resolution `SupervisorConfig::local_camoufox`
  performs". Two literals, one asserted-in-prose invariant, **nothing enforcing it.**

That claim *is* the probe. If the probe resolves a different program than the supervisor spawns,
the published flag describes a binary nobody runs, and `27-C2(b)` reopens in **either**
direction. Both were reachable; I proved one of them live.

**The fix:** the probe now builds the production config once and reads both the program and the
healthcheck URL out of it, so the two cannot disagree. Consequences beyond the drift:

* `sidecar_program: None` — the supervisor's documented **observe-only mode** — becomes
  representable. Pre-fix the probe substituted a guessed `"camofox-browser"` and hunted a binary
  the supervisor had been explicitly told not to launch.
* the `Unavailable` reason now quotes the URL the supervisor **would poll**, not one
  reconstructed in the probe. `local_camoufox` trims a trailing slash before appending `/health`;
  a caller passing `".../"` previously got a remedy naming `...//health`, an address nothing serves.

One source file changed: `crates/wcore-browser/src/liveness.rs`.

---

## 3. The four-arm live differential — the pre-fix defect, on real hardware

Full evidence: `.../evidence/27-c2b-readiness/DRIFT-DIFFERENTIAL.md`.

`hetzner-dsm`, headless, `camofox-browser` not on PATH. Drift simulated by renaming
`supervisor.rs:72`'s default `camofox-browser` → `camoufox-browser` (a plausible typo-correction —
the shipped name really is missing the `u`), with a fake executable planted under the **new** name
so the machine genuinely has a working browser.

| Arm | tree | supervisor spawns | resolves | probe verdict | withdraws? | |
|---|---|---|---|---|---|---|
| 1 | mine | `camofox-browser` | false | `Unavailable` | true | ✅ headless truth |
| 2 | mine + drift | `camoufox-browser` | **true** | `Ready{camoufox-binary}` | **false** | ✅ follows config |
| 3 | **pre-fix** + drift | `camoufox-browser` | **true** | `Unavailable` | **true** | ❌ **DEFECT** |

**Arm 3, verbatim:**

```
SUPERVISOR_WOULD_SPAWN=Some("camoufox-browser")
SUPERVISOR_PROGRAM_RESOLVES=true
PROBE_VERDICT=Unavailable(Unavailable { reason: "no browser backend can start: `camofox-browser` does not resolve on PATH and no sidecar answered http://127.0.0.1:1/health", ... })
PROBE_WITHDRAWS_CAPABILITY=true
```

A browser that **is installed and would have started** had its capability withdrawn from the
Desktop UI, and the operator was told to install a package the engine would never launch. That is
the *under-advertising* direction — the false-negative class the earlier lane's panel unanimously
flagged and which `Indeterminate` was introduced to prevent. It survived in this one path because
the resolutions were separate literals.

**And the pre-fix suite did not notice:** `5 passed; 0 failed; 0 ignored; 0 measured; 84 filtered
out`. That is §6b-ii's third assertion — *the old shape would have missed it* — demonstrated
behaviourally rather than asserted.

Arm 2 is the same drift on my tree with **no defect at all**: the fix does not detect this drift,
it makes it unrepresentable. There is one literal now.

### Three-assertion self-test, re-run at the final SHA `73a9f997`

**Known-positive:** `7 passed; 0 failed; 0 ignored; 0 measured; 84 filtered out`.

**Known-negative** — `camoufox_program` reverted to ignoring the config (the exact pre-fix shape),
verbatim:

```
thread 'liveness::tests::the_probe_reads_the_program_out_of_the_supervisors_own_config' panicked at crates/wcore-browser/src/liveness.rs:321:9:
assertion `left != right` failed: the two arms produced the same sidecar program, so this test compares a state with itself
  left: Some("camofox-browser")
 right: Some("camofox-browser")

thread 'liveness::tests::camoufox_program_honours_the_operator_override' panicked at crates/wcore-browser/src/liveness.rs:264:9:
assertion `left == right` failed
  left: Some("camofox-browser")
 right: Some("/opt/custom/camoufox")

failures:
    liveness::tests::camoufox_program_honours_the_operator_override
    liveness::tests::observe_only_mode_has_no_binary_to_look_for
    liveness::tests::the_probe_reads_the_program_out_of_the_supervisors_own_config

test result: FAILED. 4 passed; 3 failed; 0 ignored; 0 measured; 84 filtered out
```

Restored → `7 passed; 0 failed`.

Worth noting **which** assertion fired first: the `assert_ne!` that the two arms are different
experiments. A same-string comparison would have **passed** on the mutated tree, because under the
mutation both arms return the identical stale literal and `==` between two copies of one wrong
value is satisfied. That guard exists because of §6a-i.

**Both directions per §3b-iii.** Can it fail — arm 4, three named failures. **Can it pass** — arm
2, the state it claims to detect constructed on a real host, gate green. Arm 1 passes on the
un-mutated headless truth, so neither polarity is stuck.

The guard is modelled on the one that closed clause (a): it round-trips through the **real
`local_camoufox` constructor** (the exact call `adapter.rs:51` makes on the engine's startup path)
rather than comparing two strings, and asserts the probe's own **public verdict** in both
polarities from one call site.

---

## 4. The headless measurement — real binary, both directions, one host

`hetzner-dsm`, ambient `DISPLAY` and `WAYLAND_DISPLAY` both UNSET, `camofox-browser` not on PATH,
nothing on `localhost:9377`. Real `wayland-core` binary (337,521,600 bytes) built at lane head,
`--json-stream`, hermetic `WAYLAND_HOME`. Flags read out of **the product's own `ready` event**,
not inferred from my environment setup (§3b-ii).

**Arm A — headless (this box's real state):**

```
    plugins        = True
    browser_suite  = None      <- absent; skip_serializing_if(is_false), so absent IS false
    computer_use   = None
  narrowing WARN count: 2
```

```
WARN not advertising browser_suite: the plugin is loaded but no backend can start
  reason=no browser backend can start: `camofox-browser` does not resolve on PATH and no sidecar answered http://localhost:9377/health
  remedy=install @askjo/camofox-browser, or set WAYLAND_CAMOUFOX_BIN to the executable
WARN not advertising computer_use: the plugin is loaded but no backend can start
  reason=neither DISPLAY nor WAYLAND_DISPLAY is set, so no display server is reachable and the X11 backend cannot connect
  remedy=run inside a graphical session, or export DISPLAY for an available X server (e.g. an Xvfb ...)
```

`plugins = True` is the control that makes this leg mean something: the plugins **are** linked and
identity-verified, so the two absences are the narrowing and not an inert plugin system.

**Arm B — same binary, dependencies planted (`WAYLAND_CAMOUFOX_BIN=/bin/sh DISPLAY=:0`):**

```
    plugins        = True
    browser_suite  = True
    computer_use   = True
  narrowing WARN count: 0
```

**I can show the positive arm** — the flags read `true` where the dependency exists, on the same
binary and the same host. So the flags are not stuck off, and the absence in arm A is the probe
doing its job.

### What the user now sees instead of a bare spawn failure

Driven against the **production** config (`SupervisorConfig::local_camoufox(CamoufoxBackend::default_url())`)
on the headless box:

```
PRODUCTION_CONFIG program=Some("camofox-browser") health=http://localhost:9377/health
USER_SEES: Camoufox is unavailable at http://localhost:9377/health and Core could not start
`camofox-browser`: spawn camoufox: No such file or directory (os error 2).
Install @askjo/camofox-browser or set WAYLAND_CAMOUFOX_BIN to its executable
```

**Being precise, because my brief overstates this:** the ENOENT is **still in the string**. It is
not a bare `spawn camoufox: No such file or directory` — it is wrapped with the address probed,
the program name, and the remedy. I would not call the raw text a defect: the OS error is the most
diagnostic part, and stripping it would make the message worse. The wrap came from `73eb0ee2`, not
from this line of work, and I did not change it.

Reachability of the raw message was checked rather than assumed: `spawn camoufox:` is constructed
at exactly **one** site (`supervisor.rs:373`), inside `launch_camoufox_program`, whose only
non-test caller is `ensure_ready` (`:304`) which wraps it. The public `launch_camoufox` that
returns it unwrapped is called from **two places, both tests** (`:841`, `:865`). So the bare form
is not reachable from a user path.

---

## 5. The reify-time self-gating argument — where I land

**I engaged with it, and it is correct. It is also answering a different question, and the
ledger's own evidence line is what conflates them.** Both halves are true at once:

* **The in-source note is right that `:754` should stay `true`.** Making it conditional does not
  publish honest readiness — it **unregisters the CUA tool entirely** (`cua_adapter.rs:80` →
  `CapabilityDisabled`), converting a loud typed error at first use into a silently missing tool.
  That is strictly worse for the operator and is the recorded panel dissent's exact objection. So
  the remediation the ledger row implies would have been the **wrong fix**, and the earlier lane
  was right to decline it explicitly.
* **The criterion was right that readiness was not published.** Reify-time self-gating protects
  the *operation*; it does nothing about the *advertisement*, and the host renders the
  advertisement.

The resolution is that these are two different code paths and the fix belongs on the publication
path, which is exactly where `85b60a2f` put it — a separate probe layered on the flag, leaving the
reification gate alone. **The criterion is not wrong and the in-source reasoning is not wrong; the
ledger's *evidence* is wrong**, because it used a reification-gate line number as proof about a
publication property.

---

## 6. Contract coordination — the wire did NOT change

**No contract action is required from me, and the corpus is GREEN, not red.** My brief predicted
red; measured, that is wrong for this change.

* `git diff <BASE> -- crates/wcore-protocol` → **empty**. No field added, removed, renamed or
  retyped; no `CapabilityId` variant; no `CONTRACT_MINOR` bump.
* `crates/wcore-browser/src/liveness.rs` is **not one of the 41 `SOURCE_INPUTS`** entries in
  `contract/spec.rs`, so `source_inputs_digest` cannot move either. Absence verified with a
  known-positive in the same file (`"browser"` → 11 hits, so the grep was alive; no
  `wcore-browser` path inside the `SOURCE_INPUTS` block).
* Measured, not inferred: `cargo test -p wcore-protocol --test desktop_contract_corpus` →
  **`15 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out`**.

**Expected counts are unchanged: 23 commands, 52 events, `CONTRACT_MINOR = 10`.** I did not run
`wcore-contract generate`, did not edit a pin, and did not touch a fenced artifact.

Note for the integrator: `SEAM-REQUESTS/27.md` **SR-27-2 is stale** — it asks for `CONTRACT_MINOR`
8 → 9 and the tree is already at 10. SR-27-1..3 as a bundle are **not required for `27-C2(b)`**;
the criterion was satisfiable, and was satisfied, by narrowing an existing boolean's *value*.
`SEAM-REQUESTS/false-advertising.md` SR-FA-1 (one corpus regeneration over the merged tree, plus a
Desktop re-pin in the same train) is still the live request, and is unrelated to my diff.

---

## 7. Gate results — per-crate isolated runs at the final SHA `73a9f997`

| Suite | Result |
|---|---|
| `wcore-browser --lib` (whole crate) | **91 passed, 0 failed, 0 ignored, 0 filtered out** |
| `wcore-browser --lib liveness` | **7 passed, 0 failed, 0 ignored, 84 filtered out** (was 5 at base) |
| `wcore-cua --lib` (whole crate) | **51 passed, 0 failed, 0 ignored, 0 filtered out** |
| `wcore-agent --test capability_liveness_narrowing` | **3 passed, 0 failed, 0 ignored, 0 filtered out** |
| `wcore-agent --test capability_advertising_test` | **6 passed, 0 failed** (identity layer unregressed) |
| `wcore-agent --test browser_config_hint_roundtrip` | **4 passed, 0 failed** (clause (a) guard unregressed) |
| `wcore-protocol --test desktop_contract_corpus` | **15 passed, 0 failed** |
| `wcore-cli --test plugin_discovery_e2e` | **2 passed, 0 failed** (real binary, two-run polarity differential) |
| `cargo clippy -p wcore-browser -p wcore-agent --all-targets` | **0** own-code warnings, **0** lines naming `liveness.rs` |
| `cargo check --workspace --all-targets` | **Finished**, clean |
| `cargo fmt --all -- --check` (Mac) | clean |

`cargo check` was run **workspace-wide with `--all-targets`**, never `-p`. The only warning is the
pre-existing `imap-proto v0.10.2` future-incompat notice, which is not mine and is present at base.

A clippy `collapsible_if` in my first commit was a real gate finding (CI enforces clippy clean) and
is fixed in `73a9f997`; every figure above is from the final SHA, none from the superseded commit.

---

## 8. Clause (c) — NOT mine, and I am not silently absorbing it

Out of scope, untouched, still open. Naming what it needs concretely, because "no baseline" is less
useful than it sounds — **in all three cases the mechanism exists in source and it is the live
measurement that is missing:**

1. **Downloads-root confinement.** `downloads_root` exists (`wcore-browser/src/tool.rs`, 20
   references, that one file). Needs a live drive that attempts a write **outside** the configured
   root and records the refusal, plus the both-direction control — an in-root download that
   **succeeds** — or the leg is unfalsifiable.
2. **The approval gate on a computer-use operation.** The mechanism exists:
   `wcore-cua/src/policy.rs` routes `require_approval_for_app` / `first_time_per_app_approval` to
   `Suspend`, and four `wcore-cua/tests/policy*` files cover the decision. What is missing is the
   **end-to-end** observation — a real CUA op producing an `ApprovalRequired` event on the protocol
   stream and honouring the resume. Note this needs a display, so `hetzner-dsm` **cannot** host it:
   the probe I just proved correctly refuses there. It needs `SeanD@seandesktop` or an Xvfb.
3. **Process count before/during/after plus one reaper interval.** Reaper unit tests exist
   (`supervisor.rs`). What is missing is a live count across a real session on a box where a
   sidecar can actually start — again not this headless host.

Two of the three legs are **blocked on a display-capable host**, which is worth surfacing because
it is not an execution shortfall.

---

## 9. Fence compliance

* `git diff <BASE> -- crates/wcore-cli/src/lib.rs crates/wcore-cli/src/main.rs` → **empty**. Both
  shared files untouched.
* `git diff <BASE> -- crates/wcore-protocol` → **empty**.
* `$BASE` captured once at start and quoted; never diffed against a branch name.
* Full change set vs `d622cb09`: **3 files, +350 −17** — one source file
  (`crates/wcore-browser/src/liveness.rs`) and two `.planning/` evidence files.
* No `git add -A`, no `checkout`, `reset`, `stash`, `rebase` or `clean`. No `Co-Authored-By`.
* Did NOT run `wcore-contract generate`. Did NOT merge into `plan/f20-unified-audit-repair`. Did
  NOT open a PR, tag, release or close an issue. Pushed only `gh HEAD:lane/27-c2b-readiness`.
* No secret was printed, logged or committed. None was needed: the live runs used
  `--api-key test-key-unused` and stop before any provider call. Per §3b-ii I did not rely on the
  environment for any claim — the capability flags were read out of the product's own `ready` event.

### Two scratch harnesses, disclosed

`crates/wcore-browser/examples/drift_probe.rs` and `.../refusal.rs` were written on hetzner to
print the supervisor's config beside the probe's verdict, and to print the production refusal text.
**Neither is committed**; both were deleted and `git status --short` on the hetzner worktree shows
no tracked change beyond my three files. They are harnesses, not product code — but the arm-2/arm-3
comparison is only readable because they printed both values side by side, so they are named here
rather than left implicit.

---

## 10. My own instrument defect, found and repaired mid-run (§6b-ii)

My hetzner poll was written `test -f LOG && grep -c WLDONE LOG || echo MISSING`. Two separate
failures, in opposite directions, both silent:

1. An earlier form, `grep -c ... || echo 0`, reported `done=0` for **five minutes against a log
   file that did not exist** — a backgrounded `cat` had lost stdin and uploaded a **0-byte**
   script, so nothing ever ran. "Not finished" and "never started" were indistinguishable.
2. The replacement then reported `MISSING` on a log that existed and was growing, because
   `grep -c` **exits 1 on zero matches**, so `||` fired on a successful count of zero.

Repaired in-run to an explicit three-state check (`NOFILE` / `done=N bytes=N`) rather than written
up and left, and the byte count is what exposed both. This is §3b-i in my own harness: a poll that
reports absence is trivially satisfied by a broken instrument, and mine was broken twice.

---

## 11. What I did NOT do

* Did **not** implement SR-27-1..3. `CapabilityId::{Browser, ComputerUse, Web}`, the activation
  ladder and the reason-code mapping remain unimplemented — and are **not required** for this
  criterion, which is the substantive finding, not an excuse.
* Did **not** address clause **(c)**. See §8.
* Did **not** narrow `with_computer_use_advertised(true)`, and §5 argues it should stay `true`.
* Did **not** change the first-use refusal text; §4 explains why I judged it already adequate
  rather than assuming my brief's framing.
* Did **not** edit the ledger or `CRITERIA-STATUS.md`. `27-C2` should move to **MET for (b)**;
  that grade change is the orchestrator's to make, and §1 is the evidence for it.
* Did **not** run a full-workspace **test** run, by policy. `cargo check --workspace
  --all-targets` was run, as required.
* Did **not** use the §0 Darwin exception. Nothing here is Darwin-only and hetzner proved all of it.

## 12. Cleanup

Hetzner worktree `/root/wayland-27c2b` and its `target/` were created by this lane and are removed
on completion. `/tmp/27c2b*` paths are lane-unique per §6a-ii — no glob of mine could read another
lane's file. No other lane's worktree was touched.
