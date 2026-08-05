# 27-C1 — one bounded intake path

**Lane:** `lane/27-c1-intake`. **Base:** `plan/f20-unified-audit-repair` @ `5457710e`.

**Criterion 1, verbatim:** *"Standalone and host messages use one bounded,
validated attachment/document intake path and degrade explicitly on unsupported
providers."*

**Verdict: the "one bounded, validated intake path" clause is now MET.** Nine
intake paths existed; six caller-supplied-media surfaces are consolidated onto
one chokepoint; three were measured out of scope with reasons. The bound is
enforced on the consolidated path and mutation-proved. The explicit-degradation
clause was already met by `27-01` and is untouched here.

**The prior costing was wrong, and it is refuted with executable evidence, not
argument.** `CRITERIA-GAP-LEDGER.md:677` called C1 *"an architecture criterion
over code that was measured already correct on the duplicated paths — real
value, zero defect value."* Four of my known-negatives were **RED at base**,
including one where the shipped binary read a file under a deny-listed
credential path and returned its bytes as `audio/wav`.

---

## 1. How many paths existed

The `27-01` audit enumerated **four**. Source census at base found **nine**:

| # | Path | Validation at base | Bound at base |
|---|---|---|---|
| 1 | composer / host attachments → `load_local_image` | `validate_user_path` + `openat(O_NOFOLLOW)` walk | descriptor stat + `take()` |
| 2 | `vision_analyze`, local arg → same loader | as 1 | as 1 |
| 3 | `vision_analyze`, URL arg → `validate_image_bytes` | SSRF + website policy | post-fetch |
| 4 | `pdf_extract` → `media_intake::admit_path` | `validate_user_path` | descriptor stat |
| 5 | `doc_extract` → its own open sequence | `validate_user_path`, then **two more by-name resolutions** | own |
| 6 | channel enricher → inline `detect_*_mime` | n/a (connector bytes) | inline |
| 7 | **`transcribe_audio`, local path** | **NONE** | **stat-only; the read was unbounded `fs::read`** |
| 8 | `transcribe_audio`, URL arg | `validate_audio_url` | post-fetch |
| 9 | `video_analyze` | own ffmpeg-specific discipline | own |

Paths **7 and 8 were missing from the earlier census entirely**, which is why
"already correct" was never a measurement over the whole surface.

The chokepoint that plan `27-01` built had **one** production caller
(`pdf_tool.rs`), and its `admit_bytes` half had **zero** — exercised only by
its own unit test. Known-positive control for that grep in the same invocation:
`grep -c "pub fn"` on the same file returned 7.

## 2. What was consolidated

`crates/wcore-tools/src/media_intake.rs` is now the single chokepoint, and it
absorbed the **strongest** mechanism found on any path rather than the average
one. `admit_open` is the one primitive; `admit_path` and `admit_bytes` are
projections of it.

| Moved onto it | Was |
|---|---|
| `vision_tools::load_local_image` (paths 1, 2) | its own hardened open + own magic table |
| `vision_tools::validate_image_bytes` (path 3) | own inline caps |
| `transcription_tools` `AudioSource::Path` (path 7) | **no validation, unbounded read** |
| `transcription_tools` URL/bytes (path 8) | own inline caps |
| `wcore-cli::attachments` composer | its own private extension→MIME table |
| `wcore-agent::channel_media` (path 6) | inline caps + `detect_*_mime` |
| `doc_tool` (path 5) | `validate_user_path` + `is_file()` + `File::open` = 3 resolutions |
| `pdf_tool` (path 4) | already there |

Upgrades the consolidation delivered, none of which were the goal:

- **The `openat(O_NOFOLLOW)` + `O_NONBLOCK` component walk now covers every
  surface.** Only the image path had it; PDF, documents and audio used plain
  `File::open`, which follows symlinks and blocks on a FIFO.
- **`RIFF` disambiguation.** The chokepoint classified on an 8-byte prefix and
  treated bare `RIFF` as WebP. A WAV file has the same first eight bytes.
  Harmless while nothing but PDF used it; a cross-class admission the moment
  audio joined. Prefix widened to 12 bytes and both forms pinned.
