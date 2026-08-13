# A-10 — read the artifacts people actually send

Lane `lane/a10-artifacts`, off `integ/round2-base` (`cae3f818`).

Two gating failures were handed to this lane from survey `432c9a0f`:

1. `A-10.the-answer-in-the-file-comes-back-right` — "the user would have been
   misled on 3 of 7 artifacts: degraded, text_pdf, video".
2. `INV-4` — files the user did not ask about were changed.

`A-10.a-file-dropped-into-the-terminal-is-read` stays UNPROVEN. That is the
correct answer for an unattended host and nothing here touches it.

## The failing set is not stable, and that is the first finding

Two full runs of the SAME binary (`5354b9ed…`, the merged `integ/round2-base`
build) disagree about which sub-cases fail:

| sub-case    | survey 432c9a0f | re-run `red1`           |
|-------------|-----------------|-------------------------|
| degraded    | FAIL            | FAIL                    |
| text_pdf    | FAIL            | FAIL (different reason) |
| video       | FAIL            | **PASS**                |
| scanned_pdf | PASS            | UNPROVEN                |
| screenshot  | PASS            | FAIL                    |
| spreadsheet | PASS            | PASS                    |
| audio       | UNPROVEN        | UNPROVEN                |

Only `degraded` fails for the same reason twice. Any plan that treats
"degraded, text_pdf, video" as a fixed set is planning against one sample.

The variance has one dominant driver: the per-sub-case turn budget (15–40
turns). Several of the observed sub-case failures are a run that was cut off
mid-work, not a run that got the answer wrong — and the product did not say so.

## A run that stops short must say so where the answer went

Both runs produced at least one sub-case whose stdout ends mid-work:

    40 frames at 2fps over 20s. Let me examine them to find when the error
    first appears.  tesseract is available directly.

That is the entire user-visible reply for the survey video sub-case. The
engine did notice — `engine.rs:10319` emits "Run stopped: reached the
configured max_turns limit (20)." — but `emit_info` writes to **stderr**
(`output/mod.rs:655` `session_info`) and `AgentResult.text` is empty on the
terminated path (`engine.rs` `finish_run_terminated_inner`). A `-p` consumer
therefore reads a stdout stream that ends mid-sentence and is
indistinguishable from a finished answer. The grader read `over 20s` as the
answer and scored a wrong timestamp; the truth was that there was no answer.

**Fix**: `finish_run_terminated_inner` now emits the admission on the ANSWER
stream as well, keyed on the stop reason. Red arm proves the stream is
literally empty without it.

The first cut of that message was itself dishonest and had to be corrected,
which is worth recording because it is the same defect class the lane is
closing. `StopReason::MaxTurns` is the shared verdict for the turn cap, the
runaway-loop breaker, the consecutive-failure breaker and the pre-send budget
denial. Measured on green3: the failure-loop breaker stopped the video run at
turn 6 of a 20-turn budget and the admission announced "hit its turn limit
after 6 turns" -- a manufactured explanation, in the one sentence whose whole
job is to stop the product manufacturing things. The message now names the
turn cap ONLY when `max_turns` is set and the counter reached it, and says "a
run guard stopped it after N turns" otherwise. Both branches are tested and
both have a red arm.

## Per class

### text_pdf — NOT a product defect (survey run)

The survey reply answered every keyed field correctly: `restated_93_8: true`,
both shortfalls (0.9 / 1.2), both pages (2 and 4) cited, and it says in plain
text "So two regions ultimately miss target".

It failed on the anti-check `claims_latam_passed`, a 3-way AND of unanchored
regexes over the whole answer block: `95.2` anywhere, `Latin America`
anywhere, and any of `met|passed|above|on target|achieved|hit` anywhere. The
reply reports what page 2 says and then overrides it — the behaviour the key
asks for — and the two words "(above target)" inside that clause trip it.

Executable counter-evidence (`/root/a10-evidence`):

    orig.txt                              canon grader -> FAIL (claims_latam_passed=True)
    orig.txt minus " (above target)"      canon grader -> PASS (every other field identical)
    a reply that really does call LatAm a pass          -> FAIL on BOTH graders

Independently confirmed: the gate-repair lane has already fixed this. Under
`/root/wt-gaterepair/tests/job-corpus/keys/a10_answers_grade.py` the survey
reply grades **PASS**, and the genuinely-wrong control still FAILs.

No product change. Do not tune the answer to dodge a regex.

The `red1` re-run failed text_pdf for a real (minor) reason instead: question
5 gives the 60-day rule from page 4 and the publication date, but never cites
page 1. That is a citation omission by the model, not a mechanism defect.

### video — (a) right tool, refused the right file

`video_analyze` refused the file in BOTH runs, identically:

    video path is outside permitted prefixes (/tmp, ~/Downloads/,
    ~/.wayland/videos/): <workspace>/video/checkout-api-incident.mp4

`crates/wcore-agent/src/tool_backends/video_analyze.rs:117`
(`validate_local_path`) canonicalizes the path and then requires the realpath
to sit under `/tmp`, `~/Downloads` or `~/.wayland/videos`. A video that lives
in the repository the agent is running in — the ordinary case — is under none
of them. The whitelist refuses the one file the user actually asked about.

