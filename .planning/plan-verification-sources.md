# Where each verdict in `plan-verification.json` came from

Every entry is the verdict of an **independent adversarial verifier** — an agent told to
refute the lane's claims, which re-ran the lane's gates itself rather than reading its
report. A lane with no entry here is **not** assumed good: it renders as CLAIMED, which is
the honest state for work nobody has independently checked.

`CONFIRMED` is the only value that promotes a `met` criterion to DONE.

| lane | verdict | verifier run |
|---|---|---|
| `acp-mcp` | CONFIRMED | `wf_b4fc8e88-9f8` |
| `bookkeeping` | CONFIRMED | `wf_b4fc8e88-9f8` |
| `floor-disclosure` | CONFIRMED | `wf_b4fc8e88-9f8` |
| `win-bash` | CONFIRMED | `wf_70ca1d20-0ed` (re-run) |
| `atref-residuals` | PARTIAL | `wf_b4fc8e88-9f8` |
| `budget-guards` | PARTIAL | `wf_b4fc8e88-9f8` |
| `mcp-gate-mode` | PARTIAL | `wf_b4fc8e88-9f8` |
| `channel-caps` | PARTIAL | `wf_70ca1d20-0ed` (re-run) |
| `instrument-integrity` | PARTIAL | `wf_70ca1d20-0ed` (re-run) |
| `prompt-cache` | PARTIAL | `wf_70ca1d20-0ed` (re-run) |
| `win-owned-tree` | PARTIAL | `wf_70ca1d20-0ed` (re-run) |

## Why five lanes were verified twice

Six verifier agents in `wf_b4fc8e88-9f8` died. The journal's `failed` block gives the
reason: `Not logged in · Please run /login`. It was not disk pressure and not context
exhaustion — I diagnosed it as each of those first and was wrong both times, and rewrote
every verifier prompt on the second wrong theory before reading the journal. Four of the
transcripts I had used as evidence belonged to agents that finished cleanly.

**Read the workflow journal, never the agent transcripts.** The journal states the
terminal condition; a transcript only shows what the agent happened to say.

The five re-runs in `wf_70ca1d20-0ed` are the authoritative verdicts for those lanes, and
they were not kind: four of the five came back PARTIAL, and two of those PARTIALs were
**live product defects in code already merged to `integ/f13`** — the `core#360` zero-cap
message drop and the `core#358 c4` control that retries an over-broad kill into green.

## Lanes with no entry

`approvals-rest`, `telegram-topic`, `flake-584`, `container-latch`, `352-macos-green`,
`fix-flake-allowlist`, `fix-shared-lib`, and every `2-platform` / `2-decompose` /
`2-maintainer` lane have no independent verdict yet. Their `met` criteria render as
CLAIMED on purpose. Do not report a CLAIMED criterion as done — the whole reason this file
exists is that v0.13.10 shipped claiming 22 issues closed when 9 met their criteria.
