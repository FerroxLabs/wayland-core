# Known issues — v0.12.26-rc.1

DRAFT. Numbers are measured, not estimated. Sections marked **[PENDING WAVE]** are
awaiting the in-flight repair wave and must be re-checked before this ships.

This document exists because a release candidate that hides what it does not know
is worse than no release candidate. Everything below is something we found, not
something a user reported.

---

## Test status, measured on all three platforms

Measured at `2280e205` / `02575b6f`, GitHub Actions run 30698794888 / 30699019736.

| Platform | Tests run | Passed | Failed | Skipped |
|---|---:|---:|---:|---:|
| Linux (containerized) | 13,775 | 13,774 | 1 | 74 |
| macOS (hosted runner) | 13,718 | 13,700 | 18 | 105 |
| Windows (self-hosted) | 13,379 | 13,349 | 30 | 130 |

Windows worst case **99.78%**. Two consecutive Windows runs measured 30 and 33
failures with 6 flaky, so read the hard-failure count as ~30 with a few points of
noise.

**Of the macOS 18: eleven cannot pass on that runner by construction** — eight
`wcore-swarm` tests need a Docker backend on a leg where the feature is off, and
three voice-capture tests need a microphone the hosted runner does not have. They
are being made to skip loudly rather than fail silently. **[PENDING WAVE]**

---

## Defects a user could actually hit

### Cross-platform, confirmed on both macOS and Windows

**Journal path-identity guard does not hold off Linux.** The code that refuses a
symlinked, hard-linked, or swapped session-journal path passes on Linux and fails
on macOS and Windows (`session_journal::fault_tests`, three cases on each). This is
a security guard, not a convenience check. **[PENDING WAVE]**

**User-context block missing from the outbound request** on macOS and Windows
(`user_model_correction_wire`, `user_model_identity_wire`). Passes on Linux.
**[PENDING WAVE]**

**Deterministic replay diverges** off Linux
(`deterministic_openai_loop::packaged_f04_run_is_repeatable_and_content_addressed`).
**[PENDING WAVE]**

### Windows-specific

- Grep can report "no matches" for an unreadable target instead of an error, when
  ripgrep is not on PATH. The agent may then conclude a symbol does not exist.
  **[PENDING WAVE]** — highest-priority item in the wave.
- `--json-stream` may not emit the `ready` frame within the deadline, which breaks
  the Desktop app on first launch. **[PENDING WAVE]**
- Log rotation, worktree reclamation, and descendant reaping each have failing
  cases (~20 Windows-only failures total).
- `RealFs::observe_file` is unimplemented off Unix.

### All platforms

- **The credential ladder is unproven against a real OS keyring.** It landed
  2026-08-01 with test coverage but no live keyring exercise on any platform.
- **OAuth tokens are stored in plaintext**, despite documentation describing them
  as encrypted. On Windows the permission hardening is a no-op. **[PENDING WAVE]**
- `plugins.toml` is never loaded from disk, though docs and error strings instruct
  you to edit it. **[PENDING WAVE]**
- `[default] read_only = true` is accepted and enforced nowhere. **[PENDING WAVE]**
- Ollama (local inference) cannot make tool calls — chat only.
- `[browser.stealth]` keys are parsed and discarded; `allow_cloud_fallback = false`
  does not prevent a Browserbase fallback.
- Voice ships enabled by default with an open provider-side 402.

---

## Delivery guarantees — read this before relying on a channel

Three of ten channel adapters have ever been driven against the real platform.

| Channel | Status |
|---|---|
| Slack | Live-driven. **At-most-once**, not exactly-once |
| Discord | Live-driven. **At-most-once**, not exactly-once |
| Matrix | Live-driven. Exactly-once **only below `max_message_len`** |
| Telegram, WhatsApp, SMS, email, Signal, iMessage, MS Teams | Implemented, **never measured against the real platform** |

Slack and Discord previously claimed exactly-once on the strength of a mock. Live
kill-and-restart produced two messages for both. The claim was withdrawn and the
declarations corrected. Documentation still carrying the old claim is being fixed.
**[PENDING WAVE]**

---

## Certification status — the disclosure that matters

The native security, reliability, recovery and resource certification (Phase 28)
passes, and it was measured against candidate `32e2f57d`. **The tree this release
is cut from is roughly 2,100 commits past that candidate.**

In the words of the project's own drift record:

> *"It passes and it is stale. Both are true and neither cancels the other. A
> release cut today would carry a passing certification that never saw the shipped
> binary."*

**This is why this is a release candidate and not a general release.** Do not read
the certification as covering these binaries. Re-certification against the actual
candidate is the gate for GA.

---

## What is genuinely proven

Not everything here is a caveat. The following were driven against the real product
on real hardware, with controls proven able to fail in both directions:

- `index` build/status/search/verify — **all three platforms**
- `goal` durable objectives — survive `kill -9` mid-wave on Linux and Windows,
  effects 12/12/12
- `backup` create/verify/restore/recover — including a real Windows long-path fix
- `sandbox` status/exec — 50 greens, each with a differential activeness observation
- `plugin` 10-verb lifecycle — Linux and Windows
- `migrate` — against real Hermes and OpenClaw installations
- glibc floor lowered 2.39 → 2.34, live on five distribution families

And security work closed with live proof in this cycle: a root RCE in the ssh
execution backend (determined never to have shipped), an egress hole that let any
cloned repository disable the network boundary, a cross-user prompt leak proven on
the wire, and a Windows AppContainer deny rule that was silently inert on hardware.

---

## Reporting

Security issues: private report via the GitHub Security tab, per `SECURITY.md`.
Everything else: GitHub issues. Please include your platform — the table above
should tell you why.
