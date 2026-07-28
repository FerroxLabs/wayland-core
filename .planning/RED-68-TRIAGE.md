# RED-68 — the 68 triaged, and the 81 alongside them

Lane `lane/red-68`, base `plan/f20-unified-audit-repair` @ `3cfc336f`.
Every number below is read back from a named run. Running notes:
`.planning/evidence/red-68/RED-68-NOTES.md`. Full per-test tables:
`.planning/evidence/red-68/class_linux.md`, `class_win.md`.

---

## 0. The headline, before the detail

- **All 68 are classified, and all 68 were re-measured by name** — not by crate
  total. 65 are environment: the CI container is missing three binaries the suite
  needs. 1 is a real defect, 1 a stale test, 1 already-known. Final grading against
  the serial re-runs: `PASS_SERIAL=67  FAIL_SERIAL=1  ABSENT=0`.
- **The 13-test descendant-reaping cluster is a container artefact, not a
  parallelism artefact** — measured both ways, because this program keeps confusing
  the two. 761/761 pass on 96 cores natively, all 13 verified by name.
- **The 68 and the 81 overlap by 33.** They are neither the same list nor disjoint.
  **116 distinct tests** fail across the two platforms.
- **The standing HIGH — `--json-stream` emits no `ready` event on Windows — is
  disproved.** It emits one, in under a second. What was measured was a startup
  refusal caused by the *tests'* own isolation bug. Fixed here, with the mechanism
  measured on the real host.
- **The one Linux failure that is not the container is a wire-contract guard that
  a doc comment can re-redden.** It is the top-ranked real defect and it needs a
  Sean-reserved action, so it is written as a fenced seam request rather than fixed.
- **Three test-side defects fixed** (2 isolation, 1 unfailable gate). **Sixty-one
  described precisely and not touched**, because they belong to the CI-image owner
  and to a Windows lane.

---

## 1. Method, and the instrument defects found on the way

### 1a. The 68 had never been enumerated. `grep 'FAIL ['` loses two of them.

nextest emits a **compound status token** when a test both fails and leaks a
process: `FL+LK`. The obvious extractor misses it, silently, rc=0.

| matcher | unique failing tests | the run's own `Summary` says |
|---|---|---|
| `grep 'FAIL ['` | 66 | 68 |
| `.planning/scripts/extract-nextest-failures.py` | **68** | 68 |

Dropped: `wcore-exec-backend orphan::tests::the_local_scanner_finds_a_descendant_that_was_deliberately_left_behind`
and `wcore-exec-backend::fail_closed_matrix the_local_scan_finds_an_orphan_that_no_registry_remembers`.

Repaired in this lane (§6b-ii), not written up and carried. The replacement
**classifies by exclusion** — any status token it has never seen counts as a
failure — so the next novel compound status fails loud instead of vanishing. It is
cross-checked against three independent oracles (the three `Summary` lines) and
reproduces **68 / 69 / 81 exactly**, with `--expect N` returning rc=1 on mismatch.
Self-test: 3 assertions, the third being *the old matcher would have missed it*.

### 1b. A crate passing 100/100 is not evidence about the test you care about

`.planning/scripts/verify-serial-outcome.py` grades each CI failure against the
serial re-run **by name**: `PASS_SERIAL` / `FAIL_SERIAL` / `SKIPPED` / `ABSENT`.
Its A3 assertion proves the old method — reading the crate's `N passed` summary —
would have cleared a test the re-run never executed. The first pass reported
`PASS_SERIAL=37 FAIL_SERIAL=1 **ABSENT=30**`; without the tool those 30 would have
been folded into "all environment". After the second and third batches:

```
batch 2  PASS_SERIAL=65   FAIL_SERIAL=1   ABSENT=2
batch 3  PASS_SERIAL=67   FAIL_SERIAL=1   ABSENT=0      <- final, nothing unmeasured
```

### 1c. Two more, both of which would have produced a wrong headline

- **`gh run view --job <id> --log` is intercepted by the `rtk` proxy** and returns
  `rtk: Run ID required`, rc=1 — the log never downloads. Working path:
  `gh api /repos/<owner>/<repo>/actions/jobs/<id>/logs`. The brief warns `rtk`
  filters `git log`; it also breaks this.
