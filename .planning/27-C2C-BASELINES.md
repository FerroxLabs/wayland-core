# 27-C2(c) — the three policy baselines, measured

Lane `27c2c-baselines` · branch `lane/27c2c-baselines` · base `8955ee6e` · final HEAD recorded below.
Raw captures: `.planning/evidence/27-c2c-baselines/raw-*.log|txt`. Working notes:
`.planning/evidence/27-c2c-baselines/27-C2C-NOTES.md`.

> **I am not editing `CRITERIA-GAP-LEDGER.md` or `CRITERIA-STATUS.md`** — `lane/release-rank`
> owns those. This file is the input for that reconciliation. My recommendation for the row
> is in §6; the decision is not mine to write.

**All figures below were captured by redirecting to a file on the build host, `scp`-ing the
file into the evidence directory, and reading it with the Read tool** — never through a
Bash-rendered `grep`/`cat`. This is the repair for the rtk defect recorded in `LANE-BRIEF.md`
@ `27c30527`, where `/usr/bin/git diff --numstat` fabricated a count. Every number here is
re-checkable against a committed raw capture.

---

## 0. The parked blocker is FALSE in both of its halves

The ledger parked (c) on *"two of three legs are blocked on a display-capable host that
hetzner cannot provide."* Measured at session start on `hetzner-dsm`:

| Probe | Result |
|---|---|
| `/usr/bin/Xvfb`, `/usr/bin/xvfb-run` | **PRESENT** |
| `/usr/lib/x86_64-linux-gnu/libXtst.so.6` (XTest — what the X11 CUA backend uses) | **PRESENT** |
| `xdpyinfo` under `xvfb-run` | 23 extensions, **`XTEST` among them**, screen 1280x1024 |
| `camoufox` / `chromium` / `chrome` / `firefox` on PATH | all MISSING |
| `apt-cache policy chromium` | `Candidate: (none)` — snap transitional only |
| **`npm view @askjo/camofox-browser`** | **`1.13.0` — EXISTS** |

`@askjo/camofox-browser` is the exact package the product's own error text tells operators to
install (`supervisor.rs:309`). Installed to a lane-local prefix in 22s, started, and:

```
curl -w "HTTP=%{http_code}" http://127.0.0.1:9377/health
HTTP=200
{"ok":true,"engine":"camoufox","browserConnected":true,"browserRunning":true,...}
```

Its own log: `"xvfb virtual display started","display":":99"` → `"camoufox launched"` →
`"browser pre-warmed","ms":1153`. So **hetzner hosts both a display AND the real primary
browser backend.** Neither half of the blocker survives. All three baselines were measured on
that host; none needed Sean's Mac or SeanDesktop, and the macOS CUA backend (which posts real
HID events to a machine Sean uses) was never touched.

---

## 1. BASELINE 1 — downloads-root confinement

`crates/wcore-browser/tests/downloads_root_baseline_test.rs`.
Enforcement point: `validate_local_path` (`tool.rs:112`, root-confinement `:151-162`,
symlink-aware via `canonicalize_existing_prefix` `:188`), invoked at `tool.rs:471` **before any
backend dispatch**.

Raw: `raw-ev1-downloads-root.log`, `raw-ev1-mutated-known-negative.log`,
`raw-FINAL-all-three-baselines.log` lines 2-20.

### Recorded numbers

| Arm | Refused | Provider ops | File landed |
|---|---|---|---|
| A — in-root `<root>/report.pdf` | no | **1** | yes, in-root, expected bytes |
| B1 — absolute escape `<outside>/abs-escape.bin` | yes | **0** | no |
| B2 — `..` traversal | yes | **0** | no |
| B3 — dotfile `<root>/.ssh/authorized_keys` | yes | **0** | no |
| B4 — **symlink escape** `<root>/innocent → <outside>` | yes | **0** | no |
| D — **same literal path as B1, root = its own parent** | **no** | **1** | **yes** |

