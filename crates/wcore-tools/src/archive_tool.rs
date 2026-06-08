//! T7 — read-only archive handling tool (zip / tar / tar.gz).
//!
//! Plan v2 Tier 2B row "T7": read-only extraction, sandboxed,
//! zip-slip checked.
//!
//! `ArchiveTool` exposes two operations via an `action` discriminator:
//!
//! * `list` — enumerate archive entries with their uncompressed sizes.
//!   Touches no filesystem outside the archive.
//! * `extract` — extract every entry into a caller-supplied directory.
//!   READ-ONLY with respect to the archive: the tool never creates or
//!   modifies archives.
//!
//! Security — zip-slip / path-traversal:
//!
//! Archive entry names are attacker-controlled. A malicious archive can
//! carry an entry named `../../etc/cron.d/evil` or `/etc/passwd` to
//! write outside the extraction directory ("zip slip", CVE-2018-1002200
//! and friends). Before writing any entry, [`safe_join`] rejects:
//!
//!   1. absolute entry paths,
//!   2. any `..` (`ParentDir`) component,
//!   3. any entry that — after lexical join — escapes the destination
//!      directory.
//!
//! The destination directory itself is validated through
//! [`crate::path_validation::validate_user_path`] (absolute, no
//! traversal, no null byte, not an OS-secret location) so the tool
//! shares the same path-safety posture as `Read` / `Write` / `Edit`.
//!
//! Format detection is by file extension: `.zip` → zip;
//! `.tar.gz` / `.tgz` → gzip-wrapped tar; `.tar` → plain tar.

use std::path::{Component, Path, PathBuf};

use async_trait::async_trait;
use serde_json::{Value, json};

use wcore_protocol::events::ToolCategory;
use wcore_types::tool::{JsonSchema, ToolResult};

use crate::Tool;
use crate::path_validation::validate_user_path;

/// Cap on per-entry uncompressed bytes written during extraction.
/// Defends against decompression bombs without needing a streaming
/// budget. 512 MiB is generous for legitimate use.
const MAX_ENTRY_BYTES: u64 = 512 * 1024 * 1024;

/// Supported archive container formats, detected from the file path.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ArchiveKind {
    Zip,
    Tar,
    TarGz,
}

/// Detect the archive format from the path's extension.
fn detect_kind(path: &Path) -> Option<ArchiveKind> {
    let name = path.file_name()?.to_string_lossy().to_lowercase();
    if name.ends_with(".zip") {
        Some(ArchiveKind::Zip)
    } else if name.ends_with(".tar.gz") || name.ends_with(".tgz") {
        Some(ArchiveKind::TarGz)
    } else if name.ends_with(".tar") {
        Some(ArchiveKind::Tar)
    } else {
        None
    }
}

/// One entry in an archive listing.
struct Entry {
    name: String,
    size: u64,
    is_dir: bool,
}

/// Join an archive entry name onto `dest`, rejecting any path that
/// would escape the destination directory (zip-slip defense).
///
/// `dest` is assumed already validated/lex-normalized. Returns the
/// safe absolute target path, or an error describing why the entry
/// was refused.
fn safe_join(dest: &Path, entry_name: &str) -> Result<PathBuf, String> {
    if entry_name.contains('\0') {
        return Err(format!("entry name contains null byte: {entry_name:?}"));
    }

    let entry = Path::new(entry_name);

    // An absolute entry path (`/etc/passwd`, or `C:\...`) must never be
    // honored — it would ignore `dest` entirely.
    if entry.is_absolute() {
        return Err(format!(
            "refused archive entry with absolute path: {entry_name:?}"
        ));
    }

    let mut out = dest.to_path_buf();
    for comp in entry.components() {
        match comp {
            // `..` is the classic zip-slip vector — refuse outright
            // rather than trying to resolve it.
            Component::ParentDir => {
                return Err(format!(
                    "refused archive entry with `..` traversal: {entry_name:?}"
                ));
            }
            Component::CurDir => {}
            Component::Normal(seg) => out.push(seg),
            // A root or drive-prefix component inside a relative path
            // means the entry tried to re-anchor — refuse.
            Component::RootDir | Component::Prefix(_) => {
                return Err(format!(
                    "refused archive entry with root/prefix component: {entry_name:?}"
                ));
            }
        }
    }

    // Defense-in-depth: confirm the lexically-joined result still sits
    // under `dest`. `out` cannot escape given the component filtering
    // above, but this catches any future logic drift.
    if !out.starts_with(dest) {
        return Err(format!(
            "refused archive entry escaping destination: {entry_name:?}"
        ));
    }

    Ok(out)
}

