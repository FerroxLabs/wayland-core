---
phase: 29-supply-chain-release-integrity
plan: "03"
subsystem: release-integrity
status: complete
termination_state: "2 — Complete with named real-key limits"
tags: [self-update, supply-chain, rollback, freeze, revocation, key-rotation, trust-root, ed25519]
requires:
  - "29-01 release manifest, trust root and state ledger (wcore-eval-scenarios::release_integrity)"
  - "29-02 SBOM and dependency policy (wcore-eval-scenarios::sbom)"
provides:
  - "an ordered, pure UpdateDecision extracted out of self_update::run"
  - "a bundled release trust root in the shipped updater that fails closed on a placeholder"
  - "persisted freeze protection under the WAYLAND_HOME-honouring config root"
  - "revocation enforcement, including a loud report of a revoked RUNNING version"
  - "a binding from the downloaded archive to the digest the signed manifest vouches for"
  - "release-manifest lifecycle fields (sequence, issued_at, revocations) and trust-root rotation ops"
affects:
  - "29-04 claims F29-02 closure and owns the tamper corpus"
  - "Phase 28 — R28-A still open; certification stays Evidence::Unavailable"
  - "Phase 25 — SR-29-8, the runtime plugin trust root"
tech-stack:
  added: []
  patterns: [fail-closed-default, contract-pinned-by-test, centralized-path-resolution, domain-separated-signatures]
key-files:
  created:
    - crates/wcore-cli/src/update_trust.rs
    - crates/wcore-cli/tests/self_update_trust.rs
    - .planning/phases/29-supply-chain-release-integrity/29-03-UPDATE-TRUST-RESULTS.md
    - .planning/phases/29-supply-chain-release-integrity/evidence/29-03/
  modified:
    - crates/wcore-cli/src/self_update.rs
    - crates/wcore-eval-scenarios/src/release_integrity.rs
    - crates/wcore-eval-scenarios/bin/wayland-release.rs
    - crates/wcore-eval-scenarios/tests/release_integrity_contract.rs
    - .planning/SEAM-REQUESTS/29.md
decisions:
  - "A forward move with no signed release manifest is REFUSED, not installed — fail closed"
  - "The bundled release trust root ships EMPTY and its constructor refuses it (the F-021 idiom)"
  - "An ordered comparison was implemented locally rather than by adding a semver dependency"
  - "Testability by EXTRACTION; no update-source redirect of any kind was added"
metrics:
  commits: 5
  mutations_applied: 19
  mutations_caught: 18
  tests_added: 30
  suite_delta: "2575 -> 2605, 0 residual failures at either end"
---

# Phase 29 Plan 03: Update Trust Path Summary

Closed the rollback gap in the shipped updater — an ordered, hostile-tested update
decision with a fail-closed release trust root, persisted freeze protection and
revocation — and proved the downgrade refusal end to end through the real binary
against the real public API with no update-source redirect.

**Everything below was measured at `3658c4281fe228af1f700a56a42e662d3d6a9c7c`,
on branch `lane/29-03`, off `plan/f20-unified-audit-repair` at `6df10dab`.**

**Termination state: 2 — Complete with named real-key limits.** That is the state
the plan named as expected and a full pass. Every leg that genuinely requires
Sean's key or a real published release is enumerated individually in
`evidence/29-03/REAL-KEY-LIMITS.tsv` and none of them is graded MET.

---

## Did an ordered version comparison already exist?

**No — measured before writing one.** A grep across the workspace for `semver`,
`version_compare` and any ordering comparison on version strings returned nothing
usable in `wcore-cli`. The plan forbids adding a dependency (`Cargo.toml` and
`Cargo.lock` are a cross-lane serialized seam that four other phases are executing
against), so `ReleaseVersion` is implemented locally: three numeric core
components, optional pre-release ordered below its own release per SemVer §11.4,
build metadata ignored for precedence per §10, and **anything unorderable refused
rather than guessed at** — because a guess here installs something.

---

## RED before GREEN

`88425def` is the recorded RED: the entire 17-behaviour corpus written before a
line of implementation. It does not compile, which is the point:

