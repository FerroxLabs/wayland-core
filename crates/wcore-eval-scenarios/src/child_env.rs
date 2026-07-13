//! Explicit environment plan for every evaluated child process.

use std::ffi::OsString;
use std::path::Path;

use crate::providers::ProviderConfig;

#[derive(Debug, Clone)]
pub(crate) struct ChildEnvironment {
    variables: Vec<(OsString, OsString)>,
}

impl ChildEnvironment {
    pub(crate) fn build(
        cwd: &Path,
        wayland_home: &Path,
        provider: Option<&ProviderConfig>,
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

        copy_if_present(&mut variables, "PATH");
        #[cfg(windows)]
        for name in ["SystemRoot", "WINDIR", "COMSPEC", "PATHEXT"] {
            copy_if_present(&mut variables, name);
        }

        if let Some(secret) = provider.and_then(ProviderConfig::resolved_key) {
            variables.push(("API_KEY".into(), secret.into()));
        }

        // This is an explicit deterministic-fixture control, not an ambient
        // product input. F04 replaces it with the fixture protocol.
        copy_if_present(&mut variables, "WCORE_EVAL_FIXTURE_FAIL_CANARY");

        Ok(Self { variables })
    }

    pub(crate) fn apply_tokio(&self, command: &mut tokio::process::Command) {
        command.env_clear();
        command.envs(self.variables.iter().cloned());
    }

    #[cfg(unix)]
    pub(crate) fn apply_pty(&self, command: &mut portable_pty::CommandBuilder) {
        command.env_clear();
        for (key, value) in &self.variables {
            command.env(key, value);
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
