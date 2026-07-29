# CONTRACT-REGEN — running notes (append-only, committed after every measurement)

Lane `lane/contract-regen`, worktree
`/Users/seandonahoe/dev/waylandcore-frontier-worktrees/lane-contract-regen`,
branched from `plan/f20-unified-audit-repair` @ `8bcb052b`.

Authorisation: LANE-BRIEF §0 forbids `wcore-contract generate` for lanes. This lane is the
single explicit exception (`CLASS-CONTRACT-01`), regenerating **once** over the merged tree.

---

## M1 — before-state digests (Mac, no cargo)

`python3 .planning/scripts/contract-source-digest.py` at `8bcb052b`:

```
rev              <working tree>
source inputs    40
computed         sha256:c99443599a273e00c72900b12f32aa371d27b83f30e8a0f5a13f0c2191380562
pinned in manifest sha256:251709961fcc5c30c72b04fa1e7965ed472ef24c134a216df2db196b66679336
MATCH            False
rc=1
```

Manifest scalars, BEFORE:

| field | value |
|---|---|
| `generator` | `wcore-desktop-contract-gen/11` |
| `fixture_digest` | `sha256:634bbbe96bc8c0fce173165841360c7eab1f5e9d0f33a8006c99e7d71f9f30fa` |
| `schema_digest` | `sha256:e5d1744aa6cadc46d2707a1fa190ac80ee74f13477d685bb9146a71b3fff2e54` |
| `source_inputs_digest` | `sha256:251709961fcc5c30c72b04fa1e7965ed472ef24c134a216df2db196b66679336` |

Corpus is 156 files under `crates/wcore-protocol/contracts/desktop/v1/`.

The digest script's own self-test passes 3/3 including the load-bearing A3 (§6b-ii).

## M2 — still to establish

- [ ] which commits moved `source_inputs_digest` since it was last pinned (measure, do not infer)
- [ ] run `wcore-contract generate` on hetzner over `8bcb052b`
- [ ] diff the corpus file-by-file; separate digest-refresh from genuine wire-shape change
- [ ] confirm `desktop_contract_corpus` guard passes after
- [ ] confirm `golden_v0_1_21` still passes (independent wire pin)
- [ ] prove the guard can still FAIL after regeneration (mutation test)
- [ ] check whether OpenAPI 3.1.0 endpoint is in the corpus

## M3 — which commits moved the digest (measured, not inferred)

`5f74d559` was the last re-pin. 475 commits since; 11 touch a SOURCE_INPUTS path, and
**all 11 moved `source_inputs_digest`**:

| moved | commit | source inputs touched |
|---|---|---|
| MOVED | `793bead9` | bootstrap.rs, **main.rs** |
| MOVED | `85b60a2f` | bootstrap.rs, protocol_sink.rs |
| MOVED | `27d24bef` | bootstrap.rs |
| MOVED | `bf959017` | bootstrap.rs — **doc comment relocation only, 9+/9-** |
| MOVED | `9fe6ad86` | bootstrap.rs |
| MOVED | `e41dbd0e` | bootstrap.rs |
| MOVED | `bce987fa` | bootstrap.rs |
| MOVED | `743e52bb` | **main.rs** |
| MOVED | `b18ecc1f` | bootstrap.rs |
| MOVED | `f38272f8` | bootstrap.rs |
| MOVED | `71315c03` | bootstrap.rs |

Correction to the framing I was given: the dominant driver is
`crates/wcore-agent/src/bootstrap.rs` (9 of 11), not `main.rs` (2 of 11).

## M4 — regeneration (hetzner `hz/contract-regen` @ `08969a26`)

BEFORE, `cargo test -p wcore-protocol --test desktop_contract_corpus`:
`14 passed; 1 failed` — `checked_corpus_matches_real_serializers_byte_for_byte`,
drift = exactly 5 files. `WLRC=101`.

`wcore-contract generate` -> rc 0. Exactly the same 5 files modified, 0 added, 0 deleted.

