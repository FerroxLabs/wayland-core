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
