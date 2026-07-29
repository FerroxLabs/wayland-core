# 27-C3-MEDIA — running notes (append-only, committed after every measurement)

Lane `27-c3-media`. Worktree `/Users/seandonahoe/dev/waylandcore-frontier-worktrees/lane-27-c3-media`,
branch `lane/27-c3-media`, base `plan/f20-unified-audit-repair` @ `5457710e`.

**Mandate:** Phase 27 Criterion 3 — "Built-in, MCP-only, late-MCP, and combined media
generation expose consistent discovery, credentials, accounting, and failures." Graded
**NOT MET**. Two deliverables: (1) the generation surface with a hermetic MCP fixture,
(2) a cost record, treated as **blocking** because generation is billable.

Out of scope for this lane (other lanes own them): bounded intake (`27-c1-intake`),
voice/transcription/barge-in (`voice-bargein`).

---

## T+0 — inherited state, read from the phase artifacts (not assumed)

### What the verdict says about C3 (`27-PHASE-VERDICT.md` §Criterion 3)

- **None of the four generation shapes was exercised.** No MCP media-tool fixture was built,
  so MCP-only / late-MCP / combined were unreachable.
- One real positive: the honest-degradation advisory reaches the model verbatim on the wire.
  Its gap: **zero events on the protocol stream**, so a Desktop host renders nothing.
- Accounting recorded **SOURCE-ONLY**: cost is token-shaped, a media call produces no cost record.

### The accounting facts already measured by prior lanes — these constrain the design

From `27-FIXES-SUMMARY.md` "Defect 3" and `27-CREDENTIALLED-SUMMARY.md`, both live-probed
against FluxRouter:

| shape | cost in HTTP headers | cost in body | priceable by the provider at all? |
|---|---|---|---|
| transcription | YES — `x-flux-cost-usd`, `x-flux-billed-seconds` | no | yes, from headers |
| **image generation** | **NO** | **NO** (body keys are only `created`, `data`) | **NO — priced in no channel** |
| chat (contrast) | yes | `usage.cost_usd` | already priced |

**This is the hard constraint on my deliverable.** C3's accounting clause covers the
*generation* shapes, i.e. images. The provider returns **no billing figure for an image call
in any channel**. So a cost record for image generation *cannot* be a provider-reported dollar
amount. Any lane that produces one has invented it.

Corollaries I must design around:
1. There is **no tool→cost path in the product at all**. The only cost sink is
   `ProviderBudgetReservation::settle(input_tokens, output_tokens, cost_usd)` — keyed to a
   provider dispatch with token counts. A media tool call has no reservation, no dispatch,
   no tokens.
2. The user-visible surface `ProtocolEvent::SessionCost { per_turn: Vec<TurnCost> }` is
   **per-turn with no per-tool dimension**, and it is a frozen **Desktop wire contract**
   (`crates/wcore-protocol/contracts/desktop/v1/`). Extending it needs
   `wcore-contract generate`, which LANE-BRIEF §0 forbids me to run.
3. `session_cost` is **not** dishonest today — it emits `priced: false` and "cost is unpriced,
   not $0". Re-confirmed by two prior lanes. I must not weaken that.

### The broken observable I was warned off

The brief warns: the existing cost observable is broken — **invariant across harnesses**.
Located: `.planning/phases/30-continuous-scorecard-frontier-review/30-DIALECT-C2.md:291` —
the frontier scorecard's `cost ×3` legs, "**degenerate by construction** — v2 does not repair
it and does not admit so", needing "a cost observable that can vary between conforming
harnesses, or honest reclassification to NOT_MEASURED".

That is the Phase-30 scorecard cost leg, a *different* object from `SessionCost`. **I will not
route the media cost record through it, and I will not cite it.** Recorded here so the
distinction is on the record rather than assumed.

### Design position forming (to be cross-audited before building)

A media cost record that is honest under the above must:
- record **billable units actually performed** (backend, model, endpoint host, n images,
  size class, and `billed_seconds` where a provider does supply one) — these vary with the
  work and are observable without the provider pricing anything;