- **The extension-versus-bytes cross-check now covers `vision_analyze`.** It
  lived only in the composer's private table, so the surface the MODEL reaches
  did not have it.
- **`open_once` is private to the chokepoint**, so "no other module opens a
  caller-named media file" is compiler-enforced rather than a convention.

Each surface keeps its own `IntakePolicy` (caps, accepted classes, diagnostic
noun). **No cap was loosened and no accepted-format set was widened.**
`doc_tool` is the only policy setting `allow_unclassified`, because CSV and
plain text genuinely have no container signature — and it must still name
`Unclassified` in its accept list, so the flag alone is not enough.

## 3. Bound enforcement — the known-negative, and the fact that it was
   self-passing first

Required: an input exceeding the bound must be rejected, and the test must go
red if the enforcement is removed.

**Round 1 — my instrument was broken.** The fixture was `cap + 1`. Deleting the
stat-side cap from `admit_open` left the test **green**, because the
defence-in-depth check after the bounded `take(cap + 1)` refuses citing the same
number. The assertion could not tell the two enforcement points apart.

**Round 2 — repaired in this lane, not written up and left** (LANE-BRIEF
§6b-ii). A `3 × cap` sparse fixture makes the two report different numbers, plus
a third assertion pinning that the read-side number did NOT appear:

```
--- A. UNMUTATED ---
test result: ok. 14 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out

--- B. MUTATED (stat-side cap deleted from admit_open) ---
test audio_over_the_declared_cap_is_refused_from_the_stat_not_from_the_read ... FAILED
  refusal must cite the FULL length 78643200 ...; got: Audio too large: 26214401 bytes
test image_over_the_declared_cap_is_refused_from_the_stat_not_from_the_read ... FAILED
  must cite the FULL length 62914560; got: Image too large ...: 20971521 bytes
test result: FAILED. 12 passed; 2 failed; 0 ignored; 0 measured; 0 filtered out
```

The mutated build's numbers — `26214401`, `20971521` — are exactly `cap + 1`,
which is precisely the string the un-repaired assertion was searching for. So
the third assertion ("the old broken matcher would have missed it") is a
measurement from Round 1, not an inference. Full capture:
`evidence/27-c1/MUTATION-cap-removed.txt`.

## 4. Baseline RED — the refutation

`crates/wcore-tools/tests/media_intake_unification_test.rs`, run unchanged at
base `5457710e` (`evidence/27-c1/BASE-5457710e-RED.txt`):

```
test result: FAILED. 10 passed; 4 failed; 0 ignored; 0 measured; 0 filtered out
```

| RED at base | What base did |
|---|---|
| `audio_refuses_a_denylisted_credential_path` | **READ a file under a deny-listed credential path and returned its bytes as `("audio/wav", …)`** — ready to be posted to a third-party STT provider |
| `audio_refuses_a_traversal_segment` | admitted an absolute path carrying `..` |
| `audio_refuses_a_symlinked_leaf` | followed the symlink, returned the target's bytes |
| `an_extension_that_contradicts_the_bytes_…` | `vision_analyze` admitted a PNG named `.jpg` |

At HEAD: `14 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out`.

**Two tests passed at base for the wrong reason and are NOT counted.**
`audio_refuses_a_relative_path` and `audio_refuses_a_unc_target` both failed
with ENOENT on Linux rather than with a policy refusal. The UNC case
discriminates only on Windows, which this lane did not run — recorded as
source-evidenced, not measured on the platform that matters. Both annotations
are in the test file itself.

**Severity of the audio finding: HIGH.** It is reachable from a model-supplied
tool argument on the default binary (`transcribe_audio` is registered outside
the `voice` feature gate whenever an STT key resolves), and it removed a
boundary — `validate_user_path` — that every other file-reading tool in the
tree applies. Exfiltration additionally requires the target's bytes to sniff as
audio, which narrows it; the missing boundary does not depend on that.

## 5. Live evidence — the shipped binary, not a unit test

`.planning/scripts/f27-c1-intake-live.sh` against
`target/release/wayland-core` (`wayland-core 0.12.25`) on `hetzner-dsm`, each
observation changing exactly one variable. Full transcript:
`evidence/27-c1/LIVE-OBS-RAW.log`.

