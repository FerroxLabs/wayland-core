//! Non-spawning executable readiness for an explicit child launch environment.

use std::ffi::{OsStr, OsString};
use std::fmt;
use std::io;
use std::path::{Path, PathBuf};
use std::time::Duration;

/// Effective-environment field whose contents could not be used safely.
///
/// The value itself is deliberately never retained: PATH-like variables often
/// contain usernames, private mount names, or injected secrets and must not
/// appear in readiness diagnostics.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExecutableEnvironmentVariable {
    Cwd,
    Path,
    PathExt,
}

impl fmt::Display for ExecutableEnvironmentVariable {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Cwd => "working directory",
            Self::Path => "PATH",
            Self::PathExt => "PATHEXT",
        })
    }
}

/// Closed limit whose breach stopped a readiness check before filesystem I/O.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExecutableReadinessLimit {
    PathLength,
    PathEntries,
    PathExtLength,
    PathExtEntries,
    CandidateProbes,
}

impl fmt::Display for ExecutableReadinessLimit {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::PathLength => "PATH length",
            Self::PathEntries => "PATH entry count",
            Self::PathExtLength => "PATHEXT length",
            Self::PathExtEntries => "PATHEXT entry count",
            Self::CandidateProbes => "candidate probe count",
        })
    }
}

/// Typed result of a non-spawning executable readiness check.
#[derive(Clone, PartialEq, Eq)]
pub struct ResolvedExecutable {
    path: PathBuf,
}

impl ResolvedExecutable {
    #[must_use]
    pub fn as_path(&self) -> &Path {
        &self.path
    }

    #[must_use]
    pub fn into_path(self) -> PathBuf {
        self.path
    }
}

impl fmt::Debug for ResolvedExecutable {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ResolvedExecutable")
            .field("executable", &diagnostic_executable(&self.path))
            .field("path", &"<redacted>")
            .finish()
    }
}

/// Failure from [`resolve_mcp_stdio_executable`]. Error values retain no PATH/PATHEXT
/// contents or resolved directories, so both `Display` and `Debug` are safe for
/// user-facing diagnostics.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ExecutableReadinessError {
    #[error("configured executable {executable} is empty or malformed")]
    InvalidExecutable { executable: String },
    #[error("cannot resolve executable {executable}: effective PATH is unavailable")]
    MissingEffectivePath { executable: String },
    #[error("cannot resolve executable {executable}: effective {variable} is invalid")]
    InvalidEffectiveEnvironment {
        executable: String,
        variable: ExecutableEnvironmentVariable,
    },
    #[error("cannot inspect or execute configured executable {executable}: permission denied")]
    PermissionDenied { executable: String },
    #[error("configured executable {executable} is not executable by the effective user")]
    NotExecutable { executable: String },
    #[error("cannot inspect executable {executable}: filesystem error {kind:?}")]
    Io {
        executable: String,
        kind: io::ErrorKind,
    },
    #[error("cannot inspect executable {executable}: readiness probe timed out")]
    ProbeTimedOut { executable: String },
    #[error("cannot inspect executable {executable}: readiness worker failed")]
    ProbeFailed { executable: String },
    #[error("cannot verify direct Windows executable lookup for {executable}")]
    UncheckedDirectSearch { executable: String },
    #[error("cannot inspect executable {executable}: network paths are not probed")]
    NetworkPathUnsupported { executable: String },
    #[error(
        "cannot resolve executable {executable}: effective {limit} exceeds the readiness limit"
    )]
    EnvironmentLimitExceeded {
        executable: String,
        limit: ExecutableReadinessLimit,
    },
    #[error("executable {executable} was not found in the effective launch environment")]
    NotFound { executable: String },
}