- **My first Windows probe head-truncated the stderr tail.** It took the last 40
  lines, joined them, then kept the *first* 2500 characters — discarding exactly
  the end of stderr, which is where the fatal error is. Repaired; the repaired
  probe's A3 assertion reports **True**, i.e. the old shape provably missed the
  line that turned out to be the whole answer.
- **My second Windows probe measured a binary that did not exist.** The runner had
  rebuilt `target/release` mid-session. It reported `OUTBYTES=0 READYFRAMES=0` for
  every probe — which reads exactly like a confirmation of the standing HIGH. The
  v3 probe carries a **positive control** (`--version`, which must produce output)
  and an explicit `SPAWN=FAILED … this measurement is UNREADABLE, not evidence`
  branch. That control is what caught it. Without it this report would have
  confirmed the wrong finding with a straight face.

---

## 2. The 68-versus-81 question, answered

| set | count |
|---|---|
| Linux 68 ∩ Windows 81 | **33** |
| Linux only | 35 |
| Windows only | 48 |
| **distinct failing tests across both platforms** | **116** |

The 33 shared are dominated by the 23 `portability_hostile_corpus` tests (neither
host has `python3` available to the runner) plus the contract corpus, the two
`deterministic_openai_loop` cases, `sandbox_activeness`, `typed_execution_policy`,
and 4 of the `runner_contracts` reaping tests. Lists:
`.planning/evidence/red-68/{overlap,linux_only,win_only}.txt`.

The Linux job also ran **twice** — `nick-fields/retry@v3` wraps the nextest
invocation. Attempt 1: 69 failed. Attempt 2: 68. The one test that differs is
`wcore-sandbox backends::process_tree::linux_tests::required_live_descendant_teardown_before_workspace_cleanup`,
i.e. flaky across whole-suite attempts. The authoritative 68 is attempt 2.

---

## 3. The 68, classified

Serial re-runs on `hetzner-dsm` (which has `python3`, `ps` and `bwrap`) at this
lane's HEAD, `--test-threads 1 --no-fail-fast`:

| target | Summary |
|---|---|
| `-p wcore-protocol --test desktop_contract_corpus` | `15 tests run: 14 passed, 1 failed` |
| `-p wcore-exec-backend` | `124 tests run: 124 passed (1 leaky), 1 skipped` |
| `-p wcore-eval-scenarios` | `507 tests run: 507 passed, 5 skipped` |
| `-p wcore-sandbox` | `100 tests run: 100 passed, 2 skipped` |
| `-p wcore-swarm` | `150 tests run: 150 passed, 11 skipped` |
| `-p wcore-tools --test bash_sandbox_routing_test` | `18 tests run: 18 passed` |
| `-p wcore-agent --test typed_execution_policy_e2e_test` | `7 tests run: 7 passed` |
| `-p wcore-cli --lib` | `1831 tests run: 1831 passed, 1 skipped` |
| `-p wcore-cli --test portability_hostile_corpus` | `23 tests run: 23 passed` |
| `-p wcore-cli --test f14_sigkill_recovery` | `11 tests run: 11 passed, 1 skipped` |
| `-p wcore-cli --test sandbox_activeness` | `2 tests run: 2 passed` |
| `-p wcore-cli --test deterministic_openai_loop` | `13 tests run: 13 passed (1 slow)` |

Per-test grading against the CI list: **`PASS_SERIAL=67  FAIL_SERIAL=1  ABSENT=0`.**

| class | n | verdict |
|---|---|---|
| **C1** `python3` absent from the CI image | 23 | environment |
| **C3** `bubblewrap` absent from the CI image | 20 | environment |
| **C4** descendant reaping | 13 | environment (container) — parallelism disproved; exact mechanism not named |
| **C2** `ps` absent from the CI image | 6 | environment |
| **C5** container timing / provenance | 3 | environment |
| **K1** already-known | 1 | `BACKLOG.md:516` |
| **S1** stale test | 1 | test defect |
| **R1** real defect | 1 | **the contract digest guard** |

### The CI image is the cause of 52 of the 68, and it is three `apt` packages

