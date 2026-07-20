//! Retained filesystem identity for authority-bearing directories.

use std::fs::{File, OpenOptions};
use std::io::{Read, Seek, Write};
use std::ops::Deref;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use crate::{Result, SandboxError};

/// A held handle plus the filesystem identity of an authority-bearing
/// directory.
///
/// The handle keeps the original directory object alive after a rename.
/// [`Self::validate_path`] rejects a different object installed at the same
/// pathname, including a replacement by another ordinary directory.
#[derive(Clone, Debug)]
pub struct DirectoryAuthority {
    handle: Arc<File>,
    identity: DirectoryIdentity,
    display_path: Arc<PathBuf>,
    handle_loans: Arc<AtomicUsize>,
}

/// One tracked duplicate of a retained directory handle.
///
/// Destructive authority operations can mechanically refuse to proceed while
/// a lock or child process still owns a duplicate handle.
#[derive(Debug)]
pub struct DirectoryHandleLoan {
    handle: File,
    loans: Arc<AtomicUsize>,
}

impl Deref for DirectoryHandleLoan {
    type Target = File;

    fn deref(&self) -> &Self::Target {
        &self.handle
    }
}

#[cfg(unix)]
impl std::os::fd::AsFd for DirectoryHandleLoan {
    fn as_fd(&self) -> std::os::fd::BorrowedFd<'_> {
        std::os::fd::AsFd::as_fd(&self.handle)
    }
}

#[cfg(windows)]
impl std::os::windows::io::AsHandle for DirectoryHandleLoan {
    fn as_handle(&self) -> std::os::windows::io::BorrowedHandle<'_> {
        std::os::windows::io::AsHandle::as_handle(&self.handle)
    }
}

impl Drop for DirectoryHandleLoan {
    fn drop(&mut self) {
        let previous = self.loans.fetch_sub(1, Ordering::AcqRel);
        debug_assert!(previous > 0, "directory handle loan counter underflow");
    }
}

/// Retained identity and bytes for an authority-bearing regular file.
#[derive(Debug)]
pub struct RegularFileAuthority {
    handle: File,
    identity: DirectoryIdentity,
    display_path: PathBuf,
}

/// Owner-bound authority for a disposable delegated-mutation workspace.
///
/// The owner directory is retained separately from the workspace child so
/// every execution, cleanup, and (Task 1D) import decision binds to the exact
/// retained OS objects rather than re-resolving a mutable pathname. A worker
/// receives only the `workspace` checkout authority; the parent keeps the
/// `owner` to prove the child is still the exact object beneath it.
#[derive(Clone, Debug)]
pub struct RetainedWorkspaceAuthority {
    owner: DirectoryAuthority,
    workspace: DirectoryAuthority,
    child_name: String,
    /// Owner-issued transaction label bound into the durable import journal
    /// (Task 1D Docker transport). Only retained where the archive module is
    /// compiled; the lean native path validates it in `new` and discards it.
    #[cfg(any(feature = "live-docker", test))]
    transaction_id: String,
}

/// Opaque, copyable identity for cross-binding a retained directory to
/// external accounting state without cloning its live authority handle.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct DirectoryAuthorityIdentity(DirectoryIdentity);

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct DirectoryIdentity {
    #[cfg(unix)]
    device: u64,
    #[cfg(unix)]
    inode: u64,
    #[cfg(windows)]
    volume: u64,
    #[cfg(windows)]
    file_id: [u8; 16],
}

#[path = "directory_authority_archive.rs"]
#[cfg(any(feature = "live-docker", test))]
pub(crate) mod archive;
#[path = "directory_authority_file.rs"]
mod file;
#[cfg(windows)]
#[path = "directory_authority_windows.rs"]
mod windows;

impl DirectoryAuthority {
    pub fn identity_token(&self) -> DirectoryAuthorityIdentity {
        DirectoryAuthorityIdentity(self.identity)
    }

    /// Acquire authority only when the pathname and opened handle identify the
    /// same real directory throughout acquisition.
    pub fn open(path: &Path) -> Result<Self> {
        Self::open_inner(path, || {})
    }

