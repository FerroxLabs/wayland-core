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

## M2 — the consumer hunt (P2). Trees reachable, and what each said

**Reachability first.** Desktop **IS** reachable from this machine, contrary to the risk that it
would not be: `/Users/seandonahoe/dev/wayland/app` — remote `FerroxLabs/wayland`, branch `main`,
HEAD `b3694a18f`. (The parent `/Users/seandonahoe/dev/wayland` is NOT a git repo; the repo is the
nested `app/`. That is why a top-level `git -C` probe returns "not a git repository" — an easy way
to wrongly report Desktop unreachable.)

Every search below ran under `/usr/bin/grep` or `/usr/bin/git grep` (unproxied), with globs quoted,
and **each invocation carried a known-positive in the same command**.

| Tree | Known-positive (instrument alive) | openapi-concept hits | Verdict |
|---|---|---|---|
| `wayland/app` (**Desktop**) | `electron` ×24 in package.json; `wayland-core` ×136 in `src/` | 149 total — **145 inside `src/process/resources/skills-library/` (bundled LLM prompt corpus)**; of the remaining 4: 1 comment about *Gemini's* function-declaration schema, 3 in a vendored `libsignal/yarn.lock` (`@octokit/openapi-types`) | **does not parse it** |
| `wayland/app` manifests | 5 `package.json` scanned | 0 declare `openapi-typescript`/`openapi-generator`/`orval`/`oazapfts`/`hey-api`/`kubb`/`swagger` | no generated client |
| `flux-desktop` | `flux` ×213 tracked files | **0** | no |
| `flux-router` | `flux` ×1087 tracked files | 4 — kube-openapi patch-merge, a LiteLLM planning doc, `swagger` as prose in a training CSV | no |
| `getwayland` | `wayland` ×446 files | 2 — a Vercel `$schema` URL, and a research note (quoted below) | no |
| `waylandmcp` / `waylandteams` / `waylandcli` | 40 / 39 / 0 files | 0 / 0 / 0 | no (`waylandcli` is an empty stub — 0 known-positive files, so its 0 proves nothing) |
| `waylandplugins` | 90 files | 1 — a Vercel `$schema` URL | no |
| `waylandskills` | 399 files | 9 — all skill/marketing prose | no |
| `waylandcore` (this tree) | `wcore-acp` ×53 tracked files | 25 outside `.planning/` — enumerated below | **ONE consumer, see M3** |

**Concept, not keyword.** I did not search only `openapi`. The searches also covered: SDK
generators by name (`openapi-generator`, `openapi-typescript`, `orval`, `oazapfts`, `@hey-api`,
`kubb`, `redoc`, `rapidoc`, `swagger`), generated-client *idioms* (`components["schemas"]`,
`*.gen.ts`, `schema.d.ts` — all 0 in Desktop), and the *behavioural* alternative: does Desktop
issue HTTP to Core's REST surface at all?

**That last one is the trap, and it nearly caught me.** Desktop has 54 hits for `/v1/` — Core's
REST routes are also `/v1/*`. All 54 are **outbound LLM-provider** paths (`/v1/models`,
`/v1/chat/completions`, `/v1/messages`, `/v1/embeddings`, `/v1/images`) in
`src/process/providers/**`, `ConnectionTester.ts`, `Curator.ts`, `imageModels.ts`. None targets
Core.

**Positive account of how Desktop actually reaches Core** (this is what makes the negative safe —
not the absence, but a complete alternative explanation): `src/process/agent/wcore/index.ts:446`
`spawn(binaryPath, args, { stdio: ['pipe','pipe','pipe'] })`, argv built at
`src/process/agent/wcore/envBuilder.ts:541` as `['--json-stream', '--provider', …, '--model', …]`.
**Desktop drives Core as a child process over stdio JSON-stream. It never opens a socket to the
REST server, so it can never fetch the document.** `acp` appears ×1424 in Desktop `src/`; no
`acp-port` / `acp-url` / REST-listener reference exists (0 hits).

**Independent corroboration from a different lane's research**, `getwayland/.planning/
core-research/embedding-protocol.md:529` — "`wcore-acp` has a `rest.rs` transport module, but there
is **no documented OpenAPI 3.1 spec or auto-generated SDK** comparable to opencode's `opencode
serve` surface. This is an acknowledged gap in the CAPABILITY-MATRIX." Written before this bump and
by someone else; it agrees no SDK is generated from the document.

## M3 — there IS one consumer, and it is in Core's own binary

Not "no consumer". `crates/wcore-acp/src/transport/rest.rs:336` — the `/doc` HTML spec viewer
shipped inside `wayland-core` does `const res = await fetch('/openapi.json')` in browser JS and
renders it.

**It is provably version-agnostic**, readable in full at `rest.rs:332-366`. It touches exactly:
`spec.paths` → `Object.entries` → `m.toUpperCase()` → `op.summary || op.description`. It never
reads the `openapi` version field, never validates against a schema, and never touches `nullable`
or `type`. **`paths`, `summary` and `description` are byte-identical between 3.0.3 and 3.1.0**, so
this consumer cannot break on the change. Verified by reading it, and re-verified live in M4.

So the answer is not "nobody parses it" — it is "exactly one thing parses it, it ships with us,
and it is structurally indifferent to the version."

## Still to establish

- Live shape count off the real binary (hetzner). **UNANSWERED.**
- The fixture + its three-assertion self-test. **NOT BUILT.**
