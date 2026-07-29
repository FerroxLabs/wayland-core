# OPENAPI-CONSUMER — running notes (append-only, committed after every measurement)

Lane: `openapi-consumer`. Worktree `/Users/seandonahoe/dev/waylandcore-frontier-worktrees/lane-openapi-consumer`,
branch `lane/openapi-consumer`, merge-base `15cda12d`.

Assignment: MILESTONE-RC.md §8. `GET /openapi.json` moved 3.0.3 → 3.1.0 as a side effect of the
utoipa 4→5 bump (RUSTSEC-2024-0370 removal). Three deliverables:

1. Answer, with evidence, whether ANY consumer parses `/openapi.json` — and strictly.
2. Decide (pin emitted version / co-release / record as free). Do not park.
3. Close the gate gap with a fixture carrying a **three-assertion self-test**
   (known-positive passes, known-negative fails, **pre-fix shape would have slipped the old
   coverage**).

---

## Plan

- **P1 — locate the emitter.** `crates/wcore-acp/src/transport/rest.rs`. DONE (below).
- **P2 — consumer hunt.** Search every reachable tree (Core, Desktop, Flux, contract fixtures,
  docs, scripts, TS/JS clients) for a parse of `/openapi.json` or a generated client. Concept
  search, not keyword. Known-positive in every invocation. Unproxied `/usr/bin/grep`.
  State which trees I searched and which I could not reach.
- **P3 — live measurement.** Drive the real binary on hetzner-dsm, capture the actual document,
  count `type: [...,"null"]` vs `nullable:` fields. Never build on the Mac.
- **P4 — fixture.** Pin version AND nullable-encoding shape. Self-test with three assertions.
- **P5 — decision + `.planning/OPENAPI-CONSUMER.md`, commit, push.**

---

## M1 — the emitter, and the state of existing coverage (measured, unproxied git)

`crates/wcore-acp/src/transport/rest.rs:223` routes `/openapi.json` → `openapi_json()` at
`:291`, body `Json(ApiDoc::openapi())`. `ApiDoc` is the `#[openapi(...)]` derive at `:232`.
Public/unauthenticated carve-out alongside `/doc` (`wcore-cli/src/acp.rs:499-503`).

**§8 says "No fixture covers `/openapi.json`". That is nearly right but not exactly right, and the
difference matters for what I must build.** Two inline assertions DO exist:

- `crates/wcore-acp/src/transport/rest.rs:965` — `doc["openapi"].starts_with("3.1")`
- `crates/wcore-acp/tests/rest_roundtrip.rs:189` — same assertion over a live listener

`/usr/bin/git log -S'starts_with("3.1")'` returns exactly ONE commit: `cb9fa9d6`, which **is the
utoipa bump itself**. Both assertions previously read `"3.0"` and were edited to `"3.1"` in the
same commit as the change they would have caught. So:

- they are **version-prefix assertions only** — nothing anywhere pins the *shape*
  (`nullable:` vs `type: [..., "null"]`), which is the part that actually breaks a 3.0 client;
- they live in the same crate as the emitter, so a co-edit is a one-line diff in the same commit;
- there is **no fixture** in the fixture-harness / wire-contract system covering this endpoint,
  which is what §8 means and is correct.

The commit message is honest about the change ("Wire-visible consequence: GET /openapi.json now
emits OpenAPI 3.1.0"), so this was disclosed, not hidden. The gap is that nothing *external to the
changing crate* can fail.

## Still to establish

- Whether Desktop / Flux / any generated client parses the document. **UNANSWERED.**
- Live shape count off the real binary. **UNANSWERED.**
- Which trees are reachable from this machine. **UNANSWERED.**
