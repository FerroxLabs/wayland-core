//! One bounded, open-once, magic-byte-validated intake for attachments and
//! documents.
//!
//! # Why this module exists
//!
//! Phase 27 measured the shipped binary's intake paths on `hetzner-dsm` and
//! found them to disagree in ONE way that matters. Traced at the syscall level
//! (`strace -f -y -e trace=openat,statx,...`, evidence
//! `.planning/phases/27-multimodal-browser-generation-voice/evidence/27-01/`):
//!
//! - The composer/vision path resolves the caller's name, opens it ONCE, and
//!   every subsequent fact — size, bytes, format — comes from THAT descriptor
//!   (`statx(fd, "", AT_EMPTY_PATH, ...)`, then a bounded `take()` on the same
//!   handle). Measured: 2 by-name resolutions, 1 `openat`, all later reads by
//!   descriptor. That is the correct idiom and it is preserved unchanged.
//! - The PDF path resolves the name (`validate_user_path`), resolves it AGAIN
//!   (`is_file()`), and then hands the PATH to an extractor that performs a
//!   THIRD, independent resolution and reads whatever that resolution finds.
//!   Measured: 3 by-name resolutions and an `openat` issued by a third party
//!   that never saw the validated handle. The bytes parsed are therefore not
//!   provably the bytes validated.
//!
//! This module generalizes the idiom the office-document tool already proves:
//! validate the caller-supplied path, refuse non-regular and network targets,
//! open EXACTLY ONCE, take the size from that descriptor, decide admissibility
//! from a bounded prefix read off that same descriptor, and read the remainder
//! from it under a bounded `take`. Callers receive BYTES, never a path to
//! re-open.
//!
//! # What this module does NOT do
//!
//! It does not widen any accepted-format set and it does not change any cap.
//! Unification is about HOW a file is admitted, not about WHAT is admitted.

use std::fs::File;
use std::io::Read as _;
use std::path::{Path, PathBuf};

use thiserror::Error;

use crate::path_validation::validate_user_path;

/// Bytes read to decide admissibility. Every container this module admits is
/// identifiable from its first few bytes; 8 covers the longest signature (PNG).
const MAGIC_PREFIX_BYTES: usize = 8;

/// Format classes this intake can identify from bytes alone.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MediaKind {
    Png,
    Jpeg,
    Gif,
    Bmp,
    Webp,
    Pdf,
    /// An OOXML container (docx / xlsx / pptx). Which of the three it is can
    /// only be decided by inspecting the archive, which is the document tool's
    /// job — this layer proves only that it IS a ZIP container.
    Ooxml,
}

impl MediaKind {
    /// The extension-declared class, or `None` when the name declares nothing
    /// this module recognises. A name is a claim; it is only ever cross-checked
    /// against detected bytes, never trusted on its own.
    pub fn from_extension(path: &Path) -> Option<Self> {
        let ext = path.extension()?.to_str()?.to_ascii_lowercase();
        Some(match ext.as_str() {
            "png" => Self::Png,
            "jpg" | "jpeg" => Self::Jpeg,
            "gif" => Self::Gif,
            "bmp" => Self::Bmp,
            "webp" => Self::Webp,
            "pdf" => Self::Pdf,
            "docx" | "xlsx" | "pptx" => Self::Ooxml,
            _ => return None,
        })
    }

    /// Decide the class from a bounded prefix. Returns `None` when the prefix
    /// matches nothing admissible.
    pub fn from_magic(prefix: &[u8]) -> Option<Self> {
        if prefix.starts_with(b"\x89PNG\r\n\x1a\n") {
            Some(Self::Png)
        } else if prefix.starts_with(&[0xFF, 0xD8, 0xFF]) {
            Some(Self::Jpeg)
        } else if prefix.starts_with(b"GIF87a") || prefix.starts_with(b"GIF89a") {
            Some(Self::Gif)
        } else if prefix.starts_with(b"BM") {
            Some(Self::Bmp)
        } else if prefix.starts_with(b"RIFF") {
            Some(Self::Webp)
        } else if prefix.starts_with(b"%PDF-") {
            Some(Self::Pdf)
        } else if prefix.starts_with(b"PK\x03\x04") {
            Some(Self::Ooxml)
        } else {
            None
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Png => "image/png",
            Self::Jpeg => "image/jpeg",
            Self::Gif => "image/gif",
            Self::Bmp => "image/bmp",
            Self::Webp => "image/webp",
            Self::Pdf => "application/pdf",
            Self::Ooxml => "application/vnd.openxmlformats",
        }
    }
}