/// List entries inside an archive without touching the filesystem
/// outside the archive itself.
fn list_archive(path: &Path, kind: ArchiveKind) -> Result<Vec<Entry>, String> {
    let file = std::fs::File::open(path).map_err(|e| format!("cannot open archive: {e}"))?;

    match kind {
        ArchiveKind::Zip => {
            let mut zip =
                zip::ZipArchive::new(file).map_err(|e| format!("not a valid zip archive: {e}"))?;
            let mut entries = Vec::with_capacity(zip.len());
            for i in 0..zip.len() {
                let entry = zip
                    .by_index(i)
                    .map_err(|e| format!("corrupt zip entry {i}: {e}"))?;
                entries.push(Entry {
                    name: entry.name().to_string(),
                    size: entry.size(),
                    is_dir: entry.is_dir(),
                });
            }
            Ok(entries)
        }
        ArchiveKind::Tar => list_tar(tar::Archive::new(file)),
        ArchiveKind::TarGz => {
            let gz = flate2::read::GzDecoder::new(file);
            list_tar(tar::Archive::new(gz))
        }
    }
}

/// Shared tar listing for plain and gzip-wrapped tar archives.
fn list_tar<R: std::io::Read>(mut archive: tar::Archive<R>) -> Result<Vec<Entry>, String> {
    let mut entries = Vec::new();
    let iter = archive
        .entries()
        .map_err(|e| format!("not a valid tar archive: {e}"))?;
    for entry in iter {
        let entry = entry.map_err(|e| format!("corrupt tar entry: {e}"))?;
        let header = entry.header();
        let is_dir = header.entry_type().is_dir();
        let size = header.size().unwrap_or(0);
        let name = entry
            .path()
            .map_err(|e| format!("invalid tar entry path: {e}"))?
            .to_string_lossy()
            .into_owned();
        entries.push(Entry { name, size, is_dir });
    }
    Ok(entries)
}

/// Extract every entry of an archive into `dest`, rejecting any
/// zip-slip entry before writing.
///
/// Returns the count of files written. `dest` is created if missing.
fn extract_archive(path: &Path, kind: ArchiveKind, dest: &Path) -> Result<usize, String> {
    std::fs::create_dir_all(dest)
        .map_err(|e| format!("cannot create destination {}: {e}", dest.display()))?;

    let file = std::fs::File::open(path).map_err(|e| format!("cannot open archive: {e}"))?;

    match kind {
        ArchiveKind::Zip => {
            let mut zip =
                zip::ZipArchive::new(file).map_err(|e| format!("not a valid zip archive: {e}"))?;
            let mut written = 0usize;
            for i in 0..zip.len() {
                let mut entry = zip
                    .by_index(i)
                    .map_err(|e| format!("corrupt zip entry {i}: {e}"))?;
                // `safe_join` is the zip-slip guard — runs before any
                // filesystem write.
                let target = safe_join(dest, entry.name())?;
                if entry.is_dir() {
                    std::fs::create_dir_all(&target)
                        .map_err(|e| format!("cannot create dir {}: {e}", target.display()))?;
                    continue;
                }
                if entry.size() > MAX_ENTRY_BYTES {
                    return Err(format!(
                        "archive entry {:?} exceeds {MAX_ENTRY_BYTES}-byte limit",
                        entry.name()
                    ));
                }
                if let Some(parent) = target.parent() {
                    std::fs::create_dir_all(parent)
                        .map_err(|e| format!("cannot create dir {}: {e}", parent.display()))?;
                }
                let mut out = std::fs::File::create(&target)
                    .map_err(|e| format!("cannot write {}: {e}", target.display()))?;
                std::io::copy(&mut entry, &mut out)
                    .map_err(|e| format!("error extracting {}: {e}", target.display()))?;
                written += 1;
            }
            Ok(written)
        }
        ArchiveKind::Tar => extract_tar(tar::Archive::new(file), dest),
        ArchiveKind::TarGz => {
            let gz = flate2::read::GzDecoder::new(file);
            extract_tar(tar::Archive::new(gz), dest)
        }
    }
}

