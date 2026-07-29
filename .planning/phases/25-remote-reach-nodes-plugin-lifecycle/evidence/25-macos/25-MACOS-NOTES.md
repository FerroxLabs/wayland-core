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

## Open / next

1. Read the four criteria from the PLAN files, not the summaries' paraphrase.
2. Confirm or revise the Darwin-specificity ranking against that text.
3. Run the Darwin legs; run anything hetzner could prove on hetzner.
