//! **The** bounded, open-once, magic-byte-validated intake for every piece of
//! media the engine ingests — images, audio and documents, from a local path or
//! from bytes a connector already fetched.
//!
//! # Why this module exists
//!
//! Phase 27 measured the shipped binary's intake paths on `hetzner-dsm` and
//! found them to disagree. Traced at the syscall level
//! (`strace -f -y -e trace=openat,statx,...`, evidence
//! `.planning/phases/27-multimodal-browser-generation-voice/evidence/27-01/`):
//!
//! - The composer/vision path resolves the caller's name, opens it ONCE with an
//!   `openat(O_NOFOLLOW)` component walk, and every subsequent fact — size,
//!   bytes, format — comes from THAT descriptor. That is the correct idiom.
//! - The PDF path resolved the name three times and handed the PATH to an
//!   extractor that opened it independently. The bytes parsed were therefore
//!   not provably the bytes validated.
//! - The `transcribe_audio` local-file path (measured by lane `27-c1-intake`,
//!   and MISSING from the earlier four-path census) applied **no path
//!   validation at all** — no absolute-path requirement, no traversal check, no
//!   UNC refusal, no system-path deny-list — and read with an unbounded
//!   `fs::read` after a separate `fs::metadata`, so its declared 25 MB cap
//!   bounded a stat it did not read from.
//!
//! # The one sequence
//!
//! [`admit_open`] is the single primitive. Everything else is a projection of
//! it:
//!
//! 1. refuse network/UNC targets before touching the filesystem,
//! 2. validate the path (absolute, traversal, null bytes, system deny-list,
//!    symlink-target deny-list, non-regular targets),
//! 3. open EXACTLY ONCE through a symlink-refusing component walk — a raced
//!    parent rename cannot redirect the final open, a hostile FIFO cannot block
//!    it (`O_NONBLOCK`), and a Windows reparse point is refused,
//! 4. take the length from THAT descriptor and enforce both caps BEFORE any
//!    payload read,
//! 5. read a bounded prefix from that descriptor and decide the class from it,
//! 6. cross-check the extension's claim against the detected class,
//! 7. hand back the descriptor (or bounded bytes read from it) — never a path
//!    for the caller to re-open.
//!
//! # What this module does NOT do
//!
//! It does not widen any accepted-format set and it does not change any cap.
//! Each surface supplies its own [`IntakePolicy`]; only the mechanism is
//! shared. Unification is about HOW a file is admitted, not about WHAT is
//! admitted.

use std::fs::File;
#[cfg(not(unix))]
use std::fs::OpenOptions;
use std::io::{Read as _, Seek as _};
// `Prefix` is deliberately absent: UNC classification is no longer done here.
// It lives in `wcore_config::network_path`, reached via `is_unc_path` below.
// `Component` is imported by the `cfg(unix)` arm of `open_once`, its only
// consumer. Importing it here instead leaves it unused on Windows, where
// `-D warnings` turns that into a CI failure.
use std::path::{Path, PathBuf};

use thiserror::Error;

use crate::path_validation::validate_user_path;

/// Bytes read to decide admissibility.
///
/// 12 is the minimum that separates every container this module admits:
/// `RIFF....WAVE` (audio) from `RIFF....WEBP` (image) needs bytes 8..12, and
/// the MP4/M4A `ftyp` atom sits at offset 4..8. A shorter prefix silently
/// classified a WAV file as a WebP image, which is exactly the kind of
/// cross-class confusion a shared intake exists to prevent.
const MAGIC_PREFIX_BYTES: usize = 12;

/// Format classes this intake can identify from bytes alone.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MediaKind {
    // ── images ──
    Png,
    Jpeg,
    Gif,
    Bmp,
    Webp,
    // ── documents ──
    Pdf,
    /// An OOXML container (docx / xlsx / pptx). Which of the three it is can
    /// only be decided by inspecting the archive, which is the document tool's
    /// job — this layer proves only that it IS a ZIP container.
    Ooxml,
    // ── audio ──
    Mp3,
    Mp4Audio,
    Aac,
    Wav,
    Ogg,
    Webm,
    Flac,
    /// Bytes that match no container signature. NEVER returned by
    /// [`MediaKind::from_magic`] — a surface only sees it when its policy sets
    /// [`IntakePolicy::allow_unclassified`], which exists for the one surface
    /// that legitimately ingests headerless input (CSV / plain text through
    /// the office-document tool). Every other surface refuses it.
    Unclassified,
}

/// The image classes `vision_analyze` and the composer accept.
pub const IMAGE_KINDS: &[MediaKind] = &[
    MediaKind::Png,
    MediaKind::Jpeg,
    MediaKind::Gif,
    MediaKind::Bmp,
    MediaKind::Webp,
];

