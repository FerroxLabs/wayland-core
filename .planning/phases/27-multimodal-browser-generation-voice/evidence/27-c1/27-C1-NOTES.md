# 27-C1 — one bounded intake path — working notes

Lane `27-c1-intake`. Base: `plan/f20-unified-audit-repair` @ `5457710e`.
Criterion 1: *"Standalone and host messages use one bounded, validated
attachment/document intake path and degrade explicitly on unsupported
providers."* Graded PARTIAL by `27-PHASE-VERDICT.md`; the unmet clause is the
word **one**.

Append after every measurement. Do not batch.

---

## M0 — the inherited claim I was told to test, not inherit

`.planning/CRITERIA-GAP-LEDGER.md:677`:

> `27-C1` — "one intake path" is an architecture criterion over code that was
> **measured already correct** on the duplicated paths. Real value, zero defect
> value.

That rests on `27-01-INTAKE-AUDIT.md` §3, which enumerates **four** intake
paths: composer/vision, PDF, office documents, channel enricher.

**First finding: the enumeration is incomplete, so "measured already correct"
was never measured over the whole surface.** See M1.

## M1 — path census at base (source read, hetzner measurement pending)

| # | Path | Entry | Resolutions before read | Path validation | Size bound | Format admission |
|---|---|---|---|---|---|---|
| 1 | composer / host attachments | `wcore-cli/src/attachments.rs:61` → `vision_tools::load_local_image` | 1 (`openat` component walk, `O_NOFOLLOW`+`O_NONBLOCK`) | `validate_user_path` + UNC refusal | `VISION_MAX/MIN_BYTES`, `take()` bounded | `detect_image_mime` off read bytes |
| 2 | `vision_analyze` tool, local arg | `vision_tools::resolve_source` → same `load_local_image` | same as 1 | same as 1 | same as 1 | same as 1 |
| 3 | `vision_analyze` tool, URL arg | `resolve_source` → `ImageFetcher::fetch` → `validate_image_bytes` | n/a | SSRF `is_safe_url` + website policy | post-fetch only | `detect_image_mime` |
| 4 | PDF tool | `pdf_tool.rs:49` → `media_intake::admit_path` | 1 (`File::open`) | `validate_user_path` | `MAX_PDF_INGEST_BYTES` from descriptor stat | magic + extension cross-check |
| 5 | office documents | `doc_tool.rs:215/385` — own open-once sequence | 1 | `validate_user_path` | `MAX_ON_DISK_BYTES` | OOXML container signature |
| 6 | channel enricher | `wcore-agent/src/channel_media.rs` | 0 (connector supplies bytes) | n/a | `VISION_*` / `TRANSCRIPTION_*` inline | `detect_image_mime` / `detect_audio_mime` |
| 7 | **`transcribe_audio`, local path** | `transcription_tools.rs:368-385` | **2 by-name** (`fs::metadata` then `fs::read`) | **NONE** | stat-only; the read itself is **unbounded** | `detect_audio_mime` after full read |
| 8 | `transcribe_audio`, URL arg | `transcription_tools.rs:354` → `AudioFetcher` | n/a | `validate_audio_url` | post-fetch only | `detect_audio_mime` |
| 9 | video analyze, local/remote | `wcore-agent/src/tool_backends/video_analyze.rs:115/235` | own `validate_local_path`; remote capped at `REMOTE_VIDEO_MAX_BYTES` | own | own | own |

**Nine, not four.** The prior audit's four-path table omitted audio (7, 8) and
video (9) entirely.

## M2 — the chokepoint has one caller and a dead half

`/usr/bin/grep -rn "media_intake" --include="*.rs" crates/` → **3 lines**:
`pdf_tool.rs:20` (doc comment), `pdf_tool.rs:49` (the only `use`),
`lib.rs:93` (module registration).

So `media_intake::admit_path` has **one** production caller, and
`media_intake::admit_bytes` has **zero** — it is exercised only by its own unit
test. A chokepoint with one caller is not a chokepoint; `admit_bytes` is a
declared surface that enforces nothing on any real input, which is the same
shape as the decorative bounds lane 24 removed.

Known-positive control for that grep in the same invocation:
`/usr/bin/grep -c "pub fn" crates/wcore-tools/src/media_intake.rs` → **7**.
The instrument returns non-zero on a term that is present, so the 3-line result
is a measurement and not a dead grep (LANE-BRIEF §3b-i).

## M3 — the inherited claim is REFUTED at path 7 (pending executable proof)

