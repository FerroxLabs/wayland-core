# 29-02 — Clean-Room Results

**Measured at** `148330ae` / `2c89b141` (branch `lane/29-02`, merge-base
`2fd771d2cd69e13f5c686f859547ab46ebddd41f`).
**Host:** Hetzner Linux `6.8.0-101-generic x86_64`, rustc `1.95.0` (the pin in `rust-toolchain.toml`).
**Captured evidence:** `evidence/29-02/` — machine index `evidence/29-02/CLAUSE-LEDGER.tsv`.

Every number below came from running something and reading its output. No credential of any
kind was used; the one signing key was generated at run time into a temporary directory and
died with the run.

---

## 0. The three headline results

1. **The dependency policy executed for the first time and FAILED, exit 5.** Verdict verbatim:
   `advisories FAILED, bans ok, licenses FAILED, sources ok`. `deny.toml` was **not** touched
   and `deny` was **not** chained into `check-all`.
2. **Reproducibility is DOCUMENTED-VARIANCE, class `path_prefix`** — identified single-variable,
   not guessed. The shipped release is reproducible in practice but only *accidentally*.
3. **A one-bit change to the SBOM breaks manifest verification**, with the pristine control
   accepted first.

And one finding that only appeared *because* the policy finally ran:

4. **F29-02-H1 (HIGH)** — the `.cargo/audit.toml` exception silencing two quick-xml DoS
   advisories rests on a **"sole path"** claim that the real dependency graph falsifies.

---

## 1. Dependency policy — F29-CEN-04 closed as *wired*, verdict recorded as *red*

*Evidence: `deny-verdict.txt`*

`cargo-deny 0.20.2`, policy `deny.toml` sha256 `05a8535b…`, 1,017 crates.

```
advisories FAILED, bans ok, licenses FAILED, sources ok
F29-DENY-STATUS::FAIL::exit=5
```

| # | Finding | Category | Severity | Disposition |
|---|---|---|---|---|
| 1 | `RUSTSEC-2026-0194` quick-xml quadratic attribute check — **0.31.0 and 0.39.4** | vulnerability | see F29-02-H1 | escalated, SR-29-6 |
| 2 | `RUSTSEC-2026-0195` quick-xml unbounded namespace allocation | vulnerability | LOW (not reachable — `NsReader` used nowhere) | BACKLOG |
| 3 | `RUSTSEC-2026-0192` `ttf-parser` unmaintained (no safe upgrade exists) | unmaintained | MEDIUM | BACKLOG |
| 4 | `RUSTSEC-2024-0370` `proc-macro-error` unmaintained | unmaintained | MEDIUM | BACKLOG |
| 5 | `RUSTSEC-2025-0141` `bincode` unmaintained | unmaintained | MEDIUM | BACKLOG |
| 6 | `wcore-fixture-harness 0.1.0` is unlicensed | licenses | LOW | BACKLOG — one-line remedy below |

**Two positives, recorded as prominently as the gaps:**
- **`sources ok`.** The policy's `unknown-registry = "deny"` / `unknown-git = "deny"` /
  `allow-git = []` are satisfied. This is an *independent* confirmation of 29-01's F29-CEN-19
  refutation, from a different tool: zero git sources in the graph.
- **`bans ok`.** 60 `duplicate` warnings, all non-failing by policy (`multiple-versions = "warn"`).

**Finding 6's remedy, not applied here.** Every other workspace crate carries `publish = false`
and/or `license.workspace = true`; `crates/wcore-fixture-harness/Cargo.toml` carries neither, so
`deny.toml`'s existing `private = { ignore = true }` does not classify it as first-party. Adding
`publish = false` — one line, to our own never-published test-harness crate — is the correct
*change-the-input* fix and is **not** a policy widening. It was **not** applied because that file
is outside this plan's declared `files_modified` and its surgical-diff gate, and because fixing
it alone would not change the verdict (advisories still fail) or the `check-all` decision.