const MAX_EFFECTIVE_PATH_LENGTH: usize = 65_536;
const MAX_EFFECTIVE_PATH_ENTRIES: usize = 256;
const MAX_EFFECTIVE_PATHEXT_LENGTH: usize = 4_096;
const MAX_EFFECTIVE_PATHEXT_ENTRIES: usize = 64;
const MAX_EXECUTABLE_CANDIDATE_PROBES: usize = 1_024;
const EXECUTABLE_RESOLUTION_TIMEOUT: Duration = Duration::from_secs(1);
static EXECUTABLE_RESOLUTION_PERMIT: tokio::sync::Semaphore = tokio::sync::Semaphore::const_new(1);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ExecutablePlatform {
    Unix,
    Windows,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum McpStdioLaunchStrategy {
    Direct,
    CommandShell,
}

#[derive(Debug)]
struct WindowsDirectSearchRoots {
    current_executable_directory: PathBuf,
    system_directory: PathBuf,
    windows_directory: PathBuf,
    parent_path: Option<OsString>,
}

#[derive(Debug, Clone, Copy)]
struct WindowsResolutionContext<'a> {
    direct_roots: Option<&'a WindowsDirectSearchRoots>,
    cwd_search_suppressed: bool,
}

fn mcp_stdio_launch_strategy(
    program: &OsStr,
    platform: ExecutablePlatform,
) -> McpStdioLaunchStrategy {
    if platform == ExecutablePlatform::Windows
        && program.to_str().is_some_and(|program| {
            matches!(
                program.to_ascii_lowercase().as_str(),
                "cmd" | "cmd.exe" | "powershell" | "powershell.exe" | "pwsh" | "pwsh.exe"
            )
        })
    {
        McpStdioLaunchStrategy::Direct
    } else {
        McpStdioLaunchStrategy::CommandShell
    }
}

/// Resolve the executable selected by the MCP stdio launcher without starting
/// it.
///
/// `effective_cwd`, `effective_path`, and `effective_pathext` must describe the
/// exact child launch after any `env_clear()`, curated forwarding, and
/// `current_dir()` selection. Relative program and PATH entries are anchored to
/// `effective_cwd`. On Windows, `cmd`, PowerShell, and pwsh use the launcher's
/// direct `CreateProcess` branch and therefore resolve only executable images;
/// other commands use `cmd /C` and effective PATHEXT. The command-shell branch
/// derives current-directory suppression from the effective child environment;
/// callers cannot select a contradictory policy. The direct Windows branch
/// mirrors Rust's additional ambient lookup locations but never exposes those
/// paths in diagnostics. If an ambient location cannot be discovered exactly,
/// the resolver returns [`ExecutableReadinessError::UncheckedDirectSearch`]
/// instead of a false `NotFound` result. Inputs and candidate counts are bounded
/// before metadata I/O, and all metadata work runs off-thread behind a total
/// timeout so a network/autofs path cannot hang the diagnostics session.
///
/// Unix permission checks model the effective user's mode-bit access. They are
/// advisory because every filesystem result is TOCTOU-sensitive and mount
/// policy, ACLs, or a later path replacement can still reject or redirect the
/// spawn. Readiness is never launch authority. The resolver never executes the
/// target.
pub async fn resolve_mcp_stdio_executable(
    program: &OsStr,
    effective_cwd: &Path,
    effective_path: Option<&OsStr>,
    effective_pathext: Option<&OsStr>,
    effective_environment: &[(OsString, OsString)],
) -> Result<ResolvedExecutable, ExecutableReadinessError> {
    let program = program.to_os_string();
    let effective_cwd = effective_cwd.to_path_buf();
    let effective_path = effective_path.map(OsStr::to_os_string);
    let effective_pathext = effective_pathext.map(OsStr::to_os_string);
    let windows_cwd_search_suppressed = windows_cwd_search_suppressed(effective_environment);
    let executable = diagnostic_executable(Path::new(&program));
    run_bounded_resolution(EXECUTABLE_RESOLUTION_TIMEOUT, executable, move || {
        let platform = if cfg!(windows) {
            ExecutablePlatform::Windows
        } else {
            ExecutablePlatform::Unix
        };
        let strategy = mcp_stdio_launch_strategy(&program, platform);
        let windows_direct_roots = if platform == ExecutablePlatform::Windows
            && strategy == McpStdioLaunchStrategy::Direct
        {
            windows_direct_search_roots().ok()
        } else {
            None
        };
        resolve_executable_for(
            &program,
            &effective_cwd,
            effective_path.as_deref(),
            effective_pathext.as_deref(),
            platform,
            strategy,
            WindowsResolutionContext {
                direct_roots: windows_direct_roots.as_ref(),
                cwd_search_suppressed: windows_cwd_search_suppressed,
            },
        )
    })
    .await
}

