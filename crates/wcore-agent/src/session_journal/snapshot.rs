use std::fs::File;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use super::{JournalEnvelope, JournalError, ReducedSessionState, reduce};

pub const SESSION_SNAPSHOT_SCHEMA_VERSION: u32 = 5;
pub const LEGACY_SESSION_SNAPSHOT_SCHEMA_VERSION: u32 = 4;
pub(super) const SNAPSHOT_AUTHORITY_BINDING_SCHEMA_VERSION: u32 = 1;
const SNAPSHOT_AUTHORITY_HEAD_SCHEMA_VERSION: u32 = 1;
const MAX_SESSION_SNAPSHOT_BYTES: u64 = 64 * 1024 * 1024;

#[cfg(test)]
thread_local! {
    static FAIL_REPLACE_AFTER_PERSIST: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
    static AFTER_AUTHORITY_READ_HOOK: std::cell::RefCell<Option<Box<dyn FnOnce(&Path)>>> =
        std::cell::RefCell::new(None);
}

#[cfg(test)]
pub(super) fn fail_next_replace_after_persist() {
    FAIL_REPLACE_AFTER_PERSIST.with(|fail| fail.set(true));
}

#[cfg(test)]
fn set_after_authority_read_hook(hook: impl FnOnce(&Path) + 'static) {
    AFTER_AUTHORITY_READ_HOOK.with(|slot| *slot.borrow_mut() = Some(Box::new(hook)));
}

#[cfg(test)]
fn run_after_authority_read_hook(path: &Path) {
    AFTER_AUTHORITY_READ_HOOK.with(|slot| {
        if let Some(hook) = slot.borrow_mut().take() {
            hook(path);
        }
    });
}

#[cfg(not(test))]
fn run_after_authority_read_hook(_path: &Path) {}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[rustfmt::skip]
/// Self-consistent snapshot data, not durable journal authority.
///
/// Callers may construct or deserialize this type for offline inspection and
/// fixture replay. Only [`super::SessionJournal`] can establish durable
/// authority by proving it against retained journal evidence.
pub struct SessionSnapshot {
    pub schema_version: u32, pub session_id: String, pub cursor: Option<u64>,
    pub cursor_checksum: String, pub state_digest: String, pub state: ReducedSessionState,
}

impl SessionSnapshot {
    /// Construct self-consistent snapshot data for offline use.
    ///
    /// This does not bind the snapshot to a journal and therefore cannot mint
    /// durable recovery authority.
    pub fn new(
        session_id: impl Into<String>,
        mut state: ReducedSessionState,
    ) -> Result<Self, JournalError> {
        let session_id = session_id.into();
        if let Some(found) = state.session_id.as_deref()
            && found != session_id
        {
            return Err(JournalError::SessionMismatch {
                expected: session_id,
                found: found.to_owned(),
            });
        }
        state.session_id = Some(session_id.clone());
        let state_digest = state.digest()?;
        Ok(Self {
            schema_version: SESSION_SNAPSHOT_SCHEMA_VERSION,
            session_id,
            cursor: state.last_seq,
            cursor_checksum: state.last_checksum.clone(),
            state_digest,
            state,
        })
    }

