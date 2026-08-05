# 25-MACOS NOTES — running log (committed early per LANE-BRIEF §6b-i)

Lane: `lane/25-macos`. Worktree `/Users/seandonahoe/dev/waylandcore-frontier-worktrees/lane-25-macos`.
Base: `plan/f20-unified-audit-repair` @ `e77b44b0`.
Started 2026-07-29T03:30Z. This file is appended and re-committed after every measurement.

## Mandate

Phase 25 has **zero macOS evidence** across all four criteria (INV-24-25.md:17, :463). Phase is
otherwise 3.5/4. Task: establish which legs are genuinely Darwin-specific, run those, report
per-criterion with counts, grade honestly. Divergence between Linux and macOS is worth more than a
clean pass.

## Established so far (t+15min, from documents only — nothing executed yet)

### The four criteria (from INV-24-25.md §PHASE 25 and the phase's own PLAN/SUMMARY files)

| id | criterion | prior state |
|---|---|---|
| 25-C1 | same task local / container / ssh / cloud, equivalent policy, receipts, cancellation, cleanup | MET, composed across two commits |
| 25-C2 | nodes pair, advertise, revoke, recover offline, mixed versions, attribution held | MET as written, one limitation (controller cannot verify node-minted receipt identity) |
| 25-C3 | twelve-verb plugin lifecycle | MET Linux, 11/12 Windows, **zero macOS** |
| 25-C4 | compromised keys/plugins/backends and denied secret/egress fail closed, no orphans | MET |

### Prior-art traps handed to this lane (do not rediscover)

1. `SSL_CERT_FILE` does not reach TLS on macOS — `native_tls` there is Security.framework and
   ignores it (INV-24-25.md:201-203). Any fixture leaning on that trick fails for reasons unrelated
   to Phase 25.
2. macOS process-liveness semantics differ, and the old check was wrong in **both** directions.
   `wcore_types::process_liveness` now exists — use it, do not roll a new one.
3. INV 25-C4: the `ps | grep <nonce>` enumeration is the **weaker instrument** — an orphan that is
   the task's own argv (`sleep 600`) carries no nonce, so `ps | grep` read 0 while pid 1170 was
   alive. Windows had the mirror defect: a **silent false zero** from `ps -eo` being rejected with
   stderr to /dev/null and the pipeline ending `|| true`.

## Working hypothesis: which legs need Darwin (to be confirmed against criterion text)

Prior, before reading the plan criterion text. Ranked by expected Darwin-specificity:

- **HIGH — C4 orphan enumeration / process liveness.** Platform process semantics. macOS `ps` flags
  differ from GNU and from msys; liveness signalling differs (the trap above says measurably so).
  This is the leg where a Linux/macOS divergence is most likely and most consequential.
- **HIGH — C3 plugin load / install of a native artifact.** `.dylib` vs `.so` vs `.dll`, executable
  bit, Gatekeeper quarantine (`com.apple.quarantine` xattr) on a downloaded plugin bundle.
- **MEDIUM — C2 ADVERTISE.** Genuinely probes host capability; macOS has sandbox-exec, no bwrap,
  and Docker may be absent. The advertised set should differ from both prior hosts.
- **MEDIUM — C1 local backend sandbox.** macOS sandbox path is `sandbox-exec`, a different code
  path from Linux bwrap.
- **LOW / PROVES NOTHING ON RE-RUN — pairing crypto, signature verify, receipt schema, policy
  equivalence, role gating, node contract-major mismatch.** Pure logic over portable types. These
  are platform-independent; re-running them on Darwin spends effort for no evidence and I will say
  so rather than pad the count.

## Instrument discipline for this lane

- Every number that reaches the report comes from `/usr/bin/grep`, `/usr/bin/git`, or `rtk proxy`
  (§3b). `rtk` rewrites `git log`, `grep` and `git diff` output.
- Before reporting ANY absence: prove the instrument alive on a known-positive in the same
  invocation, show a non-zero count, and state the query (§3b-i).
- Assert executed test counts (`N passed`), never exit status (§3.2).
- Byte-count every capture.

## Darwin exception usage (§0)

Budget: single-crate, single-test only — `cargo test -p <crate> --test <file>`. Never a workspace
build, never clippy, never release. Will record each use with its justification here.

- (none yet)

## MEASUREMENT 1 (t+20m) — criterion text is PLATFORM-SILENT

Read from `ROADMAP.md:124-133` and `REQUIREMENTS.md:237-241`, i.e. the source text, not a summary.

