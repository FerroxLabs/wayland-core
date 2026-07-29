# RELEASE-TRUST-ROOT — working notes

Lane `release-trust-root`. Branch `lane/release-trust-root`. Base (captured once, quoted
everywhere): `63481a2d2931e888dce7214c6968b95e26f9e10d`.

Seam requests addressed: **SR-29-9**, **SR-29-11**. Both are the disposition of open HIGH
**F29-03-01** (`self-update` installs nothing).

Append-and-recommit after every measurement. No partial credit for uncommitted reasoning.

---

## Plan

1. Review the two inherited uncommitted edits adversarially. Commit or repair.
2. Prove the substitution compiles + passes on `hetzner-dsm` (never the Mac).
3. Wire `.github/workflows/release.yml` per SR-29-9 (manifest-build + manifest-sign,
   seed on stdin only, `-release-manifest.json` suffix, monotonic `--sequence`).
4. End-to-end proof with a **throwaway** run-time key through the real `ReleaseVerifier`:
   one ACCEPT control plus every refusal (rollback, stale/frozen, revoked, retired key,
   wrong role).
5. Write `.planning/RELEASE-TRUST-ROOT.md`, commit, push.

Fences honoured: no `ci.yml` (owned by lane `ci-macos-budget`), no merge/PR/tag/release,
no `wcore-contract generate`, no real release run triggered.

---

## Measurement log

### M1 — worktree identity (2026-07-29)

```
/usr/bin/git rev-parse --show-toplevel
  -> /Users/seandonahoe/dev/waylandcore-frontier-worktrees/lane-release-trust-root
/usr/bin/git rev-parse --abbrev-ref HEAD  -> lane/release-trust-root
/usr/bin/git merge-base HEAD plan/f20-unified-audit-repair -> 63481a2d2931e888...
```

Dirty at entry, exactly two files, both inherited:
`crates/wcore-cli/src/update_trust.rs`, `crates/wcore-cli/tests/self_update_trust.rs`.

### M2 — the bundled key is structurally a real Ed25519 public key

`public_key_base64 = ycwkW1xZnCxruh59zJnQiuoN5xuXYkMurhquhHMBXXY=` decodes (strict base64)
to **32 bytes**, `all_zero = false`. So it clears both placeholder refusals in
`ReleaseVerifier::with_trust_root` (empty key set; all-zeros identity point).

### M3 — the "only bundle release_acceptance" reasoning CHECKS OUT against the code

`update_trust.rs:588` — `ReleaseVerifier::resolve` refuses any key whose `role !=
RELEASE_MANIFEST_ROLE`, and `RELEASE_MANIFEST_ROLE = "release_acceptance"`
(`update_trust.rs:83`). `resolve` is the ONLY path from a manifest to a `VerifyingKey`
(`verify_manifest_json:567`). So `packaging`, `deployment_preparation` and
`rollback_rehearsal` keys in the bundled root could never authorise an install; bundling
them would be pure trust surface. **Reasoning sustained — bundling one key is correct.**

Consequence to state, not hide: the `RoleMismatch` refusal is now unreachable *via
`bundled()`*. It stays reachable via `with_trust_root_json`, and the corpus must keep
proving it there. Recorded as a deliberate consequence, not an accident.

### M4 — the test edit is PARTLY over-weakened; one deleted assertion must come back

`UpdateTrustError::PlaceholderTrustRoot`'s `#[error(...)]` string (`update_trust.rs:122-126`)
names `RELEASE_TRUST_ROOT_JSON` **unconditionally** — it is a static format string, not
derived from which root was passed. So the deleted assertion
`message.contains("RELEASE_TRUST_ROOT_JSON")` **still passes against an injected empty
root**. Deleting it removed a live guard for no reason. **Restore it.**

The rest of the split survives review: reverting the constant to `"keys":[]` tomorrow makes
`bundled()` return `Err`, which panics `the_bundled_trust_root_is_real_...`'s `.expect()`
AND fails its `keys.len() == 1` assertion. The placeholder path stays genuinely gated.

### M5 — stale module doc

`tests/self_update_trust.rs:16-17` still asserts in prose "The bundled production trust root
is EMPTY on purpose and this file proves it is refused." False as of the substitution. Must
be corrected or the file documents a guard it no longer has.

<!-- appended below as work proceeds -->

### M6 — the inherited edit was INCOMPLETE: an inline unit test would have failed the build

`crates/wcore-cli/src/update_trust.rs:1089` carried an INLINE unit test
`the_bundled_constant_is_the_empty_placeholder_and_is_refused`, asserting
`RELEASE_TRUST_ROOT_JSON.contains("\"keys\":[]")` and that `bundled()` returns
`Err(PlaceholderTrustRoot)`. The inherited edit fixed only the integration test file and never
touched this one. **It would have failed `cargo test -p wcore-cli --lib` immediately.** Found by
grepping for the constant across `crates/` rather than trusting the handover.

