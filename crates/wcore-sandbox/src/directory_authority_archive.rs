//! Bounded archive transport rooted in retained directory capabilities.

use super::*;
use std::collections::BTreeMap;
use std::io::{Cursor, Read};

pub(super) const IMPORT_JOURNAL: &str = ".wayland-workspace-import";
const IMPORT_JOURNAL_MAGIC: &[u8; 16] = b"WLIMPORTJOURNAL1";

#[derive(Clone, Copy, Debug)]
pub(crate) struct DirectoryArchiveLimits {
    pub max_entries: usize,
    pub max_bytes: u64,
    pub max_depth: usize,
}

impl DirectoryArchiveLimits {
    #[cfg(feature = "live-docker")]
    pub(crate) fn encoded_limit(self) -> Result<u64> {
        encoded_limit(self)
    }
}

#[derive(Clone, Debug)]
enum SnapshotEntry {
    Directory {
        path: PathBuf,
        mode: u32,
    },
    File {
        path: PathBuf,
        mode: u32,
        contents: Vec<u8>,
    },
}

impl SnapshotEntry {
    fn path(&self) -> &Path {
        match self {
            Self::Directory { path, .. } | Self::File { path, .. } => path,
        }
    }
}

impl DirectoryAuthority {
    /// Serialize a tree without reopening the retained root pathname.
    pub(crate) fn export_tar_bounded(
        &self,
        prefix: &str,
        denied: &[PathBuf],
        limits: DirectoryArchiveLimits,
    ) -> Result<Vec<u8>> {
        validate_child_name(prefix)?;
        self.validate_path(self.display_path())?;
        let entries = snapshot_tree(self, denied, limits)?;
        self.validate_path(self.display_path())?;

        encode_archive(&entries, prefix, limits)
    }
}

impl RetainedWorkspaceAuthority {
    #[cfg(feature = "live-docker")]
    pub(crate) fn export_tar_bounded(
        &self,
        prefix: &str,
        denied: &[PathBuf],
        limits: DirectoryArchiveLimits,
    ) -> Result<Vec<u8>> {
        self.validate()?;
        let archive = self.workspace.export_tar_bounded(prefix, denied, limits)?;
        self.validate()?;
        Ok(archive)
    }

    /// Restore an interrupted import before any new execution is admitted.
    /// The journal is owner-relative and identity-bound to this exact child.
    pub(crate) fn recover_pending_import(&self, limits: DirectoryArchiveLimits) -> Result<bool> {
        self.validate()?;
        let Some(bytes) = self
            .owner
            .read_child_bounded(IMPORT_JOURNAL, journal_limit(limits)?)?
        else {
            return Ok(false);
        };
        let journal = decode_journal(&bytes, limits)?;
        if journal.child_name != self.child_name
            || journal.transaction_id != self.transaction_id
            || journal.owner_identity != identity_binding(&self.owner.identity)
            || journal.workspace_identity != identity_binding(&self.workspace.identity)
        {
            return Err(SandboxError::PathDenied(
                "workspace import journal authority binding does not match this transaction"
                    .to_owned(),
            ));
        }
        let original = parse_archive(&journal.archive, &journal.prefix, limits)?;
        replace_tree(&self.workspace, &original, |_| {})?;
        self.validate()?;
        self.remove_journal()?;
        Ok(true)
    }

    pub(crate) fn replace_from_tar_bounded(
        &self,
        archive: &[u8],
        prefix: &str,
        limits: DirectoryArchiveLimits,
    ) -> Result<()> {
        self.replace_from_tar_bounded_inner(archive, prefix, limits, |_| {})
    }

