# Wayland Core F04 Deterministic Fixtures Receipt

**Status:** implementation and local E3 seal passed; authoritative CI and native-platform seal pending

**Implementation source:** `b681318914aef498414badb0997b9c94a0978e98` on `frontier/m0`

**Scope:** F04 only. This receipt proves deterministic fixtures and their local
packaged-Core execution boundary. It does not claim production activation of
the fixture sidecars or cross-platform release authority.

## 1. Delivered contract

F04 now provides:

- a scripted OpenAI-compatible streaming fixture with success, 429, 5xx,
  pre-header stall, truncation, duplicate delta, tool-call and cancellation
  behavior;
- a provider-neutral `read_timeout_ms` compatibility setting, rejected at zero,
  with real timeout recovery and terminal-exhaustion coverage;
- Core-emitted physical-attempt, retry-decision and typed-failure events, with
  runner aggregation that does not infer recovery from fixture request counts;
- deterministic MCP stdio, streamable HTTP, streamable HTTP/SSE and legacy SSE
  fixtures, including a packaged-Core streamable HTTP/SSE tool round trip;
- content-addressed seeded repositories and hidden filesystem outcome checks;
- bounded egress-recorder and attested fake-remote-executor fixture contracts;
- one six-component composite fixture identity covering provider, MCP, seeded
  repository, hidden outcome, egress and remote execution;
- root-independent semantic request and behavior digests, while preserving
  leaf-level diagnostics when two runs diverge; and
- receipts bound to the source SHA embedded in the tested binary, its binary
  digest, configuration digest and composite fixture digest.

The additive provider evidence wire events are documented in
`docs/json-stream-protocol.md`. Unknown hosts continue to drop them under the
existing forward-compatibility contract.

## 2. Verification evidence

Verification ran in `/root/wayland-frontier-m0` on `hetzner-dsm` with Rust
1.95.0. The lead run and an independent adversarial reviewer both exercised the
exact source above.

- packaged real agent loop: 13 passed, 0 failed in 54.9 seconds;
- full `wcore-eval-scenarios` corpus: 193 passed, 0 failed, with two documented
  doctest examples ignored;
- provider retry cohort: 37 passed, 0 failed;
- strict clippy across `wcore-providers`, `wcore-agent`,
  `wcore-eval-scenarios` and `wcore-cli`, all targets, with `-D warnings`:
  passed; and
- local `cargo fmt --all` plus `git diff --check`: passed.

The packaged cohort proves success, 429 and 503 recovery, real read-timeout
recovery, six-attempt timeout exhaustion, truncated-stream recovery, duplicate
delta preservation, approved and denied writes, active-stream cancellation,
MCP execution, a hidden repository outcome, and repeatability across two fresh
workspaces.

## 3. Retained local seal

The exact-source repeatability run retained two redacted receipts and a
machine-readable repeatability summary on the build host. Its stable identities
were:

| Evidence | SHA-256 |
|---|---|
| Packaged binary | `c6e0073d23c7f51a5028f9509617e6fb597b77b5a8bf669157a815fb08716447` |
| Composite fixture | `ded31eda4c4ae905d87133889bf8be8d6ab0163207b2f5972172430ce358d4b2` |
| Behavior | `be94ce8c09f0298244fd7abce925b22a339510e4dfdfb2d62c2a14ff05b81656` |
| OpenAI behavior | `1d197ca68a158897bd2432c05e875fd0242c1dab16a09b109cdfef2da7f1e4c5` |
| Final repository | `7b1c2c239a586933c0ec7be25b48c5b33007d89d1f81e866d5ff572f72f83006` |
| First receipt body | `361fd04709d2c73693f2d465cc6adabe5817ad84cf590627e543703536311c16` |

The receipt reports its build provenance as explicitly unavailable with code
`local_run`; it does not impersonate CI authority. Its provider evidence is
observed: four physical attempts, zero retries and complete token counts for the
successful four-turn seal scenario.

## 4. Independent audit verdict

The read-only F04 adversarial audit found no implementation blocker and returned
PASS for the local seal. It independently reran the 13 packaged tests and the
retained two-workspace seal, matched the receipt binary digest to the built
artifact, and traced provider evidence from physical HTTP sends through the
Core protocol into the receipt.

## 5. Honest boundary and deferred gates

This is a local Linux E3 result, not an E5 enterprise or cross-platform claim.

- Egress recording and fake remote execution are deterministic,
  content-addressed sidecar contracts. They are not evidence that production
  remote execution or production egress activation is wired.
- Every MCP transport is covered through the real `McpManager`; the packaged
  Core path presently proves streamable HTTP/SSE specifically.
- Provider retry policy has fake-time unit coverage. F15 remains responsible
  for injectable cooldown/pricing time and semantic failover behavior.
- The authoritative PR workflow across macOS, Windows and Linux remains
  pending by operator choice. F04 cannot receive its external/merge seal until
  that provenance-bound workflow passes.

These boundaries do not block F05 implementation. They do block any claim that
F04 already has cross-platform release authority.
