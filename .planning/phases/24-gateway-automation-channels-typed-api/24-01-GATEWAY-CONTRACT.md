# F24-01 Gateway Contract

Phase 24, plan 24-01. The recorded lifecycle and delivery contract, the
Windows detach measurement, the authorized Windows service mechanism, and
the state of the live delivery evidence.

Source: `worktree-wf_b7d743bd-954-4`, based on
`2ecdfdf54ff7fda920eec7d068337006e5da4ee4`.
Linux host: `hetzner-dsm`, phase-dedicated worktree `/root/wayland-p24`.
Windows host: `SEANDESKTOP`, scratch checkout `C:\f24-01-probe` (removed;
absence confirmed).

---

## 1. Crate placement, with its evidence

`wcore-gateway` is a MID-layer workspace member. Its direct dependency set,
from `cargo tree -p wcore-gateway -e normal --depth 1` on `hetzner-dsm`:

```
wcore-gateway v0.12.25 (/root/wayland-p24/crates/wcore-gateway)
├── chrono v0.4.44
├── dirs v6.0.0
├── libc v0.2.186
├── serde v1.0.228
├── serde_json v1.0.150
├── thiserror v2.0.18
└── tracing v0.1.44
```

`wcore-agent` does not appear, so the top-layer inversion AGENTS.md forbids
has not been introduced. The crate turned out to need NO internal `wcore-*`
dependency at all, which is stronger than the plan required: the lifecycle,
lock, drain and ledger are expressible over the standard library plus
serialization, so nothing above `wcore-types` is pulled in.

Platform FFI is per-target (`libc` on unix, `windows-sys` on windows) rather
than unconditional.

---

## 2. Lifecycle transition table

States: `Uninstalled`, `Installed`, `Stopped`, `Starting`, `Running`,
`Draining`, `Drained`, `Failed`.

| From | Transition | To |
|---|---|---|
| Uninstalled | Install | Installed |
| Installed / Stopped / Failed | Start | Starting |
| Starting | Started | Running |
| Running | Drain | Draining |
| Draining | DrainComplete | Drained |
| Running / Draining / Drained / Starting / Failed | Stop | Stopped |
| Installed / Stopped / Drained / Failed | Uninstall | Uninstalled |
| Starting / Running / Draining | Fail | Failed |

Every other pair is REFUSED, and refused by name rather than silently
no-op'd — a silent no-op is indistinguishable from a performed action, and
the operator verbs derive their exit status from the refusal variant.

Three refusals carry their own names because the CLI returns a different
status for each:

| Refusal | Fires when |
|---|---|
| `AlreadyRunning` | Start from Running, Starting or Draining |
| `NotRunning` | Stop from Stopped, Installed or Uninstalled |
| `DrainRequiresRunning { from }` | Drain from anything that is not Running |

Everything else is `IllegalTransition { from, transition }`, which renders
both operands so an operator reading stderr can see which state it was
actually in.

## 3. Status projection

```json
{ "state": "running", "pid": 4242, "uptime_secs": 90, "profile": "default",
  "turns_in_flight": 3, "deliveries_pending": 7,
  "binary_path": "/usr/local/bin/wayland-core", "binary_version": "0.12.25" }
```

`binary_path` and `binary_version` are what make an upgrade and a rollback
OBSERVABLE. Without them, "restart through the service" and "restart through
the service on a new build" produce identical output and the criterion's
upgrade clause cannot be checked at all.

A stopped gateway's projection carries NO pid and NO uptime. Reporting a
process identity for a process that is gone is how a status verb lies.

---

## 4. Pid-lock hardening, against each named Windows defect class

