# 29-01 — Supply-Chain Census

**Measured at** `c6766f02498f7bc7dda1511108c1d59ef9741af0` (branch `lane/29-01`, forked from
`plan/f20-unified-audit-repair`).
**Captured evidence:** `.planning/phases/29-supply-chain-release-integrity/evidence/29-01/`
**Machine index:** `evidence/29-01/CENSUS-LEDGER.tsv`

Every row below was produced by running a command over the real tree, a real parser over a real
workflow file, or the real shipped `wayland-core` binary. Nothing here is asserted from reading
source alone unless the row is explicitly marked **SOURCE-ONLY**, which is a reduced-severity
determination, not a promoted one.

---

## 0. The headline result, stated first

The shipped release path **already carries real, keyless, transparency-logged provenance**, and
the shipped updater **already fails closed** when it cannot verify it. That is measured, not
assumed, and this phase must not reinvent it.

What is missing is not a signing scheme. It is:

1. **Nothing binds the attested object to the certified object.** Sigstore attests the
   **archive**; the evaluation receipt certifies the **extracted binary**. No field, in any
   crate, relates those two digests. (F29-CEN-10, HIGH)
2. **A declared dependency policy that has never once executed.** (F29-CEN-04, HIGH)
3. **No SBOM of any format, anywhere.** (F29-CEN-05, HIGH)
4. **The updater's version decision is string equality, so a lower offer installs.**
   (F29-CEN-11, HIGH)
5. **The four F29-04 release states are one state wearing four labels** — a single tag push
   drives packaging, publication and npm release with no approval between them, and rollback
   rehearsal does not exist in any form. (F29-CEN-15..18, HIGH)

---

## 1. F29-01 — build and dependency integrity

### F29-CEN-01 — Toolchain lock: **CONFIRMED on the release path, DRIFT elsewhere**
*Evidence: `toolchain-lock.txt`, `release-path-unpinned-tool.txt`, `toolchain-drift-nightly.txt`*

`rust-toolchain.toml` pins `channel = "1.95.0"` with clippy and rustfmt. `vx.toml` pins
`rust = "1.95.0"` and `just = "1.48.1"`. Every release/CI build job routes through
`loonghao/vx@v0.9.17`, which honours those pins — **agreement**, measured across
`release.yml`, `ci.yml`, `e2e.yml`, `bench-regression.yml`, `marketplace-drift.yml` and
`mutants-nightly.yml`.

Two measured drifts:

- **`nightly-windows-soak.yml` installs `dtolnay/rust-toolchain@stable` at three sites**
  (lines 98, 311, 415), bypassing the 1.95.0 pin entirely. Not a release path. **MEDIUM.**
- **The aarch64-Linux release build does not route through the pin at all.** `release.yml:136`
  runs `cargo install cross --git https://github.com/cross-rs/cross` — **no tag, no rev, no
  `--locked`** — then `cross build --release` (line 142) rather than `vx cargo build`. One of
  the six shipped release binaries is therefore produced by a build tool fetched from a third
  party's git HEAD at release time. **HIGH** (F29-CEN-06).

### F29-CEN-02 — Dependency lock: **CONFIRMED**
*Evidence: `dependency-lock.txt`*

`Cargo.lock` is committed (268,139 bytes, 1,017 crates). `--locked` is used at seven call sites
across `ci.yml` and `justfile`. **The release build itself does not pass `--locked`**
(`release.yml:142,144`), so a release build could in principle resolve a different graph than CI
verified. In practice the checked-in lockfile is honoured by default for a workspace build; the
absence of the explicit flag is a **LOW** hardening gap, not a demonstrated divergence.

### F29-CEN-03 — Vulnerability policy: **CONFIRMED — it executes**
*Evidence: `vulnerability-policy.txt`*

This is a genuine positive and it is recorded as one. `vx cargo audit` runs in `ci.yml:233`
("cargo audit failures NOW block the build") and again inside the CI container at `ci.yml:354`.
Independently, `osv-scan.yml` runs `osv-scanner scan source --recursive` on every PR (with a
fail-safe relevance check that scans on any uncertainty) and nightly at 06:23 UTC. RUSTSEC and
OSV coverage is live and blocking. **No gap.**

