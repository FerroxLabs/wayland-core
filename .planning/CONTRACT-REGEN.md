# CONTRACT-REGEN — the authorised single regeneration over the merged tree

Lane `lane/contract-regen`. Base `plan/f20-unified-audit-repair` @ `8bcb052b`.
Regenerated once with `wcore-contract generate` on `hetzner-dsm`, per `CLASS-CONTRACT-01`.
No other lane may repeat this.

---

## 1. FOR SEAN / DESKTOP — hand this over first

`crates/wcore-protocol/src/contract/observation.rs:342-343` compares
`descriptor.source_inputs_digest` and returns
`HostObservationError::SourceInputsDigestMismatch` on the `ready` event. It is a **hard
error at negotiation**. (I was pointed at `:329`; the check is at `:342-343` at this
commit — same code, the file has moved under the reference.) A Desktop pinned to the old digest will **refuse to start against a
Core built from this commit**, and vice versa. The two must ship on the same release train.

**The descriptor Desktop must now expect:**

| field | value | moved? |
|---|---|---|
| `name` | `wayland-desktop-core` | no |
| `major` / `minor` | `1` / `8` | no |
| `generator` | `wcore-desktop-contract-gen/11` | no |
| `schema_digest` | `sha256:e5d1744aa6cadc46d2707a1fa190ac80ee74f13477d685bb9146a71b3fff2e54` | **no** |
| `fixture_digest` | `sha256:634bbbe9…` → `sha256:de2b19bdf52ea9ef2934a4b0fa43d5cd54befb6f1e0d7b4b2e2af60154723bb8` | **YES** |
| `source_inputs_digest` | `sha256:25170996…` → `sha256:c99443599a273e00c72900b12f32aa371d27b83f30e8a0f5a13f0c2191380562` | **YES** |

`CONTRACT_MINOR` is **not** bumped, because nothing about the wire moved (§3). This is a
provenance re-pin, so it is a re-pin Desktop must mirror, not a migration it must implement.

**Also on this train:** `GET /openapi.json` now emits **OpenAPI 3.1.0** instead of 3.0.3
(utoipa 4→5, taken to drop `proc-macro-error` / RUSTSEC-2024-0370). This is in `wcore-acp`,
**not** in the Desktop contract — see §6. It does not interact with the digests above.

---

## 2. Before state

`cargo test -p wcore-protocol --test desktop_contract_corpus` at `8bcb052b`:

```
test result: FAILED. 14 passed; 1 failed; 0 ignored; 0 measured; 0 filtered out
---- checked_corpus_matches_real_serializers_byte_for_byte
Desktop contract corpus drift: missing=[], extra=[], drifted=[
  "adversarial/events/fixture-mismatch.jsonl",
  "adversarial/events/schema-mismatch.jsonl",
  "adversarial/events/version-mismatch.jsonl",
  "events/ready.json",
  "manifest.json"]
WLRC=101
```

Evidence: `.planning/evidence/contract-regen/01-before-corpus.log`.

### Why it was red — measured, not inferred

`5f74d559` was the last authorised re-stamp. 475 commits later, **11 commits touched a
SOURCE_INPUTS path and all 11 moved `source_inputs_digest`:**

| commit | source inputs touched |
|---|---|
| `793bead9` | bootstrap.rs, **main.rs** |
| `85b60a2f` | bootstrap.rs, protocol_sink.rs |
| `27d24bef` | bootstrap.rs |
| `bf959017` | bootstrap.rs — **a doc comment moved nine lines down. Nothing else.** |
| `9fe6ad86` | bootstrap.rs |
| `e41dbd0e` | bootstrap.rs |
| `bce987fa` | bootstrap.rs |
| `743e52bb` | **main.rs** |
| `b18ecc1f` | bootstrap.rs |
| `f38272f8` | bootstrap.rs |
| `71315c03` | bootstrap.rs |

**One correction to the framing I was given.** The dominant driver is **not** `main.rs`
(2 of 11) but `crates/wcore-agent/src/bootstrap.rs` (**9 of 11**). `main.rs` being the
lane-fence file is real but secondary; `bootstrap.rs` is a large engine-bootstrap file that
every channel/provider/policy fix lands in. The conclusion the framing draws is right; the
attribution is off, and the recommendation in §5 changes because of it.

`bf959017` is the cleanest single data point. Its whole diff against the digest input set:

```
-/// Builder for creating a fully-initialized `AgentEngine`.      (9 lines removed above fn)
+/// Builder for creating a fully-initialized `AgentEngine`.      (9 identical lines re-added below fn)
```

Zero executable change. It moved a Desktop-facing cryptographic digest.

---

## 3. What changed in the regeneration, and why

`wcore-contract generate` → rc 0. **Exactly the same 5 files, 0 added, 0 deleted.**

