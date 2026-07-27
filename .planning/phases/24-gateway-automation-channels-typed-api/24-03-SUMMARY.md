---
phase: 24-gateway-automation-channels-typed-api
plan: "03"
subsystem: channels-and-typed-api
tags: [channel-framework, probe, binding, health, reload, typed-client, roles, cursor, support-bundle]
status: partial
plans-not-executed:
  - "24-04 — NOT STARTED"
requires:
  - "24-01"
  - "24-02"
  - "24-B"
  - "24-C"
provides:
  - wcore_channels::probe (setup/auth probe; default is a NAMED Unsupported, never a green)
  - wcore_channels::binding (escaped conversation keys, declared default route)
  - wcore_channels::media (declared per-adapter bounds, explicit degradation)
  - wcore_channels::health (observed per-adapter health; Unknown is not Healthy)
  - wcore_channels::manager::{reload, probe_all, health, edit_on, delete_on, take_registered}
  - wcore_channels::ChannelError::Unsupported (distinct from Rejected)
  - "wayland-core channel list|probe|health|reload (shipped binary)"
  - wcore_acp::roles (Forbidden distinct from Auth; deny-all default)
  - wcore_acp::idempotency (bounded ledger; full REFUSES rather than evicts)
  - wcore_acp::cursor (stream-identified, gap-aware, three explicit refusals)
  - wcore_acp::negotiate (named refusal, numeric version comparison)
  - wcore_gateway::support_bundle (structural elision + scrubbing, canary-proved)
affects:
  - crates/wcore-cli/src/gateway.rs (the running gateway hosts the adapters and republishes their health)
  - crates/wcore-gateway/src/pidlock.rs (process_is_alive no longer reports pid 0 as alive)
tech-stack:
  added: []
  patterns:
    - "a default that cannot be honest reports a NAMED nothing, never a green"
    - "two numbers from two independent sources, so a disagreement is detectable"
    - "a resumable position must carry the identity of the stream it was minted against"
    - "structural elision first, exact-secret scrubbing only as a backstop"
key-files:
  created:
    - crates/wcore-channels/src/{probe,binding,media,health}.rs
    - crates/wcore-channels/tests/framework_matrix.rs
    - crates/wcore-acp/src/{roles,idempotency,cursor,negotiate}.rs
    - crates/wcore-gateway/src/support_bundle.rs
    - crates/wcore-gateway/tests/support_bundle_redaction.rs
    - crates/wcore-cli/src/channel.rs
    - .planning/phases/24-gateway-automation-channels-typed-api/24-03-SURFACE-CONTRACT.md
  modified:
    - crates/wcore-channels/src/{lib,error,manager}.rs
    - crates/wcore-channels-registry/src/lib.rs
    - crates/wcore-channel-discord/src/lib.rs
    - crates/wcore-channel-email/src/lib.rs
    - crates/wcore-acp/src/{lib,error,protocol,transport/http}.rs
    - crates/wcore-gateway/src/{lib,pidlock}.rs
    - crates/wcore-cli/src/{lib,main,gateway}.rs
decisions:
  - "Channel::probe defaults to a NAMED Unsupported: a default of Ok is an adapter attesting to a configuration it never read"
  - "ChannelError::Unsupported is a new variant distinct from Rejected — 'the platform said no' is retryable, 'there is no such API' is not"
  - "channel health REFUSES when no gateway is running; it never prints an empty list"
  - "reload treats an unfingerprintable adapter as CHANGED — the opposite direction keeps a rotated credential in service"
  - "a resumable cursor carries the stream identity; a bare position resumes silently into the wrong stream after a restart"
  - "a full idempotency ledger REFUSES a new identity rather than evicting an old guarantee"
  - "the support bundle is a directory, not a .tar.gz: no lockfile edit, and the scan reads real bytes"
metrics:
  tests-green: 727
  mutations-proved: 10
  completed: 2026-07-27
---

# Phase 24 Plan 03: Channel Framework and Typed Client Summary

**`wcore-acp` was completely untouched at the start of this lane and now carries
four contracts; the channel framework gained probe, binding, media, health,
reload, edit/delete/reaction and a shipped operator surface; the CRITICAL
support-bundle threat is closed with a canary that has a positive control. The
first live run on real hardware found two HIGH defects that the 727-test suite
and clippy both missed, and one of them was a false zero produced by the code
written to close false zeros. 24-04 was not started.**