/// The audio classes `transcribe_audio` accepts.
pub const AUDIO_KINDS: &[MediaKind] = &[
    MediaKind::Mp3,
    MediaKind::Mp4Audio,
    MediaKind::Aac,
    MediaKind::Wav,
    MediaKind::Ogg,
    MediaKind::Webm,
    MediaKind::Flac,
];

impl MediaKind {
    /// The extension-declared class, or `None` when the name declares nothing
    /// this module recognises. A name is a claim; it is only ever cross-checked
    /// against detected bytes, never trusted on its own.
    pub fn from_extension(path: &Path) -> Option<Self> {
        let raw = path.extension()?.to_str()?;
        // A dropped path can carry a URL-ish `?query` / `#fragment` tail. Strip
        // it so the declared class is still recognised and still cross-checked
        // — dropping the check would be the weaker behaviour.
        let ext = raw
            .split(['?', '#'])
            .next()
            .unwrap_or(raw)
            .to_ascii_lowercase();
        Some(match ext.as_str() {
            "png" => Self::Png,
            "jpg" | "jpeg" => Self::Jpeg,
            "gif" => Self::Gif,
            "bmp" => Self::Bmp,
            "webp" => Self::Webp,
            "pdf" => Self::Pdf,
            "docx" | "xlsx" | "pptx" => Self::Ooxml,
            "mp3" => Self::Mp3,
            "m4a" | "mp4" => Self::Mp4Audio,
            "aac" => Self::Aac,
            "wav" | "wave" => Self::Wav,
            "ogg" | "oga" | "opus" => Self::Ogg,
            "webm" => Self::Webm,
            "flac" => Self::Flac,
            _ => return None,
        })
    }