/// Typed refusal reasons. Each surface renders these itself; nothing downstream
/// string-matches an intake failure.
#[derive(Debug, Error, PartialEq, Eq)]
pub enum IntakeError {
    #[error("Invalid path: {0}")]
    Path(String),
    #[error("Network/UNC paths are not allowed: {0}")]
    NetworkPath(PathBuf),
    #[error("Not a regular file: {0}")]
    NotRegularFile(PathBuf),
    /// Distinguished from [`IntakeError::Open`] so the existing user-visible
    /// "not found" wording survives unification. A surface that previously
    /// said "not found" must keep saying it — unifying the mechanism is not
    /// licence to churn a string a host may be matching on.
    #[error("File not found: {0}")]
    NotFound(PathBuf),
    #[error("Cannot open {path}: {reason}")]
    Open { path: PathBuf, reason: String },
    #[error("File too large: {actual} bytes (limit {limit} bytes)")]
    TooLarge { actual: u64, limit: u64 },
    #[error("File too small to be valid ({actual} bytes)")]
    TooSmall { actual: u64 },
    #[error("Unrecognised file format (the bytes match no supported container)")]
    UnrecognisedFormat,
    #[error("Extension declares {declared} but bytes are {detected}")]
    FormatMismatch {
        declared: &'static str,
        detected: &'static str,
    },
    #[error("{detected} is not accepted here")]
    KindNotAccepted { detected: &'static str },
}

/// Admitted bytes plus the class proved from those bytes.
#[derive(Debug, Clone)]
pub struct AdmittedMedia {
    /// The path as validated. Present for diagnostics ONLY — re-opening it
    /// would reintroduce exactly the second resolution this module removes.
    pub validated_path: PathBuf,
    pub kind: MediaKind,
    pub bytes: Vec<u8>,
}

/// The caps and accepted set a surface applies over this shared intake. Each
/// surface keeps its own policy; only the mechanism is shared.
#[derive(Debug, Clone)]
pub struct IntakePolicy {
    pub min_bytes: u64,
    pub max_bytes: u64,
    /// When `Some`, only these classes are admitted. `None` admits any class
    /// this module can identify.
    pub accept: Option<&'static [MediaKind]>,
}

impl IntakePolicy {
    pub fn new(min_bytes: u64, max_bytes: u64) -> Self {
        Self {
            min_bytes,
            max_bytes,
            accept: None,
        }
    }