Read back from the job log, lines 147-165 — the image is built inline in `ci.yml`:

```
FROM rust:1.95-slim-bookworm
RUN apt-get install ... libdbus-1-dev libseccomp-dev libssl-dev libasound2-dev \
                       pkg-config mold ca-certificates git
```

No `python3`, no `procps`, no `bubblewrap`. The failure messages name all three:

| missing | message | n |
|---|---|---|
| `python3` | `python3 must be available to materialise a hostile corpus: Os { code: 2, NotFound }` | 23 |
| `ps` | `could not run 'ps' to enumerate processes: No such file or directory` | 6 |
| `bwrap` | `required live bwrap must be installed and usable` / `sandbox backend fail_closed cannot enforce delegated read denial` | 20 |

**`python3` and `procps` are a straightforward image fix. `bubblewrap` is not** —
the `lane/ci-triage` measurement table (CI-TRIAGE.md §2) already proved that
installing bwrap changes the failure mode but not the outcome: the container needs
`--cap-add SYS_ADMIN` plus `seccomp=unconfined` plus `apparmor=unconfined` before a
namespace can be created, and even then the engine's own gate execution against the
bind-mounted `/work` failed. Those 20 need the qualify-or-skip treatment that lane
built for one test, not a package.

**I did not make this change: `ci.yml` is owned by another lane.** It is one edit to
the inline Dockerfile plus a decision on the 20.

### C4 — container, and specifically NOT parallelism (measured both ways)

13 tests assert that a descendant process or listener does not survive teardown
(`wcore-eval-scenarios::runner_contracts` ×7, `pty_capture` ×2,
`wcore-sandbox::process_capture` ×2, `wcore-swarm worktree::tests::linux` ×2).

The board's standing warning is that this program keeps confusing container
artefacts with parallelism artefacts, so the two variables were separated rather
than assumed. CI ran *containerized* **and** *parallel*; the first hetzner re-run
was *native* **and** *serial* — which decides nothing on its own. So a second run
was taken **native and parallel**, on 96 cores, unrestricted:

```
cargo nextest run -p wcore-eval-scenarios -p wcore-sandbox -p wcore-swarm --no-fail-fast
Summary [3.584s] 761 tests run: 761 passed, 18 skipped
```

and each of the 13 was checked **by name**, not by the crate total:
`PASS_SERIAL=13` (all 13 executed and passed under full parallel load).

**Verdict: container artefact. Parallelism is exonerated for this cluster.**

**It is also specifically NOT the missing `ps`.** `crates/wcore-sandbox/src/backends/process_tree.rs`
reads `/proc`; only `crates/wcore-exec-backend/src/orphan.rs:321` shells out to `ps`,
and that crate's 6 failures are separately accounted for above. The remaining
container-side candidate — PID-namespace reparenting under a non-reaping PID 1, or
`/proc` visibility inside `docker run` — was **not** narrowed further here, so the
class is "container", not a named mechanism.

---

## 4. The real defects, ranked by customer impact

The board asked for the silent-message-loss shape first: **loses data, reports a
success it did not achieve, or wedges permanently.**

### R1 — HIGH — a wire-contract guard that a doc comment re-reddens

`wcore-protocol::desktop_contract_corpus checked_corpus_matches_real_serializers_byte_for_byte`.
**The only one of the 68 that fails serially on hetzner too.** Not environment, not
parallelism.

```
Desktop contract corpus drift: missing=[], extra=[],
drifted=["adversarial/events/fixture-mismatch.jsonl", "adversarial/events/schema-mismatch.jsonl",
         "adversarial/events/version-mismatch.jsonl", "events/ready.json", "manifest.json"]
```

Those five files are exactly the five that carry the contract descriptor. **No
schema, event, command or type file drifted** — the wire shape did not change; the
digests over it did.

`source_inputs_digest` recomputed outside cargo
(`.planning/scripts/contract-source-digest.py`, mirrors `generate::source_digest`;
3-assertion self-test passes):