```
EV1-SUMMARY: escape_shapes_tested=4 escape_shapes_refused=4 provider_ops_on_refusal=0
             files_landed_outside_root=0 in_root_admitted=1 in_root_landed=1
             discrimination_control=PASS
EV1-DEFAULTROOT: out_of_root_refused=true out_of_root_provider_ops=0
                 in_root_admitted=true in_root_provider_ops=1 in_root_landed=true
test result: ok. 2 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out
```

The `provider_ops=0` column is what makes this an *enforcement* measurement rather than an
error-string check: the refusal is upstream of dispatch, and the recording provider **really
writes `dest_path`** when it is reached — so "did not escape" is a filesystem fact, not a
message. The pre-existing inline tests in `tool.rs` assert only `is_error` against a backend
that answers `Ok` to everything; they cannot tell a refusal from a backend no-op and never
look at the filesystem.

### Both directions

- **Can it pass?** ARM A and ARM D. **ARM D is the discrimination**: the *same literal target
  path* is refused under root A and admitted under root B (= its own parent). A gate that
  refused unconditionally, or a needle pointed at the wrong tree, cannot produce that split.
- **Can it fail?** I replaced the symlink-resolving confinement with a naive lexical
  `starts_with` on the unresolved path and re-ran: `MUTATED_RC=101`, `1 passed; 1 failed`,
  failing exactly at `B4-symlink: escape to /tmp/.tmp0XjPCg/innocent/loot.bin was NOT
  refused`. Mutation reverted; suite back to `2 passed; 0 failed`.
- **Third assertion (§6b-ii)** — ARM C asserts the symlink path *lexically* starts with the
  root, i.e. **the naive matcher would have ADMITTED it**. Without that, the symlink arm
  would not demonstrate what it claims.

### What I could NOT measure, and why

**No backend in the tree implements `BrowserOp::Download`.** Measured with a known-positive
control on the instrument:

- `backends/chromium.rs:332-338` returns `Unsupported("chromium: Download requires
  Browser.downloadProgress event handling...")`; `Upload` likewise at `:327`.
- `backends/camoufox.rs:240-425 dispatch()` handles exactly `Navigate`, `Snapshot`,
  `Read`(→Unsupported), `GetState`, `Click`, `Fill`, `Press`, `Screenshot`, `Back`/`Forward`,
  then `unsupported => Err(Unsupported("{op} does not have a truthful Camoufox API mapping"))`.
  **`Download` and `Upload` are not among the arms.** `op_name()` maps `Download => "download"`
  (`:503`) only to name it in that error string. (Instrument control: the same extraction finds
  `BrowserOp::Navigate`, count 1.)

So the clause splits: **"must not escape" is fully measured** (it is the security property and
it is enforced pre-dispatch); **"must land inside" is NOT measurable end-to-end in the product
as shipped**, because the operation does not exist — not for want of a host. I measure the
tool-layer half and **do not claim the end-to-end download.** Note the corollary: the
confinement gate currently guards two ops (`Download`, `Upload`) that no backend can perform.

---

## 2. BASELINE 2 — the approval gate on a computer-use operation

`crates/wcore-cua/tests/approval_gate_baseline_test.rs`. Two tests: one always-on tool-level,
one behind the crate's own `x11-test` feature observing a **real X server**.
Raw: `raw-ev2a-approval-tool-level.log`, `raw-ev2b-x11-event-delivery.log`,
`raw-ev2b-mutated-known-negative.log`, `raw-FINAL-all-three-baselines.log` lines 22-40.

### 2a — tool level (no display needed)

| Arm | Outcome | Backend dispatches |
|---|---|---|
| `require_approval_for_app` — WITHHELD | `PolicySuspended` | **0** |
| approval GRANTED | `Ok` | **1** |
| `first_time_per_app_approval` — before | `PolicySuspended` | **0** |
| after `mark_app_seen` | `Ok` | **1** |

```
EV2A-SUMMARY: arms=4 withheld_suspended=2 withheld_backend_dispatches=0
              granted_ok=2 granted_backend_dispatches=2 discrimination=PASS
test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out
```

`backend_dispatches=0` on the withheld arms is the point: approval is **not advisory** — the
op never reaches the desktop.