Rewritten, not deleted, on the same principle as the integration test: refusal proved against
injected empty + all-zeros roots, with the bundled root as the ACCEPTED control. A second inline
test now proves the role-scoping claim live — an injected `packaging` key `RoleMismatch`es while
the same key under `release_acceptance` resolves, so the refusal is about the role, not the key.

### M7 — hetzner build + test, commit `bb14c976`, worktree `/root/wayland-rtr`

```
cargo test -p wcore-cli --lib update_trust
  test result: ok. 6 passed; 0 failed; 0 ignored; 0 measured; 1840 filtered out
cargo test -p wcore-cli --test self_update_trust
  test result: ok. 22 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out
```

`0 ignored` / `0 filtered out` read back from an UNPROXIED `/root/.cargo/bin/cargo` (the rtk
proxy strips exactly those two fields). The 22-test suite is `0 filtered out` — no name filter,
so flavour (c) vacuity is excluded. The 6-test run IS filtered (`update_trust`), and both new
tests appear BY NAME in the output, so the filter is not empty.

### M8 — the guard can fail (regression drill)

Constant reverted to `"keys":[]` in the hetzner worktree, suites re-run, file restored
(`git status --porcelain` = 0 lines, HEAD `bb14c976`):

| suite | result on regression |
|---|---|
| `--lib update_trust` | **FAILED. 4 passed; 2 failed** |
| `--test self_update_trust` | **FAILED. 21 passed; 1 failed** |

Three distinct tests went red with three distinct messages:
`the bundled root regressed to the empty placeholder`;
`exactly one key belongs here (left: 0, right: 1)`;
`the bundled trust root must now construct: PlaceholderTrustRoot("it holds no keys")`.

**And `a_placeholder_trust_root_is_refused_however_it_arrives` PASSED under the regression** —
which is the point of the split: the refusal behaviour is independent of the constant, and the
constant carries its own separate guard. The inherited edit's shape is vindicated; only its
completeness was wrong (M6) and one assertion was over-deleted (M4).

### M9 — clippy clean; two full-suite failures, both proved NOT mine

`cargo clippy -p wcore-cli --all-targets` at `bb14c976`: **zero warnings.** The only `warning:`
line in the log is cargo's pre-existing `imap-proto v0.10.2` future-incompat NOTE, not a lint.

`cargo test -p wcore-cli` (full) showed 34 result lines, 32 `ok`, two `FAILED`:

| failure | disposition | evidence |
|---|---|---|
| `always_fails` (`--lib`, 0 passed / 1 failed) | **not a test — a FIXTURE.** `crates/wcore-cli/src/plugin/scaffold.rs:274` writes the literal `#[test]\nfn always_fails() { panic!("deliberate"); }` into a scaffolded plugin template. A scaffolded crate got picked up as a workspace member during the run. Cannot be reached by a change to `update_trust.rs`. | base full run, below |
| `import_is_idempotent_without_overwrite` (`migrate_hermes`, 6/7) | **full-suite contention artifact.** | isolated reruns |

Isolated reruns of `migrate_hermes`, unproxied cargo, same targets:

```
base 63481a2d, alone:      test result: ok. 7 passed; 0 failed; 0 ignored; 0 filtered out
mine bb14c976, alone:      test result: ok. 7 passed; 0 failed; 0 ignored; 0 filtered out
mine bb14c976, full suite: test result: FAILED. 6 passed; 1 failed
```

Passes alone at BOTH commits, fails only under full-suite load — the contention class the lane
brief documents. `git status --porcelain` in the hetzner worktree is **0 lines** after the run,
so no tracked file was polluted.

Reported as measured, not waved away: neither is green, and neither is caused by this lane.

### M10 — the end-to-end drill, and it can fail

`.github/scripts/release-manifest-drill.sh` on hetzner, throwaway keys from
`wayland-release trust-root-init` into a temp dir deleted on exit:

```
test result: ok. 10 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out
NEGATIVE CONTROL (pristine control swapped for the packaging-signed manifest):
test result: FAILED. 1 passed; 9 failed; 0 ignored; 0 measured; 0 filtered out
```

Nine of ten go red on a broken corpus. The tenth only checks file presence and correctly
survives. In-script known-positive/known-negative pair before the Rust side runs: the live root
verifies the control (`MANIFEST VERIFIED`), the retired root refuses the same bytes
(`key is retired: release-acceptance-key`).

### M11 — sweep instrument was DEAD on first attempt, repaired in-lane

First credential sweep returned 0 hits on every pattern — and the known-positive returned
nothing, which is what exposed it. Cause: **zsh does not word-split an unquoted `$FILES`**, so
the whole list arrived as one non-existent filename. Every sweep passed for free.

Repaired (bash array built with `while read`, not `mapfile` — bash 3 on macOS has no
`mapfile`). Post-repair, instrument proved alive (known-positive = 2 occurrences; decoy = 1):
SWEEP1/2/3 all **0**. Hit count 0.

### M12 — final state

`always_fails` confirmed FAILING at BASE `63481a2d` in the same full-suite invocation, so it is
pre-existing and proved, not assumed.

Fence vs `63481a2d`: `lib.rs` + `main.rs` = **0 lines**; `ci.yml` = **0 lines**.
