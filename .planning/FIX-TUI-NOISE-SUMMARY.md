# SUMMARY — lane/fix-tui-noise

**Base** `e7bc6d883027102ff1e5bbaa2dd19f9265268cab` · **branch** `lane/fix-tui-noise` ·
**pushed HEAD** see final report line · **build host** `hetzner-dsm`, worktree
`/root/wayland-fix-tui-noise` (branch `hz/fix-tui-noise`).

Full working record, every raw number, and all four instrument defects:
`.planning/evidence/fix-tui-noise/NOTES.md`.

---

## READ FIRST — two things the orchestrator must act on

**1. This lane edited `crates/wcore-cli/src/main.rs`, which my dispatch brief listed as FENCED
("other lanes own these, do not touch").** LANE-BRIEF §6 treats that file as a *shared* fence
permitting minimal edits, and says the LANE-BRIEF outranks an orchestrator instruction that
conflicts with it. The tracing subscriber and the entire `clap` help surface are constructible
*only* in that file, so a literal reading of the dispatch makes three of its five items
unfixable. I followed §6, kept the edits to text and one contiguous logic block, and am
flagging it rather than burying it. **Serialize this lane's `main.rs` merge.** Nothing was
renamed, reordered or reformatted; no registration was touched.

**2. Finding F7 of UAT-TUI-UNIX — "Linux TUI takes 11 s to first paint; macOS takes 2 s" — is
REFUTED as a platform finding, and the real defect is worse and lives elsewhere.** Details
below. It is diagnosed here and deliberately **not fixed** here.

---

## What landed

| # | item | before | after |
|---|---|---|---|
| 1 | headless startup spew | **41 stderr lines** (20 INFO + 19 WARN) | **2** (0 INFO / 0 WARN / 0 ERROR) |
| 3 | answer runs into a log line | stdout had **no terminator** | ends `0a` |
| 5 | headless stdout shape | **5 bytes** `2a 20 33 39 31` = `* 391` | **4 bytes** `33 39 31 0a` = `391\n` |
| 4 | `--help` internal ids | **26 lines** (`--help`), 25 (`-h`), 1 (`session --help`) | **0 / 0 / 0** |
| 2 | first paint | diagnosed, see below | **not fixed — handed off** |

Same binary, same host, same lane home, same prompt, **same empty working directory** on both
sides, so the before/after is not confounded by the cwd effect found in item 2. Real Anthropic
provider; `391` present in stdout on every run — a refused turn boots less and would have
flattered every number.

### 1 + 3 — quiet by default, and MORE diagnosable than before

`crates/wcore-cli/src/main.rs`. TUI mode already routed traces to
`$WAYLAND_HOME/logs/wayland-core.log`; every other mode still printed them to the terminal.
Now: **`RUST_LOG` is unchanged and authoritative when set.** When it is unset, INFO+ goes to
that same log file and only ERROR reaches stderr.

No new flag was invented — the brief asked me to check first, and `RUST_LOG` already existed,
already worked, and is already the documented lever.

**Both directions proven in one run** (§3b-iii: "the logs are gone" is trivially satisfiable by
breaking logging):

| `RUST_LOG` | stderr lines | stderr INFO/WARN | log file |
|---|---|---|---|
| unset (default) | **2** | 0 / 0 | **39 lines, 20 INFO + 19 WARN**, contains `spotify_playback` |
| `info` | **41** | **20 / 19** | not written — previous behaviour, byte for byte |

`RUST_LOG=info wayland-core --no-tui '<prompt>'` is the exact invocation that restores full
logging, and it is measured, not asserted. Headless runs previously kept **no** log at all, so
the record died with the scrollback; it now survives on disk without anyone asking.

### 5 — one-shot output is machine-consumable

`crates/wcore-agent/src/output/terminal.rs`. A prompt on argv means one answer then exit, so the
sink suppresses the §3.2 speaker marker (`⏺ ` / `* `) and terminates stdout with a newline. The
interactive REPL and the TUI keep the marker, where it actually separates turns.