    /// Decide the class from a bounded prefix. Returns `None` when the prefix
    /// matches nothing admissible.
    ///
    /// Order matters: the `RIFF` container is disambiguated by bytes 8..12
    /// before any shorter signature is considered, so a WAV is never mistaken
    /// for a WebP.
    pub fn from_magic(prefix: &[u8]) -> Option<Self> {
        // ── containers needing 12 bytes, checked first ──
        if prefix.len() >= 12 && prefix.starts_with(b"RIFF") {
            return match &prefix[8..12] {
                b"WEBP" => Some(Self::Webp),
                b"WAVE" => Some(Self::Wav),
                _ => None,
            };
        }
        if prefix.len() >= 12 && &prefix[4..8] == b"ftyp" {
            return Some(Self::Mp4Audio);
        }

        // ── 8-byte and shorter signatures ──
        if prefix.starts_with(b"\x89PNG\r\n\x1a\n") {
            return Some(Self::Png);
        }
        if prefix.starts_with(&[0xFF, 0xD8, 0xFF]) {
            return Some(Self::Jpeg);
        }
        if prefix.starts_with(b"GIF87a") || prefix.starts_with(b"GIF89a") {
            return Some(Self::Gif);
        }
        if prefix.starts_with(b"%PDF-") {
            return Some(Self::Pdf);
        }
        if prefix.starts_with(b"PK\x03\x04") {
            return Some(Self::Ooxml);
        }
        if prefix.starts_with(b"OggS") {
            return Some(Self::Ogg);
        }
        if prefix.starts_with(b"fLaC") {
            return Some(Self::Flac);
        }
        if prefix.starts_with(&[0x1A, 0x45, 0xDF, 0xA3]) {
            return Some(Self::Webm);
        }
        if prefix.starts_with(b"ID3") {
            return Some(Self::Mp3);
        }
        // BMP is a two-byte signature, so it is checked after every longer one.
        if prefix.starts_with(b"BM") {
            return Some(Self::Bmp);
        }
        // ADTS/MPEG frame sync is the loosest signature in the table and is
        // therefore last.
        if prefix.len() >= 2 && prefix[0] == 0xFF {
            let b1 = prefix[1];
            if b1 == 0xF1 || b1 == 0xF9 {
                return Some(Self::Aac);
            }
            if (b1 & 0xE0) == 0xE0 {
                return Some(Self::Mp3);
            }
        }
        None
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
            Self::Mp3 => "audio/mpeg",
            Self::Mp4Audio => "audio/mp4",
            Self::Aac => "audio/aac",
            Self::Wav => "audio/wav",
            Self::Ogg => "audio/ogg",
            Self::Webm => "audio/webm",
            Self::Flac => "audio/flac",
            Self::Unclassified => "application/octet-stream",
        }
    }

    pub fn is_image(self) -> bool {
        IMAGE_KINDS.contains(&self)
    }

    pub fn is_audio(self) -> bool {
        AUDIO_KINDS.contains(&self)
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
    /// A component of the path could not be opened — the symlink-refusing walk
    /// reports the whole path plus the OS reason. Distinguished from
    /// [`IntakeError::Open`] because refusing a symlinked PARENT is a different
    /// user-facing fact from failing to open the leaf.
    #[error("Cannot open {noun} path component in {path}: {reason}")]
    OpenComponent {
        noun: &'static str,
        path: PathBuf,
        reason: String,
    },
    #[error("Symlinks/reparse points are not allowed: {0}")]
    Symlink(PathBuf),
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

/// An admitted, still-open handle for a surface that streams rather than
/// buffering (the OOXML archive reader). The handle is positioned at byte 0
/// and is the SAME descriptor every admission fact was decided from.
#[derive(Debug)]
pub struct AdmittedHandle {
    pub validated_path: PathBuf,
    pub kind: MediaKind,
    /// Length taken from the descriptor's own metadata.
    pub len: u64,
    pub file: File,
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
    /// The word a surface uses for its media in path-level diagnostics
    /// ("image", "audio", "document"). Preserves each surface's historical
    /// wording through unification.
    pub noun: &'static str,
    /// Refuse a file whose extension declares a class the bytes contradict.
    /// On by default — a name that disagrees with its content is the
    /// classic confused-deputy input.
    pub cross_check_extension: bool,
    /// Admit bytes matching no container signature as
    /// [`MediaKind::Unclassified`] instead of refusing them. OFF by default.
    /// Set ONLY by the office-document surface, which legitimately ingests
    /// headerless CSV and plain text. The surface must still name
    /// `Unclassified` in its `accept` list, so turning this on is not enough
    /// on its own.
    pub allow_unclassified: bool,
}

impl IntakePolicy {
    pub fn new(min_bytes: u64, max_bytes: u64) -> Self {
        Self {
            min_bytes,
            max_bytes,
            accept: None,
            noun: "media",
            cross_check_extension: true,
            allow_unclassified: false,
        }
    }

    pub fn accepting(mut self, kinds: &'static [MediaKind]) -> Self {
        self.accept = Some(kinds);
        self
    }

    pub fn named(mut self, noun: &'static str) -> Self {
        self.noun = noun;
        self
    }

    pub fn without_extension_cross_check(mut self) -> Self {
        self.cross_check_extension = false;
        self
    }

    /// See [`IntakePolicy::allow_unclassified`].
    pub fn allowing_unclassified(mut self) -> Self {
        self.allow_unclassified = true;
        self
    }
}

/// Windows UNC and `file://host/share` forms never reach the filesystem here.
///
/// Delegates to [`wcore_config::network_path::has_unc_prefix`], the single
/// implementation. The local copy this replaces matched any `\\`/`//` prefix,
/// so it also called `\\?\C:\Users\x` — a verbatim path to a **local disk** —
/// a network path, and reported it as `IntakeError::NetworkPath`. It is not
/// one. Since core#409 c2 it is not refused for its namespace at all: a
/// verbatim path to a local disk is an ordinary local file, and it is the form
/// `std::fs::canonicalize` returns on Windows. `\\.\…` and
/// `\\?\GLOBALROOT\…` are still refused on the next line, as
/// `DeviceOrVerbatimPath`.
///
/// Spelling, not storage: a file on a mounted share is deliberately still
/// admitted. See `wcore_config::network_path` for why, and for the other
/// question.
fn is_unc_path(path: &Path) -> bool {
    wcore_config::network_path::has_unc_prefix(path)
}

/// Resolve symlinked ANCESTORS, keeping the caller's leaf name verbatim.
///
/// The `openat(O_NOFOLLOW)` walk below refuses a symlink at every level. That
/// is correct for the leaf and unsatisfiable for the ancestors on macOS, where
/// `$TMPDIR` is `/var/folders/...` and `/var` is a symlink to `/private/var`
/// shipped by the OS. Refusing it meant every media and document surface
/// refused every temp path on macOS (issue #937) — a whole platform's intake,
/// closed, because of a symlink no attacker put there.
///
/// So the ancestors are canonicalised ONCE here and the walk then runs over a
/// symlink-free chain. The module's actual threat — "a raced parent rename
/// cannot redirect the final open" — is untouched: the walk still opens every
/// resolved component with `O_NOFOLLOW`, so a component swapped for a symlink
/// AFTER this call still fails the walk. What is given up is the blanket
/// refusal of a caller-named path that merely traverses a symlink, which
/// `admit_open` compensates for by re-running the deny-list over the resolved
/// path — a symlinked ancestor therefore cannot smuggle a denied target past a
/// check that only ever saw the pre-resolution name.
///
/// The leaf is deliberately NOT canonicalised: resolving it would defeat the
/// leaf refusal, which is the half that stops an attacker swapping the file
/// itself for a link to something they want read.
#[cfg(unix)]
fn resolve_ancestor_symlinks(path: &Path, noun: &'static str) -> Result<PathBuf, IntakeError> {
    let (Some(parent), Some(name)) = (path.parent(), path.file_name()) else {
        return Err(IntakeError::Path(format!(
            "{noun} path has no file name: {}",
            path.display()
        )));
    };
    let canonical = std::fs::canonicalize(parent).map_err(|e| {
        if e.kind() == std::io::ErrorKind::NotFound {
            IntakeError::NotFound(path.to_path_buf())
        } else {
            IntakeError::OpenComponent {
                noun,
                path: path.to_path_buf(),
                reason: e.to_string(),
            }
        }
    })?;
    Ok(canonical.join(name))
}

/// Open a media file without following a symlink/reparse point.
///
/// `O_NONBLOCK` prevents a hostile FIFO from hanging before the regular-file
/// check. Unix walks from `/` with directory handles and `openat(O_NOFOLLOW)`
/// so a raced parent rename cannot redirect the final open. Windows rejects
/// reparse-point parents before opening the leaf reparse point itself.
///
/// Private on purpose: this is the ONLY place in the tree that opens a
/// caller-named media file, and keeping it private is what makes that
/// compiler-checked rather than convention.
fn open_once(path: &Path, noun: &'static str) -> Result<File, IntakeError> {
    #[cfg(unix)]
    {
        use std::ffi::CString;
        use std::os::fd::{AsRawFd as _, FromRawFd as _};
        use std::os::unix::ffi::OsStrExt as _;
        use std::path::Component;

        let mut parts = path.components();
        if !matches!(parts.next(), Some(Component::RootDir)) {
            return Err(IntakeError::Path(format!(
                "{noun} path is not absolute: {}",
                path.display()
            )));
        }
        let names = parts
            .map(|part| match part {
                Component::Normal(name) => CString::new(name.as_bytes()).map_err(|_| {
                    IntakeError::Path(format!("{noun} path contains NUL: {}", path.display()))
                }),
                _ => Err(IntakeError::Path(format!(
                    "{noun} path contains an unsupported component: {}",
                    path.display()
                ))),
            })
            .collect::<Result<Vec<_>, _>>()?;
        if names.is_empty() {
            return Err(IntakeError::Path(format!(
                "{noun} path has no file name: {}",
                path.display()
            )));
        }

        let mut parent = File::open("/").map_err(|e| IntakeError::OpenComponent {
            noun,
            path: path.to_path_buf(),
            reason: e.to_string(),
        })?;
        for (index, name) in names.iter().enumerate() {
            let is_leaf = index + 1 == names.len();
            let flags = if is_leaf {
                libc::O_RDONLY | libc::O_NOFOLLOW | libc::O_NONBLOCK | libc::O_CLOEXEC
            } else {
                libc::O_RDONLY | libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC
            };
            // SAFETY: `parent` is a live directory descriptor for every
            // non-leaf iteration, `name` is NUL-terminated, and no pointer is
            // retained after `openat` returns.
            let fd = unsafe { libc::openat(parent.as_raw_fd(), name.as_ptr(), flags) };
            if fd < 0 {
                let err = std::io::Error::last_os_error();
                // ENOENT anywhere on the walk — leaf or an intermediate
                // directory — is "not found" as the user means it. Reporting a
                // missing parent as a component-open failure would be a
                // regression against the wording every other media surface
                // produces for the same condition.
                if err.kind() == std::io::ErrorKind::NotFound {
                    return Err(IntakeError::NotFound(path.to_path_buf()));
                }
                return Err(IntakeError::OpenComponent {
                    noun,
                    path: path.to_path_buf(),
                    reason: err.to_string(),
                });
            }
            // SAFETY: `openat` returned a new owned descriptor on success.
            let opened = unsafe { File::from_raw_fd(fd) };
            if is_leaf {
                return Ok(opened);
            }
            parent = opened;
        }
        unreachable!("non-empty component walk returns at the leaf")
    }

    #[cfg(windows)]
    {
        use std::os::windows::fs::{MetadataExt as _, OpenOptionsExt as _};
        const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x0000_0400;
        const FILE_FLAG_OPEN_REPARSE_POINT: u32 = 0x0020_0000;

        for parent in path.ancestors().skip(1) {
            let metadata = std::fs::symlink_metadata(parent).map_err(|e| {
                // Same rule the unix walk states above, and it was missing
                // here: a missing ancestor is "not found" as the user means
                // it, not a component-open failure. Without this branch a
                // simply absent file reported `Cannot open audio path
                // component in C:\nonexistent\path: ... (os error 3)` on
                // Windows while every other media surface — and unix — says
                // `File not found: <path>`. That is exactly the wording
                // regression `IntakeError::NotFound` exists to prevent, and it
                // also named the PARENT rather than the path the caller asked
                // for.
                if e.kind() == std::io::ErrorKind::NotFound {
                    return IntakeError::NotFound(path.to_path_buf());
                }
                IntakeError::OpenComponent {
                    noun,
                    path: parent.to_path_buf(),
                    reason: e.to_string(),
                }
            })?;
            if metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0 {
                return Err(IntakeError::Symlink(parent.to_path_buf()));
            }
        }

        let file = OpenOptions::new()
            .read(true)
            .custom_flags(FILE_FLAG_OPEN_REPARSE_POINT)
            .open(path)
            .map_err(|e| match e.kind() {
                std::io::ErrorKind::NotFound => IntakeError::NotFound(path.to_path_buf()),
                _ => IntakeError::Open {
                    path: path.to_path_buf(),
                    reason: e.to_string(),
                },
            })?;
        let metadata = file.metadata().map_err(|e| IntakeError::Open {
            path: path.to_path_buf(),
            reason: e.to_string(),
        })?;
        if metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0 {
            return Err(IntakeError::Symlink(path.to_path_buf()));
        }
        Ok(file)
    }

    #[cfg(not(any(unix, windows)))]
    {
        let _ = noun;
        OpenOptions::new()
            .read(true)
            .open(path)
            .map_err(|e| match e.kind() {
                std::io::ErrorKind::NotFound => IntakeError::NotFound(path.to_path_buf()),
                _ => IntakeError::Open {
                    path: path.to_path_buf(),
                    reason: e.to_string(),
                },
            })
    }
}

/// **The** primitive. Admit a caller-supplied path and hand back the OPEN
/// descriptor every admission fact was decided from, positioned at byte 0.
///
/// Use this when the caller streams (an archive reader). Use [`admit_path`]
/// when the caller wants bounded bytes.
pub fn admit_open(path: &Path, policy: &IntakePolicy) -> Result<AdmittedHandle, IntakeError> {
    if is_unc_path(path) {
        return Err(IntakeError::NetworkPath(path.to_path_buf()));
    }
    let validated = validate_user_path(path).map_err(|e| IntakeError::Path(e.to_string()))?;
    if is_unc_path(&validated) {
        return Err(IntakeError::NetworkPath(validated));
    }

    // Ancestors are resolved AFTER `validate_user_path`, never before. Order is
    // load-bearing: canonicalising first would silently collapse a `..`
    // traversal that the validator is there to refuse.
    //
    // No second deny-check is added here, and that is deliberate rather than an
    // omission. `validate_user_path` already canonicalises the longest existing
    // prefix — following symlinks — and re-runs the deny-list against the
    // canonical target precisely so a symlinked ancestor cannot smuggle one
    // past (see `path_validation.rs`, the M-8 / tools-io-17 block). Adding a
    // second call here would look like the thing protecting that boundary while
    // contributing nothing, which is worse than not adding it.
    #[cfg(unix)]
    let validated = {
        let resolved = resolve_ancestor_symlinks(&validated, policy.noun)?;
        if is_unc_path(&resolved) {
            return Err(IntakeError::NetworkPath(resolved));
        }
        resolved
    };

    // THE ONLY resolution of this name that the admitted bytes depend on.
    let mut file = open_once(&validated, policy.noun)?;

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
    let detected = match MediaKind::from_magic(&prefix[..prefix_len]) {
        Some(kind) => kind,
        None if policy.allow_unclassified => MediaKind::Unclassified,
        None => return Err(IntakeError::UnrecognisedFormat),
    };

    if policy.cross_check_extension
        && let Some(declared) = MediaKind::from_extension(&validated)
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

    file.rewind().map_err(|e| IntakeError::Open {
        path: validated.clone(),
        reason: e.to_string(),
    })?;

    Ok(AdmittedHandle {
        validated_path: validated,
        kind: detected,
        len,
        file,
    })
}

/// Admit a caller-supplied path as bounded bytes, resolving the name EXACTLY
/// ONCE. A projection of [`admit_open`].
pub fn admit_path(path: &Path, policy: &IntakePolicy) -> Result<AdmittedMedia, IntakeError> {
    let AdmittedHandle {
        validated_path,
        kind,
        len,
        file,
    } = admit_open(path, policy)?;

    let mut bytes = Vec::with_capacity(len.min(policy.max_bytes) as usize);
    // `+ 1` so a file that grew past the cap between the stat and the read is
    // still detected rather than silently truncated. This is the bound that
    // matters: it applies to the READ, not to a stat the read never consulted.
    file.take(policy.max_bytes + 1)
        .read_to_end(&mut bytes)
        .map_err(|e| IntakeError::Open {
            path: validated_path.clone(),
            reason: e.to_string(),
        })?;
    if bytes.len() as u64 > policy.max_bytes {
        return Err(IntakeError::TooLarge {
            actual: bytes.len() as u64,
            limit: policy.max_bytes,
        });
    }

    Ok(AdmittedMedia {
        validated_path,
        kind,
        bytes,
    })
}

/// Decide a class for bytes a connector or fetcher already produced, so a
/// channel or a remote URL cannot introduce a class a local path would have
/// been refused. There is no path here and therefore no resolution to protect
/// — the caps and the format decision are what is shared.
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
    let detected = match MediaKind::from_magic(&bytes[..bytes.len().min(MAGIC_PREFIX_BYTES)]) {
        Some(kind) => kind,
        None if policy.allow_unclassified => MediaKind::Unclassified,
        None => return Err(IntakeError::UnrecognisedFormat),
    };
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
    // The caller (`admit_open`) rewinds the handle once the class is decided,
    // so a streaming or buffering caller reads the WHOLE body from this same
    // descriptor starting at byte 0.
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
    const WAV: &[u8] = b"RIFF\x24\x08\x00\x00WAVEfmt more-bytes";
    const WEBP: &[u8] = b"RIFF\x24\x08\x00\x00WEBPVP8 more-bytes";
    const FLAC: &[u8] = b"fLaC\x00\x00\x00\x22more-bytes-here";
    const OGG: &[u8] = b"OggS\x00\x02\x00\x00more-bytes-here";

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
            ("e.wav", WAV, MediaKind::Wav),
            ("f.webp", WEBP, MediaKind::Webp),
            ("g.flac", FLAC, MediaKind::Flac),
            ("h.ogg", OGG, MediaKind::Ogg),
        ] {
            let p = write(dir.path(), name, bytes);
            let got = admit_path(&p, &any()).unwrap();
            assert_eq!(got.kind, kind, "{name}");
            assert_eq!(got.bytes, bytes, "{name} must return every byte exactly");
        }
    }

    /// A 8-byte prefix classified `RIFF....WAVE` as a WebP IMAGE, because
    /// `RIFF` alone was treated as the WebP signature. Once audio shares this
    /// intake that is a cross-class confusion, not a cosmetic one: a WAV would
    /// have been admitted to an image-only surface.
    #[test]
    fn riff_is_disambiguated_between_wav_and_webp() {
        assert_eq!(MediaKind::from_magic(WAV), Some(MediaKind::Wav));
        assert_eq!(MediaKind::from_magic(WEBP), Some(MediaKind::Webp));
        // RIFF with an unknown form is not admitted to either class.
        assert_eq!(MediaKind::from_magic(b"RIFF\x00\x00\x00\x00AVI "), None);
        // And the 8-byte prefix that used to be enough is now not enough to
        // claim WebP.
        assert_eq!(MediaKind::from_magic(b"RIFF\x00\x00\x00\x00"), None);
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
            ("wav-body.webp", WAV),
            ("png-body.wav", PNG),
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

    /// The body read is bounded by `take(max + 1)` on the SAME descriptor, so
    /// the ingest allocation can never exceed the cap by more than one byte
    /// even if the stat under-reported. Proved by reading the whole admitted
    /// body back and asserting the length.
    #[test]
    fn the_body_read_is_bounded_by_the_cap_not_by_the_stat() {
        let dir = tempdir().unwrap();
        let body = [PNG, &vec![3u8; 4096][..]].concat();
        let p = write(dir.path(), "big.png", &body);
        // A generous cap admits it and returns every byte.
        let ok = admit_path(&p, &IntakePolicy::new(1, 1 << 20)).unwrap();
        assert_eq!(ok.bytes.len(), body.len());
        // A cap below the body length refuses; nothing oversize is returned.
        assert!(matches!(
            admit_path(&p, &IntakePolicy::new(1, 100)),
            Err(IntakeError::TooLarge { .. })
        ));
    }

    #[test]
    fn an_accept_list_refuses_a_class_it_does_not_name() {
        let dir = tempdir().unwrap();
        let p = write(dir.path(), "doc.pdf", PDF);
        let images_only = any().accepting(IMAGE_KINDS);
        assert!(matches!(
            admit_path(&p, &images_only),
            Err(IntakeError::KindNotAccepted { .. })
        ));
        // And the converse: an image is refused by an audio-only surface.
        let png = write(dir.path(), "pic.png", PNG);
        assert!(matches!(
            admit_path(&png, &any().accepting(AUDIO_KINDS)),
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
        // The forward-slash spelling of the same share. Windows accepts it;
        // the pre-consolidation local check happened to catch it by matching a
        // bare `//` prefix, and the shared check catches it as an actual UNC
        // name. Asserted so the consolidation cannot quietly narrow the guard.
        assert!(matches!(
            admit_path(&PathBuf::from("//server/share/image.png"), &any()),
            Err(IntakeError::NetworkPath(_))
        ));
    }

    /// The one input whose CLASSIFICATION the UNC consolidation changed — and
    /// which core#409 c2 then changed again.
    ///
    /// `\\?\C:\…` is a verbatim path to a **local disk**. The local
    /// `is_network_path` this file used to carry matched any `\\` prefix, so it
    /// called that a network path and refused it as `NetworkPath`. The
    /// consolidation made it correctly not-UNC, and `validate_user_path`'s
    /// namespace guard then refused it as device/verbatim instead — still the
    /// wrong refusal, just an accurately named one. core#409 c2 measured that
    /// refusal reaching real workspaces (`std::fs::canonicalize` RETURNS this
    /// form on Windows), so the namespace guard now admits the verbatim DISK
    /// form and refuses only the device spellings.
    ///
    /// Both directions are pinned here, because widening the admit half
    /// without keeping the device half would be a bad trade.
    #[test]
    fn a_verbatim_local_path_is_refused_for_neither_namespace() {
        // No such file on this host, so intake still fails — but it must never
        // fail for being a network path or a device path.
        let p = PathBuf::from(r"\\?\C:\Users\alice\image.png");
        if let Err(err) = admit_path(&p, &any()) {
            assert!(
                !matches!(err, IntakeError::NetworkPath(_)),
                "a verbatim path to a local disk is not a network path; got {err:?}"
            );
            let msg = err.to_string();
            assert!(
                !msg.contains("device namespace") && !msg.contains("verbatim root"),
                "the verbatim DISK form must not be refused for its namespace \
                 (core#409 c2); got: {msg}"
            );
        }

        // The DEVICE namespace is still refused, and still on its namespace.
        let device = admit_path(&PathBuf::from(r"\\.\PhysicalDrive0"), &any())
            .expect_err("the device namespace must stay refused");
        assert!(
            device.to_string().contains("device namespace"),
            "expected the device-namespace refusal, got: {device}"
        );

        // And the verbatim UNC form is the opposite case: it IS a network
        // path, and must not fall into the device bucket.
        assert!(matches!(
            admit_path(&PathBuf::from(r"\\?\UNC\server\share\image.png"), &any()),
            Err(IntakeError::NetworkPath(_))
        ));
    }

    /// The UNC guard, asserted at the one place the intake now consults it.
    ///
    /// This test absorbs TWO predecessors so neither lane's assertions are
    /// lost to the merge:
    ///   * `vision_tools::is_network_path_flags_unc_only` (mine, relocated
    ///     when the guard moved into the chokepoint), and
    ///   * `vision_tools::unc_guard_flags_unc_on_every_platform`
    ///     (`lane/wal-followups`), whose site disappeared because
    ///     `load_local_image` now delegates to this module and no longer
    ///     carries a UNC check of its own.
    ///
    /// The union is asserted, not the intersection. The `\\?\C:\` case is
    /// theirs and is the one that matters: a verbatim path to a LOCAL disk is
    /// not a UNC share, and must not be refused with a message naming the
    /// wrong hazard. It is still refused — see
    /// `a_verbatim_local_path_is_still_refused_but_no_longer_as_a_network_path`.
    #[test]
    fn unc_guard_flags_unc_on_every_platform() {
        // Ordinary paths are never UNC (the common case).
        assert!(!is_unc_path(Path::new("/Users/me/x.png")));
        assert!(!is_unc_path(Path::new("relative/x.png")));
        // Every UNC spelling, on EVERY platform. The implementation this
        // replaced used `Component::Prefix`, which never matches on a Unix
        // target, so these were `#[cfg(windows)]`-gated and never ran here.
        assert!(is_unc_path(Path::new(r"\\server\share\x.png")));
        assert!(is_unc_path(Path::new("//server/share/x.png")));
        assert!(is_unc_path(Path::new(r"\\?\UNC\server\share\x.png")));
        // A verbatim path to a LOCAL disk is not a network path.
        assert!(!is_unc_path(Path::new(r"\\?\C:\Users\me\x.png")));
    }

    #[test]
    fn refuses_a_relative_path_and_a_traversal() {
        assert!(admit_path(Path::new("relative.png"), &any()).is_err());
        assert!(admit_path(Path::new("./relative.png"), &any()).is_err());
        let dir = tempdir().unwrap();
        let traversal = dir.path().join("..").join("escape.png");
        assert!(admit_path(&traversal, &any()).is_err());
    }

    /// The open must refuse a symlinked LEAF. That is the half that stops an
    /// attacker swapping the named file for a link to something they want read,
    /// and it is unconditional.
    #[cfg(unix)]
    #[test]
    fn refuses_a_symlinked_leaf() {
        use std::os::unix::fs::symlink;
        let dir = tempdir().unwrap();
        let target = write(dir.path(), "target.png", PNG);
        let link = dir.path().join("link.png");
        symlink(&target, &link).unwrap();
        assert!(admit_path(&link, &any()).is_err(), "symlinked leaf");
    }

    /// A symlinked ANCESTOR is resolved, not refused — and resolving it still
    /// does not reach a denied target.
    ///
    /// This reverses the older `refuses_a_symlinked_leaf_and_a_symlinked_parent`
    /// on its second half, deliberately. Blanket ancestor refusal is not
    /// satisfiable on macOS: `$TMPDIR` is `/var/folders/...` and `/var` is an
    /// OS-shipped symlink, so the rule closed every media and document surface
    /// on the platform (issue #937) while defending against a symlink no
    /// attacker placed. The threat the module actually names — a raced parent
    /// rename redirecting the final open — is still held by the `O_NOFOLLOW`
    /// walk over the resolved chain.
    ///
    /// HONESTY NOTE, because the first version of this test lied. Arm 2 is a
    /// regression guard, NOT a proof that this module closes the hole. It was
    /// originally written to demonstrate that a deny-list re-check added in
    /// `admit_open` was load-bearing; ablating that re-check left arm 2 GREEN,
    /// which showed the re-check contributed nothing — `validate_user_path`
    /// already canonicalises through symlinks and applies the deny-list to the
    /// target. The redundant call was removed and this comment records why arm
    /// 2 cannot fail by ablating anything in THIS file: the property it asserts
    /// is owned by `path_validation.rs`. Arm 1 is the arm that can fail, and it
    /// does fail without `resolve_ancestor_symlinks`.
    #[cfg(unix)]
    #[test]
    fn a_symlinked_ancestor_resolves_but_cannot_smuggle_a_denied_target() {
        use std::os::unix::fs::symlink;
        let dir = tempdir().unwrap();

        // Arm 1 — benign ancestor symlink is now ADMITTED. Without the fix this
        // is the exact shape every macOS temp path has.
        let actual_parent = dir.path().join("actual-parent");
        fs::create_dir(&actual_parent).unwrap();
        write(&actual_parent, "nested.png", PNG);
        let linked_parent = dir.path().join("linked-parent");
        symlink(&actual_parent, &linked_parent).unwrap();
        admit_path(&linked_parent.join("nested.png"), &any())
            .expect("a benign symlinked ancestor must be admitted");

        // Arm 2 — an ancestor symlink pointing into a denied location is still
        // refused, because the deny-list runs again over the RESOLVED path.
        let secrets = dir.path().join(".ssh");
        fs::create_dir(&secrets).unwrap();
        write(&secrets, "id_rsa", PNG);
        let linked_secrets = dir.path().join("innocuous");
        symlink(&secrets, &linked_secrets).unwrap();
        assert!(
            admit_path(&linked_secrets.join("id_rsa"), &any()).is_err(),
            "a symlinked ancestor must not smuggle a denied target past the deny-list"
        );
    }

    /// A FIFO must not block the open. `O_NONBLOCK` is what makes this a
    /// refusal rather than a hang, so the assertion is that the call RETURNS.
    #[cfg(unix)]
    #[test]
    fn a_fifo_is_refused_and_does_not_block() {
        use std::ffi::CString;
        use std::os::unix::ffi::OsStrExt as _;
        let dir = tempdir().unwrap();
        let fifo = dir.path().join("pipe.png");
        let c = CString::new(fifo.as_os_str().as_bytes()).unwrap();
        // SAFETY: `c` is a valid NUL-terminated pathname; 0o600 is a
        // conventional owner-only fixture permission.
        assert_eq!(unsafe { libc::mkfifo(c.as_ptr(), 0o600) }, 0);
        assert!(admit_path(&fifo, &any()).is_err());
    }

    /// Connector-supplied bytes face the SAME format decision as a path, so a
    /// channel cannot introduce a class a local path would refuse.
    #[test]
    fn connector_bytes_face_the_same_format_decision() {
        let images_only = any().accepting(IMAGE_KINDS);
        assert_eq!(admit_bytes(PNG, &images_only).unwrap(), MediaKind::Png);
        assert!(matches!(
            admit_bytes(PDF, &images_only),
            Err(IntakeError::KindNotAccepted { .. })
        ));
        assert!(matches!(
            admit_bytes(WAV, &images_only),
            Err(IntakeError::KindNotAccepted { .. })
        ));
        assert!(matches!(
            admit_bytes(b"junk", &images_only),
            Err(IntakeError::UnrecognisedFormat)
        ));
        let audio_only = any().accepting(AUDIO_KINDS);
        assert_eq!(admit_bytes(WAV, &audio_only).unwrap(), MediaKind::Wav);
    }

    /// The whole point of the module: the caller gets BYTES (or the same open
    /// handle), so there is no path left for a second resolution to disagree
    /// about. A repoint after admission cannot change what was admitted.
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

    /// `admit_open` hands back the SAME descriptor, rewound, so a streaming
    /// caller reads the bytes that were admitted rather than re-opening.
    #[test]
    fn admit_open_returns_the_same_handle_rewound_to_zero() {
        let dir = tempdir().unwrap();
        let p = write(dir.path(), "stream.docx", ZIP);
        let mut handle = admit_open(&p, &any()).unwrap();
        assert_eq!(handle.kind, MediaKind::Ooxml);
        assert_eq!(handle.len, ZIP.len() as u64);
        let mut read_back = Vec::new();
        handle.file.read_to_end(&mut read_back).unwrap();
        assert_eq!(read_back, ZIP, "handle must be positioned at byte 0");
    }
}
