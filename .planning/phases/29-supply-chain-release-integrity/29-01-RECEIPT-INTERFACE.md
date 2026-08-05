# 29-01 — The Phase 28 Receipt Interface

**What Phase 29 consumes from Phase 28's certification receipts, and what Phase 28 must add.**

Phase 29 does **not** modify `crates/wcore-eval-scenarios/src/receipt.rs`,
`src/receipt_policy.rs`, or `bin/wayland-receipt.rs`. Phase 28 owns those three files and is
executing against them concurrently. Phase 29 consumes their public types read-only, and states
its requirements here as a request rather than discovering them later as a conflict.

Filed to Phase 28's planner as `.planning/SEAM-REQUESTS/29.md`.

---

## Part one — what Phase 29 CONSUMES (fields that exist today)

Every field below exists in `receipt.rs` at `c6766f02` and is quoted, not paraphrased. Phase 29
requires each to be **present and observed**, and binds them into
`ReleaseManifestBodyV1.certification` as `Evidence<CertificationBindingV1>`.

| Receipt field (as it exists) | Type | How Phase 29 uses it |
|---|---|---|
| `EvidenceReceiptV1.schema` | `String` (`"wayland.eval.receipt"`) | Pinned into the binding; a schema Phase 29 does not know fails closed |
| `EvidenceReceiptV1.schema_version` | `u32` (`1`) | Pinned into the binding; see **R28-F** |
| `EvidenceReceiptV1.body_sha256` | `String` | **The receipt's identity in the release manifest.** See **R28-E** |
| `AuthorityClaimV1::Ci { key_id, signature_base64 }` | enum variant | Required. A `Local` claim is NOT acceptable release evidence, and the key is verified against a key supplied from OUTSIDE the receipt via `ReceiptVerifier::trust_ci_key` |
| `body.identity.source_commit` | `String` (40 hex) | Must equal the release manifest's `source_commit` |
| `body.identity.binary_sha256` | `String` (64 hex) | The **extracted binary** that was evaluated. See **R28-A** for why this is not sufficient alone |
| `body.identity.config_sha256` | `String` (64 hex) | Consumed as configuration identity |
| `body.identity.fixture_sha256` | `String` (64 hex) | Consumed as fixture identity |
| `body.identity.build` | `Evidence<BuildProvenanceV1>` | Must be `Observed`. Carries `repository`, `source_ref`, `workflow`, `invocation_id` |
| `body.target.os` / `.architecture` / `.sandbox_backend` | `String` | Bound into the certification binding; `sandbox_backend` is the only environment fact available today — see **R28-D** |
| `body.policy.posture` | `String` | Consumed as the effective posture under which certification ran |
| `body.policy.effective_policy_sha256` | `String` (64 hex) | Consumed as the policy identity |
| `body.required_cells` | `Vec<String>` | The coverage manifest. See **R28-C** for what it cannot express |

**A note Phase 28 should read before changing anything:** Phase 29 uses
`EvidenceReceiptV1.body_sha256` as the receipt's canonical identity inside the release manifest.
It must not be repurposed, redefined, or made to cover a different projection. `behavior_sha256`
exists as a separate cross-run determinism oracle and Phase 29 does **not** consume it.

**Verification discipline Phase 29 inherits and does not weaken.** `receipt.rs`'s
`validate_ci_provenance` refuses a CI receipt with `UnsignedAuthoritative` unless **all five**
`VerificationPolicy` fields are populated. That is correct fail-closed behaviour and Phase 29
depends on it. (This was confirmed the hard way: the first version of Phase 29's contract suite
passed `VerificationPolicy::default()`, receipt.rs correctly rejected everything, and a
cross-domain test was silently passing for the wrong reason until an anti-vacuity control caught
it. See `29-01-SUMMARY.md`.)

---

## Part two — requirements on Phase 28 (R28-A .. R28-F)

Each carries **what breaks if it is omitted**.

### R28-A — A packaged-artifact list binding each distributed archive by name and digest

**Add to `ReceiptBodyV1`** a list of packaged artifacts, each with a name, a SHA-256 digest, a
byte length, and a kind — mirroring `ReleaseManifestBodyV1.artifacts`, which already has this
shape.

**Why.** `identity.binary_sha256` is the **extracted binary**. GitHub's
`actions/attest-build-provenance@v4` attests the **archive** (`subject-path:
artifacts/wayland-core-*.tar.gz`, `*.zip`). Measured at `c6766f02`: **nothing in the tree binds
those two objects to each other** — a grep for `archive_sha256|archive_digest|packaged_artifact`
across the updater, `receipt.rs` and `receipt_policy.rs` returns zero. (Census finding
**F29-CEN-10, HIGH**.)

