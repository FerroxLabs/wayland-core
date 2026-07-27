# Decision B — can an operator act on what a cross-machine restore tells them about secrets the archive could not carry?

`wayland-core backup restore` must never emit a restored config that points at
credentials which are not present. Judge the operator-facing behaviour.

Answer with exactly one line `PANEL-VERDICT: <option-id>` and one line
`PANEL-BASIS: <one sentence>`, then up to 250 words.

## Options (rotated order; ids are fixed)

### `remap-warns-and-continues`
- **Name:** The refusal is not a refusal — it warns and writes the target anyway
- **Pros:** Names the worst outcome honestly: a restore that reports a problem and then produces the broken install anyway is worse than one that refuses, because the operator believes they were warned and covered.
- **Cons:** Contradicted if every captured refusal left its target unwritten, measured by digest rather than read off the message.

### `remap-actionable`
- **Name:** An operator can act on what the restore tells them
- **Pros:** Every backend's message names the backend, the number of credential sources that will be absent, and a concrete next action; no refusal writes its target; and no source-machine absolute path survives into a restored config.
- **Cons:** Nothing is recorded as outstanding, so any residual opacity the capture did not measure ships as acceptable.

### `remap-blocked`
- **Name:** No honest remap can be defined for some backend at all
- **Pros:** Would be the honest outcome if some backend could only be handled by moving a secret Core cannot read or by emitting a config pointing at nothing.
- **Cons:** A REFUSAL naming the backend and the operator's next action is itself an honest outcome, so this is reachable only when even that cannot be built.

### `remap-opaque`
- **Name:** The message does not tell an operator enough to act
- **Pros:** A message that says a restore is incomplete without naming which backend, how many secrets, or what to do next leaves the operator to guess.
- **Cons:** Contradicted if every captured message names all three of backend, count and action.

## Evidence in this directory

- `remap-records.txt` — all four credential backends (`auto`, `plaintext`,
  `keyring`, `encrypted-file`), each with its exit status, disposition, whether
  the target was written (MEASURED by digesting the target before and after, not
  read off the message), whether the message names the backend / the count / an
  action, whether any source-machine absolute path survived, and the verbatim
  operator message between `REMAP-MESSAGE-BEGIN` / `REMAP-MESSAGE-END`.
- `peer-bar.txt` — the bar taken from two real peer checkouts on this host.

The capture script is proven able to go red: handed a binary that does not
exist it exits 1.
