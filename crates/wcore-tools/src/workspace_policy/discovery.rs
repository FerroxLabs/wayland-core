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

/// Interpreters whose PROGRAM FILES the contained profile grants, on top of
/// [`minimal_toolchain_read_dirs`].
///
/// `minimal_toolchain_read_dirs` only knows about Rust, so on a macOS host
/// whose `node` / `npm` / `python3` come from a package manager the contained
/// shell could not run them AT ALL. Measured on Darwin 25.3.0 before this:
/// `node -e …` and `npm …` both exited 127 `command not found`, and the cause
/// was not a missing tool — `ls -l /opt/homebrew/bin/node` under the same
/// profile returned `Operation not permitted`.
///
/// The list is short and deliberate: these are the interpreters the acceptance
/// matrix runs. `clang` / `xcodebuild` are NOT here — granting the Xcode
/// developer directory to untrusted workspace content is a bigger call than
/// this, and it is named as an open item rather than taken silently.
const CONTAINED_INTERPRETERS: [&str; 4] = ["node", "npm", "python3", "git"];

/// Read roots for [`super::WorkspacePolicy::contained`].
///
/// `minimal_toolchain_read_dirs` plus the PROGRAM FILES — never the
/// configuration or the state — of the [`CONTAINED_INTERPRETERS`] found on
/// `PATH`. See [`contained_capability_roots`] for where that line is drawn.
pub(super) fn contained_toolchain_read_dirs() -> Vec<PathBuf> {
    let mut v = minimal_toolchain_read_dirs();
    for name in CONTAINED_INTERPRETERS {
        let Some(executable) = resolve_path_executable(name) else {
            continue;
        };
        v.extend(contained_capability_roots(&executable));
    }
    v.sort();
    v.dedup();
    v
}

/// Package-manager prefixes whose layout separates program files from
/// configuration and state. Same three prefixes [`capability_roots`] already
/// recognises.
const PACKAGE_PREFIXES: [&str; 3] = ["/opt/homebrew", "/opt/local", "/usr/local"];

/// Program-file subtrees of a package-manager prefix.
///
/// `Cellar` is where Homebrew's installed formulae actually live, and `opt` is
/// the symlink farm every keg's dylib install names point through
/// (`/opt/homebrew/opt/libuv/lib/libuv.1.dylib`). Both are needed: seatbelt
/// evaluates the symlink node AND the resolved target, so granting only `opt`
/// still failed with `Library not loaded … (blocked by sandbox)` — measured.
///
/// What is deliberately EXCLUDED is the point of this list. `etc` holds service
/// configuration (`gnupg`, `unbound`, `pkcs11`) and `var` holds service STATE —
/// on the host this was measured on, `/opt/homebrew/var/postgresql@16` and
/// `postgresql@17`, i.e. real database contents. `Caskroom`, `share`, and
/// Homebrew's own `Library` git checkout are excluded for the same reason.
/// Granting the whole prefix, which is what [`capability_roots`] does for the
/// TRUSTED profile, would hand all of that to untrusted workspace content.
const PACKAGE_PROGRAM_SUBDIRS: [&str; 3] = ["bin", "opt", "Cellar"];

/// Read roots one contained-profile interpreter needs, and nothing more.
fn contained_capability_roots(executable: &Path) -> Vec<PathBuf> {
    let mut roots: Vec<PathBuf> = executable
        .parent()
        .map(Path::to_path_buf)
        .into_iter()
        .collect();
    let text = executable.to_string_lossy().replace('\\', "/");
    for prefix in PACKAGE_PREFIXES {
        if text != prefix && !text.starts_with(&format!("{prefix}/")) {
            continue;
        }
        for sub in PACKAGE_PROGRAM_SUBDIRS {
            let path = PathBuf::from(prefix).join(sub);
            if path.exists() {
                roots.push(canon(path));
            }
        }
        // Homebrew's OpenSSL ships its own `openssl.cnf`, and every binary
        // linked against that keg's libcrypto opens it unconditionally at init
        // — exactly like the system LibreSSL config the SBPL profile already
        // grants as a literal. Without it `node -e …` exits 1 with
        // `BIO_new_file:Operation not permitted:… fopen(/opt/homebrew/etc/
        // openssl@3/openssl.cnf, rb)` and writes no file (measured). Granted as
        // the single FILE, never the directory: the sibling `private/` in an
        // OpenSSL etc directory is where keys live.
        roots.extend(openssl_config_files(Path::new(prefix)));
    }
    // A globally installed Node CLI is a shim that `require`s its package one
    // level up — the same PACKAGE-ROOT-not-`node_modules` rule
    // `capability_roots` already applies, and the reason `/opt/homebrew/lib` is
    // NOT in `PACKAGE_PROGRAM_SUBDIRS`. Without it `npm --version` is exit 127:
    // the PATH entry is a symlink whose target the profile denies (measured).
    roots.extend(node_package_root(&text));
    roots
}