### F29-CEN-04 — License / registry / ban policy: **CONFIRMED — declared, never executed. HIGH**
*Evidence: `license-policy-orphan.txt`, `deny-policy-materiality.txt`, `baseline-counts.txt`*

`deny.toml` is a real, strict, four-section policy: a 14-entry permissive allowlist with
`AGPL`/`GPL`/`LGPL`/`SSPL` absent by design, `exceptions = []`, `yanked = "deny"`,
`advisories.ignore = []`, `unknown-registry = "deny"`, `unknown-git = "deny"`,
`allow-git = []`.

**Measured: `cargo-deny` is invoked by ZERO files under `.github/` or in `justfile`.** The only
file in the entire repository matching `cargo[- ]deny` is `deny.toml` itself. `just audit` runs
`vx cargo audit` — the RUSTSEC advisory scanner, which does not evaluate licenses, bans, or
source registries. `check-all` chains `fmt-check lint test-ci hakari-verify audit`; no
cargo-deny anywhere.

**Partial refutation, recorded as a complete result:** the alarming corollary — that git
dependencies have crept in behind an unenforced `unknown-git = "deny"` — is **REFUTED**.
`Cargo.lock` contains **0** `source = "git+"` entries. The policy is currently satisfied by
circumstance.

What remains, and why it is still HIGH: **1,017 crates have never been evaluated against the
declared license allowlist.** The correct statement of the current position is not "the
dependency set complies" but "whether the dependency set complies is *unknown*". F29-01 names a
dependency policy as a requirement; a policy that never executes is a document.
**Repair belongs to 29-02.**

### F29-CEN-05 — SBOM: **CONFIRMED ABSENT. HIGH**
*Evidence: `sbom-absent.txt`, `sbom-false-positive.txt`, `baseline-counts.txt`*

Zero files match `cyclonedx` (case-insensitive) across `*.rs`, `*.toml`, `*.yml` under
`crates/`, `.github/` and `justfile`. Zero files under `.github/workflows/` mention SBOM or SPDX.

**Noise declared rather than hidden:** a case-insensitive tree-wide grep for `sbom|spdx|cyclonedx`
returns exactly one hit, `crates/wcore-channel-msteams/src/auth.rs:348`. That hit is the literal
substring `hsbOm0` inside a base64 test fixture. It is **REFUTED** as an SBOM reference. A
case-sensitive search for `CycloneDX|SPDX|SBOM` returns zero files.

No SBOM of any format exists in this repository, and none is attached to any release.
F29-01 names SBOM explicitly. This is a build, and the build is **29-02's**.

### F29-CEN-06 — Unpinned build tool in the release path: **CONFIRMED. HIGH**
*Evidence: `release-path-unpinned-tool.txt`*

`release.yml:136` — `cargo install cross --git https://github.com/cross-rs/cross`. No rev, no
tag, no `--locked`. This runs inside the job that produces the shipped
`aarch64-unknown-linux-gnu` release binary.

The irony is measurable and worth stating: the repository's own (never-executed) dependency
policy declares `unknown-git = "deny"` and `allow-git = []`, while the release pipeline installs
an unpinned build tool from git HEAD. Sigstore provenance will faithfully attest that this
archive was built by this repository's release workflow — provenance attests *builder identity*,
not *input trustworthiness*, so it does not cover this. **Repair belongs to 29-02.**

### F29-CEN-07 — Provenance and artifact signing: **CONFIRMED PRESENT — do not reinvent**
*Evidence: `provenance-produced.txt`*

- `actions/attest-build-provenance@v4` mints keyless Sigstore SLSA build provenance over
  `artifacts/wayland-core-*.tar.gz` and `artifacts/wayland-core-*.zip` — the distributed
  archives themselves — under job-scoped `id-token: write` + `attestations: write`.
- `sha256sum wayland-core-* > wayland-core-checksums.txt` produces a checksums asset.
- `npm publish --provenance` mints keyless Sigstore provenance for every published npm package.
- `self_update.rs:245-285` refuses to extract or swap unless `gh attestation verify <archive>
  --repo FerroxLabs/wayland-core` exits zero, and **fails closed when `gh` is absent** with
  actionable guidance rather than skipping the check.

