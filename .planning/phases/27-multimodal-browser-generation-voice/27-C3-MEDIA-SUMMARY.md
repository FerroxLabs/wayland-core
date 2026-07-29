# 27-C3-MEDIA — lane summary

**Lane:** `27-c3-media` · branch `lane/27-c3-media` · base `plan/f20-unified-audit-repair` @ `5457710e`
**Mandate:** Phase 27 **Criterion 3** — *"Built-in, MCP-only, late-MCP, and combined media
generation expose consistent discovery, credentials, accounting, and failures."* Graded
**NOT MET** by `27-PHASE-VERDICT.md`.

**Honest verdict: C3 is NOT MET, and is now PARTIAL.** Two of its four clauses moved on real
evidence; two did not, and one of those is named below as an open HIGH I did not close.

---

## 1. The blocking item: the cost record

The brief's instruction was that a missing cost record for billable generation is **blocking**,
not optional. It is now built, and it is live-proved.

### What constrained the design (measured by prior lanes, re-read not assumed)

FluxRouter returns **no cost for an image in any channel** — not a header, not the body. So a
dollar figure for an image *cannot* come from the provider; any code that produces one has
invented it. The record therefore separates two things that are normally conflated:

- **Billable units** — always observable, always recorded, and they vary with the work.
- **A dollar figure** — recorded only when something actually supplied one, and always stamped
  with a `price_source` naming the channel it came from.

### Rules the record enforces

| Rule | Why |
|---|---|
| Unpriced is reported as `unpriced` **with a reason**, never as `$0.00` | "nobody prices this" and "this cost nothing" are different claims |
| A **failed** call records `call_failed_billing_unknown` | providers bill for rejected prompts; a failure is not free |
| A local rate card is stamped `local_rate_card`, never provider-reported | an operator's estimate must never render as the provider's number |
| A **locally-rejected** result (oversized payload, unsafe URL) still accounts | the upstream work happened and was billed; the refusal is ours |
| Unknown dimensions are `None`, not `0x0` | a zero sentinel reads as a measurement and would sum into a session total as if the call produced nothing |
| The ledger reports `unpriced_calls` and `calls_of_unknown_size` **beside** the totals | a total that silently absorbs them is the same lie as `$0.00` |

### Evidence it actually varies with the work done

**This is the claim the brief singled out, so it carries three independent proofs.**

**(a) Unit level** — `cargo test -p wcore-tools --lib media_cost` → `8 passed; 0 failed; 0 ignored`.

**(b) Through real HTTP against the hermetic fixture** — `builtin_shape_record_varies_with_the_requested_work`
drives three aspect ratios and asserts the records differ **and** that the fixture itself
received three distinct sizes and three distinct prompts. A record can otherwise vary while the
request is constant.

**(c) LIVE, against the real billable FluxRouter account.** Two calls, one variable moved,
two different records, extracted verbatim from `evidence/27-c3-media/live-probe-captures.txt`:

```
{"backend_id":"unresolved","model":"unresolved",
 "outcome":{"category":"other","status":"failed"},
 "price_source":{"kind":"unpriced","reason":"call_failed_billing_unknown"},
 "tool":"image_generate","units":{"height":1024,"images":1,"width":1536}}

{"backend_id":"OpenAI flux-image","model":"OpenAI flux-image",
 "outcome":{"status":"ok"},
 "price_source":{"kind":"unpriced","reason":"provider_reports_no_cost"},
 "tool":"image_generate","units":{"height":1024,"images":1,"width":1792}}
```

They differ in backend, model, outcome, unpriced reason and width. And on the CLI surface,
`CLI_RECORD_VARIES=YES` with `n=1, size unreported` versus `n=2, 1024x1024 = 2.097 MP`.

### The gates were proved able to fail — `MUTATION_CONTROL=PASS`, 13/13

`evidence/27-c3-media/mutation-control-run.txt`. Four mutations, each breaking one property:

| Mutation | Expected | Result |
|---|---|---|
| M1 units pinned so the record cannot vary | RED | RED (unit **and** through-HTTP) |
| M2 provider-cost channel dead | RED | RED |
| M3 failed call recorded as `$0.00` | RED | RED (unit **and** through-HTTP) |
| M4 local estimate laundered as provider truth | RED | RED (unit **and** through-HTTP) |

Every baseline was asserted GREEN first, and every run asserts its **executed test count** —
a suite that exits 0 having run zero tests is the failure mode §3.2 warns about.

**The M2 row is the one that matters most.** Under M2 the *unpriced* assertion stays **GREEN**,
because nothing reports a price. So that assertion alone can never distinguish "the provider is
silent" from "this code cannot price anything". The cost-header control is what carries it, and
the control is why it is trustworthy.

### Where the record surfaces, and where it does not

- **The model** sees it in the tool-result JSON. ✅
- **A protocol host** sees it: the whole tool-result string crosses on `ProtocolEvent::ToolResult.output`,
  so this needed **no wire-contract change** and I did not run `wcore-contract generate`. ✅
- **Operators** get a structured `wcore::media_cost` tracing line and, on the `image` subcommand,
  `accounting:` + `accounting_json:` on stderr. ✅
- **The TUI `/cost` screen does NOT show it.** That screen is fed by `ProtocolEvent::SessionCost`,
  whose `TurnCost` is per-turn with no per-tool dimension and is a frozen Desktop contract.
  **Filed as a seam request below, not silently skipped.** ❌

---

## 2. The MCP fixture

`crates/wcore-agent/tests/f27_media_generation.rs`. One loopback server speaking **both** wire
protocols the product uses to reach a media capability:

- `POST /v1/images/generations` — OpenAI-wire, so the **built-in** path runs through the real
  `DalleBackend`, the real SSRF-guarded egress client and the real `ImageGenerationTool` **with
  no money spent**. This is the thing that made generation practically untestable before.
- `POST /mcp` — MCP streamable-HTTP through the real `McpManager`, with a configurable tool
  name, a credential gate, a refusal trigger and an accounting-shaped result.

`cargo test -p wcore-agent --test f27_media_generation` → **12 passed; 0 failed; 0 ignored;
0 filtered out.**

### The four shapes, measured

| Shape | Discovery | Credentials | Failures | Accounting |
|---|---|---|---|---|
| **Built-in** | registers via the config arm | 401 fails closed, status reaches the user | typed categories (`prompt_rejected`, `insufficient_credits`) | **full record** |
| **MCP-only** | discovered + callable over real transport | missing credential fails closed as `isError` on a *successful* transport call | same `prompt_rejected` label | **none** |
| **Combined** | MCP **can** advertise `image_generate` and shadow the built-in | — | — | — |
| **Late-MCP** | **NOT EXERCISED** | — | — | — |

Two of these are honest negatives, asserted in tests so they cannot change silently:

- `mcp_shape_produces_no_product_cost_record_today` — **accounting is NOT consistent across
  the shapes.** The MCP proxy returns opaque text and no `MediaCostRecord` is produced. The
  test carries its own control (the success fields *are* present) so the negative is not
  passing on an empty payload.
- `combined_shape_mcp_tool_may_shadow_the_builtin_name_without_a_marker` — threat T-27-03-08
  observed rather than assumed. Nothing at this layer marks the collision.

**Late-MCP was not exercised.** I did not reach it. Saying so is the honest grade.

---

## 3. Findings — all four found by RUNNING it, none by reading source

### F-27C3-01 · The `image` subcommand spends money with no record — **FIXED**
`wayland-core image` goes through `FluxImageClient`, a **second billable generation path** that
never touches the tool. Live: it wrote a **249,886-byte JPEG** from a real paid account and
neither output stream contained a single accounting token — with the liveness control on the
same grep and same files returning 1, so the zero is real. `--n` multiplies that silently.
Now emits the same record shape; proved to vary (`CLI_RECORD_VARIES=YES`).

