//! Explicit resolution of a real `bash.exe` on Windows.
//!
//! The Bash tool is named after bash, but on Windows the interpreter has been
//! `cmd.exe`, and bash syntax there fails SILENTLY: `echo A; echo B` prints
//! `A; echo B` and exits 0 (FerroxLabs/wayland#1151). When the host actually
//! has a real bash, using it is the fix (#1164).
//!
//! Two rules shape how that bash is found, and both exist because the naive
//! implementation is dangerous:
//!
//! 1. **Known install location, never a bare `PATH` lookup.** On a box with
//!    Git for Windows installed, `where bash` happens to find
//!    `…\Git\bin\bash.exe` first. On a box without it, the first (and only)
//!    `bash.exe` on a default `PATH` is `%SystemRoot%\System32\bash.exe` — the
//!    WSL launcher, which runs the command inside a Linux distribution against
//!    a different filesystem (`/mnt/c/...`). PATH order is a property of the
//!    operator's machine, so it can never be the resolution rule.
//! 2. **Refuse the known-bad candidates explicitly.** `System32\bash.exe`
//!    (and its `Sysnative` / `SysWOW64` spellings) is the WSL launcher, and
//!    `…\AppData\Local\Microsoft\WindowsApps\bash.exe` is a Microsoft Store
//!    app-execution alias — a zero-byte reparse stub that either launches the
//!    Store or, again, WSL.
//!
//! The decision is split so it can be graded from any host: candidate
//! generation ([`windows_bash_candidates`]) and selection
//! ([`select_windows_bash`]) are pure functions over injected data, and the
//! only impure part ([`resolve_windows_bash`]) is the environment read plus the
//! existence probe. Every rule above is therefore unit-tested on Linux and
//! macOS as well as Windows.

/// Why a candidate path may not serve as the Bash tool's interpreter.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BashRefusal {
    /// `%SystemRoot%\System32\bash.exe` (or the `Sysnative` / `SysWOW64`
    /// spellings of the same directory): the WSL launcher. It is a real
    /// executable that succeeds, so it cannot be caught by an existence probe
    /// — it just runs the command in a Linux distribution against `/mnt/c`
    /// paths instead of on this filesystem.
    WslLauncher,
    /// A Microsoft Store app-execution alias under
    /// `…\AppData\Local\Microsoft\WindowsApps`. A zero-byte reparse point that
    /// opens the Store (or WSL) rather than an interpreter.
    WindowsAppsShim,
    /// The final path component does not name `bash`.
    NotBash,
    /// Not a local, absolute, drive-letter path. Bare names and relative paths
    /// would resolve against `PATH` or the working directory — the working
    /// directory being somewhere an agent can drop a `bash.exe` of its own —
    /// and UNC / device paths trigger SMB image loads in this process's
    /// security context.
    NotLocalAbsolutePath,
    /// The probe found no file at this path.
    NotPresent,
}

impl BashRefusal {
    /// Short operator-facing reason, for logs and the resolution report.
    pub fn reason(self) -> &'static str {
        match self {
            Self::WslLauncher => "System32 bash.exe is the WSL launcher, not a Windows bash",
            Self::WindowsAppsShim => "WindowsApps bash.exe is a Microsoft Store alias stub",
            Self::NotBash => "path does not name bash",
            Self::NotLocalAbsolutePath => "not a local absolute path",
            Self::NotPresent => "not present",
        }
    }
}

/// A candidate path paired with the caller's existence probe result.
///
/// Presence is injected rather than probed here so [`select_windows_bash`]
/// stays pure and every branch is reachable from a test on any host.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BashCandidate {
    pub path: String,
    pub present: bool,
}

impl BashCandidate {
    pub fn new(path: impl Into<String>, present: bool) -> Self {
        Self {
            path: path.into(),
            present,
        }
    }
}

/// The outcome of [`select_windows_bash`]: the pick, and every candidate that
/// was passed over with the reason. The refusal list is what makes a `None`
/// selection explainable rather than a silent fallback to `cmd`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct WindowsBashSelection {
    pub selected: Option<String>,
    pub refused: Vec<(String, BashRefusal)>,
}

