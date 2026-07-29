---
lane: release-trust-root
seam-requests-addressed: [SR-29-9, SR-29-11]
verdict: BOTH LANDED — open HIGH F29-03-01 is closed on the code side, not on the published side
can-self-update-install: NOT YET — necessary half done, one Sean-reserved action remains
new-finding: F-RTR-01 (MEDIUM) — the inherited substitution left a live unit test asserting the placeholder; it would have failed the build
fence-exposure: ZERO — lib.rs and main.rs untouched vs 63481a2d; ci.yml untouched
status: complete
---

# Release trust root — SR-29-9 and SR-29-11

Base, captured once and quoted everywhere: **`63481a2d2931e888dce7214c6968b95e26f9e10d`**.
Branch `lane/release-trust-root`. All compilation, tests and clippy on `hetzner-dsm`
(`/root/wayland-rtr`); the Mac ran only `cargo fmt`.

---

## 1. Did the two inherited edits survive review?

**One survived intact, one was over-deleted, and both were INCOMPLETE.** The reasoning behind
them was right; the sweep behind them was not.

### Edit 1 — bundling only `release_acceptance`: **reasoning SUSTAINED**

Checked against the code rather than accepted. `ReleaseVerifier::resolve`
(`update_trust.rs:588`) refuses any key whose `role != RELEASE_MANIFEST_ROLE`, and
`RELEASE_MANIFEST_ROLE = "release_acceptance"` (`:83`). `resolve` is the **only** path from a
manifest to a `VerifyingKey` — `verify_manifest_json:567` is its sole caller. So `packaging`,
`deployment_preparation` and `rollback_rehearsal` keys in the bundled root could never
authorise an install. Bundling them would be trust surface with no function, and it would blunt
exactly the separation the four-state ledger exists to create.

Nothing required all four. The narrower root is correct.

**The consequence, stated rather than buried:** `RoleMismatch` is now unreachable *via
`bundled()`*. It stays reachable via `with_trust_root_json`, and two tests now prove it there —
one added inline, one in the CLI drill against a manifest signed with a real packaging key.

### Edit 2 — the test split: **shape vindicated, one assertion wrongly deleted**

The split is right, and the regression drill (§3) proves it: with the constant reverted,
`a_placeholder_trust_root_is_refused_however_it_arrives` **still passes** while three other
tests go red. That is precisely the intended division — the refusal behaviour is independent of
the constant, and the constant carries its own guard.

But `assert!(message.contains("RELEASE_TRUST_ROOT_JSON"))` was deleted unnecessarily.
`PlaceholderTrustRoot`'s `#[error(..)]` string (`update_trust.rs:122-126`) names the constant
**unconditionally** — it is a static format string, not derived from which root was passed — so
the assertion holds verbatim against an injected empty root. It has been **restored**. A guard
dropped because the code moved under it is a guard retired by accident, which is the failure
mode the brief warned about, and this was a small live instance of it.

The `!contains("seed")` assertion was also **replaced**, not kept: a word-search for `seed`
passes on any document that spells the field differently, and `WireTrustedKey` is
`deny_unknown_fields` so a `seed` field could not deserialize anyway. It is now a structural
check — the key object carries exactly the five wire fields, the key decodes to 32 non-zero
bytes — **with a live self-test**: the same extractor is run over a copy carrying a smuggled
`private_key_base64` and asserted to differ. Without that, the field-set assertion proves
nothing.

### F-RTR-01 (MEDIUM, new) — the substitution was incomplete and would not have compiled green

`crates/wcore-cli/src/update_trust.rs:1089` carried an **inline unit test**,
`the_bundled_constant_is_the_empty_placeholder_and_is_refused`, asserting
`RELEASE_TRUST_ROOT_JSON.contains("\"keys\":[]")`. The inherited edit changed only the
integration test file and never touched it. **`cargo test -p wcore-cli --lib` would have failed
immediately.**

Found by grepping the constant across `crates/` instead of trusting the handover — the same
discipline that turns "I changed the constant" into "I found every consumer of it". It has been
rewritten on the same principle as the integration test (refusal proved against injected roots,
bundled root as the accepted control), and a second inline test now proves the role-scoping
claim live rather than by assertion in a comment.

**Verdict on the handover: the reasoning was sound and the sweep was not.** Both edits needed
repair before they were committable.

---

## 2. What landed