    fn open_inner(path: &Path, hook: impl FnOnce()) -> Result<Self> {
        let before_metadata = std::fs::symlink_metadata(path)?;
        validate_real_directory(path, &before_metadata)?;
        let before = path_directory_identity(path, &before_metadata)?;
        hook();
        let handle = open_directory(path)?;
        let handle_metadata = handle.metadata()?;
        validate_real_directory(path, &handle_metadata)?;
        let held = handle_directory_identity(&handle, &handle_metadata)?;
        let after_metadata = std::fs::symlink_metadata(path)?;
        validate_real_directory(path, &after_metadata)?;
        let after = path_directory_identity(path, &after_metadata)?;
        if before != held || held != after {
            return Err(identity_changed(path, "while authority was acquired"));
        }
        Ok(Self {
            handle: Arc::new(handle),
            identity: held,
            display_path: Arc::new(path.to_path_buf()),
            handle_loans: Arc::new(AtomicUsize::new(0)),
        })
    }

    /// Prove that the current pathname still names the exact retained
    /// directory object. Both pathname observations must agree with the held
    /// handle, so a replacement during validation also fails closed.
    pub fn validate_path(&self, path: &Path) -> Result<()> {
        self.validate_path_inner(path, || {})
    }

    /// Clone the retained directory handle for an inode-bound advisory lock.
    pub fn try_clone_handle(&self) -> std::io::Result<DirectoryHandleLoan> {
        let handle = self.handle.try_clone()?;
        self.handle_loans.fetch_add(1, Ordering::AcqRel);
        Ok(DirectoryHandleLoan {
            handle,
            loans: Arc::clone(&self.handle_loans),
        })
    }

    pub fn has_outstanding_handle_loans(&self) -> bool {
        self.handle_loans.load(Ordering::Acquire) != 0
    }

    /// Duplicate the retained directory descriptor for transfer across one
    /// process-spawn boundary. The caller must keep the returned file alive
    /// until the child has consumed the descriptor.
    ///
    /// Gated to Linux: its sole consumer is the Bubblewrap backend, which binds
    /// the inherited descriptor into the sandbox namespace as `/proc/self/fd/N`
    /// (see `backends::bwrap`). macOS delegates through Docker archive transport
    /// and Windows through handle-relative operations, neither of which inherits
    /// this descriptor, so the primitive would be dead code there.
    #[cfg(target_os = "linux")]
    pub(crate) fn try_clone_inheritable_handle(&self) -> Result<DirectoryHandleLoan> {
        use std::os::fd::AsRawFd;

        let handle = self.try_clone_handle()?;
        let descriptor = handle.as_raw_fd();
        // SAFETY: F_GETFD/F_SETFD operate on the live duplicate we own.
        let flags = unsafe { libc::fcntl(descriptor, libc::F_GETFD) };
        if flags == -1
            || unsafe { libc::fcntl(descriptor, libc::F_SETFD, flags & !libc::FD_CLOEXEC) } == -1
        {
            return Err(std::io::Error::last_os_error().into());
        }
        Ok(handle)
    }

    /// Human-readable path metadata. This is never filesystem authority.
    pub fn display_path(&self) -> &Path {
        self.display_path.as_path()
    }

    /// Open one direct child directory relative to this retained directory.
    pub fn open_child_directory(&self, name: &str) -> Result<Self> {
        validate_child_name(name)?;
        #[cfg(unix)]
        {
            use std::ffi::CString;
            use std::os::fd::{AsRawFd, FromRawFd};

            let name_c = CString::new(name).map_err(|_| {
                SandboxError::PathDenied("authority child name contains NUL".to_owned())
            })?;
            // SAFETY: the parent descriptor and NUL-terminated name remain
            // valid for the syscall. O_NOFOLLOW rejects a linked child.
            let fd = unsafe {
                libc::openat(
                    self.handle.as_raw_fd(),
                    name_c.as_ptr(),
                    libc::O_RDONLY | libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC,
                )
            };
            if fd < 0 {
                return Err(std::io::Error::last_os_error().into());
            }
            // SAFETY: successful openat returned a fresh owned descriptor.
            let handle = unsafe { File::from_raw_fd(fd) };
            let metadata = handle.metadata()?;
            validate_real_directory(self.display_path(), &metadata)?;
            let identity = handle_directory_identity(&handle, &metadata)?;
            Ok(Self {
                handle: Arc::new(handle),
                identity,
                display_path: Arc::new(self.display_path.join(name)),
                handle_loans: Arc::new(AtomicUsize::new(0)),
            })
        }
        #[cfg(windows)]
        return windows::open_child_directory(self, name);
        #[cfg(not(any(unix, windows)))]
        Err(SandboxError::PolicyNotSupported(
            "relative directory open is unsupported on this platform".to_owned(),
        ))
    }