impl WindowsBashSelection {
    /// True when a candidate was refused for the named reason.
    pub fn refused_for(&self, refusal: BashRefusal) -> bool {
        self.refused.iter().any(|(_, r)| *r == refusal)
    }
}

/// Environment inputs to [`windows_bash_candidates`], injected so the
/// candidate list is a pure function of them.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct WindowsBashEnv {
    /// `[tools] windows_shell` / `WAYLAND_BASH_SHELL` when the operator named
    /// bash by PATH rather than by the bare word. Tried first — an explicit
    /// operator path outranks discovery — but it is still put through the same
    /// refusals, so pointing the setting at `System32\bash.exe` does not buy a
    /// way around rule 2.
    pub explicit: Option<String>,
    /// `%ProgramFiles%`.
    pub program_files: Option<String>,
    /// `%ProgramW6432%` — the 64-bit Program Files as seen from a 32-bit
    /// process, where `%ProgramFiles%` is redirected to the x86 tree.
    pub program_w6432: Option<String>,
    /// `%ProgramFiles(x86)%`.
    pub program_files_x86: Option<String>,
    /// `%LOCALAPPDATA%` — Git for Windows' per-user ("only for me") install
    /// lands in `…\Programs\Git`, which needs no administrator and is common
    /// on locked-down machines.
    pub local_app_data: Option<String>,
}

/// Split a Windows path into its components on BOTH separator characters,
/// regardless of the host we are compiled for.
///
/// `std::path` on Unix does not treat `\` as a separator, so it cannot be used
/// to reason about Windows paths from a Linux test host — and these rules must
/// be gradable there (see the module docs).
fn windows_components(path: &str) -> Vec<&str> {
    path.split(['/', '\\']).filter(|s| !s.is_empty()).collect()
}

/// True for a local drive-letter absolute path (`C:\…`, `c:/…`). UNC
/// (`\\server\share`), device (`\\.\`, `\\?\`), rooted-but-driveless (`\foo`)
/// and relative paths are all false.
fn is_local_absolute_windows_path(path: &str) -> bool {
    if path.starts_with("\\\\") || path.starts_with("//") {
        return false;
    }
    let mut chars = path.chars();
    let (Some(drive), Some(colon), Some(sep)) = (chars.next(), chars.next(), chars.next()) else {
        return false;
    };
    drive.is_ascii_alphabetic() && colon == ':' && (sep == '\\' || sep == '/')
}

/// Whether `path` may be used as the Bash tool's interpreter, and if not, why.
///
/// PURE and host-independent: it inspects the spelling of the path only, never
/// the filesystem. Presence is a separate question, answered by the probe the
/// caller injects into [`BashCandidate`].
pub fn windows_bash_path_refusal(path: &str) -> Option<BashRefusal> {
    let path = path.trim();
    let components = windows_components(path);
    let Some(file) = components.last() else {
        return Some(BashRefusal::NotBash);
    };
    let file = file.to_ascii_lowercase();
    if file.strip_suffix(".exe").unwrap_or(&file) != "bash" {
        return Some(BashRefusal::NotBash);
    }
    // The two named hazards are checked BEFORE the generic shape rule so an
    // explicit `System32\bash.exe` reports what it actually is.
    if components
        .iter()
        .any(|c| c.eq_ignore_ascii_case("WindowsApps"))
    {
        return Some(BashRefusal::WindowsAppsShim);
    }
    let parent = components
        .len()
        .checked_sub(2)
        .and_then(|i| components.get(i))
        .copied()
        .unwrap_or("");
    if ["system32", "sysnative", "syswow64"]
        .iter()
        .any(|d| parent.eq_ignore_ascii_case(d))
    {
        return Some(BashRefusal::WslLauncher);
    }
    if !is_local_absolute_windows_path(path) {
        return Some(BashRefusal::NotLocalAbsolutePath);
    }
    None
}