| Change | File |
|---|---|
| Real FerroxLabs release trust root substituted, public halves only, one `release_acceptance` key | `crates/wcore-cli/src/update_trust.rs` |
| Placeholder refusal re-aimed at injected roots + bundled-root guard (inline and integration) | `update_trust.rs`, `tests/self_update_trust.rs` |
| `release.yml` builds, signs, VERIFIES and publishes a `*-release-manifest.json` asset | `.github/workflows/release.yml` |
| Sequence derivation, self-testing | `.github/scripts/release_manifest_sequence.py` |
| Bundled-trust-root extractor, self-testing | `.github/scripts/extract_bundled_trust_root.py` |
| End-to-end drill: real CLI mints → real shipped verifier consumes | `.github/scripts/release-manifest-drill.sh`, `crates/wcore-cli/tests/release_manifest_pipeline.rs` |

The bundled key is structurally real: `ycwkW1xZ…HMBXXY=` decodes under strict base64 to **32
bytes**, not all zero, clearing both placeholder refusals. `valid_from: 0` so it vouches for the
first release it signs; `retired_at: null`.

---

## 3. Gate results — every number from an unproxied tool

`rtk` re-renders `cargo` output and **strips `0 ignored` / `0 filtered out`**, the two fields
the anti-vacuity rule reads. Everything below came from `/root/.cargo/bin/cargo` and
`/usr/bin/grep` by absolute path.

At `bb14c976` on `hetzner-dsm`:

```
cargo test -p wcore-cli --lib update_trust
  test result: ok. 6 passed; 0 failed; 0 ignored; 0 measured; 1840 filtered out
cargo test -p wcore-cli --test self_update_trust
  test result: ok. 22 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out
cargo clippy -p wcore-cli --all-targets
  zero warnings (the only `warning:` line is cargo's pre-existing imap-proto future-incompat NOTE)
cargo fmt --all -- --check
  clean
```

The 22-test run is `0 filtered out`, so the empty-filter vacuity flavour is excluded. The 6-test
run **is** filtered, and both new tests appear by name in its output, so the filter is not empty.

### The guard can fail — regression drill

Constant reverted to `"keys":[]` in the hetzner worktree, suites re-run, file restored
(`git status --porcelain` = 0 lines, HEAD unchanged at `bb14c976`):

| suite | result |
|---|---|
| `--lib update_trust` | **FAILED. 4 passed; 2 failed** |
| `--test self_update_trust` | **FAILED. 21 passed; 1 failed** |

Three distinct tests, three distinct messages: `the bundled root regressed to the empty
placeholder`; `exactly one key belongs here (left: 0, right: 1)`; `the bundled trust root must
now construct: PlaceholderTrustRoot("it holds no keys")`.

### Two full-suite failures, both proved NOT this lane's

| failure | disposition |
|---|---|
| `always_fails` (`--lib`, 0 passed / 1 failed) | **Pre-existing — proved.** Present in the full `cargo test -p wcore-cli` run at BASE `63481a2d` as well. It is not a test but a FIXTURE: `plugin/scaffold.rs:274` writes the literal `#[test] fn always_fails() { panic!("deliberate"); }` into a scaffolded plugin template that gets picked up as a workspace member. |
| `import_is_idempotent_without_overwrite` (`migrate_hermes`, 6/7) | **Full-suite contention artifact.** `7 passed; 0 failed` **alone at BASE** and `7 passed; 0 failed` **alone at `bb14c976`**; fails only under full-suite load. |

Neither is green and neither is caused by this lane. Reported as measured.

---

## 4. The end-to-end proof — the deliverable

`.github/scripts/release-manifest-drill.sh` on `hetzner-dsm`. Keys generated at run time by
`wayland-release trust-root-init` into a temp dir the script deletes. **No real seed was read,
requested or obtainable by this lane.**

The corpus is minted with the **exact** `manifest-build` / `manifest-sign` pair `release.yml`
runs, seed on stdin, then driven through the **real shipped** `ReleaseVerifier` and
`decide_update`:

```
test result: ok. 10 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out
DRILL PASSED: 10 tests executed against a CLI-minted corpus,
and 9 failed when the corpus was deliberately broken.
```

| case | outcome |
|---|---|
| pristine newer release, CLI-minted | **PROCEEDS** — the control |
| genuine older release, correctly signed | `RefusedDowngrade` |
| correctly signed, 120 days old | `RefusedOverAgeManifest` |
| sequence at the high-water mark | `RefusedStaleSequence` |
| `--revoke-version` on the real command line | `RefusedRevokedVersion`, reason surfaced |
| key retired via real `trust-root-retire-key` | `RetiredKey` |
| signed by the real `packaging-key` | `RoleMismatch { bound: packaging, required: release_acceptance }` |
| body edited after signing, signature untouched | `BodyDigestMismatch` |
| archive digest binding | positive + altered-byte + unnamed-artifact, all three |