    /// Enumerate direct child names beneath the retained directory. Names are
    /// observations only; callers must open each child through this authority
    /// before trusting its type, metadata, or contents.
    pub fn child_names(&self) -> Result<Vec<String>> {
        #[cfg(windows)]
        return windows::child_names(self);
        #[cfg(unix)]
        {
            let mut names = Vec::new();
            for entry in cap_std::fs::Dir::from_std_file(self.handle.try_clone()?).entries()? {
                let entry = entry?;
                let name = entry.file_name().into_string().map_err(|_| {
                    SandboxError::PathDenied("authority child name is not valid Unicode".to_owned())
                })?;
                validate_child_name(&name)?;
                names.push(name);
            }
            names.sort();
            Ok(names)
        }
        #[cfg(not(any(unix, windows)))]
        Err(SandboxError::PolicyNotSupported(
            "relative enumeration is unsupported on this platform".to_owned(),
        ))
    }

    /// Open one direct regular-file child relative to this retained directory.
    pub fn open_child_file(&self, name: &str) -> Result<RegularFileAuthority> {
        validate_child_name(name)?;
        #[cfg(unix)]
        {
            use std::ffi::CString;
            use std::os::fd::{AsRawFd, FromRawFd};

            let name_c = CString::new(name).map_err(|_| {
                SandboxError::PathDenied("authority child name contains NUL".to_owned())
            })?;
            // SAFETY: the retained parent descriptor scopes this no-follow
            // open to one validated direct child.
            let fd = unsafe {
                libc::openat(
                    self.handle.as_raw_fd(),
                    name_c.as_ptr(),
                    libc::O_RDONLY | libc::O_NOFOLLOW | libc::O_CLOEXEC | libc::O_NONBLOCK,
                )
            };
            if fd < 0 {
                return Err(std::io::Error::last_os_error().into());
            }
            // SAFETY: successful openat returned one fresh owned descriptor.
            let handle = unsafe { File::from_raw_fd(fd) };
            let metadata = handle.metadata()?;
            validate_real_file(Path::new("<retained child>"), &metadata)?;
            let identity = handle_directory_identity(&handle, &metadata)?;
            Ok(RegularFileAuthority {
                handle,
                identity,
                display_path: self.display_path.join(name),
            })
        }
        #[cfg(windows)]
        {
            windows::open_child_file(self, name)
        }
        #[cfg(not(any(unix, windows)))]
        Err(SandboxError::PolicyNotSupported(
            "relative file open is unsupported on this platform".to_owned(),
        ))
    }

    /// Read one optional direct child through retained parent authority.
    pub fn read_child_bounded(&self, name: &str, max_bytes: u64) -> Result<Option<Vec<u8>>> {
        let authority = match self.open_child_file(name) {
            Ok(authority) => authority,
            Err(SandboxError::Io(error)) if error.kind() == std::io::ErrorKind::NotFound => {
                return Ok(None);
            }
            Err(error) => return Err(error),
        };
        authority.read_bounded(max_bytes).map(Some)
    }

