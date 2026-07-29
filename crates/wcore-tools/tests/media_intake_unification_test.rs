//! Phase 27 Criterion 1 — **one** bounded, validated media intake path.
//!
//! Every test here is a KNOWN-NEGATIVE: an input that must be REFUSED. They
//! are written against the public tool surfaces, not against
//! `media_intake` internals, because the criterion is about what the shipped
//! surfaces do — a module-level test would pass on a chokepoint nobody calls.
//!
//! # Which of these were RED at the phase-27 base, and why that matters
//!
//! The `27-C1` entry in `.planning/CRITERIA-GAP-LEDGER.md` argued the criterion
//! was *"an architecture criterion over code that was measured already correct
//! on the duplicated paths — real value, zero defect value."* That rested on a
//! census of FOUR intake paths which omitted audio entirely. The
//! `transcribe_audio` local-file path applied NO path validation at all: no
//! absolute-path requirement, no traversal refusal, no UNC refusal (an
//! outbound SMB connect and a NetNTLM-hash leak on Windows, the exact defect
//! `#644` closed everywhere else in this tree), no system-path deny-list, and
//! no symlink discipline — then read with an unbounded `fs::read` issued
//! against a SECOND by-name resolution.
//!
//! The `audio_*` tests below were RED at base. The matching `image_*` tests
//! were GREEN at base and are kept as the KNOWN-POSITIVE control: they prove
//! the instrument can distinguish the two, so a green `audio_*` is a
//! measurement rather than a dead test.
//!
//! Baseline capture:
//! `.planning/phases/27-multimodal-browser-generation-voice/evidence/27-c1/`.

use std::path::PathBuf;

use wcore_tools::transcription_tools::{
    AudioSource, NullAudioFetcher, TRANSCRIPTION_MAX_BYTES, TranscribeAudioTool,
};
use wcore_tools::vision_tools::load_local_image;

/// A minimal, valid WAV header — `RIFF....WAVE` — padded past the 16-byte
/// minimum so nothing is refused for being too small.
fn wav_bytes() -> Vec<u8> {
    let mut v = b"RIFF\x24\x08\x00\x00WAVEfmt ".to_vec();
    v.extend_from_slice(&[0u8; 32]);
    v
}

fn png_bytes() -> Vec<u8> {
    let mut v = b"\x89PNG\r\n\x1a\n".to_vec();
    v.extend_from_slice(&[0u8; 32]);
    v
}

/// A tool wired to the null backend. Every test here fails in `resolve_source`,
/// which runs before the backend is ever consulted, so no backend is needed.
fn audio_tool() -> TranscribeAudioTool {
    TranscribeAudioTool::new(
        std::sync::Arc::new(wcore_tools::transcription_tools::NullTranscriptionBackend),
        std::sync::Arc::new(NullAudioFetcher),
    )
}

fn resolve(path: PathBuf) -> Result<(&'static str, Vec<u8>), String> {
    futures::executor::block_on(audio_tool().resolve_source(&AudioSource::Path(path)))
}

// ── path validation: the whole boundary the audio path was missing ─────────

/// Every other file-reading tool in this tree requires an absolute path.
/// `transcribe_audio` took the model-supplied string verbatim into
/// `PathBuf::from`.
///
/// **Honest note: this one passed at base, and for the WRONG reason** — the
/// relative name does not exist relative to the test's cwd, so base failed with
/// ENOENT rather than with a path-policy refusal. It is retained as a
/// regression pin, NOT counted as one of the four measured RED results. The
/// discriminating absolute-path test is `audio_refuses_a_traversal_segment`,
/// whose target file genuinely exists.
#[test]
fn audio_refuses_a_relative_path() {
    let err = resolve(PathBuf::from("some/relative/clip.wav"))
        .expect_err("a relative path must be refused");
    assert!(
        !err.is_empty(),
        "refusal must carry a reason, got an empty string"
    );
}