async fn run_bounded_resolution<F>(
    timeout: Duration,
    executable: String,
    resolution: F,
) -> Result<ResolvedExecutable, ExecutableReadinessError>
where
    F: FnOnce() -> Result<ResolvedExecutable, ExecutableReadinessError> + Send + 'static,
{
    let deadline = tokio::time::Instant::now() + timeout;
    let permit =
        match tokio::time::timeout_at(deadline, EXECUTABLE_RESOLUTION_PERMIT.acquire()).await {
            Ok(Ok(permit)) => permit,
            Ok(Err(_)) => return Err(ExecutableReadinessError::ProbeFailed { executable }),
            Err(_) => return Err(ExecutableReadinessError::ProbeTimedOut { executable }),
        };
    let task = tokio::task::spawn_blocking(move || {
        let _permit = permit;
        resolution()
    });
    match tokio::time::timeout_at(deadline, task).await {
        Ok(Ok(result)) => result,
        Ok(Err(_)) => Err(ExecutableReadinessError::ProbeFailed { executable }),
        Err(_) => Err(ExecutableReadinessError::ProbeTimedOut { executable }),
    }
}

fn resolve_executable_for(
    program: &OsStr,
    effective_cwd: &Path,
    effective_path: Option<&OsStr>,
    effective_pathext: Option<&OsStr>,
    platform: ExecutablePlatform,
    strategy: McpStdioLaunchStrategy,
    windows: WindowsResolutionContext<'_>,
) -> Result<ResolvedExecutable, ExecutableReadinessError> {
    let executable = diagnostic_executable(Path::new(program));
    if program.is_empty() || program.to_string_lossy().contains('\0') {
        return Err(ExecutableReadinessError::InvalidExecutable { executable });
    }
    if !effective_cwd.is_absolute()
        || effective_cwd.as_os_str().is_empty()
        || effective_cwd.as_os_str().to_string_lossy().contains('\0')
    {
        return Err(ExecutableReadinessError::InvalidEffectiveEnvironment {
            executable,
            variable: ExecutableEnvironmentVariable::Cwd,
        });
    }
    if platform == ExecutablePlatform::Windows && is_windows_drive_relative(program) {
        return Err(ExecutableReadinessError::InvalidExecutable { executable });
    }
    if is_network_path(effective_cwd, platform) || is_network_path(Path::new(program), platform) {
        return Err(ExecutableReadinessError::NetworkPathUnsupported { executable });
    }
    inspect_effective_cwd(effective_cwd, &executable)?;

    let program_path = Path::new(program);
    let explicit_path = program_path.is_absolute() || program_path.components().count() > 1;

    if explicit_path {
        let extensions = executable_extensions(
            program_path,
            effective_pathext,
            platform,
            strategy,
            &executable,
        )?;
        let base = if program_path.is_absolute() {
            program_path.to_path_buf()
        } else {
            effective_cwd.join(program_path)
        };
        enforce_probe_limit(1, &extensions, &executable)?;
        return resolve_candidates([base], &extensions, platform, &executable);
    }

    if platform == ExecutablePlatform::Windows && strategy == McpStdioLaunchStrategy::Direct {
        return resolve_windows_direct_program(
            program_path,
            effective_path,
            windows.direct_roots,
            &executable,
        );
    }

    let (mut search_dirs, path_missing) =
        if let Some(path) = effective_path.filter(|value| !value.is_empty()) {
            validate_environment_length(
                path,
                MAX_EFFECTIVE_PATH_LENGTH,
                ExecutableReadinessLimit::PathLength,
                &executable,
            )?;
            if path.to_string_lossy().contains('\0') {
                return Err(ExecutableReadinessError::InvalidEffectiveEnvironment {
                    executable,
                    variable: ExecutableEnvironmentVariable::Path,
                });
            }
            let entries = split_effective_path(path, platform, &executable)?;
            let missing = entries.is_empty();
            (entries, missing)
        } else {
            (Vec::new(), true)
        };
    if path_missing && platform == ExecutablePlatform::Unix {
        return Err(ExecutableReadinessError::MissingEffectivePath { executable });
    }
    for directory in &mut search_dirs {
        if is_network_path(directory, platform) {
            return Err(ExecutableReadinessError::NetworkPathUnsupported { executable });
        }
        if !directory.is_absolute() {
            *directory = effective_cwd.join(&*directory);
        }
    }
    if platform == ExecutablePlatform::Windows && !windows.cwd_search_suppressed {
        search_dirs.insert(0, effective_cwd.to_path_buf());
    }
    let extensions = executable_extensions(
        program_path,
        effective_pathext,
        platform,
        strategy,
        &executable,
    )?;
    enforce_probe_limit(search_dirs.len(), &extensions, &executable)?;
    let result = resolve_candidates(
        search_dirs
            .into_iter()
            .map(|directory| directory.join(program_path)),
        &extensions,
        platform,
        &executable,
    );
    if path_missing && matches!(result, Err(ExecutableReadinessError::NotFound { .. })) {
        Err(ExecutableReadinessError::MissingEffectivePath { executable })
    } else {
        result
    }
}

