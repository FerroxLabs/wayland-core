# macOS legs — 27-C1, 24-C2, 27-C2(c)

Unit `macos-legs`. Branch `lane/macos-legs`, base `0675c051` on
`plan/f20-unified-audit-repair`. **Every figure below was measured on this Mac**
(`Darwin 25.3.0 arm64`, macOS 26.3 build 25D125, `rustc 1.95.0`), against a macOS
build of the tree at that SHA. Raw captures: `.planning/evidence/macos-legs/`.

Three rows were open because nobody had run them on real macOS hardware. They have
now been run. **Two closed, one produced a HIGH that Linux cannot see.**

> **The premise that this Mac cannot compile Rust is FALSE.** `AGENTS.md`'s inherited
> handoff says *"Compiles ONLY on Hetzner … NEVER the Mac (`cargo fmt` DOES work on
> Mac)."* Measured: `cargo 1.95.0` at `~/.cargo/bin/cargo`, Xcode CLT present,
> `cargo build -p wcore-cli --bin wayland-core` **succeeded**, as did every test
> target used below. Every macOS figure in this file came from a locally built
> artifact. That line should be struck; it is why these legs sat unrun.

| Leg | Row | Verdict |
|---|---|---|
| M1 | `27-C1` | **PTY drive taken (first on any platform); macOS artifact produced — and it is RED.** A macOS-only HIGH: the consolidated media-intake chokepoint refuses every path under the platform's own `$TMPDIR`. |
| M2 | `24-C2` | **All three absent legs now measured and PASSING on macOS** — macOS evidence, the PTY surface gate, and the kill-mid-fire continuation run with a no-restart control. |
| M3 | `27-C2(c)` | Baselines 1 and 2 pass on macOS. **Baseline 3 did not exist on macOS and reported `ok. 0 passed` while not existing** — ported to `ps` and now genuinely measured. Baseline 2's real-desktop half remains NOT MEASURED on macOS. |

---

## 1. M1 / 27-C1 — the PTY drive, and what it found

The ledger row: *"The TUI half was never exercised (no PTY drive) and macOS still
has no artifact."* Both are now false, and the artifact is a defect.

### 1a. The source gate re-run on macOS

`.planning/scripts/f27-c1-one-path-gate.sh` → **GATE: PASS**
(`raw-m1-onepath-gate-macos.txt`). All four checks green under BSD `grep`/`awk`,
instrument liveness `38`. This is a source gate, so a macOS pass is expected — it is
recorded because the gate had never been executed with a BSD toolchain.

### 1b. The live intake drive — the KNOWN-POSITIVE arm failed

`f27-c1-intake-live.sh` against the shipped binary and the recording mock provider
(`raw-m1-intake-live-macos.log`). The arm that must succeed did not:

| arm | macOS (`raw-m1-intake-live-macos.log`) | Linux, same script (`27-c1/LIVE-OBS-RAW-merged.log`) |
|---|---|---|
| **valid** — a real WAV under `$TMPDIR` | `is_error=true` — `Cannot open audio path component in …/good.wav: Not a directory (os error 20)` | `is_error=true` — `groq transcription returned HTTP 401: Invalid API Key` |
| denylisted (`.ssh/id_rsa`) | `path targets a denied system location` | same class |
| traversal (`sub/../good.wav`) | `path contains traversal (..)` | same class |
| symlink | **`Not a directory (os error 20)`** | refused by the symlink check |
| over-cap (75 MB) | **`Not a directory (os error 20)`** | refused by the cap |
| relative | `path must be absolute` | same class |

The Linux "valid" line is what success looks like: intake **opened the file, read the
bytes and uploaded them**, and Groq rejected the fixture key. macOS never gets that
far.

**Two of the negative arms are therefore VACUOUS on macOS.** `symlink` and
`over-cap` are recorded as refusals, but they were refused by the component walk
before the symlink check or the byte cap was ever reached. A reader comparing the two
platforms' "all arms refused" columns would conclude the gates agree. They do not.

### 1c. The PTY drive of the TUI — the discrimination