    fn replace_from_tar_bounded_inner(
        &self,
        archive: &[u8],
        prefix: &str,
        limits: DirectoryArchiveLimits,
        mut hook: impl FnMut(ImportStage),
    ) -> Result<()> {
        validate_child_name(prefix)?;
        ensure_encoded_bound(archive, limits)?;
        let replacement = parse_archive(archive, prefix, limits)?;
        self.validate()?;

        let original = snapshot_tree(&self.workspace, &[], limits)?;
        let original_archive = encode_archive(&original, prefix, limits)?;
        let journal = encode_journal(self, prefix, &original_archive, limits)?;
        let journal_authority = self
            .owner
            .create_child_file(IMPORT_JOURNAL, &journal)
            .map_err(|error| match error {
                SandboxError::Io(io) if io.kind() == std::io::ErrorKind::AlreadyExists => {
                    SandboxError::ExecFailed(
                        "workspace import is already active or requires exclusive recovery"
                            .to_owned(),
                    )
                }
                other => other,
            })?;
        self.owner.sync()?;
        hook(ImportStage::JournalDurable);

        if let Err(apply_error) = replace_tree(&self.workspace, &replacement, &mut hook) {
            let recovery = replace_tree(&self.workspace, &original, |_| {})
                .and_then(|()| self.validate())
                .and_then(|()| self.remove_journal_authority(journal_authority));
            return match recovery {
                Ok(()) => Err(apply_error),
                Err(recovery_error) => Err(SandboxError::ExecFailed(format!(
                    "workspace import failed ({apply_error}); durable recovery failed ({recovery_error})"
                ))),
            };
        }
        self.validate()?;
        self.remove_journal_authority(journal_authority)?;
        Ok(())
    }

    fn remove_journal(&self) -> Result<()> {
        let authority = self.owner.open_child_file(IMPORT_JOURNAL)?;
        self.remove_journal_authority(authority)
    }

    fn remove_journal_authority(&self, authority: RegularFileAuthority) -> Result<()> {
        self.owner.remove_child_file(IMPORT_JOURNAL, authority)?;
        self.owner.sync()
    }