**What breaks if omitted.** A release manifest cannot honestly claim that the certified
candidate is the artifact users download. An archive repackaged around a *different* binary
would still satisfy the Sigstore attestation (it really was built by this workflow) and still
satisfy the receipt (it really does describe some binary) — because no one ever checks the two
describe the same thing. Phase 29 must then carry the certification binding as permanently
`Unavailable`, which by the ledger's own rule makes **release acceptance unreachable**.

### R28-B — Retained-log digests

**Add** a digest per retained evidence log, so evidence that outlives the runner is **bound**
rather than referenced.

**What breaks if omitted.** Evidence referenced only by URL or run id is mutable and expirable.
A release manifest that binds a reference rather than a digest cannot detect that the log it
points at was replaced or garbage-collected, so the audit trail degrades silently instead of
failing closed.

### R28-C — An explicit skipped-case policy: case id, reason code, and who authorized it

**Add** a skip vocabulary to the receipt body. Today `required_cells` plus `results` **implies**
coverage but carries no way to say "this case was deliberately not run, for this reason, on this
authority".

**What breaks if omitted.** F28-04 forbids skipping a critical case, but a silent omission is
**indistinguishable from a pass** at the manifest layer: a shrunken `required_cells` and a
complete `results` set look identical to full coverage. Phase 29 cannot gate on something it
cannot see, so an unauthorized skip would be certified as if it had run.

### R28-D — An environment digest covering the toolchain version, the runner image and the environment allowlist

**Add** a single digest (or a small struct plus its digest) covering the build/run environment.

**Why now.** F28-03 names environment, and today the only environment facts in the receipt are
`target.sandbox_backend` and `policy.effective_policy_sha256`. Neither identifies the toolchain
or the runner image. The census found this matters concretely: the release path builds
`aarch64-unknown-linux-gnu` with `cross` installed from **unpinned git HEAD**
(`release.yml:136`), and three jobs in `nightly-windows-soak.yml` install
`dtolnay/rust-toolchain@stable` rather than the pinned 1.95.0 (**F29-CEN-06**, HIGH;
**F29-CEN-01b**, MEDIUM).

**What breaks if omitted.** Two receipts that differ only by toolchain or runner image are
indistinguishable, so a certification performed under a drifted toolchain is accepted as
equivalent to one performed under the pinned one. Reproducibility claims (F29-CEN-08) become
unfalsifiable for the same reason.

### R28-E — `body_sha256` remains the receipt's canonical identity and its computation does not change

**Do not** change what `body_digest` hashes, and do not repoint `body_sha256` at a projection.

**What breaks if omitted.** Phase 29 stores `receipt_body_sha256` inside a **signed** release
manifest. If the digest's meaning changes, every previously signed manifest silently begins
referring to a different object while its signature still verifies — the worst available
failure, because it is invisible.

### R28-F — Any body extension bumps the schema version, or preserves deterministic body and behavior digests

**Either** increment `RECEIPT_SCHEMA_VERSION` when `ReceiptBodyV1` gains or loses a field,
**or** guarantee the extension is digest-neutral.

**What breaks if omitted.** Phase 29 pins `receipt_schema_version` in the certification binding
and **fails closed on a version it does not know**. An unversioned body extension changes
`body_sha256` for identical evidence, so a manifest signed before the change no longer matches
the receipt it was minted against, and the failure surfaces at release time as an
indistinguishable digest mismatch rather than as an actionable version error.

---

## Part three — the degradation, and why it is deliberate

The certification binding is `Evidence<CertificationBindingV1>` — the **existing** `Evidence<T>`
from `receipt.rs`, not a new option type. This buys three properties:

1. **Absence is explicit, never an empty success.** An `Unavailable { code }` is a positive
   statement carrying a reason. An absent field, or an empty struct, would read as "fine".
2. **Phase 29 is fully executable and fully provable before Phase 28 lands.** A manifest with an
   unavailable binding builds, signs and verifies, and progresses through packaging, deployment
   preparation and rollback rehearsal.
3. **Release acceptance refuses an unavailable binding.** This is what makes it impossible to
   ship a release whose certification never happened.

Proved live, not asserted — from `evidence/29-01/live-wayland-release-e2e.txt`, against a
manifest whose binding is `Unavailable` because Phase 28 has not landed:

```
state-verify rc=0 out=CHAIN VERIFIED highest_state=rollback_rehearsal records=3 accepted=false
append[release_acceptance] rc=0 out=STATE APPENDED state=release_acceptance ...
state-verify rc=1 out=wayland-release: release acceptance requires an observed certification binding
```

Note what the third line means: **the acceptance record can be minted, and the chain still
refuses to be accepted.** Possession of a signature is not authority.

**Consequence for this lane, stated plainly:** release acceptance is currently unreachable for
two independent reasons — Sean holds the release-acceptance key, and Phase 28 has not yet
supplied a certification binding. Both are correct. Neither was simulated, stubbed, or worked
around.