| digest | before | after |
|---|---|---|
| `schema_digest` | `e5d1744a…` | `e5d1744a…` **UNCHANGED** |
| `fixture_digest` | `634bbbe9…` | `de2b19bd…` |
| `source_inputs_digest` | `2517099 6…` | `c9944359…` |

`source_inputs_digest` after == the value the Mac python script computed independently
(`c99443599a27…`), so the two implementations agree.

## M5 — the diff is a digest re-stamp, proven structurally

`git diff` is useless here: these fixtures are ONE line of JSON, so a digest re-stamp and a
wire-shape change both report `1 1`. Wrote `.planning/scripts/contract-regen-diff.py`
(JSON-leaf differ) with a 3-assertion self-test; A3 proves `git diff --numstat` reports an
identical `1\t1\tfixture.jsonl` for both cases, i.e. the old instrument could not have caught
a smuggled shape change. Self-test 3/3.

All 5 files classify **DIGEST-ONLY**, 0 shape leaves, and every file is byte-length identical
before and after (1757->1757 x4, 19326->19326). `counts`, `source_inputs`, `capabilities`,
`fixture_inventory`, `subcontracts` all unchanged.

**No wire-shape change is being absorbed.**

## M6 — still to establish

- [ ] guard passes after regeneration (assert the N passed count)
- [ ] `golden_v0_1_21.rs` still passes
- [ ] guard can still FAIL (mutation test — a corpus that matches anything is worthless)
- [ ] OpenAPI 3.1.0 endpoint — is it in the corpus at all?

## M7 — after-state gates (hetzner, run by FILE not filter, executed counts asserted)

| test file | result | rc |
|---|---|---|
| `desktop_contract_corpus` | **15 passed; 0 failed; 0 ignored** (was 14/1) | 0 |
| `desktop_contract_adversarial` | 17 passed; 0 failed; 0 ignored | 0 |
| `golden_v0_1_21` | **22 passed; 0 failed; 0 ignored** | 0 |
| `host_decoder_contract` | 31 passed; 0 failed; 0 ignored | 0 |
| `approval_resume_contract` | 4 passed; 0 failed; 0 ignored | 0 |

Zero `ignored` everywhere, so none of the three vacuity flavours (all-ignored / env-gated
early return / filter matching nothing) applies.

Blob-identity check: all 5 regenerated files have byte-identical git blob hashes on hetzner
and in my Mac commit, so what is committed is verbatim generator output, not a hand-edit.

## M8 — the guard can still FAIL (mutation test)

| probe | mutation | result |
|---|---|---|
| M-A | one byte in `events/ready.json` (`session-desktop-001`->`002`) | **FAILED** rc 101, drift=`[ready.json]` |
| M-B | **one comment line appended to `crates/wcore-agent/src/bootstrap.rs`** | **FAILED** rc 101, drift = all 5 files |
| M-C | both restored | 15 passed, rc 0 |

M-B is the whole argument in one line: a comment in a **different crate**, with no wire
effect whatsoever, reddens the Desktop contract corpus. That is the structural defect, and it
is now measured rather than asserted.

## M9 — OpenAPI 3.1.0: NOT touched by this regeneration

- `grep -rl openapi crates/wcore-protocol/contracts/` -> **0 files**
- none of the 40 SOURCE_INPUTS mentions openapi
- the 3.0.3 -> 3.1.0 move is entirely in `wcore-acp` (utoipa 4->5, taken to drop
  `proc-macro-error` / RUSTSEC-2024-0370)

Correction to the brief I was given: the endpoint **is** covered, just not by a byte fixture —
`wcore-acp` asserts `starts_with("3.1")` in two places, one of them over a live listener:
`rest_openapi_doc_served_over_live_listener ... ok` (2 passed) and
`transport::rest::tests::get_openapi_json_has_paths_and_resolves_schemas ... ok` (129 passed).
Both green at this commit.