`transcribe_audio` driven as a real tool call, refusal read back off the wire
as the `tool_result` the engine sent the model:

| Observation | `tool_result` on the wire |
|---|---|
| valid WAV | `groq transcription returned HTTP 401` — **ADMITTED**; the intake let it through and the fixture key failed at the backend. The refusals below are therefore not blanket. |
| deny-listed path | `Invalid path: path targets a denied system location: ".../.ssh/id_rsa"` |
| traversal | `Invalid path: path contains traversal (..)` |
| symlinked leaf | `Cannot open audio path component …: Too many levels of symbolic links (os error 40)` — `O_NOFOLLOW` firing live |
| 3× cap sparse | `Audio too large: 78643200 bytes (limit 26214400 bytes)` — **the full 3× length, from the descriptor's stat, live** |
| relative | `Invalid path: path must be absolute` |

Composer / host-protocol surface, unchanged through consolidation:

| Observation | What the USER saw | Wire |
|---|---|---|
| `valid-image.png` | no error | `IMAGE part media_type=image/png data_len=100` — byte-identical to `27-01`'s OBS-01 |
| `mismatch-png-body-jpg-ext.jpg` | `error`/`engine_error`: *"extension declares image/jpeg but bytes are image/png"* | **0 captured requests** — refused before any provider call |
| `valid-doc.pdf` | `error`/`engine_error`: *"Unsupported image format (only PNG, JPEG, GIF, BMP, WEBP are supported)"* | **0 captured requests** |

Both composer wordings are byte-for-byte what `27-01` recorded live at base, so
routing them through the shared chokepoint churned nothing a host may match on.

**Provider read-back (LANE-BRIEF §3b-ii):** every audio capture holds 2 recorded
requests written by *my* mock endpoint. Had the engine reached
`api.anthropic.com` the capture would be empty. The arm is read from the
product's own traffic, not inferred from the environment. A **dummy**
`GROQ_API_KEY=f27-fixture-not-a-real-key` was exported so the tool registers at
all; no real credential was used anywhere in this lane, and the secret sweep
over the whole evidence tree reports `0`.

## 6. The "one path" gate, proved able to fail

`.planning/scripts/f27-c1-one-path-gate.sh` — four checks: no media surface
opens a file itself; every surface reaches the chokepoint; `open_once` is
private; exactly one magic-byte table. Run with `/usr/bin/grep` throughout, with
a known-positive liveness check in the same invocation that aborts the whole
gate if it returns zero.

```
BASE 5457710e : GATE: FAIL   BASE_RC=1
HEAD          : GATE: PASS   HEAD_RC=0
```

At base it fails with the right diagnosis — 3 surfaces opening their own files,
only 1 of 6 reaching the chokepoint, `open_once` absent, 2 magic tables. Capture:
`evidence/27-c1/GATE-one-path.txt`.

## 7. Measured exclusions, recorded rather than dropped

- **`video_analyze` (path 9) is not an intake path.** It never ingests the
  caller's bytes: it hands the path to `ffmpeg` as a subprocess argument, so its
  discipline is argv-injection defence plus a realpath whitelist, which
  `media_intake` neither provides nor should. The frames it then reads are
  ffmpeg-produced files in an engine-owned tempdir with a hardcoded
  `image/jpeg` — engine-produced bytes, no caller-supplied name. The unbounded
  `tokio::fs::read` on those frames is a **MEDIUM for BACKLOG**, not this
  criterion.
- **Remote-fetch paths (3, 8) share caps and format decision but not the open
  sequence**, because there is no path to protect. That is the correct
  boundary, and `admit_bytes` — which had zero production callers — now has
  four.

## 8. Test state