    /// Atomically publish one direct child through a private, unguessable
    /// sibling retained until the final parent-relative rename.
    pub fn atomic_write_child(&self, name: &str, contents: &[u8]) -> Result<()> {
        validate_child_name(name)?;
        let temporary = format!(".wayland-write-{}", uuid::Uuid::new_v4().simple());
        let temporary_authority = self.create_child_file(&temporary, contents)?;
        #[cfg(unix)]
        {
            use std::ffi::CString;
            use std::os::fd::AsRawFd;

            let source = CString::new(temporary.as_str()).expect("UUID name contains no NUL");
            let destination = CString::new(name).map_err(|_| {
                SandboxError::PathDenied("authority child name contains NUL".to_owned())
            })?;
            // SAFETY: both source and destination are one validated direct
            // child beneath the retained parent.
            if unsafe {
                libc::renameat(
                    self.handle.as_raw_fd(),
                    source.as_ptr(),
                    self.handle.as_raw_fd(),
                    destination.as_ptr(),
                )
            } != 0
            {
                let publish = SandboxError::Io(std::io::Error::last_os_error());
                return match self.remove_child_file(&temporary, temporary_authority) {
                    Ok(()) => Err(publish),
                    Err(cleanup) => Err(SandboxError::ExecFailed(format!(
                        "authority publish failed ({publish}); exact temporary cleanup also failed ({cleanup})"
                    ))),
                };
            }
        }
        #[cfg(windows)]
        {
            if let Err(publish) = temporary_authority.rename_into(self, name, true) {
                return match self.remove_child_file(&temporary, temporary_authority) {
                    Ok(()) => Err(publish),
                    Err(cleanup) => Err(SandboxError::ExecFailed(format!(
                        "authority publish failed ({publish}); exact temporary cleanup also failed ({cleanup})"
                    ))),
                };
            }
        }
        #[cfg(not(any(unix, windows)))]
        return Err(SandboxError::PolicyNotSupported(
            "relative atomic write is unsupported on this platform".to_owned(),
        ));
        self.handle.sync_all()?;
        Ok(())
    }

    fn remove_child_file(&self, name: &str, authority: RegularFileAuthority) -> Result<()> {
        validate_child_name(name)?;
        let path = self.display_path.join(name);
        #[cfg(unix)]
        {
            authority.validate_path(&path)?;
            use std::ffi::CString;
            use std::os::fd::AsRawFd;

            let name = CString::new(name).map_err(|_| {
                SandboxError::PathDenied("authority child name contains NUL".to_owned())
            })?;
            // SAFETY: the name is one child of the retained parent and was
            // identity-checked against the still-open file authority.
            if unsafe { libc::unlinkat(self.handle.as_raw_fd(), name.as_ptr(), 0) } != 0 {
                return Err(std::io::Error::last_os_error().into());
            }
            drop(authority);
        }
        #[cfg(windows)]
        {
            windows::delete_open_object(&authority.handle, &path, "file")?;
            drop(authority);
        }
        self.handle.sync_all()?;
        Ok(())
    }

    /// Create and retain one private direct child directory without resolving
    /// the parent through the ambient pathname namespace.
    pub fn create_child_directory(&self, name: &str) -> Result<Self> {
        validate_child_name(name)?;
        #[cfg(unix)]
        {
            use std::ffi::CString;
            use std::os::fd::AsRawFd;

            let name_c = CString::new(name).map_err(|_| {
                SandboxError::PathDenied("authority child name contains NUL".to_owned())
            })?;
            // SAFETY: parent descriptor and name are valid. mkdirat is scoped
            // to the retained parent object, not its mutable display path.
            if unsafe { libc::mkdirat(self.handle.as_raw_fd(), name_c.as_ptr(), 0o700) } != 0 {
                return Err(std::io::Error::last_os_error().into());
            }
            match self.open_child_directory(name) {
                Ok(child) => Ok(child),
                Err(error) => {
                    // SAFETY: remove only the just-created relative entry.
                    unsafe {
                        libc::unlinkat(
                            self.handle.as_raw_fd(),
                            name_c.as_ptr(),
                            libc::AT_REMOVEDIR,
                        );
                    }
                    Err(error)
                }
            }
        }
        #[cfg(windows)]
        {
            windows::create_child_directory(self, name)
        }
        #[cfg(not(any(unix, windows)))]
        Err(SandboxError::PolicyNotSupported(
            "relative directory creation is unsupported on this platform".to_owned(),
        ))
    }