## Termination state

**State 2 of the plan's three — "Complete with named gaps".** State 1 is not
claimable: no macOS or Windows evidence, the four typed-client contracts are
not yet wired into the ACP server's request path, and the plan's two named
integration tests were not written. Every gap is in §6, none is softened.

## 1. What landed

### The channel framework contract (Task 1)

Seven elements, 17 matrix cases, recorded element by element in
**`24-03-SURFACE-CONTRACT.md` §1**. The recurring design decision is the same
one in four places, and it is the decision the phase keeps re-learning: **a
default that cannot be honest must report a NAMED nothing, never a green.**

- `Channel::probe` defaults to `ProbeOutcome::Unsupported`, which is not ready.
  A default of `Ok` would be each of the ten registered adapters attesting to a
  configuration it never read.
- `edit`/`delete`/`react` return a new `ChannelError::Unsupported`, distinct
  from `Rejected`. "The platform looked and said no" is retryable; "there is no
  such API" is not, and folding them together let a caller retry forever
  against a surface that will never exist.
- A registered-but-unpolled adapter reads `HealthState::Unknown`. Every
  non-healthy state carries a reason by construction.
- Reload treats an unfingerprintable adapter as CHANGED. The opposite direction
  is an operator rotating a credential, reloading, seeing success, and still
  sending through the adapter holding the old one.

Binding keys are escaped, so a conversation named `general/t42` cannot inherit
the binding of thread `t42` in conversation `general` (T-24-03-03). An unbound
conversation takes a **declared** default — `BindingTable::new` requires one and
there is no constructor that omits it, so there is no path where a new
conversation inherits whoever was served last.

### Two reference adapters, deliberately different SHAPES

Discord (persistent connection, one HTTP round trip, five probe cases against a
local `mockito` endpoint) and email (polling, four credential handles across two
protocols, a real IMAP `LOGIN` + `SELECT` + `LOGOUT`). **No vendor credential
and no vendor network reach in any case.** The `SELECT` is not decoration: a
login that succeeds against a mailbox name with a typo is a channel that starts
and then receives nothing, forever.

### The shipped operator surface

`wayland-core channel list | probe | health | reload`. The design point is
**where each verb gets its answer**:

| verb | truth source | needs a running gateway |
|---|---|---|
| `list` | the config directory | no |
| `probe` | the PLATFORM, asked live | no — an operator debugs credentials with the gateway DOWN |
| `health` | the RUNNING gateway's observations | **yes, and it refuses otherwise** |
| `reload` | a request the gateway acts on | **yes** |

### The typed client — four contracts (Task 2)

`roles`, `idempotency`, `cursor`, `negotiate`, 125 unit tests. Full table in
the contract document §2. The one worth reading:

**A bare `u64` is not a resumable cursor.** After a restart an in-memory log
renumbers from 1, so a client resuming at position 2 is handed positions 3.. of
a *different* stream, believes itself continuous, and silently misses the new
stream's 1 and 2. Nothing errors, nothing duplicates, and **the server's own
counts look perfect** — the exact shape lane 24c measured at the independent
sink. `Cursor` therefore carries the stream identity, and its test has a
positive control proving the same position on the right stream IS servable, so
the refusal is attributable to the identity alone.

### The support bundle — T-24-03-01, the phase's only CRITICAL

Structural elision first (key NAMES only; values never read into the bundle),
exact-secret scrubbing second and only over free text. Proved by canary with
three separate positive controls, including one on the SCANNER itself — a
scanner that always returned an empty list would otherwise make every redaction
test pass. Live-proved against the home a real gateway had just run in.

## 2. Verification

| Gate | Result |
|---|---|
| `cargo test --no-fail-fast` over 13 crates | **727 passed, 0 failed, 1 ignored** (rc=0) |
| `cargo clippy … -D warnings` (6 core crates) | rc=0 |
| `cargo clippy -p wcore-cli --all-targets -- -D warnings` | rc=0 |
| `cargo fmt --all -- --check` | rc=0 |
| Seam gate vs the **merge-base**, paths inlined | rc=0, with a control returning **1** |
| §6 fence, `wcore-cli/src/{lib,main}.rs` | **18 insertions, 0 deletions** |
| Live canary gate without its inputs | **rc=101 — fails, does not skip** |
| Live canary gate with them | rc=0, positive control satisfied |