The Windows UAT called this a "bullet prefix"; it is right about the effect and wrong about the
cause. `* ` is the **documented ASCII fallback** of a designed turn marker, not a stray artifact
— which is why it was not simply stripped.

### 4 — `--help` speaks English

26 doc comments rewritten in `main.rs`, including **12 of the top-level subcommand one-liners**
— the first screen a new user sees. `F23_SESSION=` / `F23_INDEX=` / `F23_CACHE=` are kept: those
are tokens the product really prints, and documenting them is correct. The underscore is what
separates a live contract from a sprint id.

New test `crates/wcore-cli/tests/help_no_internal_ids.rs`, **proven able to fail**:

| state | rc | counts |
|---|---|---|
| clean | 0 | 5 passed; 0 failed; 0 ignored; 0 filtered out |
| one seeded id (`/// F-089: Disable colored output`) | **101** | 3 passed; **2 failed** — ``` `--help` leaks 1 internal identifier(s) at users ``` |
| reverted | 0 | 5 passed; 0 failed |

It also ships both matcher controls: the pattern must fire on all 14 strings the UAT found (so
it cannot go inert) and must not fire on ordinary help prose (so it cannot be widened into
unpassability).

---

## Item 2 — the 11 seconds, diagnosed and handed off

**The 11 s is real and reproduces exactly. The platform framing does not survive measurement.**

Same binary, same Linux host, same minute, only the working directory changed:

| cwd | headless wall | TUI: splash | TUI: usable surface | largest single gap in the boot log |
|---|---|---|---|---|
| empty directory | 2.38 / 1.72 / 2.79 s | 0.13 s | **0.36 / 0.36 / 0.36 s** | **0.013 s** |
| `/root` (~2.5 M entries) | 12.02 / 11.80 / 11.67 s | 0.13 s | **10.63 / 10.53 / 10.63 s** | **10.26 / 10.17 / 10.19 s** |

The UAT's "Linux 11 s" and "macOS 2 s" are these two rows. **The same Linux binary in an empty
directory reaches a usable surface in 0.36 s — 5.5× faster than the macOS number the UAT used as
its good baseline.** Measured at 100 ms, not the UAT's 1 s, because 1 s buckets cannot tell one
9 s stall from ninety 100 ms ones — and it is one stall.

**What the stall is.** `RUST_LOG=debug` emits *zero* lines inside the gap. `strace -f -tt -T`
over the full turn: 2,587,703 syscalls, **0 of them slower than 0.5 s** (extractor proven alive
on 2,567,175 matched lines), dominated by `getdents64` 938,838 · `openat` 472,101 · `fstat`
468,839 · `readlink` 195,367. It is **two full recursive walks of the current working
directory**, plus **3,649 `inotify_add_watch`es** — 938 of them inside another lane's worktree.

**Which walkers.** Separated without a rebuild, using a probe git repo whose `.gitignore` hides
20,000 files (control: `git status --untracked-files=all` sees 0 of them):

| walker | opened the ignored dir? | opened `.git/`? | ⇒ |
|---|---|---|---|
| first | no | no | `wcore_repomap::scope::scope_files` (`scope.rs:209`, `standard_filters(true)`, `.git` excluded by name) |
| second | **yes** | **yes**, all 8 subdirs | `wcore_tools::workspace_policy::project_committed_secrets` (`workspace_policy.rs:837`, `standard_filters(false)`, **no prune**) |

Both are reached from `bootstrap.rs:2871`. Cost is linear in non-ignored entry count, so a normal
repo is fine (20 k gitignored files → 1.82 s; the same 20 k un-ignored → 2.86 s) and a home
directory full of `target/` and `node_modules/` is not.
`--dangerously-skip-permissions-and-sandbox` does **not** remove it (11.73 s): the policy is
still constructed even when the sandbox is bypassed at exec time.

**Why I did not fix it.** The un-pruned walk is deliberate and documented in the source: pruning
`node_modules`/`target` *"would deny a committed secret to Read/Edit/Grep while leaving it
READABLE via `Bash cat`"*, with a prior DoS (#234) already mitigated by a lexical prefilter on
the *canonicalize* rather than the *readdir*. Prune, cap, cache-across-the-two-passes and
`build_parallel()` are all plausible, and **all four change the sandbox secret-deny surface** —
not a TUI-noise lane's call on its own evidence. **Handed off with the numbers rather than
guessed at.**