    #[cfg(test)]
    pub(super) fn replace_from_tar_bounded_with_hook(
        &self,
        archive: &[u8],
        prefix: &str,
        limits: DirectoryArchiveLimits,
        hook: impl FnMut(ImportStage),
    ) -> Result<()> {
        self.replace_from_tar_bounded_inner(archive, prefix, limits, hook)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum ImportStage {
    JournalDurable,
    DescendantsRemoved,
    DirectoryCreated,
    FileWritten,
}

struct ImportJournal {
    child_name: String,
    prefix: String,
    transaction_id: String,
    owner_identity: Vec<u8>,
    workspace_identity: Vec<u8>,
    archive: Vec<u8>,
}

fn encode_journal(
    authority: &RetainedWorkspaceAuthority,
    prefix: &str,
    archive: &[u8],
    limits: DirectoryArchiveLimits,
) -> Result<Vec<u8>> {
    ensure_encoded_bound(archive, limits)?;
    let child = authority.child_name.as_bytes();
    let prefix = prefix.as_bytes();
    let transaction = authority.transaction_id.as_bytes();
    let owner_identity = identity_binding(&authority.owner.identity);
    let workspace_identity = identity_binding(&authority.workspace.identity);
    let to_u32 = |length: usize| {
        u32::try_from(length)
            .map_err(|_| SandboxError::PathDenied("journal field is too long".to_owned()))
    };
    let child_len = to_u32(child.len())?;
    let prefix_len = to_u32(prefix.len())?;
    let transaction_len = to_u32(transaction.len())?;
    let owner_len = to_u32(owner_identity.len())?;
    let workspace_len = to_u32(workspace_identity.len())?;
    let archive_len = u64::try_from(archive.len())
        .map_err(|_| SandboxError::PathDenied("archive length overflowed".to_owned()))?;
    let mut encoded = Vec::with_capacity(
        IMPORT_JOURNAL_MAGIC.len()
            + 5 * 4
            + 8
            + child.len()
            + prefix.len()
            + transaction.len()
            + owner_identity.len()
            + workspace_identity.len()
            + archive.len(),
    );
    encoded.extend_from_slice(IMPORT_JOURNAL_MAGIC);
    encoded.extend_from_slice(&child_len.to_be_bytes());
    encoded.extend_from_slice(&prefix_len.to_be_bytes());
    encoded.extend_from_slice(&transaction_len.to_be_bytes());
    encoded.extend_from_slice(&owner_len.to_be_bytes());
    encoded.extend_from_slice(&workspace_len.to_be_bytes());
    encoded.extend_from_slice(&archive_len.to_be_bytes());
    encoded.extend_from_slice(child);
    encoded.extend_from_slice(prefix);
    encoded.extend_from_slice(transaction);
    encoded.extend_from_slice(&owner_identity);
    encoded.extend_from_slice(&workspace_identity);
    encoded.extend_from_slice(archive);
    Ok(encoded)
}

fn decode_journal(bytes: &[u8], limits: DirectoryArchiveLimits) -> Result<ImportJournal> {
    const HEADER: usize = 16 + 5 * 4 + 8;
    if bytes.len() < HEADER || &bytes[..16] != IMPORT_JOURNAL_MAGIC {
        return Err(SandboxError::PathDenied(
            "workspace import journal has an invalid header".to_owned(),
        ));
    }
    let child_len = u32::from_be_bytes(bytes[16..20].try_into().expect("fixed slice")) as usize;
    let prefix_len = u32::from_be_bytes(bytes[20..24].try_into().expect("fixed slice")) as usize;
    let transaction_len =
        u32::from_be_bytes(bytes[24..28].try_into().expect("fixed slice")) as usize;
    let owner_len = u32::from_be_bytes(bytes[28..32].try_into().expect("fixed slice")) as usize;
    let workspace_len = u32::from_be_bytes(bytes[32..36].try_into().expect("fixed slice")) as usize;
    let archive_len = u64::from_be_bytes(bytes[36..44].try_into().expect("fixed slice"));
    let archive_len = usize::try_from(archive_len)
        .map_err(|_| SandboxError::PathDenied("journal archive is too large".to_owned()))?;
    let expected = HEADER
        .checked_add(child_len)
        .and_then(|length| length.checked_add(prefix_len))
        .and_then(|length| length.checked_add(transaction_len))
        .and_then(|length| length.checked_add(owner_len))
        .and_then(|length| length.checked_add(workspace_len))
        .and_then(|length| length.checked_add(archive_len))
        .ok_or_else(|| SandboxError::PathDenied("journal length overflowed".to_owned()))?;
    if expected != bytes.len() {
        return Err(SandboxError::PathDenied(
            "workspace import journal length is inconsistent".to_owned(),
        ));
    }
    let child_end = HEADER + child_len;
    let prefix_end = child_end + prefix_len;
    let transaction_end = prefix_end + transaction_len;
    let owner_end = transaction_end + owner_len;
    let workspace_end = owner_end + workspace_len;
    let child_name = std::str::from_utf8(&bytes[HEADER..child_end])
        .map_err(|_| SandboxError::PathDenied("journal child name is not UTF-8".to_owned()))?
        .to_owned();
    let prefix = std::str::from_utf8(&bytes[child_end..prefix_end])
        .map_err(|_| SandboxError::PathDenied("journal prefix is not UTF-8".to_owned()))?
        .to_owned();
    let transaction_id = std::str::from_utf8(&bytes[prefix_end..transaction_end])
        .map_err(|_| SandboxError::PathDenied("journal transaction is not UTF-8".to_owned()))?
        .to_owned();
    validate_child_name(&child_name)?;
    validate_child_name(&prefix)?;
    if transaction_id.is_empty() || transaction_id.len() > 256 || transaction_id.contains('\0') {
        return Err(SandboxError::PathDenied(
            "journal transaction ID is invalid".to_owned(),
        ));
    }
    let owner_identity = bytes[transaction_end..owner_end].to_vec();
    let workspace_identity = bytes[owner_end..workspace_end].to_vec();
    let archive = bytes[workspace_end..].to_vec();
    ensure_encoded_bound(&archive, limits)?;
    Ok(ImportJournal {
        child_name,
        prefix,
        transaction_id,
        owner_identity,
        workspace_identity,
        archive,
    })
}

fn identity_binding(identity: &DirectoryIdentity) -> Vec<u8> {
    #[cfg(unix)]
    {
        let mut encoded = Vec::with_capacity(17);
        encoded.push(b'U');
        encoded.extend_from_slice(&identity.device.to_be_bytes());
        encoded.extend_from_slice(&identity.inode.to_be_bytes());
        encoded
    }
    #[cfg(windows)]
    {
        let mut encoded = Vec::with_capacity(25);
        encoded.push(b'W');
        encoded.extend_from_slice(&identity.volume.to_be_bytes());
        encoded.extend_from_slice(&identity.file_id);
        encoded
    }
    #[cfg(not(any(unix, windows)))]
    {
        Vec::new()
    }
}

fn journal_limit(limits: DirectoryArchiveLimits) -> Result<u64> {
    encoded_limit(limits)?
        .checked_add(4096)
        .ok_or_else(|| SandboxError::PathDenied("journal size bound overflowed".to_owned()))
}

fn snapshot_tree(
    root: &DirectoryAuthority,
    denied: &[PathBuf],
    limits: DirectoryArchiveLimits,
) -> Result<Vec<SnapshotEntry>> {
    let mut entries = Vec::new();
    let mut identities = Vec::new();
    let mut bytes = 0;
    snapshot_directory(
        root,
        Path::new(""),
        denied,
        limits,
        &mut entries,
        &mut identities,
        &mut bytes,
    )?;
    validate_casefold_uniqueness(entries.iter().map(SnapshotEntry::path))?;
    Ok(entries)
}

fn encode_archive(
    entries: &[SnapshotEntry],
    prefix: &str,
    limits: DirectoryArchiveLimits,
) -> Result<Vec<u8>> {
    let mut builder = tar::Builder::new(Vec::new());
    append_directory(&mut builder, Path::new(prefix), 0o700)?;
    for entry in entries {
        let archive_path = Path::new(prefix).join(entry.path());
        match entry {
            SnapshotEntry::Directory { mode, .. } => {
                append_directory(&mut builder, &archive_path, *mode)?;
            }
            SnapshotEntry::File { mode, contents, .. } => {
                let mut header = tar::Header::new_gnu();
                header.set_entry_type(tar::EntryType::Regular);
                header.set_mode(*mode);
                header.set_size(contents.len() as u64);
                header.set_mtime(0);
                header.set_cksum();
                builder.append_data(&mut header, &archive_path, contents.as_slice())?;
            }
        }
    }
    builder.finish()?;
    let archive = builder.into_inner()?;
    ensure_encoded_bound(&archive, limits)?;
    Ok(archive)
}

fn encoded_limit(limits: DirectoryArchiveLimits) -> Result<u64> {
    let overhead = limits
        .max_entries
        .checked_add(2)
        .and_then(|entries| entries.checked_mul(1024))
        .and_then(|bytes| u64::try_from(bytes).ok())
        .ok_or_else(|| SandboxError::PathDenied("archive size bound overflowed".to_owned()))?;
    limits
        .max_bytes
        .checked_add(overhead)
        .ok_or_else(|| SandboxError::PathDenied("archive encoded-size bound overflowed".to_owned()))
}

fn ensure_encoded_bound(archive: &[u8], limits: DirectoryArchiveLimits) -> Result<()> {
    let limit = encoded_limit(limits)?;
    if archive.len() as u64 > limit {
        return Err(SandboxError::PathDenied(format!(
            "encoded authority archive exceeds {limit} bytes"
        )));
    }
    Ok(())
}

fn snapshot_directory(
    directory: &DirectoryAuthority,
    relative: &Path,
    denied: &[PathBuf],
    limits: DirectoryArchiveLimits,
    entries: &mut Vec<SnapshotEntry>,
    identities: &mut Vec<DirectoryIdentity>,
    bytes: &mut u64,
) -> Result<()> {
    for name in directory.child_names()? {
        let path = relative.join(&name);
        if denied.iter().any(|candidate| {
            path.as_path() == candidate.as_path() || path.starts_with(candidate.as_path())
        }) {
            continue;
        }
        let depth = path.components().count();
        if depth > limits.max_depth {
            return Err(SandboxError::PathDenied(format!(
                "authority archive path exceeds depth {}: {}",
                limits.max_depth,
                path.display()
            )));
        }
        if entries.len() >= limits.max_entries {
            return Err(SandboxError::PathDenied(format!(
                "authority archive exceeds {} entries",
                limits.max_entries
            )));
        }
        match directory.open_child_directory(&name) {
            Ok(child) => {
                entries.push(SnapshotEntry::Directory {
                    path: path.clone(),
                    mode: 0o700,
                });
                snapshot_directory(&child, &path, denied, limits, entries, identities, bytes)?;
            }
            Err(directory_error) => {
                let file = directory.open_child_file(&name).map_err(|file_error| {
                    SandboxError::PathDenied(format!(
                        "authority archive rejected {}: directory open failed ({directory_error}); regular-file open failed ({file_error})",
                        path.display()
                    ))
                })?;
                reject_hard_link(&file, &path)?;
                if identities.contains(&file.identity) {
                    return Err(SandboxError::PathDenied(format!(
                        "authority archive rejected hard-linked file: {}",
                        path.display()
                    )));
                }
                identities.push(file.identity);
                let remaining = limits.max_bytes.saturating_sub(*bytes);
                let contents = file.read_bounded(remaining)?;
                *bytes = bytes.checked_add(contents.len() as u64).ok_or_else(|| {
                    SandboxError::PathDenied("authority archive byte count overflowed".to_owned())
                })?;
                if *bytes > limits.max_bytes {
                    return Err(SandboxError::PathDenied(format!(
                        "authority archive exceeds {} bytes",
                        limits.max_bytes
                    )));
                }
                entries.push(SnapshotEntry::File {
                    path,
                    mode: file_mode(&file)?,
                    contents,
                });
            }
        }
    }
    Ok(())
}

fn reject_hard_link(file: &RegularFileAuthority, path: &Path) -> Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        if file.handle.metadata()?.nlink() > 1 {
            return Err(SandboxError::PathDenied(format!(
                "authority archive rejected hard-linked file: {}",
                path.display()
            )));
        }
    }
    #[cfg(windows)]
    {
        use std::os::windows::io::AsRawHandle;
        use windows_sys::Win32::Storage::FileSystem::{
            BY_HANDLE_FILE_INFORMATION, GetFileInformationByHandle,
        };
        let mut info = std::mem::MaybeUninit::<BY_HANDLE_FILE_INFORMATION>::zeroed();
        // SAFETY: the retained regular-file authority owns a valid handle and
        // `info` is initialized by the successful Windows API call.
        if unsafe {
            GetFileInformationByHandle(file.handle.as_raw_handle() as _, info.as_mut_ptr())
        } == 0
        {
            return Err(std::io::Error::last_os_error().into());
        }
        if unsafe { info.assume_init() }.NumberOfLinks > 1 {
            return Err(SandboxError::PathDenied(format!(
                "authority archive rejected hard-linked file: {}",
                path.display()
            )));
        }
    }
    Ok(())
}