A second, key-custody-bearing signing scheme layered over this would be debt for nothing. The
release manifest this plan lands **binds and references** this chain; it does not replace it.

**One sub-gap, MEDIUM:** `wayland-core-checksums.txt` is generated *after* the attest step and is
**not** in `subject-path`, so the checksums file itself carries no attestation. A consumer who
verifies only the checksums file gains nothing an attacker with release-write access could not
forge. The archives are individually attested, so the checksums file is redundant rather than
load-bearing — hence MEDIUM, to BACKLOG.

### F29-CEN-08 — Reproducibility: **CONFIRMED NEVER CHECKED. MEDIUM**
*Evidence: `reproducibility-never-checked.txt`*

No workflow, recipe or manifest sets `SOURCE_DATE_EPOCH`, rebuilds an artifact, or compares two
build outputs. The release binary is built exactly once per target
(`grep -c 'cargo build --release\|cross build --release' release.yml` = 1 branchpoint, one build
per matrix leg). No variance class is ever observed because no second observation is ever taken.

**Noise declared:** a bare grep for `reproducib` hits `ci.yml` five times; all five are English
prose in comments about a runner crash ("reproducibly crashes the runner agent"). Not a
reproducibility check. **REFUTED** as evidence of one.

Graded MEDIUM, not HIGH: reproducibility here is a detective control, and the existing
attestation already binds builder identity for every archive. It goes to BACKLOG, and 29-02
measures it. The release manifest this plan lands carries a reproducibility verdict field that
can honestly hold a documented variance rather than a false "reproduced".

---

## 2. F29-02 — the install / update path

All rows in this section were settled against the shipped `wayland-core` binary built at the
pinned SHA on Hetzner Linux, plus the source it was built from.

### F29-CEN-09 — Does the update path verify a signed manifest? **CONFIRMED: NO — by design**
*Evidence: `update-path-trust.txt`, `provenance-produced.txt`*

There is no release manifest to verify; the trust decision is delegated wholesale to
`gh attestation verify` against the pinned repo constant. That is a defensible design (it is
what removed the previous all-zeros pinned-key scheme, per the file's own header and finding
R16). The gap is that *nothing else about the release is bound* — not the SBOM, not the
dependency-policy outcome, not the certification receipt, not a rollback rehearsal. That is
precisely the object this plan lands. **No severity assigned to the design; the missing bindings
are counted in their own rows.**

### F29-CEN-10 — Source and artifact identity binding: **CONFIRMED GAP. HIGH**
*Evidence: `binding-gap-archive-vs-binary.txt`, `receipt-substrate-consumed.txt`*

This is the central finding of the census.

| Object | Digest | Who vouches for it |
|---|---|---|
| `wayland-core-vX.Y.Z-<triple>.tar.gz` (the **archive** users download) | covered by the Sigstore subject | `actions/attest-build-provenance@v4`, verified by `gh attestation verify` |
| the **extracted binary** that was actually evaluated | `receipt.body.identity.binary_sha256` | the Phase 28 evaluation receipt's Ed25519 CI signature |

**Nothing in the tree relates these two digests to each other.** `ReceiptBodyV1` (receipt.rs
lines 59-78, quoted in full in the evidence capture) carries `identity`, `target`, `policy`,
`timings`, `provider`, `tools`, `decisions`, `boundaries`, `process`, `recovery`,
`canary_scans`, `assertions`, `quarantines`, `required_cells`, `results` and `summary` — and
**no packaged-artifact field of any kind**. A grep for `archive_sha256|archive_digest|
packaged_artifact` across the updater, `receipt.rs` and `receipt_policy.rs` returns zero.

**Noise declared:** a tree-wide grep for `artifact_sha256` does hit
`crates/wcore-eval-scenarios/src/fixtures/remote_execution.rs` and its contract test. That is
the remote-execution subsystem, a different lineage, and it does **not** bind a release archive
to a certified binary. **REFUTED** as a counter-example.