| rev | computed | pinned in manifest | match |
|---|---|---|---|
| `5f74d559` — the Sean-authorized re-stamp | `sha256:2517099…` | `sha256:2517099…` | **True** |
| `189599ca` — the CI run | `sha256:e434c46…` | `sha256:2517099…` | False |
| `3cfc336f` — integration tip | `sha256:3d760cf…` | `sha256:2517099…` | False |

**The re-stamp was correct when it landed and has been invalidated twice since.**
`SOURCE_INPUTS` is **40 files**, and the three that moved are:

```
crates/wcore-agent/src/output/protocol_sink.rs
crates/wcore-agent/src/bootstrap.rs
crates/wcore-cli/src/main.rs        <-- the LANE-BRIEF §6 shared-file fence
```

Seven commits from five lanes moved them. One is
**`bf959017 fix(24-c3): restore AgentBootstrap's own doc comment`**.

**A restored doc comment moved a cryptographic wire-contract digest.** And
`crates/wcore-cli/src/main.rs` is the file the brief instructs *every* lane to make
additive edits to — so the guard is built to be re-reddened by the workflow itself.
That is why "26-02 hit it, 29-03 hit it" and why it keeps returning after each
authorized re-stamp.

**Customer impact.** `observation.rs:329` makes a digest mismatch a hard error at
ready negotiation — the session is refused outright, it does not degrade. So the
guard is protecting something real. But because it fires on changes that cannot
affect the wire, its signal is noise, and the standing cost is a mandatory
Sean-authorized re-stamp *plus a Desktop re-pin in the same release train* every
time any lane touches `main.rs` or `bootstrap.rs`.

**Recommendation:** narrow `SOURCE_INPUTS` to files that can actually change the
serialized shape. `protocol_sink.rs` arguably belongs (it emits `ready`);
`bootstrap.rs` and `main.rs` do not — nothing in either determines a wire field, and
one of the seven commits proves it by being a comment. If the coupling is deliberate
as a coarse tripwire, then `main.rs` must come out of the §6 shared-file fence,
because the two rules directly contradict each other.

**I did not run `wcore-contract generate`** (brief §0). Fenced seam request:

```seam-request
to: release-coordination (Sean-reserved)
what: re-stamp the Desktop contract corpus, AND decide the SOURCE_INPUTS question
why: source_inputs_digest at 3cfc336f is sha256:3d760cf…, pinned is sha256:2517099….
     No schema/event/command/type fixture drifted; only the 5 descriptor-carrying
     files. Moved by 7 commits across 5 lanes touching protocol_sink.rs,
     bootstrap.rs and main.rs — one of which only restored a doc comment.
     A re-stamp alone buys days, not a fix: the next lane to touch main.rs re-reds it.
     Desktop must re-pin in the same release train (observation.rs:329 refuses the
     session on mismatch) — the obligation the 5f74d559 message recorded as
     STILL OWED is still owed.
```

### R2 — HIGH (host-integration) — over `--json-stream` a startup refusal is invisible to the host

Measured on `SeanD@seandesktop`, release binary, with `plugin_discovery_e2e`'s exact
invocation. Full transcript: `.planning/evidence/red-68/windows-probe3-RESULT.txt`.

| probe | isolation | OUTBYTES | ready frames | last stderr line |
|---|---|---|---|---|
| `CTRL_version` | — | 21 | — | *(control: `wayland-core 0.12.25`)* |
| `A_home_only` | `HOME` only | **0** | **0** | `Error: storage.credentials.backend is set to "plaintext", which cannot hold the confidential key that durable session recovery requires…` |
| `B_wayland_home` | `HOME` + `WAYLAND_HOME` | 4751 | **1** | *(normal startup warnings)* |
| `C_wh_long` | same, 150s budget | 4741 | **1** | *(same)* |

Two separable findings:

**R2a — test defect, FIXED HERE.** `plugin_discovery_e2e` and `release_binary_smoke`
isolated config with `HOME` alone. `dirs::home_dir()` reads `USERPROFILE` on Windows,
so the child loaded the developer's real `%APPDATA%\wayland-core` — which carries
`storage.credentials.backend = "plaintext"`. The canonical hermetic override is
`WAYLAND_HOME` (`wcore_config::config::wayland_config_dir`). Adding it produces a
`ready` frame in under a second.