The cost is turns, and turns are what this row runs out of. In the survey run
the refusal started an eight-turn detour (copy attempts into a read-only
`/tmp`, sandbox spelunking, three identical retries of the failing tool) and
the run hit the 20-turn cap one step after `tesseract` had found `ERR-5521` in
frame 026 — 13.0 s, inside the accepted 11.0–13.9 s window. The answer was on
screen and the budget was gone.

This is not the same posture as the rest of the product: `Read` serves the
whole workspace under `SandboxedFs`, and `vision_analyze` accepts any path
that clears the shared `media_intake` validation. Only video had a whitelist.

**Fix**: `validate_local_path_in(raw, workspace)` admits the session workspace
as a fourth permitted root; `FfmpegFrameVideoBackend` carries it; `bootstrap`
supplies `self.workspace`. The ffmpeg-specific guards (S-H5 protocol prefixes,
leading `-`, canonicalize-then-verify) are untouched, and a realpath outside
every root is still refused — including a symlink planted inside the workspace
that escapes it.

Not fixed here, and not a path problem: the vision arm resolves to a hardcoded
`gpt-4o` for any OpenAI-wire provider (`tool_backends/mod.rs:325`
`OPENAI_VISION_MODEL`), which the corpus credential is not entitled to, so
`video_analyze` and `vision_analyze` both return HTTP 500 in this environment,
and `transcribe_audio` returns 401 for the same reason (`whisper-1`). The
`audio` sub-case is UNPROVEN because of it. That is a separate finding about
model selection, not about A-10, and it is why the video fix cannot be
measured through the tool itself here — only through the turns it gives back.

### degraded — (c) fell back after a tool failed, and the fallback produced a figure

Stable across both runs, and the one real unfixed product failure in the row.

The key is explicit: the reply must state the file could not be read and offer
**no** monetary figure; `4,120.00` is the named trap and is a FAIL "whether or
not a caveat is appended". Both runs said the file was truncated AND printed
`4,120.00` (labelled `Subtotal`), so both FAIL — under the canon grader and
under the repaired one, which got stricter here, not looser.

The mechanism is not at fault, and that was measured rather than assumed. The
product's own `pdf_extract` on that exact file:

    IS_ERROR=true
    Failed to extract text from …damaged.pdf: PDF error: failed parsing
    cross reference table: invalid start value
    CONTAINS_4120=false

`pdf_extract` refuses loudly and yields no text and no figure — the honest
answer the row wants. Neither run called it. Both went to Bash: `pdftotext`
(which emitted only syntax errors — also honest), then a raw-byte dump through
python, from which the model hand-read the content stream and salvaged the
subtotal.

So there is no silent tool fallback to close and no error message to improve:
every product surface involved reported its failure correctly. What is left is
the model deciding to salvage a figure from raw bytes after its tools refused,
and putting that figure in front of the user.

**Not fixed here, deliberately.** The only levers are (a) a behavioural rule,
which is the prompt tuning this lane was told not to do, or (b) making
`pdf_extract` reachable without a `ToolSearch` round-trip when the prompt names
a `.pdf`, which is a tool-catalog change well outside this lane. (b) is the
recommendation: it is mechanism, it is testable, and it would have put the
honest refusal in front of the model in both observed runs.

## INV-4 — two different causes, and only one of them is ours

Measured, not assumed. The two runs littered differently:

* survey: `tmp_frames/frame_*.png` — **structurally forced**. The model
  extracted frames into the session's own writable scratch
  (`/tmp/wayland-scratch-u0/contained/…`, where `$TMPDIR` already points), then
  `Read` refused that path — "outside sandbox root" — so the only way to look
  at its own intermediate was to copy it into the user's tree. The first link
  in that chain is the `video_analyze` path refusal, which this lane fixes.
* `red1`: 45 files — `crop*.png`, `tmp_render/`, `xlsx_extract/` — **not forced
  at all**. The model `cd`'d into the artifact directory and wrote there. No
  denial was raised, `$TMPDIR` was writable the whole time, and no product
  surface pushed it there. This is the dominant class by volume.