**No gate in this lane terminates in a pipe**; every exit status is captured
into a variable on the line after the command. The seam gate uses inlined
pathspecs — the `$SEAM` form is self-passing under zsh (F24-02-H1) and was not
used.

Two clippy errors were fixed at the source (an unnecessary `mut`, an import only
the test module used). **Neither with an `#[allow]`.**

## 3. Ten mutations, each reddening its named test

M1 probe default returns a green · M2 binding stops escaping · M3
unpolled reads Healthy · M4 reload keeps an unfingerprintable adapter · M5
pid-zero guard removed · M6 cursor drops the stream check · M7 cursor answers
an impossible position with `[]` · M8 bundle keeps config values · M9 full
ledger evicts · M10 unclassified method falls to least privilege.

Every one recorded a **baseline** run first — a mutant that reddens a
test which was already red proves nothing — and every restore was verified by
SHA-256 rather than assumed. Full table in the contract document §5.

## 4. Two process findings, both about gates

**F24-D-P1 — the mutation harness was itself self-passing.** M2 first reported
`baseline_rc=0 mutant_rc=0`. The mutation was right; the harness ran
`--test framework_matrix` with a filter naming an INLINE unit test, so the
filter matched **zero tests** and `cargo test` exited 0 both times. An eleventh
entry for the standing list, and a new shape: **a test filter that matches
nothing reports success.** It was caught only because the harness compared a
baseline against the mutant instead of asserting on the mutant alone.

**F24-D-P2 — a seam gate against a BRANCH NAME mis-attributes other lanes.**
Run as `git diff --quiet plan/f20-unified-audit-repair -- …`, the fence check
reported **28 deletions this lane never made** (`pub mod backup;`, `pub mod
node;` — both ADDED upstream after the branch point, since the branch moved
`b303a366` → `32e2f57d` during the lane). The gate must name the MERGE-BASE
SHA. Against `b303a366` the true answer is 18 insertions, 0 deletions.

## 5. Two HIGH defects the live run found — and one is mine

Both invisible to 727 tests and to clippy. Neither would have been found
without launching the shipped binary against a fresh home, which is exactly
what AGENTS.md §11 says and exactly what a green suite does not substitute for.

**F24-D-H1 (HIGH, FIXED).** `channel probe` could not run at all, and the
gateway registered **zero** channels, on any host without an LLM provider key:

```
wayland-core channel: cannot resolve configuration: No API key found.
[gateway] channel reload: credentials store: No API key found.
```

Opening the credentials store reads `storage.credentials` and has nothing to do
with a provider, but it was reached through `Config::resolve`, which enforces
provider-key presence. The host this breaks is precisely the one an operator
debugs a fresh install on. Resolution now retries with a placeholder provider
key **only** when the failure is specifically `MissingApiKey`; every other
config error still propagates, and the operator's real `storage.credentials`
backend is still what gets opened. Falling back to a *default* storage config
would have silently opened the wrong store and reported a correctly configured
channel as incomplete.

**F24-D-H2 (HIGH, FIXED) — the false zero, reintroduced by the code closing
false zeros.** With zero channels registered for that reason, `channel health`
printed:

```
gateway is running and has registered no channels
```

while two channels sat in the config directory. An empty array cannot
distinguish "you have none" from "I could not load yours", and the rendering
asserted the first. This is the shape of F24-C-M2, F24-B-H3 and the Windows
orphan scanner — and it appeared in the file whose own module documentation
says *"a zero from a surface that was not looking is not a zero."*

The published document now carries `configured` (counted by scanning the config
DIRECTORY) beside `registered` (what the gateway built) plus
`registration_error`. **The two numbers come from independent places**, and
`channel health` exits non-zero when they disagree — including on a silent
shortfall with no error, which is the quieter version of the same bug.

**F24-D-M1 (MEDIUM, FIXED).** `process_is_alive(0)` returned **true** on Unix,
always. Found by a `channel health` test, then measured directly rather than
assumed:

```
kill(0,0) = 0 errno=0        /proc/0 exists = 0
```

