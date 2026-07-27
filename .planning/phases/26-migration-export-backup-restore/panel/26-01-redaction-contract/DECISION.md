# Decision — the redaction contract (F26-01)

CHOSEN: contract-holds
BASIS: majority
RATIONALE: Three of four captured verdicts settled on contract-holds, and the measurement outranks the argument here: the multi-emitter probe drives a plan built from the canary corpus through serde, Debug, Debug-alternate, Display and the anyhow error path and finds zero of the 36 manifest-declared canaries, while its positive half proves 13 credential references were actually emitted so the absence is not vacuous. Every path a dissenting member NAMED was closed in code and re-measured rather than voted away.

## What was measured

- Multi-emitter probe, Linux, non-vacuous (1 test selected, not 0): PASS.
- `cargo nextest run -p wcore-config -p wcore-cli`: 2628 run, 2627 passed. The single
  failure is a PRE-EXISTING hermeticity finding in `crates/wcore-gateway/src/service.rs:321`,
  present verbatim at the base commit `de977949` and in a file this work never touched.
- Real arm64 binary against the REAL `~/.hermes` and `~/.openclaw`: 7 real secret values
  extracted (non-vacuous), **0 hits** in either emitted document, both trees unmutated.
- Canary corpora through the real binary: 36 and 28 canaries declared, 0 in either document.

## How the split was resolved — by closing paths, not by counting

The panel ran three rounds. Each time a member named a concrete path, the plan's rule was
applied ("extend the probe to cover it and re-measure rather than voting"):

| Round | Named path | Resolution |
|---|---|---|
| 1 | `DiscoveredItem.details` is an untyped `BTreeMap<String,String>`; an MCP `url` with `?token=`, a `command` with `--api-key`, or a `base_url` with userinfo travels through it | `scrub_detail` added and applied at every insertion; probe extended to exactly those shapes |
| 2 | `details` was still a `pub` field, so a struct literal or a deserialized document bypassed the scrubber — a sanitizer, not an invariant | `details` made private; `insert_detail` is the only writer; `deserialize_scrubbed` covers parsing; struct literals unreachable outside the module |
| 3 | `CredentialRef::name` is a public deserializable `String`, so a value could be smuggled through the field reserved for a name | `name` narrowed to `[A-Za-z0-9_.:/-]+` under 128 chars on every construction path including deserialization, with negative controls proving real names survive |

## DISSENT

**codex — `contract-cosmetic`** (captured verbatim in `codex.txt`, round 3; earlier rounds in
`round1/` and `round2/`). Its final recorded basis:

> CredentialRef::name and CredentialRef::source_file remain public deserializable Strings
> through which a credential value can enter and later be emitted.

This dissent is recorded rather than disposed of, and two things must be said plainly about it.
**First, it was acted on**: the `name` field was narrowed structurally by commit `f63da68a`
AFTER this capture was taken, which closes the specific mechanism named. The dissent is
therefore preserved against the code as it stood when the vote was cast, not against HEAD — a
fourth round was attempted and codex timed out on the larger bundle twice, so its pre-fix
verdict stands as the honest record rather than being replaced by an assumed one.
**Second, its residual form is real and is carried forward** as named follow-up rather than
claimed closed: `source_file` is still a free `String` (though it is produced only by the
walk's own `relative_to()` and is never a discovered value), and `McpServerConfig::headers` is
not currently emitted but would reopen a channel if a future edit began emitting it. The
internal adversarial pass records both.

No member voted `contract-leaks`, and no measurement supports one.

## Verdict tally (computed from the captured verdict lines)

| Member | Verdict |
|---|---|
| codex | contract-cosmetic |
| gemini | contract-holds |
| kimi | contract-holds |
| internal (adversarial) | contract-holds |
