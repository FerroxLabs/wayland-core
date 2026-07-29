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
