//! Explicit environment plan for every evaluated child process.

use std::ffi::OsString;
use std::fs::OpenOptions;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

static CREDENTIAL_FILE_COUNTER: AtomicU64 = AtomicU64::new(0);

#[derive(Debug, Clone)]
pub(crate) struct ChildEnvironment {
    variables: Vec<(OsString, OsString)>,
    credential_file: Option<PathBuf>,
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
        })
    }

    pub(crate) fn apply_tokio(&self, command: &mut tokio::process::Command) {
        command.env_clear();
        command.envs(self.variables.iter().cloned());
        if let Some(path) = &self.credential_file {
            command.arg("--api-key-file").arg(path);
        }
    }

    #[cfg(unix)]
    pub(crate) fn apply_pty(&self, command: &mut portable_pty::CommandBuilder) {
        command.env_clear();
        for (key, value) in &self.variables {
            command.env(key, value);
        }
        if let Some(path) = &self.credential_file {
            command.arg("--api-key-file");
            command.arg(path);
        }
    }
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
