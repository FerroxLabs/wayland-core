// `hello` is a framework-validation fixture (see `hello.rs`) — compiled and
// registered ONLY under `cfg(test)` so it never reaches the shipped skill
// catalog. In production it leaked: models saw it in every session's catalog
// and narrated skipping it into user-facing output.
#[cfg(test)]
mod hello;

use std::path::PathBuf;
use std::sync::OnceLock;
use std::sync::atomic::{AtomicU64, Ordering};

use crate::types::{ExecutionContext, LoadedFrom, SkillMetadata, SkillSource};

#[cfg(not(windows))]
static BUNDLED_SKILL_EXTRACT_ROOT: OnceLock<Result<PathBuf, String>> = OnceLock::new();

#[cfg(windows)]
#[derive(Debug)]
struct WindowsBundledSkillRoot {
    path: PathBuf,
    dir: Option<std::sync::Arc<cap_std::fs::Dir>>,
}

#[cfg(windows)]
static WINDOWS_BUNDLED_SKILL_EXTRACT_ROOT: OnceLock<
    Result<std::sync::Mutex<WindowsBundledSkillRoot>, String>,
> = OnceLock::new();

// ---------------------------------------------------------------------------
// Public types
// ---------------------------------------------------------------------------

/// Definition for a bundled skill compiled into the binary.
///
/// All string fields use `&'static str` because bundled skill definitions are
/// compile-time constants embedded in the binary.
pub struct BundledSkillDefinition {
    pub name: &'static str,
    pub description: &'static str,
    pub when_to_use: Option<&'static str>,
    pub argument_hint: Option<&'static str>,
    pub allowed_tools: &'static [&'static str],
    pub model: Option<&'static str>,
    pub disable_model_invocation: bool,
    pub user_invocable: bool,
    /// "inline" | "fork"
    pub context: Option<&'static str>,
    pub agent: Option<&'static str>,
    /// Embedded reference files: (relative_path, content) pairs.
    /// Extracted to disk when the owning catalog is prepared.
    pub files: &'static [(&'static str, &'static str)],
    /// Skill body content (Markdown).
    pub content: &'static str,
}

/// Session-owned bundled skill data.
///
/// Embedded definitions are copied into this owned shape, while plugin
/// adapters can move their owned strings into it without leaking memory.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BundledSkillEntry {
    pub name: String,
    pub description: String,
    pub when_to_use: Option<String>,
    pub argument_hint: Option<String>,
    pub allowed_tools: Vec<String>,
    pub model: Option<String>,
    pub disable_model_invocation: bool,
    pub user_invocable: bool,
    /// "inline" | "fork"
    pub context: Option<String>,
    pub agent: Option<String>,
    /// Embedded reference files: (relative_path, content) pairs.
    pub files: Vec<(String, String)>,
    /// Skill body content (Markdown).
    pub content: String,
}

impl From<BundledSkillDefinition> for BundledSkillEntry {
    fn from(def: BundledSkillDefinition) -> Self {
        Self {
            name: def.name.to_owned(),
            description: def.description.to_owned(),
            when_to_use: def.when_to_use.map(str::to_owned),
            argument_hint: def.argument_hint.map(str::to_owned),
            allowed_tools: def
                .allowed_tools
                .iter()
                .map(|tool| (*tool).to_owned())
                .collect(),
            model: def.model.map(str::to_owned),
            disable_model_invocation: def.disable_model_invocation,
            user_invocable: def.user_invocable,
            context: def.context.map(str::to_owned),
            agent: def.agent.map(str::to_owned),
            files: def
                .files
                .iter()
                .map(|(path, content)| ((*path).to_owned(), (*content).to_owned()))
                .collect(),
            content: def.content.to_owned(),
        }
    }
}

/// Bundled and plugin skills owned by one bootstrap/session.
///
/// Entries retain insertion order. `embedded()` installs embedded definitions
/// first; bootstrap then appends plugin entries so existing precedence stays
/// embedded-first, plugin-second.
#[derive(Debug)]
pub struct BundledSkillCatalog {
    entries: Vec<BundledSkillEntry>,
    // SkillRefs outlive the bootstrap-local catalog, so this root is retained
    // until the existing process-level cleanup removes the private temp tree.
    extraction_root: Option<PathBuf>,
    #[cfg(windows)]
    extraction_dir: Option<std::sync::Arc<cap_std::fs::Dir>>,
}

impl Default for BundledSkillCatalog {
    fn default() -> Self {
        static NEXT_CATALOG_ID: AtomicU64 = AtomicU64::new(0);

        let catalog_id = NEXT_CATALOG_ID.fetch_add(1, Ordering::Relaxed);
        #[cfg(not(windows))]
        let process_root = bundled_skill_extract_root()
            .map_err(|error| {
                tracing::warn!(%error, "bundled skill reference extraction disabled");
                error
            })
            .ok();

        #[cfg(windows)]
        let (extraction_root, extraction_dir) = match windows_bundled_skill_extract_root() {
            Ok((process_root, dir)) => (
                Some(process_root.join(format!("catalog-{catalog_id}"))),
                Some(dir),
            ),
            Err(error) => {
                tracing::warn!(%error, "bundled skill capability root unavailable");
                (None, None)
            }
        };

        #[cfg(not(windows))]
        let extraction_root = process_root.map(|root| root.join(format!("catalog-{catalog_id}")));

        Self {
            entries: Vec::new(),
            extraction_root,
            #[cfg(windows)]
            extraction_dir,
        }
    }
}

