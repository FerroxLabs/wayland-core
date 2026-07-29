# F05-TRUTH-4 (learned policy) — live measurement of the wiring

**Binary:** `wayland-core 0.12.25` release, built on `hetzner-dsm` from `lane/22-remaining`
at `a39d3945` (`/root/wayland-22-remaining/target/release/wayland-core`, 2026-07-29 13:33).
**Harness:** `../learnedpolicy-live.sh` + `../canned_delegate.py`.

## Claim under test

COMPETITIVE-LEDGER `F05-TRUTH-4`, cited by the `GOAL-*` row as a checkable blocker:

> | 4 | Learned policy | Unavailable: runtime path unwired | None | GOAL-* | **Unchanged.** No adapter surface was built |

At base this was TRUE, and worse than the row says: `AgentExecutorConfig` carried a `pub
learned_policy` field with **zero readers in the entire workspace** while its own doc comment
claimed `dispatch_once` consulted it. Setting it did nothing.

## The measurement: one run, two `Read` calls, one variable

The parent and the delegated child each issue a `Read`. Everything is held constant — same
binary, same session, same on-disk policy, same tool, same argument shape. **The only
difference is the caller class**, which is exactly what the pre-filter keys on.

The child's tool results never reach the parent's JSON stream (children get a `NullSink`, see
§Limitation). They are read back instead from **the product's own conversation state**: the
engine feeds each tool result into the child's next provider request as a `tool` message, and
the canned endpoint logs it verbatim.

### Positive arm — `~/.wayland/permissions.toml` present, `Read` = `deny-always`

`pos/canned-requests.log`, verbatim:

```
role=parent step=1 ...
    last_tool_result[parent:2] = "     1\tparent probe content"
role=child  step=1 ...
    last_tool_result[child:2]  = "Denied by sub-agent learned policy: Read matched rule `*`"
    last_tool_result[parent:3] = {"results":[{"name":"delegate-WL22P-CHILD-MARKER",
                                  "status":"completed","turns":2}]}
```

- **Parent (`CallActor::Root`): NOT narrowed.** The file was read. Root bypasses the
  pre-filter by design, and this shows it does — with the deny rule live on disk.
- **Child (`CallActor::SubAgent`): narrowed.** The dispatch never happened; the model got the
  denial instead of the file. The file itself is real and readable — the parent just read one
  beside it — so the denial is attributable to the policy and not to a missing file.

`pos/stream.jsonl`, the capability chain straight off the product:

```
learned_policy: declared -> configured -> constructed -> ready
```

The product no longer emits `unavailable / runtime_path_unwired` for this capability, and
`RuntimePathUnwired` is no longer reachable for it in source (asserted by
`learned_policy_is_unavailable_without_a_constructed_policy`).

### Control arm — no policy file, one variable

`neg/canned-requests.log` + `neg/stream.jsonl`. Identical command, identical script; the only
change is `POLICY=0`, which skips writing `permissions.toml`.

| | policy present | policy absent |
|---|---|---|
| capability chain | `declared→configured→constructed→`**`ready`** | `declared→`**`unavailable / disabled_by_config`** |
| parent `Read` (Root) | `"     1\tparent probe content"` | `"     1\tparent probe content"` |
| **child `Read` (SubAgent)** | **`"Denied by sub-agent learned policy: Read matched rule `*`"`** | `"     1\tchild probe content"` |

Both runs exit `PRODUCT_RC=0`, so a gate asserting exit status would have distinguished
nothing. Both arms carry `PROBE_POSITIVE_HTTP=200` and `PROBE_NEGATIVE=000rc=7` (adjacent
dead port), so the instrument is shown alive and falsifiable in each run.

## Compile-time falsification (separate, in-tree)

`cargo nextest run -p wcore-agent --test actor_acl_test`: **8 run, 8 passed, 0 skipped** (at
base: 1 of 6, five `#[ignore]`d). Severing only the pre-filter's input
(`let learned = cfg…` → `= None`), changing nothing else:
`NEGATIVE_CONTROL_RC=100`, **`sub_agent_with_deny_policy_short_circuits` FAILED**, other 7
unchanged. Restored, restoration verified by file content rather than by `git diff`'s
unconditionally-zero exit status.

## Limitation, stated rather than buried — the F05 *outcome-proof* column is NOT closed

`emit_learned_policy_occurrence` fires on a real narrowing, but **no current topology can
observe it.** `OutputSink::emit_capability_activation` has a **default no-op** body
(`output/mod.rs:240`) that only `ProtocolSink` overrides; every spawned child gets either
`NullSink` (`spawner.rs:2257`, the `Delegate` path) or `ChannelSink` (the `Spawn` /
workflow-runner relay), and **neither overrides it**. Since `Root` bypasses the pre-filter by
design, the occurrence can only ever fire inside a child, and every child discards it.

So for `F05-TRUTH-4`:

| column | before | after |
|---|---|---|
| effective startup truth | `Unavailable: runtime path unwired` | **`Ready` from a constructed on-disk policy** (else `disabled_by_config`) |
| runtime outcome proof | `None` | **still `None` in practice** — emitted, structurally unobservable |

**This is a general finding, not specific to `learned_policy`: no sub-agent capability
activation of any kind is observable on any topology in this tree.** The fix is for
`ChannelSink` to forward capability activations to the parent's sink, which needs a relay
event and therefore a contract regeneration this lane may not run. Named here rather than
attempted, and NOT counted as closed.
