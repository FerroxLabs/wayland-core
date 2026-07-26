# 22-01 — The cross-binary determination: can the accepted F12 session journal carry Goal, Task and Wait records?

Commit under test: `2ecdfdf54ff7fda920eec7d068337006e5da4ee4`
Linux host: `hetzner-dsm`, phase-dedicated tree `/root/wayland-p22`, target `/root/p22-target`
Windows host: `SeanD@seandesktop`, phase-dedicated worktree `C:\p22` (detached at the same commit, verified by `git rev-parse HEAD`)
Date: 2026-07-26

---

## VERDICT

**`F22-06-VERDICT: COMPATIBLE-AT-V5`**, on the Linux evidence below, with the
Windows leg's status recorded honestly in §5 rather than assumed from Linux.

---

## 1. The live product exercise (Linux)

The journal was produced by the shipped release binary — not by a test helper —
built at the recorded commit and invoked headlessly with a trailing positional
prompt against a throwaway profile.

```
export WAYLAND_HOME=/root/p22-home              # a copy of the provisioned profile with
                                                # sessions/, logs/, cache/ emptied
export WAYLAND_VAULT_PASSPHRASE=<throwaway>     # an isolated profile refuses durable
                                                # session authority without a vault
/root/p22-target/release/wayland-core \
  "Use your tools to list the files in the current directory, then reply with the single word DONE."
```

Observed:

| Signal | Observation |
|---|---|
| exit code | **1** — see the RED below |
| journal file | `/root/p22-home/sessions/9aa64ad04744.journal`, **84,327 bytes** |
| first four bytes | `WJ01` — the product wrote it |
| companions | `.journal.snapshot` (1,026 B) and `.journal.authority` (301 B) also written |
| distinct durable event types | **9**: `session_imported`, `turn_started`, `turn_failed`, `conversation_message_committed`, `provider_attempt_prepared_v2`, `provider_attempt_started`, `provider_attempt_finished_v2`, `budget_authority_committed` (×5), `checkpoint_committed` |
| reduced state | 235 lines of canonical JSON |

### RED, reported rather than engineered around

**The run did not reach a tool call, and therefore the corpus contains no
`tool_execution_*` frame.** The provisioned Anthropic credential on `hetzner-dsm`
returns:

```
error: Provider error: API error 401: {"type":"error","error":{"type":"authentication_error","message":"API key is invalid."},"request_id":null}
```

`product-stdout.txt` is consequently **zero bytes**; the product's output went to
stderr as a provider error. Two of this task's own gate clauses therefore do NOT
pass: the non-empty-stdout clause, and the plan's stated intent that the prompt
force at least one tool frame.

Supplying a working credential is reserved to Sean and was not attempted. Three
things were tried and are recorded so nobody repeats them: copying `auth.json`
alone into the isolated profile (the profile refused — "no API key found"); the
full profile copy (got further, then refused durable session authority for want
of a vault); and the full profile copy plus a throwaway vault passphrase (reached
the provider and got the 401 above). The pool entry is `auth_type: api_key`,
`source: env:ANTHROPIC_API_KEY`, sourced from `/root/.wayland/.env`; a
`chatgpt.json` OAuth blob also exists but the CLI's `--provider` surface accepts
only `anthropic` or `openai`.

**What this does and does not cost the determination.** The compatibility claim
is about three mechanisms — serde decode of an internally-tagged enum, the
exhaustive reducer, and the version-gated boundary check — and those are
variant-agnostic. It is corroboration that is missing, not a load-bearing leg.
That said, the tool region (`tools: BTreeMap<String, ToolState>` with effect
states, hook phases and resolution sources) is the densest part of the reduced
state, and the honest statement is that this corpus does not touch it. It is
carried into the SUMMARY as a named limitation, not closed.

---

## 2. Single-variable measurements (Linux)

Each moved ONE thing. Two different binaries touched one file: the pre-change
reduce instrument was saved aside before the enum grew.

**M0 — compile-time.** Adding a variant to `SessionEvent` does not compile until
an explicit arm exists in `apply_event`
(`crates/wcore-agent/src/session_journal/reducer.rs:1656`):

```
error[E0004]: non-exhaustive patterns: `&model::SessionEvent::P22Probe { .. }` not covered
```

The match has no wildcard. A new durable record therefore cannot enter the enum
without a deliberate reduction decision. This is a property worth having and it
was not previously written down anywhere.

**M1 — the enum grew; nothing was written with it.** The same 84,327-byte file,
reduced by both binaries, serialized to canonical JSON:

```
pre-change  SHA256 ff13b6eca8314114b5d96ced5c5142c512a0295113499f5824a25a34002faf56
post-change SHA256 ff13b6eca8314114b5d96ced5c5142c512a0295113499f5824a25a34002faf56
M1_RESULT=IDENTICAL
```

**M2 — the post-change binary writes one probe frame, then re-reads the whole
file.** Append succeeded at `seq=13`. Re-reduction succeeded. The ONLY diff
against the same binary's pre-probe reduction:

```
< "last_checksum": "bc0baa9f...",  "last_seq": 12,
> "last_checksum": "f35600d5...",  "last_seq": 13,
```

With the chain head removed, `M2_PREFIX_RESULT=IDENTICAL`. The chain still
validates and the v5 boundary admits the frame.

**M3 — the probe-bearing journal taken BACK to the pre-change binary.** This is
the observation that could have produced a CRITICAL finding, and did not:

```
M3_EXIT=3
M3_STDOUT_LINES=0
REDUCE-FAILED: session journal ... has a corrupt complete frame 15:
  unknown variant `p22_probe`, expected one of `session_imported`, ... `delivery_finished`
```

Fails closed. Zero bytes of reduced state. No silent truncation.

