# Phase 29 — Supply Chain and Release Integrity: PHASE VERDICT

**Phase 29's goal was not achieved. All four Success Criteria grade PARTIAL.**

That is the first line because it is the answer. Every criterion has real, executable
evidence behind part of it and a named, un-manufactured gap in the rest. Nothing here is
graded on reasoning alone, and nothing was narrowed to the part the evidence happened to
satisfy.

**Graded 2026-07-28 by plan 29-04, at `882191d4737c7552bef33214f4aadc31dbf828a3`
(lane `lane/29-04`, off `plan/f20-unified-audit-repair` at `c743f398`). All executable
evidence produced at `f6196a3275d41eaf769d4271b38d145e18739f26` on `hetzner-dsm`.**

Machine-readable index: `evidence/29-04/VERDICT-LEDGER.tsv`.

---

## The two sentences that must never be separated

**The release-acceptance mechanism is proved.** Run A drove the real `wayland-release`
binary through all four states and reported `CHAIN VERIFIED highest_state=release_acceptance
records=4 accepted=true`, with each record independently verifiable against only its own
role key.

**No release was accepted.** The key that completed run A was generated at run time into a
temporary directory and died with the run. The real release-acceptance key is Sean's; this
lane neither holds it nor requests it. Nothing was published, tagged, merged, or released.
A clean-room chain completing proves the mechanism works. It is not an authorization and
must never be read as one.

---

## Success Criterion 1

> **Clean-room builds verify provenance, SBOM, dependency policy, signatures, and
> reproducibility or documented variance.**

### Grade: **PARTIAL**

Evidence: `evidence/29-02/CLAUSE-LEDGER.tsv` (11 rows, every named file present),
`evidence/29-02/sbom-determinism.txt`, `evidence/29-02/deny-verdict.txt`,
`evidence/29-02/repro-measurement.txt`, `evidence/29-02/repro-variance-class.txt`,
`evidence/29-02/manifest-verify-pristine.txt` and `manifest-verify-mutated.txt`,
`evidence/29-01/pre-existing-controls.txt`, `evidence/29-04/open-high-f29-02-h1.txt`.