`transcription_tools.rs:368-385`, `AudioSource::Path`:

```rust
let meta = std::fs::metadata(path)...;      // resolution 1, by name
if !meta.is_file() { ... }                  // decided on resolution 1
if (meta.len() as usize) > TRANSCRIPTION_MAX_BYTES { ... }  // decided on resolution 1
std::fs::read(path)...                      // resolution 2, by name, UNBOUNDED
```

Four defects, all in the class Phase 27 already fixed once on the PDF path:

- **D-C1-1** the path is never `validate_user_path`'d. Every other local-file
  intake in the tree calls it. `AudioSource::Path(PathBuf::from(p))` at
  `:474` takes the model-supplied string verbatim.
- **D-C1-2** two independent by-name resolutions; the bytes read are not
  provably the bytes stat'd (this is D2 from `27-01`, unfixed here).
- **D-C1-3** the read is `fs::read`, not a bounded `take()`. The 25 MB cap is
  enforced against resolution 1 only, so it is **decorative** on the read that
  actually happens — the exact "declared limit that enforces nothing" shape
  lane `24-media-bounds` was chartered against.
- **D-C1-4** no UNC/network-path refusal, unlike paths 1 and 4.

If this holds under test, the ledger's "measured already correct on the
duplicated paths / zero defect value" is **false**, and C1 carries real defect
value. The prior assessment did not survive because it was scoped to a census
that omitted the defective path.

**Status: source-read only. Executable proof next.** Nothing above is claimed
as measured until a test at HEAD fails.

## Next

1. Write a known-negative for D-C1-3 (an over-cap audio file admitted through
   resolution 2) and show it RED at base.
2. Consolidate 7 (and 1/4/5 where the mechanism differs) onto one bound-
   enforcing intake.
3. Show the known-negative goes red again when the enforcement is removed.

---

## M4 — the claim is REFUTED, measured

Base `5457710e`, hetzner `/root/wayland-27c1-base`, new suite
`crates/wcore-tools/tests/media_intake_unification_test.rs`:

```
test result: FAILED. 10 passed; 4 failed; 0 ignored; 0 measured; 0 filtered out
```

The four RED (full capture: `BASE-5457710e-RED.txt`):

| Test | Base behaviour |
|---|---|
| `audio_refuses_a_traversal_segment` | returned `Ok(("audio/wav", ...))` for a path carrying `..` |
| `audio_refuses_a_symlinked_leaf` | followed the symlink and returned the target's bytes |
| `audio_refuses_a_denylisted_credential_path` | **READ a file under a deny-listed credential path and returned its bytes as `audio/wav`** |
| `an_extension_that_contradicts_the_bytes_...` | `vision_analyze` admitted a PNG named `.jpg` — the cross-check lived only in the composer's private table |

So `.planning/CRITERIA-GAP-LEDGER.md:677` — *"measured already correct on the
duplicated paths. Real value, zero defect value"* — **does not survive.** It
was scoped to a four-path census that omitted the defective surface.

Two tests passed at base **for the wrong reason** and are recorded as such
rather than counted: `audio_refuses_a_relative_path` and
`audio_refuses_a_unc_target` both failed with ENOENT on Linux, not with a
policy refusal. The UNC one discriminates only on Windows, which this lane did
not run.

## M5 — my own bound gate was self-passing, and is repaired

First version of the bound known-negative used a `cap + 1` fixture. Deleting
the stat-side cap from `admit_open` left it **green** — because the
defence-in-depth check after `take(cap + 1)` refuses citing the same number.
Repaired with a `3 × cap` sparse fixture, which makes the two enforcement
points report different numbers, plus a third assertion pinning that the
read-side number did NOT appear. Re-mutated: both surfaces now go RED, citing
`26214401` / `20971521` — i.e. exactly `cap + 1`, the string the broken
assertion was searching for. Full two-round capture:
`MUTATION-cap-removed.txt`.

## M6 — measured exclusion: `video_analyze` is not an intake path

`wcore-agent/src/tool_backends/video_analyze.rs` never ingests the caller's
bytes — it hands the path to `ffmpeg` as a subprocess argument, so its
discipline is argv-injection defence plus a realpath whitelist, which
`media_intake` neither provides nor should. The frames it then reads are
ffmpeg-produced files in an engine-owned tempdir with a hardcoded
`image/jpeg`: engine-produced bytes, no caller-supplied name. Excluded from
the consolidation deliberately, recorded rather than dropped. The unbounded
`tokio::fs::read` on those frames is a MEDIUM for BACKLOG, not this criterion.