| Gate | Host | Result |
|---|---|---|
| `cargo fmt --all -- --check` | Mac | clean |
| `cargo clippy -p wcore-tools -p wcore-agent -p wcore-cli --all-targets --all-features -- -D warnings` | hetzner-dsm | **clean, exit 0** |
| `cargo test -p wcore-tools` | hetzner-dsm | **998 passed; 0 failed; 3 ignored; 0 filtered out** (lib) plus 26 green integration binaries |
| `cargo test -p wcore-tools --test media_intake_unification_test` | hetzner-dsm | **14 passed; 0 failed; 0 ignored; 0 filtered out** |
| `cargo test -p wcore-cli --lib` | hetzner-dsm | **1854 passed; 0 failed; 1 ignored** |
| `cargo test -p wcore-agent --lib channel_media` | hetzner-dsm | **14 passed; 0 failed** |
| `cargo build --release -p wcore-cli` | hetzner-dsm | exit 0 |

**`cargo test -p wcore-agent --lib` is RED and is reported RED — and it is
pre-existing.** Base `5457710e`, same box, same command: `2165 passed; 16
failed`. HEAD: `2165 passed; 16 failed`. The failing SET is not stable between
runs at the same commit (21 / 17 / 16 names across three runs, base and HEAD
alike), and every name is in `engine::audit_2026_05_22_tests`,
`orchestration`, `session*` or `channel_lease` — journal-lease and
crash-recovery tests, none in a media path. Run in isolation at HEAD:
`engine::audit_2026_05_22_tests` → **77 passed; 0 failed**;
`session::tests::test_f034_empty_session_gc` → **1 passed**. This is the
full-suite-contention class already in `.planning/BACKLOG.md`. Not caused by
this lane, not fixed by it.

`always_fails ... FAILED` appears inside `cargo test -p wcore-cli --lib` output.
It is a NESTED cargo run: `plugin/scaffold.rs:274` scaffolds a plugin whose
generated test is `panic!("deliberate")` on purpose. The outer result is
`1854 passed; 0 failed`.

## 9. Deliberate wording changes (two), both strengthened

| Test | Change | Why not a weakening |
|---|---|---|
| `transcription_tools::tests::missing_local_path_rejected` | accepted `"Failed to stat"`; now accepts `"not found"` **and additionally requires the refusal to name the path** | `Failed to stat` described an implementation step (`fs::metadata` by name) the unified path no longer performs. The new wording is the one the PDF and document surfaces already produce for the same condition, so a host matching on "not found" now gets one answer from every media surface instead of three. The assertion gained a clause. |
| `vision_tools::is_network_path_flags_unc_only` | moved to `media_intake::network_path_detection_flags_unc_and_nothing_else` | It asserted that `\\server\share\x.png` is NOT a network path on Unix. The consolidated intake refuses that spelling on **every** platform. The relocated test asserts both spellings on all platforms — a gained assertion. A pointer comment sits at the old site so the move is not silent. |

No test was deleted, `#[ignore]`d, `#[allow]`ed, re-gated, or had a timeout
raised.

## 10. One code fix found by a test, not by reading

`open_once` reported ENOENT on a non-leaf component as `OpenComponent`, so a
path with a missing PARENT said *"Cannot open audio path component"* where every
other media surface says *"not found"*. Fixed in the walk (ENOENT anywhere maps
to `NotFound`) rather than by relaxing the test.

## 11. Still open / not done

- **No Windows leg.** The UNC known-negative is the one that discriminates
  there, and it could not be run. Source-evidenced only.
- **No macOS leg**, and no PTY/TUI drive — the same two gaps `27-01` recorded.
  Nothing this lane changed is TUI-visible, but that is an argument, not a
  measurement.
- **`wcore-agent` full-lib flakiness** (§8) is untouched and belongs to
  BACKLOG.
- **`video_analyze` frame reads are unbounded** — MEDIUM, BACKLOG.
- **`Cargo.lock` drifts by 3 lines** (`wcore-config` gaining `libc`,
  `rusqlite`, `windows-sys`) on any build in this tree. **Pre-existing:** the
  base worktree produces the identical drift, and this lane changed zero
  manifest files (`git diff $BASE -- Cargo.toml 'crates/*/Cargo.toml'` → 0).
  Not committed.

## 12. Shared-file fence

`crates/wcore-cli/src/lib.rs` and `crates/wcore-cli/src/main.rs`: **untouched.**