    pub fn validate(&self) -> Result<(), JournalError> {
        validate_snapshot_schema_for_reader(self.schema_version, SESSION_SNAPSHOT_SCHEMA_VERSION)?;
        if self.state.session_id.as_deref() != Some(self.session_id.as_str()) {
            return Err(JournalError::SessionMismatch {
                expected: self.session_id.clone(),
                found: self.state.session_id.clone().unwrap_or_default(),
            });
        }
        if self.cursor != self.state.last_seq || self.cursor_checksum != self.state.last_checksum {
            return Err(JournalError::SnapshotCursorMismatch);
        }
        if self.state.digest()? != self.state_digest {
            return Err(JournalError::SnapshotDigestMismatch);
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct SnapshotAuthorityBinding {
    pub schema_version: u32,
    pub snapshot_schema_version: u32,
    pub session_id: String,
    pub cursor: Option<u64>,
    pub cursor_checksum: String,
    pub state_digest: String,
}

impl SnapshotAuthorityBinding {
    pub(super) fn new(snapshot: &SessionSnapshot) -> Self {
        Self {
            schema_version: SNAPSHOT_AUTHORITY_BINDING_SCHEMA_VERSION,
            snapshot_schema_version: snapshot.schema_version,
            session_id: snapshot.session_id.clone(),
            cursor: snapshot.cursor,
            cursor_checksum: snapshot.cursor_checksum.clone(),
            state_digest: snapshot.state_digest.clone(),
        }
    }

    pub(super) fn validate(&self) -> Result<(), JournalError> {
        if self.schema_version != SNAPSHOT_AUTHORITY_BINDING_SCHEMA_VERSION {
            return Err(JournalError::UnsupportedSnapshotBindingSchema {
                found: self.schema_version,
                supported: SNAPSHOT_AUTHORITY_BINDING_SCHEMA_VERSION,
            });
        }
        if self.snapshot_schema_version != SESSION_SNAPSHOT_SCHEMA_VERSION {
            return Err(JournalError::UnsupportedSnapshotSchema {
                found: self.snapshot_schema_version,
                supported: SESSION_SNAPSHOT_SCHEMA_VERSION,
            });
        }
        Ok(())
    }

    pub(super) fn matches(&self, snapshot: &SessionSnapshot) -> bool {
        self.snapshot_schema_version == snapshot.schema_version
            && self.session_id == snapshot.session_id
            && self.cursor == snapshot.cursor
            && self.cursor_checksum == snapshot.cursor_checksum
            && self.state_digest == snapshot.state_digest
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
/// Bounded rollback floor for snapshot publications.
///
/// This sidecar does not claim to detect rollback of the entire journal
/// directory (including itself), nor rollback of journal-only events after the
/// last accepted snapshot. Those require an authority outside this directory.
pub(super) struct SnapshotAuthorityHead {
    pub schema_version: u32,
    pub accepted: Option<SnapshotAuthorityBinding>,
    pub pending: Option<SnapshotAuthorityBinding>,
}

impl Default for SnapshotAuthorityHead {
    fn default() -> Self {
        Self {
            schema_version: SNAPSHOT_AUTHORITY_HEAD_SCHEMA_VERSION,
            accepted: None,
            pending: None,
        }
    }
}

impl SnapshotAuthorityHead {
    pub(super) fn validate(&self) -> Result<(), JournalError> {
        if self.schema_version != SNAPSHOT_AUTHORITY_HEAD_SCHEMA_VERSION {
            return Err(JournalError::InvalidTransition(format!(
                "unsupported snapshot authority head schema {}",
                self.schema_version
            )));
        }
        if let Some(binding) = self.accepted.as_ref() {
            binding.validate()?;
        }
        if let Some(binding) = self.pending.as_ref() {
            binding.validate()?;
        }
        Ok(())
    }
}

pub(super) fn snapshot_authority_head_path(journal_path: &Path) -> PathBuf {
    let mut name = journal_path.file_name().map_or_else(
        || std::ffi::OsString::from("session"),
        std::ffi::OsString::from,
    );
    name.push(".authority");
    journal_path.with_file_name(name)
}

pub(super) fn load_snapshot_authority_head(
    journal_path: &Path,
) -> Result<Option<SnapshotAuthorityHead>, JournalError> {
    let path = snapshot_authority_head_path(journal_path);
    let mut file = match super::lease::open_existing_nofollow(&path) {
        Ok(file) => file,
        Err(JournalError::Io { source, .. }) if source.kind() == std::io::ErrorKind::NotFound => {
            return Ok(None);
        }
        Err(error) => return Err(error),
    };
    validate_private_snapshot_file(&file, &path)?;
    let mut bytes = Vec::new();
    file.read_to_end(&mut bytes)
        .map_err(|source| JournalError::Io {
            path: path.clone(),
            source,
        })?;
    run_after_authority_read_hook(&path);
    super::lease::ensure_path_identity(&file, &path)?;
    validate_private_snapshot_file(&file, &path)?;
    let head = serde_json::from_slice::<SnapshotAuthorityHead>(&bytes).map_err(|source| {
        JournalError::Json {
            context: "decoding snapshot authority head",
            source,
        }
    })?;
    head.validate()?;
    Ok(Some(head))
}

pub(super) fn write_snapshot_authority_head(
    journal_path: &Path,
    head: &SnapshotAuthorityHead,
) -> Result<(), JournalError> {
    head.validate()?;
    let path = snapshot_authority_head_path(journal_path);
    let bytes = serde_json::to_vec(head).map_err(|source| JournalError::Json {
        context: "encoding snapshot authority head",
        source,
    })?;
    replace_private_file_atomically(&path, &bytes)?;
    sync_parent_directory(&path)
}

pub(super) fn write_snapshot(
    path: impl AsRef<Path>,
    snapshot: &SessionSnapshot,
) -> Result<File, JournalError> {
    snapshot.validate()?;
    let path = path.as_ref();
    let mut bytes = serde_json::to_vec(snapshot).map_err(|source| JournalError::Json {
        context: "encoding session snapshot",
        source,
    })?;
    bytes.push(b'\n');
    let persisted = replace_snapshot_file_atomically(path, &bytes)?;
    sync_parent_directory(path)?;
    Ok(persisted)
}

/// Companion snapshot path used by [`super::SessionJournal`].
#[must_use]
pub fn snapshot_path_for(journal_path: impl AsRef<Path>) -> PathBuf {
    let journal_path = journal_path.as_ref();
    let mut name = journal_path
        .file_name()
        .map_or_else(|| "session.journal".into(), std::ffi::OsString::from);
    name.push(".snapshot");
    journal_path.with_file_name(name)
}

/// Load and validate untrusted snapshot data without granting journal authority.
pub fn load_snapshot(path: impl AsRef<Path>) -> Result<SessionSnapshot, JournalError> {
    let path = path.as_ref();
    let mut file = super::lease::open_existing_nofollow(path)?;
    validate_private_snapshot_file(&file, path)?;
    let length = file
        .metadata()
        .map_err(|source| JournalError::Io {
            path: path.to_path_buf(),
            source,
        })?
        .len();
    if length > MAX_SESSION_SNAPSHOT_BYTES {
        return Err(snapshot_too_large(path, length));
    }
    let mut bytes = Vec::with_capacity(length as usize);
    (&mut file)
        .take(MAX_SESSION_SNAPSHOT_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(|source| JournalError::Io {
            path: path.to_path_buf(),
            source,
        })?;
    if bytes.len() as u64 > MAX_SESSION_SNAPSHOT_BYTES {
        return Err(snapshot_too_large(path, bytes.len() as u64));
    }
    super::lease::ensure_path_identity(&file, path)?;
    validate_private_snapshot_file(&file, path)?;
    let value = serde_json::from_slice::<serde_json::Value>(&bytes).map_err(|source| {
        JournalError::Json {
            context: "decoding session snapshot",
            source,
        }
    })?;
    let found_schema = value
        .get("schema_version")
        .and_then(serde_json::Value::as_u64)
        .and_then(|version| u32::try_from(version).ok())
        .ok_or_else(|| {
            JournalError::InvalidTransition(
                "session snapshot is missing a valid schema_version".to_owned(),
            )
        })?;
    validate_snapshot_schema_for_reader(found_schema, SESSION_SNAPSHOT_SCHEMA_VERSION)?;
    if found_schema == LEGACY_SESSION_SNAPSHOT_SCHEMA_VERSION {
        reject_unknown_legacy_fields(&value)?;
    }
    let snapshot = serde_json::from_value::<SessionSnapshot>(value.clone()).map_err(|source| {
        JournalError::Json {
            context: "decoding session snapshot",
            source,
        }
    })?;
    let canonical = serde_json::to_value(&snapshot).map_err(|source| JournalError::Json {
        context: "encoding canonical session snapshot",
        source,
    })?;
    super::reject_dropped_typed_fields(&value, &canonical, "session snapshot")?;
    snapshot.validate()?;
    Ok(snapshot)
}

fn snapshot_too_large(path: &Path, size: u64) -> JournalError {
    JournalError::SnapshotTooLarge {
        path: path.to_path_buf(),
        size,
        max: MAX_SESSION_SNAPSHOT_BYTES,
    }
}

#[cfg(unix)]
fn secure_private_snapshot_file(file: &File, path: &Path) -> Result<(), JournalError> {
    use std::os::unix::fs::PermissionsExt as _;

    file.set_permissions(std::fs::Permissions::from_mode(0o600))
        .map_err(|source| JournalError::Io {
            path: path.to_path_buf(),
            source,
        })?;
    validate_private_snapshot_file(file, path)
}

#[cfg(windows)]
fn secure_private_snapshot_file(file: &File, path: &Path) -> Result<(), JournalError> {
    windows_snapshot_security::secure_private_file(file, path)
}

#[cfg(not(any(unix, windows)))]
fn secure_private_snapshot_file(_file: &File, path: &Path) -> Result<(), JournalError> {
    Err(JournalError::Io {
        path: path.to_path_buf(),
        source: std::io::Error::new(
            std::io::ErrorKind::Unsupported,
            "private snapshot files are unsupported on this platform",
        ),
    })
}

#[cfg(test)]
pub(super) fn write_private_snapshot_fixture(
    path: &Path,
    bytes: &[u8],
) -> Result<(), JournalError> {
    let mut file = File::create(path).map_err(|source| JournalError::Io {
        path: path.to_path_buf(),
        source,
    })?;
    secure_private_snapshot_file(&file, path)?;
    file.write_all(bytes).map_err(|source| JournalError::Io {
        path: path.to_path_buf(),
        source,
    })
}

#[cfg(unix)]
fn validate_private_snapshot_file(file: &File, path: &Path) -> Result<(), JournalError> {
    use std::os::unix::fs::MetadataExt as _;

    let metadata = file.metadata().map_err(|source| JournalError::Io {
        path: path.to_path_buf(),
        source,
    })?;
    if metadata.mode() & 0o077 != 0 {
        return Err(JournalError::SnapshotUnsafePermissions {
            path: path.to_path_buf(),
        });
    }
    // SAFETY: `geteuid` has no preconditions and only reads process metadata.
    let effective_uid = unsafe { session_snapshot_geteuid() };
    if metadata.uid() != effective_uid {
        return Err(JournalError::SnapshotOwnerMismatch {
            path: path.to_path_buf(),
        });
    }
    Ok(())
}

#[cfg(windows)]
fn validate_private_snapshot_file(file: &File, path: &Path) -> Result<(), JournalError> {
    windows_snapshot_security::validate_private_file(file, path)
}

#[cfg(not(any(unix, windows)))]
fn validate_private_snapshot_file(_file: &File, path: &Path) -> Result<(), JournalError> {
    Err(JournalError::Io {
        path: path.to_path_buf(),
        source: std::io::Error::new(
            std::io::ErrorKind::Unsupported,
            "private snapshot validation is unsupported on this platform",
        ),
    })
}

pub(super) fn validate_snapshot_authority_file(
    file: &File,
    path: &Path,
) -> Result<(), JournalError> {
    super::lease::ensure_path_identity(file, path)?;
    validate_private_snapshot_file(file, path)?;
    super::lease::ensure_path_identity(file, path)
}

#[cfg(unix)]
unsafe extern "C" {
    #[link_name = "geteuid"]
    fn session_snapshot_geteuid() -> u32;
}

#[cfg(windows)]
mod windows_snapshot_security {
    use std::fs::File;
    use std::path::Path;

    use super::JournalError;

    struct TokenUser {
        buffer: Vec<usize>,
    }

    impl TokenUser {
        fn sid(&self) -> windows_sys::Win32::Security::PSID {
            use windows_sys::Win32::Security::TOKEN_USER;

            // SAFETY: token_user sizes and fills this aligned allocation with
            // a TOKEN_USER record whose SID remains live with the buffer.
            unsafe { (*(self.buffer.as_ptr().cast::<TOKEN_USER>())).User.Sid }
        }
    }

    struct LocalAllocation(*mut core::ffi::c_void);

    impl Drop for LocalAllocation {
        fn drop(&mut self) {
            if !self.0.is_null() {
                // SAFETY: the wrapped Windows security APIs allocate these
                // buffers with LocalAlloc and transfer them to the caller.
                unsafe { windows_sys::Win32::Foundation::LocalFree(self.0) };
            }
        }
    }

    fn token_user() -> std::io::Result<TokenUser> {
        use std::os::windows::io::{AsRawHandle as _, FromRawHandle as _, OwnedHandle};
        use windows_sys::Win32::Foundation::{ERROR_INSUFFICIENT_BUFFER, HANDLE};
        use windows_sys::Win32::Security::{GetTokenInformation, TOKEN_QUERY, TokenUser};
        use windows_sys::Win32::System::Threading::{GetCurrentProcess, OpenProcessToken};

        let mut token: HANDLE = std::ptr::null_mut();
        // SAFETY: token is a live out-pointer and successful ownership is
        // transferred immediately into OwnedHandle.
        if unsafe { OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token) } == 0 {
            return Err(std::io::Error::last_os_error());
        }
        // SAFETY: OpenProcessToken returned this owned handle.
        let token = unsafe { OwnedHandle::from_raw_handle(token) };
        let mut needed = 0;
        // SAFETY: null/zero is the documented sizing probe.
        let sized = unsafe {
            GetTokenInformation(
                token.as_raw_handle(),
                TokenUser,
                std::ptr::null_mut(),
                0,
                &mut needed,
            )
        };
        if sized != 0 {
            return Err(std::io::Error::other(
                "GetTokenInformation(TokenUser) sizing unexpectedly succeeded",
            ));
        }
        let error = std::io::Error::last_os_error();
        if error.raw_os_error() != Some(ERROR_INSUFFICIENT_BUFFER as i32) || needed == 0 {
            return Err(error);
        }
        let words = (needed as usize).div_ceil(std::mem::size_of::<usize>());
        let mut buffer = vec![0usize; words];
        // SAFETY: the aligned buffer is writable for `needed` bytes and the
        // token handle remains live for this call.
        if unsafe {
            GetTokenInformation(
                token.as_raw_handle(),
                TokenUser,
                buffer.as_mut_ptr().cast(),
                needed,
                &mut needed,
            )
        } == 0
        {
            return Err(std::io::Error::last_os_error());
        }
        Ok(TokenUser { buffer })
    }

    #[derive(Clone, Copy)]
    enum AceKind {
        Allow,
        #[cfg(test)]
        Deny,
    }

    struct PrivateAcl {
        buffer: Vec<usize>,
    }

    impl PrivateAcl {
        fn as_ptr(&self) -> *const windows_sys::Win32::Security::ACL {
            self.buffer.as_ptr().cast()
        }

        fn as_mut_ptr(&mut self) -> *mut windows_sys::Win32::Security::ACL {
            self.buffer.as_mut_ptr().cast()
        }
    }

    fn build_acl(
        aces: &[(AceKind, windows_sys::Win32::Security::PSID)],
    ) -> std::io::Result<PrivateAcl> {
        use windows_sys::Win32::Security::{
            ACCESS_ALLOWED_ACE, ACL, ACL_REVISION, AddAccessAllowedAceEx, AddAccessDeniedAceEx,
            GetLengthSid, InitializeAcl,
        };
        use windows_sys::Win32::Storage::FileSystem::FILE_ALL_ACCESS;

        let mut bytes = std::mem::size_of::<ACL>();
        for (_, sid) in aces {
            // SAFETY: every SID is backed by a live TokenUser or OwnedSid
            // buffer for the duration of ACL construction.
            let sid_bytes = unsafe { GetLengthSid(*sid) } as usize;
            if sid_bytes == 0 {
                return Err(std::io::Error::last_os_error());
            }
            bytes = bytes
                .checked_add(
                    std::mem::size_of::<ACCESS_ALLOWED_ACE>() - std::mem::size_of::<u32>()
                        + sid_bytes,
                )
                .ok_or_else(|| std::io::Error::other("private DACL size overflow"))?;
        }
        let acl_bytes =
            u32::try_from(bytes).map_err(|_| std::io::Error::other("private DACL is too large"))?;
        let words = bytes.div_ceil(std::mem::size_of::<usize>());
        let mut acl = PrivateAcl {
            buffer: vec![0usize; words],
        };
        // SAFETY: the aligned allocation is writable for acl_bytes bytes.
        if unsafe { InitializeAcl(acl.as_mut_ptr(), acl_bytes, ACL_REVISION) } == 0 {
            return Err(std::io::Error::last_os_error());
        }
        for (kind, sid) in aces {
            // SAFETY: the ACL has space for every pre-counted ACE and each SID
            // remains live until the API has copied it into the ACL.
            let added = unsafe {
                match kind {
                    AceKind::Allow => AddAccessAllowedAceEx(
                        acl.as_mut_ptr(),
                        ACL_REVISION,
                        0,
                        FILE_ALL_ACCESS,
                        *sid,
                    ),
                    #[cfg(test)]
                    AceKind::Deny => AddAccessDeniedAceEx(
                        acl.as_mut_ptr(),
                        ACL_REVISION,
                        0,
                        FILE_ALL_ACCESS,
                        *sid,
                    ),
                }
            };
            if added == 0 {
                return Err(std::io::Error::last_os_error());
            }
        }
        Ok(acl)
    }

    fn set_file_dacl(
        file: &File,
        acl: Option<&PrivateAcl>,
        protected: bool,
    ) -> std::io::Result<()> {
        use std::os::windows::io::AsRawHandle as _;
        use windows_sys::Win32::Security::Authorization::{SE_FILE_OBJECT, SetSecurityInfo};
        use windows_sys::Win32::Security::{
            DACL_SECURITY_INFORMATION, PROTECTED_DACL_SECURITY_INFORMATION,
            UNPROTECTED_DACL_SECURITY_INFORMATION,
        };

        let protection = if protected {
            PROTECTED_DACL_SECURITY_INFORMATION
        } else {
            UNPROTECTED_DACL_SECURITY_INFORMATION
        };
        // SAFETY: file is a live handle; SetSecurityInfo copies the supplied
        // ACL before returning and all omitted owner/group/SACL pointers are
        // intentionally null.
        let result = unsafe {
            SetSecurityInfo(
                file.as_raw_handle(),
                SE_FILE_OBJECT,
                DACL_SECURITY_INFORMATION | protection,
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                acl.map_or(std::ptr::null(), PrivateAcl::as_ptr),
                std::ptr::null(),
            )
        };
        if result == 0 {
            Ok(())
        } else {
            Err(std::io::Error::from_raw_os_error(result as i32))
        }
    }

    fn open_security_handle(file: &File, path: &Path) -> Result<File, JournalError> {
        use std::fs::OpenOptions;
        use std::os::windows::fs::OpenOptionsExt as _;
        use windows_sys::Win32::Security::{READ_CONTROL, WRITE_DAC};
        use windows_sys::Win32::Storage::FileSystem::{
            FILE_FLAG_OPEN_REPARSE_POINT, FILE_SHARE_DELETE, FILE_SHARE_READ, FILE_SHARE_WRITE,
        };

        super::super::lease::ensure_path_identity(file, path)?;
        let security_file = OpenOptions::new()
            .access_mode(READ_CONTROL | WRITE_DAC)
            .share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE)
            .custom_flags(FILE_FLAG_OPEN_REPARSE_POINT)
            .open(path)
            .map_err(|source| JournalError::Io {
                path: path.to_path_buf(),
                source,
            })?;
        super::super::lease::ensure_same_identity(file, &security_file, path)?;
        Ok(security_file)
    }

    fn set_identity_bound_file_dacl(
        file: &File,
        path: &Path,
        acl: Option<&PrivateAcl>,
        protected: bool,
    ) -> Result<(), JournalError> {
        let security_file = open_security_handle(file, path)?;
        set_file_dacl(&security_file, acl, protected).map_err(|source| JournalError::Io {
            path: path.to_path_buf(),
            source,
        })?;
        super::super::lease::ensure_path_identity(file, path)
    }

    pub(super) fn secure_private_file(file: &File, path: &Path) -> Result<(), JournalError> {
        let user = token_user().map_err(|source| JournalError::Io {
            path: path.to_path_buf(),
            source,
        })?;
        let acl =
            build_acl(&[(AceKind::Allow, user.sid())]).map_err(|source| JournalError::Io {
                path: path.to_path_buf(),
                source,
            })?;
        set_identity_bound_file_dacl(file, path, Some(&acl), true)?;
        validate_private_file(file, path)
    }

    fn owner_is_private(
        owner: windows_sys::Win32::Security::PSID,
        user: windows_sys::Win32::Security::PSID,
    ) -> bool {
        use windows_sys::Win32::Security::{
            EqualSid, IsWellKnownSid, WinBuiltinAdministratorsSid, WinLocalSystemSid,
        };

        !owner.is_null()
            && unsafe {
                EqualSid(owner, user) != 0
                    || IsWellKnownSid(owner, WinLocalSystemSid) != 0
                    || IsWellKnownSid(owner, WinBuiltinAdministratorsSid) != 0
            }
    }

    fn restrictive_deny_ace(ace_type: u32) -> bool {
        use windows_sys::Win32::System::SystemServices::{
            ACCESS_DENIED_ACE_TYPE, ACCESS_DENIED_CALLBACK_ACE_TYPE,
            ACCESS_DENIED_CALLBACK_OBJECT_ACE_TYPE, ACCESS_DENIED_OBJECT_ACE_TYPE,
        };

        matches!(
            ace_type,
            ACCESS_DENIED_ACE_TYPE
                | ACCESS_DENIED_OBJECT_ACE_TYPE
                | ACCESS_DENIED_CALLBACK_ACE_TYPE
                | ACCESS_DENIED_CALLBACK_OBJECT_ACE_TYPE
        )
    }

    #[cfg(test)]
    struct OwnedSid {
        buffer: Vec<usize>,
    }

    #[cfg(test)]
    impl OwnedSid {
        fn sid(&self) -> windows_sys::Win32::Security::PSID {
            self.buffer.as_ptr().cast_mut().cast()
        }
    }

    #[cfg(test)]
    fn well_known_sid(
        kind: windows_sys::Win32::Security::WELL_KNOWN_SID_TYPE,
    ) -> std::io::Result<OwnedSid> {
        use windows_sys::Win32::Foundation::ERROR_INSUFFICIENT_BUFFER;
        use windows_sys::Win32::Security::CreateWellKnownSid;

        let mut needed = 0;
        // SAFETY: null/zero is the documented sizing probe.
        let sized = unsafe {
            CreateWellKnownSid(
                kind,
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                &mut needed,
            )
        };
        if sized != 0 {
            return Err(std::io::Error::other(
                "CreateWellKnownSid sizing unexpectedly succeeded",
            ));
        }
        let error = std::io::Error::last_os_error();
        if error.raw_os_error() != Some(ERROR_INSUFFICIENT_BUFFER as i32) || needed == 0 {
            return Err(error);
        }
        let words = (needed as usize).div_ceil(std::mem::size_of::<usize>());
        let mut sid = OwnedSid {
            buffer: vec![0usize; words],
        };
        // SAFETY: the aligned buffer is writable for needed bytes.
        if unsafe { CreateWellKnownSid(kind, std::ptr::null_mut(), sid.sid(), &mut needed) } == 0 {
            return Err(std::io::Error::last_os_error());
        }
        Ok(sid)
    }

    #[cfg(test)]
    pub(super) fn install_unprotected_private_dacl(
        file: &File,
        path: &Path,
    ) -> std::io::Result<()> {
        let user = token_user()?;
        let acl = build_acl(&[(AceKind::Allow, user.sid())])?;
        set_identity_bound_file_dacl(file, path, Some(&acl), false).map_err(std::io::Error::other)
    }

    #[cfg(test)]
    pub(super) fn install_null_dacl(file: &File, path: &Path) -> std::io::Result<()> {
        set_identity_bound_file_dacl(file, path, None, true).map_err(std::io::Error::other)
    }

    #[cfg(test)]
    pub(super) fn install_empty_dacl(file: &File, path: &Path) -> std::io::Result<()> {
        let acl = build_acl(&[])?;
        set_identity_bound_file_dacl(file, path, Some(&acl), true).map_err(std::io::Error::other)
    }

    #[cfg(test)]
    pub(super) fn install_broad_allow_dacl(file: &File, path: &Path) -> std::io::Result<()> {
        use windows_sys::Win32::Security::WinWorldSid;

        let user = token_user()?;
        let world = well_known_sid(WinWorldSid)?;
        let acl = build_acl(&[(AceKind::Allow, user.sid()), (AceKind::Allow, world.sid())])?;
        set_identity_bound_file_dacl(file, path, Some(&acl), true).map_err(std::io::Error::other)
    }

    #[cfg(test)]
    pub(super) fn install_restrictive_deny_dacl(file: &File, path: &Path) -> std::io::Result<()> {
        use windows_sys::Win32::Security::WinWorldSid;

        let user = token_user()?;
        let world = well_known_sid(WinWorldSid)?;
        let acl = build_acl(&[(AceKind::Deny, world.sid()), (AceKind::Allow, user.sid())])?;
        set_identity_bound_file_dacl(file, path, Some(&acl), true).map_err(std::io::Error::other)
    }

    #[cfg(test)]
    pub(super) fn elevated_owner_policy_is_private() -> std::io::Result<bool> {
        use windows_sys::Win32::Security::{WinBuiltinAdministratorsSid, WinLocalSystemSid};

        let user = token_user()?;
        let administrators = well_known_sid(WinBuiltinAdministratorsSid)?;
        let system = well_known_sid(WinLocalSystemSid)?;
        Ok(owner_is_private(administrators.sid(), user.sid())
            && owner_is_private(system.sid(), user.sid()))
    }

    pub(super) fn validate_private_file(file: &File, path: &Path) -> Result<(), JournalError> {
        use std::os::windows::io::AsRawHandle as _;
        use windows_sys::Win32::Security::Authorization::{GetSecurityInfo, SE_FILE_OBJECT};
        use windows_sys::Win32::Security::{
            ACCESS_ALLOWED_ACE, ACE_HEADER, ACL, DACL_SECURITY_INFORMATION, GetAce,
            GetSecurityDescriptorControl, INHERITED_ACE, OWNER_SECURITY_INFORMATION, PSID,
            SE_DACL_PROTECTED,
        };
        use windows_sys::Win32::System::SystemServices::ACCESS_ALLOWED_ACE_TYPE;

        let mut owner: PSID = std::ptr::null_mut();
        let mut acl: *mut ACL = std::ptr::null_mut();
        let mut descriptor = std::ptr::null_mut();
        // SAFETY: file is a live handle and every output pointer is writable.
        let result = unsafe {
            GetSecurityInfo(
                file.as_raw_handle(),
                SE_FILE_OBJECT,
                DACL_SECURITY_INFORMATION | OWNER_SECURITY_INFORMATION,
                &mut owner,
                std::ptr::null_mut(),
                &mut acl,
                std::ptr::null_mut(),
                &mut descriptor,
            )
        };
        if result != 0 {
            return Err(JournalError::Io {
                path: path.to_path_buf(),
                source: std::io::Error::from_raw_os_error(result as i32),
            });
        }
        let _descriptor = LocalAllocation(descriptor);
        let user = token_user().map_err(|source| JournalError::Io {
            path: path.to_path_buf(),
            source,
        })?;
        if !owner_is_private(owner, user.sid()) {
            return Err(JournalError::SnapshotOwnerMismatch {
                path: path.to_path_buf(),
            });
        }
        let mut control = 0;
        let mut revision = 0;
        // SAFETY: descriptor is the live self-relative descriptor returned by
        // GetSecurityInfo and remains owned by _descriptor.
        if unsafe { GetSecurityDescriptorControl(descriptor, &mut control, &mut revision) } == 0 {
            return Err(JournalError::Io {
                path: path.to_path_buf(),
                source: std::io::Error::last_os_error(),
            });
        }
        if acl.is_null() {
            return Err(JournalError::SnapshotUnsafePermissions {
                path: path.to_path_buf(),
            });
        }
        if control & SE_DACL_PROTECTED == 0 {
            return Err(JournalError::SnapshotUnsafePermissions {
                path: path.to_path_buf(),
            });
        }
        // Only explicit allow ACEs for the current user, LocalSystem, or
        // built-in Administrators may grant access. Restrictive deny ACEs do
        // not grant authority and are safe; inherited or unknown ACEs fail
        // closed even when their current expansion happens to look private.
        let ace_count = unsafe { (*acl).AceCount };
        if ace_count == 0 {
            return Err(JournalError::SnapshotUnsafePermissions {
                path: path.to_path_buf(),
            });
        }
        for index in 0..u32::from(ace_count) {
            let mut raw_ace = std::ptr::null_mut();
            // SAFETY: index is within the DACL's reported ACE count.
            if unsafe { GetAce(acl, index, &mut raw_ace) } == 0 {
                return Err(JournalError::Io {
                    path: path.to_path_buf(),
                    source: std::io::Error::last_os_error(),
                });
            }
            let header = unsafe { &*raw_ace.cast::<ACE_HEADER>() };
            if u32::from(header.AceFlags) & INHERITED_ACE != 0 {
                return Err(JournalError::SnapshotUnsafePermissions {
                    path: path.to_path_buf(),
                });
            }
            let ace_type = u32::from(header.AceType);
            let trusted = unsafe {
                if ace_type == ACCESS_ALLOWED_ACE_TYPE {
                    let ace = raw_ace.cast::<ACCESS_ALLOWED_ACE>();
                    let sid = std::ptr::addr_of_mut!((*ace).SidStart).cast();
                    owner_is_private(sid, user.sid())
                } else {
                    restrictive_deny_ace(ace_type)
                }
            };
            if !trusted {
                return Err(JournalError::SnapshotUnsafePermissions {
                    path: path.to_path_buf(),
                });
            }
        }
        Ok(())
    }
}

fn validate_snapshot_schema_for_reader(found: u32, supported: u32) -> Result<(), JournalError> {
    if found < LEGACY_SESSION_SNAPSHOT_SCHEMA_VERSION || found > supported {
        return Err(JournalError::UnsupportedSnapshotSchema { found, supported });
    }
    Ok(())
}

fn reject_unknown_legacy_fields(value: &serde_json::Value) -> Result<(), JournalError> {
    const ROOT_FIELDS: &[&str] = &[
        "schema_version",
        "session_id",
        "cursor",
        "cursor_checksum",
        "state_digest",
        "state",
    ];
    const STATE_FIELDS: &[&str] = &[
        "session_id",
        "last_seq",
        "last_checksum",
        "imported_baseline",
        "conversation",
        "turns",
        "streams",
        "provider_attempts",
        "tools",
        "hook_phases",
        "approvals",
        "budgets",
        "budget_event_ids",
        "budget_authority",
        "checkpoints",
        "children",
        "deliveries",
    ];
    let root = value.as_object().ok_or_else(|| {
        JournalError::InvalidTransition("legacy session snapshot must be a JSON object".to_owned())
    })?;
    reject_unknown_fields(root, ROOT_FIELDS, "legacy snapshot envelope")?;
    let state = root
        .get("state")
        .and_then(serde_json::Value::as_object)
        .ok_or_else(|| {
            JournalError::InvalidTransition(
                "legacy session snapshot state must be a JSON object".to_owned(),
            )
        })?;
    reject_unknown_fields(state, STATE_FIELDS, "legacy reduced state")
}

fn reject_unknown_fields(
    object: &serde_json::Map<String, serde_json::Value>,
    allowed: &[&str],
    layer: &'static str,
) -> Result<(), JournalError> {
    if let Some(field) = object
        .keys()
        .find(|field| !allowed.contains(&field.as_str()))
    {
        return Err(JournalError::UnknownCriticalField {
            layer,
            field: field.clone(),
        });
    }
    Ok(())
}

/// Reduce a suffix from self-consistent snapshot data for offline use.
///
/// This function does not consult retained journal bindings or a full prefix;
/// its result is not durable recovery authority. Use
/// [`super::SessionJournal::recovered_state`] for authoritative recovery.
pub fn replay_from_snapshot(
    snapshot: &SessionSnapshot,
    suffix: &[JournalEnvelope],
) -> Result<ReducedSessionState, JournalError> {
    snapshot.validate()?;
    suffix.iter().try_fold(snapshot.state.clone(), reduce)
}

pub(super) fn load_snapshot_if_present(
    path: impl AsRef<Path>,
) -> Result<Option<SessionSnapshot>, JournalError> {
    let path = path.as_ref();
    match load_snapshot(path) {
        Ok(snapshot) => Ok(Some(snapshot)),
        Err(JournalError::Io { source, .. }) if source.kind() == std::io::ErrorKind::NotFound => {
            Ok(None)
        }
        Err(error) => Err(error),
    }
}

pub(super) fn replace_file_atomically(path: &Path, bytes: &[u8]) -> Result<File, JournalError> {
    replace_file_atomically_inner(path, bytes, true, false)
}

fn replace_private_file_atomically(path: &Path, bytes: &[u8]) -> Result<File, JournalError> {
    replace_file_atomically_inner(path, bytes, false, true)
}

fn replace_snapshot_file_atomically(path: &Path, bytes: &[u8]) -> Result<File, JournalError> {
    replace_file_atomically_inner(path, bytes, false, true)
}

fn replace_file_atomically_inner(
    path: &Path,
    bytes: &[u8],
    _inject_test_failure: bool,
    private: bool,
) -> Result<File, JournalError> {
    let parent = path.parent().unwrap_or_else(|| Path::new("."));
    std::fs::create_dir_all(parent).map_err(|source| JournalError::Io {
        path: parent.to_path_buf(),
        source,
    })?;
    let mut temp = tempfile::NamedTempFile::new_in(parent).map_err(|source| JournalError::Io {
        path: path.to_path_buf(),
        source,
    })?;
    if private {
        secure_private_snapshot_file(temp.as_file(), temp.path())?;
    }
    #[cfg(unix)]
    if !private {
        use std::os::unix::fs::PermissionsExt;
        temp.as_file()
            .set_permissions(std::fs::Permissions::from_mode(0o600))
            .map_err(|source| JournalError::Io {
                path: path.to_path_buf(),
                source,
            })?;
    }
    temp.write_all(bytes)
        .and_then(|()| temp.as_file().sync_all())
        .map_err(|source| JournalError::Io {
            path: path.to_path_buf(),
            source,
        })?;
    // Lock the replacement inode before publication. This preserves the same
    // writer authority across the atomic rename and makes hard-link aliases
    // contend on the journal data itself, not only on its pathname sentinel.
    super::lease::lock_data_file(temp.as_file(), path)?;
    let persisted = temp.persist(path).map_err(|error| JournalError::Io {
        path: path.to_path_buf(),
        source: error.error,
    })?;
    if private {
        super::lease::ensure_path_identity(&persisted, path)?;
        validate_private_snapshot_file(&persisted, path)?;
    }
    #[cfg(test)]
    if _inject_test_failure && FAIL_REPLACE_AFTER_PERSIST.with(|fail| fail.replace(false)) {
        // Publication is uncertain. Keep the new inode locked until process
        // exit rather than let an alias acquire a second writer authority.
        std::mem::forget(persisted);
        return Err(JournalError::Io {
            path: path.to_path_buf(),
            source: std::io::Error::other("injected replacement failure after persist"),
        });
    }
    if let Err(source) = persisted.sync_all() {
        // See the injected-failure branch above: leaking one descriptor on an
        // exceptional durability failure is the fail-closed choice.
        std::mem::forget(persisted);
        return Err(JournalError::Io {
            path: path.to_path_buf(),
            source,
        });
    }
    Ok(persisted)
}

pub(super) fn sync_parent_directory(path: &Path) -> Result<(), JournalError> {
    #[cfg(unix)]
    {
        let parent = path.parent().unwrap_or_else(|| Path::new("."));
        File::open(parent)
            .and_then(|directory| directory.sync_all())
            .map_err(|source| JournalError::Io {
                path: parent.to_path_buf(),
                source,
            })?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::super::TurnState;
    use super::*;

    fn write_value(path: &Path, value: &serde_json::Value) {
        std::fs::write(path, serde_json::to_vec(value).unwrap()).unwrap();
        make_private(path);
    }

    #[cfg(unix)]
    fn make_private(path: &Path) {
        use std::os::unix::fs::PermissionsExt as _;

        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600)).unwrap();
    }

    #[cfg(not(unix))]
    fn make_private(_path: &Path) {}

    fn authority_head() -> SnapshotAuthorityHead {
        SnapshotAuthorityHead::default()
    }

    #[test]
    fn authority_head_round_trip_uses_private_identity_bound_file() {
        let dir = tempfile::tempdir().unwrap();
        let journal_path = dir.path().join("session.journal");
        let head = authority_head();

        write_snapshot_authority_head(&journal_path, &head).unwrap();

        assert_eq!(
            load_snapshot_authority_head(&journal_path).unwrap(),
            Some(head)
        );
    }

    #[cfg(unix)]
    #[test]
    fn authority_head_publication_is_owner_only() {
        use std::os::unix::fs::PermissionsExt as _;

        let dir = tempfile::tempdir().unwrap();
        let journal_path = dir.path().join("session.journal");
        write_snapshot_authority_head(&journal_path, &authority_head()).unwrap();
        let path = snapshot_authority_head_path(&journal_path);

        assert_eq!(
            std::fs::metadata(path).unwrap().permissions().mode() & 0o777,
            0o600
        );
    }

    #[cfg(unix)]
    #[test]
    fn authority_loader_rejects_symlink() {
        use std::os::unix::fs::symlink;

        let dir = tempfile::tempdir().unwrap();
        let journal_path = dir.path().join("session.journal");
        let path = snapshot_authority_head_path(&journal_path);
        let target = dir.path().join("foreign.authority");
        std::fs::write(&target, serde_json::to_vec(&authority_head()).unwrap()).unwrap();
        make_private(&target);
        symlink(target, &path).unwrap();

        assert!(matches!(
            load_snapshot_authority_head(&journal_path),
            Err(JournalError::SymbolicLink { .. })
        ));
    }

    #[cfg(windows)]
    #[test]
    fn authority_loader_rejects_symlink() {
        use std::os::windows::fs::symlink_file;

        let dir = tempfile::tempdir().unwrap();
        let journal_path = dir.path().join("session.journal");
        let path = snapshot_authority_head_path(&journal_path);
        let target = dir.path().join("foreign.authority");
        std::fs::write(&target, serde_json::to_vec(&authority_head()).unwrap()).unwrap();
        symlink_file(target, &path)
            .unwrap_or_else(|error| panic!("Windows symlink fixture is required: {error}"));

        assert!(matches!(
            load_snapshot_authority_head(&journal_path),
            Err(JournalError::SymbolicLink { .. })
        ));
    }

    #[test]
    fn authority_loader_rejects_hard_link() {
        let dir = tempfile::tempdir().unwrap();
        let journal_path = dir.path().join("session.journal");
        write_snapshot_authority_head(&journal_path, &authority_head()).unwrap();
        let path = snapshot_authority_head_path(&journal_path);
        std::fs::hard_link(&path, dir.path().join("authority-alias")).unwrap();

        assert!(matches!(
            load_snapshot_authority_head(&journal_path),
            Err(JournalError::MultipleLinks { path: rejected_path }) if rejected_path == path
        ));
    }

    #[cfg(unix)]
    #[test]
    fn authority_loader_rejects_group_or_world_access() {
        use std::os::unix::fs::PermissionsExt as _;

        let dir = tempfile::tempdir().unwrap();
        let journal_path = dir.path().join("session.journal");
        write_snapshot_authority_head(&journal_path, &authority_head()).unwrap();
        let path = snapshot_authority_head_path(&journal_path);
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o640)).unwrap();

        assert!(matches!(
            load_snapshot_authority_head(&journal_path),
            Err(JournalError::SnapshotUnsafePermissions { path: rejected_path })
                if rejected_path == path
        ));
    }

    #[cfg(unix)]
    #[test]
    fn authority_loader_rejects_wrong_owner_when_running_as_root() {
        use std::os::unix::ffi::OsStrExt as _;

        if unsafe { session_snapshot_geteuid() } != 0 {
            return;
        }
        let dir = tempfile::tempdir().unwrap();
        let journal_path = dir.path().join("session.journal");
        write_snapshot_authority_head(&journal_path, &authority_head()).unwrap();
        let path = snapshot_authority_head_path(&journal_path);
        let c_path = std::ffi::CString::new(path.as_os_str().as_bytes()).unwrap();
        // SAFETY: c_path is NUL-terminated; UID 1 deliberately differs from
        // root and u32::MAX leaves the group unchanged.
        assert_eq!(unsafe { libc::chown(c_path.as_ptr(), 1, u32::MAX) }, 0);

        assert!(matches!(
            load_snapshot_authority_head(&journal_path),
            Err(JournalError::SnapshotOwnerMismatch { path: rejected_path })
                if rejected_path == path
        ));
    }

    #[test]
    fn authority_loader_rejects_replacement_during_validation() {
        let dir = tempfile::tempdir().unwrap();
        let journal_path = dir.path().join("session.journal");
        let head = authority_head();
        write_snapshot_authority_head(&journal_path, &head).unwrap();
        let path = snapshot_authority_head_path(&journal_path);
        let displaced = dir.path().join("displaced.authority");
        let replacement = serde_json::to_vec(&head).unwrap();
        set_after_authority_read_hook(move |canonical| {
            std::fs::rename(canonical, &displaced).unwrap();
            std::fs::write(canonical, replacement).unwrap();
            make_private(canonical);
        });

        assert!(matches!(
            load_snapshot_authority_head(&journal_path),
            Err(JournalError::PathIdentityMismatch { path: rejected_path })
                if rejected_path == path
        ));
    }

    #[cfg(windows)]
    #[test]
    fn windows_private_dacl_rejects_unprotected_inheritance() {
        let temp = tempfile::NamedTempFile::new().unwrap();
        windows_snapshot_security::install_unprotected_private_dacl(temp.as_file(), temp.path())
            .unwrap();

        assert!(matches!(
            validate_private_snapshot_file(temp.as_file(), temp.path()),
            Err(JournalError::SnapshotUnsafePermissions { .. })
        ));
    }

    #[cfg(windows)]
    #[test]
    fn windows_private_dacl_rejects_null_empty_and_broad_allow() {
        for install in [
            windows_snapshot_security::install_null_dacl as fn(&File, &Path) -> std::io::Result<()>,
            windows_snapshot_security::install_empty_dacl,
            windows_snapshot_security::install_broad_allow_dacl,
        ] {
            let temp = tempfile::NamedTempFile::new().unwrap();
            install(temp.as_file(), temp.path()).unwrap();
            assert!(matches!(
                validate_private_snapshot_file(temp.as_file(), temp.path()),
                Err(JournalError::SnapshotUnsafePermissions { .. })
            ));
        }
    }

    #[cfg(windows)]
    #[test]
    fn windows_private_dacl_accepts_restrictive_deny_ace() {
        let temp = tempfile::NamedTempFile::new().unwrap();
        windows_snapshot_security::install_restrictive_deny_dacl(temp.as_file(), temp.path())
            .unwrap();

        validate_private_snapshot_file(temp.as_file(), temp.path()).unwrap();
    }

    #[cfg(windows)]
    #[test]
    fn windows_private_owner_policy_accepts_elevated_owners() {
        assert!(windows_snapshot_security::elevated_owner_policy_is_private().unwrap());
    }

    #[test]
    fn snapshot_loader_rejects_oversize_before_reading() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("oversize.snapshot");
        let file = File::create(&path).unwrap();
        file.set_len(MAX_SESSION_SNAPSHOT_BYTES + 1).unwrap();
        make_private(&path);

        assert!(matches!(
            load_snapshot(&path),
            Err(JournalError::SnapshotTooLarge {
                path: rejected_path,
                size,
                max: MAX_SESSION_SNAPSHOT_BYTES,
            }) if rejected_path == path && size == MAX_SESSION_SNAPSHOT_BYTES + 1
        ));
    }

