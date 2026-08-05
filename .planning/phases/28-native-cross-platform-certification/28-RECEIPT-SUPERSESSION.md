# 28-RECEIPT-SUPERSESSION — making the Phase 28 record stop contradicting itself

**Lane:** `lane/28-receipt` · **Date:** 2026-07-29 · **Base:** `1b9f148f`

---

## 1. The contradiction, as measured

| Artifact | `F-28-02-002` |
|---|---|
| `28-04-FINDING-LEDGER.md` | `FIXED` |
| `evidence/28-04/findings.tsv` line 39 | `FIXED` |
| `28-04-CERTIFICATION-RECEIPT.json` (**signed**) | `OPEN` |

The signed artifact was the wrong one, and it was wrong under an Ed25519 signature over
`body_sha256 = 2037352cff1c2f2c8f8b35e59289ba73b514cd56977c8e22d599ed45e49e0fbb`.

This is not merely cosmetic. The receipt's acceptance gate is the AND of its three A3 claims, and
two of them were `false` **because of that single row**:

```
zero_skipped_critical_cases        true
zero_undispositioned_findings      false   <- caused by F-28-02-002 OPEN
zero_unresolved_critical_or_high   false   <- caused by F-28-02-002 OPEN
```

The tooling already detects this without being told. Running the phase's own verifier against
today's evidence:

```
f28-verify-bindings.py --verify 28-04-CERTIFICATION-RECEIPT.json     rc=1  REJECTED (4)
  F28V-CLAIM  body.claims.zero_undispositioned_findings:    receipt asserts False, recomputed True
  F28V-CLAIM  body.claims.zero_unresolved_critical_or_high: receipt asserts False, recomputed True
  F28V-ARTIFACT artifacts[evidence/28-04/findings.tsv].sha256: 511e19dd… vs 51ddac03…
  F28V-ARTIFACT artifacts[evidence/28-04/findings.tsv].bytes:  55568 vs 58035
f28-verify-bindings.py --check-enumeration …                         rc=1
  F28V-ENUM  F-28-02-002.disposition: receipt says 'OPEN', ledger says 'FIXED'
```

**The original receipt's integrity is intact; its currency is not.** Those are different
properties and the distinction is the whole of this lane. The Rust verifier — which checks digest
and signature — still accepts it. The Python verifier — which recomputes against today's raw
evidence — rejects it.

## 2. The decision, and the challenge it was given

The adjudicating lane called re-issuing the receipt a release action reserved to Sean. **I was
told that call had been overturned, and instructed to challenge it if the tooling or the contract
contradicted the overturn. It does not.** Checked, not assumed:

- The receipt's own `authority.scope` reserves exactly four things — *"tagging, releasing, merging
  and issue closure are reserved to Sean"* — and names itself *"NOT a release trust root, NOT a
  seal, and NOT an authorization to release"*. Issuing a phase-scoped evidence receipt is none of
  the four.
- `receipt.rs` encodes the same separation in code: `CertAuthority::PhaseScopedSigned` is
  explicitly *not* authority, and `VerifiedCertification.acceptance_gate_passed` carries the
  comment *"This is NOT 'no defects'"*.
- `f28-build-receipt.py`'s key is **deterministic**, derived from the certification id. It is
  re-derivable by any later reader without this machine. A key anyone can re-derive is, by
  construction, not a release trust root — the builder says so itself, and says Phase 29's trust
  root must not be derived this way.

So the overturn holds. **One thing did need saying, and I have acted on it rather than merely
noting it:** the concern behind the original refusal was real, just misaimed. The danger is not
issuing a new receipt — it is *overwriting the old one*. The tool permitted exactly that. It no
longer does (§4).

## 3. What was issued

`28-04-CERTIFICATION-RECEIPT.json` is **byte-identical** to what it was; nothing in this lane
writes to it. Beside it:

**`28-04-CERTIFICATION-RECEIPT-SUPERSEDING-001.json`**

| | Original | Superseding |
|---|---|---|
| `certification_id` | `f28-native-cross-platform-certification` | `…-supersession-001` |
| `key_id` | `phase-28-certification-2026-07-28` | `phase-28-certification-supersession-001-2026-07-29` |
| key fingerprint | `f0ef7d06…` | `20e970e9…` |
| `body_sha256` | `2037352cff1c…e49e0fbb` | `8db1ef07600f…46116fb` |
| `F-28-02-002` | `OPEN` | `FIXED` |
| **acceptance gate** | `false` | **`true`** |
| findings | 63 | 63 |

It carries **its own key** (a distinct certification id yields a distinct derived key), repeats the
PHASE-SCOPED scope text verbatim, and states the supersession **inside the signed body** rather
than in a sidecar nobody digests:

- `bindings.posture[]` → `supersedes-a-prior-signed-phase-28-receipt`, naming the superseded
  `body_sha256` and `key_id` in full, and stating that the superseded receipt is *not* withdrawn,
  *not* altered and *not* invalid.