| Clause | Settled by | State |
|---|---|---|
| **SBOM** | 29-02 built it: a pure, byte-deterministic CycloneDX generator from `cargo metadata --locked`, proved by regeneration against a pinned fixture. The cross-path check caught a real defect — a `serialNumber` derived from the raw metadata text — that would have made a second party's regeneration differ. | **MET** |
| **reproducibility or documented variance** | 29-02 measured two clean-room build digests and produced `DOCUMENTED-VARIANCE`, class `path_prefix`, isolated to a single variable (cranelift's build-script `OUT_DIR` reaching the binary via `file!()`). The criterion explicitly admits documented variance. | **MET**, with the honest rider that the shipped release is reproducible only *accidentally* — GitHub runners happen to check out at the same path. |
| **dependency policy** | 29-02 wired `deny.toml` into a `just` recipe and a CI workflow where the measured baseline was **zero** execution sites, then captured the real verdict verbatim: `advisories FAILED, bans ok, licenses FAILED`, **exit 5**. The policy was not weakened by a character and was deliberately **not** chained into `check-all` while the verdict is red. | **NOT MET.** The policy now executes and its verdict is FAIL. A clean-room build that runs a policy and does not pass it has not verified it. |
| **signatures** | Manifest signing and verification proved end to end against an independently supplied trust root, with a one-byte tamper refused. But this is the *release-manifest* signature Phase 29 built; the artifact signature users actually rely on is keyless Sigstore, and only its fail-closed paths were exercised. | **PARTIAL** |
| **provenance** | Keyless Sigstore SLSA build provenance already ships (`actions/attest-build-provenance@v4`), `--provenance` on npm, and `self_update` fails **closed** when `gh` is absent. 29-02 re-measured these directly against the tree rather than citing the census. | **PARTIAL.** The **ACCEPT** path against the real transparency log has never been observed by this phase — F29-LIMIT-04. Fail-closed is proved; fail-open is unmeasured. |

**Which clauses this phase did versus which the pipeline already did:** SBOM,
reproducibility, dependency-policy execution, and the manifest-binding/tamper controls are
work Phase 29 performed. Toolchain lock, dependency lock, provenance attestation and
artifact signing are pre-existing pipeline behaviour that 29-02 re-measured and named as
`PRE-EXISTING` rather than claiming.

**Why not MET:** the dependency-policy verdict is red, and the provenance accept path is
unobserved. Open HIGH **F29-02-H1** bears directly on this criterion (below).

---

## Success Criterion 2

> **Install and update paths verify source/artifact identity, rollback/freeze protection, revocation, and key rotation.**

### Grade: **PARTIAL**

Evidence: `evidence/29-03/CLAUSE-LEDGER.tsv`, `evidence/29-03/LIVE-LEDGER.tsv`,
`evidence/29-03/live-downgrade.txt`, `evidence/29-03/rotation-drill.txt`,
`evidence/29-03/mutation-drill.txt` (19 mutations applied, 18 caught, revert control
green), `evidence/29-03/source-artifact-identity.txt`,
`evidence/29-03/plugin-backend-trust-root-gap.txt`,
`evidence/29-03/release-assets-have-no-manifest.txt`.

| Clause | 29-03's ledger | Why |
|---|---|---|
| rollback protection | **MET** | An ordered SemVer comparison in the shipped updater, measured live through the real binary against the real public GitHub API, with no update-source redirect. This is the strongest single result in the phase. |
| key rotation | **MET** | Full add-then-retire drill; a retired key is refused from its retirement instant on even though its signatures stay cryptographically valid. |
| source/artifact identity | **PARTIAL** | Archive bound to the manifest by SHA-256 **and** byte length, ANDed with provenance verification. Manifest↔**certified-binary** is **not** bound — `certification` remains `Evidence::Unavailable` pending R28-A. |
| freeze protection | **PARTIAL** | Persisted high-water mark plus `issued_at` staleness; mechanism proved by mutation, never exercised against a real published sequence. |
| revocation | **PARTIAL** | Version and artifact-digest revocation with a user-visible reason; never exercised against a really revoked release. |
| plugin/backend trust roots | **NOT PROVABLE HERE** | `INDEX_PUBKEY_HEX` is still the all-zeros placeholder (F-021). This is a **runtime install-time** trust root owned by Phase 25. F29-LIMIT-03. |

**The criterion is not narrowed to what was done.** It says "install and update paths
verify". Today the update path verifies by **refusing everything**: open HIGH
**F29-03-01** means `self-update` installs nothing at all until a real trust root is
substituted (F29-LIMIT-01) *and* releases publish a manifest asset (F29-LIMIT-02). No
install path has ever verified a real artifact end to end (F29-LIMIT-05). A reader must
not read PARTIAL here as "works".

**Why not NOT MET:** rollback protection and key rotation are genuinely MET, one of them
measured live through the shipped binary against the real public API. The mechanisms for
the rest exist and were hostile-tested. That is materially more than nothing.

---

## Success Criterion 3

> **Tampered artifacts, manifests, receipts, plugins, backends, or keys are rejected.**

### Grade: **PARTIAL**

Evidence: `evidence/29-04/TAMPER-LEDGER.tsv` (12 paired rows, all
`control=ACCEPTED::mutated=REFUSED`), `evidence/29-04/tamper-corpus-run.txt`,
`evidence/29-04/tamper-negative-controls.txt`, `evidence/29-03/mutation-drill.txt`,
`evidence/29-02/manifest-verify-mutated.txt`,
`evidence/29-03/plugin-backend-trust-root-gap.txt`.

Twelve cases, seven object classes, three distinct attacks in the key class. Every case is
a pair — pristine **ACCEPTED**, then exactly one mutation **REFUSED** — and the pairing is
enforced by the type rather than by review: a case without its control cannot be
constructed. The corpus's own ability to fail was proved four ways (a no-op mutation, all
controls refused, a dropped class, an inert mutation), each of which turned it red, with
the tree restored byte-clean afterwards.

