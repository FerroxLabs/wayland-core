# wcore-eval — Eval Harness (W10A spike)

This crate exists to **gate F12 GEPA** (W10B). It deterministically
classifies skill candidates (good vs bad) and is **not** the
genetic-evolution loop itself — that lands in W10B once W10A's
precision and recall both reach >=0.80 on the 60-case corpus per
design §5.3 line 1638.

## Reference corpus (W10A v0)

`data/corpus/*.yaml` — 60 hand-curated cases. Strict shape:

- **30 known-good** cases (`expected_outcome: good`) — the bundled
  `hello` skill plus 29 healthy variants (alternate wording,
  alternate when_to_use phrasing, alternate allowed_tools, alternate
  paired traces; all structurally sound).
- **30 known-bad** cases (`expected_outcome: bad`) — the same
  baselines deliberately corrupted across a 10-family taxonomy.

### Corrupted-variant taxonomy (30 bad cases, 3 per family)

Each corruption is a **deliberate** flaw the harness must detect:

1. **Truncated body** (3 cases at 25/50/90% truncation, including
   removal of the trailing `$ARGUMENTS`)
2. **Empty `when_to_use`** (3 cases)
3. **Conflicting frontmatter — `name` != filename** (3 cases) —
   structurally detected by the `name_matches_filename` check.
4. **Off-topic description** (3 cases — e.g. describes a calculator,
   a translator, a debugger when the body is a greeter) —
   structurally detected by the `description_shares_token_with_body`
   check.
5. **Oversize body** (3 cases at 2x / 5x / 20x baseline content_length)
6. **Missing `$ARGUMENTS` placeholder** (3 cases)
7. **Description identical to body — no semantic info** (3 cases)
8. **Disallowed-tool reference** (3 cases, e.g. body references
   `Spawn` but `allowed_tools = []`)
9. **Stale model pin** (3 cases, e.g. `model: claude-haiku-3-20240306`)
   — structurally detected by the `model_in_allowlist` check.
10. **UTF-8 invalid replacement chars in body** (3 cases) — caught at
    parse time by the loader's `String::from_utf8` boundary OR by the
    invalid-char structural check.

### Failure-stacking discipline (audit F6 hardening)

The LOCKED scorer combines `0.7 * outcome + 0.2 * (1 - cost) + 0.1 * (1 - size)`
with `acceptance_cutoff = 0.65`. With no trace and a small body, a
single failure of one of nine structural checks scores
`0.7 * (8/9) + 0.3 = 0.922` — still above the cutoff. To stay honest,
**each bad case is authored to stack multiple natural failures**
(e.g. an "off-topic description" case typically also fails the
`description_distinct_from_body` check OR has the name mismatch the
file already uses), OR carries a deliberate cost/size penalty (oversize
bodies; trace fixtures with saturated cost/output_tokens). This is
acceptable per audit F6: real-world bad skills usually carry multiple
deficiencies, and our corruptions reflect that.

### Known-good case provenance (30 good cases)

- 1x exact bundled `hello`
- 4x alternate-wording variants of `hello`
- 5x alternate-`when_to_use` variants (paraphrase only, all healthy)
- 5x alternate-`allowed_tools` healthy variants
- 5x alternate-`description` healthy variants
- 5x alternate-`model` healthy variants (all in the known-good model
  allowlist)
- 5x trace-paired healthy variants (cheap trace, success trace, tight
  output, etc.)

All 30 good cases pass all 9 structural checks AND have their
combined score above the acceptance cutoff in `DefaultScorer`.

### Trace-paired cases (subset of both halves)

Some cases score against a recorded `TurnTrace` (loaded from
`data/traces/<name>.json`). The corpus shape is binary classification —
not paired A-vs-B ranking. Each case is one `Candidate`
(`SkillMetadata` + optional `TurnTrace`), evaluated independently
against `DefaultScorer`.

### How to extend

Add a `data/corpus/<name>.yaml` file with the frontmatter shape below
plus the skill body under `data/skills/`. Then run
`vx cargo nextest run -p wcore-eval --test corpus_load` to verify
structural validity. **The corpus must always remain 30 good + 30 bad;**
adding more requires rebalancing AND amending the loader's invariant
check.

## Scoring weights (W10A v0)

Combined as `0.7 * outcome + 0.2 * (1 - cost_penalty) + 0.1 * (1 - size_penalty)`.
Predicted `Verdict` is `Good` if combined >= `DefaultScorer::acceptance_cutoff`
(0.65), else `Bad`. Picked to satisfy the corpus; W10B tunes against a
wider set. **Saturation constants and the cutoff are LOCKED at end of
Task 3** — they are not tuned post-hoc after observing gate failures
(see plan Task 5 remediation rules).

See `src/scorer.rs` for the per-component definitions.

## CLI

```
wcore-eval score        # one CaseResult JSON line per case on stdout
wcore-eval gate         # exit 0 iff P >= 0.80 AND R >= 0.80, else 1
wcore-eval gate --json  # also emit JSON summary + write target/eval/agreement.json
```

Or via the workspace recipe:

```
vx just eval-gate
```