/// KNOWN-POSITIVE control: the image surface already did this at base. If this
/// ever goes red the instrument is broken, not the audio path.
#[test]
fn image_refuses_a_relative_path_control() {
    assert!(
        load_local_image("some/relative/pic.png").is_err(),
        "control: the image path has always refused a relative path"
    );
}

/// KNOWN-NEGATIVE. **Measured RED at base** (`evidence/27-c1/BASE-*-RED.txt`):
/// base returned `Ok(("audio/wav", ...))` for a path carrying a `..`
/// component.
#[test]
fn audio_refuses_a_traversal_segment() {
    let dir = tempfile::tempdir().unwrap();
    let target = dir.path().join("clip.wav");
    std::fs::write(&target, wav_bytes()).unwrap();
    // An absolute path that still carries a `..` component. The file it names
    // EXISTS and is a perfectly valid WAV, so nothing but the traversal check
    // can refuse it — which is what makes this a sharp negative.
    let via_traversal = dir.path().join("sub").join("..").join("clip.wav");
    std::fs::create_dir(dir.path().join("sub")).unwrap();
    let err = resolve(via_traversal).expect_err("a `..` component must be refused");
    assert!(!err.is_empty());
}

#[test]
fn image_refuses_a_traversal_segment_control() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("pic.png"), png_bytes()).unwrap();
    std::fs::create_dir(dir.path().join("sub")).unwrap();
    let via_traversal = dir.path().join("sub").join("..").join("pic.png");
    assert!(load_local_image(via_traversal.to_str().unwrap()).is_err());
}

/// On Windows, opening a UNC target triggers an outbound SMB connect that leaks
/// a NetNTLM hash to an attacker-chosen host BEFORE any content check. `#644`
/// closed this on every other file surface; the audio path never had it.
///
/// **Honest note: this passed at base ON LINUX, and for the wrong reason** —
/// `\\attacker\share\clip.wav` is a relative filename on Unix, so base failed
/// with ENOENT. The platform where this negative discriminates is Windows,
/// which this lane could not run. Recorded as source-evidenced, not measured
/// on the platform that matters.
#[test]
fn audio_refuses_a_unc_target() {
    for spelling in [r"\\attacker\share\clip.wav", "//attacker/share/clip.wav"] {
        let outcome = resolve(PathBuf::from(spelling));
        assert!(
            outcome.is_err(),
            "a UNC target must be refused before any open: {spelling} was ADMITTED"
        );
    }
}

/// KNOWN-NEGATIVE. **Measured RED at base**, and the most serious of the four:
/// base READ a file at a deny-listed credential path and returned its bytes as
/// `("audio/wav", ...)`, ready to be posted to a third-party speech-to-text
/// provider. `path_validation`'s deny-list — which every other file tool in
/// this tree consults — was never called on this path.
#[test]
fn audio_refuses_a_denylisted_credential_path() {
    let dir = tempfile::tempdir().unwrap();
    let ssh = dir.path().join(".ssh");
    std::fs::create_dir(&ssh).unwrap();
    let secret = ssh.join("id_rsa");
    // Real WAV bytes under a deny-listed name: only the deny-list can refuse
    // this, so a pass here is not the format check doing the work.
    std::fs::write(&secret, wav_bytes()).unwrap();
    let err = resolve(secret).expect_err("a deny-listed path must be refused");
    assert!(!err.is_empty());
}

#[test]
fn image_refuses_a_denylisted_credential_path_control() {
    let dir = tempfile::tempdir().unwrap();
    let aws = dir.path().join(".aws");
    std::fs::create_dir(&aws).unwrap();
    let secret = aws.join("credentials");
    std::fs::write(&secret, png_bytes()).unwrap();
    assert!(load_local_image(secret.to_str().unwrap()).is_err());
}

/// KNOWN-NEGATIVE. **Measured RED at base**: the image path walked with
/// `O_NOFOLLOW`; the audio path followed symlinks freely and returned the
/// target's bytes.
#[cfg(unix)]
#[test]
fn audio_refuses_a_symlinked_leaf() {
    use std::os::unix::fs::symlink;
    let dir = tempfile::tempdir().unwrap();
    let target = dir.path().join("real.wav");
    std::fs::write(&target, wav_bytes()).unwrap();
    let link = dir.path().join("link.wav");
    symlink(&target, &link).unwrap();
    let err = resolve(link).expect_err("a symlinked leaf must be refused");
    assert!(!err.is_empty());
}

