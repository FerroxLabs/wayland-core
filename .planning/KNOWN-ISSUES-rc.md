# Known issues — v0.12.26-rc.1

DRAFT. Numbers are measured, not estimated. Sections marked **[PENDING WAVE]** are
awaiting the in-flight repair wave and must be re-checked before this ships.

> **Markers re-checked 2026-08-02 against the tree at `b8d51309`** by
> `lane/doc-truth-residual`. Every `[PENDING WAVE]` below was verified individually
> against the code and the merge that claims to close it; the ones that were closed
> now cite the merge SHA instead, and the ones that were not are left listed. One
> entry — a journal path-identity guard described as a security defect — was
> **removed**, because the finding itself was retracted, not fixed
> (`HANDOFF-2026-08-02.md` §7). **The test-count table below was NOT re-measured**;
> it still carries its stated `2280e205` / `02575b6f` measurement, and several
> counts in it are now stale in the *user's favour*. Where a closure could not be
> verified, the issue stays listed: over-claiming closure is the failure this
> document exists to prevent.

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
three voice-capture tests need a microphone the hosted runner does not have.

**Partly closed.** The eight `wcore-swarm` tests now skip loudly with a ledger CI
reads back (`9ddf929a`); the skip is armed three ways on Linux so it cannot spread
silently. **The three voice-capture tests are unchanged** — nothing in the wave
touched them, and they still fail rather than skip. **[PENDING WAVE — voice only]**

---

## Defects a user could actually hit

### Cross-platform, confirmed on both macOS and Windows

> **Removed 2026-08-02: "Journal path-identity guard does not hold off Linux."**
> This entry described the guard that refuses a symlinked, hard-linked or swapped
> session-journal path as failing on macOS and Windows, and called it *"a security
> guard, not a convenience check"*. **The finding was retracted, not fixed.** The
> guards were always correct and still bite under mutation; the *fixtures* compared
> the journal's reported path — which `lease::reported_path` canonicalizes — against
> a raw `tempfile::tempdir()` path, and on macOS `$TMPDIR` reaches the test through
> the `/var` → `/private/var` symlink, so the fixture and the refusal named the same
> file under two spellings. Reproduced on Linux by pointing `$TMPDIR` through a
> symlink: the refusals are exactly the ones asserted (`SymbolicLink`,
> `MultipleLinks`, `PathIdentityMismatch`), only spelled resolved. Fixture derived
> from `canonical_journal_root` in `c93e6a43`; the path equality was deliberately
> kept, because dropping it would retire the guard instead of fixing the fixture.
> A fixture defect, not a security defect — publishing it as one is a security scare
> that is not real. See `HANDOFF-2026-08-02.md` §7.

**User-context block missing from the outbound request** on macOS and Windows
(`user_model_correction_wire`, `user_model_identity_wire`). Passed on Linux.
**FIXED — `0e105082`, `90cd6298`, `b9b661fc`.** Same alias seam, three spellings:
the harnesses seeded through the raw `TempDir` name while `AgentBootstrap::new`
keys user-model paths off the resolved one, so the session opened a different
bucket (macOS `/var` → `/private/var`; Windows `\\?\C:\…` → `C:\…` via
`dunce::simplified`; and the guard's own needle needed JSON escaping to survive
`D:\…`). The product path was never wrong.

**Deterministic replay diverges** off Linux
(`deterministic_openai_loop::packaged_f04_run_is_repeatable_and_content_addressed`).
**FIXED — `7cdb5ca6`**, and this one was a real product defect rather than a test
artefact: `workspace_forms` normalized only the caller's spelling, so the random
per-run tempdir name stayed inside the leaf digest and the repeatability gate
diverged on **every** run. It could not pass.

### Windows-specific

- ~~Grep can report "no matches" for an unreadable target instead of an error, when
  ripgrep is not on PATH.~~ **FIXED — `bf5457f9`.** Measured on Windows 10.0.26200,
  `findstr` returns exit 1 with empty stdout for *both* a clean no-match and
  "Cannot open <path>", so the earlier exit-code guard was a no-op there. Two
  further defects found and closed at the same site: a **single file** returned
  nothing (the `<dir>\*` lowering was applied to files, yielding `file.txt\*`), and
  a path that trimmed to empty produced `\*` — which with `/S` **walked the whole
  drive** (still running at 25s against 157ms scoped; a verifier reproduced this and
  rendered a workstation unusable).
