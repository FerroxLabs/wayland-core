//! Explicit environment plan for every evaluated child process.

use std::ffi::OsString;
use std::fs::OpenOptions;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{LazyLock, Mutex};

use rand::RngCore;

static CREDENTIAL_FILE_COUNTER: AtomicU64 = AtomicU64::new(0);
static VAULT_PASSPHRASES: LazyLock<Mutex<std::collections::HashMap<PathBuf, String>>> =
    LazyLock::new(|| Mutex::new(std::collections::HashMap::new()));

pub(crate) struct ChildEnvironment {
    variables: Vec<(OsString, OsString)>,
    credential_file: Option<PathBuf>,
    vault_passphrase: String,
}

pub(crate) struct VaultGuard {
    #[cfg(unix)]
    fd: std::os::unix::io::RawFd,
}

#[cfg(unix)]
impl Drop for VaultGuard {
    fn drop(&mut self) {
        // SAFETY: the guard uniquely owns the parent's copy of this descriptor.
        let _ = unsafe { libc::close(self.fd) };
    }
}

impl ChildEnvironment {
    pub(crate) fn build(
        cwd: &Path,
        wayland_home: &Path,
        secret: Option<&str>,
    ) -> std::io::Result<Self> {
        let env_root = wayland_home.join("eval-environment");
        let home = env_root.join("home");
        let config = env_root.join("config");
        let data = env_root.join("data");
        let cache = env_root.join("cache");
        let state = env_root.join("state");
        let runtime = env_root.join("runtime");
        let temp = env_root.join("temp");
        for directory in [&home, &config, &data, &cache, &state, &runtime, &temp] {
            std::fs::create_dir_all(directory)?;
        }

        let mut variables = vec![
            pair("WAYLAND_HOME", wayland_home),
            pair("HOME", &home),
            pair("USERPROFILE", &home),
            pair("XDG_CONFIG_HOME", &config),
            pair("XDG_DATA_HOME", &data),
            pair("XDG_CACHE_HOME", &cache),
            pair("XDG_STATE_HOME", &state),
            pair("XDG_RUNTIME_DIR", &runtime),
            pair("APPDATA", &config),
            pair("LOCALAPPDATA", &data),
            pair("TMPDIR", &temp),
            pair("TEMP", &temp),
            pair("TMP", &temp),
            pair("PWD", cwd),
            ("GIT_CONFIG_NOSYSTEM".into(), "1".into()),
            ("GIT_TERMINAL_PROMPT".into(), "0".into()),
            ("LANG".into(), "C".into()),
            ("LC_ALL".into(), "C".into()),
            ("TERM".into(), "dumb".into()),
            ("NO_COLOR".into(), "1".into()),
        ];

        variables.push(("PATH".into(), controlled_path()));
        #[cfg(windows)]
        for name in ["SystemRoot", "WINDIR", "COMSPEC", "PATHEXT"] {
            copy_if_present(&mut variables, name);
        }

        // This is an explicit deterministic-fixture control, not an ambient
        // product input. F04 replaces it with the fixture protocol.
        copy_if_present(&mut variables, "WCORE_EVAL_FIXTURE_FAIL_CANARY");

        let credential_file = secret
            .filter(|value| !value.is_empty())
            .map(|value| write_credential_file(&env_root, value))
            .transpose()?;

        Ok(Self {
            variables,
            credential_file,
            vault_passphrase: vault_passphrase_for(wayland_home),
        })
    }

    pub(crate) fn apply_tokio(
        &self,
        command: &mut tokio::process::Command,
    ) -> std::io::Result<VaultGuard> {
        command.env_clear();
        command.envs(self.variables.iter().cloned());
        if let Some(path) = &self.credential_file {
            command.arg("--api-key-file").arg(path);
        }
        self.configure_tokio_vault(command)
    }