impl BundledSkillCatalog {
    /// Create an empty catalog.
    pub fn new() -> Self {
        Self::default()
    }

    /// Create a catalog containing only definitions embedded in this binary.
    pub fn embedded() -> Self {
        Self::new()
    }

    /// Append one owned entry to this catalog.
    pub fn register(&mut self, entry: BundledSkillEntry) {
        self.entries.push(entry);
    }

    /// Convert this catalog to runtime metadata without extracting files.
    pub fn get_bundled_skills(&self) -> Vec<SkillMetadata> {
        self.entries.iter().map(entry_to_metadata).collect()
    }

    /// Convert this catalog to runtime metadata and extract reference files.
    pub async fn prepare_bundled_skills(&self) -> Vec<SkillMetadata> {
        let mut skills = self.get_bundled_skills();
        let Some(extraction_root) = self.extraction_root.as_ref() else {
            return skills;
        };

        for (entry_index, entry) in self
            .entries
            .iter()
            .enumerate()
            .filter(|(_, entry)| !entry.files.is_empty())
        {
            let files: Vec<(&str, &str)> = entry
                .files
                .iter()
                .map(|(path, content)| (path.as_str(), content.as_str()))
                .collect();
            let dir = extraction_root.join(format!("skill-{entry_index}"));
            #[cfg(windows)]
            let extracted = match self.extraction_dir.as_ref() {
                Some(root_dir) => {
                    let relative_dir = PathBuf::from(
                        extraction_root
                            .file_name()
                            .expect("catalog extraction root has a final component"),
                    )
                    .join(format!("skill-{entry_index}"));
                    extract_bundled_skill_files_to_dir_windows(
                        root_dir.clone(),
                        &entry.name,
                        relative_dir,
                        dir.clone(),
                        &files,
                    )
                    .await
                }
                None => None,
            };
            #[cfg(not(windows))]
            let extracted =
                extract_bundled_skill_files_to_dir(&entry.name, dir.clone(), &files).await;

            if let Some(dir) = extracted
                && let Some(meta) = skills.get_mut(entry_index)
            {
                meta.skill_root = Some(dir.to_string_lossy().into_owned());
            }
        }

        skills
    }
}

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// Append one compile-time bundled definition to an explicit catalog.
pub fn register_bundled_skill(catalog: &mut BundledSkillCatalog, def: BundledSkillDefinition) {
    catalog.register(def.into());
}

/// Initialize all built-in bundled skills.
///
/// Returns a fresh catalog each time, so one bootstrap cannot observe entries
/// appended by another bootstrap.
pub fn init_bundled_skills() -> BundledSkillCatalog {
    // The only bundled skill today is the `hello` test fixture, which must NOT
    // ship in the production catalog (models notice it and narrate skipping
    // it). Register it solely under `cfg(test)` so the bundled-skill framework
    // stays exercised by TC-10.04 / TC-10.28 without leaking to users. In a
    // shipped build this returns an empty catalog — correct, since no
    // production bundled skills exist yet.
    #[cfg(test)]
    {
        let mut catalog = BundledSkillCatalog::embedded();
        hello::register_hello_skill(&mut catalog);
        catalog
    }
    #[cfg(not(test))]
    {
        BundledSkillCatalog::embedded()
    }
}

/// F-086: remove the per-process bundled-skill extraction root directory.
///
/// Called at graceful shutdown to clean up the `$TMPDIR/wayland-core-bundled-skills-{uuid}/`
/// directory that catalog preparation creates. Best-effort: failures
/// are silently ignored (the OS will eventually purge `$TMPDIR`).
///
/// Register this with an `atexit`-style hook or call from the CLI's shutdown
/// path to prevent temp-dir accumulation across restarts.
pub fn cleanup_bundled_skill_extract_dir() {
    #[cfg(not(windows))]
    {
        if let Some(Ok(root)) = BUNDLED_SKILL_EXTRACT_ROOT.get()
            && root.is_dir()
        {
            let _ = std::fs::remove_dir_all(root);
        }
    }
    #[cfg(windows)]
    {
        let Some(Ok(root)) = WINDOWS_BUNDLED_SKILL_EXTRACT_ROOT.get() else {
            return;
        };
        let Ok(mut root) = root.lock() else {
            return;
        };
        let path = root.path.clone();
        let retained_handle = root.dir.take();
        drop(root);
        // The CLI guard is declared before bootstrap/session state, so normal
        // reverse drop order releases every catalog clone before this point.
        drop(retained_handle);
        let _ = std::fs::remove_dir_all(path);
    }
}

#[cfg(not(windows))]
fn bundled_skill_extract_root() -> std::io::Result<PathBuf> {
    match BUNDLED_SKILL_EXTRACT_ROOT
        .get_or_init(|| create_bundled_skill_extract_root().map_err(|error| error.to_string()))
    {
        Ok(root) => Ok(root.clone()),
        Err(error) => Err(std::io::Error::other(error.clone())),
    }
}