`git diff` cannot answer the question that matters here: these fixtures are **one line of
JSON each**, so a digest re-stamp and a smuggled wire change both report `1 insertion,
1 deletion`. I wrote a JSON-leaf differ
(`.planning/scripts/contract-regen-diff.py`) that parses both revisions, flattens to leaf
paths, and classifies every differing leaf as digest or shape. Its self-test has the three
assertions §6b-ii requires — including A3, which proves `git diff --numstat` emits an
identical `1\t1\tfixture.jsonl` for both cases, i.e. that the instrument it replaces
**would have missed a shape change**. Self-test 3/3
(`.planning/evidence/contract-regen/03-differ-selftest.txt`).

### File by file

| file | bytes | verdict | leaves that moved |
|---|---|---|---|
| `events/ready.json` | 1757 → 1757 | **DIGEST-ONLY** | `contract.fixture_digest`, `contract.source_inputs_digest` |
| `adversarial/events/fixture-mismatch.jsonl` | 1757 → 1757 | **DIGEST-ONLY** | `contract.source_inputs_digest` only¹ |
| `adversarial/events/schema-mismatch.jsonl` | 1757 → 1757 | **DIGEST-ONLY** | `contract.fixture_digest`, `contract.source_inputs_digest` |
| `adversarial/events/version-mismatch.jsonl` | 1757 → 1757 | **DIGEST-ONLY** | `contract.fixture_digest`, `contract.source_inputs_digest` |
| `manifest.json` | 19326 → 19326 | **DIGEST-ONLY** | `fixture_digest`, `source_inputs_digest` |

¹ correct: this fixture's `fixture_digest` is deliberately poisoned to `ffff…`, so only the
source digest is live in it. The three adversarial files are negative fixtures built by
mutating one digest of the real `ready` descriptor, so they necessarily carry the other two.

**`0 shape leaves` across all five.** `counts`, `source_inputs`, `capabilities`,
`fixture_inventory`, `subcontracts`, every command and event spec: unchanged. Every file is
**byte-length identical** before and after, which is what a pure hex-digest swap looks like.

**`schema_digest` did not move** — `e5d1744a…` was pinned at `5f74d559` and recomputes to
`e5d1744a…` now. The schema is what defines the shape, so this is the independent
confirmation that no shape moved. `fixture_digest` moved **only as a cascade**: `ready.json`
is a fixture and embeds the descriptor, so re-stamping the source digest inside it
necessarily re-hashes the fixture set.

**Answering the question I was told to stop on:** no change here is a genuine shape change.
Nothing is being baked in. This is a two-value re-stamp across five files.

**Provenance of the committed bytes.** I generated on hetzner and copied the five files to
the Mac. All five have **byte-identical git blob hashes** on both machines
(`3e2360c1…`, `4c701162…`, `9a224881…`, `30e415b5…`, `3a481ca1…`), so what is committed is
verbatim generator output, not a hand-edit that happens to look right.

---

## 4. After state, and proof the guard can still fail

Run on hetzner, **by test file, never by filter**, with executed counts read back:

| test file | result | rc |
|---|---|---|
| `desktop_contract_corpus` | **15 passed; 0 failed; 0 ignored** (was 14/1) | 0 |
| `desktop_contract_adversarial` | 17 passed; 0 failed; 0 ignored | 0 |
| **`golden_v0_1_21`** | **22 passed; 0 failed; 0 ignored** | 0 |
| `host_decoder_contract` | 31 passed; 0 failed; 0 ignored | 0 |
| `approval_resume_contract` | 4 passed; 0 failed; 0 ignored | 0 |

`golden_v0_1_21` pins the wire independently of the corpus and is **unchanged and green**,
which is the second independent check that no shape moved.

`0 ignored` everywhere, so none of the three vacuity flavours applies — not all-`#[ignore]`d,
not an env-gated early return, and not a filter matching zero tests (I ran files, not names).

### Mutation test — a corpus that passes because it matches anything is worthless

| probe | mutation | result |
|---|---|---|
| M-A | one byte in `events/ready.json` (`session-desktop-001`→`002`) | **FAILED** rc 101, drift `[ready.json]` |
| M-B | **one comment line appended to `crates/wcore-agent/src/bootstrap.rs`** | **FAILED** rc 101, drift = all 5 files |
| M-C | both restored | 15 passed, rc 0 |

Evidence: `.planning/evidence/contract-regen/06-mutation-test.log`.

The guard is still live in both directions: it catches corpus tampering (M-A) and it catches
source drift (M-B). M-B is also the whole of §5 in one line — a **comment**, in a **different
crate**, with no wire effect, reddens all five Desktop contract files.

---

## 5. Recommendation on the input set — RECOMMENDATION ONLY, NOT CHANGED IN THIS LANE

I did not touch `SOURCE_INPUTS`. It is still the same 40 files. This is a separate decision.

**Cross-audit panel (§4), asked whether a raw-byte digest over 40 whole files, enforced as a
hard startup error, is the right design:**