### 2b — the same gate, observed on a REAL X server under Xvfb

Run under `xvfb-run -a -s "-screen 0 1280x1024x24"` with `--features x11-test`, on
`DISPLAY=:100`, against the real `LinuxX11Backend` (XTest via `x11rb`). The observable is
**what the X server actually DELIVERS to an independent X client** — a second x11rb connection
selecting `POINTER_MOTION | BUTTON_PRESS | BUTTON_RELEASE` on the root window, exactly as
`xev -root` does. Not the Rust return value.

| Step | Outcome | Delivered events |
|---|---|---|
| 1 — instrument liveness, permissive `MouseMove(100,100)` | `Ok` | **1**, MotionNotify **at (100,100)** |
| 2 — approval WITHHELD, `LeftClick(700,500)` | `PolicySuspended` | **0** |
| 3 — approval GRANTED, **same op** | `Ok` | **3**, ButtonPress **at (700,500)** |
| 4 — first-time gate, before approval | `PolicySuspended` | **0** |
| 5 — after `mark_app_seen` | `Ok` | **3**, ButtonPress **at (300,250)** |

```
EV2B-SUMMARY: display=:100 steps=5 instrument_liveness=PASS withheld_arms=2
              withheld_delivered_events=0 granted_arms=2 granted_delivered_events_nonzero=2
              discrimination=PASS
test result: ok. 2 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out
```

Steps 2 and 3 are the **identical op at the identical coordinate**; only the approval state
differs. That is the discrimination.

### The instrument-liveness step earned its place immediately — read this one

My first version read the pointer coordinate back with `QueryPointer`. **It failed, and it was
right to.** The product's `MouseMove{100,100}` returned **`Ok`** while `QueryPointer` read
**`(640,512)`** — the untouched screen centre. A green return with no observable effect.

The cause is **environmental, not a product defect**, and I confirmed that with an independent
instrument before concluding anything (`raw-xtest-probe2.txt`): on this Xvfb
`xdotool mousemove --sync 300 200`, `mousemove_relative --sync 50 50` and `click 1` **all
return rc=0 and all leave the coordinate pinned at 640,512**. Pointer *position* is simply not
a live observable on this display, for any tool. Event *delivery* is: `xev -root -event mouse
-event button` recorded **2 MotionNotify + 2 ButtonPress + 2 ButtonRelease at the exact
requested coordinates, `synthetic NO`** (`raw-xev-probe2.txt`).

Two lessons worth carrying, both already in `LANE-BRIEF.md` and both instantiated here:

1. Had I asserted only on the Rust return value, step 2's "no input reached the desktop" would
   have been free and the whole arm vacuous. The known-positive is what gives the negative
   meaning.
2. Had I kept `QueryPointer`, the gate would have been **permanently red** (§3b-iii) — and it
   would have looked like a product failure. One intermediate capture in this lane also went
   the other way: my first `xev` probe used an invalid mask (`-event pointer`) and reported
   `ButtonPress=0, MotionNotify=0`. **Those zeros were a dead instrument**, and only reading
   the raw file (which contained `xev: unrecognized event mask 'pointer'` + a usage dump)
   revealed it. Both directions of the same defect class, in one lane.

### Both directions

- **Can it pass?** Steps 1, 3, 5 — real delivered events at the requested coordinates.
- **Can it fail?** I mutated `CuaTool::dispatch` to treat `CuaPolicyOutcome::Suspend` as
  advisory and proceed. Both tests reddened: `0 passed; 2 failed`, at
  `WITHHELD: expected PolicySuspended, got Ok(Ok)` and `STEP 2: expected PolicySuspended, got
  Ok(Ok)`. Reverted (`git diff` on `wcore-cua/src/tool.rs` empty).

### Limitation I am recording rather than glossing