fn parse_archive(
    archive: &[u8],
    prefix: &str,
    limits: DirectoryArchiveLimits,
) -> Result<Vec<SnapshotEntry>> {
    let mut parsed = BTreeMap::<PathBuf, SnapshotEntry>::new();
    let mut bytes = 0_u64;
    let mut root_seen = false;
    let mut reader = tar::Archive::new(Cursor::new(archive));
    for entry in reader.entries()? {
        let entry = entry?;
        let path = normalized_archive_path(entry.path()?.as_ref())?;
        let mut components = path.components();
        let Some(std::path::Component::Normal(root)) = components.next() else {
            return Err(SandboxError::PathDenied(
                "archive entry has no root".to_owned(),
            ));
        };
        if root != std::ffi::OsStr::new(prefix) {
            return Err(SandboxError::PathDenied(format!(
                "archive entry escaped {prefix}: {}",
                path.display()
            )));
        }
        let relative = components.collect::<PathBuf>();
        let entry_type = entry.header().entry_type();
        if relative.as_os_str().is_empty() {
            if !entry_type.is_dir() || root_seen {
                return Err(SandboxError::PathDenied(
                    "archive root must be one unique directory".to_owned(),
                ));
            }
            root_seen = true;
            continue;
        }
        if relative.components().count() > limits.max_depth {
            return Err(SandboxError::PathDenied(format!(
                "archive entry exceeds depth {}: {}",
                limits.max_depth,
                relative.display()
            )));
        }
        if parsed.len() >= limits.max_entries {
            return Err(SandboxError::PathDenied(format!(
                "archive exceeds {} entries",
                limits.max_entries
            )));
        }
        let mode = entry.header().mode()? & 0o777;
        let value = if entry_type.is_dir() {
            SnapshotEntry::Directory {
                path: relative.clone(),
                mode,
            }
        } else if entry_type.is_file() {
            let declared = entry.header().size()?;
            let remaining = limits.max_bytes.saturating_sub(bytes);
            if declared > remaining {
                return Err(SandboxError::PathDenied(format!(
                    "archive exceeds {} bytes",
                    limits.max_bytes
                )));
            }
            let mut contents = Vec::with_capacity(declared as usize);
            entry
                .take(remaining.saturating_add(1))
                .read_to_end(&mut contents)?;
            if contents.len() as u64 != declared {
                return Err(SandboxError::PathDenied(format!(
                    "archive size mismatch for {}",
                    relative.display()
                )));
            }
            bytes = bytes.checked_add(declared).ok_or_else(|| {
                SandboxError::PathDenied("archive byte count overflowed".to_owned())
            })?;
            SnapshotEntry::File {
                path: relative.clone(),
                mode,
                contents,
            }
        } else {
            return Err(SandboxError::PathDenied(format!(
                "archive rejected non-file entry: {}",
                relative.display()
            )));
        };
        if parsed.insert(relative.clone(), value).is_some() {
            return Err(SandboxError::PathDenied(format!(
                "archive contains duplicate path: {}",
                relative.display()
            )));
        }
    }
    if !root_seen {
        return Err(SandboxError::PathDenied(
            "archive is missing its workspace root".to_owned(),
        ));
    }
    for entry in parsed.values() {
        let mut ancestor = entry.path().parent();
        while let Some(path) = ancestor.filter(|path| !path.as_os_str().is_empty()) {
            if !matches!(parsed.get(path), Some(SnapshotEntry::Directory { .. })) {
                return Err(SandboxError::PathDenied(format!(
                    "archive entry has a missing or non-directory parent: {}",
                    entry.path().display()
                )));
            }
            ancestor = path.parent();
        }
    }
    validate_casefold_uniqueness(parsed.keys().map(PathBuf::as_path))?;
    Ok(parsed.into_values().collect())
}