---

## Gates — hetzner, at the fix commit, status read from a file, never from an ssh exit code

```
WLFMT=0        cargo fmt --all -- --check
WLMETA=0       cargo metadata --locked
WLCHECK=0      cargo check --workspace --all-targets
WLCLIPPYC=0    cargo clippy -p wcore-cli   --all-targets -- -D warnings
WLCLIPPYA=101  cargo clippy -p wcore-agent --all-targets -- -D warnings   <- PRE-EXISTING (proven)
WLCLIPPY_AGENT_LIB=0   cargo clippy -p wcore-agent --lib -- -D warnings   (the target I changed)
```

| suite | passed | failed | ignored | filtered out |
|---|---|---|---|---|
| `-p wcore-cli --test help_no_internal_ids` | 5 | 0 | 0 | 0 |
| `-p wcore-cli --lib` | **1917** | 0 | 1 | 0 |
| `-p wcore-agent --lib` (96 threads, contended) | 2232 | 20 | 3 | 0 |
| `-p wcore-agent --lib -- --test-threads=8` | **2252** | **0** | 3 | 0 |
| `…--lib engine::audit_2026_05_22_tests -- --test-threads=1` | **77** | **0** | 0 | 2178 |
| `-p wcore-cli --test f14_sigkill_recovery` (solo) | 10 | **1** | 1 | 0 |

### Both reds are PRE-EXISTING at base, proven rather than argued

* **`clippy -p wcore-agent --all-targets`** — 4 × `needless_borrow` in
  `tests/user_model_identity_wire.rs`, a file **byte-identical to base**, last touched
  2026-07-29 by `39b69fe2` (another lane). With my only `wcore-agent` change reverted so the
  crate is byte-identical to base, clippy **still exits 101 with the same 4 errors**.
  **Integration is currently red on this gate.**
* **`f14_sigkill_recovery::isolated_profile_without_secure_store_fails_before_turn_or_provider_intent`**
  — `ready` frame carries no `session_id`. Fails identically under `RUST_LOG=info` (which
  disables the new file-sink path entirely), and fails with **both** my source files reverted and
  **both crates recompiled** — the rebuild was verified in the log (`Compiling wcore-agent`,
  `Compiling wcore-cli`, `Finished in 14.38s`), because a probe that silently skipped the rebuild
  would have measured my own binary and proved nothing. **Integration is currently red here too.**
* The 20 `wcore-agent` failures are all `session journal writer lease is already held at /tmp/…`
  in one module; that module alone is 77/77 and the whole lib at 8 threads is 2252/0. Load, not
  regression — the contention class LANE-BRIEF §6 names.

---

## Cross-audit — the one real judgement call

*File sink for default INFO, or just lower stderr to WARN/ERROR with no file?*

codex `gpt-5.6-sol` **KEEP_FILE** · gemini `3.1-pro-preview` **DROP FILE** (12-factor; unmanaged
disk growth; blinds CI scrapers) · kimi K3 **KEEP FILE but send WARN to stderr too**. Majority
2–1; I hold it, and record both dissents.

**kimi's amendment is refuted by measurement** — in this product the 19 WARN lines *are* the
noise (`tts: no TTS backend configured`, `ffmpeg not found`, `no API key found`, `semantic search
degraded`). Sending WARN to stderr takes the default from 2 lines back to **21**, which is still
the defect the lane was opened for. Those lines carry the wrong level for what they say, but
re-levelling them spans crates several lanes own — follow-up, not smuggled in here.

**gemini's objection is partly correct and is a defect I introduced:** the log file has **no
rotation**, and this change extends unbounded append from TUI runs only to *every* headless run.
Measured: **7,447 bytes per trivial turn.** Pre-existing for the TUI path; the growth rate is now
materially higher.

---

## Follow-ups this lane did NOT take

1. **Bound the boot-time workspace walk** — the whole of item 2. Security-surface decision in
   `wcore-tools` / `wcore-repomap`; evidence above.