**This baseline reads no merged config.** It constructs `CuaPolicy` programmatically. Prompted
by `lane/egress-merge-polarity`'s finding that an untrusted project config can *raise*
`max_tokens`/`max_turns`, I checked which layer I was on: the answer is "none — in-process
struct". So the measurement is not sitting on that defect, **but it also does not prove the
config→`CuaPolicy` plumbing preserves approval settings across the trust boundary.** If an
untrusted project config can weaken `require_approval_for_app` or flip
`first_time_per_app_approval` to `false`, this baseline would not see it. **Untested adjacent
surface, recommended as follow-up work** (§7).

---

## 3. BASELINE 3 — process count before / during / after, plus one reaper interval

`crates/wcore-browser/tests/process_count_reaper_baseline_test.rs`, `#[cfg(target_os = "linux")]`
(metrics read `/proc`). Raw: `raw-ev3ab-process-count-reaper.log`, `raw-ev3c-real-camoufox.log`,
`raw-ev3-mutated-known-negative.log`, `raw-ev3-final-after-revert.log`,
`raw-FINAL-all-three-baselines.log` lines 42-73.

Three metrics, never asserted alone: **A** supervisor-tracked sessions, **B** OS-level PID
liveness from `/proc/<pid>/stat` (rejecting state `Z`, so an un-reaped zombie counts as *not*
cleaned up), **C** live descendant count from a `/proc` walk.

### 3a — lifecycle, stand-in sidecar (real process, real HTTP health gate)

| Phase | Tracked sessions | PID alive | Tree size |
|---|---|---|---|
| before | 0 | — | 0 |
| during (`ensure_ready`, `/health` 2xx) | **1** | **true** | **1** |
| after (`on_session_end`) | **0** | **false** | **0** |

```
EV3A-SUMMARY: before_tracked=0 before_tree=0 during_tracked=1 during_tree=1
              after_tracked=0 after_tree=0 leaked_processes=0
```

### 3b — one reaper interval, with a control arm

`reaper_interval = 200ms`. Both arms spawn a real `/bin/sleep 300` and assert it reached a live
state **before** the behaviour under test (§6a-i — an actor that never launched is a dead
instrument).

| Arm | Registered parent | Child alive after | Tracked after |
|---|---|---|---|
| orphan | **dead** (`0x7ffffffe`, asserted not alive) | **false** — reaped within one interval | 0 |
| **control** | **alive** (our own PID), waited **10×** the interval | **true** | 1 |

```
EV3B-SUMMARY: arms=2 orphan_reaped_within_one_interval=true
              live_parent_child_survived=true discrimination=PASS
```

The control arm is what rules out a reaper that kills indiscriminately, and rules out "the
child exited on its own". It waits **strictly longer** than the orphan arm took, so "still
alive" cannot be explained by not having waited.

### 3c — the SAME lifecycle against the REAL Camoufox sidecar

Production constructor `SupervisorConfig::local_camoufox("http://127.0.0.1:9377")`, production
port, `WAYLAND_CAMOUFOX_BIN` pointing at the real `camofox-browser`.

```
EV3C: phase=before tracked_sessions=0 tree_size=0 preexisting_sidecar=none
EV3C: phase=during tracked_sessions=1 sidecar_pid=226370 tree_size=3
      descendants=["Xvfb", "camoufox-bin"] health=2xx
EV3C: phase=after  tracked_sessions=0 tree_size=0 returned_to_baseline=true
EV3C-SUMMARY: backend=real-camoufox before_tree=0 during_tree=3 after_tree=0 leaked_processes=0
test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 2 filtered out
```

Two guards that make this a real measurement rather than a shape:

- **Pre-existing-sidecar guard.** `ensure_ready` *reuses* an externally healthy sidecar and
  spawns nothing. The test asserts the health probe is FALSE first, so the counts describe a
  process **this run started**.
- **Browser-child wait.** The first run of 3c tore down 1.12s after `ensure_ready` and recorded
  `during_tree=2` — it measured the teardown of a sidecar **whose browser had not spawned
  yet**, so `after_tree=0` said nothing about browser cleanup. It now waits for `tree >= 3`
  *and* asserts a descendant `comm` matches `camoufox|firefox`, and the evidence names them.
  The leak an operator cares about is a leaked **browser**, not a leaked node process.