`macos-legs-m1-pty.py` starts the **real TUI on a real controlling terminal**
(44×120), points it at the mock, and has the mock answer turn 1 with a
`transcribe_audio` tool call, so the chokepoint runs inside a terminal session.
Rendered screens: `pty-tui-intake-home.txt`, `pty-tui-intake-tmpdir.txt`. Two arms,
**identical in every respect except the directory the fixture sits in**:

| arm | `audio_path` | tool_result the engine put back on the wire |
|---|---|---|
| `home` | `$HOME/.wl-m1-pty/good.wav` | `groq transcription returned HTTP 401: Invalid API Key` — **intake succeeded** |
| `tmpdir` | `$TMPDIR/…/good.wav` | `Cannot open audio path component …: Not a directory (os error 20)` |

Same binary, same TUI, same bytes, `MOCK_REQUESTS_CAPTURED=2` on both (so neither arm
is void for want of reaching the provider). The `home` arm is the known-positive that
makes the `tmpdir` arm mean something.

**A second thing the rendered screen shows, which no wire capture could.** On BOTH
arms the final screen is:

```
   ▌ transcribe it

     F27-MOCK-REPLY
```

No tool block, no result, and on the `tmpdir` arm **no sign that the transcription
failed at all**. The raw PTY bytes confirm a transient `…ing transcribe_audio)` was
painted at row 6 and then cleared, so the TUI does start a tool indicator and drops
it. The JSON-stream surface renders its intake refusals verbatim
(`composer attachment rejected: … extension declares image/jpeg but bytes are
image/png`); the terminal surface, on this run, rendered nothing. I am recording this
as an **observation, not a new finding** — `.planning/evidence/fix-tui-tool-results/`
shows TUI tool-result rendering is already an owned area, and one mock-driven turn is
not enough to characterise it. It is the kind of thing only a PTY drive can see, which
is the argument for having taken one.

### 1d. F-M1-01 (HIGH, macOS only) — root cause, measured at the syscall

`media_intake::open_once` (`crates/wcore-tools/src/media_intake.rs:440-450`) walks the
path from `/`, opening every **intermediate** component with
`O_RDONLY | O_DIRECTORY | O_NOFOLLOW | O_CLOEXEC`.

`macos-legs-m1-symlink-probe.py` replays that exact `openat` sequence outside the
product (`raw-m1-symlink-probe-macos.log`):

```
/tmp   islink=True  -> private/tmp
/var   islink=True  -> private/var
/etc   islink=True  -> private/etc
/Users islink=False

TMPDIR as handed out   /var/folders/…/T/…/good.wav   walk_ok=False failed_at=var  errno=ENOTDIR
TMPDIR via realpath    /private/var/folders/…        walk_ok=True
$HOME                  /Users/…/good.wav             walk_ok=True
literal /tmp           /tmp/…                        walk_ok=False failed_at=tmp  errno=ENOTDIR
```

On macOS `/tmp`, `/var` and `/etc` are OS-provided symlinks into `/private`, and
`$TMPDIR` is **always** under `/var/folders/...`. An `O_NOFOLLOW` component walk
therefore refuses every path the platform's own temp APIs hand out. Linux has no such
top-level symlinks, which is exactly why nine months of Linux runs never saw it.

**Blast radius.** `open_once` is the single chokepoint for all six consolidated media
surfaces (`vision_tools`, `transcription_tools`, `pdf_tool`, `doc_tool`,
`wcore-cli/attachments`, `wcore-agent/channel_media`). On macOS, every one of them
refuses `/tmp/...`, `/var/...`, `$TMPDIR/...` — the ordinary destination for anything
a script, a hook, a downloaded artefact or an agent-authored file lands in. `~/Downloads`
works. The user-facing text is `Not a directory (os error 20)` against a path that is
plainly a file.

**Not fixed here.** This is a real design question, not a typo: the walk refuses
symlinked components *on purpose*, to defeat a raced parent rename. Any repair has to
decide whether resolving `/var -> /private/var` once, up front, weakens that guarantee,
and that decision belongs to the chokepoint's owner. Reported with a reproducer.