/// `<prefix>/etc/openssl*/openssl.cnf` — one `read_dir` of `<prefix>/etc`, so
/// the version suffix (`openssl@3` today, `openssl@4` tomorrow) is discovered
/// rather than hardcoded.
fn openssl_config_files(prefix: &Path) -> Vec<PathBuf> {
    let Ok(entries) = std::fs::read_dir(prefix.join("etc")) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for entry in entries.flatten() {
        if !entry.file_name().to_string_lossy().starts_with("openssl") {
            continue;
        }
        let config = entry.path().join("openssl.cnf");
        if config.is_file() {
            out.push(canon(config));
        }
    }
    out
}

/// The PACKAGE root of a `<prefix>/lib/node_modules/<pkg>/…` executable, scoped
/// packages included. Shared with [`capability_roots`]' rationale; see there.
fn node_package_root(text: &str) -> Option<PathBuf> {
    const NODE_MODULES: &str = "/lib/node_modules/";
    let index = text.find(NODE_MODULES)?;
    let base = index + NODE_MODULES.len();
    let mut segments = text[base..].split('/');
    let mut package = segments.next().unwrap_or_default().to_string();
    if package.starts_with('@')
        && let Some(scoped) = segments.next()
    {
        package.push('/');
        package.push_str(scoped);
    }
    if package.is_empty() {
        return None;
    }
    let root = PathBuf::from(format!("{}{package}", &text[..base]));
    root.is_dir().then(|| canon(root))
}

/// The two paths libgit2 probes for a global git configuration, in the order it
/// probes them, whether or not they exist.
///
/// libgit2 derives these from `$HOME` / `$XDG_CONFIG_HOME` and **ignores
/// `GIT_CONFIG_GLOBAL`** — measured on Darwin 25.3.0: with the redirect from
/// `git_config_env` in place, `cargo new` still exited 101 with `failed to stat
/// '<home>/.gitconfig'; class=Config (7)`. It hard-errors on EPERM but is happy
/// with ENOENT, so the contained profile grants METADATA on these two and
/// nothing else; see `WorkspacePolicy::metadata_readable_roots`.
///
/// Both are required. Granting only `~/.gitconfig` on a host that HAS an XDG
/// git config moves the same failure one path along — measured, `cargo new`
/// exited 101 with `failed to stat '<home>/.config/git/config'`.
///
/// Existence is deliberately NOT checked: seatbelt answers EPERM for an
/// ungranted path whether or not it exists, so a host with no `~/.gitconfig`
/// needs the grant just as much (measured — a grant on a decoy path left
/// `cargo new` at exit 101).
pub(super) fn libgit2_global_config_probes() -> Vec<PathBuf> {
    let Some(home) = dirs::home_dir() else {
        return Vec::new();
    };
    let xdg = std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .filter(|p| p.is_absolute())
        .unwrap_or_else(|| home.join(".config"));
    vec![home.join(".gitconfig"), xdg.join("git").join("config")]
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
    // A globally installed Node CLI resolves to
    // `<prefix>/lib/node_modules/<pkg>/bin/<entry>.js`, so the executable's own
    // directory — the only root granted above — holds the entry shim while the
    // package code that shim `require`s on its first line lives one level up.
    // Measured under the macOS sandbox: `npm --version` dies with
    // `Cannot find module '../lib/cli.js'`. Grant the PACKAGE root only, never
    // the whole `node_modules` tree; a scoped package spends two segments.
    // An attacker gains read of that one package's own program files, which are
    // world-readable on disk and already executable by this shell.
    const NODE_MODULES: &str = "/lib/node_modules/";
    if let Some(index) = text.find(NODE_MODULES) {
        let base = index + NODE_MODULES.len();
        let mut segments = text[base..].split('/');
        let mut package = segments.next().unwrap_or_default().to_string();
        if package.starts_with('@')
            && let Some(scoped) = segments.next()
        {
            package.push('/');
            package.push_str(scoped);
        }
        if !package.is_empty() {
            let root = PathBuf::from(format!("{}{package}", &text[..base]));
            if root.is_dir() {
                roots.push(canon(root));
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

/// Reads that are only useful to a child that HAS a network, and that leak host
/// network topology to one that does not. Granted by `readable_roots()` only
/// when the policy's `NetworkPolicy` is not `Deny`.
///
/// `/etc/resolv.conf` names the host's DNS servers and its search domains — on
/// the host this was measured on, a private tailnet domain. It used to sit in
/// [`trusted_config_and_certificate_reads`], which CANONICALIZES, so on a
/// systemd-resolved host the grant landed on the resolved
/// `/run/systemd/resolve/stub-resolv.conf`. That defeated the bwrap backend's
/// `NETWORK_RO_ETC` gate: the backend correctly withheld `/etc/resolv.conf`
/// under `Deny`, and the child read the resolved path instead. Gating here
/// closes both spellings at once and does so for every backend, not just bwrap.
pub(super) fn network_scoped_reads() -> Vec<PathBuf> {
    let mut paths = Vec::new();
    for path in [PathBuf::from("/etc/resolv.conf")] {
        if path.exists() {
            paths.push(canon(path));
        }
    }
    paths
}

pub(super) fn trusted_config_and_certificate_reads() -> Vec<PathBuf> {
    let mut paths = Vec::new();
    for path in [
        PathBuf::from("/etc/ssl/certs"),
        PathBuf::from("/etc/ssl/cert.pem"),
        PathBuf::from("/etc/paths"),
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