`#[ignore]` by default (needs the npm sidecar, unavailable in CI) and **run explicitly with
`-- --ignored`**, with its executed count reported. An `ignored` line is visibly not a pass; a
silent skip would not be.

### Both directions

- **Can it pass?** All three arms above, plus 3b's live-parent control.
- **Can it fail?** I gated `terminate_session` + `terminate_owned_session` behind a flag that
  skips termination. All three arms reddened:
  `AFTER: sidecar PID 83233 is STILL ALIVE after on_session_end — process leak`;
  `ARM 1: orphan PID 85642 survived the reaper — cleanup policy NOT preserved`;
  and on the real sidecar **`7 process(es) leaked from PID 167286`**.
  `0 passed; 2 failed; 1 ignored` then `0 passed; 1 failed`. Reverted (`git diff` on
  `supervisor.rs` empty), leaked processes cleaned up, suite back to `2 passed; 0 failed;
  1 ignored` and `1 passed; 0 failed`.

---

## 4. Consolidated final run (single capture, formatted tree)

`raw-FINAL-all-three-baselines.log`, at `HEAD=6f68848f9810dc2ff700a32856b1bc46dd8b5dc3`:

| Suite | Result |
|---|---|
| B1 downloads-root | `2 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out` |
| B2 approval (xvfb + `x11-test`) | `2 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out` |
| B3a/B3b process count + reaper | `2 passed; 0 failed; 1 ignored; 0 measured; 0 filtered out` |
| B3c real Camoufox | `1 passed; 0 failed; 0 ignored; 0 measured; 2 filtered out` |

