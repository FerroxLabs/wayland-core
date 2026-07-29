# 24-SUPPORT-BUNDLE — working notes (lane `support-bundle`)

Started 2026-07-29. Base `861d1b1a`. Branch `lane/support-bundle`.
Live, append-only. Committed early per LANE-BRIEF §6b-i.

---

## T+0 — finding re-measured BEFORE building. `F24-C4-H1` HOLDS.

The brief instructed me to re-measure rather than inherit, since findings on this
programme have been wrong in both directions. I did. **It holds.**

### Measurement 1 — production call sites of `support_bundle`

```
/usr/bin/grep -rn "support_bundle" --include="*.rs" crates/
```

Result — **3 hits, all non-production**:

| file | line | kind |
|---|---|---|
| `crates/wcore-gateway/src/lib.rs` | 19 | `pub mod support_bundle;` — module declaration |
| `crates/wcore-gateway/tests/support_bundle_redaction.rs` | 20 | its own test importing it |
| `crates/wcore-gateway/tests/support_bundle_redaction.rs` | 269 | doc-comment naming the test cmd |

**Zero production call sites. Zero references outside the gateway crate.**

**Liveness control, same shape, run to prove the instrument is not dead:**
`/usr/bin/grep -rln "pub fn" crates/wcore-gateway/src` → **8 files**. Non-zero, so
grep is alive and searching the right tree. (§3b-i: an absence is free on a dead
instrument. This is the known-positive.)

### Measurement 2 — concept search for an operator surface (not one keyword)

Per §3b-i.3, vocabulary differs, so I searched the *concept* not the word:
`bundle|diagnostic|doctor` across `crates/wcore-cli/src/`.

Every hit is one of:
- TUI `/doctor` panel (`tui/commands/`, `tui/keybind.rs`, `tui/surfaces/palette.rs`) —
  an interactive in-TUI health panel, **not** a bundle, produces no artifact;
- `self_update.rs` "bundled release trust root" — unrelated sense of the word;
- `tui/auth.rs` "persist the bundle" — OAuth token bundle, unrelated sense.

**Liveness control for that search:** `/usr/bin/grep -rln "gateway" crates/wcore-cli/src/`
→ **7 files**. Non-zero, instrument alive.

**Conclusion: no CLI verb produces a support artifact. The grading lane's finding is
correct as written.** I am building, not grading.

### What DOES exist (so I do not rebuild it)

`crates/wcore-gateway/src/support_bundle.rs`, **543 lines**, already implements:

- `Redactor` — `learn()`, `learn_from_environment()`, `scrub() -> (String, usize)`
- `name_marks_secret(name)` — key-name heuristic
- `MIN_SCRUBBABLE_SECRET_LEN = 8`, `REDACTED = "[REDACTED]"`, `MAX_LOG_BYTES = 256KiB`
- `collect(...)` -> `BundleManifest` / `BundleSources`
- `bundle_files(root)`

So the *library* is real and tested. **What is missing is exactly and only the
operator surface.** That is a much smaller job than "build a support bundle", and it
matches the grading lane's half-session estimate.

---

## Plan (subject to revision as I measure)

1. Read `support_bundle.rs` in full — establish what `collect()` actually gathers and
   what it needs, especially whether it requires a *running* gateway.
2. Add a CLI verb wired to `collect()`. Fence: `wcore-cli` — additive contiguous only.
3. Redaction proof with a **planted secret positive control** (not "no secrets found",
   which is self-passing).
4. Degraded cases OBSERVED, not asserted: dead gateway, missing config, unwritable out path.
5. Only then does the verb appear in `--help`. (Nine advertised-but-dead instances on
   this programme; I will not make it ten.)

## Open questions at T+0

- Does `collect()` need a live gateway, or does it read from disk? Determines the whole
  shape of the degraded-case work.
- Is there an existing `scripts/f24-secret-sweep.sh`? Brief says yes with self-tests —
  prefer it over hand-rolling.

## Status

Nothing built yet. Nothing claimed. Next append after reading `support_bundle.rs`.

---

## T+20 — module read in full. Both open questions answered.

### Q1: does `collect()` need a running gateway? **NO.**

`collect(home, out_dir, &BundleSources, &Redactor)` is **entirely disk-based**. It takes
explicit source *paths* and reads them. It never contacts a process, never opens a socket.