**`deny.toml` is unweakened in all four directions, gate-checked:** the licence allowlist is
untouched, `exceptions = []`, `advisories.ignore = []`, `unknown-registry = "deny"`,
`unknown-git = "deny"`. No entry was added to close any red.

### 1a. F29-02-H1 (HIGH) — the quick-xml exception rests on a falsified "sole path"

*Evidence: `deny-verdict.txt` §"REACHABILITY OF THE IGNORED ADVISORIES"; escalation `SR-29-6`*

`cargo audit` exits **0** on the identical tree. That divergence is **not** advisory-database
staleness — the database cargo-audit loaded contains both advisories. It is silent because
`.cargo/audit.toml:54` carries `ignore = ["RUSTSEC-2026-0194", "RUSTSEC-2026-0195"]`, and
`.github/osv-scanner.toml` carries the same disposition. `deny.toml`'s `advisories.ignore` is
`[]`, so cargo-deny re-raises them.

The recorded justification claims:

> Parent trace (**sole path**): quick-xml 0.39.4 ← plist 1.9.0 ← syntect 5.3.0 ← wcore-cli.
> Threat model — **UNREACHABLE**: … No user-supplied … XML parsing anywhere in the workspace.

Measured, there are **three** consumer paths:

| Path | Named in the justification? |
|---|---|
| quick-xml 0.39.4 ← plist ← syntect ← wcore-cli | yes |
| quick-xml 0.39.4 ← **wcore-tools** (direct, docx/pptx OOXML) | **no** |
| quick-xml 0.31.0 ← **calamine 0.26.1 ← wcore-tools** (xlsx) | **no** |

`crates/wcore-tools/src/doc_tool.rs` exists to read **user-supplied** documents. calamine 0.26.1
has **25 `.attributes()` call sites** and **zero `with_checks(false)`**, so 0194's vulnerable
default duplicate-attribute check is live on attacker-supplied spreadsheets.

- **0195 (`NsReader`): the UNREACHABLE claim HOLDS** — `NsReader` appears zero times in calamine
  and zero times in the workspace.
- **0194: the UNREACHABLE claim is FALSIFIED.**

