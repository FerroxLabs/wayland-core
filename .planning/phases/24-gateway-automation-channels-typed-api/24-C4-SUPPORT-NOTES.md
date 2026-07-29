# 24-C4-SUPPORT — working notes (lane `24-c4-support`)

Base: `5140d640` (`lane/grade-24` HEAD, the verdict commit). Started 2026-07-29.

Mandate: close `F24-C4-H1` — `wcore_gateway::support_bundle` has zero production call
sites and no operator verb. Then take the residual `24-C2` / `24-C3` items the verdict
names, EXCEPT `24-C2` webhook+poll which Sean cut on 2026-07-29.

---

## M1 — F24-C4-H1 re-derived in MY tree (not inherited)

```
$ /usr/bin/grep -rn "support_bundle" crates/ --include="*.rs"
crates/wcore-gateway/tests/support_bundle_redaction.rs:20:use wcore_gateway::support_bundle::{
crates/wcore-gateway/tests/support_bundle_redaction.rs:269:///   cargo test -p wcore-gateway --test support_bundle_redaction -- --ignored live_bundle_canary
crates/wcore-gateway/src/lib.rs:19:pub mod support_bundle;
```

**Instrument alive control, same invocation, same shape** (LANE-BRIEF §3b-i):
```
$ /usr/bin/grep -rn "pub mod lifecycle" crates/ --include="*.rs"
crates/wcore-cli/src/plugin/mod.rs:23:pub mod lifecycle;
crates/wcore-gateway/src/lib.rs:16:pub mod lifecycle;
```
Two hits for a known-positive of the identical shape. The grep is not dead, and the
glob is quoted (zsh ate an unquoted `--include=*.rs` on my first attempt — that
attempt returned "no matches found" and would have confirmed the absence for free).

**CONFIRMED: the verdict's F24-C4-H1 is true in this tree.** One module declaration,
two references inside the module's own test file, zero production callers.

## M2 — the shape of the fix

`crates/wcore-cli/src/gateway.rs` already owns the `gateway` verb family
(`GatewayCmd`, `run()` at :246). `main.rs:745` already routes `TopCmd::Gateway`.
So a new `GatewayCmd::SupportBundle` variant needs **zero edits to either fenced
file** (`wcore-cli/src/lib.rs`, `wcore-cli/src/main.rs`). Fence exposure target: ZERO.

`support_bundle::collect(home, out_dir, &BundleSources, &Redactor)` is the entry.
`BundleSources { config, credentials, log, projections }`.

Open questions at this point (to be answered from source, not assumed):
- where the gateway log lives, if anywhere
- whether `channel-health.json` is actually published by a running gateway or is
  only a fixture invented by the test's `seed_home()`
- the config + credentials store paths for a profile

## M3 — the live proof that is required

Not "the verb compiles". The verdict's §8 asks for: a running gateway, a real bundle
produced by the shipped binary, and `live_bundle_canary` run against it with its three
env vars — which also converts that `#[ignore]`d opt-in gate into an exercised one.
A canary must be seeded into a real input so the positive control fires.

---

_(appended as measurements land; do not read this file as a conclusion)_

---

## M4 — the verb is built, and DRIVEN from the shipped binary (2026-07-29)

`gateway support-bundle --home <H> --out <D>`, added in `80d1bdf8`. Zero edits to
either fenced file.

- unit: `cargo test -p wcore-cli --lib gateway::` → **14 passed, 0 failed, 0 ignored,
  1836 filtered out** (5 of the 14 are new).
- existing suite: `cargo test -p wcore-gateway --test support_bundle_redaction` →
  **4 passed, 0 failed, 1 ignored**.
- LIVE: real gateway running (`pid 2999135`, `status` → `Running`), canary seeded into
  `config.toml`, `credentials.toml` and `gateway.log`, bundle produced BY THE SHIPPED
  BINARY: **8 members, known_secrets=2, redactions=1, absent_sources=0**.
- the `#[ignore]`d `live_bundle_canary` gate DRIVEN for the first time:
  **1 passed, 0 ignored, 4 filtered out**.
- that gate PROVED ABLE TO FAIL twice: leaky bundle → `A_RC=101`; unseeded input dir
  (positive-control leg) → `B_RC=101`.