| Object class in the criterion | State |
|---|---|
| artifacts | **MET** — one byte of a real compiled binary refused, plus 29-02's one-byte splice and 29-03's live archive-digest binding |
| manifests | **MET** — a body field changed without re-signing, and an unknown schema version, both refused; plus 29-01's live splice |
| keys | **MET** — unknown key id, retired key, and a cross-domain replay in which the *same* key signed the *same* body digest under a different domain separator |
| receipts | **PARTIAL** — the manifest↔receipt binding is proved in both directions, including a case where the manifest is untouched and only the bound receipt is mutated. But the shipped `manifest build` cannot emit an observed binding at all (F29-04-01), and R28-A is unlanded |
| plugins | **NOT MET at the layer that matters** — covered only at the release-manifest layer. The runtime install-time trust root is the all-zeros placeholder |
| backends | **NOT MET at the layer that matters** — same: release-manifest layer only; no runtime backend trust root exists |

**Stated rather than absorbed:** plugins and backends are covered here *only* because their
runtime paths belong to Phase 25. That is a judgement this verdict states and does not
settle — whether release-manifest-layer coverage suffices for F29-03 is a question for
Phase 30's independent review.

---

## Success Criterion 4

> **Packaging, deployment preparation, rollback rehearsal, and release acceptance remain separate evidence and authorization states.**

### Grade: **PARTIAL**

Evidence: `evidence/29-04/STATE-SEPARATION.tsv`, `evidence/29-04/STANDALONE-VERIFY.tsv`,
`evidence/29-04/COLLAPSE-ATTEMPTS.tsv`, `evidence/29-04/run-a-positive-control.txt`,
`evidence/29-04/run-a1-shipped-tool-only.txt`,
`evidence/29-04/run-b-withheld-acceptance-key.txt`,
`evidence/29-04/standalone-verify.txt`, `evidence/29-04/release-pipeline-states.txt`.

**As a mechanism, the separation is proved comprehensively:**

- **Run A** (all four role keys, Phase 28 seam instantiated in the clean room) reached all
  four states with four distinct role keys, per-state signature domains, canonical order,
  previous-record digest chaining and disjoint evidence sets — `accepted=true`.
- **Run A-1** (the same four keys, shipped tool only) reached only rollback rehearsal:
  `release acceptance requires an observed certification binding`. Holding every key is not
  sufficient. Possession of a signature is not authority.
- **Run B** (release-acceptance key withheld) capped at rollback rehearsal with **rc=0**
  and `accepted=false` — a stopping point, not a corrupt chain.
- **Standalone**: each of the four records verified by OpenSSL against only its own role
  key, with twelve cross-role controls all failing.
- **Seven collapse attempts** — relabel, wrong-role signature, reused evidence, skipped
  state, reordered chain, an invented fifth state named `termination_state_4`, and
  acceptance signed by a freshly rotated *packaging*-role key — every one refused, every
  one with a capture behind it.

**And yet the criterion is about the system, not about the artifact Phase 29 built.**
Re-measured at this commit (`release-pipeline-states.txt`), in the shipped pipeline:

- **zero** jobs in **any** workflow declare a GitHub `environment:` — the only native
  approval gate — so no manual approval separates any release stage;
- **zero** workflow steps mention rollback in any form, so rollback rehearsal does not
  happen at all;
- **zero** occurrences of `wayland-release`, `state-append`, `state-verify` or
  `release-manifest` in `release.yml` — the four-state ledger is not wired into the release
  pipeline in any form;
- one tag push matching `v*-wayland-*` drives build → github-release → publish-npm.

Packaging, deployment preparation and release acceptance are therefore **one authorization
act** in the product today. **This criterion can legitimately be MET while no release is
accepted** — separation is what it asks for — but it cannot be MET while the mechanism that
separates them is not used by anything that ships. That is finding **F29-04-03**.

---

## The two open HIGH findings — disposed of explicitly, not inherited silently

### F29-02-H1 — the `.cargo/audit.toml` suppression rests on a falsified premise

**Status: OPEN. Severity HIGH. Not closed by this phase, and not this phase's to accept
away.** Re-measured independently at this commit — `evidence/29-04/open-high-f29-02-h1.txt`.

