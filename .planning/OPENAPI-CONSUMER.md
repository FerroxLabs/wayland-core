---
lane: openapi-consumer
question: "Does any consumer actually parse GET /openapi.json, and strictly? (MILESTONE-RC.md §8)"
verdict: "Exactly ONE consumer parses it — Core's own /doc HTML viewer, shipped inside the binary — and it is provably version-agnostic. No external consumer exists in any reachable tree. Desktop CANNOT fetch the document: it drives Core as a child process over stdio JSON-stream and never opens a socket to the REST server."
recommendation: "A — RECORD AS FREE and ship, with release-note disclosure. Do NOT pin the emitted version. Do NOT couple it to the Desktop digest re-pin: §8's stated reason for coupling ('Desktop is the primary consumer') is disproved. Cross-audit panel 3/3 for A; internal adversarial dissent recorded below and it does not overturn A."
new-finding: "LOW — all 10 REST operations lack `summary`/`description`, so /doc renders bare METHOD /path rows. PRE-EXISTING, not a bump regression (no handler carries a doc comment). Non-blocking → BACKLOG."
fence-exposure: "ZERO. `git diff 15cda12d -- crates/wcore-cli/src/lib.rs crates/wcore-cli/src/main.rs` is empty. 4 files changed total, all inside crates/wcore-acp/ + .planning/. No contract regeneration, no PR, no merge, no tag."
status: complete
---

# `/openapi.json` — the consumer question, answered

Assignment: MILESTONE-RC.md **§8**. Merge-base `15cda12d`. Branch `lane/openapi-consumer`,
HEAD `aeebe05f`.

Running measurements and full search transcript: `.planning/OPENAPI-CONSUMER-NOTES.md`.

---

## 1. The answer

> **Open question, not yet answered:** does any consumer actually parse this document strictly?
> If Desktop does not consume `/openapi.json` at all, this is free and should be recorded as free.
> — MILESTONE-RC.md §8

**Desktop does not consume it, and structurally cannot.** It launches Core as a child process:

```
src/process/agent/wcore/index.ts:446   spawn(binaryPath, args, { stdio: ['pipe','pipe','pipe'] })
src/process/agent/wcore/envBuilder.ts:541
        const args: string[] = ['--json-stream', '--provider', provider, '--model', …]
```

Desktop never opens a socket to Core's REST listener — there is no `acp-port` / `acp-url` /
REST-endpoint reference anywhere in its source (0 hits), against `acp` appearing 1424 times. A
process that never connects to the server cannot fetch the server's document.

**But the answer is not "no consumer".** There is exactly one, and it ships inside `wayland-core`:

`crates/wcore-acp/src/transport/rest.rs:336` — the `/doc` HTML spec viewer —
`const res = await fetch('/openapi.json')`.

**It is version-agnostic, and that is the load-bearing fact.** Its entire parse, readable at
`rest.rs:332-366`, is `spec.paths` → `Object.entries` → `m.toUpperCase()` →
`op.summary || op.description`. It never reads `spec["openapi"]`. It never touches `nullable` or
`type`. `paths`, `summary` and `description` are byte-identical between 3.0.3 and 3.1.0.

Verified live, not only by reading: `GET /doc` → `200`, and re-running the viewer's exact parse
over the live 3.1.0 document renders all **10 operations**.

So: **one consumer, it is ours, and the change cannot reach it.**

## 2. How I proved it — and what I could not reach

Per the standing rule that a known-negative is self-passing on a dead instrument, every search
below used `/usr/bin/grep` or `/usr/bin/git grep` (unproxied), quoted globs, and **carried a
known-positive in the same invocation**.