### F-27C3-02 · The advisory omitted the arm that actually enables generation — **FIXED**
A session with **only `FLUX_API_KEY`** set registered `image_generate` — through
`dalle_backend_from_config`, which the honest-unavailable hint did not name. A Flux user hitting
that advisory was told to set one of four keys, none of which was the one they already had.
Same family as the `[browser]`/`[browser.policy]` defect this phase already found.

**The anti-drift guard could not have caught it.** It compares the hint against `read_env_key`
calls, and the arm in question reads no env var — so it *certified* the omission. Per §6b-ii I
repaired the instrument in the same commit, with a three-assertion self-test whose **third**
assertion proves the old matcher would have missed it (without it the self-test passes on the
broken guard too).

### F-27C3-03 · `image_gen` was the one media resolver that would not say where it points — **FIXED**
It logged `image_gen: using gpt-image-1 at  ` — an empty base_url — because it printed
`config.base_url`, which FluxRouter leaves empty. `vision` and `transcription` on the same boot
both printed their full Flux URL. This is not cosmetic: **LANE-BRIEF §3b-ii requires a provider
claim to be read back from the product's own output**, and image generation was the one media
capability where that was impossible. Now logs the resolved endpoint:
`image_gen: using flux-image at https://api.fluxrouter.ai/v1/images/generations`.

### F-27C3-04 · The built-in image tool is broken by default in a FluxRouter session — **OPEN, HIGH**
**I did not fix this.** In a Flux session the tool sends `gpt-image-1`, which a Flux key is not
entitled to. Measured, one variable:

| Arm | Model sent | Result |
|---|---|---|
| default | `gpt-image-1` | **FAILED** — `outcome: failed`, `call_failed_billing_unknown` |
| `OPENAI_IMAGE_MODEL=flux-image` | `flux-image` | **succeeded**, real billable image |

#310 fixed the *endpoint and key* routing for Flux but not the *model*, so the flagship
configuration's built-in image tool fails until the user finds an undocumented env var. The fix
is a per-provider default image model, which is a ProviderCompat question and beyond a lane that
was scoped to accounting — so it is reported open rather than patched badly.

---

## 4. Credential handling and the secret sweep

Used the live burn key at `~/.wayland-secrets/flux.env` on `hetzner-dsm`, per the LANE-BRIEF §0
sanctioned exception: **stdin only**, never in argv, never written to disk, never echoed, never
in a capture. Real spend: ~6 image generations.

**`SECRET_SWEEP=PASS` — 0 hits, and every liveness control fired.**
(`evidence/27-c3-media/secret-sweep-run.txt`)

| Sweep | Hits | Control |
|---|---|---|
| keyfile itself (known-positive) | **1** | *is* the control |
| `crates/` | 0 | ↑ |
| `.planning/phases/27-*` | 0 | ↑ |
| evidence dir | 0 | ↑ |
| `git log -p BASE..HEAD` | 0 lines | separate control on the piped grep shape: **1** |
| hetzner captures + isolated home | 0 | remote known-positive (`fluxrouter`): **5** |

The pattern reaches grep through a process substitution — it is never written to disk.

**Two instrument defects in my own harness, repaired rather than noted:**

1. **The sweep's remote section produced NO OUTPUT and the script still printed `PASS`.** It
   piped the pattern into `ssh 'bash -s' <<'HEREDOC'`; the heredoc took stdin, so bash read the
   *script* and the `cat` inside got nothing. **A remote sweep that never ran is
   indistinguishable from one that found zero.** Two channels now, and the section is VOID
   unless it produces output.
2. **The probe's key parser mangled the credential.** It anchored on `FLUX_API_KEY` at line
   start; the file is `export FLUX_API_KEY=...`, so it fell through and handed the provider the
   entire shell line. The provider replied `401 ... Received=expo****`. **That reads exactly
   like a dead key and I was one step from reporting a credential blocker Sean would have had to
   answer.** Repaired, plus a post-parse reject for anything still containing `=` or a space,
   and the same parser is used by the sweep so a mangled pattern cannot silently produce a
   comforting zero.