`.cargo/audit.toml` silences RUSTSEC-2026-0194 and -0195 and states the parent trace as a
**"sole path"** — `quick-xml 0.39.4 ← plist 1.9.0 ← syntect 5.3.0 ← wcore-cli` — with a
threat model of **UNREACHABLE**, on the grounds that syntect only reads its own embedded
binary dumps.

**Sustained.** The lockfile at this commit carries a **second** path onto the same affected
version: `wcore-tools 0.12.25` depends on **quick-xml 0.39.4 directly**, behind the
**default-on** `doc-extract` feature, and uses it to parse docx/pptx OOXML parts from
**user-supplied files**. That input is attacker-controlled, nothing in `crates/` calls
`with_checks(false)`, and both advisories are denial-of-service advisories against exactly
that parser. The suppression's stated premise is false.

**One leg corrected and withdrawn.** The earlier framing attributed reachability to
calamine's 25 `.attributes()` call sites. `calamine 0.26.1` resolves to **quick-xml
0.31.0**, which the advisories do not name. That leg does not land on the affected version.
The finding does not depend on it and stands without it. 0195's UNREACHABLE claim is not
disturbed.

**NOT PROVABLE HERE:** no proof-of-concept `.docx`/`.pptx` was constructed. This is static
reachability on the dependency graph plus input provenance, not an executed exploit.
**Substitution point:** a crafted OOXML part driven through `wcore-tools`' `doc_tool`
against quick-xml 0.39.4.

**Effect on the grade:** it is one of the two reasons Success Criterion 1 is PARTIAL rather
than MET. A dependency policy whose only suppression rests on a false premise has not
verified anything. **Criterion 1 cannot be graded MET while this is open.**

**Owner:** `.cargo/audit.toml` was fenced out of 29-02 and is fenced out of 29-04, which
modifies no production source at all. **Escalated via SR-29-6, not repaired.**

### F29-03-01 — `self-update` installs nothing

**Status: OPEN. Severity HIGH. Deliberate fail-closed behaviour, and not agent-fixable.**

The shipped updater refuses every update until two independent inputs exist: a real release
trust root substituted into `RELEASE_TRUST_ROOT_JSON` (**F29-LIMIT-01**, SR-29-11 — Sean's
credential), and a signed release-manifest asset published by `release.yml`
(**F29-LIMIT-02**, SR-29-9 — a release-pipeline change). Measured: release v0.12.25
publishes 7 assets and none is a release manifest.

**This is correct behaviour, and it is also a broken update path.** Both things are true.
Failing closed is the right choice; the consequence is that the install path has never
verified a real artifact end to end (**F29-LIMIT-05**).

**Effect on the grade:** it is why Success Criterion 2 is PARTIAL, and why it cannot be
graded MET by any amount of further testing. **Neither of these HIGHs was closed, and
neither is closable inside this phase.**

---

## Every real-key, real-account and real-release limit, enumerated

Full table with substitution points: `evidence/29-04/PHASE-LIMITS.tsv`. Eight limits,
none of them graded MET anywhere in this phase, none simulated or worked around, and no
gate in any of the four plans passable by supplying one.

| ID | Kind | What changes on the day it exists |
|---|---|---|
| F29-LIMIT-01 | REAL-KEY | The real FerroxLabs release trust root replaces the empty `keys` array in `update_trust.rs`; `ReleaseVerifier::bundled()` stops refusing. |
| F29-LIMIT-02 | REAL-RELEASE | `release.yml` publishes `wayland-core-vX.Y.Z-release-manifest.json`, built and signed by `wayland-release`. |
| F29-LIMIT-03 | REAL-KEY | `INDEX_PUBKEY_HEX` in `plugin/index.rs` stops being all-zeros; the runtime plugin trust root becomes real. Phase 25's. |
| F29-LIMIT-04 | REAL-RELEASE | The **ACCEPT** path of `gh attestation verify` against the real Sigstore transparency log is observed for the first time. |
| F29-LIMIT-05 | REAL-RELEASE | A genuine end-to-end install — download, digest binding, attestation, extraction, atomic swap. Needs 01 and 02 first. |
| F29-LIMIT-06 | REAL-KEY | The release-acceptance role key is Sean's **by design**. Run A used a run-time key that died with the run. |
| F29-LIMIT-07 | REAL-KEY | A CI-signed evaluation receipt. Measured: `wayland-receipt sign` correctly **refuses** to CI-sign a fixture receipt, and this lane did not bypass that guard. |
| F29-LIMIT-08 | REAL-RELEASE | Phase 28's certification receipts on real native hardware, plus **R28-A** — the packaged-artifact list that binds the distributed archive to the certified binary. |