**This disproves the standing HIGH.** `--json-stream` does emit `ready` on that host.
Independent corroboration from the same CI job: `migrate_quarantine` and `smoke_p0`
both captured full `{"type":"ready","version":"0.12.25",…}` frames on the Windows
runner — 4 occurrences in the job log. The earlier probe reproduced the tests' own
isolation bug and read only stdout, so the refusal on stderr was never seen.

**R2b — product defect, NOT fixed here, and the one with the customer-impact shape.**
When durable session recovery cannot start, the engine **exits before emitting any
protocol frame**. Over `--json-stream` a host sees the child close stdout with **zero
bytes** — no `ready`, no `error`, no code. The diagnosis exists only on stderr, which
the Desktop app does not surface. That is "wedges permanently with nothing the host
can act on": the exact class the board asked to be found first. **An `error` frame
emitted before exit would close it.** Not this lane's to fix; it is a protocol-surface
change.

### R3 — MEDIUM — a provenance gate that could not pass, and was vacuous where it could

`wcore-cli::build_provenance binary_matches_repo_head`. **FIXED HERE.**

Step 1 resolved `git rev-parse --short HEAD` (7 chars). Step 2 asserts the embedded
SHA is exactly 40 hex characters. Step 4 — armed whenever `CI` is set —
`assert_eq!(embedded_sha, head)`. By the test's own step 2 those can never be equal.

```
STRICT: binary source 189599ca5af5b84661ce7f93d4318758155d26b9 != HEAD 189599c (stale build — rebuild required)
```

The second string is a **prefix** of the first: the build was not stale, the
comparison was. Meanwhile on the Linux leg the same test **passed in 0.009s** — too
fast to have spawned a 90 MB binary for `--build-info` — because `git rev-parse`
does not succeed inside the CI container and the early return exits having asserted
nothing.

One test, both known failure modes at once: **unfailable in the wrong direction on
one platform, vacuous on the other.** Fixed by comparing full 40-hex to full 40-hex.

**Proof the repaired gate can both pass and fail** — and this took two attempts,
which is itself the finding:

*Attempt 1 self-passed.* Advance HEAD with an empty commit, re-run via
`cargo nextest` → `1 test run: 1 passed`. Cargo had **rebuilt** the binary, so the
embedded SHA moved with HEAD and the gate was never actually presented with a stale
build. (Incidentally this disproves the test's own doc comment, which asserts
"Cargo's rerun-if-changed guards don't re-trigger on a plain `git commit`".) A
falsification that lets the tool under test refresh itself proves nothing.

*Attempt 2, running the already-built test binary directly so cargo cannot
intervene:*

| leg | condition | result |
|---|---|---|
| 1 | embedded == HEAD, `CI=1` (strict armed) | `test result: ok. 1 passed` |
| 2 | HEAD advanced by an empty commit, **same prebuilt binary**, `CI=1` | `test result: FAILED. 0 passed; 1 failed` |

```
STRICT: binary source bdf0b829c4706656ed972d1c3d548274109de203
     != HEAD 6f1f6f1cef49c56ca4cc6d08878fff96161a4ffc (stale build — rebuild required)
```

40-hex against 40-hex, two genuinely different commits, the message it was written
to produce. **Leg 1 was impossible before this fix** — the gate could only ever
produce leg 2's outcome, for the wrong reason.

The remaining vacuity (silent skip when git is unavailable) is **not** fixed here —
making it a hard failure would introduce a new Linux red that belongs to whoever
owns the container's git configuration.

### R4 — LOW — a stale test that is tautological where no backend exists

`wcore-cli sandbox_cmd::tests::sandbox_context_carries_the_contained_profile_and_the_selected_registry`
does `assert_ne!` on two values that are both `"fail_closed"` on a host with no real
sandbox backend. Not fixed — it passes on any host that has one, and the correct fix
is to plant the expectation rather than assert a difference, which is the same
argument `lane/ci-triage` already made for the capability tests.

---

## 5. The 81, and what they are

Full table: `.planning/evidence/red-68/class_win.md`. Not this lane's to fix; the
point of enumerating them is to size the problem.