#[cfg(not(windows))]
fn create_bundled_skill_extract_root() -> std::io::Result<PathBuf> {
    // Resolve any platform-managed symlinks in the OS temp path once, before
    // joining our exclusively-created leaf. No attacker-precreated path
    // component is followed during the leaf creation itself.
    let temp_root = std::fs::canonicalize(std::env::temp_dir())?;

    for _ in 0..16 {
        let candidate = temp_root.join(format!(
            "wayland-core-bundled-skills-{}",
            uuid::Uuid::new_v4()
        ));
        match create_owner_only_process_root(&candidate) {
            Ok(()) => return Ok(candidate),
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(error) => return Err(error),
        }
    }

    Err(std::io::Error::new(
        std::io::ErrorKind::AlreadyExists,
        "could not allocate a unique bundled skill extraction root",
    ))
}

#[cfg(not(windows))]
fn create_owner_only_process_root(path: &std::path::Path) -> std::io::Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        std::fs::DirBuilder::new().mode(0o700).create(path)
    }
    #[cfg(not(unix))]
    {
        std::fs::create_dir(path)
    }
}

#[cfg(windows)]
fn windows_bundled_skill_extract_root()
-> std::io::Result<(PathBuf, std::sync::Arc<cap_std::fs::Dir>)> {
    let root = WINDOWS_BUNDLED_SKILL_EXTRACT_ROOT.get_or_init(|| {
        create_windows_bundled_skill_extract_root()
            .map(std::sync::Mutex::new)
            .map_err(|error| error.to_string())
    });
    let root = root
        .as_ref()
        .map_err(|error| std::io::Error::other(error.clone()))?;
    let root = root
        .lock()
        .map_err(|_| std::io::Error::other("bundled skill root lock poisoned"))?;
    let dir = root.dir.as_ref().cloned().ok_or_else(|| {
        std::io::Error::other("bundled skill root has already entered shutdown cleanup")
    })?;
    Ok((root.path.clone(), dir))
}

#[cfg(windows)]
fn create_windows_bundled_skill_extract_root() -> std::io::Result<WindowsBundledSkillRoot> {
    let temp_root = std::fs::canonicalize(std::env::temp_dir())?;

    for _ in 0..16 {
        let candidate = temp_root.join(format!(
            "wayland-core-bundled-skills-{}",
            uuid::Uuid::new_v4()
        ));
        match create_windows_owner_only_directory(&candidate) {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(error) => return Err(error),
        }

        // Pin the exact leaf before ACL hardening. The handle is opened
        // no-follow and without FILE_SHARE_DELETE, so the directory cannot be
        // renamed or replaced while its protected DACL is installed.
        let dir = match open_windows_capability_root(&candidate) {
            Ok(dir) => dir,
            Err(error) => {
                let _ = std::fs::remove_dir(&candidate);
                return Err(error);
            }
        };
        if let Err(error) = restrict_windows_handle_acl(&dir, true) {
            drop(dir);
            let _ = std::fs::remove_dir(&candidate);
            return Err(error);
        }
        return Ok(WindowsBundledSkillRoot {
            path: candidate,
            dir: Some(std::sync::Arc::new(dir)),
        });
    }

    Err(std::io::Error::new(
        std::io::ErrorKind::AlreadyExists,
        "could not allocate a unique bundled skill extraction root",
    ))
}

#[cfg(not(windows))]
async fn extract_bundled_skill_files_to_dir(
    skill_name: &str,
    dir: PathBuf,
    files: &[(&str, &str)],
) -> Option<PathBuf> {
    if files.is_empty() {
        return None;
    }

    match write_skill_files(&dir, files).await {
        Ok(()) => Some(dir),
        Err(e) => {
            // Non-fatal: log and degrade gracefully (skill runs without skill_root)
            eprintln!(
                "[wayland-core] failed to extract bundled skill '{}' to {}: {}",
                skill_name,
                dir.display(),
                e
            );
            None
        }
    }
}

#[cfg(windows)]
async fn extract_bundled_skill_files_to_dir_windows(
    root_dir: std::sync::Arc<cap_std::fs::Dir>,
    skill_name: &str,
    relative_dir: PathBuf,
    absolute_dir: PathBuf,
    files: &[(&str, &str)],
) -> Option<PathBuf> {
    if files.is_empty() {
        return None;
    }

    let skill_name = skill_name.to_owned();
    let files: Vec<(String, String)> = files
        .iter()
        .map(|(path, content)| ((*path).to_owned(), (*content).to_owned()))
        .collect();
    let extraction_dir = absolute_dir.clone();
    let result = tokio::task::spawn_blocking(move || {
        write_skill_files_windows(&root_dir, &relative_dir, &extraction_dir, &files)
    })
    .await
    .map_err(std::io::Error::other)
    .and_then(|result| result);

    match result {
        Ok(()) => Some(absolute_dir),
        Err(error) => {
            eprintln!(
                "[wayland-core] failed to extract bundled skill '{}' to {}: {}",
                skill_name,
                absolute_dir.display(),
                error
            );
            None
        }
    }
}

// ---------------------------------------------------------------------------
// Internal: conversion
// ---------------------------------------------------------------------------