fn validate_casefold_uniqueness<'a>(paths: impl Iterator<Item = &'a Path>) -> Result<()> {
    let mut folded = BTreeMap::<String, PathBuf>::new();
    for path in paths {
        let key = path.to_string_lossy().to_lowercase();
        if let Some(existing) = folded.insert(key, path.to_path_buf())
            && existing != path
        {
            return Err(SandboxError::PathDenied(format!(
                "archive contains case-fold collision: {} and {}",
                existing.display(),
                path.display()
            )));
        }
    }
    Ok(())
}

fn normalized_archive_path(path: &Path) -> Result<PathBuf> {
    let mut normalized = PathBuf::new();
    for component in path.components() {
        match component {
            std::path::Component::Normal(name) => normalized.push(name),
            _ => {
                return Err(SandboxError::PathDenied(format!(
                    "archive path is not canonical and relative: {}",
                    path.display()
                )));
            }
        }
    }
    if normalized.as_os_str().is_empty() {
        return Err(SandboxError::PathDenied("archive path is empty".to_owned()));
    }
    Ok(normalized)
}

fn replace_tree(
    root: &DirectoryAuthority,
    entries: &[SnapshotEntry],
    mut hook: impl FnMut(ImportStage),
) -> Result<()> {
    root.remove_descendants()?;
    hook(ImportStage::DescendantsRemoved);
    let mut ordered = entries.iter().collect::<Vec<_>>();
    ordered.sort_by_key(|entry| {
        (
            entry.path().components().count(),
            matches!(entry, SnapshotEntry::File { .. }),
            entry.path().to_path_buf(),
        )
    });
    for entry in ordered {
        match entry {
            SnapshotEntry::Directory { path, .. } => {
                create_directory_path(root, path)?;
                hook(ImportStage::DirectoryCreated);
            }
            SnapshotEntry::File {
                path,
                mode,
                contents,
            } => {
                let parent = open_directory_path(root, path.parent().unwrap_or(Path::new("")))?;
                let name = path
                    .file_name()
                    .and_then(std::ffi::OsStr::to_str)
                    .ok_or_else(|| {
                        SandboxError::PathDenied("archive filename is not valid Unicode".to_owned())
                    })?;
                parent.atomic_write_child(name, contents)?;
                set_child_mode(&parent, name, *mode)?;
                hook(ImportStage::FileWritten);
            }
        }
    }
    root.sync()
}