POSIX defines `kill(0, sig)` as the CALLER'S process group, so it succeeds for
everybody. Pid 0 is a value this module itself emits — `AlreadyHeld { pid: 0 }`
on an unreadable record — so `gateway status` and `channel health` would report
a live gateway from a placeholder. The Windows arm was already correct, so the
guard restores agreement across families rather than adding a quirk.

## 6. What was NOT delivered — stated plainly

1. **24-04 was NOT STARTED.** No journey driver, no receipt schema, no receipt
   on any platform. Its terminal publication task was never reached; nothing was
   pushed to main, merged, tagged, released, or used to close an issue.

2. **The four typed-client contracts are NOT wired into the ACP server's
   request path.** `roles`, `idempotency`, `cursor` and `negotiate` are complete,
   tested and mutation-proved as MODULES. `server.rs` does not yet call
   `authorize` before dispatch, `message/send` does not yet append to an
   `EventLog`, and no transport yet serves a resume request. **The contracts
   exist; the plane does not.** This is the same honest distinction 24-02 drew
   about `event`/`webhook`/`poll` — a complete vocabulary and an incomplete
   plane — and it is stated the same way rather than rounded up.

3. **The plan's two named integration tests were not written.**
   `crates/wcore-acp/tests/typed_client_recovery.rs` and
   `crates/wcore-acp/tests/roles_and_idempotency.rs` do not exist. The recovery
   test in particular is the one that matters, because driving a REAL client
   against a REAL server over the streaming transport and severing it mid-stream
   is the only thing that would exercise item 2 above. Unit-level evidence is
   NOT offered as a substitute for it.

4. **No macOS and no Windows evidence.** CI fires on `lane/**` and `lane/24d`
   is pushed (run `30270352912`), so the artefacts are obtainable per
   `.planning/intel/MACOS-BINARY-IS-OBTAINABLE.md`; the run had not produced
   them before this lane's budget ran out. This is a budget outcome, not an
   impossibility, and it is the SAME gap lane 24c reported — it is not closed.

5. **24-03 Task 3's full live matrix was not run.** What ran is in the contract
   document §4: list, probe, health, reload, gateway up, `kill -9`, health
   refuses. What did NOT run: an inbound fixture message admitted / deduplicated
   on replay / access-decided / bound / routed; an attachment normalised end to
   end through a running adapter; edit, delete and reaction against a live
   fixture; a severed fixture connection followed by a visible reconnect. Those
   elements are proved at unit level and **not** end to end from the binary.

6. **The PTY surface gate was not attempted.**
   `crates/wcore-eval-scenarios/tests/pty_channels_surface.rs` does not exist and
   no rendered screen text was captured. This is the SECOND lane to leave
   24-02's SURFACE gate unattempted, and it is still unattempted.

7. **There is no `gateway support-bundle` CLI verb.** The live bundle was
   produced by a test driver calling the production `collect`. The redaction is
   proved; the operator's route to it is not built.

8. **The email probe checks SMTP credentials for PRESENCE only.** A present-but-
   wrong SMTP password passes the probe and fails at first send. Stated in the
   method's own documentation, not discovered later.

9. **24-02's CONTINUATION and SURFACE gates are still not satisfied as written**
   — this lane did not attempt either. That inherited gap is unchanged.

## 7. Deviations

**[Reordering] Task 2's four contracts were built as modules before Task 1's
live matrix was finished, and Task 3 was only partially run.** Reported in §6,
not absorbed.

**[Rule 3 — blocking] Files outside the plan's `files_modified` were edited.**
`crates/wcore-gateway/src/{lib,pidlock}.rs` (F24-D-M1's guard, and registering
the bundle module), `crates/wcore-acp/src/{error,protocol,transport/http}.rs`
(the `Forbidden` error variant, its JSON-RPC code and its HTTP mapping — the
compiler's exhaustiveness check demanded the transport mapping, which is the
correct outcome), and `crates/wcore-cli/src/gateway.rs` (the gateway must host
the adapters or `channel health` could only ever fail). Each edit is additive
and carries its own test.