Consequence, stated plainly: a release manifest cannot today honestly claim that the certified
candidate is the artifact users download. The archive could be repackaged from a *different*
binary and both the Sigstore attestation (which covers the archive as built by this workflow)
and the receipt (which covers a binary) would still verify independently — because no one ever
checks that they describe the same thing.

**This is the seam to Phase 28.** It is filed as requirement **R28-A** in
`29-01-RECEIPT-INTERFACE.md`, and this plan's manifest models the join as
`Evidence<CertificationBindingV1>` so 29 is buildable and provable before 28 lands.

### F29-CEN-11 — Downgrade protection: **CONFIRMED ABSENT. HIGH**
*Evidence: `update-path-downgrade.txt`, plus the live run in `live-self-update-check.txt`*

The entire version decision in the shipped updater is `self_update.rs:58-65`:

```rust
if latest_version == current_version {
    println!("already up to date.");
    return Ok(());
}
if check_only { ... }
// ...otherwise: download, verify provenance, extract, atomic_swap
```

**String equality.** Measured: zero occurrences of `semver`, `version_compare`, or any ordering
comparison on versions anywhere in the file. There is no branch that distinguishes "the offer is
newer" from "the offer is older". Any version that is merely *different* falls through to
install.

Consequence: if the `releases/latest` pointer ever resolves to an **older** genuine release —
by a maintainer un-publishing a release, a re-tag, or an actor with release-write access — a
running newer binary will **downgrade itself to it**, and every existing control will pass while
it does, because the older archive is a genuine artifact with genuine provenance. This is the
classic rollback attack, and the provenance check is structurally incapable of catching it: the
old archive really was built by this repository's release workflow.

**Determination: SOURCE-ONLY, and this is a correction made against my own first draft.**

I initially drafted this row claiming the live run demonstrated selection of the *install*
branch. **Running it refuted that.** The live capture at the pinned SHA shows:

```
current: v0.12.25
latest:  v0.12.25
already up to date.
```

The running binary's version is *equal* to the published latest, so the live run took the
**equality** branch and returned before reaching any install decision. The install branch was
**not** observed, and this row does not claim it was.

What the live run does establish, and it is not nothing: the comparison is reached and its two
operands are printed verbatim; the whole path runs **unauthenticated** with `GH_TOKEN` and
`GITHUB_TOKEN` explicitly unset; and the printed surface contains no "offered version is older"
concept whatsoever, consistent with the source, where no such branch exists.

The downgrade behaviour itself is therefore graded **SOURCE-ONLY at reduced confidence of
reachability** — though note the source is unusually unambiguous here: there is exactly one
comparison in the function and it is `==`.

**Not measurable here, honestly stated:** driving a genuinely lower *offer* through the shipped
entry point end to end requires redirecting the update source. Per this plan's threat model
(T-29-01-08) and its explicit instruction, **no environment override that repoints the update
source was added** — an env var that repoints the updater is itself a supply-chain attack
surface, and adding one to measure a weakness would be a net loss. **29-03 makes the decision
testable by extraction, not by redirection.**

### F29-CEN-21 — Shipped `self-update --help` misstates the trust model: **CONFIRMED. MEDIUM**
*Evidence: `live-help-text-misstates-trust-model.txt`, `live-self-update-check.txt`*

**Found only by running the binary.** No source read of `self_update.rs` would have surfaced it,
because the defect is in `crates/wcore-cli/src/main.rs:693`, not in the updater.

The shipped binary tells the user, verbatim:

> `Verifies the .sig artifact against the pinned marketplace pubkey (ed25519) before atomic swap.`

**No part of that is true any more.** There is no `.sig` artifact — `release.yml` never produces
one. There is no pinned marketplace pubkey in the update path. Verification is keyless Sigstore
via `gh attestation verify`. `self_update.rs`'s own header states the advertised scheme was
removed precisely *because* it shipped an all-zeros placeholder key and the pipeline never
produced `.sig` files (finding R16).

Graded MEDIUM, not HIGH: the **actual** control is stronger than the advertised one and it fails
closed, so no security property is weakened. The harm is that a user auditing their own supply
chain is told a false mechanism, and will look for a signature file and a key rotation story
that do not exist.