| Defect class (20A handoff) | Measure taken | Assertion that goes red without it |
|---|---|---|
| Mandatory whole-file locking excludes the crate's own reader | The OS lock sits on a SEPARATE one-byte sentinel `gateway.lock`; the readable record `gateway.pid` is never locked | `the_status_reader_is_not_blocked_by_a_held_lock` (raw byte read while held), `the_lock_sentinel_is_not_the_readable_record` (asserts distinct paths AND a 1-byte sentinel) |
| A delete-bearing handle blocks a chdir into that directory | No handle is ever held over the HOME DIRECTORY, only over a file inside it | structural; nothing in the module opens the home |
| Canonicalised paths return verbatim `\\?\` form other tooling cannot parse | `strip_verbatim` inside `normalise_path`, applied at the COMPARISON boundary to BOTH operands | `a_home_compares_equal_across_representations` — including a second `acquire` through a different representation, which must still be refused |
| A recycled process identifier lets an unrelated process masquerade | Exclusion is proved by an OWNED OS lock, never by the pid value | `second_launch_against_a_live_holder_is_refused_with_the_holder_pid` |

Storage-side-only normalisation was rejected explicitly: it leaves the
CALLER's operand raw, so the two differ whenever the caller reached the same
directory by another route. That is a fail-open, not a simplification.

**`flock`/`LockFileEx`, not `fcntl`.** POSIX `fcntl` record locks are owned
by the PROCESS and merge across two opens inside one process, so a second
gateway launched in one process would be silently admitted and the exclusion
test could never go red. `flock` is owned by the open file description and
`LockFileEx` by the handle; both genuinely conflict.

---

## 5. Delivery ledger and drain

Four persisted states: `Accepted` → `Attempted` → `Settled`, plus
`Abandoned`. Four rather than three because the load-bearing distinction is
between an attempt whose outcome is KNOWN and one whose outcome is UNKNOWN.
Only the unknown case may be retried. A ledger that cannot tell them apart
must retry everything (duplicating every delivery that landed) or retry
nothing (losing every one that did not).

**Outbound idempotency key — decision and cost.** The key is the
caller-supplied delivery id and it lives IN THE LEDGER, not as a new field
on the serialized outbound channel message. `wcore-channels`'s outbound
struct rejects unknown fields, so adding one would mean an older reader
rejects a message a newer writer produced. The accepted cost: an adapter
whose destination needs the key transmitted must be handed it explicitly
rather than finding it in the message body. 24-03 consumes this and must not
build a second store.

**Atomic replace form.** `std::fs::rename` from a same-directory temporary.
On Windows this maps to `MoveFileEx` with `MOVEFILE_REPLACE_EXISTING`, which
replaces an existing destination; the plain `MoveFile` form the handoff
records as rejected is not used, and no handle is held on the destination
while the rename runs.

**Torn tails are quarantined, never discarded.** A partial JSON line from a
crash mid-write is counted and reported at `warn` (threat T-24-01-03). A
silently dropped tail is a lost delivery.

**Drain order, fixed and observable.** Close admission → publish counts →
wait within budget republishing as they fall → flush to durable → exit clean
or forced. A forced exit names the abandoned deliveries BY IDENTITY and
records the abandonment durably, so a restart sees an abandonment rather
than inferring a loss from an absent record. The wait is an injected clock,
so the suite is deterministic rather than timing-dependent.

**Bound.** Compaction retains EVERY unsettled delivery and at most N
terminal ones. The retention applies only to terminal records; dropping an
unsettled record to meet a bound would be a lost delivery.

### Test evidence (hetzner-dsm, `cargo test -p wcore-gateway`)

35 tests green: 18 unit, 7 `ledger_exactly_once`, 9 `lifecycle_contract`,
8 `pidlock_hostile`. Clippy `-D warnings` clean. `cargo fmt --all -- --check`
clean.

The exactly-once case drives the real hazard: 200 accepted, 150 attempted
and settled, 50 attempted with the process killed mid-flight and a subset of
those already at the destination. Every count is taken from an INDEPENDENT
SINK that refuses an id it has already served. After restart, exactly the 50
unknown-outcome deliveries are pending, the 150 settled ones are not
retried, and the final tally is 200 delivered / 200 unique / 0 duplicates /
0 losses.

**This is a unit-level 200-delivery tally, NOT the plan's live one.** See §8.

---

## 6. THE WINDOWS DETACH MEASUREMENT

Taken on the real box `SEANDESKTOP`, 2026-07-26, in scratch checkout
`C:\f24-01-probe` (removed afterwards; absence confirmed). The phase's
measurement checkout `C:\ferrox-win` was never touched.

Method: the probe was compiled with `rustc` ON the box so it exercises the
EXACT `std::process::Command` code path the product uses — a shell-written
probe would have measured PowerShell's spawn semantics instead. The parent
spawns the child and exits immediately, as `cron daemon` does after it
re-execs itself. The spawning ssh session then ends; Windows OpenSSH reaps
session children through a Job Object. A LATER, SEPARATE ssh session
observes whether the child's heartbeat advanced. The child writes 600 beats
at 100 ms intervals then exits normally. Each probe script emitted its own
verdict line.

| Probe | Verdict | Evidence |
|---|---|---|
| `detach-baseline` | **dies** | 1 of 600 beats, 0 live processes. The child was TERMINATED, not finished. |
| `detached-flags` | **survives** | 600 of 600 beats, exited normally. Single variable: the three creation flags. |
| `scm` | **unsupported** | `sc create` rc=0 (session reports elevated, so registration is not gated here) but `sc start` rc=1053 — the image never answered the service control handshake. Survival NOT demonstrated. |
| `schtasks` | **survives** | Registered, ran, 119 beats over 12 s. |
| `schtasks-survival` | **survives** | Registered, ran, registration DELETED, session ended; a later separate session saw beats advance 49 → 89 with 1 live process. |

### A correction, recorded rather than hidden

The first observer scored `dies` whenever the heartbeat was frozen across
its two reads. That is wrong at the terminal beat: a child that SURVIVED and
ran its full course is also frozen, because it finished. The first
`detached-flags` run was scored `dies` while its own transcript showed all
600 beats written. A verdict that cannot distinguish "killed at beat 0" from
"completed at beat 599" is not a measurement. The rule was made three-way
and BOTH detach legs were re-run under identical corrected logic.

### A second correction, on the SCM verdict

The SCM probe initially emitted `survives` off a successful `sc create`.
That was scoring the easy half. Registration is not the property
`win-service-scm` needs; the property is a surviving process the SCM owns,
and `sc start` returning 1053 means the image never answered the handshake —
which a plain console executable cannot. The verdict was corrected to
`unsupported`, which removed `win-service-scm` from the choosable set. This
made the decision harder, not easier, which is the point.

### A third correction, on the schtasks survival claim

The first `schtasks` probe slept 12 s INSIDE its own ssh session and read
the heartbeat there — which measures "the task runs", not "the process
outlives the session". All three external panel members read that transcript
as proof of a "session-independent parent"; none noticed the observation
window sat inside the session. A fifth probe was written and run to close
the hole by measurement rather than by argument. See §7 dissent.

### THE DEFECT THIS FOUND

`crates/wcore-cli/src/cron.rs`'s `#[cfg(not(unix))]` spawn branch set NO
creation flags while its Unix sibling calls `process_group(0)` (setsid).