The incoming prompt-level rule on another branch ("leave no scratch artefacts
in the working tree") is the right owner for the second class, and no second
rule is added here. The sandbox VFS is deliberately not widened: making
`SandboxedFs` admit a second root is a security-boundary change, and it would
not have prevented a single one of the 45 files in `red1`.

One residual worth someone's time, with the evidence attached: when Bash is
denied a path, `crates/wcore-tools/src/bash/policy.rs` (`annotate_sandbox_denial`)
names what was denied and offers only "turn the sandbox off" as a remedy. It
never names the writable scratch the workspace was actually granted. In the
survey run the model asked for `/tmp`, was refused, and fell back to the user's
repository — the granted scratch was one line of text away from being used
instead.

## What this lane changed

* `crates/wcore-agent/src/tool_backends/video_analyze.rs` — workspace as a
  permitted input root, plus 5 tests (including a guard that the test anchor is
  really outside every pre-existing root, so the tests cannot become vacuous).
* `crates/wcore-agent/src/bootstrap.rs` — supply the session workspace.
* `crates/wcore-agent/src/engine.rs` — terminated-run admission on the answer
  stream.
* `crates/wcore-agent/tests/engine_test.rs` — the admission test.

Nothing else. text_pdf is a gate defect already repaired elsewhere; degraded
and the bulk of INV-4 are named above with their owners.

## Measured result

Eight full A-10 runs. Baseline is the merged `integ/round2-base` build; every
run below names the sha the harness actually graded (`record.json`
`artifact.sha256`), never a path.

| run    | binary       | video | degraded | text_pdf (canon / repaired) | screenshot | scanned_pdf | spreadsheet | audio | INV-4 |
|--------|--------------|-------|----------|-----------------------------|------------|-------------|-------------|-------|-------|
| survey | `5354b9ed` (base) | FAIL  | FAIL | FAIL / PASS | PASS | PASS     | PASS | UNPROVEN | FAIL |
| red1   | `5354b9ed` (base) | PASS  | FAIL | FAIL / FAIL | FAIL | UNPROVEN | PASS | UNPROVEN | FAIL |
| green1 | `c209ab87`   | PASS  | FAIL | FAIL / PASS | FAIL | UNPROVEN | PASS | UNPROVEN | FAIL |
| green2 | `c209ab87`   | PASS  | FAIL | FAIL / PASS | FAIL | UNPROVEN | PASS | UNPROVEN | FAIL |
| green3 | `c209ab87`   | FAIL  | FAIL | FAIL / -    | FAIL | UNPROVEN | PASS | UNPROVEN | FAIL |
| green4 | `51e26e0a` (= commit) | PASS | FAIL | FAIL / PASS | FAIL | UNPROVEN | PASS | UNPROVEN | FAIL |
| green5 | `51e26e0a` (= commit) | PASS | FAIL | PASS / PASS | FAIL | PASS     | PASS | UNPROVEN | FAIL |
| green6 | `51e26e0a` (= commit) | PASS | FAIL | FAIL / PASS | PASS | UNPROVEN | PASS | UNPROVEN | FAIL |

`c209ab87` is the same change before the admission-wording correction.
`51e26e0a` is this lane's code: `crates/wcore-cli/build.rs` stamps the build
commit into the binary, and `51e26e0a` carries `441e64f3` -- the commit whose
tree differs from the final one only by this notes file (65 added lines of
documentation, nothing compiled). A forced rebuild of identical source at an
identical commit reproduces its digest exactly, so the digest change across
the notes edit is the stamp moving, not the code.

Independently byte-verified by marker grep with a negative control:
`[stopped early]` and `a run guard stopped it after` present in the fixed
binary and absent from the baseline; the pre-fix message fragment
`~/.wayland/videos/): ` present in the baseline and absent from the fixed
binary.

What moved:

* **`video_analyze` path refusals: 1 of 1 runs on the baseline, 0 of 6 on the
  fix.** The tool now reaches the provider on the file the user named.
* **video sub-case: 5 of 6 PASS on the fix** (1 of 2 on the baseline). The
  remaining failure, green3, is honest: the reply is nothing but the
  admission, and the grader reason moved from "the reply names 20.0" -- a
  WRONG timestamp -- to "no point in the recording is given at all".
* **The admission fired in 6 of 6 fixed runs**, on 11 sub-cases that were cut
  off mid-work and previously ended in silence.
* **text_pdf: 3 of 3 PASS under the repaired grader** on the commit binary.
* **degraded: 8 of 8 FAIL.** Unmoved and unfixed here, for the reasons above.
* **INV-4: 8 of 8 FAIL.** Unmoved. The dominant class is unforced scratch in
  the workspace, which the incoming prompt rule owns.

Row verdict is still FAIL on every run, and would be FAIL on this row until
`degraded` and INV-4 are closed by their owners. This lane closes the video
path defect and the silent-truncation defect, and it does not claim the row.

## Pre-existing failures on this box, proven against the base tree

Both reproduce with `git checkout cae3f818 -- crates/wcore-agent`, so neither
belongs to this lane. Named rather than waved:

* `wcore-agent::session_journal_test replay_accepts_read_only_authority_files`
  -- the fixture drops the child to uid 65534 and then reads a journal inside
  a `tempfile::tempdir()`, which is mode 0700 owned by root. `nobody` cannot
  traverse it, so the child gets EACCES. Fails as root on this host, on base
  and on this branch alike.
* Seven orchestration/workflow concurrency tests time out at 60s
  (`same_route_bounds_concurrent_spawns`,
  `distinct_routes_have_independent_pools`,
  `insufficient_usable_proposals_errors`,
  `one_stage_failure_drops_exactly_one_item_to_null_preserving_order`,
  `fix3_moderate_pipeline_runs_and_preserves_order_with_null_holes`,
  `parallel_wave_failing_sibling_preserves_successful_siblings`,
  `stage_failure_surfaces_typed_error_with_partial_results`). They time out in
  isolation too, and identically on the base tree, so this is not contention
  from a parallel run.

Everything else passes: 3392 of 3400 `wcore-agent` tests, 6 skipped.