### 1e. Observation, not a claim — the TUI's own `@file` path

`crates/wcore-cli/src/tui/commands/at_ref_resolve.rs:203`, `:328` and
`at_ref_send.rs:396` read **caller-named files** with `fs::read_to_string`, outside the
chokepoint, and none of those files is in the one-path gate's `SURFACES` list. It is a
text-context inclusion path bounded by a token budget and `DIR_MAX_FILES`, not a media
intake path, so I do **not** count it as a ninth media surface. Flagged for the row
owner because the gate's surface list has never been audited against the TUI.

---

## 2. M2 / 24-C2 — macOS evidence, the PTY surface gate, the kill-mid-fire run

`macos-legs-m2-cron.py`. Raw: `raw-m2-cron-macos.log`,
`raw-m2-cron-partC-macos.log`. All three legs the ledger lists as absent are now
measured, and all three pass.

### 2a. macOS evidence — the hetzner record reproduces exactly

```
cron add --trigger webhook:/hooks/build --slash /brief   → refusal text, EXIT=1
cron add --trigger poll:https://…:300  --slash /brief    → refusal text, EXIT=1
cron list                                                 → (no cron jobs)      NOTHING_PERSISTED=True
cron add --trigger event:build.finished --channel team    → added <uuid>, EXIT=0
cron publish build.finished                               → QUEUED_EVENTS_ON_DISK=1
cron daemon                                               → pid …, ALIVE=True
cron history <id>                                         → 2026-07-31T00:49:50Z  staged (no live dispatcher)
PART-A-SUMMARY: … history_before=0 history_after=1 queue_after=0
```

Same refusal strings, same `staged (no live dispatcher)` outcome, same queue drain as
`24-C2-LIVE-EVIDENCE.md` recorded on `hetzner-dsm`. **macOS was the blank column; it
is now filled and it agrees.**

### 2b. The PTY surface gate — PASS

Driven through `.planning/scripts/pty-drive.py`, which forks a real PTY, makes it the
child's controlling terminal, sets a 40×110 window, and renders the child's output
through a VT parser so the artifact is a screen rather than escape bytes.

Instrument liveness, both directions, in the same run:

```
under the PTY harness -> ISATTY=1 / stty size = 10 60
through a pipe        -> ISATTY=0
```

Three rendered screens (`pty-cron-refuse-webhook.txt`, `pty-cron-add-event.txt`,
`pty-cron-list.txt`), each with the child's true exit status:
`PART-B-SUMMARY: pty_isatty=1 pipe_isatty=0 screens=3 rc_all_expected=True`.

The webhook refusal renders in full on a TTY (word-wrapped at 110 columns, exit 1);
`cron list` renders the job row and its `last_fired=never`. Nothing about the tty
branch differs from the piped branch here — which is itself the result, and it had
never been checked.

### 2c. The kill-mid-fire continuation run — PASS, with a control

Six events published with no consumer, then `cron daemon` started and **SIGKILL**ed
the instant the first fire record landed — that is "between firing a job and clearing
the event", the exact window `cron publish --help` makes a promise about. SIGKILL, not
SIGTERM, because the daemon documents a clean SIGTERM path and that would measure the
graceful branch.

| arm | killed at | after kill | final |
|---|---|---|---|
| **restart** | history=1 | history=1, **queued=5** | history=**4**, queued=**2** |
| **no-restart control** | history=1 | history=1, queued=5 | history=**1**, queued=**5** |

```
PART-C-VERDICT: continuation_after_kill=True no_restart_control_made_no_progress=True
                events_conserved(fired+queued==published)=True discrimination=PASS
```

Both arms took the kill with **5 events still outstanding** — the comparison is taken
while the two sides can still disagree, not after everything has settled. `fired +
queued == published == 6` on both arms: nothing lost, nothing double-counted. Run
twice independently; identical numbers both times.

