# NOTES — lane/fix-tui-noise

Base: `e7bc6d883027102ff1e5bbaa2dd19f9265268cab` (integration `plan/f20-unified-audit-repair`).
Mac worktree: `/Users/seandonahoe/dev/waylandcore-frontier-worktrees/lane-fix-tui-noise`.
hetzner worktree: `/root/wayland-fix-tui-noise`, branch `hz/fix-tui-noise`, asserted at the
same SHA after `worktree add` (`git rev-parse HEAD` = `e7bc6d88…`).
Build log `/root/wayland-fix-tui-noise-build.log`. `df -h /root` = 993G free at start.

Scope (5 items from UAT-TUI-UNIX F4/F5/F7/F8 and UAT-TUI-WINDOWS F3/F4/F5):

1. startup log spew on headless turns (Linux/macOS 32–68 INFO; Windows 29 INFO + 5 WARN)
2. Linux TUI 11s to first paint vs macOS 2s
3. answer concatenated with a log line
4. `--help` 216 lines leaking internal ticket IDs
5. headless stdout: no trailing newline, and a `* ` prefix survives `--no-tui` + `NO_COLOR=1`

## T+0 — source recon at base SHA (derivation, NOT yet measurement)

All greps run with `/usr/bin/grep`, globs quoted (zsh ate an unquoted `--include=*.rs` on the
first attempt and the shell reported `no matches found` — instrument defect #1, repaired by
quoting; the repaired form returned 217 hits, so the instrument is proven alive).

### (1) + (3) log spew — subscriber lives in `crates/wcore-cli/src/main.rs:1156-1200`

```rust
let will_enter_tui = prompt_guess.is_empty() && !cli.no_tui && tui_capable && !cli.json_stream;
let tui_log_file = if will_enter_tui { open_tui_log_file().ok() } else { None };
let env_filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info"));
```

So a prior fix (v0.9.1 W2 cycle-2 HIGH 2) already routes **TUI-mode** traces to
`$WAYLAND_HOME/logs/wayland-core.log`. Everything else — `-p`, `--no-tui`, `--json-stream`,
piped stdout — keeps INFO on **stderr**. That is exactly the population all three UAT lanes hit.

The default level is the literal `"info"` on line 1176, reached only when `RUST_LOG` is unset
or unparseable. `RUST_LOG` therefore already exists as the escape hatch; **no new flag is
needed** and none should be invented.

Predicted minimal fix: change the *fallback* (not the `RUST_LOG` path) to a quiet level, and
keep the full record reachable. Both directions must be proven.

`main.rs` is on the LANE-BRIEF §6 shared fence AND my dispatch brief lists it as owned by
another lane. §6 permits *minimal additive edits in one contiguous block*; the dispatch says
do not touch. The subscriber is only constructible there, so a literal reading of the dispatch
makes items (1) and (3) unfixable. Resolution: follow §6 (LANE-BRIEF outranks the dispatch by
its own terms), keep the edit to the smallest possible diff, and disclose it at the top of the
summary as a fence edit the orchestrator must serialize.

### (4) `--help` ticket IDs — clap doc comments on `struct Cli`, `main.rs:261-600`

Confirmed present in source at base, in `///` doc comments that clap renders verbatim:
`W5 (A.5)`, `A4b`, `W4 F19`, `F-072`, `W9.1 T4 (T11)`, `23A-C1` (a nine-line paragraph
including *"an advertised dead surface is the most-repeated defect class on this program"*),
`M3.4`, `M5.2`, `M5.4`, `F-092 (W7-N)`, `#111`, `contract §5`.
Workspace-wide grep for the ticket-ID set: **217 lines** across `crates/` — most are internal
comments and out of scope; only doc comments on `Cli`/`TopCmd` reach `--help`.

### (5) headless `* ` prefix — `crates/wcore-agent/src/output/mod.rs:497`

Not a stray artifact. It is the documented ASCII fallback of the "assistant turn marker"
(spec §3.2): `⏺ ` in colour mode, `* ` otherwise, deliberately on **stdout** because it leads
the body text. So the Windows UAT's framing ("bullet prefix") is right about the effect and
wrong about the cause — this is a designed marker, and removing it unconditionally would be a
product regression for interactive `--no-tui` REPL use. The scriptable-output complaint is
real for one-shot `-p` though. Needs a measurement + a decision, not a blind strip.
`crates/wcore-agent/src/output/` is NOT fenced, so this one is mine outright.

## Premise claims to re-measure before acting (brief says assume stale)

| # | claim | source | status |
|---|---|---|---|
| C1 | 32–68 INFO lines per headless turn | UAT-TUI-UNIX F4 | UNMEASURED |
| C2 | 29 INFO + 5 WARN on Windows | UAT-TUI-WINDOWS F3 | UNMEASURED |
| C3 | Linux 11s / macOS 2s to first paint | UAT-TUI-UNIX F7 | UNMEASURED |
| C4 | answer + log concatenated on one line | UAT-TUI-UNIX F8 | UNMEASURED |
| C5 | `--help` 215/216 lines, N internal IDs | UAT-TUI-UNIX F5 | source CONFIRMED, count UNMEASURED |
| C6 | headless stdout is 5 bytes `* 391`, no `\n` | UAT-TUI-WINDOWS F4/F5 | source CONFIRMED, byte count UNMEASURED |

## Plan

1. Build release on hetzner at base; assert `--build-info` = `e7bc6d88`.
2. Measure C1–C6 on the real binary, output redirected to files and read with the Read tool
   (LANE-BRIEF §3b: `rtk` fabricates machine-readable counts).
3. Diagnose C3 with evidence before touching it. Do not fix an unexplained latency.
4. Fix, then re-measure the same way, plus a both-directions proof that `RUST_LOG` restores
   everything.
5. Add a regression test that fails if an internal identifier reappears in help output, and
   prove it can fail by seeding one.

## Instrument defects found in my own harness (repaired in-lane, per §6b-ii)

1. Unquoted `--include=*.rs` — zsh glob expansion killed two greps with `no matches found`
   (exit 1), which a careless reader grades as "zero hits". Repaired: every glob quoted, and
   every absence claim now carries a known-positive in the same capture.