    #[cfg(unix)]
    #[test]
    fn snapshot_loader_rejects_symlink() {
        use std::os::unix::fs::symlink;

        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("target.snapshot");
        let link = dir.path().join("link.snapshot");
        write_value(&target, &serde_json::json!({}));
        symlink(&target, &link).unwrap();

        assert!(matches!(
            load_snapshot(&link),
            Err(JournalError::SymbolicLink { .. })
        ));
    }

    #[cfg(windows)]
    #[test]
    fn snapshot_loader_rejects_symlink() {
        use std::os::windows::fs::symlink_file;

        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("target.snapshot");
        let link = dir.path().join("link.snapshot");
        write_value(&target, &serde_json::json!({}));
        symlink_file(&target, &link)
            .unwrap_or_else(|error| panic!("Windows symlink fixture is required: {error}"));

        assert!(matches!(
            load_snapshot(&link),
            Err(JournalError::SymbolicLink { .. })
        ));
    }

    #[test]
    fn snapshot_loader_rejects_hard_link() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("session.snapshot");
        let alias = dir.path().join("alias.snapshot");
        write_value(&path, &serde_json::json!({}));
        std::fs::hard_link(&path, alias).unwrap();

        assert!(matches!(
            load_snapshot(&path),
            Err(JournalError::MultipleLinks { .. })
        ));
    }

    #[cfg(unix)]
    #[test]
    fn snapshot_loader_rejects_group_or_world_access() {
        use std::os::unix::fs::PermissionsExt as _;

        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("public.snapshot");
        write_value(&path, &serde_json::json!({}));
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o640)).unwrap();

        assert!(matches!(
            load_snapshot(&path),
            Err(JournalError::SnapshotUnsafePermissions { path: rejected_path })
                if rejected_path == path
        ));
    }

    #[cfg(unix)]
    #[test]
    fn snapshot_loader_rejects_wrong_owner_when_running_as_root() {
        use std::os::unix::ffi::OsStrExt as _;

        // Changing ownership is only safe and available in the privileged test
        // environment. Other Unix runs exercise the matching-owner path
        // without mutating process-wide credentials.
        if unsafe { session_snapshot_geteuid() } != 0 {
            return;
        }
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("foreign.snapshot");
        write_value(&path, &serde_json::json!({}));
        let path_bytes = path.as_os_str().as_bytes();
        let c_path = std::ffi::CString::new(path_bytes).unwrap();
        // SAFETY: `c_path` is a valid NUL-terminated path. UID 1 deliberately
        // differs from root; u32::MAX leaves the group unchanged.
        assert_eq!(unsafe { libc::chown(c_path.as_ptr(), 1, u32::MAX) }, 0);

        assert!(matches!(
            load_snapshot(&path),
            Err(JournalError::SnapshotOwnerMismatch { path: rejected_path })
                if rejected_path == path
        ));
    }

    #[test]
    fn legacy_reader_rejects_current_snapshot_with_typed_version_error() {
        assert!(matches!(
            validate_snapshot_schema_for_reader(
                SESSION_SNAPSHOT_SCHEMA_VERSION,
                LEGACY_SESSION_SNAPSHOT_SCHEMA_VERSION,
            ),
            Err(JournalError::UnsupportedSnapshotSchema {
                found: SESSION_SNAPSHOT_SCHEMA_VERSION,
                supported: LEGACY_SESSION_SNAPSHOT_SCHEMA_VERSION,
            })
        ));
    }

    #[test]
    fn snapshot_and_reduced_state_unknown_fields_fail_closed() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("session.snapshot");
        let snapshot = SessionSnapshot::new("s1", ReducedSessionState::default()).unwrap();

        let mut unknown_envelope = serde_json::to_value(&snapshot).unwrap();
        unknown_envelope["future_authority"] = serde_json::json!(true);
        write_value(&path, &unknown_envelope);
        assert!(matches!(
            load_snapshot(&path),
            Err(JournalError::Json { .. })
        ));

        let mut unknown_state = serde_json::to_value(&snapshot).unwrap();
        unknown_state["state"]["future_authority"] = serde_json::json!(true);
        write_value(&path, &unknown_state);
        assert!(matches!(
            load_snapshot(&path),
            Err(JournalError::Json { .. })
        ));

        unknown_state["schema_version"] = serde_json::json!(LEGACY_SESSION_SNAPSHOT_SCHEMA_VERSION);
        write_value(&path, &unknown_state);
        assert!(matches!(
            load_snapshot(&path),
            Err(JournalError::UnknownCriticalField {
                layer: "legacy reduced state",
                ..
            })
        ));
    }

    #[test]
    fn future_snapshot_version_is_rejected_before_state_decode() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("session.snapshot");
        write_value(
            &path,
            &serde_json::json!({
                "schema_version": SESSION_SNAPSHOT_SCHEMA_VERSION + 1,
                "state": "not a reduced state"
            }),
        );
        assert!(matches!(
            load_snapshot(&path),
            Err(JournalError::UnsupportedSnapshotSchema {
                found,
                supported: SESSION_SNAPSHOT_SCHEMA_VERSION,
            }) if found == SESSION_SNAPSHOT_SCHEMA_VERSION + 1
        ));
    }

    #[test]
    fn nested_snapshot_fields_fail_closed_but_opaque_payload_fields_survive() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("session.snapshot");
        let mut state = ReducedSessionState::default();
        state.session_id = Some("s1".to_owned());
        state.turns.insert(
            "t0".to_owned(),
            TurnState {
                user_message: "hello".to_owned(),
                completion: None,
            },
        );
        state.conversation.push(serde_json::json!({
            "role": "user",
            "future_payload_field": {"must": "remain verbatim"},
        }));
        let snapshot = SessionSnapshot::new("s1", state).unwrap();

        let mut current = serde_json::to_value(&snapshot).unwrap();
        current["state"]["turns"]["t0"]["future_authority"] = serde_json::json!(true);
        write_value(&path, &current);
        assert!(load_snapshot(&path).is_err());

        current["schema_version"] = serde_json::json!(LEGACY_SESSION_SNAPSHOT_SCHEMA_VERSION);
        write_value(&path, &current);
        assert!(load_snapshot(&path).is_err());

        write_value(&path, &serde_json::to_value(&snapshot).unwrap());
        let loaded = load_snapshot(&path).unwrap();
        assert_eq!(loaded, snapshot);
        assert_eq!(
            loaded.state.conversation[0]["future_payload_field"]["must"],
            "remain verbatim"
        );
    }
}