**Every refusal re-proves the ACCEPTED control first**, inside the same test. A corpus of only
rejections passes trivially against a verifier that refuses everything — which would brick every
legitimate update while looking green.

### The drill proves itself

Ten green tests prove nothing until the harness is shown to discriminate. The drill therefore
swaps its pristine control for the packaging-signed manifest — a real, valid signature under the
wrong role — and **requires** the suite to go red:

```
test result: FAILED. 1 passed; 9 failed; 0 ignored; 0 measured; 0 filtered out
```

Nine of ten fail. The tenth (`the_minted_corpus_holds_every_case…`) only checks file presence
and correctly still passes. Three self-passing modes are closed by construction: the test file
**panics** on a missing corpus rather than returning early; the driver asserts the **executed
pass count** and `0 ignored` / `0 filtered out` rather than trusting exit status; and each test
carries its own accepted control.

There is also a known-positive/known-negative pair inside the script itself, before the Rust
side runs: `manifest-verify` accepts the control under the live root and is required to
**refuse** the same bytes under the retired root (`key is retired: release-acceptance-key`).

---

## 5. How `--sequence` is derived, and why

**`next = max(sequence over every manifest ever published) + 1`; `1` when none exists.**

Chosen because it is derived from the very artifact the client compares against —
`decide_update` refuses a manifest at or below the machine's high-water mark — so it cannot
drift away from it. It also survives a **deleted or re-run release**, which `latest + 1` and
`count the releases` both fail: a max over all releases does not retreat when the newest one is
removed. `github.run_number` was rejected because it is a different quantity that merely happens
to increase, and it resets if the workflow file is renamed.

Two properties matter more than the formula:

- **It never skips.** Any manifest that cannot be read — unparseable, non-object, missing field,
  non-integer, boolean, negative — is a **hard error**. A max that silently drops an unreadable
  entry is how a sequence goes *backwards* with every step reporting success, which would
  disable the very check the sequence feeds. The script's self-test proves a skip-on-error
  implementation would have answered lower.
- **The absence claim is instrumented.** "No manifest has ever been published" is exactly what a
  broken query, a wrong repo or an empty response return for free. The step counts releases and
  their assets from the same query and **fails** if it sees releases with zero assets, because
  a broken query would otherwise yield `sequence=1` and silently disable freeze protection.

Today the honest answer is `sequence=1`: v0.12.25 publishes seven assets and no manifest.

---

## 6. The three clean-room results — a deliberate choice

`Evidence` models an omission as *"no result was produced"*, never *"the result was fine"*. So:

- **`--sbom` IS bound.** A real, byte-deterministic CycloneDX SBOM over the **locked** dependency
  graph, generated by `wayland-release sbom` from `cargo metadata --locked` and published as an
  asset. Marginal cost is one command — the tool is built anyway — and the evidence is genuine.
- **`--dependency-policy` OMITTED.** This pipeline produces no policy verdict, and a verdict
  requires `--dependency-policy-config` because a pass against an empty policy is not a pass.
  Recording an unearned `pass` would be worse than recording the absence. Related open HIGH:
  **SR-29-6 / F29-02-H1**, `.cargo/audit.toml`'s falsified "sole path".
- **`--reproducibility` OMITTED.** No second clean-room build happens here. There is no
  "unknown" value that reads as success, by design, and inventing one would be redefining
  success downward.

`certification` stays `Unavailable` because `manifest_build` hardcodes it — **SR-29-14**, not
mine to close.

---

## 7. Can `self-update` install anything now?

**No — and it is closer to yes than at any point before today.**

Stated precisely, because overstating this is the easy failure:

| | before | now |
|---|---|---|
| Bundled trust root | empty placeholder; `bundled()` **refused** | real key; **constructs** |
| `release.yml` publishes a manifest | never | **wired, never run** |
| A published release carrying a signed manifest | none | **still none** |

`self-update` against v0.12.25 today still reports
`UNAVAILABLE — release … publishes no *-release-manifest.json asset` and refuses, because
**that release has no manifest and never will** — the asset is minted at release time and
cannot be back-filled by any agent action.

**The trust root landing is necessary, not sufficient.** What remains is exactly one thing, and
it is Sean-reserved:

> **Cut a real release from a tag** so the wired `release.yml` job runs, mints the manifest with
> the CI secret, verifies it against the bundled root, and publishes the
> `*-release-manifest.json` asset. From the first release that carries one, `self-update`
> installs.

Nothing else blocks it. The workflow edit has **never executed** — no release run was triggered,
per the lane fence — so it is proved by construction, by unit self-tests, and by an end-to-end
drill over the identical command pair, but **not** by a live release. That gap is stated rather
than papered over: it is the one leg only Sean can run.

### What a first real run must be watched for

1. `Derive the manifest sequence` should print `considered 0 previously published manifest(s)`
   and `sequence=1`. Any other pair means the enumeration found something unexpected.
2. `Verify the signed manifest against the SHIPPED trust root` must print `MANIFEST VERIFIED`.
   If it fails, the CI secret does not correspond to the bundled public key and **nothing should
   be published** — which is what the step enforces.
3. The published asset must be named `wayland-core-<TAG>-release-manifest.json`. A near-miss
   fails silently on the client.

---

## 8. Credential and fence discipline

**Credentials.** This lane never needed and never sought the seed. Every key in every test and
in the drill is generated at run time from the OS CSPRNG into memory or a temp dir that is
deleted. The seed reaches `manifest-sign` on **stdin only**, never argv; no step enables
`set -x`.

Sweep over all 8 changed files, `/usr/bin/grep`, instrument proved alive first:

```
KNOWN_POSITIVE (bundled PUBLIC key must be found) = 2 occurrences, in 2 files
SWEEP 1  44-char base64 literals that are not a known public key ....... 0
SWEEP 2  hex blobs of 64+ chars ........................................ 0
SWEEP 3  a seed assigned to a literal .................................. 0
NEGATIVE CONTROL on a decoy carrying a fake 44-char key ................ 1 (instrument discriminates)
```

**Hit count: 0.** The secret's NAME appears 5 times in `release.yml` as an env reference, which
is required; its VALUE appears nowhere.

A first attempt at this sweep was **dead** — `zsh` does not word-split an unquoted variable, so
the file list never reached `grep` and every sweep returned 0 for free. The known-positive
returned nothing and exposed it. Repaired in-lane (bash array + `while read`) rather than noted
and moved past, and the numbers above are from the repaired instrument.

**Fences.** Measured against `63481a2d`, not against a branch name:

```
git diff 63481a2d -- crates/wcore-cli/src/lib.rs crates/wcore-cli/src/main.rs  ->  0 lines
git diff 63481a2d -- .github/workflows/ci.yml                                  ->  0 lines
```

**Fence exposure: ZERO.** `ci.yml` belongs to lane `ci-macos-budget` and was not touched; this
lane changed `release.yml` only. Not done, all reserved: no merge to `main`, no PR, no tag, no
release, no issue closed, no `wcore-contract generate`, **no release run triggered**.

---

## 9. Still open, and whose

| id | severity | owner |
|---|---|---|
| **F29-03-01 half two** — no published release carries a manifest | HIGH | **Sean** — cut a release; the pipeline is wired |
| **SR-29-10** — `self-update --help` still advertises the removed `.sig` scheme | MEDIUM | next lane holding the `main.rs` fence (fenced out here; replacement text ready in SR-29-10) |
| **SR-29-13** — no four-state ledger append/verify, no GitHub `environment:` approval gate | MEDIUM | release coordination. This lane added the manifest step SR-29-13 says to append *after*; the ledger and the human gate are untouched |
| **SR-29-14** — `manifest build` cannot record a certification binding | MEDIUM | whoever holds `wayland-release.rs` |
| **SR-29-6 / F29-02-H1** — `.cargo/audit.toml`'s falsified "sole path" | HIGH | `wcore-tools` owner. Blocks Success Criterion 1 and is why `--dependency-policy` is omitted above |
| **F-RTR-01** (new, MEDIUM) | — | closed in this lane |

### For the orchestrator to serialize

- **`.github/workflows/release.yml`** — sole editor this wave. No protocol seam, no contract
  request, no shared-file edit.
- **New workspace files** `.github/scripts/*` and `crates/wcore-cli/tests/release_manifest_pipeline.rs`
  are additions only.
- The drill is **not wired into `ci.yml`** because that file belongs to another lane. Whoever
  next owns it should add `.github/scripts/release-manifest-drill.sh` as a job — it is
  self-contained, needs no secret, and takes about a minute.