| Tree | Instrument-alive control | openapi-concept hits | Verdict |
|---|---|---|---|
| **Desktop** `/Users/seandonahoe/dev/wayland/app` | `electron` ×24 in package.json; `wayland-core` ×136 in `src/` | 149 total, **145 inside the bundled LLM prompt corpus** `src/process/resources/skills-library/`; the other 4 are a Gemini-schema comment and a vendored `@octokit/openapi-types` lockfile | no parse |
| Desktop manifests | 5 `package.json` scanned | **0** declare `openapi-typescript` / `openapi-generator` / `orval` / `oazapfts` / `@hey-api` / `kubb` / `swagger` | no generated client |
| Desktop generated-client idioms | — | `components["schemas"]` = 0, `*.gen.ts` = 0, `schema.d.ts` = 0 | none exist |
| `flux-desktop` | `flux` ×213 tracked files | 0 | no |
| `flux-router` | `flux` ×1087 tracked files | 4 (kube-openapi, a LiteLLM planning doc, `swagger` as prose in a CSV) | no |
| `getwayland` (website) | `wayland` ×446 files | 2 (a Vercel `$schema` URL; one research note) | no |
| `waylandmcp` / `waylandplugins` / `waylandskills` / `waylandteams` | 40 / 90 / 399 / 39 files | 0 / 1 / 9, all prose or Vercel URLs | no |
| `waylandcore` (this tree) | `wcore-acp` ×53 tracked files | 25 outside `.planning/` | **one: the `/doc` viewer** |

**Concept, not keyword.** I did not search only `openapi`. I searched SDK generators by name, the
*idioms* a generated client leaves behind, and — most usefully — the **behavioural alternative**:
does Desktop issue HTTP to Core's REST surface at all?

**That last one nearly caught me and is worth flagging for the next lane.** Desktop has 54 hits for
`/v1/`, and Core's REST routes are *also* `/v1/*`. All 54 are **outbound LLM-provider** paths —
`/v1/models`, `/v1/chat/completions`, `/v1/messages`, `/v1/embeddings`, `/v1/images` — in
`src/process/providers/**`. A keyword search for `/v1/` would have produced a confident, wrong
"Desktop calls Core's REST API".

**Reachability, stated plainly.** Desktop **IS** reachable (`FerroxLabs/wayland`, `main`,
`b3694a18f`). One trap: `/Users/seandonahoe/dev/wayland` is **not** a git repo — the repo is the
nested `app/`. A `git -C /Users/seandonahoe/dev/wayland` probe returns *"not a git repository"*,
which is an easy way to wrongly report Desktop unreachable.

**What I could NOT reach, and it is the real limit on this evidence:** any **third-party** consumer.
The endpoint is public and unauthenticated *by design* — README.md:480 advertises it "for
discovery". Whoever it was built for is, by construction, outside every tree on this machine. I can
prove no *first-party* consumer parses it; I cannot enumerate external ones. That limit is the
substance of the dissent in §4 and I am not papering over it.

**Independent corroboration** from a different lane, written before this bump:
`getwayland/.planning/core-research/embedding-protocol.md:529` — "`wcore-acp` has a `rest.rs`
transport module, but there is **no documented OpenAPI 3.1 spec or auto-generated SDK**… an
acknowledged gap in the CAPABILITY-MATRIX."

## 3. Live measurement — the real binary, not the test harness

Built on `hetzner-dsm` (never the Mac), targeted: `cargo build -p wcore-cli --bin wayland-core`,
`Finished in 1m 40s`, 0 `^error` lines, `wayland-core 0.12.25`.

**Getting the real binary to serve required a real fix, recorded here because the next lane will
hit it.** `wayland-core acp serve` **exits 1** on a headless Linux host:

```
wayland-core acp: keychain store failed: … no keychain backend available: Secret Service: no result found
```

`--api-key` does not avoid it — the key is persisted via `store_api_key` (`wcore-cli/src/acp.rs:321`)
*before* the bind. Working recipe: run the server under `dbus-run-session` with
`gnome-keyring-daemon --unlock --components=secrets` fed an ephemeral passphrase **on stdin**.

Then, `curl` against the live listener — `200 OK`, `application/json`, `18286` bytes, 8 paths:

| Measurement | Value |
|---|---|
| `openapi` version field | **`3.1.0`** |
| 3.1 form `type: [T, "null"]` | **9** |
| 3.0 form `nullable: true` | **0** |