    /// Open an existing child or create it exactly once beneath this retained
    /// parent. A concurrent non-directory or linked entry is rejected.
    pub fn open_or_create_child_directory(&self, name: &str) -> Result<Self> {
        match self.open_child_directory(name) {
            Ok(child) => Ok(child),
            Err(SandboxError::Io(error)) if error.kind() == std::io::ErrorKind::NotFound => {
                match self.create_child_directory(name) {
                    Ok(child) => Ok(child),
                    Err(SandboxError::Io(error))
                        if error.kind() == std::io::ErrorKind::AlreadyExists =>
                    {
                        self.open_child_directory(name)
                    }
                    Err(error) => Err(error),
                }
            }
            Err(error) => Err(error),
        }
    }

    /// Create a new private regular file relative to the retained directory.
    pub fn create_child_file(&self, name: &str, contents: &[u8]) -> Result<RegularFileAuthority> {
        validate_child_name(name)?;
        #[cfg(unix)]
        {
            use std::ffi::CString;
            use std::os::fd::{AsRawFd, FromRawFd};

            let name_c = CString::new(name).map_err(|_| {
                SandboxError::PathDenied("authority child name contains NUL".to_owned())
            })?;
            // SAFETY: the retained parent descriptor scopes creation and the
            // flags forbid following or replacing an existing entry.
            let fd = unsafe {
                libc::openat(
                    self.handle.as_raw_fd(),
                    name_c.as_ptr(),
                    libc::O_RDWR
                        | libc::O_CREAT
                        | libc::O_EXCL
                        | libc::O_NOFOLLOW
                        | libc::O_CLOEXEC,
                    0o600,
                )
            };
            if fd < 0 {
                return Err(std::io::Error::last_os_error().into());
            }
            // SAFETY: successful openat returned a fresh owned descriptor.
            let mut handle = unsafe { File::from_raw_fd(fd) };
            if let Err(error) = handle.write_all(contents).and_then(|()| handle.sync_all()) {
                // SAFETY: only the newly created relative name is removed.
                unsafe {
                    libc::unlinkat(self.handle.as_raw_fd(), name_c.as_ptr(), 0);
                }
                return Err(error.into());
            }
            let metadata = handle.metadata()?;
            let identity = handle_directory_identity(&handle, &metadata)?;
            Ok(RegularFileAuthority {
                handle,
                identity,
                display_path: self.display_path.join(name),
            })
        }
        #[cfg(windows)]
        {
            windows::create_child_file(self, name, contents)
        }
        #[cfg(not(any(unix, windows)))]
        Err(SandboxError::PolicyNotSupported(
            "relative file creation is unsupported on this platform".to_owned(),
        ))
    }

    /// Run a child process with its working directory bound to this retained
    /// directory object. Unix uses `fchdir` after fork; Windows relies on the
    /// no-delete-share handle preventing pathname replacement.
    pub fn bind_command_cwd(&self, command: &mut tokio::process::Command) -> Result<()> {
        #[cfg(unix)]
        {
            use std::os::fd::AsRawFd;
            use std::os::unix::process::CommandExt;

            let fd = self.handle.as_raw_fd();
            // SAFETY: pre_exec invokes only the async-signal-safe fchdir
            // syscall. The retained handle outlives command construction and
            // is inherited across fork until exec.
            unsafe {
                command.as_std_mut().pre_exec(move || {
                    if libc::fchdir(fd) == 0 {
                        Ok(())
                    } else {
                        Err(std::io::Error::last_os_error())
                    }
                });
            }
            Ok(())
        }
        #[cfg(windows)]
        {
            windows::bind_command_cwd(self, command)
        }
        #[cfg(not(any(unix, windows)))]
        Err(SandboxError::PolicyNotSupported(
            "retained command cwd is unsupported on this platform".to_owned(),
        ))
    }

    /// Sync directory metadata through the retained handle.
    pub fn sync(&self) -> Result<()> {
        self.handle.sync_all()?;
        Ok(())
    }