- `bindings.artifacts[]` → the superseded receipt's **exact bytes** (`c4ab82ae…`, 97795 bytes), so
  the supersession is a binding rather than a claim. §5 proves this bites.
- `bindings.logs[]` → the `28-adj` adjudication transcripts (panel votes, the M3 mutation log, the
  gate-falsification log), so the evidence for the changed disposition travels with the receipt.
  Log bindings went 47 → 53, artifacts 23 → 24.

### Amendment A3 — three true claims are not "zero known defects"

The adjudication itself opened two MEDIUM findings, and **neither is a row in
`evidence/28-04/findings.tsv`** (verified: `awk -F'\t' '$1 ~ /^F-28-ADJ/'` matches nothing; the
lone grep hit is a mention inside another row's prose). So a receipt rebuilt from that ledger
asserts three trues while two real defects exist — the precise shape A3 forbids.

They are therefore named explicitly in the `posture` binding, inside the signature:

- **`F-28-ADJ-001`** — the residual-grant disclosure is guarded by nothing. Mutant M3 deletes it
  and the suite stays byte-identical at `133 passed 0 failed 23 ignored`.
- **`F-28-ADJ-002`** — **the same permanent-wedge shape as `F-28-02-002`, through a different
  door**: a crash between `create_new_nofollow` and `write_and_sync` leaves a 0-byte `.toml` that
  aborts recovery on every subsequent `ExecutionIdentity::start`. Stated as a static reading, not
  reproduced.

`F-28-ADJ-002` is the one that matters to a reader of the verdict: **`F-28-02-002` being FIXED does
not mean the wedge class is eliminated.** The receipt says so in its own body. Their absence from
the ledger is recorded as a gap in the ledger, not as evidence of their absence.

## 4. The tooling — found and used, not hand-assembled

`.planning/scripts/f28-build-receipt.py` is the real issuing path, and it is a supported command,
not a one-shot script. It was **parameterised, not duplicated** (`--supersede`, `--ledger`,
`--cert-id`, `--key-id`, `--out`, `--extra-evidence-dir`, `--disclose`). Nothing was
hand-assembled and no signature was hand-waved.

Two things were added because the gap was structural, not incidental:

1. **The builder now refuses to overwrite a phase-scoped-signed receipt.** Before this lane,
   `f28-build-receipt.py` with no arguments would have silently rewritten the signed artifact over
   a moved ledger — which is precisely the failure this lane exists to repair, sitting one command
   away at all times. It now exits 2 and points at `--supersede`.
2. **`--supersede` reads the superseded digest and key id out of the file itself** rather than
   accepting them as typed arguments, so the supersession cannot record a digest that was
   mistyped.

**The refactor is proven behaviour-preserving, A/B, on a pristine checkout at the commit that
signed the original** (`evidence/28-receipt/control-ab-refactor.md`):

| Leg | vs the committed receipt | `cmp` rc |
|---|---|---|
| original tool, unmodified, at `3f85026a` | byte-identical | **0** |
| refactored tool, same inputs | byte-identical | **0** |

Both reproduce `body_sha256` *and* `signature_base64` exactly. The guard was measured refusing
(`rc=2`, receipt still `cmp`-identical afterwards) **and** allowing on a fresh path (`rc=0`) — a
guard that refused everything would pass the first test and be useless.

## 5. Both receipts verify — and the verifier was proven able to say no first

**Rust (`hetzner-dsm`, by file, never by filter):**

```
cargo test -p wcore-eval-scenarios --test f28_receipt_contract
test result: ok. 28 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out
```

The executed count is read back, not inferred from exit status. Baseline was 26; the two new
supersession tests are named in the log. Non-vacuity is proven by `--nocapture`, because both new
tests early-`return` when the file is absent — the env-gated-vacuity shape this program has
measured:

```
verified 63 findings, gate_passed=false, unresolved CRITICAL/HIGH = ["F-28-02-002"]
superseding receipt verified: 63 findings, gate_passed=true,
  supersedes 2037352cff1c… under phase-28-certification-2026-07-28
```

Both receipts verified in the same run, each against its own recorded key.

**Proof the new check can fail — surgical and isolated.** Appending **one newline** to the
superseded receipt (bytes change; the parsed JSON body is identical, so every pre-existing digest
check still passes):

```
27 passed; 1 failed
  the_superseding_receipt_verifies_and_names_what_it_supersedes
  'the bound digest of the superseded receipt disagrees with the file on disk'
restore -> 28 passed; 0 failed
```

Exactly one test fails, it is the new one, and the message is the new assertion. The supersession
detects any edit to the receipt it supersedes, down to a single trailing byte.

**Python probe** (`evidence/28-receipt/probe-supersession-tamper.sh`): 4 tampers rejected, 1
pristine control accepted.

> **The probe carried the defect class it hunts, and was caught doing it.** Its first run scored
> case 4 GREEN. Cause: `f28-verify-bindings.py` sets `base = receipt.resolve().parent`, and
> `resolve()` **follows symlinks** — so a symlinked receipt in the test farm silently redirected
> the entire verification at the *real* phase directory, making every planted tamper invisible.
> That is the ninth instance of this shape on this program and the first found inside an
> instrument built to catch it. Fixed by copying the receipt under test as a real file; case 4
> then failed correctly.

**Full gate set:** `--verify` superseding `rc=0` · `--verify` original `rc=1` (4 rejections) ·
`--check-enumeration` superseding `rc=0` / original `rc=1` · `--check-tamper-detection` both
`rc=0` · `--check-verdict` `rc=0` · `--check-requirements` `rc=0` · `f28-ledger.py --self-test`,
`--check-a2`, `--check-downgrades` all `rc=0` · **`--validate` strict now `rc=0` at
`allow_open=False`, 63 findings** — the transition the verdict's line 308 anticipated.

## 6. Does Phase 28's acceptance gate now pass, and against which artifact?

**Yes — against the superseding receipt, and only against it.**

- `28-04-CERTIFICATION-RECEIPT-SUPERSEDING-001.json` → `acceptance_gate_passed = true`
- `28-04-CERTIFICATION-RECEIPT.json` → `acceptance_gate_passed = false`, permanently and
  correctly, because it is a record of 2026-07-28

The ledger and the gate now agree. **This is an evidence gate, not a release gate**, and it
excludes `F-28-ADJ-001`/`-002` by construction (§3).

### What Phase 29 actually consumes — checked, not assumed

`28-04-SUMMARY.md:310` asserts *"Phase 29 must consume `body.findings` in the signed receipt"*, and
warns that if it does not, *"the accounting control has no consumer"*. The brief asked me to check
rather than assume that a stale receipt is load-bearing downstream.

**It is not, and the reason is worse than the risk.** Phase 29 does not consume `body.findings` at
all:

- `grep -rn "findings" .planning/phases/29-*/` returns only Phase 29's own findings. No reference
  to the certification receipt's list anywhere.
- `CertificationBindingV1` (`crates/wcore-eval-scenarios/src/release_integrity.rs:393`) — the
  struct Phase 29 actually binds — has fields `receipt_body_sha256`, `receipt_schema`,
  `receipt_schema_version`, `receipt_signing_key_id`, `source_commit`, `binary_sha256`,
  `target_os`, `target_architecture`. **There is no findings field.**
- `29-01-RECEIPT-INTERFACE.md` consumes `EvidenceReceiptV1` / `wayland.eval.receipt` /
  `AuthorityClaimV1::Ci` — the **v1 per-run eval receipt**, a *different artifact* from the v2
  `wayland.cert.receipt` repaired here.
- Phase 30's `check-staleness.sh` touches the receipt with `test -f` only.

Two consequences, pointing opposite ways:

1. **The blast radius was small.** No consumer read `disposition: OPEN`, so the contradiction did
   not corrupt a downstream decision. The stale receipt was not load-bearing on findings.
2. **The accounting control has no consumer at all** — exactly the failure `28-04-SUMMARY.md`
   predicted for itself in the same sentence. That is a live gap and it is **not this lane's to
   fix**; it belongs to whoever owns the Phase 29 seam.

**What Phase 29 *does* pin is `receipt_body_sha256` and `receipt_signing_key_id`, and the
superseding receipt changes both.** Any future release manifest must pin the superseding pair, not
the original. That is a seam consequence for the orchestrator to serialise, not something to act
on unilaterally here.

## 7. Honest limits

- **`--verify` on the original will now always fail**, and that is correct rather than a
  regression: two of its four rejections are the stale claims, and two are the `findings.tsv`
  artifact digest, which moved when the ledger did. A signed receipt cannot track a moving ledger.
  That is the argument for supersession, restated as a measurement.
- **The supersession prose is not machine-checked by the Python verifier.** `posture` is free text
  to `f28-verify-bindings.py`. It *is* covered by the body digest and by the new Rust test, which
  compares the named digest and key id against the file actually on disk — so the gap is closed,
  but by the Rust half, not the Python half.
- **This receipt is only as current as the ledger it was built from**
  (`findings.tsv` sha256 `51ddac03…`, 63 rows, all terminal). Lane `28-adj2` is working
  `F-28-ADJ-001`/`-002`. **If that lane adds them as ledger rows, this superseding receipt goes
  stale the same way the original did** and needs its own supersession (`-002`). That is now a
  cheap, supported operation rather than a crisis, which was the point.
- **The Rust leg ran on `hetzner-dsm` only.** No macOS or Windows execution — this is a
  document-and-verifier change with no platform-specific code.
- I did **not** merge, open a PR, tag, release, close an issue, run `wcore-contract generate`, or
  supply a credential.