> **My first verdict criterion was wrong and I am recording it.** It asserted
> `queued_final == 0`, and reported `discrimination=FAIL` on a run whose numbers
> (1→4 fired, 5→2 queued) plainly show continuation. The drain is **rate-bounded by
> design**, so a zero-queue criterion measures the rate limiter and goes red against a
> correct product. Corrected to "the restarted daemon made progress the control did
> not, and events are conserved". The first log line is preserved in
> `raw-m2-cron-macos.log`; the corrected re-run is `raw-m2-cron-partC-macos.log`.

---

## 3. M3 / 27-C2(c) — the three policy baselines on macOS

`macos-legs-m3.sh` → `raw-m3-baselines-macos.log`.
`macos-legs-m3-mutations.py` → `raw-m3-mutations-macos.log`.

### 3a. The instrument-liveness line that changes a result

The script runs `--list` on every binary **before** running it, and prints
`LISTED_TESTS=`. On the first pass:

```
downloads_root_baseline_test          LISTED_TESTS=2
process_count_reaper_baseline_test    LISTED_TESTS=0     ← baseline 3 does not exist here
approval_gate_baseline_test           LISTED_TESTS=1
…
=== BASELINE 3 — browser process count + reaper ===
running 0 tests
test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out
BASELINE3_RC=0
```

**`ok`, exit 0, and nothing ran.** `process_count_reaper_baseline_test.rs` carried
`#![cfg(target_os = "linux")]`, so on macOS it compiles to an empty harness that
reports success. Any CI job or reader taking the exit status would score baseline 3 as
green on macOS. `LISTED_TESTS=0` is the only line that distinguishes it.

### 3b. Baseline 3 ported to macOS, and now genuinely measured

The Linux-only part was four `/proc` readers. They now have a `ps` twin
(`crates/wcore-browser/tests/process_count_reaper_baseline_test.rs`); every
measurement above them is shared, so both platforms run the *same* baseline. Added
`ps_instrument_is_live`: the reader must find **this** process with its own ppid and a
non-empty `comm`, and must report an impossible PID as not-alive — so an empty or
malformed `ps` table cannot make every "the process is gone" assertion free.

```
LISTED_TESTS=4
EV3-INSTRUMENT: ps_rows=839 self_pid=86520 self_ppid=86034 self_comm=… alive_self=true
                alive_impossible=false discrimination=PASS
EV3A: phase=before tracked_sessions=0 sidecar_pid=none tree_size=0
EV3A: phase=during tracked_sessions=1 sidecar_pid=86544 pid_alive=true tree_size=1 health=2xx
EV3A: phase=after  tracked_sessions=0 pid_alive=false tree_size=0 returned_to_baseline=true
EV3A-SUMMARY: … leaked_processes=0
EV3B: arm=orphan             parent=dead  child_alive_after=false tracked_sessions_after=0 PASS
EV3B: arm=live-parent-control parent=alive child_alive_after=true  tracked_sessions_after=1 PASS
EV3B-SUMMARY: arms=2 orphan_reaped_within_one_interval=true live_parent_child_survived=true
              discrimination=PASS
test result: ok. 3 passed; 0 failed; 1 ignored; 0 measured; 0 filtered out
```

The `1 ignored` is 3c, the real-Camoufox lifecycle. It is disclosed, not hidden: it
needs `@askjo/camofox-browser` at `WAYLAND_CAMOUFOX_BIN`, which I did not install.
**NOT MEASURED on macOS.**

### 3c. Baselines 1 and 2

```
BASELINE 1  EV1-SUMMARY: escape_shapes_tested=4 escape_shapes_refused=4
            provider_ops_on_refusal=0 files_landed_outside_root=0 in_root_admitted=1
            in_root_landed=1 discrimination_control=PASS
            EV1-DEFAULTROOT: out_of_root_refused=true … in_root_landed=true
            test result: ok. 2 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out

BASELINE 2  EV2A-SUMMARY: arms=4 withheld_suspended=2 withheld_backend_dispatches=0
            granted_ok=2 granted_backend_dispatches=2 discrimination=PASS
            test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out
```

