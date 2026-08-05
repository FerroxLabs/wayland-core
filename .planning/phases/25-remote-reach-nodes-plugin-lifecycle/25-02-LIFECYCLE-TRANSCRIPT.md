# 25-02 — Twelve-verb plugin lifecycle: live transcript

Every line below came from running the **shipped release binary**. The full unedited capture is
`evidence/25-02-lifecycle-linux.log` (475 lines); the machine-parseable verdicts are
`evidence/25-02-lifecycle-ledger.txt`; each verb additionally has its own capture file.

- **Host:** `hetzner-dsm` (Linux), 2026-07-27
- **Binary:** `/root/wayland-25/target/release/wayland-core` — `wayland-core 0.12.25`
- **Commit:** `c5552e69`
- **Driver:** `/root/f25-02-live.sh`, one continuous run against a throwaway `--install-root`

**The printed line is never the evidence.** After each verb the drive observes the resulting
state change independently — the install root's contents, `generations.json`, `approvals.json`,
a recursive `diff` against a pre-update snapshot, or the real engine's boot behaviour.

---

## 1. Verb ledger — all twelve

| # | Verb | Exit | State change observed after it |
|---|---|---|---|
| 1 | `plugin new` | 0 | `scaffolded/smoke-plugin/Cargo.toml` exists on disk |
| 2 | `plugin test` | 0 | the scaffold's own cargo suite ran and passed; separately proven to return **non-zero** for a deliberately red fixture |
| 3 | `plugin verify` | 0 | reported `COMPATIBLE` / `VERIFIED`; with a doctored `plugin_api_version` the same verb exits **non-zero** and prints `INCOMPATIBLE` |
| 4 | `plugin sign` | 0 | 64 bytes at `bin/wayland-plugin.sig` — beside the entry artifact, where the engine reads it |
| 5 | `plugin publish` | 0 | `market/plugins/lifecycle-demo/bundle.json` + `.claude-plugin/marketplace.json` written |
| 6 | `plugin install` | 0 | install dir populated; `generations.json` gained a retained generation and a live pointer |
| 7 | `plugin inspect` | 0 | reported `loads NO — the loader will refuse this plugin` while unapproved |
| 8 | `plugin approve` | 0 | `approvals.json` gained a record bound to digest `c564afd22804` |
| 9 | `plugin update` | 0 | live digest `c564afd22804 → a818b717bde6`; **two** generation directories on disk |
| 10 | `plugin rollback` | 0 | live digest back to `c564afd22804`; recursive diff vs the pre-update snapshot: **byte-identical** |
| 11 | `plugin recover` | 0 | install dir restored, interrupted staging dir swept; on a sound store it reports `nothing to repair` |
| 12 | `plugin remove` | 0 | install dir gone, approval record gone, a following `inspect` exits non-zero |

`plugin --help` on the release binary lists all twelve alongside `list`, `available` and the
three `marketplace` verbs.

---

## 2. The four negative cases

### 2.1 An unapproved plugin is REFUSED at load — by the real engine

Not the CLI's opinion of itself. The drive boots `wayland-core --json-stream` with
`WAYLAND_PLUGINS_DIR` pointed at the install root and reads what the loader does:

```
WARN on-disk plugin load failed (continuing)
  plugin=lifecycle-demo
  manifest=/root/f25-02-lab/home/plugins/lifecycle-demo@market/plugin.toml
  error=plugin approval required: lifecycle-demo: installed at digest c564afd22804
        but never approved — run `wayland-core plugin approve lifecycle-demo`
```

Verdict `F25-SC3-NEG-UNAPPROVED-REFUSED: PASS`. Capture: `evidence/25-02-neg-unapproved.txt`.

**And the positive half, which matters just as much.** After `plugin approve` (and after
installing the author's verifying key as a trust anchor — the exact step `sign --new-key`
prints), the SAME plugin loads:

```
INFO on-disk plugin loaded
  plugin=lifecycle-demo
  manifest=/root/f25-02-lab/home/plugins/lifecycle-demo@market/plugin.toml
```

Verdict `F25-SC3-NEG-APPROVED-LOADS: PASS`. Capture: `evidence/25-02-pos-approved.txt`.

Worth recording because it was found the hard way: the intermediate run showed the approval
gate opening and the **signature** gate then refusing, with
`no trusted keys available`. The two gates are independent and both real, which is the
correct design — but it means "approved" alone is not "trusted", and the transcript says so
rather than blurring them.

### 2.2 A tampered bundle refuses to install

One byte appended to the published entry artifact:

```
$ wayland-core plugin install lifecycle-demo@market --install-root ...
error: quarantine: bundle integrity check FAILED for lifecycle-demo:
  bundle.json records 4fa0ee587fbf... but the tree hashes to 305a0b31a636...
  — the published bytes were modified after publication
[exit=1]
STATE: refused (exit=1) and NOTHING landed in the install root
```

Verdict `F25-SC3-NEG-TAMPERED-REFUSED: PASS`. Capture: `evidence/25-02-neg-tampered.txt`.

### 2.3 Rollback restores byte-identical prior content

```
$ wayland-core plugin rollback lifecycle-demo --install-root ...
rolled back lifecycle-demo a818b717bde6 → c564afd22804 (v1.0.0)
  restored digest equals retained generation digest: c564afd228049eb9...
  the restored bytes are already approved and will load
[exit=0]

STATE: live digest is now c564afd22804 (pre-update was c564afd22804)
STATE: recursive diff against the pre-update snapshot: BYTE-IDENTICAL
```

The digest equality is asserted by the verb, and independently re-checked by a recursive
`diff -r` against a copy of the install directory taken **before** the update. Verdict
`F25-SC3-NEG-ROLLBACK-DIGEST-EQUAL: PASS`.

Also visible above: `inspect` after the update reported
`approval REFUSED — approved digest c564afd22804 does not match installed digest a818b717bde6`.
Consent did not travel across the change of bytes.

### 2.4 Recover repairs INDUCED half-written state

The damage is real, not a flag:

```
STATE (damage induced):
  removed the live install directory .../lifecycle-demo@market
  left an interrupted staging directory .generations/lifecycle-demo/.staging-deadbeef

$ wayland-core plugin recover --install-root ...
repaired 2 item(s):
  + removed interrupted staging directory .staging-deadbeef
  + lifecycle-demo: install directory did not match live generation c564afd22804 — restored it
quarantine    absent (recovery never promotes quarantined content)
  lifecycle-demo: approved at c564afd22804
[exit=0]

STATE: install dir restored = YES
STATE: staging dir swept    = YES
```

And the control that keeps this from being self-passing: run against a **sound** store the same
verb reports `plugin store is sound — nothing to repair`. A recover that always finds something
proves nothing. Verdict `F25-SC3-NEG-RECOVER-REPAIRED: PASS`.

---

## 3. Windows leg — `SeanDesktop`

- **Host:** `SeanD@seandesktop`, checkout `C:\ferrox-win` at commit `c5552e69`
- **Binary:** `C:\ferrox-win\target\release\wayland-core.exe`, built `--release --locked`, exit 0
- **Captures:** `evidence/25-02-lifecycle-windows.log` (369 lines),
  `evidence/25-02-lifecycle-windows-ledger.txt`

`plugin --help` lists all twelve verbs on the Windows binary.