// ── the bound ──────────────────────────────────────────────────────────────

/// Build a sparse fixture that is `multiple` times the cap, with a real
/// container header at the front so the FORMAT check can never be what does
/// the refusing. Sparse, so it costs ~0 bytes on disk.
fn oversize_fixture(path: &std::path::Path, header: &[u8], len: u64) {
    use std::io::{Seek, SeekFrom, Write};
    let f = std::fs::File::create(path).unwrap();
    f.set_len(len).unwrap();
    drop(f);
    let mut f = std::fs::OpenOptions::new().write(true).open(path).unwrap();
    f.seek(SeekFrom::Start(0)).unwrap();
    f.write_all(header).unwrap();
    assert_eq!(std::fs::metadata(path).unwrap().len(), len);
}

/// **THE bound-enforcement known-negative.**
///
/// An input that exceeds `TRANSCRIPTION_MAX_BYTES` must be REFUSED, and the
/// refusal must cite the file's FULL length — a number only obtainable from
/// the descriptor's own metadata, before any payload read.
///
/// # This test was itself self-passing once, and the repair is the point
///
/// The first version used a fixture of exactly `cap + 1`. Deleting the
/// stat-side cap from `media_intake::admit_open` did NOT turn it red, because
/// the defence-in-depth check after the bounded `take(cap + 1)` refuses with
/// `actual = cap + 1` — the SAME number the stat would have reported. The
/// assertion could not tell the two enforcement points apart, so it passed on
/// an instrument with the enforcement under test removed. That is exactly the
/// self-passing class LANE-BRIEF §3.2 names, found in this lane's own gate.
///
/// The repair is the fixture size: at `3 × cap` the two enforcement points
/// report DIFFERENT numbers — the stat reports `3 × cap`, the read-side
/// fallback can only ever report `cap + 1`. Asserting the full length
/// therefore discriminates.
///
/// Three assertions, per LANE-BRIEF §6b-ii:
/// 1. known-positive — an under-cap file is admitted (`a_valid_wav_...`);
/// 2. known-negative — this oversize file is refused;
/// 3. **the old broken assertion would have missed it** — the refusal must
///    NOT cite `cap + 1`, which is the only number the un-repaired test could
///    ever have seen and the number the read-side fallback produces.
///
/// Mutation-proved: deleting the stat-side cap turns this red. See
/// `evidence/27-c1/MUTATION-cap-removed.txt`.
#[test]
fn audio_over_the_declared_cap_is_refused_from_the_stat_not_from_the_read() {
    let dir = tempfile::tempdir().unwrap();
    let over = dir.path().join("over-cap.wav");
    let cap = TRANSCRIPTION_MAX_BYTES as u64;
    let oversize = cap * 3;
    oversize_fixture(&over, &wav_bytes(), oversize);

    let err = resolve(over).expect_err("a file over the declared cap must be refused");
    assert!(
        err.contains("too large"),
        "refusal must name the size, got: {err}"
    );
    assert!(
        err.contains(&oversize.to_string()),
        "refusal must cite the FULL length {oversize} taken from the descriptor's \
         own metadata BEFORE any payload read; got: {err}"
    );
    assert!(
        !err.contains(&(cap + 1).to_string()),
        "refusal cites {} — that is the read-side fallback's number, which means \
         the stat-side cap did not fire; got: {err}",
        cap + 1
    );
}

