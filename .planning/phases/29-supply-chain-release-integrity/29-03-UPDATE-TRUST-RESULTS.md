# 29-03 — Update trust path: F29-02 graded, clause by clause

**Commit everything below was measured at:** `3658c4281fe228af1f700a56a42e662d3d6a9c7c`
**Lane branch:** `lane/29-03`, off `plan/f20-unified-audit-repair` at `6df10dab`
**Authoritative platform:** `hetzner-dsm`, Linux x86_64, `/root/wayland-29-03`
**Evidence:** `evidence/29-03/` — every row below names a captured artifact.

---

## The one-paragraph answer

The updater compared versions for **string equality only**, so an offer that was
merely *different* took the install path — including a lower one. That is closed:
there is now an ordered SemVer comparison inside a pure, extracted `UpdateDecision`,
and **the refusal was observed end to end through the shipped binary against the
real public GitHub API, with no update-source redirect** (`live-downgrade.txt`).
F29-CEN-11 moves from **SOURCE-ONLY to MEASURED and CLOSED**. Alongside it: a
bundled release trust root that ships EMPTY and refuses to construct, persisted
freeze protection, a maximum-manifest-age rule, revocation enforcement, a
four-step key-rotation and compromise drill that was RUN, and a binding from the
downloaded archive to the digest the signed manifest vouches for. What is **not**
closed: the manifest ↔ certified-binary join (F29-CEN-10, Phase 28's R28-A), the
runtime plugin trust root (Phase 25), and every leg that needs Sean's real key or
a real published release. Those are named individually, not absorbed.

---

## F29-02, clause by clause

Machine-checkable index: `evidence/29-03/CLAUSE-LEDGER.tsv`.

| Clause | Grade | What settled it | Platform | Artifact |
|---|---|---|---|---|
| **Signed manifests** | **PARTIAL** | The shipped verifier parses, re-derives the body digest, resolves the key id in an externally supplied trust root, enforces role and validity window, and checks an Ed25519 signature over a domain-separated message. Proved against keys generated at run time. **Missing:** the bundled root is empty, so no *real* release manifest has ever been verified. | linux-x86_64 | `rotation-drill.txt` |
| **Source and artifact identity** | **PARTIAL** | Manifest ↔ archive is now bound: `check_archive` compares the downloaded file's SHA-256 **and byte length** to the artifact the signed manifest names, immediately before `verify_provenance` and ANDed with it. **Missing:** manifest ↔ certified binary — `certification` is still `Evidence::Unavailable`, which is 29-01's R28-A on Phase 28. | linux-x86_64 | `source-artifact-identity.txt` |
| **Rollback protection** | **MET** | Live, through the shipped binary, against the real API, no redirect: check-only prints `REFUSED: the offered release v0.12.25 is OLDER than the running v0.99.0`, exit 0; the install path refuses with exit **1** and the binary did **not** swap itself. Mutation M01 (order flip) is caught. | linux-x86_64 | `live-downgrade.txt` |
| **Freeze protection** | **PARTIAL** | A sequence at or below the persisted high-water mark is refused as stale (tested at 1, 19 and **20 against a mark of 20**, with 21 as the accepted control); an over-age manifest is refused **on a first run**, when no mark exists. Mutations M03 and M04 caught by exactly those tests. **Missing:** no real manifest carries a real sequence yet. | linux-x86_64 | `mutation-drill.txt` |
| **Revocation** | **PARTIAL** | A revoked offered version is refused with the reason surfaced verbatim; a revoked **running** version is reported prominently on a check-only run, with an unrevoked control proving the reporter does not always shout. Mutations M05 and M06 caught. **Missing:** no real revocation list exists to enforce. | linux-x86_64 | `mutation-drill.txt` |
| **Key rotation** | **MET** | The four-step drill was **RUN**, not argued, through the real `wayland-release` binary with keys generated at run time: accepted under key A → key B added and key A retired → the **same unchanged manifest** refused (`key is retired: release-acceptance-key`) → a new manifest under key B accepted. Two acceptances, two refusals. | linux-x86_64 | `rotation-drill.txt` |
| **Trust roots — plugins and backends** | **NOT-PROVABLE-HERE** | Release-side root proved and fail-closed. The **runtime install-time** root, `plugin/index.rs`'s `INDEX_PUBKEY_HEX`, is still the all-zeros placeholder needing Sean's key, and its lifecycle is Phase 25's. Filed as **SR-29-8**. Both halves named; the release half alone is not this clause. | linux-x86_64 | `plugin-backend-trust-root-gap.txt` |

---

## Live legs

Index: `evidence/29-03/LIVE-LEDGER.tsv`.

| Leg | Verdict | Exact invocation | Observable outcome |
|---|---|---|---|
| Real check-only | **PASS** | `env -u GH_TOKEN -u GITHUB_TOKEN HOME=<tmp> WAYLAND_HOME=<tmp> ./target/release/wayland-core self-update --check-only` | `current: v0.12.25 / latest: v0.12.25 / already up to date.`, exit 0, **nothing written** under `WAYLAND_HOME` |
| Trust-root placeholder refusal | **PASS** | same run | The user is told the bundled root is a placeholder and *which constant to replace*; clean exit |
| Downgrade, end to end | **PASS** | same binary rebuilt at version `0.99.0`; **no redirect** — the update source stays the pinned real repo | check-only refuses (exit 0); install refuses (exit **1**); `--version` afterwards still `0.99.0`; no freeze state written |
| Provenance fail-closed | **PASS** | `cargo nextest run -p wcore-cli -E 'test(verify_provenance_fails_closed_without_gh)'` | 1 passed. `gh` *is* present on this host, so absence is exercised by injecting a nonexistent program name |
| Rotation and compromise drill | **PASS** | `bash rotation-drill.sh` against `./target/debug/wayland-release` | 4 rows, 2 ACCEPTED, 2 REFUSED |
| Mutation drill | **PASS** | 19 mutations, applied and reverted one at a time | 18 caught, 1 explained; revert control green |
| **macOS** | **NOT RUN** | — | No macOS binary exists for this commit; CI run 30323212984 for SHA `3658c428` had not started its jobs. The Mac may not run Cargo. Reason and the exact completing command are in `macos-leg.txt`. **No Linux result is presented in its place.** |

**Windows: NOT ACHIEVED.** `seandesktop` is refusing SSH authentication on every account
tried. This is a Sean-reserved credential (`F29-LIMIT-06`). No Windows result is asserted.

---

## How I know these tests can fail

A corpus that has never gone red is a corpus with no measured power. Nineteen
mutations were applied to `update_trust.rs` one at a time, each reverted before
the next, with the corpus re-run every time (`mutation-drill.txt`).

- **18 of 19 CAUGHT.** Twelve were caught by *exactly* the one test claiming that
  behaviour, and no others.
- **Revert control green after the drill**, and green at baseline before it.
- **M07 SURVIVED, and that result is reported rather than tidied away.** Removing
  the digit guard in `parse_release_version` changes nothing, because
  `part.parse::<u64>()` already refuses every non-numeric component. The guard is
  defence in depth, not the load-bearing check. **M19** removed the load-bearing
  check too and was caught by exactly
  `an_unorderable_version_string_is_refused_rather_than_guessed` — so the
  behaviour *is* covered; only the redundant guard is not independently pinned.
- **The drill's own first run was defective and is kept in the record.** Its
  parser matched the wrong nextest line shape and reported `failed=0` for every
  mutation while the exit status said 100 — a measurement that could not be taken
  rendering as zero, the exact defect class this program keeps paying for. The
  parser now *raises* when a non-zero exit yields no parsed failures.

---

## Findings

| ID | Severity | Finding | Disposition |
|---|---|---|---|
| **F29-03-01** | **HIGH** | **`self-update` now installs nothing at all** until `RELEASE_TRUST_ROOT_JSON` holds real keys **and** releases publish a `*-release-manifest.json` asset. Measured: v0.12.25 publishes 7 assets, none a manifest. Unlike the pre-existing `gh`-missing refusal, this one is **not user-fixable** — only Sean can clear it. | **Intended fail-closed posture**, decided deliberately (see below). Filed as **SR-29-9** and **SR-29-11**. The refusal names the working npm route. |
| **F29-03-02** | **HIGH** | The runtime install-time plugin trust root (`INDEX_PUBKEY_HEX`) is still the all-zeros placeholder; no third-party plugin can be verified at install time. | Phase 25's. Measured, not modified. **SR-29-8**. F29-02's plugin clause is graded NOT-PROVABLE-HERE. |
| **F29-03-03** | **MEDIUM** | The manifest gate now refuses **before** the download, so `verify_provenance` is unreachable in production while manifests do not exist. The check itself is untouched (6 call sites, baseline 6) and re-enters the path the moment manifests ship. | Accepted. Refusing earlier is strictly safer than downloading an archive you have already declined. Recorded, not repaired. |
| **F29-03-04** | **MEDIUM** | `crates/wcore-cli/src/update_trust.rs` is **1142 lines**, over AGENTS.md's 1000-line cap. It was not split because this plan's own SURGICAL-DIFF gate whitelists exactly two `wcore-cli/src` files, and a third would have turned that scope-control gate red — a gate that protects five concurrent lanes. | **BACKLOG**, non-blocking per the severity policy. Remedy: add `update_trust_wire` to the whitelist and split the wire mirror out. **SR-29-12**. |
| **F29-03-05** | **MEDIUM** | `self-update --help` still advertises the removed `.sig` + pinned-ed25519 scheme (`main.rs:697`), re-measured live and now *more* wrong. | `main.rs` is the shared fence and this plan's FENCE GATE asserts it untouched. Exact replacement text filed as **SR-29-10**. |
| **F29-03-06** | **MEDIUM** | Three of this plan's own gates grep `self_update.rs` for symbols the same plan instructs the executor to extract into a sibling module; the plan's Task-3 remote gate uses a `--check` flag that does not exist. | Reported red as written, passing in scope-corrected form. **SR-29-12**. Not "fixed" by adding a reference purely to satisfy a grep. |
| **F29-03-07** | **LOW** | `body_sha256` is a digest over a **re-serialization** of the body, not over the bytes as they arrive (the document travels pretty-printed). Any independent verifier must therefore reproduce serde's field order byte for byte. | Accepted and pinned: the shipped side mirrors the body typed with `deny_unknown_fields`, and `a_harness_minted_manifest_verifies_under_the_shipped_verifier` fails in both directions if the two ever drift. |

---

## The one judgement call, and why it went the way it did

**Should a forward move with no signed manifest install, or refuse?**

Refusing means `self-update` is inert for every user until Sean acts, with no
user-side remedy. Allowing it means three named protections — freeze, revocation,
manifest identity — silently do not apply, which is exactly the "absent reads as
fine" that this codebase's own `Evidence` discipline exists to forbid.

**Decided: refuse.** Three things settled it.

1. The plan mandates it (`T-29-03-02`, and Task 2's done-criteria).
2. The path is **already** fail-closed for the majority of installs: `gh` is
   required and most users do not have it, with npm as the documented,
   provenance-backed alternative. This is not a new class of user experience.
3. The alternative degrades three protections silently, and a silent degradation
   in an update path is the failure mode this whole phase exists to close.

The refusal names the alternative in its own text, and the consequence is filed
as **F29-03-01 (HIGH)** with its exact substitution points rather than buried.

---

## Known unknowns — recorded, not resolved here

- **Ninety days is a policy number, not a measurement.** `DEFAULT_MAX_MANIFEST_AGE_SECS`
  is chosen to sit comfortably beyond any plausible release gap while still catching a
  mirror that has stopped moving. The project's release cadence is not established, so
  this should be revisited when it is.
- **A user who deliberately wants to roll back is now blocked**, and is served only by a
  documented manual reinstall. Whether that warrants a signed rollback allowance is open.
- **Whether the high-water mark should survive a profile switch** depends on the
  isolated-profiles work in another lane. Today it lives under
  `wayland_config_dir()/release-freeze-state.json`, so it follows `WAYLAND_HOME`.

---

## What this plan did not touch

Gate-checked zero-line `git status --porcelain` over: `crates/wcore-cli/src/plugin`,
`crates/wcore-pluginsrc`, `crates/wcore-exec-backend`, `crates/wcore-cli/src/lib.rs`,
`crates/wcore-cli/src/main.rs`, `crates/wcore-eval-scenarios/src/receipt.rs`,
`src/receipt_policy.rs`, `bin/wayland-receipt.rs`, `Cargo.toml`, `Cargo.lock`.
`.github/workflows/` untouched. `wcore-contract generate` not run. No dependency added.
No update-source redirect of any kind: `env::var(` stays at its measured **0** and the
pinned `RELEASES_REPO` constant at its measured **3**.