fn entry_to_metadata(entry: &BundledSkillEntry) -> SkillMetadata {
    let execution_context = match entry.context.as_deref() {
        Some("fork") => ExecutionContext::Fork,
        _ => ExecutionContext::Inline,
    };

    let content_length = entry.content.len();

    SkillMetadata {
        name: entry.name.clone(),
        display_name: None,
        description: entry.description.clone(),
        has_user_specified_description: true,
        allowed_tools: entry.allowed_tools.clone(),
        argument_hint: entry.argument_hint.clone(),
        argument_names: Vec::new(),
        when_to_use: entry.when_to_use.clone(),
        version: None,
        model: entry.model.clone(),
        disable_model_invocation: entry.disable_model_invocation,
        user_invocable: entry.user_invocable,
        execution_context,
        agent: entry.agent.clone(),
        effort: None,
        shell: None,
        paths: Vec::new(),
        artifacts: Vec::new(),
        hooks_raw: None,
        source: SkillSource::Bundled,
        loaded_from: LoadedFrom::Bundled,
        content: entry.content.clone(),
        content_length,
        // skill_root is set later when the owning catalog is prepared.
        skill_root: None,
        max_turns: None,
        max_tokens: None,
    }
}

// ---------------------------------------------------------------------------
// Internal: file extraction
// ---------------------------------------------------------------------------

#[cfg(not(windows))]
async fn write_skill_files(dir: &std::path::Path, files: &[(&str, &str)]) -> std::io::Result<()> {
    use std::collections::HashMap;

    // Group files by parent directory to minimise mkdir calls.
    let mut by_parent: HashMap<PathBuf, Vec<(PathBuf, &str)>> = HashMap::new();
    for (rel_path, content) in files {
        let target = resolve_skill_file_path(dir, rel_path)?;
        let parent = target
            .parent()
            .ok_or_else(|| {
                std::io::Error::new(std::io::ErrorKind::InvalidInput, "path has no parent")
            })?
            .to_owned();
        by_parent.entry(parent).or_default().push((target, content));
    }

    // Create directories and write files.
    for (parent, entries) in by_parent {
        create_dir_secure(&parent).await?;
        for (path, content) in entries {
            safe_write_file(&path, content).await?;
        }
    }

    Ok(())
}

/// Create a directory (and all parents) with owner-only permissions.
#[cfg(not(windows))]
async fn create_dir_secure(dir: &std::path::Path) -> std::io::Result<()> {
    let dir = dir.to_owned();
    tokio::task::spawn_blocking(move || {
        #[cfg(unix)]
        {
            use std::os::unix::fs::DirBuilderExt;
            std::fs::DirBuilder::new()
                .recursive(true)
                .mode(0o700)
                .create(&dir)
        }
        #[cfg(not(unix))]
        {
            std::fs::create_dir_all(&dir)
        }
    })
    .await
    .map_err(std::io::Error::other)?
}

#[cfg(windows)]
struct WindowsTokenUser {
    // `TOKEN_USER` contains a pointer into this allocation. Store machine
    // words rather than bytes so dereferencing the header is aligned.
    buffer: Vec<usize>,
}

#[cfg(windows)]
impl WindowsTokenUser {
    fn sid(&self) -> windows_sys::Win32::Security::PSID {
        use windows_sys::Win32::Security::TOKEN_USER;

        // SAFETY: `current_windows_token_user` sizes and fills this allocation
        // with GetTokenInformation(TokenUser), and Vec<usize> supplies the
        // alignment required by TOKEN_USER.
        unsafe { (*(self.buffer.as_ptr().cast::<TOKEN_USER>())).User.Sid }
    }
}

#[cfg(windows)]
fn current_windows_token_user() -> std::io::Result<WindowsTokenUser> {
    use std::os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle};
    use windows_sys::Win32::Foundation::{ERROR_INSUFFICIENT_BUFFER, HANDLE};
    use windows_sys::Win32::Security::{GetTokenInformation, TOKEN_QUERY, TokenUser};
    use windows_sys::Win32::System::Threading::{GetCurrentProcess, OpenProcessToken};

    let mut token: HANDLE = std::ptr::null_mut();
    // SAFETY: token is a valid out-pointer and the returned handle is
    // transferred immediately into OwnedHandle.
    if unsafe { OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token) } == 0 {
        return Err(std::io::Error::last_os_error());
    }
    // SAFETY: OpenProcessToken returned ownership of this valid handle.
    let token = unsafe { OwnedHandle::from_raw_handle(token) };

    let mut needed = 0;
    // SAFETY: the null/zero probe is the documented sizing call.
    if unsafe {
        GetTokenInformation(
            token.as_raw_handle(),
            TokenUser,
            std::ptr::null_mut(),
            0,
            &mut needed,
        )
    } != 0
    {
        return Err(std::io::Error::other(
            "GetTokenInformation(TokenUser) sizing unexpectedly succeeded",
        ));
    }
    let sizing_error = std::io::Error::last_os_error();
    if sizing_error.raw_os_error() != Some(ERROR_INSUFFICIENT_BUFFER as i32) || needed == 0 {
        return Err(sizing_error);
    }

    let words = (needed as usize).div_ceil(std::mem::size_of::<usize>());
    let mut buffer = vec![0usize; words];
    // SAFETY: buffer is writable for at least `needed` bytes and token remains
    // owned for the duration of the call.
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
    Ok(WindowsTokenUser { buffer })
}