**This is the single most important fact for the degraded-case work** and it is good news:
the diagnostic machinery cannot be taken down by the failure it is diagnosing.

### Q2: `scripts/f24-secret-sweep.sh` exists — 148 lines, **five** self-test assertions

Better than the brief promised. It already implements §6b-ii's three-assertion rule and adds
two more, and its own header records that it was *itself* found self-passing once (it counted
a control token it had planted in its own temp dir, so it reported CLEAN over two nonexistent
paths). It now refuses `rc=4` on unreadable paths and refuses an empty/short needle `rc=3`.
**I will use it rather than hand-rolling a sweep.**

### What the library already does well (do not rebuild)

Structural elision is the primary defence and it is the right design: config, credentials and
environment contribute **key NAMES only** (`key_names()` is a lexical scan that never
constructs a value). Exact-secret scrubbing is an explicitly-labelled backstop over free text.
`collect()` refuses a non-empty `out_dir`. Absent sources are **named**, not skipped.

### THE GAP I FOUND WHILE READING — and it is not the one I was sent for

Two defects, both of which would have shipped in the naive wiring:

**(a) The redactor never learns config-file secrets.** `learn_from_environment()` is the only
learn-in-bulk entry point, so it learns env values only. A `config.toml` holding
`api_key = "sk-..."` is structurally elided *from the config member* — but if that same key
appears in `gateway.log`, the scrubber has never heard of it and **the log member ships it**.
The backstop has a hole exactly where the backstop is the only defence.
→ Fix: add `Redactor::learn_secret_values_from_file()`, learning values whose KEY NAME marks a
secret. Values are held in memory and never written.

**(b) `home/gateway-status.json` is written by a running gateway and is NOT removed when that
process dies.** `read_live_projection()` (gateway.rs:402) exists precisely because of this —
it checks `process_is_alive(record.pid)` FIRST and refuses to return a projection for a dead
pid. A support bundle that copies `gateway-status.json` verbatim therefore ships a file
claiming `Running` with a pid that is gone — **and the bundle is created precisely when the
gateway has died**, so this is not an edge case, it is the modal case. It would hand a support
engineer a confident lie in the one file they open first.
→ Fix: derive the status member through `read_live_projection()` and carry an explicit
liveness verdict.

### Fence decision — ZERO exposure achievable

`Gateway` is **already registered** in `main.rs:745` and dispatched at `1437`, and
`wcore-gateway` is already a dep of `wcore-cli` (Cargo.toml:115). So a new verb added as a
`gateway` SUBCOMMAND needs **no edit to `main.rs` or `lib.rs` at all**.

`gateway.rs` is already 1633 lines (AGENTS.md wants <1000), so rather than grow it I add
`crates/wcore-cli/src/gateway/support.rs` as a child module — which also keeps `lib.rs` clean,
since a child module is declared in `gateway.rs`, not in `lib.rs`.

**Projected fence exposure: 0 lines in both fence files.**

### Surface-contract check (I must not run `wcore-contract generate`)

- `/usr/bin/grep -rln "gateway" crates/wcore-contract/` → **no files**. No contract fixture
  names a gateway verb, so an added subcommand regenerates nothing.
- The only verb-set test is `gateway.rs:1493 verb_names()`, and it asserts `run` is *present*.
  It is an inclusion check, not an exact-set check, so an **additive** verb cannot break it.
  (Read the assertions directly — `every_generated_unit_invokes_the_verb_this_module_implements`.)

### Design settled

`wayland-core gateway support-bundle [--out DIR] [--json] [--profile P] [--home H]`

Bundle members: `config-keys.txt`, `credential-keys.txt`, `environment-keys.txt`,
`recent-log.txt` (256KiB tail, scrubbed), `gateway-status.json` (**live-checked**, derived),
`ledger-summary.json` (delivery counts by state — bounded, unlike the raw `deliveries.jsonl`
which is unbounded and would bloat the bundle), `manifest.json`.

Degraded cases to OBSERVE, not assert: dead gateway, missing config, unwritable out path,
non-empty out dir.

**Not advertising before it works:** the verb reaches `--help` in the same commit that makes it
functional, and I live-prove it on hetzner before pushing. Nine advertised-but-dead instances
on this programme; I will not make it ten.

