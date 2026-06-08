# wcore-evolve — W10B F12 GEPA loop

`wcore-evolve` is the second-half of the skills lifecycle pipeline. F10
(W9) drafts new skills from observed tool sequences; F11 (W9) curates
them; F12 (this crate) improves them generation-by-generation against
W10A's deterministic eval harness.

## Where it sits

```
W9 F10 draft  →  W9 F11 curate  →  W10B F12 evolve  →  W9 F11 curate (winners)
                                       ↑                       ↓
                                wcore_eval scoring        graveyard (losers)
```

Skills evolve only against W10A's `DefaultScorer` (locked constants
under `crates/wcore-eval/src/scorer.rs`). Scoring NEVER touches an LLM
— W10A is the trust boundary.

## Architecture

```
parent skill ─┐
              ├─► Mutator (round-robin: Paraphrase / Reorder / SwapSynonym / Precondition)
              │
              └─► N children ─► wcore_eval::Scorer ─► ScoredCandidate
                                                       │
                            ┌──────────────────────────┤
                            ▼                          ▼
                  retained (top child)          archived (loser)
                            │                          │
                            ▼                          ▼
                       CuratorPort              graveyard JSON
                       (W9 F11)
```

Single-parent mutation only — no cross-skill recombination this iteration.
The loop terminates on (a) generation ceiling, (b) plateau, or (c) budget
exhaustion.

## Mutator catalogue

All four mutators are deterministic-seeded from `(parent_hash, generation,
child_index)` via blake3 → ChaCha20Rng. The same triple ALWAYS produces
byte-identical output (Paraphrase via fixture-replay; real provider drift
is documented as out-of-contract).

| Mutator | Description | Determinism |
|---|---|---|
| `Reorder` | Shuffles the `## Steps` list with a ChaCha20-derived permutation | Pure-Rust, strict |
| `SwapSynonym` | Picks one (word, substitute) pair from a small static table; replaces FIRST occurrence | Pure-Rust, strict |
| `Precondition` | Adds or drops one row from the `## Preconditions` block. Drops only if list length > 1 (never empty) | Pure-Rust, strict |
| `Paraphrase` | LLM-backed via `ParaphraseProvider` trait. Production wires a real provider; tests use a fixture-replay stub | Fixture-replay strict; real-provider best-effort |

## Budget semantics

The loop queries `Budget::is_exhausted()` between every child + between
every generation. Per-child `tokio::time::timeout(child_timeout, ...)`
also wraps mutator + score, so a hung Paraphrase provider cannot leak
past the loop-level budget (F5 audit fix).

Cancellation slack is bounded to one step's worth of work after
exhaustion — the next child-loop iteration observes the cancellation and
exits.

## Plateau heuristic

```
def plateau(history, window, min_delta):
    if len(history) <= window: return False
    baseline = history[-window-1]
    best_after = max(history[-window:])
    return (best_after - baseline) < min_delta
```

Defaults: `window=3`, `min_delta=0.01`.

**Window MUST be >= number of mutator strategies in rotation** (currently
4) — otherwise one noisy generation can produce a false plateau. The
W10B High 1 audit fix raised the default from 2 to 3 to give every
mutator at least one shot before declaring no improvement.

## Graveyard layout

Filesystem:

```
<graveyard_root>/<run-id>/<generation>/<child_index>.json
```

JSON schema:

```json
{
  "run_id": "run-001",
  "generation": 2,
  "child_index": 3,
  "parent_id": "skill-refactor-imports",
  "mutation_kind": "Reorder",
  "score": 0.42,
  "body_excerpt": "## Steps\n- ..."
}
```

Default root: `dirs::data_dir().unwrap_or_else(std::env::temp_dir).join("wayland/evolve/graveyard")`
— resolves to:
- macOS: `~/Library/Application Support/wayland/evolve/graveyard`
- Linux: `~/.local/share/wayland/evolve/graveyard`
- Windows: `%APPDATA%\wayland\evolve\graveyard`

The path is created with `fs::create_dir_all` on first use. Retention is
host-policy — wcore-evolve never deletes graveyard entries.

## CLI usage

```
wcore-evolve --seed-file <path/to/skill.md> \
             --seed-name <stable-id> \
             --generations 5 \
             --fan-out 4 \
             --plateau-window 3 \
             --plateau-min-delta 0.01 \
             --child-timeout-secs 30 \
             --graveyard-root <optional path>
```

Outputs (one key=value per line):

```
run_id=run-12345-1715xxxxx
parent_id=refactor-imports
generations_run=5
termination=generation_ceiling
parent_score=0.612
best_score=0.683
losers_archived=19
graveyard_root=/.../wayland/evolve/graveyard
curator_decision=promote
```

## Trace event

When a host advertises `capabilities.gepa_enabled = true`, the engine
emits one `evolution_event` per scored child. See
[`docs/json-stream-protocol.md`](json-stream-protocol.md#1n-evolution_event-w10b)
for the wire shape. `gepa_enabled` is INDEPENDENT of `structured_traces`
(F6 audit fix in the W10B revision).

## Out of scope (this iteration)

- Online evolution from live sessions. `wcore-evolve` runs offline against
  the W10A reference set + W1 replay traces.
- Cross-skill recombination (genetic crossover between two parents).
- Mutator meta-evolution.
- Live rollback monitor (the design §"GEPA mutation drift" auto-monitor is
  W11+; W10B emits the events that future monitor needs).
- Multi-session evolutionary state persistence.

## References

- Plan: `docs/superpowers/plans/2026-05-15-wcore-W10B-gepa.md`
- W10A LOCKED PUBLIC SURFACE: `docs/superpowers/plans/2026-05-15-wcore-W10A-eval-harness-spike.md`
- Spec: `docs/superpowers/specs/2026-05-14-wcore-super-agent-design.md` §5.3