**§8's 9 / 0 independently reproduced off the real binary**, and now with the nine sites named —
`AgentInfo.description`, `ApprovalResolveRequest.{answer,prefix,resume_token}`,
`SessionCreateRequest.{agent,model,system_prompt}`, `SessionCreateResponse.model`,
`SessionMetadata.model`.

*Secret hygiene:* the server's first-run API key was printed to its own stderr log; that log is not
copied into the repo, and a sweep of the served document against the value returned **0**. The
keyring passphrase was generated on the remote host and injected on stdin — never argv, never disk.

## 4. Recommendation: **A — record it as free.** With the dissent stated.

**Cross-audit panel (§4), all three invoked correctly, votes extracted unanchored:**

| Auditor | Position | Core reasoning |
|---|---|---|
| codex `gpt-5.6-sol` | **A** | Sole viewer demonstrably version-agnostic; pinning preserves an unused promise while reintroducing an advisory; "the Desktop wire-contract digest is unrelated to the REST document" |
| gemini `3.1-pro-preview` | **A** | Reverting means pinning utoipa 4 and deliberately reintroducing RUSTSEC-2024-0370 "without any functional benefit" |
| kimi K3 | **A** | "Pre-1.0 is precisely when such a side-effect bump is free"; option C couples an unrelated detail to a separate channel |

**3/3 for A.**

**Internal adversarial pass, arguing against the consensus** — the strongest case for B/C:

> "No consumer" claims on this program have been wrong about as often as right. This search covers
> *repos on this Mac*. The endpoint is **public and unauthenticated by design** and the README
> advertises it "for discovery" — so its intended consumer is precisely the population I cannot
> enumerate. Concluding "free" from first-party absence is the same shape of error the program has
> made three times.

**Why it does not overturn A, but does shape it:**

1. The population that actually breaks is "pinned **strictly** to 3.0.x". Every mainstream generator
   — `openapi-generator`, Speakeasy, `openapi-typescript` — reads 3.1.
2. The cost of B is concrete and security-negative: pinning utoipa 4 **reintroduces
   RUSTSEC-2024-0370** (`proc-macro-error`, unmaintained, `patched=[]`). Trading a measured advisory
   for a hypothetical client is the wrong direction.
3. **C's premise is disproved.** §8 argued "It must ride the same train as the Desktop digest re-pin"
   *because* "Desktop is the primary consumer". Desktop is not a consumer of this document at all.
   The digest re-pin (`observation.rs:329/342-343`, a hard error at `ready`) governs the **stdio
   JSON-stream** contract — a different channel. Coupling adds a release-train dependency and
   reduces no risk.
4. The honest form of A is **not bare**: it is A **plus** release-note disclosure **plus** the gate
   in §5, so the next move of this surface is caught rather than discovered.

**So: A, and the dissent's real contribution is a condition** — if Core ever publishes a REST SDK or
a public API-docs site generated from this document, the 3.1 pin becomes a genuine external
contract and this decision must be revisited. Recorded, not deferred.

## 5. The gate gap — closed

§8: *"No fixture covers `/openapi.json`, so nothing in CI caught it and nothing will catch the next
one."*

**Nearly right, and the difference matters.** Two assertions did exist —
`rest.rs:965` and `rest_roundtrip.rs:189` — but `git log -S'starts_with("3.1")'` returns **exactly
one commit: `cb9fa9d6`, the utoipa bump itself.** Both previously read `"3.0"` and were edited to
`"3.1"` in the same commit as the change they would have caught. They are **version-prefix**
assertions living in the emitting crate; **nothing anywhere examined the nullable encoding** — the
part that actually breaks a 3.0 client. §8's core claim (no fixture) is correct.

**Added:**

- `crates/wcore-acp/tests/fixtures/openapi/rest-openapi-shape.json` — pins the emitted version
  **exactly** (`3.1.0`, not a prefix) and lists every site in each encoding, plus a
  version/encoding consistency rule.
- `crates/wcore-acp/tests/openapi_contract.rs` — scans the document served over a **live TCP
  listener**, exactly as a consumer would see it.

**Three assertions, not two** — the third is the only one that proves the repair does anything:

| # | Test | What it establishes |
|---|---|---|
| 1 | `openapi_shape_matches_committed_fixture` | **known-positive** — the live document satisfies the fixture. Carries an anti-vacuity guard: if the scanner finds ZERO nullable fields in *either* encoding it fails as a dead instrument rather than passing. |
| 2 | `shape_checker_rejects_a_3_0_encoded_document` | **known-negative** — the pre-fix document, reconstructed by inverting the bump, is REJECTED, and the rejection must name both the version and the encoding. The gate can fail. |
| 3 | `old_coverage_is_blind_to_a_pure_encoding_flip` | **the old instrument would have missed it** — see below. |

**Assertion 3 in detail.** It builds a document whose nullable encoding has reverted to 3.0's
`nullable: true` while the version string is left untouched at `3.1.0`, then asserts **both**:

- the **complete** pre-existing gate — replicated verbatim, not a strawman: version prefix, both
  keystone paths, the no-dangling-`$ref` sweep, `SessionMetadata` present, drawn from
  `rest.rs:964-995` and `rest_roundtrip.rs:186-196` — **passes** on it; and
- the new checker **fails** on it, naming the inconsistency.

It also asserts the replica **passes on the real document first**, so a simply-broken predicate
cannot fake the result.

**Executed results on hetzner** (`/usr/bin/env cargo` — the proxy strips the `0 ignored` /
`0 filtered out` fields this rule depends on):

```
running 3 tests
test openapi_shape_matches_committed_fixture ... ok
test shape_checker_rejects_a_3_0_encoded_document ... ok
test old_coverage_is_blind_to_a_pure_encoding_flip ... ok
test result: ok. 3 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out
```

**Falsified two independent ways, with a green control, to prove the fixture is load-bearing and
not decorative:**

| Falsification | Result |
|---|---|
| Fixture version → `3.0.3` (pretend the bump never happened) | `FAILED. 1 passed; 2 failed` |
| Drop ONE of the nine nullable sites from the fixture | `FAILED. 2 passed; 1 failed` |
| Restore fixture (control) | `ok. 3 passed; 0 failed; 0 ignored; 0 filtered out`, fixture diff empty |

Full crate, isolated: `wcore-acp` **151 passed, 0 failed, 0 ignored, 0 filtered** across 6 binaries
(129 unit + 2 + 3 + 2 + 11 + 4). Clippy `-p wcore-acp --all-targets`: **0** warning/error lines.
`cargo fmt --all -- --check`: clean.

## 6. Second finding — separate, and NOT a regression

Every one of the 10 REST operations carries only `operationId`, `responses`, `tags`. **Not one has
`summary` or `description`**, so `/doc` renders bare `METHOD /path` rows with no prose.

**This is pre-existing, not a bump side effect.** utoipa derives operation summary/description from
the `///` doc comment on the handler fn in *both* majors, and no handler in `rest.rs` has one
(`#[utoipa::path(...)]` sits directly on each fn, with no explicit `summary =` either). It was
equally empty under 3.0.3. Attributing it to the bump would have been a false regression report.

Graded **LOW**, non-blocking → BACKLOG. **I did not edit `.planning/BACKLOG.md`**: it is a shared
file several lanes touch and an edit would create a needless merge conflict. Orchestrator: please
fold this one line in.

## 7. Fences — what I did NOT do

- **No** `wcore-contract generate`. The second regeneration §5 owes remains outstanding and
  Sean-reserved. **Nothing in this lane touches the wire contract** — my change is confined to
  `crates/wcore-acp/tests/` plus one comment in `crates/wcore-acp/Cargo.toml`.
- **No** merge to `main`, no PR, no tag, no publish, no GitHub issue closed.
- **Shared-file fence: ZERO exposure.** `git diff 15cda12d -- crates/wcore-cli/src/lib.rs
  crates/wcore-cli/src/main.rs` → empty.
- 4 files changed vs `15cda12d`, +686 / −1.
- No credential supplied; nothing secret printed, committed or swept up (sweep returned 0).
- **Nothing to serialize** for the orchestrator beyond the one BACKLOG line in §6.