fn windows_cwd_search_suppressed(effective_environment: &[(OsString, OsString)]) -> bool {
    effective_environment.iter().any(|(name, _)| {
        name.to_str()
            .is_some_and(|name| name.eq_ignore_ascii_case("NoDefaultCurrentDirectoryInExePath"))
    })
}

fn resolve_windows_direct_program(
    program: &Path,
    effective_path: Option<&OsStr>,
    roots: Option<&WindowsDirectSearchRoots>,
    executable: &str,
) -> Result<ResolvedExecutable, ExecutableReadinessError> {
    let Some(roots) = roots else {
        return Err(ExecutableReadinessError::UncheckedDirectSearch {
            executable: executable.to_string(),
        });
    };
    let extensions = executable_extensions(
        program,
        None,
        ExecutablePlatform::Windows,
        McpStdioLaunchStrategy::Direct,
        executable,
    )?;
    let mut search_dirs = Vec::new();

    append_direct_path_entries(&mut search_dirs, effective_path, executable)?;
    search_dirs.push(roots.current_executable_directory.clone());
    search_dirs.push(roots.system_directory.clone());
    search_dirs.push(roots.windows_directory.clone());
    append_direct_path_entries(&mut search_dirs, roots.parent_path.as_deref(), executable)?;

    for directory in &search_dirs {
        if !directory.is_absolute() || is_network_path(directory, ExecutablePlatform::Windows) {
            return Err(ExecutableReadinessError::UncheckedDirectSearch {
                executable: executable.to_string(),
            });
        }
    }
    enforce_probe_limit(search_dirs.len(), &extensions, executable)?;
    resolve_candidates(
        search_dirs
            .into_iter()
            .map(|directory| directory.join(program)),
        &extensions,
        ExecutablePlatform::Windows,
        executable,
    )
}

fn append_direct_path_entries(
    search_dirs: &mut Vec<PathBuf>,
    path: Option<&OsStr>,
    executable: &str,
) -> Result<(), ExecutableReadinessError> {
    let Some(path) = path.filter(|path| !path.is_empty()) else {
        return Ok(());
    };
    validate_environment_length(
        path,
        MAX_EFFECTIVE_PATH_LENGTH,
        ExecutableReadinessLimit::PathLength,
        executable,
    )?;
    if path.to_string_lossy().contains('\0') {
        return Err(ExecutableReadinessError::InvalidEffectiveEnvironment {
            executable: executable.to_string(),
            variable: ExecutableEnvironmentVariable::Path,
        });
    }
    search_dirs.extend(split_effective_path(
        path,
        ExecutablePlatform::Windows,
        executable,
    )?);
    Ok(())
}