- §3b-ii read-back: the bundle at the REAL home shows
  `ANTHROPIC_API_KEY [value elided: name marks a secret]` (1 hit) and the real key
  VALUE appears in **0** bundle files, while the same `/usr/bin/grep -F -l` finds it in
  `/root/.wayland/.env` (**1**). Differential negative, not a bare zero.
- the isolated-home run did NOT see `ANTHROPIC_API_KEY` (0 hits) — because
  `main.rs:951 load_wayland_env_file()` reads `$WAYLAND_HOME/.env` and my home had none.
  Not a defect; recorded because it looked like one for ten minutes.

Evidence: `24-C4-SUPPORT-evidence/`.

Still to do: mutation-prove the 5 new tests and the new production refusal path; then
the C2/C3 residuals.

---

## M5 — RECONCILIATION (2026-07-29, after the coordinator's message)

Integration is `plan/f20-unified-audit-repair` @ `4a872413`. My base `5140d640`
(`lane/grade-24`) IS an ancestor of it, so this is a clean fast-forward plus one real
content conflict: `lane/support-bundle` built the same verb while I was working.

**I am MERGING integration into my lane, not rebasing.** LANE-BRIEF §0 forbids
`git rebase` outright ("other lanes share the repository object store"). The
coordinator asked for a rebase; a merge reaches the same stated end — a branch that
merges cleanly into `4a872413` — moves only my own ref, and does not use the forbidden
verb. Flagging rather than silently routing around, per §0.

### Read both implementations before touching either. The landed one is better in
### four places and mine is better in two.

**Landed (`crates/wcore-cli/src/gateway/support.rs`, 298 lines) has that mine LACKS:**

1. **`Redactor::learn_secret_values_from_file(config)` and `(credentials)`** — a library
   method added for it. **This is a real gap in mine.** My redactor learned from the
   environment ONLY, so a credential living in `config.toml` was never learned and a log
   line quoting it would have shipped verbatim. Structural elision covers the config
   member itself; it does NOT cover the log. Landed wins outright.
2. **Ledger SUMMARY, not the raw journal.** Mine pushed `deliveries.jsonl` in as a
   projection; projections are copied whole and the journal is unbounded. Landed's
   counts tell a support engineer the same thing without the bloat. Landed wins.
3. **Does not create the state it reports on** — opens the ledger only if the journal
   already exists, because `DeliveryLedger::open` creates `home`.
4. **`Uninstalled` vs merely stopped** in the status member, via `is_registered`.

**Mine has that landed LACKS:**

1. **The production post-condition.** Landed has NO equivalent: nothing re-reads the
   finished bundle. Mine re-scans every known secret across every member over raw bytes
   and REFUSES with a non-zero exit. Must survive.
2. **`--home` is honoured for the config/credentials paths.** Landed calls
   `wcore_config::config::global_config_path()` unconditionally. That reads the process
   env `WAYLAND_HOME`; `ScopeArgs::home()` does NOT export the flag. So
   `support-bundle --home /X` (without the env set) collects key names from the AMBIENT
   config, not `/X`'s — and, worse, `learn_secret_values_from_file` then learns the
   WRONG secrets and misses `/X`'s. That is a redaction gap, not a cosmetic one.
   Provisionally `F24-C4-M1`; to be proved by driving `--home` alone.

**Also dropped by landed and worth restoring:** `channel-health.json` as a projection.
Criterion 4 names "redacted **health**/log/support evidence" and that file is the
gateway's health publication. Cheap; absent sources are named when missing.

**Overruling my own steer on one point:** landed's `--out` is OPTIONAL with a
timestamped default under the gateway home. I argued required-with-no-default. I am
keeping LANDED. The default lands inside the operator's own `$WAYLAND_HOME` — not
"somewhere nobody meant to send it from" — the path is printed, the read-before-send
warning is louder than mine, and requiring a flag adds friction at exactly the moment
something is broken. My argument was a preference, not a defect.

Plan: keep the landed module as the base, graft my post-condition + `--home` honouring
+ `channel-health.json`, port the mutation-proved tests that still apply, then re-prove
EVERYTHING on the reconciled code.
