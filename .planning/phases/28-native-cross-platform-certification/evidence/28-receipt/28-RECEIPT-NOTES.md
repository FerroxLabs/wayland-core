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

## Still to establish

1. receipt.rs schema strictness + how signature verification is invoked (test? binary?).
2. Whether Phase 29 actually consumes `body.findings` from the signed receipt (28-04-SUMMARY.md:310
   claims it must) — check, do not assume.
3. Prove any verifier I rely on can REJECT a tampered superseding receipt before trusting a pass.