fn create_directory_path(root: &DirectoryAuthority, path: &Path) -> Result<DirectoryAuthority> {
    let mut current = root.clone();
    for component in path.components() {
        let std::path::Component::Normal(name) = component else {
            return Err(SandboxError::PathDenied(
                "invalid directory path".to_owned(),
            ));
        };
        let name = name.to_str().ok_or_else(|| {
            SandboxError::PathDenied("archive directory is not valid Unicode".to_owned())
        })?;
        current = current.open_or_create_child_directory(name)?;
    }
    Ok(current)
}

fn open_directory_path(root: &DirectoryAuthority, path: &Path) -> Result<DirectoryAuthority> {
    let mut current = root.clone();
    for component in path.components() {
        let std::path::Component::Normal(name) = component else {
            return Err(SandboxError::PathDenied(
                "invalid directory path".to_owned(),
            ));
        };
        let name = name.to_str().ok_or_else(|| {
            SandboxError::PathDenied("archive directory is not valid Unicode".to_owned())
        })?;
        current = current.open_child_directory(name)?;
    }
    Ok(current)
}

fn append_directory(builder: &mut tar::Builder<Vec<u8>>, path: &Path, mode: u32) -> Result<()> {
    let mut header = tar::Header::new_gnu();
    header.set_entry_type(tar::EntryType::Directory);
    header.set_mode(mode);
    header.set_size(0);
    header.set_mtime(0);
    header.set_cksum();
    builder.append_data(&mut header, path, std::io::empty())?;
    Ok(())
}

fn file_mode(file: &RegularFileAuthority) -> Result<u32> {
    let metadata = file.handle.metadata()?;
    validate_real_file(file.display_path(), &metadata)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        Ok(metadata.mode() & 0o777)
    }
    #[cfg(not(unix))]
    {
        Ok(0o600)
    }
}

fn set_child_mode(parent: &DirectoryAuthority, name: &str, mode: u32) -> Result<()> {
    let file = parent.open_child_file(name)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        file.handle
            .set_permissions(std::fs::Permissions::from_mode(mode & 0o777))?;
    }
    #[cfg(not(unix))]
    let _ = mode;
    file.sync()
}
