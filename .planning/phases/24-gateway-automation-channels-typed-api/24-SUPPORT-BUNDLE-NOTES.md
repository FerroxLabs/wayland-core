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

---

## T+75 — built, and live-driven on hetzner. Redaction PROVEN with four arms.

Built at `4e4090b9` on `hetzner-dsm:/root/wayland-support-bundle` (worktree `hz/support-bundle`).
`cargo build -p wcore-cli --bin wayland-core` → **rc=0**. fmt clean on the Mac (rc=0, 0 diff
lines — re-run **unpiped** after the first attempt read `tail`'s status instead of cargo's,
which is §3.2's pipe-steals-exit-status trap, caught in-lane).

### The coordinator's warning applied: `0` is my success value

A hit count of `0` means "redacted" — and a missing bundle, an unwritable path, an errored
grep, a mangled variable and a bundle that collected nothing ALL produce `0`. So the sweep
alone proves nothing. Four arms, each with its own exit code, in
`evidence/24-support-bundle/sb-proof.sh`:

| arm | guards against | measured |
|---|---|---|
| **3** — secret really IS in the input (exit 71) | redaction of something never present is free | config plant in **2** input files, env plant in **1** |
| **1** — bundle exists, non-empty (exit 72) | absent-because-nothing-generated | **5** files; **1861** bytes by THREE independent methods (`cat\|wc -c`, `du -sb`, `stat -c%s` summed) — all three agree, so the `wc -c`-says-0-for-72-bytes defect is excluded |
| **4** — bundle actually COLLECTED (exit 74) | a bundle that collects nothing passes every redaction test | non-secret marker `COLLECTION-MARKER-8fa31c` present **1×** in `recent-log.txt`; `api_key` NAME present **1×** in `config-keys.txt` |
| **2** — sweep can FIND this needle (exit 73) | dead instrument | known-positive over a control dir **rc=1 (found)** for BOTH plants, in the same run as the real sweep |

**Result: real sweep hits = 0 for both plants, rc=0 CLEAN, with the known-positive returning 1
in the same invocation.** `scripts/f24-secret-sweep.sh --selftest` → **5/5 PASS** first.

**And the redaction is positively evidenced, not just an absence:** `manifest.redactions = 2`
— a NON-ZERO count. The scrubber demonstrably replaced two values rather than finding nothing
to do. The scrubbed log reads:

```
INFO  gateway starting COLLECTION-MARKER-8fa31c      <- marker survived
ERROR auth rejected for key [REDACTED] (401)         <- config-file secret
ERROR upstream refused bearer [REDACTED]             <- environment secret
```

The config-file secret is the one the pre-fix redactor could not have learned.

### Degraded cases — OBSERVED (`sb-degraded.sh`, `sb-d3.sh`)

- **D1 stale status file + dead gateway** — the trap the fix exists for. Planted a
  `gateway-status.json` claiming `running / uptime 98765 / turns_in_flight 7` with a dead pid
  999999. Bundle reports `running=false`, `state=uninstalled`; **the stale `98765` and
  `turns_in_flight: 7` do NOT appear in the bundle**, with a liveness control proving the same
  grep DOES find both in the planted file. 4/4 PASS.
- **D2 empty home, no config, no log** — bundle still produced (rc=0); **3** absent sources
  NAMED in the manifest; no empty log member invented. 3/3 PASS.
- **D4 non-empty out dir** — refused rc=1 with the reason named; pre-existing file untouched;
  no members written. 3/3 PASS.
- **D3 unwritable out path** — see the instrument defect below. Now 4/4 (D3a) + 6/6 (D3b).

### TWO instrument defects in MY OWN harness, both repaired in-lane (§6b-ii)

**(i) D3 measured root's `CAP_DAC_OVERRIDE`, not the product.** `chmod 500` + running as root
means the write SUCCEEDS, so my first D3 reported three product failures that do not exist. I
did not note-and-move-on; I repaired it, and added a **control that proves the bypass**: a
plain `touch` also succeeds in the same 500 dir as root. Replaced with two causes root cannot
bypass — **D3a** parent-is-a-regular-file (ENOTDIR): `rc=1`, *"Not a directory (os error 20)"*,
no success banner, blocking file untouched. **D3b** genuinely unprivileged user (uid 65534).

**(ii) D3b's first run SELF-PASSED, and this is the sharper one.** It returned `rc=126`,
`env: '/root/.../wayland-core': Permission denied` — `nobody` could not **execute** the binary
under `/root`, so **the product never ran at all**. All three assertions passed anyway:
`rc != 0` ✓, "names permission denied" ✓ (that was `env`'s own message, not the product's),
"no partial bundle" ✓ (nothing ran). A textbook pass-for-the-wrong-reason. Repaired with two
controls: **CONTROL A** proves `nobody` can now execute the binary (`--version` rc=0), and a
**SANITY** arm proves the same unprivileged user + same verb **succeeds on a writable path**
(rc=0) — without which `rc != 0` could just mean the verb never works for that user. Re-run:
`rc=1`, *"Permission denied (os error 13)"* **from the product**. 6/6 PASS.

Both defects are of the class this programme keeps hitting, found in the instrument built to
hunt it. Repaired, not documented-and-left.

### Still to do

- Binary-level differential proving the PRE-FIX build leaks the config secret (the third
  assertion, at the binary rather than the unit level).
- `cargo test -p wcore-gateway`, `-p wcore-cli` with explicit counts read back; clippy.