2. **Log rotation / size cap** on `$WAYLAND_HOME/logs/wayland-core.log`; 7,447 bytes per run.
3. **Re-level the capability advisories** from WARN to DEBUG/INFO — 19 lines that describe a
   static capability inventory, not a warning about the run. Spans crates other lanes own.
4. **Two pre-existing integration reds** (clippy `wcore-agent --all-targets`; `f14_sigkill_recovery`).
5. `--doctor`'s Linux/macOS grading split, and the "3 of ~10 providers" API-key error
   (UAT-TUI-UNIX F6/F9) — same family, different lanes' briefs.

## The non-one-shot path was live-checked, not reasoned about

`printf 'What is 17 times 23?…\n/quit\n' | wayland-core --no-tui` (REPL: no prompt on argv):
stdout is `\n> * 391\n> ` — **the `* ` marker is still there**, the prompt is intact, `391` is
correct, stderr is 2 lines. So the gate is exactly `!cli.prompt.is_empty()` and nothing else
changed. rc=0.

## Coverage — what was NOT run

**Five unrun cells, counted, not implied:**

1. **Windows.** UAT-TUI-WINDOWS F3/F4/F5 were **not re-measured**; every number here is Linux.
   `SeanD@seandesktop` was reachable in principle and simply not used — no Windows binary was
   built at this commit. **This lane claims no Windows coverage.**
2. **macOS.** No permitted host runs macOS and this change needs a full workspace build, so the
   LANE-BRIEF §0 Darwin carve-out (single crate, single test) does not apply. **The UAT's macOS
   "2 s" was never independently re-measured** — the refutation of F7 rests on the Linux
   empty-cwd figure (0.36 s), which is sufficient because it is *faster* than the macOS number
   being used as the good baseline.
3. **`--json-stream` under the new sink** — exercised only indirectly, via `f14_sigkill_recovery`
   (10/1 both before and after, i.e. unchanged), not driven directly.
4. **`cargo test -p wcore-agent --tests`** (integration targets) — not run; only `--lib`.
5. **Behaviour when the log file cannot be opened** (no `$HOME`/`$WAYLAND_HOME`) — the code falls
   back to the previous stderr-at-INFO path, which is the existing documented degradation, but it
   was not exercised.

## Instrument defects found in my own harness — all four repaired in-lane

1. Unquoted `--include=*.rs`; zsh reported `no matches found`, which reads as "zero hits".
2. **`-p` is `--provider`, not "prompt".** The first baseline died in argv parsing and reported
   `INFO_LINES=0 / WARN_LINES=0` — **a perfect "no spew" result produced by never starting the
   engine.** Repaired both ways: corrected argv *and* a hard `ASSERT_TURN` that reddens the
   harness on any non-zero child exit, so the class cannot recur silently.
3. The attribution probe counted opens of *files inside* the ignored directory — but a walk
   `getdents64`s a directory, it does not `openat` each file, so the count was structurally 0 and
   would have "proved" both walkers respected gitignore. Repaired to count `O_DIRECTORY` opens
   per thread id, which is what separated the two walkers.
4. The gate runner's `grep '^test result:'` produced a **FALSE RED**: `wcore-cli` scaffolds a
   throwaway crate containing `fn always_fails() { panic!("deliberate") }` and runs a **nested**
   `cargo test` to prove failing suites propagate; the nested `FAILED. 0 passed; 1 failed` landed
   in the same log and made an outer 1917/0 read as broken. Repaired to key result lines to the
   `Running <target>` line above them.

Two of the four would have put a wrong number in this report rather than raising a visible error.

## Secret handling

The box's own `ANTHROPIC_API_KEY` was read from `/root/.wayland/.env` **inside a hetzner-side
script**, exported into the child's environment only, and never placed in argv, printed, written
to an evidence file, or transmitted to the operator. Only its length (108) was recorded. Per
LANE-BRIEF §3b-ii the provider arm was read back out of the product's own output
(`INFO vision: using Anthropic (ANTHROPIC_API_KEY found)`) rather than inferred from the
environment.