| class | n |
|---|---|
| W1 `python3` absent on the Windows runner too | 23 |
| W2 credential store unusable as the runner's service account | 13 |
| W9 downstream of W2 (turn never ran, so the gate assertion could not) | 8 |
| W3 test hardcodes a POSIX absolute path (`/workspace/file.txt`, `/srv/wayland/…`) | 7 |
| W12 journal snapshot guards (symlink / hardlink / DACL / oversize) do not fire | 4 |
| W14 runner-contract expectations differ on Windows | 4 |
| **W4 `HOME` does not isolate — FIXED HERE** | **4** |
| W5 service-account temp-dir DACL denies the ACL the test writes | 2 |
| W10 Windows sandbox / AppContainer | 2 |
| W11 platform layer reports the capability unavailable | 2 |
| K1 already-known wall-clock budgets | 2 |
| R1 the same contract guard as Linux | 1 |
| **W6 the unfailable provenance gate — FIXED HERE** | **1** |
| W7 PATHEXT resolution returns probe casing, not on-disk casing | 1 |
| W8 a closed port times out on Windows where it refuses on Linux | 1 |
| W13 traversal refusal does not fire on Windows separators | 1 |
| not investigated | 5 |

**The single most useful thing in that table:** the runner service account is
`NETWORK SERVICE` — visible in every failing temp path as
`C:\WINDOWS\SERVIC~1\NETWOR~1\AppData\Local\Temp`. It has no usable Windows
Credential Manager (`Keyring("Platform secure storage failure: Windows error code 8")`)
and a restrictive temp DACL. **W2+W9+W5 = 23 of the 81 are that one account
property**, not 23 defects. Combined with W1's 23, **46 of the 81 are two facts about
the runner host.** The Windows leg is not "81 broken things".

The same product question as R2b sits under W2: the engine treats an unusable
credential store as a **fatal, non-retryable** session error
(`"Session persistence authority unavailable: secure recovery storage is unavailable"`,
`retryable:false`), producing 0 characters of output. Any customer on a host where the
OS keyring is unreachable gets the same wedge. `HEADLESS-KEYRING-FINDING.md` covers
the ground; this is a second independent sighting, now with a Windows repro.

---

## 6. What I changed

| commit | what |
|---|---|
| `82288335` | enumerate the 68/81; repair the `FL+LK`-blind extractor (3-assertion self-test) |
| `768560e0` | serial re-run per test; `verify-serial-outcome.py`; `contract-source-digest.py` |
| `2f6210f3` | **fix**: `WAYLAND_HOME` isolation in `plugin_discovery_e2e` + `release_binary_smoke`; full-SHA comparison in `build_provenance` |
| *(this file)* | the report and the classified tables |

Three test-side fixes. No product code touched. No `ci.yml` touched. No test weakened,
`#[ignore]`d, re-gated or deleted; no timeout raised.

---

## 7. What I did NOT do — stated plainly

- **Did not fix 61 of the 68.** 52 need one `ci.yml` image edit plus a decision on
  the 20 bwrap tests, and `ci.yml` belongs to another lane. That is deliberate: a
  triage that fixes three and describes sixty-five precisely is worth more than a
  lane that half-fixes thirty.
- **Did not name the mechanism for the 13 descendant-reaping failures (C4).** I
  established what it is *not* — not parallelism (761/761 pass on 96 cores natively,
  all 13 checked by name) and not the missing `ps` — but did not narrow the
  container-side cause to a single mechanism.
- **Did not run `wcore-contract generate`** (brief §0). R1 is a fenced seam request.
- **Did not fix R2b** (no protocol frame before a startup refusal) — protocol surface.
- **Did not verify the two Windows fixes by running the tests on Windows.** The
  mechanism is proven at binary level with the tests' exact invocation and
  environment on the real host, and the fixes are verified green on Linux. The
  Windows runner was mid-rebuild with five `rustc` processes live; contending with
  an active CI job for a test-level rerun was not worth the risk. **This is a gap,
  and it is a gap in the strongest of the three fixes.**
- **Did not touch any Windows-only defect** beyond the two above.
- Did not merge, open a PR, tag, close an issue, or touch `main`.