    /// Remove every entry beneath this retained directory without reopening
    /// the directory's display path. The directory object itself remains.
    pub fn remove_descendants(&self) -> Result<()> {
        #[cfg(unix)]
        {
            let duplicate = self.handle.try_clone()?;
            let directory = cap_std::fs::Dir::from_std_file(duplicate);
            for entry in directory.entries()? {
                let entry = entry?;
                let name = entry.file_name();
                let metadata = entry.metadata()?;
                if metadata.is_dir() {
                    directory.remove_dir_all(&name)?;
                } else {
                    directory.remove_file(&name)?;
                }
            }
            self.handle.sync_all()?;
            Ok(())
        }
        #[cfg(windows)]
        {
            windows::remove_descendants(self)
        }
        #[cfg(not(any(unix, windows)))]
        Err(SandboxError::PolicyNotSupported(
            "capability-relative cleanup is unsupported on this platform".to_owned(),
        ))
    }

    /// Remove this exact retained directory object and all descendants.
    /// Unix delegates to cap-std's open-directory removal, which locates the
    /// directory by the held inode rather than trusting its display path.
    pub fn remove_open_dir_all(self) -> std::result::Result<(), Box<(SandboxError, Self)>> {
        #[cfg(unix)]
        {
            let duplicate = self
                .handle
                .try_clone()
                .map_err(|error| Box::new((error.into(), self.clone())))?;
            cap_std::fs::Dir::from_std_file(duplicate)
                .remove_open_dir_all()
                .map_err(|error| Box::new((error.into(), self)))
        }
        #[cfg(windows)]
        {
            windows::remove_open_dir_all(self)
        }
        #[cfg(not(any(unix, windows)))]
        Err(Box::new((
            SandboxError::PolicyNotSupported(
                "open-directory cleanup is unsupported on this platform".to_owned(),
            ),
            self,
        )))
    }

    /// Remove one empty direct child through this retained parent authority.
    /// Callers must first retain and empty the exact child object.
    pub fn remove_empty_child_directory(&self, name: &str) -> Result<()> {
        validate_child_name(name)?;
        #[cfg(unix)]
        {
            use std::ffi::CString;
            use std::os::fd::AsRawFd;

            let name = CString::new(name).map_err(|_| {
                SandboxError::PathDenied("authority child name contains NUL".to_owned())
            })?;
            // SAFETY: the operation is scoped to the retained parent and one
            // validated direct child name.
            if unsafe { libc::unlinkat(self.handle.as_raw_fd(), name.as_ptr(), libc::AT_REMOVEDIR) }
                != 0
            {
                return Err(std::io::Error::last_os_error().into());
            }
            self.handle.sync_all()?;
            Ok(())
        }
        #[cfg(windows)]
        {
            windows::remove_empty_child_directory(self, name)
        }
        #[cfg(not(any(unix, windows)))]
        Err(SandboxError::PolicyNotSupported(
            "capability-relative cleanup is unsupported on this platform".to_owned(),
        ))
    }

    fn validate_path_inner(&self, path: &Path, hook: impl FnOnce()) -> Result<()> {
        let before_metadata = std::fs::symlink_metadata(path)?;
        validate_real_directory(path, &before_metadata)?;
        let before = path_directory_identity(path, &before_metadata)?;
        hook();
        let held_metadata = self.handle.metadata()?;
        validate_real_directory(path, &held_metadata)?;
        let held = handle_directory_identity(&self.handle, &held_metadata)?;
        let after_metadata = std::fs::symlink_metadata(path)?;
        validate_real_directory(path, &after_metadata)?;
        let after = path_directory_identity(path, &after_metadata)?;
        if held != self.identity || before != held || held != after {
            return Err(identity_changed(path, "after authority was retained"));
        }
        Ok(())
    }

    /// Rename the exact held Windows directory object beneath an already-held
    /// destination parent, never whichever object occupies either pathname.
    #[cfg(windows)]
    pub fn rename_into(
        &self,
        destination_parent: &DirectoryAuthority,
        child_name: &str,
        replace: bool,
    ) -> Result<()> {
        validate_child_name(child_name)?;
        windows::rename_directory_into(self, destination_parent, child_name, replace)
    }
}