#[cfg(windows)]
fn windows_direct_search_roots() -> io::Result<WindowsDirectSearchRoots> {
    let current_executable_directory = std::env::current_exe()?
        .parent()
        .map(Path::to_path_buf)
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "missing executable parent"))?;
    Ok(WindowsDirectSearchRoots {
        current_executable_directory,
        system_directory: windows_known_directory(WindowsKnownDirectory::System)?,
        windows_directory: windows_known_directory(WindowsKnownDirectory::Windows)?,
        parent_path: std::env::var_os("PATH"),
    })
}

#[cfg(not(windows))]
fn windows_direct_search_roots() -> io::Result<WindowsDirectSearchRoots> {
    Err(io::Error::new(
        io::ErrorKind::Unsupported,
        "Windows direct search is unavailable",
    ))
}

#[cfg(windows)]
#[derive(Debug, Clone, Copy)]
enum WindowsKnownDirectory {
    System,
    Windows,
}

#[cfg(windows)]
fn windows_known_directory(directory: WindowsKnownDirectory) -> io::Result<PathBuf> {
    use std::os::windows::ffi::OsStringExt;

    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn GetSystemDirectoryW(buffer: *mut u16, size: u32) -> u32;
        fn GetWindowsDirectoryW(buffer: *mut u16, size: u32) -> u32;
    }

    let mut buffer = vec![0_u16; 32_768];
    // SAFETY: both APIs receive an initialized writable buffer and its exact
    // element count. The returned length is checked before slicing.
    let written = unsafe {
        match directory {
            WindowsKnownDirectory::System => {
                GetSystemDirectoryW(buffer.as_mut_ptr(), buffer.len() as u32)
            }
            WindowsKnownDirectory::Windows => {
                GetWindowsDirectoryW(buffer.as_mut_ptr(), buffer.len() as u32)
            }
        }
    };
    if written == 0 {
        return Err(io::Error::last_os_error());
    }
    let written = written as usize;
    if written >= buffer.len() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "Windows directory path exceeds the readiness buffer",
        ));
    }
    buffer.truncate(written);
    Ok(PathBuf::from(OsString::from_wide(&buffer)))
}

fn is_windows_drive_relative(program: &OsStr) -> bool {
    let Some(program) = program.to_str() else {
        return false;
    };
    let bytes = program.as_bytes();
    bytes.len() >= 2
        && bytes[0].is_ascii_alphabetic()
        && bytes[1] == b':'
        && bytes
            .get(2)
            .is_none_or(|separator| !matches!(*separator, b'/' | b'\\'))
}

fn is_network_path(path: &Path, platform: ExecutablePlatform) -> bool {
    if platform != ExecutablePlatform::Windows {
        return false;
    }
    path.to_str()
        .is_some_and(|path| path.starts_with(r"\\") || path.starts_with("//"))
}

fn inspect_effective_cwd(
    effective_cwd: &Path,
    executable: &str,
) -> Result<(), ExecutableReadinessError> {
    let metadata = effective_cwd
        .metadata()
        .map_err(|error| match error.kind() {
            io::ErrorKind::NotFound | io::ErrorKind::NotADirectory => {
                ExecutableReadinessError::InvalidEffectiveEnvironment {
                    executable: executable.to_string(),
                    variable: ExecutableEnvironmentVariable::Cwd,
                }
            }
            io::ErrorKind::PermissionDenied => ExecutableReadinessError::PermissionDenied {
                executable: executable.to_string(),
            },
            kind => ExecutableReadinessError::Io {
                executable: executable.to_string(),
                kind,
            },
        })?;
    if !metadata.is_dir() {
        return Err(ExecutableReadinessError::InvalidEffectiveEnvironment {
            executable: executable.to_string(),
            variable: ExecutableEnvironmentVariable::Cwd,
        });
    }
    #[cfg(unix)]
    if unix_effective_execute_permission(&metadata).is_err() {
        return Err(ExecutableReadinessError::PermissionDenied {
            executable: executable.to_string(),
        });
    }
    Ok(())
}