#[cfg(windows)]
struct WindowsLocalAlloc(*mut core::ffi::c_void);

#[cfg(windows)]
impl Drop for WindowsLocalAlloc {
    fn drop(&mut self) {
        use windows_sys::Win32::Foundation::LocalFree;

        if !self.0.is_null() {
            // SAFETY: SetEntriesInAclW allocates this buffer with LocalAlloc.
            unsafe {
                LocalFree(self.0);
            }
        }
    }
}

#[cfg(windows)]
impl WindowsLocalAlloc {
    fn as_acl(&self) -> *mut windows_sys::Win32::Security::ACL {
        self.0.cast()
    }
}

#[cfg(windows)]
fn windows_token_user_acl(
    token_user: &WindowsTokenUser,
    directory: bool,
) -> std::io::Result<WindowsLocalAlloc> {
    use windows_sys::Win32::Security::Authorization::{
        EXPLICIT_ACCESS_W, GRANT_ACCESS, SetEntriesInAclW, TRUSTEE_IS_SID, TRUSTEE_IS_USER,
    };
    use windows_sys::Win32::Security::{CONTAINER_INHERIT_ACE, OBJECT_INHERIT_ACE};
    use windows_sys::Win32::Storage::FileSystem::FILE_ALL_ACCESS;

    // SAFETY: EXPLICIT_ACCESS_W is a plain C record and zero is the required
    // initial state for fields not assigned below.
    let mut access: EXPLICIT_ACCESS_W = unsafe { std::mem::zeroed() };
    access.grfAccessPermissions = FILE_ALL_ACCESS;
    access.grfAccessMode = GRANT_ACCESS;
    access.grfInheritance = if directory {
        CONTAINER_INHERIT_ACE | OBJECT_INHERIT_ACE
    } else {
        0
    };
    access.Trustee.TrusteeForm = TRUSTEE_IS_SID;
    access.Trustee.TrusteeType = TRUSTEE_IS_USER;
    access.Trustee.ptstrName = token_user.sid().cast();

    let mut dacl = std::ptr::null_mut();
    // SAFETY: access points to a live TokenUser SID for this call and dacl is a
    // valid out-pointer. Passing no old ACL constructs exactly one allow ACE.
    let result = unsafe { SetEntriesInAclW(1, &access, std::ptr::null(), &mut dacl) };
    if result != 0 {
        return Err(std::io::Error::from_raw_os_error(result as i32));
    }
    if dacl.is_null() {
        return Err(std::io::Error::other(
            "SetEntriesInAclW returned a null DACL",
        ));
    }
    Ok(WindowsLocalAlloc(dacl.cast()))
}

#[cfg(windows)]
fn with_windows_owner_only_security_descriptor<T>(
    directory: bool,
    operation: impl FnOnce(*mut core::ffi::c_void) -> std::io::Result<T>,
) -> std::io::Result<T> {
    use windows_sys::Win32::Security::{
        InitializeSecurityDescriptor, SE_DACL_PROTECTED, SECURITY_DESCRIPTOR,
        SetSecurityDescriptorControl, SetSecurityDescriptorDacl, SetSecurityDescriptorOwner,
    };

    const SECURITY_DESCRIPTOR_REVISION: u32 = 1;

    let token_user = current_windows_token_user()?;
    let dacl = windows_token_user_acl(&token_user, directory)?;
    // SAFETY: SECURITY_DESCRIPTOR is initialized by the API before use.
    let mut descriptor: SECURITY_DESCRIPTOR = unsafe { std::mem::zeroed() };
    let descriptor_ptr = std::ptr::addr_of_mut!(descriptor).cast();
    // SAFETY: descriptor_ptr names writable storage for SECURITY_DESCRIPTOR.
    if unsafe { InitializeSecurityDescriptor(descriptor_ptr, SECURITY_DESCRIPTOR_REVISION) } == 0 {
        return Err(std::io::Error::last_os_error());
    }
    // SAFETY: TokenUser and DACL storage remain live through the operation.
    if unsafe { SetSecurityDescriptorOwner(descriptor_ptr, token_user.sid(), 0) } == 0
        || unsafe { SetSecurityDescriptorDacl(descriptor_ptr, 1, dacl.as_acl(), 0) } == 0
        || unsafe {
            SetSecurityDescriptorControl(descriptor_ptr, SE_DACL_PROTECTED, SE_DACL_PROTECTED)
        } == 0
    {
        return Err(std::io::Error::last_os_error());
    }

    operation(descriptor_ptr)
}

