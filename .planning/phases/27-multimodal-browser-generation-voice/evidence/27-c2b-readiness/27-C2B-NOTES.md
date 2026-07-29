# 27-C2(b) — lane NOTES (append-only, committed as I measure)

Lane: `lane/27-c2b-readiness`. Base: `d622cb09de01329cef6f20d6f9183df171462daf`
(asserted against `git ls-remote gh plan/f20-unified-audit-repair` — matched).

## Headline, established in the first 20 minutes

**My brief's premise (b) is STALE. The fix already landed in integration at
`85b60a2f fix(agent): advertise browser/CUA capabilities on liveness, not linkage`.**

Per `LANE-BRIEF.md` "Your brief's MEASUREMENTS are probably stale", reporting which
claims held is part of the deliverable. Measured at base:

| brief / ledger claim | verdict |
|---|---|
| `bootstrap.rs:754` is `PluginRunner::new().with_computer_use_advertised(true)`, unconditional | **HELD.** Verbatim at `:754`. |
| the in-source justification is per-OS reify-time self-gating | **HELD.** `bootstrap.rs:745-753`. |
| therefore "`browser_suite`/`computer_use` are advertised on the basis of whether a plugin crate is linked" | **FALSE at base.** The wire flags are NOT produced by `:754`. |

`:754` is a **reify-time registry gate** on `CuaToolSpec` capture (`plugins/adapters/cua_adapter.rs:80`
returns `CapabilityDisabled` when false). The **wire capability flags** are produced ~187 lines later
at `bootstrap.rs:939-942`:

```rust
let plugin_capabilities =
    crate::output::protocol_sink::PluginCapabilitySet::from_verified(&verified_plugins)
        .narrowed_to_live()
        .await;
```

`narrowed_to_live()` (`output/protocol_sink.rs:186-219`) runs
`wcore_browser::liveness::probe(CamoufoxBackend::default_url()).await` and
`wcore_cua::liveness::probe()`, and clears the flag on `Unavailable`. Both probe modules
exist and are non-trivial: `wcore-browser/src/liveness.rs` (11.0K),
`wcore-cua/src/liveness.rs` (6.8K).

So the ledger row cites a **real, unchanged line** that does **not** support its
conclusion. Two different properties were conflated: reify-time tool admission vs.
wire-published readiness.

## Instrument liveness for the greps above

Absence claims per §3b-i. Known-positive in the same invocation:
`/usr/bin/grep -rn "with_computer_use_advertised\|computer_use_advertised" --include='*.rs' crates/`
returned **31 hits across 11 files** (non-zero ⇒ grep alive), including the `:754` needle.
`/usr/bin/grep -rn "narrowed_to_live"` returned **5 hits in 3 files** — one of them the
production call site in `bootstrap.rs`, which is the claim that matters.

## What I still have to establish

1. Does the probe **actually narrow on a real headless box** (hetzner)? Both directions.
2. Does the user now get a **clear refusal** instead of `spawn camoufox: No such file or directory`?
3. Is `wcore_cua::liveness` reachable on the **default** feature set, or is it behind a
   feature that is off in the shipped binary (the `27-C4` `voice` trap)?
4. Did the wire shape change (contract corpus counts)?
5. Whether the reify-time-self-gating argument in `bootstrap.rs:747-753` is right.
6. Clause (c) — out of my scope; name what it still needs.
