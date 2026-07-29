# 30-DIALECT — running notes (append-only; committed early per LANE-BRIEF §6b-i)

Lane `lane/30-dialect`, branched from `plan/f20-unified-audit-repair` @ `8bcb052b`.
Started 2026-07-29.

---

## T+0 — what I read, and what the defect actually is

Read: `30-PHASE-VERDICT.md` §2, `30-02-TRIAL-PROTOCOL.md`, `30-02-TRIAL-RESULTS.md`,
`.planning/SEAM-REQUESTS/30.md` (SR-30-3), `evidence/30-02/protocol.json`.

The frozen canonical script (`protocol.json.fixture_script`) emits, for correctness / recovery /
cost, a tool call literally named **`write_file`** with arguments `{path, content}`; for security,
**`read_file`** with `{path}`. Measured outcome: Hermes 30/30 on correctness and recovery,
Wayland 0/30, OpenClaw 0/30. Wayland's equivalent tool is named `Write`.

So the script is not neutral — it is one tool's dialect. Two of three harnesses failed to *parse*
the task, not to *do* it. All nine RUN legs are confounded and 30-03's
`confounded_leg_supports_no_comparison` mechanically refuses every comparison resting on them.

## T+0 — the constraint that shapes the whole design

`protocol.json` is **frozen** and pre-registered (commit `a7bd5d87`, provably before any
measurement). Amending it is the single forbidden act of 30-02. So this lane must produce a
**new pre-registration (protocol v2)** carrying translation digests — not an edit to v1.
SR-30-3 says this explicitly.

## T+0 — where a tool's own dialect can be obtained mechanically (the key idea)

An OpenAI-compatible agent harness **declares its own tool schema on the wire**: the `tools:`
array of the `/v1/chat/completions` request body it sends to the model. The loopback fixture
already sits on that wire and already records request bodies for the purpose of routing.

Therefore the translation does **not** have to be hand-written per tool. It can be *derived from
the peer's own bytes*: capture each harness's declared `tools` array in an unscored discovery
pass, then compile the canonical semantic intent into whichever declared tool the harness itself
advertises for that intent, using that harness's own declared parameter names.

This is the difference between "I wrote a mapping for each tool" (bias hides here) and
"each tool told the meter what it exposes, and one identical rule read all three answers".

## T+0 — the bias attack surface I have to close, stated before I build

A dialect compiler is where a vendor-run benchmark would cheat. Named risks:

- **R1** — I translate Wayland's intent faithfully and the peers' sloppily.
- **R2** — the matching rule keys on tokens that happen to be Wayland's names (`Write`, `Edit`).
- **R3** — I retry / hand-tune until Wayland's translation works and stop early on a peer.
- **R4** — an ambiguous match gets resolved "helpfully" for us and "strictly" for them.
- **R5** — I claim byte identity across tools when the bytes necessarily differ.

Planned guards (to be built and panel-attacked, not asserted):

- **G1** pre-register the intent vocabulary *before* capturing any schema, commit order provable;
  assert no vocabulary token is a product name.
- **G2** compile on **anonymized** schemas — the compiler never receives tool identity, and a
  **label-permutation self-test** asserts output is invariant under relabelling.
- **G3** **refuse on ambiguity**: no clear winner ⇒ `DIALECT_UNRESOLVED` ⇒ leg UNPROVEN.
  A refusal is never scored as a peer failure.
- **G4** exactly one compilation attempt per tool from the same inputs; any hand edit invalidates
  the digest.
- **G5** hash every translation; state plainly that translations are *semantically* equivalent and
  **not** byte-identical (codex's exact prescription in SR-30-3).

## T+0 — scope decision, taken up front

Per the lane brief: **do not publish re-taken comparatives in this lane unless the panel clears
the compiler.** Deliverable is the compiler + its bias guard + a registered protocol v2 + panel
verdict + a statement of what becomes re-takeable. Live evidence targeted at the *discovery*
pass (real declared schemas off the wire), which is not a comparative.

## STILL TO ESTABLISH

- [ ] Does the fixture retain enough of the request to read the `tools` array at discovery time?
      (It records digests, not bodies — SR-30-1. Discovery may need its own capture path that is
      NOT a change to the frozen meter.)
- [ ] Are the peer installs still present on hetzner from lane/30-02?
- [ ] Compiler location: new module in `wcore-eval-scenarios` (not `fixtures/openai.rs`, fenced).
- [ ] Panel run (4-way) on the compiler + protocol v2, before any re-run.

---
