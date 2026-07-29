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

## Open / next

1. Execute the machine_id prediction on Darwin via the §0 exception; execute the same on hetzner.
2. Check the C4 orphan/liveness leg (`wcore_types::process_liveness`) for Darwin divergence.
3. Check C3 plugin-load for a `.dylib`/quarantine path.
4. Say plainly which legs need Darwin and which prove nothing re-run.