impl RetainedWorkspaceAuthority {
    /// Bind an owner directory to one of its direct child workspaces. Both
    /// authorities must already be retained; `new` proves the child is exactly
    /// the object the owner currently names, so a later pathname replacement
    /// cannot redirect execution or cleanup.
    ///
    /// `transaction_id` is validated (non-empty, bounded, NUL-free) as an
    /// owner-issued label. It is not retained by the Task 1A/1B native path;
    /// the Docker archive transport (Task 1D) re-introduces a stored id when it
    /// needs a durable recovery-journal key.
    pub fn new(
        owner: DirectoryAuthority,
        workspace: DirectoryAuthority,
        transaction_id: impl Into<String>,
    ) -> Result<Self> {
        let display = workspace.display_path();
        let parent = display.parent().ok_or_else(|| {
            SandboxError::PathDenied("retained workspace has no owner parent".to_owned())
        })?;
        if parent != owner.display_path() {
            return Err(SandboxError::PathDenied(
                "retained workspace is not a direct child of its owner".to_owned(),
            ));
        }
        let child_name = display
            .file_name()
            .and_then(std::ffi::OsStr::to_str)
            .ok_or_else(|| {
                SandboxError::PathDenied("retained workspace name is not valid Unicode".to_owned())
            })?
            .to_owned();
        validate_child_name(&child_name)?;
        let transaction_id = transaction_id.into();
        if transaction_id.is_empty() || transaction_id.len() > 256 || transaction_id.contains('\0')
        {
            return Err(SandboxError::PathDenied(
                "retained workspace transaction ID is invalid".to_owned(),
            ));
        }
        owner.validate_path(owner.display_path())?;
        workspace.validate_path(display)?;
        let observed = owner.open_child_directory(&child_name)?;
        if observed.identity_token() != workspace.identity_token() {
            return Err(SandboxError::PathDenied(
                "retained workspace child identity contradicts owner authority".to_owned(),
            ));
        }
        #[cfg(not(any(feature = "live-docker", test)))]
        let _ = transaction_id;
        Ok(Self {
            owner,
            workspace,
            child_name,
            #[cfg(any(feature = "live-docker", test))]
            transaction_id,
        })
    }

    /// The retained checkout authority a worker is bound to. This is the only
    /// authority a delegated child receives.
    pub fn workspace(&self) -> &DirectoryAuthority {
        &self.workspace
    }

    /// True while any descendant still holds a duplicate of the retained
    /// checkout handle — for example a worker that inherited the directory
    /// descriptor across the sandbox spawn boundary (see `backends::bwrap`).
    ///
    /// Terminal cleanup must refuse to remove the checkout while this holds, so
    /// a live child cannot have its working directory deleted out from under it
    /// and a same-path replacement cannot be substituted before the loan drops.
    pub fn checkout_has_outstanding_loans(&self) -> bool {
        self.workspace.has_outstanding_handle_loans()
    }

    /// Re-prove, at a trust boundary, that the owner and its named child are
    /// still the exact retained objects. Fails closed on any identity drift.
    pub fn validate(&self) -> Result<()> {
        self.owner.validate_path(self.owner.display_path())?;
        self.workspace
            .validate_path(self.workspace.display_path())?;
        let observed = self.owner.open_child_directory(&self.child_name)?;
        if observed.identity_token() != self.workspace.identity_token() {
            return Err(SandboxError::PathDenied(
                "retained workspace identity changed beneath its owner".to_owned(),
            ));
        }
        Ok(())
    }
}

fn validate_child_name(name: &str) -> Result<()> {
    let mut components = Path::new(name).components();
    if !matches!(components.next(), Some(std::path::Component::Normal(_)))
        || components.next().is_some()
    {
        return Err(SandboxError::PathDenied(format!(
            "authority child must be one safe component: {name:?}"
        )));
    }
    Ok(())
}

fn identity_changed(path: &Path, when: &str) -> SandboxError {
    SandboxError::PathDenied(format!(
        "directory identity changed {when}: {}",
        path.display()
    ))
}

fn file_identity_changed(path: &Path, when: &str) -> SandboxError {
    SandboxError::PathDenied(format!(
        "regular file identity changed {when}: {}",
        path.display()
    ))
}

fn open_directory(path: &Path) -> std::io::Result<File> {
    let mut options = OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;

        options.custom_flags(libc::O_DIRECTORY | libc::O_NOFOLLOW);
    }
    #[cfg(windows)]
    {
        return windows::open_directory(path);
    }
    options.open(path)
}