```
error[E0432]: unresolved import `wcore_cli::self_update::update_trust`
error[E0560]: struct `ReleaseManifestBodyV1` has no field named `sequence`
error[E0560]: struct `ReleaseManifestBodyV1` has no field named `issued_at`
error[E0560]: struct `ReleaseManifestBodyV1` has no field named `revocations`
error: could not compile `wcore-cli` (test "self_update_trust") due to 5 previous errors
rc=101
```

Full capture: `evidence/29-03/red-baseline.txt` (712 lines).

**Deviation, stated plainly:** the plan asks for RED per task. Because the plan
names ONE test file for both task corpora, an intermediate state where Task 1's
tests are green and Task 2's are red is a red suite, so both corpora were written
RED together and the per-task remote gates were run once, after both
implementation commits. Every behaviour has a genuine recorded RED; what was not
taken is a *per-task* green checkpoint. Recorded rather than implied — the same
correction 29-01 made about its own RED discipline.

---

## What the decision can return, and where it lives

`decide_update` is a pure function of the running version, the offered release,
the verified manifest and the persisted state, with an injected instant. No
network, no filesystem, no environment. It returns:

`Proceed` · `AlreadyUpToDate` · `RefusedDowngrade` · `RefusedUnorderableVersion` ·
`RefusedMissingManifest` · `RefusedManifestDoesNotDescribeOffer` ·
`RefusedStaleSequence` · `RefusedOverAgeManifest` · `RefusedRevokedVersion` ·
`RefusedRevokedArtifact`

Each carries a `message()` naming its cause — and for a downgrade, its
**direction**. `run` consults it exactly where it used to consult `==`. The
download, the `verify_provenance` call, the extraction and the atomic swap did
not move.

**Order is deliberate:** the version comparison runs FIRST, so an equal offer
stays the clean no-op it has always been and a downgrade is refused *without any
dependence on the manifest machinery working*. The rollback defence must not be
contingent on the freeze machinery.

**Module placement.** `update_trust.rs` is declared from `self_update.rs` via
`#[path = "update_trust.rs"] pub mod update_trust;`. That keeps the change off
`lib.rs` and `main.rs` — the two files every lane shares — while landing the file
at `src/update_trust.rs`, which is the path this plan's own SURGICAL-DIFF gate
whitelists. `self_update.rs` is 792 lines, under the cap.

---

## The persisted state

`FreezeState` at `wcore_config::config::wayland_config_dir()/release-freeze-state.json`
— the `WAYLAND_HOME`-honouring resolver, so a sandboxed or test run can never
touch a developer's real installation.

- A missing, unreadable, malformed **or foreign-schema** file is a **first run**,
  not an error. Refusing to update because a cache file got corrupted would be a
  denial of service — and a first run still enforces the maximum-age rule, which
  is the only freeze protection available before a mark exists.
- The mark **only ever rises** (`max`), so a rolled-back view cannot reset the
  memory that would have caught it.
- It advances **only after a successful install**, never on a decision. Observed:
  the check-only run left `WAYLAND_HOME` completely empty.
- **Hermetic by construction:** every test but one addresses the state by explicit
  path, so no test can pollute another. The single exception,
  `the_persisted_state_path_honours_wayland_home`, exists precisely to prove the
  production path resolves through the resolver, and is the only test in the file
  that touches the environment.

---

## The bundled trust root, and the one production step this plan cannot perform

```rust
pub const RELEASE_TRUST_ROOT_JSON: &str =
    r#"{"schema":"wayland.release.trust-root","schema_version":1,"keys":[]}"#;
```

It ships EMPTY on purpose and `ReleaseVerifier::bundled()` REFUSES it, naming the
constant to replace. An all-zeros key — the Ed25519 identity point, whose
signatures can be forged with no secret — is refused too, on the **injected** path
as well as the bundled one. This is `IndexVerifier::bundled()`'s F-021 discipline
copied in shape; `plugin/index.rs` itself is gate-checked untouched.

**Sean's step (SR-29-11):** replace that empty `keys` array with the real
FerroxLabs release trust root — PUBLIC halves only, `role: "release_acceptance"`,
`valid_from` at or before the first release it vouches for, as produced by
`wayland-release trust-root-init`. Nothing else in the file changes.

---

## The rotation and compromise drill — RUN, not argued