```
BASE=$(git merge-base HEAD plan/f20-unified-audit-repair)   # 5457710e5bcc...
git diff "$BASE" -- crates/wcore-cli/src/lib.rs crates/wcore-cli/src/main.rs   # empty
```

No protocol seam, no contract change, nothing for the orchestrator to
serialize.

---

# RECONCILIATION — merged integration `fc7ecafb`, re-proved on the merged code

Integration moved `5457710e` → `fc7ecafb` during this lane. `lane/wal-followups`
(`29d1c882`, `76474a0d`) consolidated five divergent UNC checks onto
`crates/wcore-config/src/network_path.rs` and landed in **both** files my
consolidation centres on. Merged with `git merge`, **not** rebase (LANE-BRIEF
§0). Merge commit `751a2955`; reconciled HEAD `8a85913c`.

`git merge-tree --write-tree` against the tip confirmed the conflict set before
touching anything: exactly `media_intake.rs` and `vision_tools.rs`, 3 hunks each.

## What I dropped, because theirs was better

**My hand-rolled `media_intake::is_network_path` is gone.** I had strengthened
the base version by combining a `Component::Prefix` UNC match with a `\\` / `//`
string match. Their module's own defect table names `media_intake` as one of the
five disagreeing copies, and the row that matters is:

| input | `media_intake` (incl. mine) | `wcore_config::network_path` |
|---|---|---|
| `\\?\C:\Users\x` — verbatim path to a **local disk** | "network" | not network |

**My version carried that defect verbatim**, because I kept the `\\` string
prefix check. Their `has_unc_prefix` explicitly excludes `?` and `.` after the
double separator. `media_intake::is_unc_path` now delegates to theirs, and
their test
`a_verbatim_local_path_is_still_refused_but_no_longer_as_a_network_path`
passes on the merged tree — the input is **still refused**, as
`DeviceOrVerbatimPath`, which is the accurate reason. No previously-refused
input is now accepted.

I also agree with their cross-audit's reasoning and did not attempt a kernel
check: touching an attacker's UNC name to classify it is the dial-out the guard
exists to prevent. The syntactic answer is the complete answer to the question
being asked.

## What survived from mine

All of it, unchanged in behaviour: six surfaces reaching
`media_intake::admit_open`; `open_once` private so the one-path property stays
compiler-enforced; the `openat(O_NOFOLLOW)` + `O_NONBLOCK` walk covering all six
rather than only images; the extension-vs-bytes cross-check on `vision_analyze`;
the `RIFF` WAV/WebP fix; and `transcribe_audio` no longer reading deny-listed
credential paths.

## One place the merge made the result better than either lane's

Their `vision_tools::is_unc_path` is now **dead in this tree** and was removed.
`load_local_image` delegates to the chokepoint, so the UNC guard is applied
**once**, inside `admit_open`, still through their shared function — rather than
twice (once in `vision_tools`, once in `media_intake`). Their property is
strictly better served by the merged shape than by either branch alone.

Nothing was dropped silently: a comment at the old `vision_tools` site records
that `is_unc_path` and `open_local_image` moved into the chokepoint and why.

## No test from either lane was lost

Their `vision_tools::unc_guard_flags_unc_on_every_platform` lost its call site.
It was folded with my relocated `is_network_path_flags_unc_only` into a single
`media_intake::tests::unc_guard_flags_unc_on_every_platform` asserting the
**union**, not the intersection — including their `\\?\C:\` verbatim-local case.
Their two `media_intake` test additions were taken as-is (their
`refuses_a_unc_target…` edit is a superset of mine). Merged-tree run:

```
test media_intake::tests::a_verbatim_local_path_is_still_refused_but_no_longer_as_a_network_path ... ok
test media_intake::tests::refuses_a_unc_target_without_touching_the_filesystem ... ok
test media_intake::tests::unc_guard_flags_unc_on_every_platform ... ok
test result: ok. 20 passed; 0 failed; 0 ignored; 0 measured; 987 filtered out
```

## Re-proof on the merged code — all four artefacts

**1. The four known-negatives are still RED at the NEW base `fc7ecafb`.**
`wal-followups` did not cover them; the HIGH is live at the current integration
tip. (`evidence/27-c1/BASE-fc7ecafb-RED.txt`)

```
test result: FAILED. 10 passed; 4 failed; 0 ignored; 0 measured; 0 filtered out
    an_extension_that_contradicts_the_bytes_is_refused_at_both_surfaces
    audio_refuses_a_denylisted_credential_path
    audio_refuses_a_symlinked_leaf
    audio_refuses_a_traversal_segment