/// Shared tar extraction for plain and gzip-wrapped tar archives.
///
/// Deliberately does NOT use `tar`'s built-in `unpack` — that path
/// has its own traversal handling, but routing every entry through
/// [`safe_join`] keeps a single audited zip-slip guard for all
/// formats.
fn extract_tar<R: std::io::Read>(
    mut archive: tar::Archive<R>,
    dest: &Path,
) -> Result<usize, String> {
    let mut written = 0usize;
    let iter = archive
        .entries()
        .map_err(|e| format!("not a valid tar archive: {e}"))?;
    for entry in iter {
        let mut entry = entry.map_err(|e| format!("corrupt tar entry: {e}"))?;
        let header = entry.header();
        let is_dir = header.entry_type().is_dir();
        let size = header.size().unwrap_or(0);
        let name = entry
            .path()
            .map_err(|e| format!("invalid tar entry path: {e}"))?
            .to_string_lossy()
            .into_owned();
        let target = safe_join(dest, &name)?;
        if is_dir {
            std::fs::create_dir_all(&target)
                .map_err(|e| format!("cannot create dir {}: {e}", target.display()))?;
            continue;
        }
        if size > MAX_ENTRY_BYTES {
            return Err(format!(
                "archive entry {name:?} exceeds {MAX_ENTRY_BYTES}-byte limit"
            ));
        }
        if let Some(parent) = target.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|e| format!("cannot create dir {}: {e}", parent.display()))?;
        }
        let mut out = std::fs::File::create(&target)
            .map_err(|e| format!("cannot write {}: {e}", target.display()))?;
        std::io::copy(&mut entry, &mut out)
            .map_err(|e| format!("error extracting {}: {e}", target.display()))?;
        written += 1;
    }
    Ok(written)
}

/// Read-only archive list + extraction tool.
pub struct ArchiveTool;

#[async_trait]
impl Tool for ArchiveTool {
    fn name(&self) -> &str {
        "Archive"
    }

    fn description(&self) -> &str {
        "Read-only archive handling for zip and tar archives.\n\n\
         Actions:\n\
         - list: enumerate archive entries with their uncompressed sizes.\n\
         - extract: extract all entries into a destination directory.\n\n\
         Usage:\n\
         - Supports .zip, .tar, .tar.gz, and .tgz (format detected by extension).\n\
         - This tool NEVER creates or modifies archives — it is read-only.\n\
         - archive_path and (for extract) dest_dir must be absolute paths.\n\
         - Entries with `..` traversal or absolute paths are rejected (zip-slip safe)."
    }

    fn input_schema(&self) -> JsonSchema {
        json!({
            "type": "object",
            "properties": {
                "action": {
                    "type": "string",
                    "enum": ["list", "extract"],
                    "description": "Operation to perform"
                },
                "archive_path": {
                    "type": "string",
                    "description": "Absolute path to the .zip/.tar/.tar.gz archive"
                },
                "dest_dir": {
                    "type": "string",
                    "description": "Absolute destination directory (required for extract)"
                }
            },
            "required": ["action", "archive_path"]
        })
    }

    fn is_concurrency_safe(&self, input: &Value) -> bool {
        // `list` only reads; `extract` writes to dest_dir.
        input["action"].as_str() == Some("list")
    }

    async fn execute(&self, input: Value) -> ToolResult {
        let Some(action) = input["action"].as_str() else {
            return err("Missing required parameter: action");
        };
        let Some(archive_path) = input["archive_path"].as_str() else {
            return err("Missing required parameter: archive_path");
        };

        let archive = match validate_user_path(Path::new(archive_path)) {
            Ok(p) => p,
            Err(e) => return err(&format!("Refused archive_path {archive_path}: {e}")),
        };

        let Some(kind) = detect_kind(&archive) else {
            return err(&format!(
                "Unsupported archive type for {archive_path} (expected .zip/.tar/.tar.gz/.tgz)"
            ));
        };

        match action {
            "list" => match list_archive(&archive, kind) {
                Ok(entries) => {
                    if entries.is_empty() {
                        return ToolResult {
                            content: "Archive is empty".to_string(),
                            is_error: false,
                        };
                    }
                    let mut lines = Vec::with_capacity(entries.len() + 1);
                    lines.push(format!("{} entries:", entries.len()));
                    for e in &entries {
                        if e.is_dir {
                            lines.push(format!("  {}/ (dir)", e.name));
                        } else {
                            lines.push(format!("  {} ({} bytes)", e.name, e.size));
                        }
                    }
                    ToolResult {
                        content: lines.join("\n"),
                        is_error: false,
                    }
                }
                Err(e) => err(&format!("Failed to list {archive_path}: {e}")),
            },
            "extract" => {
                let Some(dest_dir) = input["dest_dir"].as_str() else {
                    return err("Missing required parameter for extract: dest_dir");
                };
                let dest = match validate_user_path(Path::new(dest_dir)) {
                    Ok(p) => p,
                    Err(e) => return err(&format!("Refused dest_dir {dest_dir}: {e}")),
                };
                match extract_archive(&archive, kind, &dest) {
                    Ok(n) => ToolResult {
                        content: format!("Extracted {n} file(s) to {}", dest.display()),
                        is_error: false,
                    },
                    Err(e) => err(&format!("Failed to extract {archive_path}: {e}")),
                }
            }
            other => err(&format!("Unknown action: {other} (expected list|extract)")),
        }
    }

