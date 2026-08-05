panel_run_nonce=05028d2c95e3a675

# F23-05 clock-policy cross-audit bundle

You are one of four independent members of a cross-audit panel. Answer the
DECISION below on the EVIDENCE below. Do not ask questions; commit to one option.

## The decision

Which legs of a multi-day wait/resume/complete journey for Wayland Core must run
against REAL elapsed wall time, and which may legitimately use an accelerated
clock?

## Success Criterion 5, verbatim

> A multi-day wait, resume and complete journey preserves cumulative authority,
> resource, evidence, memory and delivery state, with exactly one loop owner at
> every resume point.

## The LIVE determination, measured before this panel was convened

These lines are the verbatim output of scripts/f23-clock-probe.sh, run on the
authoritative Linux host against the binary built at the commit under test. The
probe armed durable budget authority in one real process, let that process EXIT,
elapsed a REAL gap during which no process existed, and bound the authority again
in a SECOND real process.

```
F23_04_PROBE_NONCE=64b35c212168149b
F23_04_PROBE_HOST=Ubuntu-2404-noble-amd64-base
F23_04_PROBE_UNAME=Linux_6.8.0-101-generic_x86_64
F23_04_PROBE_SHA=9be07203bc65bb2cce87621a85df609b2da0ccaa
F23_04_PROBE_BINARY_BUILD_INFO=wayland-core 0.12.25 (source 9be07203bc65bb2cce87621a85df609b2da0ccaa)
F23_04_PROBE_HARNESS=/root/wayland-23B-04/target/debug/deps/multi_day_journey_test-cd7922f357e39e40
F23_04_EXPERIMENT_B=absolute-deadline gap=0s exceeded=false reason=none
F23_04_PROBE_GAP_BEGIN=2026-07-27T14:01:26Z seconds=45
F23_04_PROBE_GAP_END=2026-07-27T14:02:11Z
F23_04_EXPERIMENT_A=absolute-deadline gap=45s exceeded=true reason=max_wall_time rc=0
F23_04_EXPERIMENT_C=active-runtime gap=45s exceeded=false reason=none
F23_04_SEAM_ATTEMPT=WAYLAND_CLOCK_NOW_MS rc=0 exceeded=false
F23_04_SEAM_ATTEMPT=WAYLAND_NOW_UNIX_MILLIS rc=0 exceeded=false
F23_04_SEAM_ATTEMPT=WAYLAND_FAKE_CLOCK rc=0 exceeded=false
F23_04_SEAM_ATTEMPT=WAYLAND_TEST_CLOCK rc=0 exceeded=false
F23_04_SEAM_ATTEMPT=WAYLAND_CLOCK_OFFSET_MS rc=0 exceeded=false
F23_04_SEAM_ATTEMPT=WAYLAND_BUDGET_DEADLINE_UNIX_MILLIS rc=0 exceeded=false
F23_04_SEAM_ATTEMPT=WCORE_CLOCK_NOW_MS rc=0 exceeded=false
F23_04_SEAM_ATTEMPT=WCORE_NOW_UNIX_MILLIS rc=0 exceeded=false
F23_04_SEAM_ATTEMPT=WCORE_FAKE_CLOCK rc=0 exceeded=false
F23_04_SEAM_ATTEMPT=WCORE_TEST_CLOCK rc=0 exceeded=false
F23_04_SEAM_ATTEMPT=WCORE_CLOCK_OFFSET_MS rc=0 exceeded=false
F23_04_SEAM_ATTEMPT=SOURCE_DATE_EPOCH rc=0 exceeded=false
F23_04_SEAM_ATTEMPT=FAKETIME rc=0 exceeded=false
F23_04_SEAM_CLI_FLAG_HITS=0
F23_04_ABSDEADLINE_MATCH_SITES_NONTEST=8
F23_04_ABSDEADLINE_PRODUCT_CONSTRUCTION_SITES=0
F23_04_ABSDEADLINE_PRODUCT_REACHABLE=false
F23_04_ABSDEADLINE_EVAL=system-clock-at-evaluation
F23_04_ABSDEADLINE_CHARGES_DOWNTIME=true
F23_04_ACTIVERUNTIME_CHARGES_DOWNTIME=false
F23_04_CLOCK_INJECTION_SEAM=none
F23_04_ACCEL_HONEST_FOR_ABSOLUTE_DEADLINE=false
F23_04_REAL_GAP_SECONDS=46
F23_04_PROBE=PASS nonce=64b35c212168149b
```