/// Join a Windows install root to a relative tail with a single `\`.
fn join_windows(root: &str, tail: &str) -> String {
    format!("{}\\{}", root.trim_end_matches(['\\', '/']), tail)
}

/// The ordered candidate list: the KNOWN install locations of a real bash on
/// Windows, most-preferred first. PURE — it never touches the filesystem.
///
/// Within a Git for Windows root, `bin\bash.exe` comes before
/// `usr\bin\bash.exe`: the former is the launcher Git installs for interactive
/// use and sets up the MSYS environment, the latter is the raw MSYS binary.
///
/// An explicit operator path is included only when it actually looks like a
/// path — a bare `windows_shell = "bash"` means "find one for me", not "run
/// the file named `bash` in the working directory".
pub fn windows_bash_candidates(env: &WindowsBashEnv) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    if let Some(explicit) = env
        .explicit
        .as_deref()
        .map(str::trim)
        .filter(|e| e.contains(['\\', '/']))
    {
        out.push(explicit.to_string());
    }
    let git_roots = [
        env.program_files.as_deref(),
        env.program_w6432.as_deref(),
        env.program_files_x86.as_deref(),
    ]
    .into_iter()
    .flatten()
    .map(|p| join_windows(p, "Git"))
    .chain(
        env.local_app_data
            .as_deref()
            .map(|p| join_windows(p, "Programs\\Git")),
    );
    for root in git_roots {
        out.push(join_windows(&root, "bin\\bash.exe"));
        out.push(join_windows(&root, "usr\\bin\\bash.exe"));
    }
    // %ProgramFiles% and %ProgramW6432% name the same directory in a 64-bit
    // process, so the list would otherwise probe every path twice.
    let mut seen: Vec<String> = Vec::with_capacity(out.len());
    out.retain(|p| {
        let key = p.to_ascii_lowercase();
        if seen.contains(&key) {
            return false;
        }
        seen.push(key);
        true
    });
    out
}

/// Pick the first candidate that is neither refused by spelling nor absent.
///
/// PURE. This is the whole selection rule, and it is the function the #1164
/// criteria are graded against: it takes an injected candidate list, so every
/// branch — including the `System32` and WindowsApps refusals, which need
/// neither a Windows host nor those files to exist — is reachable from a unit
/// test on any platform.
pub fn select_windows_bash(candidates: &[BashCandidate]) -> WindowsBashSelection {
    let mut selection = WindowsBashSelection::default();
    for candidate in candidates {
        if let Some(refusal) = windows_bash_path_refusal(&candidate.path) {
            selection.refused.push((candidate.path.clone(), refusal));
            continue;
        }
        if !candidate.present {
            selection
                .refused
                .push((candidate.path.clone(), BashRefusal::NotPresent));
            continue;
        }
        selection.selected = Some(candidate.path.clone());
        break;
    }
    selection
}

/// Read the environment, probe the known install locations, and select.
///
/// The only impure step. `explicit` is the configured
/// `WAYLAND_BASH_SHELL` / `[tools] windows_shell` value, forwarded so an
/// operator-supplied bash path is tried first and still refused if it names
/// one of the known-bad stubs.
pub fn resolve_windows_bash(explicit: Option<&str>) -> WindowsBashSelection {
    let env = WindowsBashEnv {
        explicit: explicit.map(str::to_string),
        program_files: std::env::var("ProgramFiles").ok(),
        program_w6432: std::env::var("ProgramW6432").ok(),
        program_files_x86: std::env::var("ProgramFiles(x86)").ok(),
        local_app_data: std::env::var("LOCALAPPDATA").ok(),
    };
    let candidates: Vec<BashCandidate> = windows_bash_candidates(&env)
        .into_iter()
        .map(|path| {
            let present = std::path::Path::new(&path).is_file();
            BashCandidate { path, present }
        })
        .collect();
    select_windows_bash(&candidates)
}

#[cfg(test)]
#[path = "windows_bash_tests.rs"]
mod tests;