Through the real `wayland-release` binary, keys generated at run time into a
tempdir that dies with the drill. **No seed was printed, echoed, or passed as an
argument at any point.** Full transcript: `evidence/29-03/rotation-drill.txt`.

```
F29-ROTATE-01::ACCEPTED::key-a-active
    MANIFEST VERIFIED body_sha256=684acc7f7d75e8bc72fe5bff0a8d0a67457e6ca5622206e137efde8b618a795c
    KEY ADDED   key_id=release-acceptance-key-b role=release_acceptance
    KEY RETIRED key_id=release-acceptance-key   retired_at=1799999999
F29-ROTATE-02::REFUSED::key-a-retired
    wayland-release: key is retired: release-acceptance-key
F29-ROTATE-03::ACCEPTED::key-b-active
    MANIFEST VERIFIED body_sha256=160f7c70b8c3bc2de1c0b2c97ac8666574f5f8eaa5be5d77666cf3e0c413ed82
F29-ROTATE-04::REFUSED::placeholder-root
    shipped binary reported the placeholder refusal on 1 line(s)
```

Step 2 is the load-bearing one: **the manifest did not change**. Its signature is
still cryptographically valid. Retirement is enforced *at verification*, which is
what a compromise drill is for.

---

## The anti-drift guard, in both directions

Two independent verifiers are a feature only if a test pins the wire format they
meet at. `a_harness_minted_manifest_verifies_under_the_shipped_verifier` proves:

- **ACCEPT** — a manifest the harness mints and its own verifier accepts is
  accepted by the shipped verifier over the identical bytes, with matching
  `body_sha256`, sequence and version.
- **REJECT** — a body mutated after signing is rejected by the harness *and*
  independently by the shipped verifier (`BodyDigestMismatch`).
- **REJECT (smuggle)** — a body field swapped wholesale while keeping the signed
  digest and signature is refused. Without independent digest recomputation the
  updater would read sequence, age and revocations out of an **unauthenticated**
  body. Mutation M13 removed that recomputation and was caught by exactly this test.

Plus `a_signature_minted_for_another_domain_does_not_verify_as_a_manifest`:
same key, same body digest, a release-**state** domain separator — refused.

---

## Deviations

1. **Both corpora written RED together** rather than per task — see above. Reason:
   one test file, named by the plan.
2. **Deviation Rule 2 — added the archive↔manifest digest binding** (`3658c428`).
   The manifest carried artifact digests and nothing checked the downloaded bytes
   against them, so they were decorative: a correctly signed manifest would have
   sat beside whatever archive the source handed over. ANDed with
   `verify_provenance`, immediately before it. Not in the plan's task list; it is
   missing critical functionality for correctness and is in the plan's own threat
   surface.
3. **The plan's Task-3 remote gate uses `--check`, which does not exist**
   (`error: unexpected argument '--check' found`, rc=2). Run with `--check-only`
   and the discrepancy recorded rather than silently corrected. **SR-29-12**.
4. **Three local gates go RED as written** because they grep `self_update.rs` for
   symbols the same plan instructs the executor to extract into a sibling module.
   Both forms were run; the scope-corrected forms over the two-file artifact pair
   pass. **Not** "fixed" by adding a reference to `self_update.rs` purely to
   satisfy a grep — that would be gaming a gate. Both results in
   `evidence/29-03/local-gates.txt`, filed as **SR-29-12**.
5. **`update_trust.rs` is 1142 lines, over the 1000-line cap** (F29-03-04,
   MEDIUM → BACKLOG). Not split because the plan's SURGICAL-DIFF gate whitelists
   exactly two `wcore-cli/src` files and a third would have turned red a
   scope-control gate that protects five concurrent lanes.
6. **The plan's FENCE and SURGICAL-DIFF gates are vacuous as written** — they
   use `git status --porcelain`, which is empty once work is committed, and this
   plan commits per task. Re-run in the merge-base form against the SHA captured
   once at lane start: **0 fenced files touched between `6df10dab` and HEAD; 6
   `crates/` paths touched, every one whitelisted; 0 off-whitelist.** F29-03-08,
   MEDIUM.
7. **My own mutation drill was defective on its first run and the defect is kept
   in the record.** Its parser matched the wrong nextest line shape and reported
   `failed=0` for all 18 mutations while the exit status said 100 — a measurement
   that could not be taken rendering as zero. The parser now raises instead.