fn executable_extensions(
    program: &Path,
    effective_pathext: Option<&OsStr>,
    platform: ExecutablePlatform,
    strategy: McpStdioLaunchStrategy,
    executable: &str,
) -> Result<Vec<String>, ExecutableReadinessError> {
    if platform == ExecutablePlatform::Unix {
        return Ok(Vec::new());
    }
    if strategy == McpStdioLaunchStrategy::Direct {
        return match program.extension() {
            Some(extension) if extension.eq_ignore_ascii_case("exe") => Ok(Vec::new()),
            Some(_) => Err(ExecutableReadinessError::NotExecutable {
                executable: executable.to_string(),
            }),
            None => Ok(vec![".exe".to_string()]),
        };
    }
    let Some(raw) = effective_pathext.filter(|value| !value.is_empty()) else {
        return Err(ExecutableReadinessError::InvalidEffectiveEnvironment {
            executable: executable.to_string(),
            variable: ExecutableEnvironmentVariable::PathExt,
        });
    };
    validate_environment_length(
        raw,
        MAX_EFFECTIVE_PATHEXT_LENGTH,
        ExecutableReadinessLimit::PathExtLength,
        executable,
    )?;
    let Some(value) = raw.to_str().filter(|value| !value.contains('\0')) else {
        return Err(ExecutableReadinessError::InvalidEffectiveEnvironment {
            executable: executable.to_string(),
            variable: ExecutableEnvironmentVariable::PathExt,
        });
    };

    let mut extensions = Vec::new();
    for extension in value
        .split(';')
        .map(str::trim)
        .filter(|part| !part.is_empty())
    {
        if extension.len() <= 1
            || !extension.starts_with('.')
            || extension.chars().any(|ch| matches!(ch, '/' | '\\'))
        {
            return Err(ExecutableReadinessError::InvalidEffectiveEnvironment {
                executable: executable.to_string(),
                variable: ExecutableEnvironmentVariable::PathExt,
            });
        }
        if !extensions
            .iter()
            .any(|known: &String| known.eq_ignore_ascii_case(extension))
        {
            extensions.push(extension.to_string());
        }
        if extensions.len() > MAX_EFFECTIVE_PATHEXT_ENTRIES {
            return Err(ExecutableReadinessError::EnvironmentLimitExceeded {
                executable: executable.to_string(),
                limit: ExecutableReadinessLimit::PathExtEntries,
            });
        }
    }
    if extensions.is_empty() {
        return Err(ExecutableReadinessError::InvalidEffectiveEnvironment {
            executable: executable.to_string(),
            variable: ExecutableEnvironmentVariable::PathExt,
        });
    }

    if let Some(extension) = program.extension() {
        let mut explicit = String::from(".");
        explicit.push_str(&extension.to_string_lossy());
        if !extensions
            .iter()
            .any(|allowed| allowed.eq_ignore_ascii_case(&explicit))
        {
            return Err(ExecutableReadinessError::NotExecutable {
                executable: executable.to_string(),
            });
        }
        return Ok(Vec::new());
    }
    Ok(extensions)
}

fn validate_environment_length(
    value: &OsStr,
    maximum: usize,
    limit: ExecutableReadinessLimit,
    executable: &str,
) -> Result<(), ExecutableReadinessError> {
    if os_str_storage_len(value) > maximum {
        Err(ExecutableReadinessError::EnvironmentLimitExceeded {
            executable: executable.to_string(),
            limit,
        })
    } else {
        Ok(())
    }
}

#[cfg(unix)]
fn os_str_storage_len(value: &OsStr) -> usize {
    use std::os::unix::ffi::OsStrExt;
    value.as_bytes().len()
}

#[cfg(windows)]
fn os_str_storage_len(value: &OsStr) -> usize {
    use std::os::windows::ffi::OsStrExt;
    value.encode_wide().count()
}