A third: `--json-stream` with a positional prompt exits after the capability handshake without
driving a turn — it produced three byte-identical 4506-byte captures containing no model turn.
A capture that looks like evidence and contains none.

---

## 5. Gates

| Gate | Result |
|---|---|
| `cargo test -p wcore-tools --lib media_cost` | **8 passed; 0 failed; 0 ignored; 995 filtered** |
| `cargo test -p wcore-agent --test f27_media_generation` | **12 passed; 0 failed; 0 ignored; 0 filtered** |
| `cargo test -p wcore-agent --lib capability_advisory` | **5 passed; 0 failed; 0 ignored** |
| mutation control | **13/13, `MUTATION_CONTROL=PASS`** |
| secret sweep | **`SECRET_SWEEP=PASS`, 0 hits, 4/4 controls fired** |
| live probe | **rc=0 on all five legs; arm read back from the product's own output** |
| `cargo fmt --all` | clean |

### Two RED results I am reporting as RED, both shown NOT to be mine

- **`wcore-config`: 561 passed, 1 failed** — `profile_home_ignores_control_char_override`.
  Passes **1/1 in isolation**. Cause: `wayland_config_dir_uses_wayland_home_when_set` sets the
  process-global `WAYLAND_HOME`, and its in-source comment claims *"serial isolation is not
  required here … the variable name is unique to this assertion"* — which is **false**; the
  failing test reads the same variable. My `wcore-config` diff is **+33/−0** and touches neither
  test. **MEDIUM, pre-existing, non-blocking → BACKLOG.**
- **`wcore-agent --lib`: 2170 passed, 12 failed** — every one a
  `session journal writer lease is already held` under parallelism. Serially: **77/77** and
  **37/37**. Disk was at 70% (515 G free), so this is *not* the exhaustion cause the phase
  verdict described. Pre-existing parallel-execution contention, non-blocking.

**One regression that WAS mine, and how it was caught:** `cargo clippy --all-targets` failed on
`ToolsConfig` literals in two test helpers that no per-package `cargo test -p` ever compiled.
That is the workspace-vs-package lesson exactly; fixed in `4` follow-up commits.

---

## 6. Seam request — Desktop wire contract (do NOT action in a lane)

> **SR-27-C3-1 — `session_cost` needs a per-tool dimension.**
> Media spend is now recorded per call and reaches the model and any protocol host through
> `ToolResult.output`, but the TUI/Desktop `/cost` screen is fed by
> `ProtocolEvent::SessionCost { per_turn: Vec<TurnCost> }`, which is **per-turn with no per-tool
> dimension**. Proposed forward-additive shape:
> `per_tool[] { tool, backend_id, cost_usd, priced, price_source, units }`, leaving every
> existing `per_turn` consumer untouched. Requires regenerating
> `contracts/desktop/v1/{schema,events,manifest}` via `wcore-contract generate`, which is
> release-coordinated. **Blocked on that decision, not on engineering** — the record it would
> carry already exists and is typed.

---

## 7. What I did NOT do

- **Late-MCP was never exercised.** The fixture makes it reachable; I did not reach it.
- **MCP accounting parity was not built.** An MCP-served media tool still produces no product
  cost record. Asserted as a test so it cannot drift, not fixed.
- **F-27C3-04 (Flux default image model) is open** and is the HIGH in this lane.
- **The combined-shape collision is measured, not resolved** — nothing marks the shadowing.
- **No transcription/voice or intake code was touched** (owned by `voice-bargein` and
  `27-c1-intake`).
- **No PR, no merge, no tag, no issue closed, no `wcore-contract generate`.**
- The `image` subcommand accounting has **no hermetic test** — it is proved live only.

## 8. Shared-file fence

**`crates/wcore-cli/src/lib.rs` and `main.rs` were NOT touched.** Verified against the captured
merge-base, not the branch name.