- ~~`--json-stream` may not emit the `ready` frame within the deadline, which breaks
  the Desktop app on first launch.~~ **FIXED for the known cause — `42382f24`,
  `8c16b009`.** `required_for_session` resolved the Windows backend by running a
  real guarded `cmd.exe /c exit 0` through the whole AppContainer pipeline, bounded
  by a 15s wall-clock guard, and bootstrap resolved that runtime *before* the `ready`
  frame — so a first launch that hit an AV image scan or a slow profile-service RPC
  emitted no `ready` frame at all. Selection and probing are now separate questions
  and the probe moved to `spawn_blocking`. A second defect was found alongside it:
  `NEGATIVE_PROBE_TTL` erased the *answer* as well as scheduling the next probe, so
  containment predicates alternated between two verdicts on a host that never
  changed. **Caveat:** proven by cross-platform unit tests plus a Windows test that
  drives the production selector and reads the production probe cache — **not** by a
  live `ready`-frame timing measurement on Windows hardware. If your Desktop first
  launch still stalls, report it.
- ~~Log rotation, worktree reclamation, and descendant reaping each have failing
  cases.~~ **Root causes FIXED — `ecde6bb7` (rotation, worktree), `d19678ce`
  (reaping grade).** Two were user-visible and worse than "a failing test": Windows
  rotation truncated the live log through its own append handle, which
  `FILE_APPEND_DATA` forbids, so **every record past 5 MiB failed to be written at
  all** on the headless gateway; and `.swarm-worktrees/` could never be emptied, so
  the aggregate budget stayed committed until dispatch was refused. The reaping item
  was an *instrument* fault — containment always worked; a descendant inheriting the
  stdout pipe handle meant EOF never arrived, and clean exits, assertion failures and
  early crashes all came back as `Hung`. **The Windows failure count has not been
  re-measured at this SHA** — treat the table above as the last real measurement.
- **`RealFs::observe_file` is unimplemented off Unix**, and this is larger than one
  function. It returns `Unsupported` on non-Unix targets
  (`crates/wcore-tools/src/vfs.rs:406`), and durable receipts use it precisely so
  that matching bytes alone can never resolve an uncertain effect — so **the durable
  filesystem-receipt, crash-reconciliation and rollback path is inert on Windows**.
  A candidate fix exists and was **deliberately held**: it is `cfg(windows)`-gated
  throughout (Linux has no power to prove any of it), its mutant does not cover two
  of seven changes, and it adds ~200 lines of `NtCreateFile` FFI with no tests on any
  platform for identity-token stability, parent-token binding, reparse-point refusal,
  or `Conflict`-vs-`NotStarted` reconciliation. Holding it unproven is the correct
  outcome; the defect remains **open**.

### All platforms

- **The credential ladder is unproven against a real OS keyring.** It landed
  2026-08-01 with test coverage but no live keyring exercise on any platform.
- **OAuth tokens are stored in plaintext**, despite documentation describing them
  as encrypted. On Windows the permission hardening is a no-op. **[PENDING WAVE —
  still open at `b8d51309`.]** A repair lane exists and was **rejected**, not merged:
  it missed a third consumer that would render a signed-in user "Not configured", and
  it carried a one-way migration cliff that signs the user out with the cleartext file
  already consumed. Do not read the marker as "fixed shortly".
- ~~`plugins.toml` is never loaded from disk, though docs and error strings instruct
  you to edit it.~~ **FIXED — `7075e6f8`.** `bootstrap.rs` constructed
  `PluginsConfig::default()` and never opened the file, so `enabled = false` did not
  disable a plugin and `plugin_signature_verification` / `trusted_plugin_keys` bound
  to nothing. Now loaded from beside `config.toml`; an absent file yields defaults, a
  malformed one is fatal and names the path. Proven live on Linux against the real
  binary with a `plugins.toml` disabling `wayland-ollama`.
- ~~`[default] read_only = true` is accepted and enforced nowhere.~~
  **FIXED — `4df3ca20`, plus `e81509a0`.** The flag parsed and round-tripped but was
  dropped at *resolution*, so nothing downstream could see it. The resolved `Config`
  now carries the posture and the dispatcher default-denies any tool that does not
  declare `read_only_safe` for its concrete input — `Read`, `Grep` and `Glob` are the
  only built-ins that claim it. `e81509a0` closes a second reader: the headless cron
  daemon took the posture off `Config::default()`, a constant, and would have run
  skill shell unattended in a session the operator had made read-only; a config it
  cannot read is now a refusal, not `false`.
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
declarations corrected. **The documentation is FIXED — `add8a3d1`.** `README.md` and
`docs/channels.md` no longer claim Slack/Discord exactly-once, the adapter count is
corrected from seven to nine, and Matrix's guarantee now travels with its
precondition. `scripts/verify-doc-truth.sh` binds each corrected sentence to the
code fact behind it — `supports_outbound_idempotency() == false` for both adapters,
`max_message_len() == 32_768` for Matrix — so the doc and the code cannot drift apart
silently. Verified green at this SHA.

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