**Not repaired here, and the reason is a fence, not a judgement:** the string lives in
`crates/wcore-cli/src/main.rs`, which this plan and every concurrent lane are forbidden to
touch. Filed to BACKLOG and flagged to **29-03**, which owns the update path.

### F29-CEN-12 — Freshness / stale-offer protection: **CONFIRMED ABSENT. MEDIUM**
*Evidence: `update-freshness-revocation.txt`*

Measured: **0** occurrences of `expires|expiry|timestamp|published_at|created_at|freshness|
SystemTime` in `self_update.rs`. The `Release` struct models exactly two fields — `tag_name`
and `assets` — so no publication time is even parsed. Nothing bounds the age of an offer and
nothing detects a frozen `releases/latest`.

Graded MEDIUM: exploiting it requires an adversary able to hold a TLS connection to
`api.github.com` at a chosen response, which is a materially higher bar than the rollback in
F29-CEN-11 (which needs no network position at all). **BACKLOG**; 29-03 may fold it in.

### F29-CEN-13 — Revocation: **CONFIRMED ABSENT. MEDIUM**
*Evidence: `update-freshness-revocation.txt`*

Measured: **0** occurrences of `revoke|revocation|crl|blocklist|denylist` in `self_update.rs`.
There is no list of known-bad releases and no path by which a published-then-withdrawn release
is refused by an already-installed client. Under the keyless model, revocation would have to be
expressed as attestation-level or release-level state; neither is consulted. **BACKLOG**, 29-03.

### F29-CEN-14 — Multiple trust keys / rotation: **CONFIRMED SINGLE ANCHOR. MEDIUM**
*Evidence: `update-path-trust.txt`*

The sole trust anchor is `pub const RELEASES_REPO: &str = "FerroxLabs/wayland-core"` — a
compile-time constant, deliberately pinned so a misconfigured workspace cannot redirect updates.
Measured: **0** occurrences of `env::var(` in the file, confirming no runtime override exists.
Because the scheme is keyless, there is no key list to rotate — rotation is GitHub's and
Sigstore's problem, not this repository's. The residual gap is that a *policy* change (e.g.
moving orgs) requires a new binary. **BACKLOG.**

This constant, and its 3 occurrences, is asserted untouched by this plan's no-redirect gate.

---

## 3. F29-04 — the four release states in the pipeline as it exists

*Evidence: `f29-04-states-in-pipeline.txt`, `f29-04-authorization.txt`*

`release.yml` has five jobs: `prepare-release` → `build` → `github-release` →
`post-tag-smoke` → `publish-npm`.

**Measured, and this is the finding:** `grep -rn 'environment:' .github/workflows/` returns
**ZERO**. No job in any workflow declares a GitHub Environment — the only native manual-approval
gate GitHub offers. Every one of the states below is therefore authorized by exactly one act:
pushing a tag matching `v*-wayland-*`.

| F29-04 state | Job that performs it | Evidence artifact it produces | Distinct authorization | Determination |
|---|---|---|---|---|
| **1. Packaging** | `build` (6-way matrix) | the six archives, uploaded via `actions/upload-artifact@v7` | none — the tag push | present, unauthorized separately |
| **2. Deployment preparation** | `github-release` | Sigstore attestation + `wayland-core-checksums.txt` + the published GitHub release | none — inherits from `build` via `needs:` | **COLLAPSED into state 1** |
| **3. Rollback rehearsal** | — none — | — none — | — none — | **ABSENT ENTIRELY** |
| **4. Release acceptance** | `publish-npm` | published npm packages with `--provenance` | none — inherits via `needs: post-tag-smoke` | **COLLAPSED into states 1-2** |

### F29-CEN-15 — Packaging state: **CONFIRMED PRESENT, NOT SEPARATELY AUTHORIZED. HIGH**
The `build` job produces real artifacts, but nothing gates entry to it beyond the tag push and
nothing distinct signs its output as "packaging complete".

### F29-CEN-16 — Deployment preparation: **CONFIRMED COLLAPSED. HIGH**
`github-release` runs automatically the instant `build` succeeds. It mints the attestation and
publishes the GitHub release in the same job, under the same trigger, with no human in the loop.
Packaging and publication are one event.

