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