fn open_regular_file(path: &Path) -> std::io::Result<File> {
    let mut options = OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;

        options.custom_flags(libc::O_NOFOLLOW);
    }
    #[cfg(windows)]
    {
        return windows::open_regular_file(path);
    }
    options.open(path)
}

fn validate_real_directory(path: &Path, metadata: &std::fs::Metadata) -> Result<()> {
    if !metadata.is_dir() || is_symlink_or_reparse(metadata) {
        return Err(SandboxError::PathDenied(format!(
            "refused non-directory or linked authority root: {}",
            path.display()
        )));
    }
    Ok(())
}

fn validate_real_file(path: &Path, metadata: &std::fs::Metadata) -> Result<()> {
    if !metadata.is_file() || is_symlink_or_reparse(metadata) {
        return Err(SandboxError::PathDenied(format!(
            "refused non-file or linked authority file: {}",
            path.display()
        )));
    }
    Ok(())
}

fn is_symlink_or_reparse(metadata: &std::fs::Metadata) -> bool {
    if metadata.file_type().is_symlink() {
        return true;
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;

        const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x400;
        return metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0;
    }
    #[cfg(not(windows))]
    false
}

#[cfg(unix)]
fn path_directory_identity(
    _path: &Path,
    metadata: &std::fs::Metadata,
) -> Result<DirectoryIdentity> {
    use std::os::unix::fs::MetadataExt;

    Ok(DirectoryIdentity {
        device: metadata.dev(),
        inode: metadata.ino(),
    })
}

#[cfg(unix)]
fn path_file_identity(path: &Path, metadata: &std::fs::Metadata) -> Result<DirectoryIdentity> {
    path_directory_identity(path, metadata)
}

#[cfg(unix)]
fn handle_directory_identity(
    _handle: &File,
    metadata: &std::fs::Metadata,
) -> Result<DirectoryIdentity> {
    path_directory_identity(Path::new("."), metadata)
}

#[cfg(windows)]
fn path_directory_identity(
    path: &Path,
    _metadata: &std::fs::Metadata,
) -> Result<DirectoryIdentity> {
    let handle = open_directory(path)?;
    windows::identity(&handle)
}

#[cfg(windows)]
fn path_file_identity(path: &Path, _metadata: &std::fs::Metadata) -> Result<DirectoryIdentity> {
    let handle = open_regular_file(path)?;
    windows::identity(&handle)
}

#[cfg(windows)]
fn handle_directory_identity(
    handle: &File,
    _metadata: &std::fs::Metadata,
) -> Result<DirectoryIdentity> {
    windows::identity(handle)
}

#[cfg(not(any(unix, windows)))]
fn path_directory_identity(
    _path: &Path,
    _metadata: &std::fs::Metadata,
) -> Result<DirectoryIdentity> {
    Err(SandboxError::PolicyNotSupported(
        "directory identity is unsupported on this platform".to_owned(),
    ))
}

#[cfg(not(any(unix, windows)))]
fn path_file_identity(_path: &Path, _metadata: &std::fs::Metadata) -> Result<DirectoryIdentity> {
    Err(SandboxError::PolicyNotSupported(
        "file identity is unsupported on this platform".to_owned(),
    ))
}

#[cfg(not(any(unix, windows)))]
fn handle_directory_identity(
    _handle: &File,
    _metadata: &std::fs::Metadata,
) -> Result<DirectoryIdentity> {
    Err(SandboxError::PolicyNotSupported(
        "directory identity is unsupported on this platform".to_owned(),
    ))
}

#[cfg(all(test, unix))]
#[path = "directory_authority_tests.rs"]
mod tests;

// Windows-only retained-handle proof. Physically implemented in
// `directory_authority_windows_tests.rs`, but mounted here as
// `directory_authority::tests` so its identities are exactly
// `directory_authority::tests::windows_*`. Only one `tests` module is ever
// compiled per platform (the cfgs are mutually exclusive).
#[cfg(all(test, windows))]
#[path = "directory_authority_windows_tests.rs"]
mod tests;
