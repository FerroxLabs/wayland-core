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

## T+1h — BASELINE MEASURED on the real binary

Binary `/root/wayland-fix-tui-noise/target/release/wayland-core`, sha256
`f2078c290cffb053bfd27dbe6133e9e35a75eab157e7b669fa657cc7469a26e5`,
`--build-info` self-reports source **`e7bc6d88…`** = the lane base. Provenance measured.

Turn: `wayland-core --no-tui 'What is 17 times 23? Reply with just the number.'`, real
Anthropic provider, answer `391` present (`ANSWER_HAS_391=1`) — a refused turn boots less and
would have flattered every number below.

| claim | UAT said | measured at base | verdict |
|---|---|---|---|
| C1 headless log lines | 32–68 | **42 stderr lines = 20 INFO + 19 WARN + 3 other** | CONFIRMED |
| C2 Windows 29 INFO + 5 WARN | — | Linux figure above; Windows not re-run | UNRUN (see coverage) |
| C4 answer + log on one line | yes | stdout has **no trailing newline**, so the next write lands on the same line | CONFIRMED, cause identified |
| C5 `--help` size + ids | 215 / 216 lines | **`--help` 215 lines, `-h` 142** and **26 lines carrying internal ids** | CONFIRMED, and worse than reported |
| C6 stdout `* 391`, 5 bytes | 5 bytes | **5 bytes, `2a 20 33 39 31`**, last byte `31` not `0a` | CONFIRMED byte-exact |
| C3 Linux 11s vs macOS 2s | platform gap | **REFUTED as a platform gap — see below** | REFUTED |

## T+1h30 — C3 DIAGNOSED, and the brief's framing is wrong

**The 11 seconds is real. "Linux is 5x slower than macOS" is not.**

Same binary, same host, same platform, same minute, only the working directory changed:

| cwd | run 1 | run 2 | run 3 |
|---|---|---|---|
| empty directory | **2.38 s** | **1.72 s** | **2.79 s** |
| `/root` (40 build worktrees) | **12.02 s** | **11.80 s** | **11.67 s** |

The UAT's "macOS 2s" and "Linux 11s" are these two numbers. Linux in an empty directory is at
or below the macOS figure. The variable is **the size of the working-directory tree**, not the
operating system.

### What the 9.8 s is

Log-timestamp gap analysis on the baseline stderr: one dominant gap of **9.800 s** between
`INFO user-model: using local backend` and the second tool-registration pass. Everything else
is sub-millisecond except the provider round trip (1.898 s). `RUST_LOG=debug` does not fill the
gap — **zero** trace lines are emitted inside it.

`strace -f -tt -T` over the whole turn, 2,587,703 syscalls:

```
938838 getdents64      472101 openat      468839 fstat      468806 close      195367 readlink
     0 syscalls slower than 0.5 s        (extractor proven alive: 2,567,175 / 2,587,703 lines matched)
```

So it is not a blocked syscall or a network timeout. It is **two full recursive walks of the
current working directory**, one per boot pass, each on the main thread. Top prefixes:
`/root/rambuild/flux-litellm-…` 226,455 opens, `…/node_modules` 20,133, `/root/.cargo/registry`
13,417, `/root/wayland/target` 10,473. The walk also plants **3,649 `inotify_add_watch`es**,
including 891 inside this lane's own worktree and 938 inside `/root/wayland-25c4/crates` — i.e.
other lanes' trees.

### Which walkers — separated without a rebuild

The two candidate walkers differ observably, so a purpose-built probe repo separates them:
a git repo whose `.gitignore` hides a 20,000-file directory, run as cwd under
`strace -e trace=openat`. `git status --untracked-files=all` sees 0 of the ignored files, so the
ignore file is genuinely in force (control).

| walker thread | opened `ignored_big/`? | opened `.git/`? | ⇒ |
|---|---|---|---|
| 3349564 (first) | **no** | **no** | `standard_filters(true)` + `.git` excluded by name ⇒ `wcore_repomap::scope::scope_files` (`scope.rs:209-213`) |
| 3349666 (second) | **yes** | **yes**, all 8 subdirs | `standard_filters(false)`, no prune ⇒ `wcore_tools::workspace_policy::project_committed_secrets` (`workspace_policy.rs:837-841`) |

Both are reached from boot: `bootstrap.rs:2871` builds `WorkspacePolicy::contained(&workspace)`
for any non-trusted workspace, and the secret-deny walk is documented as deliberately
un-pruned — *"NO directory prune … pruning `node_modules`/`target`/`.wcache` would deny a
committed secret to Read/Edit/Grep while leaving it READABLE via `Bash cat`"* — with issue #234
named as a prior DoS that was mitigated by a lexical prefilter on the *canonicalize*, not on the
*readdir*.

Cost is linear in non-ignored entry count, and a normal repo is fine:

| cwd | wall |
|---|---|
| empty | 1.72–2.79 s |
| probe repo, 20k files gitignored | **1.82 s** |
| probe repo, same 20k files NOT gitignored | **2.86 s** |
| probe repo, not a git repo at all | 2.31 s |
| `/root`, ~2.5M entries | 11.67–12.02 s |

`--dangerously-skip-permissions-and-sandbox` does **not** remove it (11.73 s) — the policy is
still constructed even when the sandbox is bypassed at exec time.

### Verdict on C3 — diagnosed here, NOT fixed here

The remedy is a bounded-workspace-scan decision inside `wcore-tools` /`wcore-repomap`: either
prune, cap, cache across the two passes, or parallelise `WalkBuilder`. All four change a
**security-relevant** surface (the sandbox secret-deny list) whose current shape is a documented,
deliberate tradeoff carrying two issue numbers. That is not a TUI-noise lane's call to make on
its own evidence. Handed off with the measurements above rather than guessed at.

## Instrument defects found in my own harness (repaired in-lane, per §6b-ii)

1. **Unquoted `--include=*.rs`** — zsh glob expansion killed two greps with `no matches found`
   (exit 1), which a careless reader grades as "zero hits". Repaired: every glob quoted, and
   every absence claim now carries a known-positive in the same capture.
2. **`-p` is `--provider`, not "prompt"** (`main.rs:269`; the prompt is a trailing positional).
   The first baseline run therefore died in argv parsing and reported `INFO_LINES=0`,
   `WARN_LINES=0` — **a perfect "no spew" result produced by never starting the engine.** Exactly
   the self-passing shape §3b-i describes. Repaired two ways: the argv is corrected, *and* a hard
   `ASSERT_TURN` now reddens the harness (`WLRC=96`) on any non-zero child exit, so the class
   cannot recur silently even if a future invocation is wrong for a different reason.
3. **The attribution probe counted opens of files inside the ignored directory** — but a
   directory walk `getdents64`s a directory, it does not `openat` each file, so the count was
   structurally 0 and would have "proved" gitignore was respected by *both* walkers. Repaired to
   count `O_DIRECTORY` opens of the directory itself and to attribute them per thread id, which
   is what separated the two walkers above.