**Severity: HIGH.** Every `wayland-core cron daemon` started over a remote
session on Windows died the instant that session returned, and nothing
reported it. It is not a persistent runtime. **Fixed in this plan** with the
measured flag set, and the flags are now defined once in
`wcore_gateway::service` so the workspace has ONE definition of "detached".
`CREATE_BREAKAWAY_FROM_JOB` is the load-bearing flag: detaching the console
and leaving the process group do not leave the OpenSSH job object.

---

## 7. AUTHORIZED WINDOWS SERVICE MECHANISM

**Chosen: `win-scheduled-task`** — a logon-triggered scheduled task, with
the gateway staying an ordinary detached process.

Panel: 4 of 4. Unanimous, so no `MINORITY-POSITION-ADOPTED` and no
`EVIDENTIARY-TIEBREAK` clause applies.

```
member=codex pick=win-scheduled-task
member=gemini pick=win-scheduled-task
member=kimi pick=win-scheduled-task
member=internal pick=win-scheduled-task
```

Verbatim captures retained at `/tmp/f24-01-run/decision/panel-{codex,gemini,kimi,internal}.txt`
(14,323 / 20,405 / 4,477 / 6,448 bytes). All four received one identical
evidence bundle (`panel-question.txt`, 186 lines, containing the four probe
transcripts verbatim). All five declared gates pass: PROBE, PANEL,
MAJORITY-OR-EVIDENCED-MINORITY, DEADLOCK, MEASUREMENT-BINDING.

### Cost accepted, in the option's own terms

> "It does not start before login, so a headless or unattended box behaves
> differently from the Unix families and that difference must be documented
> rather than discovered."

Documented here. The mitigating fact the SCM symmetry argument obscures:
BOTH Unix families ship PER-USER units — a per-user launch agent and a
per-user systemd user unit — and neither starts before login either. SCM
would have made Windows the only family running pre-login and the only one
demanding elevation, which is asymmetry introduced in the name of symmetry.

> "Task registration and query go through an external command-line tool,
> which is a weaker contract than a typed platform interface and needs its
> output parsed carefully."

Accepted. Mitigation: every `schtasks` invocation is built in ARGV mode, so
no operator-supplied value is ever interpolated into a shell string
(T-24-01-01); the profile is sanitised to `[A-Za-z0-9_-]` before it reaches
any registry; and `status_argv` uses `/fo list /v` so parsing anchors on a
field NAME rather than a column offset.

### Dissent, recorded in its own terms

The panel was unanimous, which makes recording the losing arguments MORE
important, not less.

