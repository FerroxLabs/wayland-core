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

/// KNOWN-NEGATIVE, RED at base. Every other file-reading tool in this tree
/// requires an absolute path. `transcribe_audio` took the model-supplied string
/// verbatim into `PathBuf::from`.
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

/// KNOWN-NEGATIVE, RED at base.
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

/// KNOWN-NEGATIVE, RED at base, and the sharpest one: on Windows, opening a
/// UNC target triggers an outbound SMB connect that leaks a NetNTLM hash to an
/// attacker-chosen host BEFORE any content check. `#644` closed this on every
/// other file surface. The audio path never had it.
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

/// KNOWN-NEGATIVE, RED at base. A path the deny-list names must never be
/// opened by a media tool, whatever the extension claims.
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

/// KNOWN-NEGATIVE, RED at base. The image path walked with `O_NOFOLLOW`; the
/// audio path followed symlinks freely.
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

/// **THE bound-enforcement known-negative.**
///
/// An input that exceeds `TRANSCRIPTION_MAX_BYTES` must be REFUSED, and the
/// refusal must cite the file's FULL length — a number only obtainable from
/// the descriptor's own metadata, never from a truncated read. If the cap
/// check is deleted from `media_intake::admit_open`, this test goes red:
/// the oversize file is admitted and `resolve_source` returns `Ok`.
///
/// The fixture is sparse (`set_len`), so it costs ~0 bytes on disk while
/// genuinely reporting a length above the cap.
#[test]
fn audio_over_the_declared_cap_is_refused_before_it_is_read() {
    let dir = tempfile::tempdir().unwrap();
    let over = dir.path().join("over-cap.wav");
    let f = std::fs::File::create(&over).unwrap();
    let oversize = TRANSCRIPTION_MAX_BYTES as u64 + 1;
    f.set_len(oversize).unwrap();
    drop(f);
    // Write a real WAV header at the front so the format check cannot be what
    // does the refusing — only the cap can.
    {
        use std::io::{Seek, SeekFrom, Write};
        let mut f = std::fs::OpenOptions::new().write(true).open(&over).unwrap();
        f.seek(SeekFrom::Start(0)).unwrap();
        f.write_all(&wav_bytes()).unwrap();
    }
    assert_eq!(std::fs::metadata(&over).unwrap().len(), oversize);

    let err = resolve(over).expect_err("a file over the declared cap must be refused");
    assert!(
        err.contains("too large"),
        "refusal must name the size, got: {err}"
    );
    assert!(
        err.contains(&oversize.to_string()),
        "refusal must cite the FULL length {oversize} from the descriptor's own \
         metadata, not a truncated read length; got: {err}"
    );
}

/// The same bound at the image surface, so the two surfaces are shown to be
/// enforcing through the SAME mechanism rather than each having their own.
#[test]
fn image_over_the_declared_cap_is_refused_before_it_is_read() {
    let dir = tempfile::tempdir().unwrap();
    let over = dir.path().join("over-cap.png");
    let f = std::fs::File::create(&over).unwrap();
    let oversize = wcore_tools::vision_tools::VISION_MAX_BYTES as u64 + 1;
    f.set_len(oversize).unwrap();
    drop(f);
    {
        use std::io::{Seek, SeekFrom, Write};
        let mut f = std::fs::OpenOptions::new().write(true).open(&over).unwrap();
        f.seek(SeekFrom::Start(0)).unwrap();
        f.write_all(&png_bytes()).unwrap();
    }
    let err = load_local_image(over.to_str().unwrap()).expect_err("over cap must be refused");
    assert!(err.contains("too large"), "got: {err}");
    assert!(err.contains(&oversize.to_string()), "got: {err}");
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