/// Create the process root with its final owner-only policy already attached.
/// This closes the inherited-ACL interval that would exist between a normal
/// directory create and the first retained-handle ACL update.
#[cfg(windows)]
fn create_windows_owner_only_directory(path: &std::path::Path) -> std::io::Result<()> {
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::Security::SECURITY_ATTRIBUTES;
    use windows_sys::Win32::Storage::FileSystem::CreateDirectoryW;

    let mut wide: Vec<u16> = path.as_os_str().encode_wide().collect();
    wide.push(0);
    with_windows_owner_only_security_descriptor(true, |descriptor| {
        let attributes = SECURITY_ATTRIBUTES {
            nLength: std::mem::size_of::<SECURITY_ATTRIBUTES>() as u32,
            lpSecurityDescriptor: descriptor,
            bInheritHandle: 0,
        };
        // SAFETY: wide is NUL-terminated and attributes references live
        // TokenUser, ACL, and descriptor storage for the create call.
        if unsafe { CreateDirectoryW(wide.as_ptr(), &attributes) } == 0 {
            return Err(std::io::Error::last_os_error());
        }
        Ok(())
    })
}

/// Atomically create one child beneath a retained directory handle with its
/// final TokenUser owner and protected owner-only DACL. The path is a single
/// counted component, so no ambient path lookup or reparse traversal occurs.
#[cfg(windows)]
fn create_windows_relative_object(
    base: &cap_std::fs::Dir,
    name: &std::ffi::OsStr,
    directory: bool,
) -> std::io::Result<std::fs::File> {
    use std::os::windows::ffi::OsStrExt;
    use std::os::windows::io::{AsRawHandle, FromRawHandle};
    use windows_sys::Wdk::Foundation::OBJECT_ATTRIBUTES;
    use windows_sys::Wdk::Storage::FileSystem::{
        FILE_CREATE, FILE_DIRECTORY_FILE, FILE_NON_DIRECTORY_FILE, FILE_OPEN_REPARSE_POINT,
        FILE_SYNCHRONOUS_IO_NONALERT, NtCreateFile,
    };
    use windows_sys::Win32::Foundation::{HANDLE, STATUS_OBJECT_NAME_COLLISION, UNICODE_STRING};
    use windows_sys::Win32::Storage::FileSystem::{
        FILE_ALL_ACCESS, FILE_ATTRIBUTE_DIRECTORY, FILE_ATTRIBUTE_NORMAL, FILE_SHARE_READ,
        FILE_SHARE_WRITE,
    };
    use windows_sys::Win32::System::IO::IO_STATUS_BLOCK;

    let path = std::path::Path::new(name);
    let mut components = path.components();
    if !matches!(components.next(), Some(std::path::Component::Normal(_)))
        || components.next().is_some()
    {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            format!("invalid retained-handle child name: {}", path.display()),
        ));
    }

    let mut wide: Vec<u16> = name.encode_wide().collect();
    let byte_len = wide
        .len()
        .checked_mul(std::mem::size_of::<u16>())
        .ok_or_else(|| {
            std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "Windows child name is too long",
            )
        })?;
    let byte_len = u16::try_from(byte_len).map_err(|_| {
        std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "Windows child name is too long",
        )
    })?;
    let unicode_name = UNICODE_STRING {
        Length: byte_len,
        MaximumLength: byte_len,
        Buffer: wide.as_mut_ptr(),
    };

    with_windows_owner_only_security_descriptor(directory, |descriptor| {
        let attributes = OBJECT_ATTRIBUTES {
            Length: std::mem::size_of::<OBJECT_ATTRIBUTES>() as u32,
            RootDirectory: base.as_raw_handle(),
            ObjectName: &unicode_name,
            Attributes: 0x40, // OBJ_CASE_INSENSITIVE
            SecurityDescriptor: descriptor,
            SecurityQualityOfService: std::ptr::null(),
        };
        // SAFETY: IO_STATUS_BLOCK is an out-parameter initialized by
        // NtCreateFile before it is observed.
        let mut io_status: IO_STATUS_BLOCK = unsafe { std::mem::zeroed() };
        let mut handle: HANDLE = std::ptr::null_mut();
        let create_options = FILE_OPEN_REPARSE_POINT
            | FILE_SYNCHRONOUS_IO_NONALERT
            | if directory {
                FILE_DIRECTORY_FILE
            } else {
                FILE_NON_DIRECTORY_FILE
            };
        let file_attributes = if directory {
            FILE_ATTRIBUTE_DIRECTORY
        } else {
            FILE_ATTRIBUTE_NORMAL
        };
        // SAFETY: the retained base handle, counted name, security descriptor,
        // and all out-parameters remain live for the duration of the call.
        let status = unsafe {
            NtCreateFile(
                &mut handle,
                FILE_ALL_ACCESS,
                &attributes,
                &mut io_status,
                std::ptr::null(),
                file_attributes,
                FILE_SHARE_READ | FILE_SHARE_WRITE,
                FILE_CREATE,
                create_options,
                std::ptr::null(),
                0,
            )
        };
        if status == STATUS_OBJECT_NAME_COLLISION {
            return Err(std::io::Error::new(
                std::io::ErrorKind::AlreadyExists,
                format!("retained-handle child already exists: {}", path.display()),
            ));
        }
        if status < 0 {
            // SAFETY: translating an NTSTATUS has no pointer preconditions.
            let code = unsafe { windows_sys::Win32::Foundation::RtlNtStatusToDosError(status) };
            return Err(std::io::Error::from_raw_os_error(code as i32));
        }
        if handle.is_null() {
            return Err(std::io::Error::other(
                "NtCreateFile succeeded without returning a handle",
            ));
        }
        // SAFETY: successful NtCreateFile transfers ownership of the handle.
        Ok(unsafe { std::fs::File::from_raw_handle(handle) })
    })
}