    pub fn accepting(mut self, kinds: &'static [MediaKind]) -> Self {
        self.accept = Some(kinds);
        self
    }
}

/// Windows UNC and `file://host/share` forms never reach the filesystem here.
///
/// Delegates to [`wcore_config::network_path::has_unc_prefix`], the single
/// implementation. The local copy this replaces matched any `\\`/`//` prefix,
/// so it also called `\\?\C:\Users\x` — a verbatim path to a **local disk** —
/// a network path, and reported it as `IntakeError::NetworkPath`. That input
/// is still refused, by `validate_user_path` on the next line, but now as
/// `DeviceOrVerbatimPath`: the accurate reason. No input that was rejected
/// before is accepted now.
///
/// Spelling, not storage: a file on a mounted share is deliberately still
/// admitted. See `wcore_config::network_path` for why, and for the other
/// question.
fn is_unc_path(path: &Path) -> bool {
    wcore_config::network_path::has_unc_prefix(path)
}

/// Admit a caller-supplied path as bytes, resolving the name EXACTLY ONCE for
/// the read.
///
/// Sequence, and the order is the contract:
/// 1. validate the path (traversal, null bytes, secret deny-list),
/// 2. refuse network targets,
/// 3. open once — every later fact comes from this descriptor,
/// 4. take the length from the descriptor and enforce both caps BEFORE any
///    payload read,
/// 5. read a bounded prefix from the descriptor and decide the class from it,
/// 6. cross-check the extension's claim against the detected class,
/// 7. read the remainder from the same descriptor under a bounded take.
pub fn admit_path(path: &Path, policy: &IntakePolicy) -> Result<AdmittedMedia, IntakeError> {
    if is_unc_path(path) {
        return Err(IntakeError::NetworkPath(path.to_path_buf()));
    }
    let validated = validate_user_path(path).map_err(|e| IntakeError::Path(e.to_string()))?;
    if is_unc_path(&validated) {
        return Err(IntakeError::NetworkPath(validated));
    }

    // THE ONLY resolution of this name that the admitted bytes depend on.
    let mut file = File::open(&validated).map_err(|e| match e.kind() {
        std::io::ErrorKind::NotFound => IntakeError::NotFound(validated.clone()),
        _ => IntakeError::Open {
            path: validated.clone(),
            reason: e.to_string(),
        },
    })?;

    // Everything below is by descriptor. A name re-pointed from here on cannot
    // change which bytes are admitted.
    let meta = file.metadata().map_err(|e| IntakeError::Open {
        path: validated.clone(),
        reason: e.to_string(),
    })?;
    if !meta.is_file() {
        return Err(IntakeError::NotRegularFile(validated));
    }
    let len = meta.len();
    if len > policy.max_bytes {
        return Err(IntakeError::TooLarge {
            actual: len,
            limit: policy.max_bytes,
        });
    }
    if len < policy.min_bytes {
        return Err(IntakeError::TooSmall { actual: len });
    }

    let mut prefix = [0u8; MAGIC_PREFIX_BYTES];
    let prefix_len = read_prefix(&mut file, &mut prefix)?;
    let detected =
        MediaKind::from_magic(&prefix[..prefix_len]).ok_or(IntakeError::UnrecognisedFormat)?;

    if let Some(declared) = MediaKind::from_extension(&validated)
        && declared != detected
    {
        return Err(IntakeError::FormatMismatch {
            declared: declared.as_str(),
            detected: detected.as_str(),
        });
    }
    if let Some(accept) = policy.accept
        && !accept.contains(&detected)
    {
        return Err(IntakeError::KindNotAccepted {
            detected: detected.as_str(),
        });
    }

    // The prefix is already consumed from the handle; keep it and append the
    // bounded remainder read from the SAME handle.
    let mut bytes = Vec::with_capacity(len.min(policy.max_bytes) as usize);
    bytes.extend_from_slice(&prefix[..prefix_len]);
    // `+ 1` so a file that grew past the cap between the stat and the read is
    // still detected rather than silently truncated.
    file.take(policy.max_bytes - prefix_len as u64 + 1)
        .read_to_end(&mut bytes)
        .map_err(|e| IntakeError::Open {
            path: validated.clone(),
            reason: e.to_string(),
        })?;
    if bytes.len() as u64 > policy.max_bytes {
        return Err(IntakeError::TooLarge {
            actual: bytes.len() as u64,
            limit: policy.max_bytes,
        });
    }

    Ok(AdmittedMedia {
        validated_path: validated,
        kind: detected,
        bytes,
    })
}

/// Decide a class for bytes a connector already fetched, so a channel cannot
/// introduce a class the composer would have refused. There is no path here and
/// therefore no resolution to protect — only the format decision is shared.
pub fn admit_bytes(bytes: &[u8], policy: &IntakePolicy) -> Result<MediaKind, IntakeError> {
    let len = bytes.len() as u64;
    if len > policy.max_bytes {
        return Err(IntakeError::TooLarge {
            actual: len,
            limit: policy.max_bytes,
        });
    }
    if len < policy.min_bytes {
        return Err(IntakeError::TooSmall { actual: len });
    }
    let detected = MediaKind::from_magic(&bytes[..bytes.len().min(MAGIC_PREFIX_BYTES)])
        .ok_or(IntakeError::UnrecognisedFormat)?;
    if let Some(accept) = policy.accept
        && !accept.contains(&detected)
    {
        return Err(IntakeError::KindNotAccepted {
            detected: detected.as_str(),
        });
    }
    Ok(detected)
}

/// Read up to `buf.len()` bytes, tolerating short reads. A file shorter than
/// the prefix is not an error here — the caps above already decided whether its
/// length is admissible, and a short prefix simply matches no signature.
fn read_prefix(file: &mut File, buf: &mut [u8]) -> Result<usize, IntakeError> {
    let mut filled = 0;
    while filled < buf.len() {
        match file.read(&mut buf[filled..]) {
            Ok(0) => break,
            Ok(n) => filled += n,
            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(e) => {
                return Err(IntakeError::Open {
                    path: PathBuf::new(),
                    reason: e.to_string(),
                });
            }
        }
    }
    Ok(filled)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::tempdir;

    const PNG: &[u8] = b"\x89PNG\r\n\x1a\nbody-bytes-here";
    const JPEG: &[u8] = b"\xff\xd8\xff\xe0JFIF-body-bytes";
    const PDF: &[u8] = b"%PDF-1.4 body bytes here";
    const ZIP: &[u8] = b"PK\x03\x04ooxml-container-bytes";

    fn any() -> IntakePolicy {
        IntakePolicy::new(1, 1024 * 1024)
    }

    fn write(dir: &Path, name: &str, bytes: &[u8]) -> PathBuf {
        let p = dir.join(name);
        fs::write(&p, bytes).unwrap();
        p
    }

    #[test]
    fn admits_each_class_from_bytes_not_from_the_name() {
        let dir = tempdir().unwrap();
        for (name, bytes, kind) in [
            ("a.png", PNG, MediaKind::Png),
            ("b.jpg", JPEG, MediaKind::Jpeg),
            ("c.pdf", PDF, MediaKind::Pdf),
            ("d.docx", ZIP, MediaKind::Ooxml),
        ] {
            let p = write(dir.path(), name, bytes);
            let got = admit_path(&p, &any()).unwrap();
            assert_eq!(got.kind, kind, "{name}");
            assert_eq!(got.bytes, bytes, "{name} must return every byte exactly");
        }
    }

    #[test]
    fn a_name_with_no_extension_is_admitted_on_its_bytes() {
        let dir = tempdir().unwrap();
        let p = write(dir.path(), "no-extension-at-all", PNG);
        assert_eq!(admit_path(&p, &any()).unwrap().kind, MediaKind::Png);
    }

    #[test]
    fn refuses_every_extension_versus_bytes_disagreement() {
        let dir = tempdir().unwrap();
        for (name, bytes) in [
            ("png-body.jpg", PNG),
            ("jpeg-body.png", JPEG),
            ("not-a-pdf.pdf", PNG),
            ("not-a-container.docx", PDF),
        ] {
            let p = write(dir.path(), name, bytes);
            assert!(
                matches!(
                    admit_path(&p, &any()),
                    Err(IntakeError::FormatMismatch { .. })
                ),
                "{name} must be refused as a mismatch"
            );
        }
    }

    #[test]
    fn refuses_bytes_matching_no_signature() {
        let dir = tempdir().unwrap();
        let p = write(dir.path(), "junk.bin", b"absolutely not a container");
        assert!(matches!(
            admit_path(&p, &any()),
            Err(IntakeError::UnrecognisedFormat)
        ));
    }

    #[test]
    fn refuses_a_truncated_header() {
        let dir = tempdir().unwrap();
        let p = write(dir.path(), "short.png", &PNG[..4]);
        assert!(matches!(
            admit_path(&p, &any()),
            Err(IntakeError::UnrecognisedFormat)
        ));
    }

    #[test]
    fn caps_fire_at_the_boundary_in_both_directions() {
        let dir = tempdir().unwrap();
        let policy = IntakePolicy::new(16, 32);

        let at_min = write(dir.path(), "at-min.png", &[PNG, &[0u8; 8]].concat()[..16]);
        assert_eq!(fs::metadata(&at_min).unwrap().len(), 16);
        assert!(
            admit_path(&at_min, &policy).is_ok(),
            "16 == min is admitted"
        );

        let under = write(dir.path(), "under.png", &PNG[..15]);
        assert!(matches!(
            admit_path(&under, &policy),
            Err(IntakeError::TooSmall { actual: 15 })
        ));

        let over = write(dir.path(), "over.png", &[PNG, &[0u8; 64]].concat());
        assert!(matches!(
            admit_path(&over, &policy),
            Err(IntakeError::TooLarge { .. })
        ));
    }

    /// The size cap must be decided from the descriptor's own metadata BEFORE
    /// any payload is read. If it were enforced after ingestion, a hostile file
    /// would be fully read first — T-27-01-03.
    #[test]
    fn the_size_cap_is_decided_before_the_payload_is_read() {
        let dir = tempdir().unwrap();
        let huge = write(
            dir.path(),
            "huge.png",
            &[PNG, &vec![7u8; 4096][..]].concat(),
        );
        let policy = IntakePolicy::new(1, 64);
        match admit_path(&huge, &policy) {
            // The refusal must cite the file's FULL length, which is only
            // knowable from the stat — not a truncated read length.
            Err(IntakeError::TooLarge { actual, limit }) => {
                assert_eq!(limit, 64);
                assert_eq!(actual, PNG.len() as u64 + 4096);
            }
            other => panic!("expected TooLarge from the stat, got {other:?}"),
        }
    }

    #[test]
    fn an_accept_list_refuses_a_class_it_does_not_name() {
        let dir = tempdir().unwrap();
        let p = write(dir.path(), "doc.pdf", PDF);
        let images_only = any().accepting(&[MediaKind::Png, MediaKind::Jpeg]);
        assert!(matches!(
            admit_path(&p, &images_only),
            Err(IntakeError::KindNotAccepted { .. })
        ));
    }

    #[test]
    fn refuses_a_directory_and_a_missing_name() {
        let dir = tempdir().unwrap();
        assert!(admit_path(dir.path(), &any()).is_err());
        assert!(matches!(
            admit_path(&dir.path().join("nope.png"), &any()),
            Err(IntakeError::NotFound(_))
        ));
        // The wording a host may already be matching on must survive.
        assert!(
            admit_path(&dir.path().join("nope.png"), &any())
                .unwrap_err()
                .to_string()
                .contains("not found")
        );
    }

    #[test]
    fn refuses_a_unc_target_without_touching_the_filesystem() {
        let p = PathBuf::from(r"\\server\share\image.png");
        assert!(matches!(
            admit_path(&p, &any()),
            Err(IntakeError::NetworkPath(_))
        ));
    }

    /// Connector-supplied bytes face the SAME format decision as a composer
    /// path, so a channel cannot introduce a class the composer would refuse.
    #[test]
    fn connector_bytes_face_the_same_format_decision() {
        let images_only = any().accepting(&[MediaKind::Png, MediaKind::Jpeg]);
        assert_eq!(admit_bytes(PNG, &images_only).unwrap(), MediaKind::Png);
        assert!(matches!(
            admit_bytes(PDF, &images_only),
            Err(IntakeError::KindNotAccepted { .. })
        ));
        assert!(matches!(
            admit_bytes(b"junk", &images_only),
            Err(IntakeError::UnrecognisedFormat)
        ));
    }

    /// The whole point of the module: the caller gets BYTES, so there is no
    /// path left for a second resolution to disagree about. A repoint after
    /// admission cannot change what was admitted.
    #[test]
    fn a_repoint_after_admission_cannot_change_the_admitted_bytes() {
        let dir = tempdir().unwrap();
        let p = write(dir.path(), "swap.png", PNG);
        let admitted = admit_path(&p, &any()).unwrap();

        fs::write(&p, b"\x89PNG\r\n\x1a\nCOMPLETELY-DIFFERENT").unwrap();

        assert_eq!(
            admitted.bytes, PNG,
            "admitted bytes must be immune to a later repoint"
        );
    }
}
