# Decision B — remap operability

CHOSEN: remap-actionable
BASIS: majority

## The vote

| Member | Verdict |
|---|---|
| codex (gpt-5.6-sol) | `remap-actionable` |
| gemini (3.1-pro-preview) | `remap-actionable` |
| kimi (K3) | `remap-actionable` |
| internal adversarial | `remap-actionable`, with a recorded limit |

Unanimous. Codex's first invocation produced NO verdict — it blocked reading
stdin and returned 39 bytes. That is the silent vote-drop the phase warns about,
and it was caught and re-run with stdin closed rather than recorded as a
three-way panel.

## The measurement that binds this choice

Across all four credential backends (`auto`, `plaintext`, `keyring`,
`encrypted-file`):

| Bound quantity | Required for `remap-actionable` | Measured |
|---|---|---|
| `REMAP-NAMES-{BACKEND,COUNT,ACTION}: no` occurrences | 0 | **0** |
| `REMAP-CARRIES-SOURCE-ABSOLUTE-PATH: yes` occurrences | 0 | **0** |
| refusals that nevertheless wrote their target | 0 | **0** |

`REMAP-TARGET-WRITTEN` is obtained by digesting the target before and after the
attempt, not by reading the message — which is how a warn-and-continue would
have been caught. The capture script is proven able to go red: handed a
nonexistent binary it exits 1.

## Against the peer bar

Both peers name what a backup does not carry and state the operator's next
action; neither faces Core's harder case, where a backend's secrets are in the
OS keyring and therefore not in the filesystem at all. Core applies the peers'
own stated pattern to that harder case: the `keyring` refusal names the backend,
names the count, and prescribes `wayland-core auth add <provider>` — a real,
runnable command — rather than reporting success and yielding an install whose
credentials resolve to nothing.

## Recorded limit (dissent, in its own terms)

The `REMAP-NAMES-*` fields are PRESENCE checks. They record that a backend, a
count and an action were named — not that the prescribed action is correct or
achievable. No gate in this plan executes the recovery action and then verifies
the resulting install works. So the claim supported by this evidence is
precisely "the operator is told which backend, how many sources are absent, and
what to do", and NOT "following that instruction is proven to produce a working
install". That gap is real, is not closed here, and is recorded rather than
absorbed.