    #[cfg(unix)]
    pub(crate) fn apply_pty(
        &self,
        command: &mut portable_pty::CommandBuilder,
    ) -> std::io::Result<VaultGuard> {
        command.env_clear();
        for (key, value) in &self.variables {
            command.env(key, value);
        }
        if let Some(path) = &self.credential_file {
            command.arg("--api-key-file");
            command.arg(path);
        }
        let guard = self.inheritable_vault_pipe()?;
        command.env("WAYLAND_VAULT_PASSPHRASE_FD", guard.fd.to_string());
        Ok(guard)
    }

    #[cfg(unix)]
    fn configure_tokio_vault(
        &self,
        command: &mut tokio::process::Command,
    ) -> std::io::Result<VaultGuard> {
        let guard = self.inheritable_vault_pipe()?;
        command.env("WAYLAND_VAULT_PASSPHRASE_FD", guard.fd.to_string());
        Ok(guard)
    }

    #[cfg(not(unix))]
    fn configure_tokio_vault(
        &self,
        command: &mut tokio::process::Command,
    ) -> std::io::Result<VaultGuard> {
        // Windows has no Unix-style inherited file descriptor. This is a
        // hermetic evaluator child; production still warns on this legacy
        // compatibility path.
        command.env("WAYLAND_VAULT_PASSPHRASE", &self.vault_passphrase);
        Ok(VaultGuard {})
    }

    #[cfg(unix)]
    fn inheritable_vault_pipe(&self) -> std::io::Result<VaultGuard> {
        use std::os::unix::io::FromRawFd;

        let mut pipe = [0; 2];
        // SAFETY: `pipe` points to two valid integers. Plain `pipe(2)` is
        // intentional: the read end must survive exec into packaged Core.
        if unsafe { libc::pipe(pipe.as_mut_ptr()) } != 0 {
            return Err(std::io::Error::last_os_error());
        }
        let guard = VaultGuard { fd: pipe[0] };
        // SAFETY: this function uniquely owns the write descriptor and the
        // `File` closes it on every return path.
        let mut writer = unsafe { std::fs::File::from_raw_fd(pipe[1]) };
        writer.write_all(self.vault_passphrase.as_bytes())?;
        writer.flush()?;
        Ok(guard)
    }
}

fn random_vault_passphrase() -> String {
    let mut bytes = [0_u8; 32];
    rand::rngs::OsRng.fill_bytes(&mut bytes);
    let mut encoded = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        use std::fmt::Write as _;
        write!(&mut encoded, "{byte:02x}").expect("writing to String cannot fail");
    }
    encoded
}

fn vault_passphrase_for(wayland_home: &Path) -> String {
    let mut passphrases = VAULT_PASSPHRASES
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    passphrases
        .entry(wayland_home.to_path_buf())
        .or_insert_with(random_vault_passphrase)
        .clone()
}

fn pair(name: &str, value: &Path) -> (OsString, OsString) {
    (name.into(), value.as_os_str().to_owned())
}

fn copy_if_present(variables: &mut Vec<(OsString, OsString)>, name: &str) {
    if let Some(value) = std::env::var_os(name) {
        variables.push((name.into(), value));
    }
}

fn controlled_path() -> OsString {
    if let Some(path) = std::env::var_os("WCORE_EVAL_TOOL_PATH").filter(|value| !value.is_empty()) {
        return path;
    }
    #[cfg(windows)]
    {
        let root = std::env::var_os("SystemRoot").unwrap_or_else(|| r"C:\Windows".into());
        let root = PathBuf::from(root);
        return std::env::join_paths([root.join("System32"), root])
            .unwrap_or_else(|_| r"C:\Windows\System32;C:\Windows".into());
    }
    #[cfg(not(windows))]
    {
        "/usr/local/bin:/usr/bin:/bin:/opt/homebrew/bin".into()
    }
}

fn write_credential_file(root: &Path, secret: &str) -> std::io::Result<PathBuf> {
    let sequence = CREDENTIAL_FILE_COUNTER.fetch_add(1, Ordering::Relaxed);
    let path = root.join(format!(
        "provider-credential-{}-{sequence}",
        std::process::id()
    ));
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options.open(&path)?;
    file.write_all(secret.as_bytes())?;
    file.sync_all()?;
    Ok(path)
}