**None of Phase 25's four criteria, and none of F25-01..05, names any operating system.** Compare
with siblings that DO name one: 24-C5 ("passes on macOS, Linux, Windows"), 27-C5 ("native macOS,
Linux, and Windows"), 28-C1 ("Native macOS, Linux, and Windows"). Phase 28 is *Native
Cross-Platform Certification* and **depends on** Phases 24-27.

Consequence for grading, stated before I run anything so it cannot be tuned to the result:
**I cannot grade C1-C4 NOT MET merely for lacking macOS** — the criteria do not ask for macOS, and
the cross-platform matrix is Phase 28's declared job. The honest finding available to this lane is
therefore NOT "the criteria are unmet", it is **whether the behaviour behind them actually differs
on Darwin**. A divergence is a defect against the criterion's own words; an absence of macOS runs
is not.

## MEASUREMENT 2 (t+30m) — HIGH candidate: node machine_id is Linux-shaped

`crates/wcore-exec-backend/src/node/pairing.rs:102-113`:

```rust
/// Unix hosts publish the hostname on disk regardless of shell environment.
fn read_hostname_file() -> Option<String> {
    for path in ["/etc/hostname", "/proc/sys/kernel/hostname"] {
```

**The doc comment's claim is false on macOS.** Measured, both sides, unproxied, with live positive
controls in the same invocation:

| path | macOS (Darwin 25.3.0 arm64) | hetzner-dsm (Linux 6.8.0-101 x86_64) |
|---|---|---|
| `/etc/hostname` | ABSENT | EXISTS, `Ubuntu-2404-noble-amd64-base` |
| `/proc/sys/kernel/hostname` | ABSENT | EXISTS, `Ubuntu-2404-noble-amd64-base` |
| `/proc` | ABSENT | EXISTS |
| `/etc/hosts`, `/etc/passwd` (positive control) | EXISTS, EXISTS | — |

The positive control is what makes the two ABSENT rows a measurement rather than a broken probe
(§3b-i). macOS keeps the hostname in the SystemConfiguration store, not on disk: `scutil --get
LocalHostName` → `Seans-MacBook-Pro`.

So `local_machine_id()`'s fallback chain on a macOS node reached the way a controller actually
reaches one (non-login ssh, where `HOSTNAME` is a shell variable and is not exported — the code's
own comment says this was found by running the real binary over ssh):

1. `WAYLAND_NODE_MACHINE_ID` — unset
2. `HOSTNAME` — not exported over non-login ssh
3. `COMPUTERNAME` — Windows only
4. `read_hostname_file()` — **None on macOS**
5. → `"unknown-host"`

**Predicted: every macOS node reports `machine_id = unknown-host`.** The field's declared purpose
(`pairing.rs:39-41`) is "Stable per-host discriminator. Distinguishes two nodes an operator happened
to give confusingly similar names" — on Darwin it distinguishes nothing, and two macOS nodes
collide. Note this is the *same class* of bug the comment records having already fixed for Linux;
the fix was Linux-shaped and Darwin kept the pre-fix behaviour. That is the
"works-on-the-surface-we-test" shape the brief asked for.

Not a key-forgery issue — `key_id` carries security and the code says so. Severity judged after
the live measurement, not before.

**Still a PREDICTION at this point. Must be executed, not reasoned.** `NodeIdentity::local()` is
`pub`, so it is provable under the §0 single-crate exception.

## MEASUREMENT 3 (t+55m) — §0 exception used: macOS liveness arm executed in Rust, first time

`wcore_types::process_liveness` has a real `#[cfg(target_os = "macos")]` arm using
`sysctl KERN_PROC_PID` with **hardcoded ABI offsets** (`p_stat`=36, `p_pid`=40, `SZOMB`=5) taken
from `offsetof` on real hardware. Its recorded provenance is a **C** probe
(`.planning/evidence/zombie-probe/`), and the Rust arm had only ever been `cargo check`ed for
`aarch64-apple-darwin` — compiled, never executed. This is exactly what the §0 exception was
granted for.

**§0 exception invoked.** `cargo test -p wcore-types --test real_zombie`. Single crate, single test
file, bottom-layer crate with zero internal deps. No workspace build, no clippy, no release build.
Justification: the arm is `#[cfg(target_os = "macos")]` — hetzner cannot compile it, let alone run
it. Host: Darwin 25.3.0 arm64, macOS 26.3 (25D125), uid 501.

Result (`25m-real-zombie-darwin.txt`, unproxied via `/Users/seandonahoe/.cargo/bin/cargo`):

```
test result: ok. 4 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out
independent oracle for pid 83408: ps state=Z
```

4 executed, 0 ignored, 0 filtered — read back per §3.2, not inferred from exit status. The
`ps state=Z` line is an oracle independent of the code under test. **The hardcoded offsets are
still correct on macOS 26.3**, an OS several releases newer than the C probe's host; had Apple
moved them the `p_pid` readback self-check would have degraded to `Indeterminate` → "assume alive"
→ the corpse test would have gone red.

### Instrument finding: `rtk` rewrites `cargo` too

The first run of this command was rewritten by `rtk` to `cargo test: 4 passed (1 suite, 0.01s)`.
The count happened to be right, but the re-render **strips `0 ignored` and `0 filtered out`** —
precisely the two fields §3.2 requires to detect a suite that exits 0 having run nothing. So the
proxied form is structurally incapable of supporting the check the brief mandates. Every count in
this lane comes from `/Users/seandonahoe/.cargo/bin/cargo` invoked by absolute path. Adding
`cargo` to the §3b list of rewritten tools.

## MEASUREMENT 4 (t+70m) — ARM D closed: a Rust test that did not exist

The C probe recorded four arms. Three (live, zombie, reaped) have Rust tests. **ARM D did not**,
and ARM D is the one that fails in the dangerous direction:

> `ARM D: live, other user (launchd) pid=1 kill(pid,0)_says_alive=0 … sysctl.p_stat=2 -> LIVE`

Absence verified per §3b-i: `/usr/bin/grep -n "launchd\|EPERM\|other_user\|ARM D\|arm_d"
crates/wcore-types/tests/real_zombie.rs` → **rc=1, zero matches**, with a live positive control in
the same file (`grep -c zombie` → **4**). So the zero is a measurement, not a dead instrument.

Why it matters: a probe that reads a **live** process as **dead** makes an orphan reaper believe
it has nothing to clean up — a *false clean*. That is the C4-relevant direction ("no orphaned
execution"). And it is **unobservable on the Linux proof host**, which runs as root: `kill(1, 0)`
succeeds for root, so hetzner literally cannot demonstrate this arm. Darwin at uid 501 can.

Added `a_live_process_owned_by_another_user_reads_as_live_and_the_old_shape_called_it_dead` with
three assertions per §6b-ii — known-positive (pid 1 is Live), known-negative (a reaped pid is
still Dead, in the same test, so a probe answering Live to everything cannot pass), and **the old
shape would have missed it** (`kill(1,0)` fails EPERM at that same instant). It `assert_ne!`s on
euid 0 rather than skipping, so it cannot go green by being unobservable.

```
ARM D reproduced on Darwin: uid=501 pid=1 new_probe=Live old_shape(kill(1,0))=alive:false errno=1 (EPERM=1)
test result: ok. 5 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out
```

## MEASUREMENT 5 (t+80m) — mutation control: the new test CAN fail

Per §3.2 a gate is worthless until it has been seen to fail. The macOS arm was temporarily
replaced with the pre-repair `kill(pid,0)` shape and the suite re-run
(`25m-mutation-control-darwin.txt`):

```
test result: FAILED. 3 passed; 2 failed; 0 ignored; 0 measured; 0 filtered out
  a_live_process_owned_by_another_user_reads_as_live_and_the_old_shape_called_it_dead ... FAILED
  a_real_unreaped_corpse_reads_as_dead_and_the_old_shape_would_have_missed_it ... FAILED
```

Both directions go red without the sysctl arm — the corpse arm and ARM D. `process_liveness.rs`
was restored byte-identically afterwards (`git diff --stat` empty; `grep -c "MUTATION CONTROL"` →
0). The mutation lived only in the working tree and was never committed.

## Open / next

1. Execute the machine_id prediction (C2) — still unproven.
2. Decide C3 plugin-load Darwin relevance.
3. Say plainly which legs need Darwin and which prove nothing re-run.

Note: `Cargo.lock` regenerates on any cargo run (the committed lock predates `wcore-types`'
`libc`/`windows-sys` deps). Pre-existing drift, not this lane's; restored to HEAD and left
unstaged to avoid cross-lane conflict.

## MEASUREMENT 6 (t+100m) — C2 divergence EXECUTED on both sides

Darwin: `machine_id=unknown-host os=macos validates=yes key_id_differs=yes` — 19 passed; 0 failed;
0 ignored; 0 measured; 0 filtered out.
Linux (hetzner, non-login ssh): `machine_id=ubuntu-2404-noble-amd64-base` — 19 passed; 0 failed;
0 ignored; 0 measured; 0 filtered out.
Divergence is now executable on both sides, not measured on one and inferred on the other.

## MEASUREMENT 7 (t+110m) — C3 needs no Darwin run. Two candidate traps closed.

No `target_os = "macos"` anywhere in `crates/wcore-cli/src/plugin/`. Every platform branch is
`cfg(unix)`/`cfg(windows)` and every `cfg(unix)` body is plain POSIX identical on Darwin and Linux
(`set_permissions(0o600)` sign.rs:58; `PermissionsExt::mode()` carry generations.rs:432; `symlink`
generations.rs:585).

- **APFS case-insensitive vs ext4 case-sensitive** — REAL divergence, measured both sides
  (`Plugin.txt` resolves as `plugin.txt` on macOS; does not on hetzner). **Unreachable**:
  `validate_plugin_name` restricts to leading `[a-z]` then `[a-z0-9-]`, enforced at 9 call sites.
- **Gatekeeper / `com.apple.quarantine`** — does not arise. "quarantine" in this codebase is their
  term for the isolated git clone of a foreign plugin source. Checked, not assumed.
- **`ps -eo`** (the msys-rejected flag behind the Windows false zero) — works on Darwin: rc=0,
  852 rows; hetzner 1848 rows. Enumeration instrument is portable.

## MEASUREMENT 8 (t+115m) — report written

`25-MACOS.md` committed. Verdict: lane goal achieved; gap was mis-sized not mis-reported; one
MEDIUM divergence to BACKLOG; Phase 25 stays 3.5/4; binary-level macOS behaviour remains unproven
and belongs to Phase 28 (named, not worked around).