Baseline 1 includes the symlink-escape arm (`B4`) and the naive-prefix discrimination
arm (`C`), both of which exercise real macOS symlink resolution.

**Baseline 2's real-desktop half is NOT MEASURED on macOS, and I did not attempt it.**
`baseline_approval_gate_observed_on_real_x11` is
`#[cfg(all(target_os = "linux", feature = "x11-test"))]` — there is no macOS twin in
the tree. Writing one means driving `crates/wcore-cua/src/backends/macos.rs`, which
posts **real HID events** to the machine Sean is using. That is a deliberate
non-attempt, recorded as a gap, not a pass. It is also the one leg here that genuinely
needs a decision before it can be built: a disposable macOS host, or an accepted window
in which the Mac takes synthetic clicks.

### 3d. Negative controls — four mutations, all RED

`macos-legs-m3-mutations.py`. Each breaks exactly one mechanism, rebuilds, re-runs, and
restores from a byte-for-byte backup.

| # | mutation | target | result |
|---|---|---|---|
| MUT-1 | symlink-aware root confinement → naive lexical `starts_with` on the unresolved path | product | `MUTATED_RC=101`, **0 passed; 2 failed** |
| MUT-2 | orphan reaper terminates every session regardless of parent liveness | product | `MUTATED_RC=101`, **2 passed; 1 failed** — the live-parent control arm |
| MUT-3 | macOS `ps` reader returns an empty process table | **this lane's instrument** | `MUTATED_RC=101`, **0 passed; 3 failed**, `ps_instrument_is_live` among them |
| MUT-4 | drop the `Suspend` arm so a withheld approval falls through to dispatch | product | `MUTATED_RC=101`, **0 passed; 1 failed** |

MUT-3 is the one that matters for the port: it proves the new macOS instrument's zeros
are not free.

> **The harness caught itself in a documented trap, and the trap is worth repeating.**
> The first version restored sources with `shutil.copy2`, which preserves the
> **original mtime** — older than the artifact the mutated build had just produced. Cargo
> skipped the rebuild and the restore-verification ran the **stale mutated binary**,
> reporting two of three suites still red against clean sources. `AGENTS.md` §11 already
> names this ("an artifact newer than its source is a build that did not happen"), and it
> is only benign here because the stale binary was the *broken* one; when the stale
> binary is the permissive one the same mechanism yields a false GREEN. Restore now uses
> `copyfile` + `os.utime(src, None)`.

---

## 4. What I did NOT measure — counted, not skipped

| Leg | State | Why |
|---|---|---|
| 27-C2 baseline 2, real-desktop half, macOS | **NOT MEASURED** | no macOS twin exists (`cfg(target_os="linux", feature="x11-test")`); writing one posts real HID events to Sean's working machine — a deliberate non-attempt |
| 27-C2 baseline 3c, real Camoufox sidecar, macOS | **NOT MEASURED** | needs `@askjo/camofox-browser` + `WAYLAND_CAMOUFOX_BIN`; not installed. Visible as `1 ignored`, never as a pass |
| all three rows on **Windows** | **NOT MEASURED** | out of this unit's scope; `SeanDesktop` was not touched |
| a fix for F-M1-01 | **NOT ATTEMPTED** | the walk refuses symlinked components deliberately; relaxing it is the chokepoint owner's call |
| 27-C1 composer/host attachment arms on macOS | **PARTIAL** | `mismatch` and `pdf-as-image` produced correct user-facing refusals from `/Users/...`; the `valid-png` arm's wire extraction came back empty and I did not chase it |

Two instrument caveats, so nobody reads more into the artifacts than is there:

1. `pty-drive.py`'s VT renderer drops a CSI sequence when it is split across two
   `os.read` boundaries — visible as a stray `17;1H` in `pty-tui-intake-home.txt`. The
   untouched bytes are kept alongside every screen (`--raw-out`), so no measurement
   depends on the parser.
2. Part C's numbers are two-run reproducible but are wall-clock dependent: the kill
   lands at the first history record, and how many of the six events are outstanding at
   that moment is a property of this machine's scheduling.