## The four options, verbatim from the plan

### real-time-full
Full real elapsed time — the journey spans at least three real calendar days on
all three platforms with real process restarts and no clock manipulation anywhere.
PROS: the strongest possible evidence and the only one that proves a persisted
deadline, a session store, a journal and an operating system all behaved across
days of the process not existing; it also incidentally exercises the Windows box's
real reboots, which is the environment a user actually has; no argument about what
was simulated.
CONS: costs at least three calendar days of elapsed wall time before the phase can
close, and occupies the Linux host, the Windows box and a Mac for that span; a
defect found on day three costs another full cycle to re-prove.

### real-time-linux-accelerated-elsewhere
Real elapsed time on the long-lived Linux host, accelerated clock on the macOS and
Windows legs.
PROS: keeps the strongest evidence on the one host that genuinely stays up
unattended for days, while the two attended machines finish in an afternoon; the
accelerated legs still perform real process restarts and real persistence, so only
the time span is simulated; total calendar cost is the same three days but only one
host is occupied.
CONS: the macOS and Windows platform claims are weaker than the Linux one and must
be labelled that way in the evidence rather than presented as equivalent; a
platform-specific time or persistence defect could hide on exactly the two
platforms that were accelerated.

### accelerated-except-absolute-deadline
Accelerated clock everywhere except the absolute-deadline leg, which always runs
against real elapsed time on every platform.
PROS: targets the cost precisely at the one authority whose meaning depends on real
time passing while the process is dead, and leaves everything with no real-time
dependency free to run fast; finishes in roughly one day of calendar.
CONS: depends entirely on the determination being correct about which behaviors
have a genuine real-time dependency; if that determination is wrong, an accelerated
leg silently proves nothing and the error is invisible in the evidence.

### escalate
Escalate — none of the above buys evidence worth its cost right now, so record the
decision as open and do not run the journey.
PROS: spends no calendar time on a proof whose shape is not yet agreed, and leaves
the criterion visibly open rather than closed on evidence the owner does not accept.
CONS: Success Criterion 5 stays open, Phase 23B cannot close, and the criterion most
likely to reveal a cross-restart defect goes unexercised.

## The three host facts

* Linux (authoritative, and the only host that stays up unattended for days):
  hetzner-dsm, /root/wayland. Full workspace aggregate is 11,519 tests in roughly
  194 seconds.
* Windows (native leg): SeanDesktop, C:\ferrox-win. The box reboots and is shared;
  the journey surviving that is a feature of the test rather than a problem with it.
* macOS (native leg): a developer Mac. The phase's controlling execution instruction
  forbids running Cargo on it, and the binary resolver the plan expected
  (scripts/f23-macos-binary.sh) was never landed by its owning plan.

## What to weigh explicitly

An accelerated leg makes a platform's span assertion TRIVIALLY satisfiable, which is
exactly why an accelerated leg is a WEAKER claim and must be labelled as one rather
than presented as equivalent evidence. Weigh that against three calendar days.

Consider carefully whether the measured determination REMOVES any option outright.
An option that rests on a mechanism the product does not expose is not a cheaper
option; it is an argument about a thing that does not exist.

## Reply contract — the last three lines of your reply MUST be exactly:

PANEL_NONCE=05028d2c95e3a675
PANEL_POSITION=<one of: real-time-full real-time-linux-accelerated-elsewhere accelerated-except-absolute-deadline escalate>
PANEL_RATIONALE=<one single line, at least forty characters, no newlines>