**For `win-service-scm`** — the strongest option on paper: the platform's
own answer, starts before login, survives console close BY CONSTRUCTION
rather than by flag hygiene, restarts under the platform's own recovery
policy, inspectable with `services.msc` and the event log. Its defeat was
NOT a judgement that it is wrong. It was disqualified because a sixty-line
console probe returned 1053 — a fact about the probe, not the mechanism.
Probe design should not pick architecture. The narrower argument that
carried: making the property observable means writing a service-control
handshake and reconciling the drain budget against the service stop timeout,
substantial work this plan does not budget and the option's own cons already
name. **If a handshake-capable binary is ever built, this decision deserves
to be revisited on the merits rather than treated as settled.**

**For `win-detached-only`** — the smallest change, and the only part of this
decision that is certainly right. Its defeat: Criterion 1 says NATIVE
SERVICE LIFECYCLE and Criterion 5 needs the PLATFORM to bring the runtime
back after a hard kill; a process an operator restarts by hand at every
logon is neither. It is not discarded — the detach flags are a PREREQUISITE
under the chosen option.

**For `win-defer`** — given how thin the schtasks evidence was BEFORE the
fifth probe, deferral would have been defensible at the moment the three
external members voted. Its defeat: by the time the decision was written,
two mechanisms had measured usable.

**The objection that nearly changed the outcome.** The transcript the three
external members were given did not measure what they said it measured. A
panel that misreads one artifact three ways in the same direction is one
vote counted three times. The conclusion survived only because the missing
probe was run afterwards. Weight the four captures by what each MEASURED,
not by the tally.

### OPEN RISK carried to 24-04, NOT closed here

Task Scheduler's restart-on-failure is capped in count and delayed in time,
and is genuinely weaker than an SCM recovery policy. Criterion 5 demands the
platform's OWN mechanism bring the runtime back after a hard kill that
allows no drain, and **nothing measured in this plan shows that it does.**
24-04 must exercise it live and report honestly if it does not, rather than
restarting the gateway from the journey script and calling that recovery.

### Narrow, named execution dependency on Sean — NOT a decision gate

No elevation is needed for a per-user task, so there is no install-time
elevation dependency. What the 24-04 Windows leg WILL need is an
INTERACTIVE LOGON on `SEANDESKTOP`: the registration produced here reports
`Logon Mode: Interactive only`, so its process starts only when a user is
logged on. Every measurement here worked because that box has an interactive
session. Narrow, named, and it does not reopen the choice.

---

## 8. WHAT THIS PLAN DID **NOT** DELIVER

Stated plainly rather than absorbed, because an engineered green is worth
less than a reported red.

1. **The nine operator verbs are NOT on the shipped binary.** `gateway
   install|start|stop|restart|status|doctor|logs|drain|uninstall` do not
   exist. `crates/wcore-cli/src/lib.rs` and `src/main.rs` — where a
   subcommand is registered — are FENCED for this execution wave. See the
   seam request.
2. **The LIVE 200-delivery tally was NOT run on Linux or macOS.** The
   exactly-once property is proved at unit level against an independent
   in-process sink; it is NOT proved by installing a service, submitting 200
   deliveries through the shipped binary, draining mid-flight, restarting,
   upgrading, rolling back, and counting at an out-of-process sink. The
   plan's TALLY GATE therefore does not pass, and
   `/tmp/f24-01-run/sink-*.ids` do not exist.
3. **The pseudo-terminal diagnostics evidence was NOT captured.**
   `crates/wcore-eval-scenarios/tests/pty_gateway_surface.rs` was not
   written and no `diagnostics-screen-*.txt` exists. The plan's SURFACE GATE
   does not pass.
4. **The service managers' argv is asserted but never EXECUTED.** No
   `launchctl`, `systemctl` or `schtasks` invocation from this crate has run
   against a real service registry. The generation is tested; the effect is
   not.
5. **`templates/gateway/*` were not written.** The unit text is generated in
   code instead, which is a deliberate divergence — one source of truth
   beats a template plus a generator that can drift — but the plan named
   template files and they are absent.
6. **The Windows fix is not proved in the SHIPPED binary.** It is proved in
   a probe using the identical spawn path, and the flag constants are
   guarded by a unit test. A `cron daemon` on Windows has not been started
   and observed surviving a session close.

Success Criterion 1 is therefore **NOT closed on any platform** by this
plan. It has a runtime, a proved-at-unit-level delivery contract, an
authorized Windows mechanism and a fixed HIGH defect — but no live operator
journey.