**The HIGH attaches to the control failure, not the exploitability.** The impact is a local CPU
DoS (~6s per 80k-attribute tag, by the advisory's own table) with no data loss and no privilege
escalation — MEDIUM in isolation. But a security exception was granted and renewed across two
gating scanners on a trace that omits the reachable consumer.

**Cross-audit panel: codex 5.6 HIGH, kimi K3 HIGH, gemini 3.1 Pro MEDIUM (2–1).** Gemini's
dissent is recorded, not dropped: it argues severity must track actual risk and that the
procedural flaw belongs in backlog. The majority position, adopted here, is that an exception
process able to certify a false threat model must be corrected before the phase closes.

**NOT PROVEN HERE:** no malicious `.xlsx` was crafted and no CPU cost was measured end to end.
This is a static reachability argument from the real graph and the real crate sources.
**Substitution point:** a PoC document driven through `doc_tool`, by whoever owns `wcore-tools`.

---

## 2. SBOM — F29-CEN-05 closed

*Evidence: `sbom-determinism.txt`; fixture `crates/wcore-fixture-harness/fixtures/f29/`*

865 components from the real locked graph, 447,364 bytes, sha256
`5028fe289ceb05f39578c6e8848e8e91dba5aaef22a526a5396858f2a8ac9d1a`.

| Property | Result |
|---|---|
| same-path regeneration byte-identical | **YES** |
| **cross-path** byte-identical (two checkouts, differing metadata inputs) | **YES** |
| components purl-sorted | **True** |
| `file://` / `manifest_path` leaks | **0 / 0** |
| `metadata` keys | `['tools']` — no timestamp |
| explicit-unknown licences | 2 (`wcore-fixture-harness`, `workspace-hack`) — present, not omitted |
| origins | 808 registry, 57 workspace-member, **0 git** |

**The defect the measurement caught.** The first implementation derived `serialNumber` from a
digest of the **raw** `cargo metadata` text. The contract suite passed, the pinned fixture
reproduced, and same-directory regeneration was byte-identical — and it was still wrong.
Generating from the same commit at `/root/wayland-29-02` and `/root/wl29-pathb` produced
documents of identical length differing at **byte 83**: the serial, because the raw text embeds
the checkout path 1,538 times. Fixed by deriving the serial from the **canonical output**.
Regression-locked by `the_serial_number_is_derived_from_the_canonical_output_not_the_raw_input`,
which was RED before the fix and GREEN after.

**Honest scope:** this is an SBOM of the **locked dependency graph**, not of one built binary.
`cargo metadata` resolves across all targets, so the component set is a superset of what any
single binary links. Narrowing it would need `--filter-platform`, which would make the digest
platform-dependent — precisely what must not happen.

---

## 3. Reproducibility — measured for the first time

*Evidence: `repro-measurement.txt`, `repro-variance-class.txt`, `repro-observation-a.txt`,
`repro-observation-b.txt`*

```
F29-REPRO::DOCUMENTED-VARIANCE::a=ca35c34f…::b=8272fae8…
```

Artifact: `wayland-core`, `cargo build --release --locked --target x86_64-unknown-linux-gnu
-p wcore-cli`. Profile `lto=thin codegen-units=1 strip=debuginfo overflow-checks=true`.
Disk before the run: 919G free of 1.8T.

| Observation | What varied | Digests | Result |
|---|---|---|---|
| **A** | two clean target dirs at **different** paths | `ca35c34f…` / `d6cd3154…` | **DIFFER** |
| **B** | one target dir path, **wiped between builds** | `8272fae8…` / `8272fae8…` | **IDENTICAL** |

**Single-variable.** Same commit, lockfile, toolchain, `--locked`, clean target directory every
time. The only thing that changed between A and B is whether the two builds shared a target-dir
path. A differs, B is byte-identical ⇒ **the varying input is the absolute build-directory path**
(`VarianceClass::PathPrefix`).

**Mechanism, located:** seven `OUT_DIR` paths from `cranelift-codegen`'s build script (arriving
via wasmtime) reach the binary through `file!()` in ISLE-generated sources. They also land in a
different order in each build, which is why the size delta (408 bytes) exceeds what equal-length
path substitution alone would produce — the ELF string table tail-merges, so ordering changes
suffix sharing. The differing GNU build-id is a *consequence*, not an independent cause:
observation B shows it is stable when the path is stable.

**Staleness guard.** Each build took 320s and exited 0; none was a cache hit. Every target dir
was created empty immediately before its build, and observation B *wipes* the directory between
builds, so B's agreement cannot come from reusing b1's artifacts.

**What this means for the shipped release.** It is reproducible in practice but **only
accidentally**: GitHub-hosted runners always check out at
`/home/runner/work/wayland-core/wayland-core`, so `release.yml` holds the varying input constant
without intending to. A third party rebuilding anywhere else gets a different digest and cannot
confirm the artifact — which is what reproducibility is *for*.

**Remedy, not applied (release.yml is outside this fence):** `--remap-path-prefix` for the target
and source roots, or `trim-paths` once stable. Either should move this verdict to REPRODUCED.

---

## 4. Everything bound into a signed manifest

*Evidence: `manifest-verify-pristine.txt`, `manifest-verify-mutated.txt`*

Throwaway trust root generated at run time; all four seed files mode `0600`; only public keys
printed. Artifact store: the real 35MB release tarball, its checksums, and the real 447KB SBOM.

```
sbom             : observed  sha256=5028fe28…  format=cyclone_dx_json
dependency_policy: observed  tool=cargo-deny  policy_sha256=05a8535b…  result=fail
reproducibility  : variance  class=path_prefix  evidence_sha256=9a9cc834…
certification    : unavailable (phase_28_certification_binding_not_yet_available)
body_sha256      : 00d5294c…
F29-MANIFEST-VERIFY::PRISTINE::exit=0
```