/// The same bound at the image surface, so the two surfaces are shown to be
/// enforcing through the SAME mechanism rather than each having their own.
#[test]
fn image_over_the_declared_cap_is_refused_from_the_stat_not_from_the_read() {
    let dir = tempfile::tempdir().unwrap();
    let over = dir.path().join("over-cap.png");
    let cap = wcore_tools::vision_tools::VISION_MAX_BYTES as u64;
    let oversize = cap * 3;
    oversize_fixture(&over, &png_bytes(), oversize);

    let err = load_local_image(over.to_str().unwrap()).expect_err("over cap must be refused");
    assert!(err.contains("too large"), "got: {err}");
    assert!(
        err.contains(&oversize.to_string()),
        "must cite the FULL length {oversize}; got: {err}"
    );
    assert!(
        !err.contains(&(cap + 1).to_string()),
        "cites the read-side fallback's number, so the stat-side cap did not \
         fire; got: {err}"
    );
}

// ── cross-class confusion ──────────────────────────────────────────────────

/// A WAV file has the same first eight bytes as a WebP image (`RIFF` + a
/// length). Before consolidation the shared chokepoint classified on an
/// 8-byte prefix and would have called a WAV an `image/webp`. Once audio and
/// images share one magic table that is a cross-class admission, not a
/// cosmetic bug — so it is pinned from both directions.
#[test]
fn a_wav_is_not_admitted_as_an_image_and_a_webp_is_not_admitted_as_audio() {
    let dir = tempfile::tempdir().unwrap();

    let wav_named_webp = dir.path().join("clip.webp");
    std::fs::write(&wav_named_webp, wav_bytes()).unwrap();
    assert!(
        load_local_image(wav_named_webp.to_str().unwrap()).is_err(),
        "a WAV must never be admitted to an image surface"
    );

    let mut webp = b"RIFF\x24\x08\x00\x00WEBPVP8 ".to_vec();
    webp.extend_from_slice(&[0u8; 32]);
    let webp_named_wav = dir.path().join("clip.wav");
    std::fs::write(&webp_named_wav, webp).unwrap();
    assert!(
        resolve(webp_named_wav).is_err(),
        "a WebP must never be admitted to an audio surface"
    );
}

/// A file whose extension contradicts its bytes is refused at BOTH surfaces,
/// by the same cross-check, in the same place.
///
/// **Measured RED at base** on the IMAGE half: the cross-check lived in
/// `wcore-cli`'s composer against its own private extension table, so
/// `vision_analyze` — reached by the MODEL rather than by the user — admitted
/// a PNG named `.jpg`. Consolidating moved the check under both surfaces and
/// deleted the composer's duplicate table.
#[test]
fn an_extension_that_contradicts_the_bytes_is_refused_at_both_surfaces() {
    let dir = tempfile::tempdir().unwrap();

    let png_named_jpg = dir.path().join("forged.jpg");
    std::fs::write(&png_named_jpg, png_bytes()).unwrap();
    assert!(load_local_image(png_named_jpg.to_str().unwrap()).is_err());

    let wav_named_mp3 = dir.path().join("forged.mp3");
    std::fs::write(&wav_named_mp3, wav_bytes()).unwrap();
    assert!(resolve(wav_named_mp3).is_err());
}

// ── the happy path still works ─────────────────────────────────────────────

/// Consolidation must not have broken admission. A real WAV under a `.wav`
/// name is admitted with the right MIME and every byte intact.
#[test]
fn a_valid_wav_is_still_admitted_with_its_bytes_intact() {
    let dir = tempfile::tempdir().unwrap();
    let ok = dir.path().join("good.wav");
    let bytes = wav_bytes();
    std::fs::write(&ok, &bytes).unwrap();
    let (mime, got) = resolve(ok).expect("a valid WAV must still be admitted");
    assert_eq!(mime, "audio/wav");
    assert_eq!(got, bytes, "every byte must survive the intake");
}

#[test]
fn a_valid_png_is_still_admitted_with_its_bytes_intact() {
    let dir = tempfile::tempdir().unwrap();
    let ok = dir.path().join("good.png");
    let bytes = png_bytes();
    std::fs::write(&ok, &bytes).unwrap();
    let (mime, got) = load_local_image(ok.to_str().unwrap()).expect("valid PNG");
    assert_eq!(mime, "image/png");
    assert_eq!(got, bytes);
}