**M4 — snapshot authority binding across the change.** A `.snapshot` +
`.journal.authority` pair written by the PRE-change binary, read by the
POST-change binary: exit 0, empty stderr, and the reduction is the byte-identical
one from M1. The binding's version pairing admits the change.

**M5 — writer authority lease across a restart (Linux).** Two sequential opens by
two separate processes both succeeded (`seq=2`, then `seq=3`), i.e. the lease is
released on drop rather than leaked.

**What would have refuted the verdict, and did not happen:** two different SHA256s
at M1; anything but the chain head changing at M2; M3 exiting 0 with a partial
reduction; M4 rejecting a pre-change snapshot.

---

## 3. The decision (Task 2)

**Question.** How do Goal, Task and Wait durable records enter the accepted F12
session journal?

**Three options**, put verbatim and in rotated order to all four panel members as
ONE shared bundle (`22-01-EVIDENCE/decision/panel-prompt.txt`, 12.7 KB, carrying
the question, the three options, the verdict with its supporting and refuting
evidence, the source boundary with resolvable citations, and both measurements):
`explicit-versioned-migration`, `escalate`, `additive-at-current-version`.

**Two measurements, both taken rather than asserted.**

- `source-boundary.txt` — 14 `SOURCE-CITE:` lines re-taken from the tree at
  execution time, including `LEGACY-EVENT-TYPES-COUNT: 49` (the planning-time
  reading of 49 was re-measured and agreed).
- `migration-surface.txt` — all four claimed pieces of migration machinery
  measured **present**; 4 files / 32 occurrences declare the legacy versions; and
  a live scratch-checkout probe on `hetzner-dsm` bumped the schema constant alone
  and printed
  `MIGRATION-COST-PROBE: failing_tests=3 total_tests=76 commit=2ecdfdf5...`.
  All three reds are version-boundary constant assertions. The expensive option
  is cheaper than its own `cons` paragraph implies; cost does not disqualify it.

**Panel result — 4 of 4.**

| Member | Position | Verdict sound? |
|---|---|---|
| codex (gpt-5.6-sol) | `additive-at-current-version` | yes |
| gemini (3.1-pro-preview) | `additive-at-current-version` | yes |
| kimi (K3) | `additive-at-current-version` | yes |
| internal adversarial | `additive-at-current-version` | yes (qualified) |

**Verdict challenge: UPHELD, `unsound_votes=0`.** Two of the four qualified their
yes and those qualifications are preserved in `panel-dissent.txt` rather than
flattened: the evidence supports a SCHEMA verdict, M5 is Linux-only, and Task 3
must not close F22-06 on Linux-only evidence.

**COMMITTED: `additive-at-current-version`, basis `majority`.**

**The cost accepted, in the option's own terms.** An older binary reading a newer
journal fails closed rather than degrading, so a user who downgrades gets a hard
refusal — accepted, and documented here rather than discovered. And the version
number stops distinguishing "has Goal records" from "does not", so anything that
needs to know must ask the content.

**Dissent preserved** in `22-01-EVIDENCE/decision/panel-dissent.txt`, naming all
three options. Two MEDIUM findings came out of it and are routed to BACKLOG via
the seam request, non-blocking:

1. A version-skewed journal is reported to the user as **corrupt**. It is not
   corrupt, it is newer. The message should distinguish an unknown event variant
   from frame corruption. (`self-update` makes downgrade a one-command operation.)
2. `SESSION_JOURNAL_SCHEMA_VERSION` will no longer imply a content model once Goal
   records land additively; whatever needs that distinction must be given an
   explicit content-level marker rather than inferring it from the header.

---

## 4. What the determination binds

Goal, Task and Wait durable records enter as additive `SessionEvent` variants at
`SESSION_JOURNAL_SCHEMA_VERSION = 5`, with **no** version bump, **no** second
store, **no** sidecar file, **no** second reducer and **no** second cursor. New
state added to `ReducedSessionState` must be `#[serde(default,
skip_serializing_if = ...)]` so that an empty map serializes to exactly the bytes
it serializes to today — otherwise M1's byte-identity property stops holding the
moment the records land, and the whole determination becomes historical.

---

## 5. Windows — status, stated plainly

`C:\p22` was created as a phase-dedicated detached worktree at exactly
`2ecdfdf54ff7fda920eec7d068337006e5da4ee4` (verified). The release build was run
detached under the task scheduler as SYSTEM (Windows OpenSSH kills session
children on disconnect; two earlier `/ru SeanD` attempts returned Last Result 1
with no log at all, which is recorded because it costs an hour to rediscover).

`wayland-core.exe` (87,730,176 bytes) built with `CLI_EXIT=0`. At the time this
record was written the second build — the reduce instrument — was still running,
so the Windows legs of M1–M5 were **NOT RUN**.

**`F22-06-LEG-WINDOWS: NOT RUN`** — the reduce instrument had not finished
building on a heavily contended box within this session's budget; the Windows
observations of M1–M5, and specifically the writer-lease-across-restart
observation that threat T-22-06 exists for, are open.

This is the single largest gap in the determination and it is not papered over.
The schema half of the verdict is platform-independent Rust compiled from one
source and the panel said so explicitly; the **lease** half is
`#[cfg(unix)]`-gated with a recorded prior defect class about Windows byte-range
locks being mandatory rather than advisory, and nothing here measured it.

---

## 6. Cleanup

The probe variant, the probe reducer arm and both measurement examples were
reverted; `git status` shows no modification under `crates/` from Task 1. The
throwaway home, the scratch migration tree and its target dir were removed. The
84,327-byte journal, its snapshot and its authority file are retained as
`22-01-EVIDENCE/linux/session-journal.bin` and companions — they are the corpus
Task 3's F12 non-regression canary is meant to pin.