    fn category(&self) -> ToolCategory {
        ToolCategory::Info
    }

    fn describe(&self, input: &Value) -> String {
        let action = input.get("action").and_then(|v| v.as_str()).unwrap_or("?");
        let path = input
            .get("archive_path")
            .and_then(|v| v.as_str())
            .unwrap_or("?");
        format!("Archive {action}: {path}")
    }
}

fn err(msg: &str) -> ToolResult {
    ToolResult {
        content: msg.to_string(),
        is_error: true,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use tempfile::TempDir;

    /// Build a small zip archive at `path` from `(name, contents)` pairs.
    fn build_zip(path: &Path, entries: &[(&str, &[u8])]) {
        let file = std::fs::File::create(path).unwrap();
        let mut zip = zip::ZipWriter::new(file);
        let opts: zip::write::FileOptions<'_, ()> =
            zip::write::FileOptions::default().compression_method(zip::CompressionMethod::Deflated);
        for (name, body) in entries {
            zip.start_file(*name, opts).unwrap();
            zip.write_all(body).unwrap();
        }
        zip.finish().unwrap();
    }

    /// Build a small gzip-wrapped tar archive from `(name, contents)` pairs.
    fn build_tar_gz(path: &Path, entries: &[(&str, &[u8])]) {
        let file = std::fs::File::create(path).unwrap();
        let gz = flate2::write::GzEncoder::new(file, flate2::Compression::default());
        let mut tar = tar::Builder::new(gz);
        for (name, body) in entries {
            let mut header = tar::Header::new_gnu();
            header.set_size(body.len() as u64);
            header.set_mode(0o644);
            header.set_cksum();
            tar.append_data(&mut header, name, &body[..]).unwrap();
        }
        tar.into_inner().unwrap().finish().unwrap();
    }

    async fn run(input: Value) -> ToolResult {
        ArchiveTool.execute(input).await
    }

    #[tokio::test]
    async fn list_zip_archive() {
        let dir = TempDir::new().unwrap();
        let zip_path = dir.path().join("sample.zip");
        build_zip(
            &zip_path,
            &[("readme.txt", b"hello"), ("data/nums.csv", b"1,2,3")],
        );

        let result = run(json!({
            "action": "list",
            "archive_path": zip_path.to_str().unwrap(),
        }))
        .await;

        assert!(!result.is_error, "list should succeed: {}", result.content);
        assert!(result.content.contains("readme.txt (5 bytes)"));
        assert!(result.content.contains("data/nums.csv (5 bytes)"));
        assert!(result.content.contains("2 entries"));
    }

    #[tokio::test]
    async fn list_tar_gz_archive() {
        let dir = TempDir::new().unwrap();
        let tgz_path = dir.path().join("sample.tar.gz");
        build_tar_gz(
            &tgz_path,
            &[("note.md", b"# title"), ("bin", b"\x00\x01\x02")],
        );

        let result = run(json!({
            "action": "list",
            "archive_path": tgz_path.to_str().unwrap(),
        }))
        .await;

        assert!(!result.is_error, "list should succeed: {}", result.content);
        assert!(result.content.contains("note.md (7 bytes)"));
        assert!(result.content.contains("bin (3 bytes)"));
    }

    #[tokio::test]
    async fn extract_zip_to_tempdir() {
        let dir = TempDir::new().unwrap();
        let zip_path = dir.path().join("payload.zip");
        build_zip(
            &zip_path,
            &[("top.txt", b"top-level"), ("nested/deep.txt", b"deep")],
        );
        let dest = TempDir::new().unwrap();

        let result = run(json!({
            "action": "extract",
            "archive_path": zip_path.to_str().unwrap(),
            "dest_dir": dest.path().to_str().unwrap(),
        }))
        .await;

        assert!(
            !result.is_error,
            "extract should succeed: {}",
            result.content
        );
        let top = std::fs::read_to_string(dest.path().join("top.txt")).unwrap();
        assert_eq!(top, "top-level");
        let deep = std::fs::read_to_string(dest.path().join("nested/deep.txt")).unwrap();
        assert_eq!(deep, "deep");
    }

    #[tokio::test]
    async fn extract_tar_gz_to_tempdir() {
        let dir = TempDir::new().unwrap();
        let tgz_path = dir.path().join("payload.tar.gz");
        build_tar_gz(&tgz_path, &[("docs/intro.txt", b"intro body")]);
        let dest = TempDir::new().unwrap();

        let result = run(json!({
            "action": "extract",
            "archive_path": tgz_path.to_str().unwrap(),
            "dest_dir": dest.path().to_str().unwrap(),
        }))
        .await;

        assert!(
            !result.is_error,
            "extract should succeed: {}",
            result.content
        );
        let body = std::fs::read_to_string(dest.path().join("docs/intro.txt")).unwrap();
        assert_eq!(body, "intro body");
    }

    /// A malicious archive carrying a `../` traversal entry must be
    /// rejected before anything is written outside the destination.
    #[tokio::test]
    async fn extract_rejects_zip_slip_traversal() {
        let dir = TempDir::new().unwrap();
        let evil_path = dir.path().join("evil.zip");
        // Entry name escapes the destination directory.
        build_zip(
            &evil_path,
            &[("../../escaped.txt", b"pwned"), ("ok.txt", b"fine")],
        );
        let dest = TempDir::new().unwrap();

        let result = run(json!({
            "action": "extract",
            "archive_path": evil_path.to_str().unwrap(),
            "dest_dir": dest.path().to_str().unwrap(),
        }))
        .await;

        assert!(
            result.is_error,
            "zip-slip extraction must be refused, got: {}",
            result.content
        );
        assert!(
            result.content.contains("traversal") || result.content.contains(".."),
            "error should name the traversal: {}",
            result.content
        );
        // The escaping file must NOT have been created anywhere near dest.
        let escaped = dir.path().join("escaped.txt");
        assert!(
            !escaped.exists(),
            "traversal entry must not be written to disk"
        );
    }

    #[tokio::test]
    async fn list_rejects_corrupt_archive() {
        let dir = TempDir::new().unwrap();
        let bad_path = dir.path().join("broken.zip");
        // Not a real zip — just garbage bytes with a .zip extension.
        std::fs::write(&bad_path, b"this is definitely not a zip file").unwrap();

        let result = run(json!({
            "action": "list",
            "archive_path": bad_path.to_str().unwrap(),
        }))
        .await;

        assert!(result.is_error, "corrupt archive should be an error");
        assert!(
            result.content.contains("Failed to list"),
            "error should explain the failure: {}",
            result.content
        );
    }

    #[cfg(unix)]
    #[test]
    fn safe_join_rejects_absolute_entry() {
        let dest = Path::new("/tmp/wcore-extract");
        let err = safe_join(dest, "/etc/passwd").unwrap_err();
        assert!(err.contains("absolute"), "got: {err}");
    }

    #[cfg(windows)]
    #[test]
    fn safe_join_rejects_absolute_entry() {
        // Path::new("/etc/passwd").is_absolute() is FALSE on Windows
        // because there's no drive letter, so a Unix-shaped absolute is
        // not the right reproducer here. Use a real Windows absolute.
        let dest = Path::new(r"C:\Temp\wcore-extract");
        let err = safe_join(dest, r"C:\Windows\System32\cmd.exe").unwrap_err();
        assert!(err.contains("absolute"), "got: {err}");
    }

    #[cfg(unix)]
    #[test]
    fn safe_join_allows_normal_nested_entry() {
        let dest = Path::new("/tmp/wcore-extract");
        let ok = safe_join(dest, "a/b/c.txt").unwrap();
        assert_eq!(ok, PathBuf::from("/tmp/wcore-extract/a/b/c.txt"));
    }

    #[cfg(windows)]
    #[test]
    fn safe_join_allows_normal_nested_entry() {
        let dest = Path::new(r"C:\Temp\wcore-extract");
        let ok = safe_join(dest, "a/b/c.txt").unwrap();
        // Path::join uses the platform separator (`\` on Windows) so we
        // assert the resulting components rather than a literal string.
        assert_eq!(ok, dest.join("a").join("b").join("c.txt"));
    }
}
