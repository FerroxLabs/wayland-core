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

**Re-derived 2026-08-03. Every row now carries its own provenance**, because the
previous version of this table did not and could not: its three rows were sourced
from a `report` job that merged artifacts with `merge-multiple: true` and silently
kept ONE — proven on run 30699019736, where three artifacts were uploaded and the
report counted `junit report count : 1`. A table whose rows come from an unnamed
runner is not a measurement.

| Platform | Runner | Run / job | Commit | Executed | Failed |
|---|---|---|---:|---:|---:|
| macOS 15 | GitHub-hosted `macos-latest` | 30800234966 / 91642878745 | `c615daed` | 13,844 | **3** |
| Windows | GitHub-**hosted** `windows-latest` | 30800234966 / 91642878530 | `c615daed` | 13,507 | **14** |
| Linux | hetzner `orch-gate` (not CI) | local gate | `3750c916` | 13,904 | **0** |

Read the columns literally. **"Executed" is not "total".** `cargo-nextest`'s JUnit
records only tests that ran — it emits no `skipped` elements at all — so a Skipped
column cannot be derived from the same artifact as the failure counts, and the
previous table's Skipped column therefore came from somewhere else. It has been
removed rather than guessed at.

**The Linux row is not from CI.** `CI (linux-containerized)` never reached its test
step on this run — it failed at a repository lint (`No vacuous cargo test
invocations`) four steps in. The number quoted is a real full-suite run on our own
Linux hardware at the lockfile-bump commit, and it is labelled as such rather than
being dressed up as a CI result.

**There is no self-hosted Windows row, and that is the finding.** `CI (Array)` — the
self-hosted Windows leg, and a *required* check on `main` — has served **zero jobs
across the last 60 CI runs**; every instance ever scheduled is still `queued` or
died at the 24-hour runner-wait timeout. Both registered Windows runners report
`online` **and** `busy` while executing nothing. The hosted `windows-latest` leg
above exists precisely because of that single point of failure, and this is the
first complete hosted-Windows verdict this branch has ever produced.

**macOS: 18 → 3.** Of the previously-reported 18, the eight `wcore-swarm` tests now
skip loudly against a ledger CI reads back (`9ddf929a`). The remaining **3 are the
live acoustic arms**, and they are now `#[ignore]`d with reasons: they require a real
speaker→microphone path that no hosted runner has, so they were failing on the
missing hardware rather than on the product — which made the macOS leg permanently
red and every future macOS regression invisible behind it. `goertzel_instrument_self_test`
stays un-ignored so the instrument itself is still proven every run, and a CI step
names the absence and re-derives the count of 3 from source so an arm cannot be
deleted or silently un-ignored. **[PENDING WAVE — voice only] is closed.**

**Windows: 14, and most are not product defects.** Triaged individually: three were
tests asserting hardcoded Unix paths against an absoluteness check; two were `sh`
syntax handed to `cmd.exe` (`;` is not a separator there, so `exit 1` never ran —
the capture path is fine, and the control test using `exit 3`, valid in both
dialects, passed on the same run); one needs a Docker daemon that runs Linux images.
Fixes for those are in this branch. What remains genuinely open on Windows is
recorded in the defect sections below — including one swarm-dispatch path that
reports a cause it cannot have established, and the sandbox differential that has
never run on Windows at all.

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
**PARTLY FIXED — `7cdb5ca6`**, and this one was a real product defect rather than a
test artefact: `workspace_forms` normalized only the caller's spelling, so the random
per-run tempdir name stayed inside the leaf digest and the repeatability gate
diverged on **every** run. It could not pass.

**Correction, 2026-08-03 — this entry said "FIXED" and the test is still red on
Windows.** The *digest* half is genuinely fixed; the three `read.input.contains(...)`
assertions were not given the same treatment. On a hosted Windows runner the raw
`TempDir` spelling is the 8.3 short form (`C:\Users\RUNNER~1\...`) while
`AgentBootstrap::new` canonicalizes the session workspace through `dunce::simplified`,
so the trace carries the long form and those three asserts fail. Measured on
run 30800234966, job 91642878530. No product defect remains here — but claiming a
named test FIXED while it fails is exactly the thing this file exists to prevent
(see the opening section), so it is retracted rather than quietly amended.

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

  **Correction, 2026-08-03 — read the previous sentence carefully, because it
  implied something false.** Saying the path is inert *on Windows* invites the
  reading that Linux and macOS users have the guarantee. They do not. **The path
  is dormant on every platform**, for a reason that has nothing to do with
  `observe_file`: no production tool implements `prepare_effect` at all. The only
  three definitions in the tree are the trait default
  (`crates/wcore-tools/src/lib.rs:394`), the `Box`/`Arc` forwarder (`:616`) and one
  in a test file — and `Write` and `Edit` both return `ToolEffectContract::default()`
  (`write.rs`, `edit.rs:419`) rather than declaring `FilesystemTransactional`. The
  dispatcher retains the type for a future opt-in backend, which the code says
  outright. So the honest statement is: **no user on any platform is currently
  getting durable filesystem-receipt crash recovery.** Where the path *is* reached
  it fails closed and says so — an unresolved effect stays `Unknown{Interrupted}`
  and rollback returns `suspended(...)` with a reason — so nothing lies and nothing
  is lost; the capability is simply not yet wired.
  A candidate fix exists and was **deliberately held**: it is `cfg(windows)`-gated
  throughout (Linux has no power to prove any of it), its mutant does not cover two
  of seven changes, and it adds ~200 lines of `NtCreateFile` FFI with no tests on any
  platform for identity-token stability, parent-token binding, reparse-point refusal,
  or `Conflict`-vs-`NotStarted` reconciliation. Holding it unproven is the correct
  outcome; the defect remains **open**.