- carry `cost_usd: Option<_>` + an explicit **price source** (provider header / provider body /
  local rate card / unpriced), never a bare float that a reader will assume is provider truth;
- default to **unpriced** rather than to an invented figure;
- reach the user without a frozen-contract change (CLI/TUI surface + structured record), with
  the wire dimension filed as a seam request.

**The evidence bar I have set for myself:** it is not enough to emit a record. I must show the
record **varies** with the work done — different n, different size, different backend produce
different records — because the failure this programme keeps hitting is an observable that
reports the same value regardless of what happened.

## Still to establish
- [ ] Read `image_generation_tool.rs` + `image_gen.rs` fully; find where a per-call record could hang.
- [ ] Determine the four generation shapes concretely (built-in / MCP-only / late-MCP / combined).
- [ ] Build the hermetic MCP media-tool fixture.
- [ ] Build the cost record + prove variance.
- [ ] Live probe on hetzner with the burn key, reading the arm back from the product's own output.
- [ ] Secret sweep with liveness control.

---

## T+~3h — what is established, with the measurement behind each

### Landed
- `crates/wcore-tools/src/media_cost.rs` — the record. 8 unit tests green.
- `image_generate` emits it into its result JSON (reaches model AND host via
  `ToolResult.output`, needing no wire-contract change) plus a structured
  `wcore::media_cost` tracing line and an optional session ledger.
- `[tools.media_pricing]` operator rate card, empty by default, merged
  global+project key-by-key.
- `crates/wcore-agent/tests/f27_media_generation.rs` — hermetic fixture serving
  BOTH `/v1/images/generations` and MCP streamable-HTTP. 12/12 green,
  0 ignored, 0 filtered.

### Mutation control — 13/13, `MUTATION_CONTROL=PASS`
Every accounting gate proved able to fail (M1 invariant units, M2 dead
provider-cost channel, M3 failure recorded as $0, M4 local estimate laundered
as provider truth). The M2 case is the important one: it shows the *unpriced*
assertion stays GREEN under mutation, so it cannot be the only evidence — the
cost-header control is what carries it.

### Live probe — two instrument defects found and repaired before any result
1. The credential parser anchored on `FLUX_API_KEY` at line start; the file is
   `export FLUX_API_KEY=...`, so it fell through and handed the provider the
   whole shell line. Provider replied `401 ... Received=expo****`. **That reads
   exactly like a dead key and would have been reported as a credential
   blocker.** Repaired, plus a post-parse reject for anything still containing
   `=` or a space.
2. `--json-stream` with a positional prompt exits after the capability
   handshake without driving a turn: three byte-identical 4506-byte captures
   containing no model turn. Switched to `--no-tui`.
3. Third: the run then died at `Session persistence authority unavailable` —
   an isolated headless profile has no vault. Added `[session] enabled = false`.

### Live findings so far (real spend on the burn key)
- **`wayland-core image --model flux-image` SUCCEEDED**: wrote a 199,576-byte
  JPEG, 1024x1024. A genuine billable generation, and `accounting_hits=0` in
  both its streams. **This is a SECOND billable generation surface**
  (`FluxImageClient`, `crates/wcore-providers/src/flux_image.rs`) that does not
  go through the tool and produces no cost record.
- **Arm read back from the product's own output** per §3b-ii: the same boot
  logged `vision: using flux-auto at https://api.fluxrouter.ai/v1/...` and
  `transcription: ... api.fluxrouter.ai`. FluxRouter, not Anthropic.
- **`image_generate` registered with ONLY `FLUX_API_KEY` set** — through the
  config-provider arm the honest-unavailable advisory did not name. Fixed, and
  the anti-drift guard that was structurally blind to it repaired in the same
  commit with a three-assertion self-test.
- **`image_gen` logged `at  `** (empty base_url) where every other media
  resolver printed its full Flux URL. Fixed to log the resolved endpoint.

### Still open
- [ ] Re-run the live probe on the rebuilt binary for the tool-path spend.
- [ ] Decide/act on the `image` subcommand accounting gap.
- [ ] Secret sweep with liveness control.
- [ ] SUMMARY.
