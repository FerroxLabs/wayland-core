# 28-receipt NOTES — running log (append after every measurement, §6b-i)

Lane `lane/28-receipt`. Base `1b9f148f046967ba019db6bd648157dba98f82d7` (captured once).

## M1 — the contradiction, measured (not restated)

| Source | F-28-02-002 disposition |
|---|---|
| `evidence/28-04/findings.tsv` line 39 | `FIXED` |
| `28-04-CERTIFICATION-RECEIPT.json` `body.findings[]` | `OPEN` |

- receipt `body_sha256` = `2037352cff1c2f2c8f8b35e59289ba73b514cd56977c8e22d599ed45e49e0fbb`
  (matches the value named in the lane brief — the brief's grounding is correct)
- receipt `authority.key_id` = `phase-28-certification-2026-07-28`
- receipt claims as signed:
  - `zero_skipped_critical_cases` = **true**
  - `zero_undispositioned_findings` = **false**
  - `zero_unresolved_critical_or_high` = **false**
  Both falses are caused by exactly one row: F-28-02-002 OPEN at signing time.
- ledger now: 63 rows, dispositions 28 ACCEPTED / 18 DEFERRED / 10 FIXED / 7 DISPROVED.
  **Zero non-terminal rows.** So a recomputation over today's ledger yields all three claims TRUE.
- `findings.tsv` sha256 at time of measurement =
  `51ddac033dc99a4b1b4d06d3b247b2a4287362b2aae12a9fb83f9513a243e75a`

## M2 — F-28-ADJ-001 / F-28-ADJ-002 are NOT ledger rows

`awk -F'\t' '$1 ~ /^F-28-ADJ/'` over findings.tsv returns **nothing**. The single grep hit for
"F-28-ADJ" is a *mention inside F-28-02-002's rationale prose*, not a row.

Consequence, and it is the sharp edge of this lane: a receipt rebuilt from today's ledger
asserts three TRUE claims while two real MEDIUM findings exist and are open in lane 28-adj2.
The three claims are individually true *as defined* (they range over the ledger), but a reader
who sees three greens and no mention of ADJ-001/002 has been handed "zero known defects" by
implication — which Amendment A3 forbids. The superseding receipt must therefore name them
explicitly somewhere that is NOT `body.claims` (the A3 allowlist is enforced on that key only).

Open question for M3: does the Rust `CertificationVerifier` reject unknown `body` fields? If it
deserializes strictly, I cannot add a disclosure field and must find another carrier.

## M3 — tooling inventory (read, not assumed)

- `.planning/scripts/f28-build-receipt.py` — the issuing path. **Hardcodes** `KEY_ID`,
  `CERT_ID`, and the output path `28-04-CERTIFICATION-RECEIPT.json`. Key is *deterministic*:
  `seed = sha256("wayland.phase28.certification." + CERT_ID)`. So re-running it as-is would
  OVERWRITE the original receipt in place — forbidden. A superseding receipt needs a distinct
  cert id (hence a distinct key), a distinct key_id, and a distinct output path.
- `.planning/scripts/f28-verify-bindings.py --verify <path>` — takes an ARBITRARY receipt path
  and sets `base = path.resolve().parent`. So a superseding receipt placed in the same phase
  directory is verifiable by the same command, unmodified. It recomputes bindings/claims off
  raw evidence. **It does NOT check the signature.**
- `crates/wcore-eval-scenarios/src/receipt.rs` — `CertificationVerifier`; owns digest +
  Ed25519 signature + schema. This is the only thing that verifies a signature. Rust ⇒ hetzner
  (no cargo on the Mac).

## M4 — schema strictness: the disclosure CANNOT be a new body field

`CertificationBodyV2`, `CertBindingsV2`, `CertFindingV2` are all `#[serde(deny_unknown_fields)]`.
So a `supersedes:` key added to `body` is an immediate parse failure in the Rust verifier.
Carriers that ARE available inside the signed body:

- `bindings.posture[]` — `{name, description, evidence_ref}`, free text. Python `--verify` only
  requires posture to be NON-EMPTY; it resolves refs for `fixture_corpus` only. Rust requires
  each field non-empty. **This is the carrier for both the supersession record and the ADJ
  disclosure**, and it is inside the digest, so the statement is itself signed.
- `bindings.artifacts[]` — python recomputes each listed entry's sha256/bytes off disk but does
  NOT require the list to be exhaustive. So the ORIGINAL receipt file can be bound as an
  artifact, making the superseding receipt cryptographically name the exact bytes it supersedes.
  Its path resolves because `base = receipt.resolve().parent` = the phase dir.

Rust rules that constrain the F-28-02-002 row (checked against the ledger, all satisfied):
`disposition == "FIXED"` requires a NON-EMPTY `executable_check` (`F28R-REPAIREVID`) — the TSV
row carries one (the SeanDesktop re-measurement, 133/0/23 + mutant M3). `origin` must be
non-empty — it is `control`. Row has NF=13, so `downgrade_review` pads to `""`; harmless.

## M5 — the acceptance gate, and why re-signing changes its verdict

`cert_acceptance_gate` = AND of the three claims. Original receipt = `false` (two claims false).
Recomputed over today's ledger all three are true ⇒ a superseding receipt reports
`acceptance_gate_passed = true`. That is a REAL change in verdict, not a cosmetic one, and it is
the reason this lane is not a documentation edit.

## M6 — Phase 29 does NOT consume `body.findings`. MEASURED, not assumed.

28-04-SUMMARY.md:310 asserts "Phase 29 must consume `body.findings` in the signed receipt".
**It does not.**

- `grep -rn "findings" .planning/phases/29-*/` returns only prose about Phase 29's OWN findings.
  No reference to the certification receipt's findings list anywhere in Phase 29.
- `CertificationBindingV1` (`crates/wcore-eval-scenarios/src/release_integrity.rs:393`) is the
  struct Phase 29 actually binds. Its fields are: `receipt_body_sha256`, `receipt_schema`,
  `receipt_schema_version`, `receipt_signing_key_id`, `source_commit`, `binary_sha256`,
  `target_os`, `target_architecture`. **There is no findings field.**
- 29-01-RECEIPT-INTERFACE.md consumes `EvidenceReceiptV1` / `wayland.eval.receipt` /
  `AuthorityClaimV1::Ci` — the **v1 per-run eval receipt**, a DIFFERENT artifact from the v2
  `wayland.cert.receipt` this lane is repairing.
- Phase 30's `check-staleness.sh` touches the receipt with `test -f` only. Not a semantic reader.

Two consequences, and they point opposite ways:

1. **Blast radius is smaller than feared.** No consumer reads the stale `disposition: OPEN`, so
   the contradiction did not silently corrupt a downstream decision.
2. **The accounting control has no consumer at all** — exactly the failure 28-04-SUMMARY.md
   predicted for itself in the same sentence ("worth less than it looks"). That is a live gap,
   and it is NOT this lane's to fix.

What Phase 29 *does* pin is `receipt_body_sha256` + `receipt_signing_key_id`. A superseding
receipt has BOTH new. So any future release manifest must pin the superseding pair, not the
original — that is a seam consequence to surface, not to act on unilaterally.

## Still to establish

1. Prove the verifier REJECTS a tampered superseding receipt before trusting it accepting a good
   one (the instrument-carries-the-defect trap).
2. Rust verification of both receipts — needs hetzner (no cargo on the Mac).
