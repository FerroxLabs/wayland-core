# ForgeFlows

A **ForgeFlow** (the Dynamic Workflows feature) is a declarative, multi-stage
agent pipeline authored in [RON](https://github.com/ron-rs/ron). It lets you
describe a fan-out / pipeline / verify topology once and run it as a coordinated
set of sub-agents — instead of asking the model to orchestrate the stages
turn-by-turn.

A ForgeFlow lowers to the engine's existing execution-graph IR and runs through
a dedicated executor (`WorkflowRunner`) over the same sub-agent dispatch path
as the `Spawn` tool. There is no separate runtime: stages are sub-agents.

ForgeFlows are surfaced three ways:

- **`Workflow` LLM tool** — the model runs an inline RON ForgeFlow mid-conversation.
- **`wayland-core workflow` CLI** (alias `forgeflows`) — validate, list, and run
  saved `.ron` ForgeFlows.
- **Shadow-mode detection** — a telemetry-only signal that flags turns that *look
  like* a ForgeFlow (off by default; never prompts or routes — see
  [Detection](#shadow-mode-detection)).

---

## RON ForgeFlow format

A ForgeFlow is authored in RON as a single `Workflow(...)` root document — the
brand is ForgeFlows, but the grammar keyword stays `Workflow(`. It has three
parts: `meta`, an optional `schemas` table, and an ordered list of `phases`.

```ron
Workflow(
    meta: (name: "review-changes", description: "review a diff end to end", est_agents: 7),
    schemas: {
        "findings": "{ \"type\": \"object\", \"required\": [\"findings\"], \"properties\": { \"findings\": { \"type\": \"array\", \"items\": { \"type\": \"string\" } } } }",
    },
    phases: [
        Phase(
            title: "scan",
            steps: [
                // No-barrier pipeline: each item of `changed_files` streams
                // through both stages independently (see below).
                Pipeline(id: "scan", over: Some("changed_files"), stages: [
                    (id: "extract",  prompt: "extract symbols from the file"),
                    (id: "classify", prompt: "classify the extracted symbols"),
                ]),
            ],
        ),
        Phase(
            title: "verify",
            steps: [
                // Parallel fan-out: branches run concurrently and Collect folds
                // their outputs into an array on the `verdict` key.
                Parallel(id: "verdict", branches: [
                    (id: "lint",  prompt: "lint and return findings JSON",
                                  schema: Some("findings"), input: Some("scan")),
                    (id: "audit", prompt: "audit for risk", input: Some("scan")),
                ], join: Collect),
                // A plain agent reading the collected verdict downstream.
                Agent((id: "summarize", prompt: "summarize the verdict", input: Some("verdict"))),
            ],
        ),
    ],
)
```

### `meta`

| Field | Type | Required | Notes |
|-------|------|----------|-------|
| `name` | string | yes | ForgeFlow name (also the saved-file stem for `run <NAME>`). |
| `description` | string | no (default `""`) | Shown in `list`. |
| `est_agents` | integer | no (default `0`) | Author hint only. **The authoritative agent count comes from the IR-walking estimator, not this field** — the estimator ignores it entirely. |

### `schemas`

An optional name → JSON-string table. A step's `schema: Some("findings")` must
resolve to a key here. Bodies use a **minimal JSON-Schema subset**, validated
at parse time:

- `type`: one of `object`, `array`, `string`, `number`, `integer`, `boolean`, `null`.
- `properties` (object only): field name → sub-schema.
- `required` (object only): field names that must be present.
- `items` (array only): one sub-schema every element must match.

Everything else (`enum`, `oneOf`, `$ref`, `pattern`, numeric bounds,
`additionalProperties`) is **out of scope and ignored** — extra properties are
permitted. An unknown `type` keyword or a non-object schema node is rejected
when the workflow is parsed, so an author typo surfaces immediately rather than
at run time.

### `phases` and `steps`

Phases run in declaration order; their `title` is a grouping label. Each phase
holds one or more steps, also run in declaration order — the previous step's
terminal node(s) feed the next step's entry node. There are three step kinds.

**`Agent((id, prompt, ...))`** — one sub-agent call.

**`Pipeline(id, over, stages)`** — an ordered chain of sub-agent calls. Two modes:

- **Chain (no `over`):** the running state flows through the stages once, each
  stage feeding the next. Each stage lowers to its own graph node.
- **No-barrier pipeline (`over: Some(ref)`):** `ref` resolves to an array in the
  running state; **each item streams through all stages independently with no
  barrier between stages** — item A may be in stage 3 while item B is still in
  stage 1. A stage error drops that one item to `null` and skips its remaining
  stages; the run as a whole does not abort. The result is an order-preserving
  array (with `null` holes for dropped items) written to `state[id]`. A
  non-array (or missing) `over` ref runs zero items and writes an empty array.

**`Parallel(id, branches, join)`** — sibling sub-agent calls that run
concurrently and fold into an aggregator named `id`. `join` is one of:

| `join` | Behavior |
|--------|----------|
| `Collect` (default) | Collect each branch's output into an array on `state[id]` (installs the `Collect` reducer). |
| `Merge` | Maps to the same `MergeObjects` aggregation strategy as `Collect`; in v1 the runner's default fold for that strategy also yields an array of branch outputs. |
| `Concat` | Maps to the `ConcatOutputs` strategy; in v1 the runner's default fold also yields an array of branch outputs. |

> In v1 all three joins ultimately produce an array of branch outputs on
> `state[id]` — `Collect` via its installed reducer, `Merge`/`Concat` via the
> aggregator's default fold. The distinct deep-merge / string-concat semantics
> are reserved for a later iteration.

A `Parallel` step requires **at least two branches** (a one-branch fan-out is
rejected as degenerate).

### Agent fields

Every agent / stage / branch shares the `AgentSpec` shape:

| Field | Required | Meaning |
|-------|----------|---------|
| `id` | yes | Stable graph node id. Must be unique across the whole workflow. |
| `prompt` | yes | The instruction the sub-agent runs. |
| `schema` | no | Named schema ref (must exist in `schemas`). |
| `model` | no | Parsed and reserved; **not yet wired to a per-stage model override in v1**. |
| `input` | no | Flat-key ref into the running state. Selects `state[<key>]` and injects it into the prompt; absent means the whole state is passed through. |

### Data flow between stages

Each agent stage writes its output to `state[<id>]`. A later stage reads an
earlier stage's output by naming it in `input` (e.g. `input: Some("scan")`).
For schema-bearing stages the **parsed, structured JSON** is stored (so a
downstream stage sees an object/array, not a JSON string); schema-less stages
store their text.

A cross-stage `input` ref is validated at parse time against ids declared
earlier in the workflow — a dangling ref is a parse error. (Exception: a
no-barrier pipeline's per-item stage `input` selects a field of the *item
value*, not a prior node, and the `over` ref resolves against the running state
at execution time, so neither is checked against earlier ids.)

### Parse-time validation

`WorkflowPlan::parse` returns a typed error (each carries a field/location
pointer) for: invalid RON syntax, a workflow with no phases, an empty phase, an
empty agent id, a pipeline with no stages, a parallel step with fewer than two
branches, a duplicate node id, an unknown schema ref, a dangling input ref, and
an invalid schema body.

---

## The `Workflow` LLM tool

The agent has a built-in `Workflow` tool (category **Exec**, registered
alongside `Spawn`) that runs a ForgeFlow. The model invokes it with an inline
RON string:

```json
{ "workflow": "Workflow(meta: (name: \"...\"), phases: [ ... ])" }
```

The tool parses the RON, runs it through `WorkflowRunner`, and returns a
per-stage summary plus the final state. Invalid RON returns the typed parse
error to the model (the ForgeFlow does not run); a mid-run failure returns the
error **plus the partial result** so completed stages are not lost.

**v1 limitation — empty initial state.** The `Workflow` tool runs with an empty
initial state object. A tool-invoked `Pipeline(over: ...)` therefore has no host
data to stream over yet (its `over` ref resolves to nothing and the pipeline
runs zero items). Tool-invoked ForgeFlows that need a starting collection should
produce it in an earlier stage rather than rely on injected host data. The CLI
`run` path has the same behavior (empty initial state). Host-injected initial
state (e.g. a `changed_files` array) is exercised today through the
`WorkflowRunner` API directly.

---

## The `wayland-core workflow` CLI

Saved ForgeFlows live in `<project-root>/.wayland/workflows/*.ron`. The project
root is the nearest ancestor of the cwd containing a `.wayland` directory
(falling back to `<cwd>/.wayland/workflows`). The subcommand is `workflow`, with
the visible alias `forgeflows` — `wayland-core forgeflows list` works too.

```bash
# Parse and validate a single .ron file (no execution, no provider).
wayland-core workflow validate path/to/review.ron

# List saved ForgeFlows: "name  ~N agents  — description".
wayland-core workflow list

# Run a saved ForgeFlow by name (resolves .wayland/workflows/<NAME>.ron).
wayland-core workflow run review-changes
```

- **`validate <FILE>`** prints `OK: <name>` plus the node count and the
  IR-estimated agent count, or the typed parse error (with its field pointer)
  and a non-zero exit. Pure — no provider is constructed.
- **`list`** discovers, parses, and summarizes each `.ron`. Unparseable files
  are skipped with a warning to stderr, never aborting the listing. The agent
  count per line comes from the estimator, not `meta.est_agents`.
- **`run <NAME>`** resolves the name, parses, builds a provider + spawner from
  the same config-resolution path as a normal launch, prints a pre-execution
  estimate to stderr, executes through `WorkflowRunner`, and prints the
  structured outcome (per-stage records + final state) as JSON. This is the
  explicit tier — the operator opted in by invoking `run`, so there is no
  confirm gate.

> The CLI binary is `wayland-core`; the subcommand is `workflow` (alias `forgeflows`).

---

## Architecture

Workflows live in `crates/wcore-agent/src/orchestration/workflow/`. The pipeline
is **RON → `GraphConfig` → `WorkflowRunner`**.

1. **Lowering (`dsl.rs`).** `parse_workflow` / `WorkflowPlan::parse` parse the
   RON and *lower* it onto the engine's existing `GraphConfig` DAG builder
   (`empty()` + `add_*` + edges) — **no custom IR is introduced**. `Agent`
   becomes one `AgentCall` node; a classic `Pipeline` becomes a chain of agent
   nodes; a `Parallel` becomes sibling agent nodes feeding an `Aggregator` (via
   a synthetic fan-out root). A no-barrier `Pipeline(over: ...)` lowers to a
   single placeholder node and parks its stages in a side-table. The graph IR
   carries only node ids + input mappers, so a `WorkflowPlan` keeps the
   per-node prompts, the schema table, and the no-barrier pipeline defs that the
   IR drops.

2. **Execution (`runner.rs`).** `WorkflowRunner` walks the lowered graph in
   topological (Kahn) order and dispatches each `AgentCall` through the
   `AgentSpawner` path (`spawn_one` for a single node, the per-task parallel
   helper for a sibling fan-out) — **the same sub-agent dispatch the `Spawn`
   tool uses**. It does **not** reuse the per-turn `ExecutionGraph` walker:
   that walker is on the hot path of every tool-bearing turn and is limited to
   one real agent call per turn by a first-dispatch-wins guard. `WorkflowRunner`
   runs **outside** that one-batch-per-turn contract, so every stage runs for
   real. Aggregator nodes fold their inbound siblings per the node's
   `StateReducer`; `PassThrough`/`End` are inert sinks unless the id is a
   no-barrier pipeline.

3. **No-barrier pipeline (`pipeline.rs`).** The item-level streaming mechanic is
   implemented in the runner, not by modifying the walker. A shared semaphore
   caps total in-flight stage agents (including schema-retry re-dispatches) so a
   wide pipeline cannot starve the relay/heartbeat tasks.

4. **Schema validation (`schema.rs`).** A schema-bearing stage's output is
   validated against the compiled subset. On mismatch the runner re-dispatches
   the same stage with the validation error appended to the prompt, up to a
   retry budget (1 original + 2 retries); on success it stores the parsed
   `Value`; on exhausting the budget it surfaces a typed
   `SchemaValidationFailed` carrying the partial result.

5. **Cost estimate (`estimate.rs`).** `estimate(plan, initial_state)` statically
   walks the lowered IR: every `AgentCall` node is one dispatch, and each
   no-barrier pipeline contributes `stages × cardinality(over)` (resolving `over`
   against the provided initial state, falling back to a flagged floor when it
   can't be resolved). The author's `meta.est_agents` is **never** the source of
   truth. The CLI uses this for `validate`/`list`/`run` summaries.

**Partial results on failure.** When a stage fails, `WorkflowRunner` returns a
typed error (`StageFailed` / `SchemaValidationFailed`) carrying the partial
state and the per-stage results gathered so far — completed work is never
discarded. There is no full resume/journaling in v1 (this is the cheap subset).

This module is the lowest crate where the workflow engine semantically belongs:
it reuses `wcore-agent`'s `GraphConfig` IR and `AgentSpawner`, and the per-turn
`ExecutionGraph` walker is left untouched.

---

## Shadow-mode detection

A ForgeFlow can also be *detected* from a normal turn — but in v1 this is
**shadow-only and telemetry-only**.

The flag `[observability] workflow_detection_enabled` (default **off**) gates a
cheap per-turn heuristic that flags turns which look like a fan-out /
multi-step audit / "be comprehensive" ForgeFlow. When on, it emits a
`workflow_detection` trace record describing what the Detected tier *would have*
proposed (confidence, rationale, a capped task excerpt) so real-traffic
precision can be measured.

When the flag is off (the default), the heuristic is **not even computed**, so a
default-config session behaves byte-for-byte as before. Even when on, the signal
**never** prompts the user, routes the turn, or selects a template — it only
writes to the trace log.

**v1 limitation.** The live confirm gate ("Run as a Fleet workflow? ~N agents /
~$X / ~T min — [y/N]") and the detected-tier auto-proposal are **not active in
v1**. Detection is shadow-mode only; there is no silent fan-out and no implicit
pre-authorization. The Auto tier (run without asking) is gated on the
shadow-mode precision phase and deferred. When the confirm gate does land, a
ForgeFlow approval will pre-authorize nothing — every inner `Edit`/`Exec` call
keeps its own tool gate.