/// Replace the DACL on the exact retained handle with one protected allow ACE
/// for the current process TokenUser. No executable lookup, mutable account
/// name, or ambient pathname participates in the operation.
#[cfg(windows)]
fn restrict_windows_handle_acl<T: std::os::windows::io::AsRawHandle>(
    handle: &T,
    directory: bool,
) -> std::io::Result<()> {
    use windows_sys::Win32::Security::Authorization::{SE_FILE_OBJECT, SetSecurityInfo};
    use windows_sys::Win32::Security::{
        DACL_SECURITY_INFORMATION, OWNER_SECURITY_INFORMATION, PROTECTED_DACL_SECURITY_INFORMATION,
    };

    let token_user = current_windows_token_user()?;
    let dacl = windows_token_user_acl(&token_user, directory)?;

    // SAFETY: the caller retained this live file/directory handle with
    // WRITE_DAC and WRITE_OWNER. The DACL buffer and TokenUser SID remain live
    // for the call.
    let result = unsafe {
        SetSecurityInfo(
            handle.as_raw_handle(),
            SE_FILE_OBJECT,
            DACL_SECURITY_INFORMATION
                | OWNER_SECURITY_INFORMATION
                | PROTECTED_DACL_SECURITY_INFORMATION,
            token_user.sid(),
            std::ptr::null_mut(),
            dacl.as_acl(),
            std::ptr::null(),
        )
    };
    if result != 0 {
        return Err(std::io::Error::from_raw_os_error(result as i32));
    }
    Ok(())
}

/// Write `content` to `path` using O_CREAT|O_EXCL and O_NOFOLLOW.
#[cfg(not(windows))]
async fn safe_write_file(path: &std::path::Path, content: &str) -> std::io::Result<()> {
    let file = open_secure(path).await?;
    let mut file = tokio::fs::File::from_std(file);
    use tokio::io::AsyncWriteExt;
    file.write_all(content.as_bytes()).await?;
    file.flush().await
}

/// Open a file for writing with O_CREAT|O_EXCL and O_NOFOLLOW (mode 0o600).
#[cfg(not(windows))]
async fn open_secure(path: &std::path::Path) -> std::io::Result<std::fs::File> {
    let path = path.to_owned();
    // Use spawn_blocking because OpenOptions with custom_flags is synchronous.
    tokio::task::spawn_blocking(move || {
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            std::fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .mode(0o600)
                // O_NOFOLLOW: refuse to open if final path component is a symlink.
                // Belt-and-suspenders alongside O_EXCL (mirrors TS implementation).
                .custom_flags(libc::O_NOFOLLOW)
                .open(&path)
        }
        #[cfg(not(unix))]
        {
            std::fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&path)
        }
    })
    .await
    .map_err(std::io::Error::other)?
}

/// Open the unpredictable process root as a retained Windows directory
/// capability. `FILE_FLAG_OPEN_REPARSE_POINT` rejects a reparse-point root,
/// and omitting `FILE_SHARE_DELETE` prevents it being renamed underneath the
/// handle-relative operations below.
#[cfg(windows)]
fn open_windows_capability_root(path: &std::path::Path) -> std::io::Result<cap_std::fs::Dir> {
    use std::os::windows::fs::{MetadataExt, OpenOptionsExt};
    use windows_sys::Win32::Foundation::GENERIC_READ;
    use windows_sys::Win32::Storage::FileSystem::{
        FILE_ATTRIBUTE_DIRECTORY, FILE_ATTRIBUTE_REPARSE_POINT, FILE_FLAG_BACKUP_SEMANTICS,
        FILE_FLAG_OPEN_REPARSE_POINT, FILE_SHARE_READ, FILE_SHARE_WRITE, READ_CONTROL, WRITE_DAC,
        WRITE_OWNER,
    };

    let file = std::fs::OpenOptions::new()
        .access_mode(GENERIC_READ | READ_CONTROL | WRITE_DAC | WRITE_OWNER)
        .share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE)
        .custom_flags(FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT)
        .open(path)?;
    let attributes = file.metadata()?.file_attributes();
    if attributes & FILE_ATTRIBUTE_DIRECTORY == 0 {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            format!(
                "bundled skill capability root is not a directory: {}",
                path.display()
            ),
        ));
    }
    if attributes & FILE_ATTRIBUTE_REPARSE_POINT != 0 {
        return Err(std::io::Error::new(
            std::io::ErrorKind::PermissionDenied,
            format!(
                "bundled skill capability root is a reparse point: {}",
                path.display()
            ),
        ));
    }
    Ok(cap_std::fs::Dir::from_std_file(file))
}