---

## Findings opened by 29-04 — all MEDIUM, all non-blocking

The severity policy is restated verbatim and **not tightened**: CRITICAL and HIGH must be
fixed or disproved with executable evidence; MEDIUM and below are filed to
`.planning/BACKLOG.md` and do not block. Inventing a stricter rule at the end of a phase is
what turned Phase 20 into a seventy-four-plan loop lasting two weeks.

| ID | Severity | Summary |
|---|---|---|
| F29-04-01 | MEDIUM | `wayland-release manifest build` hardcodes `certification: Unavailable`; the four-state chain cannot be completed through the shipped tooling. |
| F29-04-02 | MEDIUM | Release acceptance gates on the certification field merely being `Observed`; nothing verifies the binding joins to a real receipt. |
| F29-04-03 | MEDIUM | The four-state ledger is not wired into `release.yml`; one tag push drives all three shipped stages with no approval and no rollback rehearsal. |
| F29-04-04 | MEDIUM | 29-03's `F29-LIMIT-06` recorded `seandesktop` as unreachable. **Falsified:** `ssh SeanD@seandesktop 'hostname'` → `SeanDesktop`, rc=0. The account is `SeanD`. The Windows leg was **not** run here — a concurrent lane holds that host and running it would corrupt both measurements. |

---

## Requirement rows

**A requirement is not marked complete on the strength of a PARTIAL criterion.** All four
criteria graded PARTIAL, so:

| Requirement | Complete? |
|---|---|
| **F29-01** — toolchain/dependency lock, vulnerability/license policy, SBOM, provenance, artifact signing, reproducibility or documented variance | **NOT COMPLETE.** SBOM and reproducibility done; the dependency-policy verdict is FAIL and open HIGH F29-02-H1 is unresolved. |
| **F29-02** — installers/updates verify signed manifests, identity, rollback/freeze, revocation, rotation, plugin/backend trust roots | **NOT COMPLETE.** Rollback and rotation MET; open HIGH F29-03-01 leaves the update path installing nothing, and the plugin/backend trust-root clause is Phase 25's with a placeholder key. |
| **F29-03** — tampered binaries, SBOMs, updates, plugins, backend receipts, manifests or keys fail closed | **NOT COMPLETE.** Four of six object classes rejected by shipped verifiers with paired proof; plugins and backends covered only at the release-manifest layer. |
| **F29-04** — the four states remain distinct evidence states and separate authorization gates | **NOT COMPLETE.** Distinct **evidence** states: proved. Separate **authorization gates**: not in the shipped pipeline, which has zero approval gates and never invokes the ledger. |

---

## What a later reader should do with this

Three questions this verdict states and does **not** settle, recorded rather than resolved:

1. Whether release-manifest-layer coverage of plugins and backends is sufficient for F29-03
   in the eyes of an independent reviewer.
2. Whether the state ledger's evidence-disjointness rule is the right strength for a real
   release where some evidence legitimately spans states.
3. Whether these criteria can be re-graded upward once Phase 25 and Phase 28 land their
   halves. That is Phase 30's independent review to make, not this phase's to pre-empt.

**Phase 21 declared its own goal not achieved twice and that was the correct and most
useful outcome available. Phase 27 graded four of five criteria NOT MET and that grade is
why the phase is trustworthy.** Phase 29 grades four of four PARTIAL, with the evidence
behind every half-grade and the gap named in every other half. An honest PARTIAL costs the
program far less than a manufactured MET.