Then one bit of the SBOM was flipped (offset 40000, `0x6e → 0x6f`; `cmp -l` reports **1**
differing byte), the manifest body was rebuilt over the mutated store, and that body was spliced
into the **already-signed** manifest keeping the original signature and `body_sha256`:

```
signed  body_sha256: 00d5294c…
mutated body_sha256: 2e9ced38…
wayland-release: body digest mismatch
F29-MANIFEST-VERIFY::MUTATED-SBOM::exit=1
```

The pristine control is recorded **first** and was accepted, so the refusal is about the
mutation and not about a verifier that rejects everything.

Note the manifest honestly carries `result=fail` for the dependency policy. That is the design
working: the manifest records what was measured, not what one would prefer.

---

## 5. Eight-clause coverage

*Machine-readable: `evidence/29-02/CLAUSE-LEDGER.tsv`*

| F29-01 clause | Status | Platform | Settled by | Evidence |
|---|---|---|---|---|
| Toolchain lock | PRE-EXISTING | source-tree | `rust-toolchain.toml` 1.95.0 + `vx.toml` + 7 workflows via `loonghao/vx` | `pre-existing-controls.txt` |
| Dependency lock | PRE-EXISTING | source-tree | `Cargo.lock` committed, 1,017 crates, **0** git sources | `pre-existing-controls.txt` |
| Vulnerability policy | PRE-EXISTING | linux-x86_64 | `cargo audit` in `ci.yml` ×3 + `osv-scan.yml` — **but see F29-02-H1** | `deny-verdict.txt` |
| License policy | **MEASURED** | linux-x86_64 | `cargo deny check` — **FAILED**, first-ever execution | `deny-verdict.txt` |
| SBOM | **MEASURED** | linux-x86_64 | 865 components, byte-deterministic across paths | `sbom-determinism.txt` |
| Provenance | PRE-EXISTING | github-actions | `actions/attest-build-provenance@v4`, `npm publish --provenance` | `pre-existing-controls.txt` |
| Artifact signing | PRE-EXISTING | github-actions | keyless Sigstore; updater fails closed on `gh attestation verify` | `pre-existing-controls.txt` |
| Reproducibility | **MEASURED** | linux-x86_64 | DOCUMENTED-VARIANCE, class `path_prefix` | `repro-measurement.txt` |

The four PRE-EXISTING rows are **named, not assumed**: `pre-existing-controls.txt` re-measures
each directly against the tree rather than citing 29-01's census.

---

## 6. NOT PROVABLE HERE — with substitution points

- **Whether the SBOM is byte-identical on macOS and Windows.** The transform is pure and reads no
  platform-varying input, and determinism is proved on Linux plus offline against the pinned
  fixture — but no macOS or Windows run was taken, because this lane may not run cargo on the
  Mac. **Substitution:** the new `supply-chain.yml` on a matrix, or Phase 28's native hardware.
- **Whether the `path_prefix` variance is the ONLY variance on macOS and Windows.** Needs Phase
  28's hardware. The aarch64-Linux leg goes through `cross` (F29-CEN-06) and was not measured.
- **Whether RUSTSEC-2026-0194 is exploitable end to end.** Static reachability only; no PoC
  document was crafted. **Substitution:** a PoC through `doc_tool`, owned by `wcore-tools`.
- **Whether GitHub's attestation would cover an SBOM asset.** Depends on a `subject-path` in
  `release.yml`, which this plan does not touch. 29-01 named this plan as the substitution point;
  it turned out to be behind this plan's own fence too. **Escalated as part of SR-29-7.**
- **The new workflow has never been dispatched.** It is authored and validated as a document —
  parsed by a real YAML parser, job-and-step structure asserted. No workflow run, token or
  published release was needed and none was used.

**No gate in this plan can be passed by supplying a credential, and none requires one.**