fn split_effective_path(
    value: &OsStr,
    platform: ExecutablePlatform,
    executable: &str,
) -> Result<Vec<PathBuf>, ExecutableReadinessError> {
    let entries = if platform == ExecutablePlatform::Windows {
        #[cfg(windows)]
        {
            std::env::split_paths(value).collect::<Vec<_>>()
        }
        #[cfg(not(windows))]
        {
            value
                .to_str()
                .ok_or_else(|| ExecutableReadinessError::InvalidEffectiveEnvironment {
                    executable: executable.to_string(),
                    variable: ExecutableEnvironmentVariable::Path,
                })?
                .split(';')
                .filter(|entry| !entry.is_empty())
                .map(|entry| PathBuf::from(entry.trim_matches('"')))
                .collect::<Vec<_>>()
        }
    } else {
        std::env::split_paths(value).collect::<Vec<_>>()
    };
    if entries.len() > MAX_EFFECTIVE_PATH_ENTRIES {
        return Err(ExecutableReadinessError::EnvironmentLimitExceeded {
            executable: executable.to_string(),
            limit: ExecutableReadinessLimit::PathEntries,
        });
    }
    Ok(entries)
}

fn enforce_probe_limit(
    base_count: usize,
    extensions: &[String],
    executable: &str,
) -> Result<(), ExecutableReadinessError> {
    let variants = extensions.len().max(1);
    if base_count
        .checked_mul(variants)
        .is_none_or(|count| count > MAX_EXECUTABLE_CANDIDATE_PROBES)
    {
        Err(ExecutableReadinessError::EnvironmentLimitExceeded {
            executable: executable.to_string(),
            limit: ExecutableReadinessLimit::CandidateProbes,
        })
    } else {
        Ok(())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CandidateFailure {
    Missing,
    NotExecutable,
    PermissionDenied,
    Io(io::ErrorKind),
}

fn resolve_candidates(
    bases: impl IntoIterator<Item = PathBuf>,
    extensions: &[String],
    platform: ExecutablePlatform,
    executable: &str,
) -> Result<ResolvedExecutable, ExecutableReadinessError> {
    let mut aggregate = CandidateFailure::Missing;
    for base in bases {
        if extensions.is_empty() {
            match inspect_candidate(&base, platform) {
                Ok(()) => return Ok(ResolvedExecutable { path: base }),
                Err(failure) => aggregate = stronger_failure(aggregate, failure),
            }
            continue;
        }
        for extension in extensions {
            let mut candidate = base.as_os_str().to_os_string();
            candidate.push(extension);
            let candidate = PathBuf::from(candidate);
            match inspect_candidate(&candidate, platform) {
                Ok(()) => return Ok(ResolvedExecutable { path: candidate }),
                Err(failure) => aggregate = stronger_failure(aggregate, failure),
            }
        }
    }
    Err(candidate_error(aggregate, executable))
}

fn stronger_failure(left: CandidateFailure, right: CandidateFailure) -> CandidateFailure {
    fn rank(failure: CandidateFailure) -> u8 {
        match failure {
            CandidateFailure::Missing => 0,
            CandidateFailure::NotExecutable => 1,
            CandidateFailure::PermissionDenied => 2,
            CandidateFailure::Io(_) => 3,
        }
    }
    if rank(right) > rank(left) {
        right
    } else {
        left
    }
}

fn candidate_error(failure: CandidateFailure, executable: &str) -> ExecutableReadinessError {
    match failure {
        CandidateFailure::Missing => ExecutableReadinessError::NotFound {
            executable: executable.to_string(),
        },
        CandidateFailure::NotExecutable => ExecutableReadinessError::NotExecutable {
            executable: executable.to_string(),
        },
        CandidateFailure::PermissionDenied => ExecutableReadinessError::PermissionDenied {
            executable: executable.to_string(),
        },
        CandidateFailure::Io(kind) => ExecutableReadinessError::Io {
            executable: executable.to_string(),
            kind,
        },
    }
}

fn inspect_candidate(path: &Path, platform: ExecutablePlatform) -> Result<(), CandidateFailure> {
    let metadata = path.metadata().map_err(|error| match error.kind() {
        io::ErrorKind::NotFound => CandidateFailure::Missing,
        io::ErrorKind::PermissionDenied => CandidateFailure::PermissionDenied,
        kind => CandidateFailure::Io(kind),
    })?;
    if !metadata.is_file() {
        return Err(CandidateFailure::NotExecutable);
    }
    if platform == ExecutablePlatform::Windows {
        return Ok(());
    }
    unix_effective_execute_permission(&metadata)
}

#[cfg(unix)]
fn unix_effective_execute_permission(metadata: &std::fs::Metadata) -> Result<(), CandidateFailure> {
    use std::os::raw::{c_int, c_uint};
    use std::os::unix::fs::MetadataExt;

    unsafe extern "C" {
        fn geteuid() -> c_uint;
        fn getegid() -> c_uint;
        fn getgroups(size: c_int, groups: *mut c_uint) -> c_int;
    }

    let mode = metadata.mode();
    if mode & 0o111 == 0 {
        return Err(CandidateFailure::NotExecutable);
    }
    // SAFETY: these libc identity functions take no pointers and have no
    // preconditions. They are queried only to interpret already-read mode bits.
    let effective_uid = unsafe { geteuid() };
    // SAFETY: same no-argument libc contract as `geteuid`.
    let effective_gid = unsafe { getegid() };
    if effective_uid == 0 {
        return Ok(());
    }
    // SAFETY: a null list with size zero is the specified sizing query.
    let count = unsafe { getgroups(0, std::ptr::null_mut()) };
    if count < 0 {
        return Err(CandidateFailure::Io(io::Error::last_os_error().kind()));
    }
    let mut groups = vec![0; count as usize];
    // SAFETY: `groups` has exactly the capacity reported by the sizing call and
    // remains exclusively borrowed for the duration of the call.
    let actual = unsafe { getgroups(count, groups.as_mut_ptr()) };
    if actual < 0 {
        return Err(CandidateFailure::Io(io::Error::last_os_error().kind()));
    }
    groups.truncate(actual as usize);
    unix_mode_execute_permission(
        mode,
        metadata.uid(),
        metadata.gid(),
        effective_uid,
        effective_gid,
        &groups,
    )
}

#[cfg(unix)]
fn unix_mode_execute_permission(
    mode: u32,
    file_uid: u32,
    file_gid: u32,
    effective_uid: u32,
    effective_gid: u32,
    supplementary_groups: &[u32],
) -> Result<(), CandidateFailure> {
    if mode & 0o111 == 0 {
        return Err(CandidateFailure::NotExecutable);
    }
    if effective_uid == 0 {
        return Ok(());
    }
    let permitted = if effective_uid == file_uid {
        mode & 0o100 != 0
    } else if effective_gid == file_gid || supplementary_groups.contains(&file_gid) {
        mode & 0o010 != 0
    } else {
        mode & 0o001 != 0
    };
    permitted
        .then_some(())
        .ok_or(CandidateFailure::PermissionDenied)
}

#[cfg(not(unix))]
fn unix_effective_execute_permission(
    _metadata: &std::fs::Metadata,
) -> Result<(), CandidateFailure> {
    Ok(())
}

fn diagnostic_executable(program: &Path) -> String {
    let candidate = program.file_name().unwrap_or(program.as_os_str());
    let candidate = candidate.to_string_lossy();
    let normalized = candidate.to_ascii_lowercase();
    let stem = [".exe", ".cmd", ".bat"]
        .into_iter()
        .find_map(|suffix| normalized.strip_suffix(suffix))
        .unwrap_or(&normalized);
    if matches!(
        stem,
        "bun"
            | "cmd"
            | "deno"
            | "docker"
            | "node"
            | "npx"
            | "powershell"
            | "pwsh"
            | "python"
            | "python3"
            | "uvx"
    ) {
        candidate.into_owned()
    } else {
        "<configured executable>".to_string()
    }
}

#[cfg(test)]
#[path = "executable_readiness_tests.rs"]
mod tests;