`cargo fmt --all -- --check` → **rc 0, zero diffs**. (Only my three files needed formatting; I
formatted those three specifically rather than `--all`, so no other lane's files were touched.)

---

## 5. NOT MEASURED — stated plainly, not counted as passes

1. **macOS and Windows: all three baselines.** Baseline 3 is `#[cfg(target_os = "linux")]`
   (`/proc`); baseline 2b is Linux/X11; baseline 1's symlink and naive-prefix arms are
   `#[cfg(unix)]`. Baseline 1's fail-closed default-root pair is cross-platform and *would*
   run on Windows, which is why it is deliberately ungated — the file never runs zero tests.
   **This is a gap, not a pass.** The macOS CUA backend was deliberately not exercised: it
   posts real HID events on a machine Sean uses.
2. **End-to-end browser download.** Impossible in the shipped product — no backend implements
   `BrowserOp::Download` (§1). Not a host limitation.
3. **The config → policy trust boundary** for CUA approval settings (§2 limitation).
4. **Wayland CUA backend.** Only X11 was measured.
5. **`BrowserOp::Upload` confinement end-to-end** — same unimplemented-backend situation as
   Download.

---

## 6. Recommendation for the ledger row (for `lane/release-rank` to apply or reject)

The three baselines named in (c) **now exist, are executed, and each is proven to move in both
directions.** On that basis (c)'s stated deliverable is met on Linux. I do **not** recommend a
bare `MET`, because two of the row's own words do not survive contact:

> **Suggested: `27-C2` PARTIAL → MET-WITH-STATED-EXCEPTIONS**, exceptions being (i) Linux only
> — macOS and Windows are NOT MEASURED for all three baselines; (ii) the *"must land inside the
> downloads root"* half of the clause is **vacuous in the shipped product** because no backend
> implements `Download`; (iii) the CUA approval baseline measures the programmatic policy, not
> the config→policy trust boundary.
>
> **And the row's blocker sentence must not be read forward.** *"Two of its three legs are
> blocked on a display-capable host that hetzner cannot provide"* is **false in both halves** —
> hetzner has Xvfb + XTest, and the real Camoufox sidecar installs and runs there. §0 has the
> probes.

I am not claiming the criterion myself, and the release-blocking judgement is not mine.

---

## 7. Findings for the backlog

**F-27C2C-01 (MEDIUM, pre-existing, NOT mine to fix) — `cargo --locked` fails at integration
head.** `crates/wcore-eval-scenarios/Cargo.toml:123` declares `serial_test.workspace = true`
but `Cargo.lock` does not list it under that package. At base `8955ee6e`:

```
cargo metadata --locked --format-version 1
METADATA_LOCKED_RC=101
error: cannot update the lock file /root/wayland-27c2c/Cargo.lock because --locked was passed
```

Any build/audit/release job using `--locked` fails at head. Verified with a known-positive
control (`wcore-browser`'s own `serial_test`/`wiremock`/`tempfile` *are* present in the lock at
the same base, so the extraction is alive). Raw: `raw-cargo-lock-check.txt`. **I did not fix
it** — `Cargo.lock` is touched by every lane and `wcore-eval-scenarios` is not mine; a
one-line `cargo update -p wcore-eval-scenarios` by whoever owns the lock closes it.

**F-27C2C-02 (LOW / documentation) — the confinement gate guards two ops no backend can
perform.** `validate_local_path` protects `Download::dest_path` and `Upload::path`; both ops
are `Unsupported` on every backend (§1). The gate is correct and should stay — it is the right
place for the check when the ops land — but anyone reading the security posture should know the
ops are currently unreachable.

**F-27C2C-03 (environmental, for whoever writes the next X11 fixture) — `QueryPointer` is not
a usable observable on Xvfb here.** Pointer position reads `(640,512)` forever even for
`xdotool mousemove --sync`. **Use event delivery** (`EventRecorder` in
`approval_gate_baseline_test.rs`, or `xev -root -event mouse -event button`), which works
correctly and additionally reports `synthetic NO` — the backend's background-clean claim.
Recorded so the next lane does not spend the time I spent, and does not misread it as a
product defect.

**F-27C2C-04 (operational) — the real Camoufox sidecar is obtainable on hetzner.**
`npm install --prefix <dir> @askjo/camofox-browser@1.13.0`, then
`WAYLAND_CAMOUFOX_BIN=<prefix>/node_modules/.bin/camofox-browser`. It brings its own Xvfb. This
unblocks any future browser leg that was parked on "no browser on the build host", and it is
the package the product's own error message names.

---

## 8. Files added by this lane

| File | Purpose |
|---|---|
| `crates/wcore-browser/tests/downloads_root_baseline_test.rs` | Baseline 1 |
| `crates/wcore-cua/tests/approval_gate_baseline_test.rs` | Baseline 2 (2a always-on, 2b behind `x11-test`) |
| `crates/wcore-browser/tests/process_count_reaper_baseline_test.rs` | Baseline 3 (3a/3b always-on, 3c `#[ignore]`) |
| `.planning/27-C2C-BASELINES.md` | This file |
| `.planning/evidence/27-c2c-baselines/27-C2C-NOTES.md` | Working notes, append-only |
| `.planning/evidence/27-c2c-baselines/raw-*.log|txt` | Raw captures behind every number above |

**No production source file was modified.** The two mutations used for the known-negatives were
applied on the build host only, never committed, and both reverted with an empty `git diff`
verified afterwards. No shared-fence file (`wcore-cli/src/lib.rs`, `wcore-cli/src/main.rs`) was
touched. `CRITERIA-GAP-LEDGER.md` and `CRITERIA-STATUS.md` were **not** touched.

### How to re-run

```bash
# Baseline 1
cargo test -p wcore-browser --test downloads_root_baseline_test -- --nocapture
# Baseline 2 (needs a display; x11-test is off by default)
xvfb-run -a -s "-screen 0 1280x1024x24" \
  cargo test -p wcore-cua --features x11-test --test approval_gate_baseline_test \
  -- --nocapture --test-threads=1
# Baseline 3a/3b
cargo test -p wcore-browser --test process_count_reaper_baseline_test -- --test-threads=1
# Baseline 3c — real sidecar; nothing may already be listening on 9377
export WAYLAND_CAMOUFOX_BIN=/path/to/node_modules/.bin/camofox-browser
cargo test -p wcore-browser --test process_count_reaper_baseline_test -- --test-threads=1 --ignored
```