```

Base `fc7ecafb` still returns `("audio/wav", …)` for a file under a deny-listed
credential path. Merged HEAD: `14 passed; 0 failed; 0 ignored; 0 filtered out`.

**2. The `3 × cap` bound mutation still reddens BOTH surfaces on the merged
tree** (`evidence/27-c1/MUTATION-cap-removed-merged.txt`):

```
--- MUTATED (stat-side cap deleted from admit_open) ---
image_…_from_the_stat_not_from_the_read FAILED — got: Image too large …: 20971521 bytes
audio_…_from_the_stat_not_from_the_read FAILED — got: Audio too large: 26214401 bytes
test result: FAILED. 12 passed; 2 failed; 0 ignored; 0 measured; 0 filtered out
```

Both mutated numbers are again exactly `cap + 1` — the value my pre-repair
fixture searched for. The instrument repair survived the merge, which is
precisely where it could have quietly come back.

**3. The one-path gate is still non-vacuous across the merge**
(`evidence/27-c1/GATE-one-path-merged.txt`):

```
BASE fc7ecafb : GATE: FAIL   BASE_RC=1
MERGED HEAD   : GATE: PASS   HEAD_RC=0
```

At `fc7ecafb` it still fails with the right diagnosis — 3 surfaces opening their
own files, 1 of 6 reaching the chokepoint, `open_once` absent, 2 magic tables.

**4. All six live audio refusals reproduce off the wire on the merged binary**
(`evidence/27-c1/LIVE-OBS-RAW-merged.log`, `wayland-core 0.12.25`):

| Observation | `tool_result` on the wire |
|---|---|
| valid WAV | `groq transcription returned HTTP 401` — **ADMITTED** |
| **deny-listed** | `Invalid path: path targets a denied system location: ".../.ssh/id_rsa"` |
| traversal | `Invalid path: path contains traversal (..)` |
| symlinked leaf | `Cannot open audio path component …: Too many levels of symbolic links (os error 40)` |
| 3× cap sparse | `Audio too large: 78643200 bytes (limit 26214400 bytes)` |
| relative | `Invalid path: path must be absolute` |

Composer unchanged: `valid-image.png` → `IMAGE part media_type=image/png
data_len=100`; both refusals reach the user as `engine_error` with **0 captured
provider requests**. Secret sweep over the whole evidence tree: `0`.

## Merged-tree gate state

| Gate | Result |
|---|---|
| `cargo fmt --all -- --check` | clean |
| `cargo clippy -p wcore-tools -p wcore-agent -p wcore-cli --all-targets --all-features -- -D warnings` | **clean, exit 0** |
| `cargo test -p wcore-tools` | **1004 passed; 0 failed; 3 ignored; 0 filtered out** (was 998 pre-merge; +6 from both lanes' new tests) |
| `media_intake_unification_test` | **14 passed; 0 failed; 0 ignored; 0 filtered out** |
| `cargo test -p wcore-cli --lib` | **1858 passed; 0 failed; 1 ignored** |
| `cargo test -p wcore-agent --lib channel_media` | **14 passed; 0 failed** |
| `cargo test -p wcore-config --lib network_path` | **6 passed; 0 failed** — their module, unbroken |
| `cargo build --release -p wcore-cli` | exit 0 |

Shared-file fence re-checked against the **new** merge-base: `git diff
$(git merge-base HEAD gh/plan/f20-unified-audit-repair) -- crates/wcore-cli/src/lib.rs
crates/wcore-cli/src/main.rs` → empty.

`cargo test -p wcore-agent --lib` remains the pre-existing contention flake
(§8); noted by the coordinator as not mine to take, along with the
`video_analyze` unbounded frame read.