| Verb | Windows | Note |
|---|---|---|
| new | **NOT-RUN** | `cargo-generate` is not installed on this host. The verb refused with the exact install command and left no directory behind — the correct behaviour, but the verb itself is unexercised here. |
| test | PASS | Proven against a deliberately red fixture: exit 1. |
| verify | PASS | And exits non-zero on an incompatible declared API version. |
| sign | PASS | Signature written beside the entry artifact. |
| publish | PASS | Bundle + catalog written. |
| install | PASS | Install dir + generation ledger written. |
| inspect | PASS | Reports `loads NO` while unapproved. |
| approve | PASS | Approval bound to digest `c3fde03c3523`. |
| update | PASS | Live digest changed; two generation directories retained. |
| rollback | PASS | **Per-file SHA-256 comparison against the pre-update snapshot: byte-identical.** |
| recover | PASS | Install dir restored, staging dir swept; sound store reports nothing to repair. |
| remove | PASS | Install dir and approval gone; a following `inspect` exits non-zero. |

**This is the leg most likely to have broken and did not.** Phase 20A's Windows defect classes
were overwhelmingly path and handle semantics — `MoveFileExW` with replace-existing failing on
an open destination, `DELETE`-bearing handles blocking directory operations. Generation swap
and rollback are exactly rename-and-replace over directories. The staged-then-swap
implementation (`generations::swap_in` retires the old tree before moving the new one in,
rather than renaming onto an existing directory) works on Windows unchanged, and rollback
produced byte-identical content verified file by file.

### The one Windows divergence, stated exactly

`F25-SC3-WIN-NEG-APPROVED-LOADS: PARTIAL`.

The **approval gate itself behaves identically on both platforms.** Unapproved:

```
WARN on-disk plugin load failed (continuing) plugin=lifecycle-demo
  error=plugin approval required: lifecycle-demo: installed at digest c3fde03c3523 ...
```

After `plugin approve`, that message is **gone** — the gate opened. The load then fails at the
next stage for a reason that has nothing to do with this plan:

```
WARN on-disk plugin load failed (continuing) plugin=lifecycle-demo
  error=SubprocessPluginRunner::load: subprocess spawn failed: %1 is not a valid Win32 application
```

The demo plugin's entry artifact is a Python script. Windows `CreateProcess` cannot execute a
`.py` file directly, so the fixture cannot be spawned there. **This is a limitation of the test
fixture, not of the product and not of the gate** — and I am recording it as PARTIAL rather than
PASS precisely because the completed-load line is what a PASS would require and it is not
present. Closing it needs a Windows-native entry artifact (a compiled `.exe`), which is fixture
work outside this plan's fence.

Linux carries the full positive proof: `INFO on-disk plugin loaded plugin=lifecycle-demo`.

---

## 4. Surfaces recorded as UNEXERCISED, with reasons

- **TUI.** No part of this lifecycle surfaces in the TUI, and the repo's PTY harness is
  `#![cfg(unix)]` regardless. No TUI observation is claimed on either platform.
- **Non-local marketplace sources.** The drive publishes to and installs from a local directory
  marketplace. Git and GitHub sources go through the same `resolve_source` path (unchanged by
  this plan) but were not exercised here.
- **WASM-runtime plugins.** The demo plugin is a subprocess-runtime plugin. `plugin sign` and
  `plugin verify` resolve a WASM `component_path` through the same code path, but no WASM
  plugin was driven end to end.

## 5. Environment notes for whoever repeats this

- `cargo-generate` was **not** installed on `hetzner-dsm` at the start of this work. Installing
  it is what turned `plugin new` from `NOT-RUN` into a real pass — and is also what surfaced the
  template defect in §6 of the SUMMARY. `wcore-plugin-api::template_smoke` had been silently
  skipping for the same reason.
- The demo plugin's entry artifact is a ~25-line Python script implementing the real
  `wcore-plugin-subprocess` JSON-Lines protocol (`init` / `list_tools` / `call_tool` /
  `shutdown`). That is what makes the positive approval leg a genuine load rather than a load
  that fails for an unrelated reason.
- The lab lives at `/root/f25-02-lab` and the evidence at `/root/f25-02-evidence` on
  `hetzner-dsm`; the hetzner worktree is `/root/wayland-25` on branch `hz/25`. All are named for
  this plan and safe to remove.