---

## Gates

**Local (Mac, source only):** 18 of 21 PASS. The 3 FAILs are the file-scoped greps
in deviation 4, each passing scope-corrected. `cargo fmt --all -- --check` clean.

**Authoritative (hetzner-dsm, Linux x86_64):**

| Gate | Result |
|---|---|
| `cargo clippy -p wcore-cli -p wcore-eval-scenarios --all-targets -- -D warnings` | **rc=0**, zero errors |
| `cargo nextest run -p wcore-cli -p wcore-eval-scenarios --no-fail-fast` | **2605 passed, 0 failed**, 14 skipped |
| the same at the lane base `6df10dab` | **2575 passed, 0 failed** |
| **delta** | **+30 tests, 0 residual failures at either end** |

The one retried test (`packaged_core_cancels_an_active_stream`) is the known
wall-clock-budgeted flake in `.planning/BACKLOG.md`; it passed on retry and
touches no file in this plan.

**Mutation drill:** 19 applied, **18 caught**, 12 of them by exactly the single
test claiming that behaviour. Revert control green. The one survivor (M07) is a
redundant defence-in-depth guard whose behaviour M19 proves *is* covered.

---

## Real-key limits — every one of them

Enumerated individually with substitution points in
`evidence/29-03/REAL-KEY-LIMITS.tsv`:

| ID | Kind | The one missing input |
|---|---|---|
| F29-LIMIT-01 | REAL-KEY | `RELEASE_TRUST_ROOT_JSON` — Sean's real release public keys |
| F29-LIMIT-02 | REAL-RELEASE | a published `*-release-manifest.json` asset (v0.12.25 has 7 assets, none a manifest) |
| F29-LIMIT-03 | REAL-KEY | `INDEX_PUBKEY_HEX` — the runtime plugin trust root (Phase 25) |
| F29-LIMIT-04 | REAL-RELEASE | the `gh attestation verify` ACCEPT path against the real Sigstore log |
| F29-LIMIT-05 | REAL-RELEASE | an end-to-end install of a real signed artifact |
| ~~F29-LIMIT-06~~ | ~~REAL-ACCOUNT~~ | **CLOSED 2026-07-28** — never a real-credential limit. `seandesktop` is reachable as `SeanD`; the Windows leg RAN and is MET. See `evidence/29-03-windows/RESULT.md`. |

**macOS: NOT RUN.** No macOS binary exists for this commit; CI run `30323212984`
for SHA `3658c428` had not started its jobs, and this lane may not run Cargo on
the Mac. `evidence/29-03/macos-leg.txt` carries the reason and the exact command
that completes it. **No Linux result is presented as a macOS result.**

**Windows: ACHIEVED 2026-07-28** by the windows-requeue lane. The blocker was false — the
account is `SeanD`, and `ssh -o BatchMode=yes SeanD@seandesktop` succeeds with no credential
supplied. The leg was run with the SAME construction as Linux (rebuilt at `0.99.0`, real
`api.github.com`, no update-source redirect, no credential) and matches Linux on every clause:
`--check-only` REFUSED at rc=0, INSTALL path REFUSED at rc=1, version after the refused install
still `0.99.0`. Evidence: `evidence/29-03-windows/RESULT.md`.

---

## Untouched — gate-checked

`crates/wcore-cli/src/plugin/`, `crates/wcore-pluginsrc/`,
`crates/wcore-exec-backend/`, `crates/wcore-cli/src/lib.rs`,
`crates/wcore-cli/src/main.rs`, `crates/wcore-eval-scenarios/src/receipt.rs`,
`src/receipt_policy.rs`, `bin/wayland-receipt.rs`, `Cargo.toml`, `Cargo.lock` —
all zero lines of `git status --porcelain`. `.github/workflows/` untouched.
`wcore-contract generate` not run. No dependency added. `INDEX_PUBKEY_HEX` still
present at 3+ sites in an unmodified `plugin/index.rs`.

No test was deleted, weakened, re-gated, `#[ignore]`d or `#[allow]`ed. No
`Co-Authored-By` trailer. **Marks no requirement complete — closure is 29-04's.**