**[Deviation] `ErrorCode::Forbidden = -32005` was added to `wcore-acp`'s
JSON-RPC vocabulary.** Verified first that the ACP error-code space appears
NOWHERE outside `wcore-acp/src/protocol.rs` and its own test — a repository-wide
search for `-32004` returns two hits, both in that file — so no contract fixture
and no Desktop manifest moves. **No `wcore-contract generate` was run.**

**[Deviation] The support bundle is a DIRECTORY, not a `.tar.gz`.** The plan's
canary gate decompresses an archive. `tar` and `flate2` are workspace deps but
not `wcore-gateway`'s, and adding either rewrites `Cargo.lock`. Reasoning and
the equivalent (stronger) scan are in the contract document §3.

**[Deviation] `wcore-cli` reaches the channel types through a re-export from
`wcore-channels-registry`** rather than a direct dependency edge, for the same
lockfile reason. **`Cargo.toml` and `Cargo.lock` are untouched by this lane** —
seam gate rc=0 against the merge-base, with a control returning 1.

**Shared-file fence honoured.** `crates/wcore-cli/src/{lib,main}.rs`: 18
insertions, 0 deletions, one contiguous block in `lib.rs` and two in `main.rs`
(the clap variant and its dispatch arm, which cannot be contiguous with each
other). No reformatting, no reordering, no renaming.

**Base not merged forward.** This lane is based at `b303a366`; the branch has
since moved to `32e2f57d`. The orchestrator merges lanes serially, and the brief
forbids merging into the base. Gates were run against the lane tree, not against
a merged one — an integrator should re-run them post-merge.

## 8. Findings ledger

| ID | Severity | Status |
|---|---|---|
| **F24-D-H1** channel subsystem silently disabled on any host with no LLM provider key | **HIGH** | **FIXED**, found live, re-measured live |
| **F24-D-H2** `channel health` reported a failed registration as an empty installation | **HIGH** | **FIXED**, two independently-sourced counts + non-zero exit |
| F24-D-M1 `process_is_alive(0)` returned true on Unix, always | MEDIUM | **FIXED**, measured with a C probe, mutation-proved (M5) |
| F24-D-P1 the mutation harness was self-passing (a filter matching zero tests) | MEDIUM (process) | **FIXED** in the harness; recorded as a new self-passing shape |
| F24-D-P2 a seam gate against a branch NAME mis-attributes other lanes' work | MEDIUM (process) | **REPORTED** — use the merge-base SHA; affects every lane in this program |
| F24-D-M2 the typed-client contracts are not wired into the server request path | MEDIUM | BACKLOG — named gap, §6 item 2 |
| F24-D-L1 email probe checks SMTP credentials for presence only | LOW | BACKLOG — documented in the method |
| F24-D-L2 no `gateway support-bundle` operator verb | LOW | BACKLOG |

No CRITICAL. Both HIGHs are fixed with executable, live evidence.

## 9. Verdict — Success Criteria 3 and 4

**Criterion 3 (reference channels): PARTIALLY MET on Linux, NOT MET on macOS or
Windows.** Setup/auth, access, routing, media, native actions, reconnect and
health all exist, are tested, and are mutation-proved; the operator surface is
on the shipped binary and was live-exercised. Idempotency was closed by lane
24c. What is NOT met: the end-to-end inbound matrix from the binary against a
fixture (admit → dedupe → access → bind → route), and any evidence at all on the
other two platforms.

**Criterion 4 (typed authenticated clients): NOT MET.** Roles, command
idempotency, an ordered gap-aware cursor and version negotiation all exist as
correct, tested, mutation-proved contracts, and the redacted support bundle is
canary-proved live. But **no typed client has recovered an event gap**, because
the cursor is not yet on the server's request path and the recovery integration
test was not written. A criterion that says clients *recover event gaps* is not
met by a module that *could* let them.

**24-04: NOT STARTED.** Criterion 5 is untouched by this lane.

## Self-Check

Every test count, exit status, transcript line and mutation result above was
copied from captured tool output, not recalled. Files asserted present were
verified on disk; commit subjects were read from `git log`. The `kill(0,0)`
claim was measured with a C probe on the host rather than reasoned from the man
page. The gates that do **not** pass — macOS, Windows, 24-02's continuation and
surface gates — are named as not passing, and the unwired contracts and the
unstarted plan are named as unwired and unstarted rather than sampled.

**Self-Check: PASSED**
