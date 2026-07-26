---
phase: 27-multimodal-browser-generation-voice
plan: "02"
subsystem: browser-cua-web-readiness
tags: [readiness, false-ready, capability-activation, cross-audit, seam-blocked]
status: complete
termination_state: 1-measured-3-blocked
requires: []
provides:
  - "A measured, single-variable proof that the capability handshake is linkage-derived, not readiness-derived"
  - "An adjudicated 4-0 publication decision with its dissent preserved"
  - "Seam requests for the fenced protocol files"
affects: []
tech-stack:
  added: []
  patterns: []
key-files:
  created:
    - .planning/phases/27-multimodal-browser-generation-voice/27-02-READINESS-AUDIT.md
    - .planning/phases/27-multimodal-browser-generation-voice/evidence/27-02/
    - .planning/SEAM-REQUESTS/27.md
  modified: []
decisions:
  - "chain-plus-derived-flags, 4-0, with two binding conditions"
metrics:
  completed: 2026-07-26
---

# Phase 27 Plan 02: Browser, CUA and Web Readiness Summary

Proved on real hardware that the shipped handshake claims two capabilities the
machine cannot deliver, took the publication decision four ways, and stopped at
the fenced protocol seam with an exact, paste-ready request rather than
creating a guaranteed integration conflict.

## Termination state

**Tasks 1 and 2 complete. Task 3 blocked on a fenced seam, not attempted.**

## The finding, measured

Host `hetzner-dsm`, SHA `2ecdfdf5`, shipped release binary. Machine facts
established first: no `chromium`, `chromium-browser`, `google-chrome` or
`camoufox` anywhere on PATH; `DISPLAY` and `WAYLAND_DISPLAY` both unset.

Five handshake captures, each changing exactly ONE variable:

| Observation | Variable | `browser_suite` | `computer_use` |
|---|---|---|---|
| baseline | — | **true** | **true** |
| no-browser-backend | PATH emptied | **true** | **true** |
| display-advertised | `DISPLAY=:99`, no X server | **true** | **true** |
| cloud-creds-absent | no credential | **true** | **true** |
| cloud-creds-present | fixture credential | **true** | **true** |

**Invariant under every absence.** That is the evidence the flag is not derived
from a probe. `crates/wcore-cli/tests/release_binary_smoke.rs` documents why:
the flags come from `PluginCapabilitySet::from_verified`, i.e. from linkage.

Then the operation the flag promised, on that same machine:

```
Browser {"op":{"kind":"navigate","url":"https://example.com/"}}
is_error = True
session: backend error: Camoufox is unavailable at http://localhost:9377/health
and Core could not start `camofox-browser`: spawn camoufox: No such file or
directory (os error 2).
```

The handshake said available; the very next operation could not deliver.

**The mechanism to fix it already exists.** The same captures show the
activation ladder publishing an honest `unavailable` with a reason for
`pricing_refresher`, `learned_policy`, `smart_handoff` and
`delegate_isolation`, and a full `declared → configured → constructed → ready`
chain for four more. It publishes **nothing at all** for browser, computer use
or web — `CapabilityId` has eight variants and none of them names these
surfaces. This is a wiring gap, not a missing mechanism.

## A new HIGH found on the way

With the browser tool disabled, the product prints its own remediation:

```
[browser]
allowed_origins = ["example.com", "*.mysite.com"]
```

**That instruction does not work.** It was followed verbatim and the tool
reported itself disabled again with the identical message. The key actually
read is `[browser.policy] allowed_origins`
(`crates/wcore-config/src/browser.rs:41-42`). An "unavailable" whose stated fix
is wrong is not an honest unavailable — it sends the user in a circle. **NOT
FIXED**; `wcore-browser/src/tool.rs:499` is outside this plan's declared files
and belongs with the Task 3 wiring that did not run.

## A guarantee that DOES hold

`http://127.0.0.1:1/` was refused before the backend was touched:
`policy: policy denied: loopback IP blocked: 127.0.0.1`. Recorded as a
baseline.

## The decision

Bundle `evidence/27-02/panel/PROMPT.md`, sha256
`5dda53566b934f2d2e751488e2fd3d19956814f2e17613e407867c59cf85b311`. All four
members received that exact bundle and each capture carries the digest; all
four captures are distinct.

| Member | Vote |
|---|---|
| codex gpt-5.6-sol | `chain-plus-derived-flags` |
| gemini 3.1-pro-preview | `chain-plus-derived-flags` |
| kimi K3 | `chain-plus-derived-flags` |
| internal adversarial | `chain-plus-derived-flags` |

**CHOSEN: `chain-plus-derived-flags`, 4-0 majority.**

The internal pass was written specifically to break the emerging 3-0 consensus
and argued `chain-plus-new-flags` at full strength. It conceded on a point none
of the other three made: its own measured witness, `release_binary_smoke.rs`
going red, turns out to be evidence FOR the change, because that test asserts a
statement measured to be false about the machine it runs on and passes only
because the flag does not mean what its name says. Its fifth argument was NOT
withdrawn and is carried as an open risk.

**Accepted cost:** redefining the meaning of a field already on the wire is a
compatibility event and must be carried as one in the contract bump.

**Two binding conditions:**
1. The LTO guard in `release_binary_smoke.rs` must be RETARGETED onto the new
   additive linkage field, never deleted — it exists to catch the
   release-profile dead-code-strip regression of v0.2.0 BLOCKER #1.
2. The Desktop consumer's actual reading of these flags is **UNVERIFIED from
   this repository** and must be confirmed before the bump is published.

Dissent preserved in full at `evidence/27-02/panel/DISSENT.md`.

## Why Task 3 did not run

Its first required edits are `crates/wcore-protocol/src/contract/generate.rs`
and `crates/wcore-protocol/contracts/desktop/v1/manifest.json`, both **FENCED**
for this execution as the only files overlapping concurrently-running phases.
Editing either guarantees an integration conflict; the manifest is byte-exact,
so a regeneration from a divergent tree conflicts deterministically.

`.planning/SEAM-REQUESTS/27.md` carries SR-27-1 through SR-27-4 with exact
files, exact insertion points, exact lines, and what breaks if each is omitted.
**No fenced file was edited, not even locally.**

## Not run, named plainly

- **Windows handshake over a non-interactive session.** `seandesktop` was
  verified reachable at the start of this work and was not used. This is the
  single most valuable observation in the plan — it is exactly where a
  display-dependent capability is most likely to be claimed falsely — and it is
  absent.
- **macOS handshake.** No artifact exists for this unpushed SHA.
- **Three of four policy baselines.** Only origin admission was measured. The
  downloads-root confinement, the approval gate, and the process count
  before/during/after a session plus one reaper interval have **no baseline**.
- **The browser corpus** `fixtures/f27/browser/` was not built.
- **Discriminator experiments E1, E2, E3.** The panel received the OBS-01..09
  hardware captures instead. Those are real measurements at the pinned SHA
  bearing on the same claims, but they are not the three experiments the plan
  named; in particular the Windows half was not measured at all, and the
  compatibility cost of the chosen option was reasoned from the test's source
  rather than measured by running it on a backend-less box.

## Requirements

**F27-02 is NOT marked complete.** The measurement is done and the decision is
taken, but nothing is published: browser, CUA and web readiness is exactly as
dishonest at HEAD as it was before this plan ran.

Commit: `47a5dd09`.
