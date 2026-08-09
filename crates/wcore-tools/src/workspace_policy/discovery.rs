//! Developer-capability discovery helpers (F20-03 Task 2 split).

use super::*;

/// Minimal read/exec toolchain dirs for a contained shell to run compilers.
pub(super) fn minimal_toolchain_read_dirs() -> Vec<PathBuf> {
    let mut v = Vec::new();
    if let Some(home) = dirs::home_dir() {
        for sub in [".rustup", ".cargo/bin"] {
            let p = home.join(sub);
            if p.exists() {
                v.push(p);
            }
        }
    }
    v
}

pub(super) fn detect_developer_capabilities() -> Vec<DeveloperCapability> {
    let mut capabilities = Vec::new();
    for name in [
        "git",
        "cargo",
        "rustc",
        "node",
        "npm",
        "xcodebuild",
        "clang",
        "cmake",
        "make",
        "brew",
        "port",
    ] {
        let Some(executable) = resolve_path_executable(name) else {
            continue;
        };
        let mut roots = capability_roots(&executable);
        roots.sort();
        roots.dedup();
        capabilities.push(DeveloperCapability {
            name: name.to_string(),
            executable: executable.to_string_lossy().into_owned(),
            read_only_roots: roots
                .into_iter()
                .map(|path| path.to_string_lossy().into_owned())
                .collect(),
        });
    }

    for (name, variable) in [
        ("custom_sdk", "SDKROOT"),
        ("developer_dir", "DEVELOPER_DIR"),
    ] {
        let Some(path) = std::env::var_os(variable).map(PathBuf::from) else {
            continue;
        };
        let path = canon(path);
        if !path.is_dir() {
            continue;
        }
        capabilities.push(DeveloperCapability {
            name: name.to_string(),
            executable: String::new(),
            read_only_roots: vec![path.to_string_lossy().into_owned()],
        });
    }

    capabilities
}

fn resolve_path_executable(name: &str) -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    #[cfg(windows)]
    let suffixes: Vec<String> = std::env::var("PATHEXT")
        .unwrap_or_else(|_| ".COM;.EXE;.BAT;.CMD".to_string())
        .split(';')
        .map(|suffix| suffix.to_ascii_lowercase())
        .collect();

    for directory in std::env::split_paths(&path) {
        let candidate = directory.join(name);
        if candidate.is_file() {
            return std::fs::canonicalize(candidate).ok();
        }
        #[cfg(windows)]
        for suffix in &suffixes {
            let candidate = directory.join(format!("{name}{suffix}"));
            if candidate.is_file() {
                return std::fs::canonicalize(candidate).ok();
            }
        }
    }
    None
}

pub(super) fn capability_roots(executable: &Path) -> Vec<PathBuf> {
    let mut roots = executable
        .parent()
        .map(Path::to_path_buf)
        .into_iter()
        .collect::<Vec<_>>();
    let text = executable.to_string_lossy().replace('\\', "/");
    for prefix in ["/opt/homebrew", "/opt/local", "/usr/local"] {
        if text == prefix || text.starts_with(&format!("{prefix}/")) {
            let path = PathBuf::from(prefix);
            if path.exists() {
                roots.push(canon(path));
            }
        }
    }
    if let Some(index) = text.find(".app/Contents/Developer/") {
        let developer = PathBuf::from(&text[..index + ".app/Contents/Developer".len()]);
        if developer.exists() {
            roots.push(canon(developer));
        }
    }
    if let Some(home) = dirs::home_dir()
        && executable.starts_with(home.join(".cargo/bin"))
    {
        for path in [home.join(".cargo/bin"), home.join(".rustup")] {
            if path.exists() {
                roots.push(canon(path));
            }
        }
    }
    roots
}

pub(super) fn trusted_config_and_certificate_reads() -> Vec<PathBuf> {
    let mut paths = Vec::new();
    for path in [
        PathBuf::from("/etc/ssl/certs"),
        PathBuf::from("/etc/ssl/cert.pem"),
        PathBuf::from("/etc/paths"),
        PathBuf::from("/etc/resolv.conf"),
    ] {
        if path.exists() {
            paths.push(canon(path));
        }
    }
    if let Some(home) = dirs::home_dir() {
        for path in [
            home.join(".gitconfig"),
            home.join(".config/git"),
            home.join(".cargo/config.toml"),
            home.join(".npmrc"),
        ] {
            if path.exists() {
                paths.push(canon(path));
            }
        }
    }
    paths
}