| panel | position |
|---|---|
| codex `gpt-5.6-sol` | **C** — demote to advisory; the digest proves provenance, not wire compatibility |
| gemini `3.1-pro` | **C** — if schemas and fixtures are unchanged, the contract is unchanged |
| kimi K3 | **D** — semantic/AST digest; explicitly called C "a reasonable stopgap" |

**All three rejected A (keep as-is) and, notably, all three rejected B (just drop `main.rs`).**
B is the option the framing pointed me at, and the panel is right to reject it: my own
measurement says `bootstrap.rs` drives 9 of 11 movements, so dropping `main.rs` alone fixes
about 18% of the noise and leaves the problem intact.

**Internal adversarial pass, arguing against the C consensus — and it failed, honestly.**
My counter was: demoting the source digest removes the only tripwire for a producer
*behaviour* change that leaves the *shape* alone, and `85b60a2f` ("advertise browser/CUA
capabilities on liveness, not linkage") looked like exactly that case — it changed what
`capabilities.browser_suite` reports at runtime while `schema_digest` stayed put. I checked
it rather than asserting it. The change is **within the already-declared value domain**
(`false` was always a reachable value for that field, and the commit reasons this through
explicitly). So it is not a contract change, and the digest firing on it was noise, not
signal. **I did not find a single case where `source_inputs_digest` caught something the
other two digests should have caught.** The consensus survives the attack.

**My recommendation, which is C with one refinement the panel did not raise:**

> The defect is not the file list — it is that **one digest serves two consumers with
> opposite precision needs**. CI wants maximum sensitivity and a red there is cheap and
> actionable. A running Desktop client wants wire compatibility, and a false positive there
> is a product that will not launch. Split them: keep `source_inputs_digest` computed over
> all 40 files and keep it a **hard failure in the corpus test**, but make it **advisory and
> logged at `ready`** instead of `HostObservationError::SourceInputsDigestMismatch`. Leave
> `schema_digest` and `fixture_digest` as hard errors, since their precision matches the
> consequence.

That keeps the forensic value ("exactly which source state built this binary"), keeps CI
honest, and stops a doc-comment move from bricking Desktop startup. Narrowing the file list
(B) or AST-hashing (D) can follow later; neither is needed to remove the harm, and D is real
implementation work.

**Answering the question as posed:** no, `main.rs` should not be in the 40 — but removing it
is not the fix, because `bootstrap.rs` is nine times the problem and removing *that* would
gut the digest's purpose. The precision mismatch is at the enforcement point, not the input
set.

---

## 6. OpenAPI 3.1.0 — not touched by this regeneration

- `grep -rl openapi crates/wcore-protocol/contracts/` → **0 files**.
- **None of the 40 SOURCE_INPUTS mentions openapi.**
- The 3.0.3 → 3.1.0 move lives entirely in `wcore-acp` (`transport/rest.rs`).

So the regeneration does not touch it, and it cannot move any of the three digests.

**One correction:** the endpoint is **not** uncovered. There is no byte *fixture*, but there
are two live assertions, one of them over a real listener, and both are green at this commit:

```
rest_openapi_doc_served_over_live_listener ... ok      (wcore-acp --test rest_roundtrip: 2 passed)
transport::rest::tests::get_openapi_json_has_paths_and_resolves_schemas ... ok
                                                       (wcore-acp --lib: 129 passed)
```

Both assert `doc["openapi"].starts_with("3.1")` — an updated fact, exactly as strict as the
`starts_with("3.0")` it replaced. Evidence:
`.planning/evidence/contract-regen/07-openapi-acp.log`.

---

## 7. Scope, and what I did NOT do

Changed, versus merge-base `8bcb052b` (captured once, quoted):

```
1 1  crates/wcore-protocol/contracts/desktop/v1/adversarial/events/fixture-mismatch.jsonl
1 1  crates/wcore-protocol/contracts/desktop/v1/adversarial/events/schema-mismatch.jsonl
1 1  crates/wcore-protocol/contracts/desktop/v1/adversarial/events/version-mismatch.jsonl
1 1  crates/wcore-protocol/contracts/desktop/v1/events/ready.json
1 1  crates/wcore-protocol/contracts/desktop/v1/manifest.json
     + .planning/ notes, evidence and one script
```

- **Zero `.rs` files changed** in this lane.
- **Shared fence untouched:** `git diff $BASE -- crates/wcore-cli/src/lib.rs
  crates/wcore-cli/src/main.rs` → 0 lines.
- `.github/workflows/ci.yml` and `.planning/BACKLOG.md` untouched, as instructed.
- **`SOURCE_INPUTS` not modified.** §5 is a recommendation.
- **`observation.rs` not modified.** The hard error at `ready` is still a hard error.
- No merge, no PR, no tag, no issue closed, no full-workspace run.
- No credential was needed, so none was used.