### F29-CEN-17 — Rollback rehearsal: **CONFIRMED ABSENT. HIGH**
Measured: `grep -rniE 'rollback|roll-back|revert release|downgrade|previous version'` across
**all** of `.github/workflows/` returns **ZERO matches**. No step in any workflow rehearses,
tests, or even mentions a rollback. There is no evidence artifact because there is no rehearsal.

This is the sharpest F29-04 gap: the state does not merely lack authorization, it does not
exist. And it compounds F29-CEN-11 — the product has no rehearsed rollback procedure *and* its
updater will silently install a lower version, so the one rollback mechanism that does work in
practice is the one nobody controls.

### F29-CEN-18 — Release acceptance: **CONFIRMED COLLAPSED, WITH A SILENT-SKIP PATH. HIGH**
`publish-npm` is the closest thing to release acceptance and it is fully automatic. Worse, its
`Guard — NPM_TOKEN present` step converts a missing `NPM_TOKEN` into
`::notice::NPM_TOKEN not set — skipping npm publish` and a **successful** job. A release can
therefore be declared complete while the acceptance step silently did nothing. There is no
artifact that records "a human accepted this release", because no human is asked.

**This is the structural justification for the closed four-state ledger this plan lands.** The
four states are collapsed in the real pipeline, so a prose separation would be indistinguishable
from the status quo. The ledger makes each state a separately-signed record under a
role-scoped key with disjoint evidence, so collapsing them becomes a type error rather than a
convention someone can walk around under pressure.

---

## 4. Gap table, severity-ordered

**The CRITICAL/HIGH set below binds the rest of Phase 29.** No finding reached CRITICAL.

| ID | Gap | Severity | Determination | Repair owner |
|---|---|---|---|---|
| F29-CEN-10 | Nothing binds the attested **archive** to the certified **binary** | HIGH | CONFIRMED | 29-01 manifest + **R28-A on Phase 28** |
| F29-CEN-11 | Updater compares versions by string equality; a lower offer installs | HIGH | **SOURCE-ONLY** (live run took the equality branch — see row) | **29-03** |
| F29-CEN-04 | `deny.toml` license/ban/source policy has never executed; 1,017 crates unevaluated | HIGH | CONFIRMED (git-source corollary REFUTED) | **29-02** |
| F29-CEN-05 | No SBOM of any format anywhere | HIGH | CONFIRMED | **29-02** |
| F29-CEN-06 | Release path installs `cross` from unpinned git HEAD | HIGH | CONFIRMED | **29-02** |
| F29-CEN-15 | Packaging state has no distinct authorization | HIGH | CONFIRMED | 29-01 ledger; pipeline wiring 29-02/29-04 |
| F29-CEN-16 | Deployment preparation collapsed into packaging | HIGH | CONFIRMED | 29-01 ledger |
| F29-CEN-17 | Rollback rehearsal does not exist in any workflow | HIGH | CONFIRMED | 29-01 ledger; rehearsal itself 29-03/29-04 |
| F29-CEN-18 | Release acceptance collapsed and silently skippable on missing `NPM_TOKEN` | HIGH | CONFIRMED | 29-01 ledger |
| F29-CEN-21 | Shipped `self-update --help` advertises a removed `.sig` + pinned-ed25519 scheme | MEDIUM | CONFIRMED **(live-only find)** | BACKLOG + 29-03 |
| F29-CEN-01b | `nightly-windows-soak.yml` bypasses the 1.95.0 pin (`dtolnay/rust-toolchain@stable` ×3) | MEDIUM | CONFIRMED | BACKLOG |
| F29-CEN-08 | Reproducibility never measured; no second build ever taken | MEDIUM | CONFIRMED | BACKLOG + 29-02 |
| F29-CEN-12 | No freshness bound on the update offer | MEDIUM | CONFIRMED | BACKLOG + 29-03 |
| F29-CEN-13 | No revocation surface consulted | MEDIUM | CONFIRMED | BACKLOG + 29-03 |
| F29-CEN-14 | Single compile-time trust anchor; policy change needs a new binary | MEDIUM | CONFIRMED | BACKLOG |
| F29-CEN-07b | `wayland-core-checksums.txt` is unattested (not in `subject-path`) | MEDIUM | CONFIRMED | BACKLOG |
| F29-CEN-02b | Release build omits `--locked` | LOW | CONFIRMED | BACKLOG |
| F29-CEN-03 | Vulnerability policy (`cargo audit` + `osv-scanner`) | — | **CONFIRMED PRESENT, no gap** | — |
| F29-CEN-07 | Provenance produced and fail-closed on verify | — | **CONFIRMED PRESENT, no gap** | — |
| F29-CEN-19 | Git dependencies behind unenforced `unknown-git = "deny"` | — | **REFUTED** — 0 git sources in `Cargo.lock` | — |
| F29-CEN-20 | An SBOM reference exists in `wcore-channel-msteams` | — | **REFUTED** — base64 fixture substring `hsbOm0` | — |