/// Windows extraction creates every descendant through `NtCreateFile`
/// relative to a retained directory handle. Each create receives the final
/// TokenUser owner and protected DACL atomically; existing components are
/// reopened without following reparse points and hardened through that handle.
#[cfg(windows)]
fn write_skill_files_windows(
    root_dir: &cap_std::fs::Dir,
    relative_dir: &std::path::Path,
    absolute_dir: &std::path::Path,
    files: &[(String, String)],
) -> std::io::Result<()> {
    use std::io::Write;

    let skill_dir = ensure_windows_directory(root_dir, relative_dir)?;

    for (rel_path, content) in files {
        let target = resolve_skill_file_path(std::path::Path::new(""), rel_path)?;
        let parent = target.parent().ok_or_else(|| {
            std::io::Error::new(std::io::ErrorKind::InvalidInput, "path has no parent")
        })?;
        let parent_dir = ensure_windows_directory(&skill_dir, parent)?;
        let file_name = target.file_name().ok_or_else(|| {
            std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                format!("bundled skill file path has no file name: {rel_path}"),
            )
        })?;

        let mut file = create_windows_relative_object(&parent_dir, file_name, false)?;
        let absolute_path = absolute_dir.join(&target);
        if let Err(error) = restrict_windows_handle_acl(&file, false) {
            drop(file);
            let cleanup_error = parent_dir.remove_file(file_name).err();
            return Err(match cleanup_error {
                Some(cleanup_error) => std::io::Error::other(format!(
                    "{error}; additionally failed to remove unsecured file {}: {cleanup_error}",
                    absolute_path.display()
                )),
                None => error,
            });
        }
        file.write_all(content.as_bytes())?;
        file.flush()?;
    }

    Ok(())
}

/// Create and reopen every relative directory component through a retained
/// capability. Reopening with `FollowSymlinks::No` rejects junctions and other
/// reparse points before they can become the base for the next component.
#[cfg(windows)]
fn ensure_windows_directory(
    base: &cap_std::fs::Dir,
    relative: &std::path::Path,
) -> std::io::Result<cap_std::fs::Dir> {
    use std::path::Component;

    let mut current = base.try_clone()?;
    for component in relative.components() {
        let Component::Normal(name) = component else {
            if component == Component::CurDir {
                continue;
            }
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                format!(
                    "invalid capability-relative directory: {}",
                    relative.display()
                ),
            ));
        };

        let child = match create_windows_relative_object(&current, name, true) {
            Ok(file) => windows_directory_from_file(file)?,
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
                open_windows_child_directory(&current, name)?
            }
            Err(error) => return Err(error),
        };
        restrict_windows_handle_acl(&child, true)?;
        current = child;
    }
    Ok(current)
}

#[cfg(windows)]
fn open_windows_child_directory(
    base: &cap_std::fs::Dir,
    name: &std::ffi::OsStr,
) -> std::io::Result<cap_std::fs::Dir> {
    use cap_fs_ext::{FollowSymlinks, OpenOptionsFollowExt};
    use cap_std::fs::OpenOptionsExt as _;
    use windows_sys::Win32::Foundation::GENERIC_READ;
    use windows_sys::Win32::Storage::FileSystem::{
        FILE_FLAG_BACKUP_SEMANTICS, FILE_SHARE_READ, FILE_SHARE_WRITE, READ_CONTROL, WRITE_DAC,
        WRITE_OWNER,
    };

    let mut options = cap_std::fs::OpenOptions::new();
    options
        .access_mode(GENERIC_READ | READ_CONTROL | WRITE_DAC | WRITE_OWNER)
        .share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE)
        .custom_flags(FILE_FLAG_BACKUP_SEMANTICS);
    options.follow(FollowSymlinks::No);
    let file = base.open_with(name, &options)?.into_std();
    windows_directory_from_file(file)
}

#[cfg(windows)]
fn windows_directory_from_file(file: std::fs::File) -> std::io::Result<cap_std::fs::Dir> {
    use std::os::windows::fs::MetadataExt;
    use windows_sys::Win32::Storage::FileSystem::{
        FILE_ATTRIBUTE_DIRECTORY, FILE_ATTRIBUTE_REPARSE_POINT,
    };

    let attributes = file.metadata()?.file_attributes();
    if attributes & FILE_ATTRIBUTE_DIRECTORY == 0 {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "bundled skill extraction component is not a directory",
        ));
    }
    if attributes & FILE_ATTRIBUTE_REPARSE_POINT != 0 {
        return Err(std::io::Error::new(
            std::io::ErrorKind::PermissionDenied,
            "bundled skill extraction component is a reparse point",
        ));
    }
    Ok(cap_std::fs::Dir::from_std_file(file))
}

/// Validate and resolve a skill-relative path.
/// Rejects absolute paths and any path containing `..` components.
fn resolve_skill_file_path(base_dir: &std::path::Path, rel_path: &str) -> std::io::Result<PathBuf> {
    let normalized = std::path::Path::new(rel_path);

    if normalized.is_absolute() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            format!("bundled skill file path must be relative: {rel_path}"),
        ));
    }

    for component in normalized.components() {
        use std::path::Component;
        match component {
            Component::Normal(_) | Component::CurDir => {}
            Component::ParentDir => {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::InvalidInput,
                    format!("bundled skill file path escapes skill dir: {rel_path}"),
                ));
            }
            Component::Prefix(_) | Component::RootDir => {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::InvalidInput,
                    format!("bundled skill file path must be relative: {rel_path}"),
                ));
            }
        }
    }

    Ok(base_dir.join(normalized))
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
#[path = "bundled_tests.rs"]
mod tests;