### All platforms

- **The credential ladder is unproven against a real OS keyring.** It landed
  2026-08-01 with test coverage but no live keyring exercise on any platform.
- ~~OAuth tokens are stored in plaintext, despite documentation describing them
  as encrypted.~~ **FIXED — verified in the tree, not inferred from a branch name.**
  `OAuthStorage::store_serialized` (`crates/wcore-agent/src/oauth/storage.rs:131`)
  runs **write-secure → verify-readback → remove-cleartext**, in that order: it
  puts through the credential ladder, fails closed with
  `NoSecureBackend` if neither keyring nor vault is available (it does **not**
  fall back to a file), reads the value back from the store rather than trusting
  `put` returning `Ok` — a keyring that silently truncates an oversized blob
  returns `Ok` and hands back something else — and only then deletes the legacy
  cleartext copy. `docs/advanced.md` ("there is no cleartext rung") is accurate.

  **This entry previously said the opposite, and it was wrong.** The rejected
  repair lane it described was superseded by one that landed. A known-issues
  file that invents a credential-security failure is the same defect class as
  the product hiding one — it is disclosure that cannot be trusted either way.
  Verified by reading the merged source, because the branch-ancestry check that
  first raised the alarm is the wrong instrument: work can land under a
  different branch than the one a plan named.

  Residual, genuinely open: migration off the legacy file is **one-way**. Run
  once with the vault unlocked and the token moves and the cleartext copy is
  deleted; run again without the passphrase and neither tier can return it. The
  code refuses rather than presenting a signed-in user as signed-out, but keep
  your vault passphrase available the first time you run v0.12.26.
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
- ~~`[memory] enabled = false` does not stop memory from recording.~~
  **FIXED — `35510975`, plus `7b6ffd0a`.** The advertised privacy opt-out was
  ORed with `[observability] skills_lifecycle`, which **defaults on**, so the
  combination a privacy-conscious user actually produces — switch memory off,
  leave everything else alone — still opened a real store on disk, registered
  the `record_episode` / `assert_fact` write tools, drafted skills into
  `$WAYLAND_HOME/skills/`, and ran auto-memorize at every session end.
  `fire_auto_memorize` consulted no config field at all. The opt-out now
  dominates, and `35510975` alone was not enough: `7b6ffd0a` closes a host
  bypass a four-way cross audit found unanimously — `set_memory_api()` is
  public API and reinstated every durable write (session-end consolidation, KG
  transcript ingest, and the verbatim pre-compaction transcript written by
  smart handoff). Both directions are tested at three layers, and four
  mutations of the fix each turn a specific test red.
  **If you set `enabled = false` before v0.12.26, content was recorded anyway
  — check `~/.wayland` and your project's memory directory.**
- **Two Wayland processes signed into the same ChatGPT account can collide when
  the token refreshes.** The refresh POST is single-flighted only *within* a
  process, and ChatGPT refresh tokens rotate and are single-use, so if two
  processes reach expiry together both POST and one gets `invalid_grant`. It is
  recoverable — sign in again — and it needs two concurrent processes on one
  profile to happen at all. A fix is designed and cross-audited but deliberately
  **not** in this candidate: getting the failure policy wrong risks revoking the
  whole grant rather than failing one call, and that is not a trade to make
  under release pressure. Run one Wayland process per account and you will not
  see it.
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
- `sandbox` status/exec — 50 greens, each with a differential activeness
  observation — **Linux and macOS only. The differential has NEVER run on
  Windows.** This row shipped with no platform scope while every row around it
  carried one; corrected 2026-08-03. `sandbox_activeness.rs` drives its
  uncontained baseline with `touch <path> 2>/dev/null; echo RAN`, and on Windows
  that string is handed to `cmd /C`, which has no `touch`, no `/dev/null`, and
  does not treat `;` as a separator — so the baseline cannot execute and the
  test fails with "the uncontained baseline did not run". It is `cfg!(windows)`,
  so this has never passed on any Windows host. The test behaved correctly: it
  refused to grade containment on a control that did not run. `sandbox status`
  *not* reporting a bypass is not a substitute — that only reads what `status`
  says about itself.
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