---

## 5. Live product observation (verbatim)

*Evidence: `live-self-update-check.txt`, `live-help-text-misstates-trust-model.txt`*

The real `wayland-core` binary was built `cargo build --release --locked -p wcore-cli` at
`c6766f02498f7bc7dda1511108c1d59ef9741af0` on Hetzner Linux (build exit 0, 5m20s) and run
against the real public GitHub API with **no credential of any kind** — `GH_TOKEN` and
`GITHUB_TOKEN` were explicitly unset for the run to prove it.

```
GH_TOKEN=[<unset>] GITHUB_TOKEN=[<unset>]
### wayland-core --version
wayland-core 0.12.25
### wayland-core self-update --check-only (real public API, unauthenticated)
current: v0.12.25
latest:  v0.12.25
already up to date.
### self-update --help (the full shipped surface)
v0.8.1 U9: update wayland-core to the latest signed release from `FerroxLabs/wayland-core`.
Verifies the `.sig` artifact against the pinned marketplace pubkey (ed25519) before atomic
swap. Use `--check-only` to print versions without installing

Usage: wayland-core self-update [OPTIONS]

Options:
      --check-only  Print current vs. latest version and exit without installing
  -h, --help        Print help
```

**Three things this live run produced that no source read would have:**

1. **The plan's own command was wrong.** 29-01-PLAN.md specifies
   `wayland-core self-update --check`. The shipped flag is `--check-only`; `--check` exits **2**
   with `error: unexpected argument '--check' found`. The first capture recorded that failure
   verbatim before the command was corrected. A gate written as the plan specified would have
   failed for a reason unrelated to the property under test.
2. **F29-CEN-21** — the help text misstates the trust model. See that row.
3. **F29-CEN-11 was falsified as a live claim** and demoted to SOURCE-ONLY. See that row.

**A note on the gate that produced this.** My first invocation ended the remote command with an
`echo`, so `rc` captured the echo's status and would have reported 0 even had the binary
crashed — the exact self-passing class this program keeps rediscovering. The capture above
comes from the corrected form, where each binary invocation is `|| exit 21/22/23`, so a failing
binary produces a distinct non-zero remote status that the gate asserts on.

---

## 7. Honest limits of this census

- Files under `evidence/29-01/` are written by the executor, so a ledger row is not
  self-proving. The mitigation and its ceiling: every row **names** a captured file, the
  automated gate stats each named file and asserts a line floor, and the two headline
  baselines are **independently re-measured by a gate that reads the tree directly**, not the
  census. Substantive claims about the product are additionally re-measured by a remote gate
  that runs a real binary.
- The install-side half of F29-CEN-11 is SOURCE-ONLY by deliberate choice, not by omission.
  See that row.
- **NOT PROVABLE HERE, with the substitution point named:** whether a real Sigstore attestation
  for a *published* archive verifies on a machine without `gh` authentication cannot be answered
  without a real published release and a real `gh` install. Substitution point: a post-release
  job, or 29-03's install-path work, running `gh attestation verify` against a real published
  asset. No credential was fabricated to close this.
- **NOT PROVABLE HERE:** whether GitHub's attestation covers the checksums file or any future
  manifest asset depends on a `subject-path` this plan does not change. Substitution point:
  29-02's workflow work.
- Whether Phase 28 accepts R28-A..R28-F as written or counter-proposes is Phase 28's decision.
