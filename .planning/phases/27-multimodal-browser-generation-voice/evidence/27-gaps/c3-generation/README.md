# C3 — the four generation shapes, exercised

Phase 27's verdict read: "**None of the four generation shapes was exercised.**
No MCP media-tool fixture was built, so MCP-only, late-MCP and combined were
never reachable."

All four are now exercised against the real binary over the real host
protocol, on `hetzner-dsm`, at lane HEAD. `10 PASS / 0 FAIL / 7 NOT MEASURED`.

## What unblocked them

Two things, in order:

1. **A local-model route that actually works.** Three of the four shapes need
   the agent engine to BOOT, and the engine would not boot without a provider
   credential — including via the credential-free path it advertises itself.
   That is the `9fe6ad86` fix. Without it these shapes stay unreachable for
   anyone without a paid key.
2. **A deterministic MCP media fixture** (`scripts/f27-mcp-media-fixture.mjs`),
   advertising one generation tool that succeeds with fixed bytes and one that
   fails the way a paid-but-uncleared arm fails.

No credential of any kind was present in any of these sessions. Seven
credential-bearing environment variables were stripped and `WAYLAND_HOME` was
a throwaway directory.

## Results by shape and clause

| Shape | discovery | credentials | failures | accounting |
|---|---|---|---|---|
| A built-in | PASS | PASS | PASS | NOT MEASURED |
| B MCP-only | PASS | PASS | NOT MEASURED | NOT MEASURED |
| C late-MCP | PASS | PASS | NOT MEASURED | NOT MEASURED |
| D combined | PASS | PASS | NOT MEASURED | NOT MEASURED |
| control absent-server | — | — | PASS | — |

## The late-MCP shape fails closed, and that is correct

First run, `AddMcpServer` was refused:

```
mcp_failed: {"f27media-late": "active assistant identity is required for a
             runtime MCP declaration"}
```

This is deliberate per-assistant MCP scoping (#111), not a defect. The shape
was re-run with `--assistant f27-shapes` supplied, and the server connected.
Recording it here because the first result looks like a failure and is not
one — a runtime MCP declaration with no assistant identity is refused by
design, loudly, with a named cause. That is the behaviour Criterion 3's
"failures" clause asks for.

## Finding — discovery is NOT consistent across shapes (MEDIUM)

Criterion 3 asks for **consistent** discovery. Measured, it is consistent
*within* a shape and inconsistent *across* them. The same fixture, the same
two tools:

| Shape | Names the host is told |
|---|---|
| B MCP-only (config server alone) | `media_generate_image`, `media_generate_locked` |
| C late-MCP (late server alone) | `media_generate_image`, `media_generate_locked` |
| D combined (both) | late server: `media_generate_image`, `media_generate_locked`<br>config server: `mcp__f27media__media_generate_image`, `mcp__f27media__media_generate_locked` |

Two observations:

1. **A tool's host-visible name depends on what else is in the session.** A
   host that learned `media_generate_image` from a config server in one
   session sees that same server's tool as
   `mcp__f27media__media_generate_image` in the next, purely because a late
   server happened to carry a colliding name.
2. **The late server wins the bare name and the config-declared server is the
   one renamed.** `RemoveMcpServer`'s own doc comment states "Configured and
   plugin-owned servers remain authoritative." The disambiguation resolves
   the other way.

Prefixing on collision is a sound strategy; which side gets prefixed is the
part that looks inverted. Graded **MEDIUM** — the names stay unique and
functional within a session, nothing is silently dropped, and no security or
correctness property is broken. Per the standing severity policy MEDIUM goes
to BACKLOG rather than blocking. It is recorded here because it is a direct,
measured answer to the word "consistent" in the criterion, and Criterion 3
cannot honestly be called fully met while it stands.

## What is NOT MEASURED, and why — not zero, and not passing

**Accounting, all four shapes.** The criterion asks whether media generation
produces a consistent cost record. No generation completed in any shape:

- Shape A needs a cleared `FLUX_API_KEY`. **This is the named blocker.** It is
  Sean-reserved; no key was embedded, copied or printed.
- Shapes B/C/D register the media tools successfully, but *invoking* one needs
  a model turn to call it. The local-model route boots the engine; it does not
  supply inference. No Ollama server runs on `hetzner-dsm`, so no tool call was
  issued and there was no cost record to inspect.

The phase verdict recorded accounting as SOURCE-ONLY ("cost is token-shaped and
a media call produces no cost record"). This lane did not improve on that and
does not claim to. Closing it needs either a Flux credential or a local
inference server on the measurement host — the second of which needs no
credential at all and is the cheaper path for a successor.

**Failures, shapes B/C/D.** No server failed in those runs. The host-visible
MCP failure path IS proved, by the negative control and by the late-MCP
identity refusal above — both produced `mcp_failed` with a named reason — but
it was not exercised *inside* each shape, so each shape's own cell reads NOT
MEASURED rather than borrowing the control's result.

## The negative control

Without it, every `discovery` PASS above could equally have been produced by a
driver that always reports PASS. A config server pointing at a command that
does not exist yields:

```
mcp_ready:  {}
mcp_failed: {"f27missing": "Transport error: MCP stdio server exited before responding"}
```

The observable reports absence, with a reason, and does not report the server
ready. Both directions of the discovery observable are therefore demonstrated.
