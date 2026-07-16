//! Opaque launch environment shared by MCP stdio spawning and readiness.

use std::collections::{BTreeMap, HashMap};
use std::ffi::{OsStr, OsString};
use std::io;
use std::path::PathBuf;

use tokio::process::Command;

use super::{ExecutableReadinessError, ResolvedExecutable, resolve_mcp_stdio_executable};

/// Environment variables inherited by an MCP stdio child.
///
/// Provider credentials, vault passphrases, and other ambient secrets are
/// deliberately absent. A server that needs an additional variable must name
/// it explicitly in its own configuration.
const FORWARDED_ENVIRONMENT_VARIABLES: &[&str] = &[
    "PATH",
    "HOME",
    "USER",
    "LANG",
    "TZ",
    "LC_ALL",
    "LC_CTYPE",
    "LC_MESSAGES",
    "LC_MONETARY",
    "LC_NUMERIC",
    "LC_TIME",
    "TMPDIR",
    "WAYLAND_HOME",
    "SYSTEMROOT",
    "WINDIR",
    "COMSPEC",
    "PATHEXT",
    "PROCESSOR_ARCHITECTURE",
    "USERPROFILE",
    "APPDATA",
    "LOCALAPPDATA",
    "PROGRAMFILES",
    "PROGRAMFILES(X86)",
    "PSMODULEPATH",
    "TEMP",
    "TMP",
];

/// Failure while capturing the effective environment for an MCP stdio child.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum McpStdioLaunchContextError {
    #[error("cannot capture the MCP stdio working directory: filesystem error {kind:?}")]
    CurrentDirectory { kind: io::ErrorKind },
    /// Kept as one redacted error class so callers never retain a configured
    /// key or value. This covers malformed entries on every platform and
    /// case-ambiguous entries on Windows.
    #[error("configured MCP environment is invalid or contains ambiguous keys")]
    AmbiguousWindowsEnvironmentKeys,
}

/// Redacted origin of a PATH-like value in the effective MCP child context.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LaunchValueSource {
    InheritedAllowlist,
    ExplicitServer,
    Unavailable,
}

/// Exact working directory and environment used for one MCP stdio launch.
///
/// Fields remain private and this type intentionally implements neither
/// `Debug` nor serialization: explicit per-server values may contain secrets.
/// Callers can only apply the context to a child command or ask the bounded,
/// non-spawning resolver for executable readiness.
pub struct McpStdioLaunchContext {
    current_dir: PathBuf,
    environment: BTreeMap<String, OsString>,
    windows_environment: bool,
    path_source: LaunchValueSource,
    pathext_source: LaunchValueSource,
}

impl McpStdioLaunchContext {
    /// Capture the exact environment that an MCP stdio child should receive.
    ///
    /// The current working directory is snapshotted once. Ambient variables
    /// are read with `var_os` from the curated allowlist, the canonical profile
    /// home is injected next, and explicit per-server values are layered last.
    pub fn capture(
        explicit_environment: &HashMap<String, String>,
    ) -> Result<Self, McpStdioLaunchContextError> {
        let current_dir = std::env::current_dir()
            .map_err(|error| McpStdioLaunchContextError::CurrentDirectory { kind: error.kind() })?;
        let profile_home = crate::config::profile_home();
        Self::capture_for(
            current_dir,
            profile_home,
            explicit_environment,
            cfg!(windows),
            |key| std::env::var_os(key),
        )
    }

    /// Apply the captured context without inheriting any other parent values.
    pub fn apply_to(&self, command: &mut Command) {
        command
            .env_clear()
            .current_dir(&self.current_dir)
            .envs(&self.environment);
    }

    /// Return where the effective PATH came from without exposing its value.
    #[must_use]
    pub fn path_source(&self) -> LaunchValueSource {
        self.path_source
    }

    /// Return where the effective PATHEXT came from without exposing its value.
    #[must_use]
    pub fn pathext_source(&self) -> LaunchValueSource {
        self.pathext_source
    }

    /// Resolve `program` against this exact child environment without spawning.
    pub async fn resolve_executable(
        &self,
        program: &OsStr,
    ) -> Result<ResolvedExecutable, ExecutableReadinessError> {
        let effective_environment = self
            .environment
            .iter()
            .map(|(key, value)| (OsString::from(key), value.clone()))
            .collect::<Vec<_>>();
        resolve_mcp_stdio_executable(
            program,
            &self.current_dir,
            self.environment_value("PATH"),
            self.environment_value("PATHEXT"),
            &effective_environment,
        )
        .await
    }

    fn capture_for<F>(
        current_dir: PathBuf,
        profile_home: PathBuf,
        explicit_environment: &HashMap<String, String>,
        windows_environment: bool,
        mut ambient_value: F,
    ) -> Result<Self, McpStdioLaunchContextError>
    where
        F: FnMut(&str) -> Option<OsString>,
    {
        let mut explicit_entries = explicit_environment.iter().collect::<Vec<_>>();
        if explicit_entries
            .iter()
            .any(|(key, value)| !environment_entry_is_valid(key, value))
        {
            return Err(McpStdioLaunchContextError::AmbiguousWindowsEnvironmentKeys);
        }
        explicit_entries.sort_by(|(left, _), (right, _)| {
            environment_key_order(left, right, windows_environment)
        });
        if windows_environment
            && explicit_entries.windows(2).any(|pair| {
                windows_environment_key(pair[0].0) == windows_environment_key(pair[1].0)
            })
        {
            return Err(McpStdioLaunchContextError::AmbiguousWindowsEnvironmentKeys);
        }

        let mut environment = BTreeMap::new();
        let mut path_source = LaunchValueSource::Unavailable;
        let mut pathext_source = LaunchValueSource::Unavailable;
        for key in FORWARDED_ENVIRONMENT_VARIABLES {
            if let Some(value) = ambient_value(key) {
                insert_environment_value(&mut environment, key, value, windows_environment);
                update_value_source(
                    key,
                    LaunchValueSource::InheritedAllowlist,
                    windows_environment,
                    &mut path_source,
                    &mut pathext_source,
                );
            }
        }
        insert_environment_value(
            &mut environment,
            "WAYLAND_PROFILE_HOME",
            profile_home.into_os_string(),
            windows_environment,
        );
        for (key, value) in explicit_entries {
            insert_environment_value(
                &mut environment,
                key,
                OsString::from(value),
                windows_environment,
            );
            update_value_source(
                key,
                LaunchValueSource::ExplicitServer,
                windows_environment,
                &mut path_source,
                &mut pathext_source,
            );
        }

        Ok(Self {
            current_dir,
            environment,
            windows_environment,
            path_source,
            pathext_source,
        })
    }

    fn environment_value(&self, requested_key: &str) -> Option<&OsStr> {
        if self.windows_environment {
            self.environment
                .iter()
                .find(|(key, _)| windows_environment_keys_match(key, requested_key))
                .map(|(_, value)| value.as_os_str())
        } else {
            self.environment.get(requested_key).map(OsString::as_os_str)
        }
    }
}

fn update_value_source(
    key: &str,
    source: LaunchValueSource,
    windows_environment: bool,
    path_source: &mut LaunchValueSource,
    pathext_source: &mut LaunchValueSource,
) {
    let key_matches = |expected: &str| {
        if windows_environment {
            windows_environment_keys_match(key, expected)
        } else {
            key == expected
        }
    };
    if key_matches("PATH") {
        *path_source = source;
    } else if key_matches("PATHEXT") {
        *pathext_source = source;
    }
}

fn environment_key_order(left: &str, right: &str, windows_environment: bool) -> std::cmp::Ordering {
    if windows_environment {
        windows_environment_key(left)
            .cmp(&windows_environment_key(right))
            .then_with(|| left.cmp(right))
    } else {
        left.cmp(right)
    }
}

fn insert_environment_value(
    environment: &mut BTreeMap<String, OsString>,
    key: &str,
    value: OsString,
    windows_environment: bool,
) {
    if windows_environment
        && let Some(existing_key) = environment
            .keys()
            .find(|existing| windows_environment_keys_match(existing, key))
            .cloned()
    {
        environment.remove(&existing_key);
    }
    environment.insert(key.to_owned(), value);
}

fn environment_entry_is_valid(key: &str, value: &str) -> bool {
    !key.is_empty() && !key.contains('\0') && !key.contains('=') && !value.contains('\0')
}

/// Windows treats environment names case-insensitively. Use Unicode uppercase
/// normalization rather than ASCII-only comparison so configured names cannot
/// evade duplicate detection and then collapse when CreateProcess builds the
/// child environment block. False-positive normalization is fail-closed.
fn windows_environment_key(key: &str) -> String {
    key.chars().flat_map(char::to_uppercase).collect()
}

fn windows_environment_keys_match(left: &str, right: &str) -> bool {
    windows_environment_key(left) == windows_environment_key(right)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    fn context_for(
        explicit_environment: HashMap<String, String>,
        windows_environment: bool,
    ) -> Result<McpStdioLaunchContext, McpStdioLaunchContextError> {
        McpStdioLaunchContext::capture_for(
            Path::new("/captured/cwd").to_path_buf(),
            Path::new("/profile/home").to_path_buf(),
            &explicit_environment,
            windows_environment,
            |key| Some(OsString::from(format!("ambient-{key}"))),
        )
    }

    #[test]
    fn capture_forwards_only_curated_ambient_values() {
        let context = context_for(HashMap::new(), false).expect("context should be captured");

        assert_eq!(
            context.environment_value("PATH"),
            Some(OsStr::new("ambient-PATH"))
        );
        assert_eq!(context.path_source(), LaunchValueSource::InheritedAllowlist);
        assert_eq!(
            context.pathext_source(),
            LaunchValueSource::InheritedAllowlist
        );
        assert_eq!(context.environment_value("OPENAI_API_KEY"), None);
        assert_eq!(
            context.environment_value("WAYLAND_PROFILE_HOME"),
            Some(OsStr::new("/profile/home"))
        );
    }

    #[test]
    fn explicit_environment_wins_after_profile_and_allowlist() {
        let explicit_environment = HashMap::from([
            ("PATH".to_owned(), "operator-path".to_owned()),
            (
                "WAYLAND_PROFILE_HOME".to_owned(),
                "operator-profile".to_owned(),
            ),
            ("SERVER_TOKEN".to_owned(), "configured-secret".to_owned()),
        ]);
        let context = context_for(explicit_environment, false).expect("context should be captured");

        assert_eq!(
            context.environment_value("PATH"),
            Some(OsStr::new("operator-path"))
        );
        assert_eq!(context.path_source(), LaunchValueSource::ExplicitServer);
        assert_eq!(
            context.environment_value("WAYLAND_PROFILE_HOME"),
            Some(OsStr::new("operator-profile"))
        );
        assert_eq!(
            context.environment_value("SERVER_TOKEN"),
            Some(OsStr::new("configured-secret"))
        );
    }

    #[test]
    fn windows_explicit_key_replaces_ambient_case_insensitively() {
        let explicit_environment = HashMap::from([("Path".to_owned(), "operator".to_owned())]);
        let context = context_for(explicit_environment, true).expect("context should be captured");

        assert_eq!(
            context.environment.len(),
            FORWARDED_ENVIRONMENT_VARIABLES.len() + 1
        );
        assert_eq!(
            context.environment_value("PATH"),
            Some(OsStr::new("operator"))
        );
        assert_eq!(context.path_source(), LaunchValueSource::ExplicitServer);
        assert!(context.environment.contains_key("Path"));
        assert!(!context.environment.contains_key("PATH"));
    }

    #[test]
    fn missing_path_like_values_report_unavailable_without_values() {
        let context = McpStdioLaunchContext::capture_for(
            Path::new("/captured/cwd").to_path_buf(),
            Path::new("/profile/home").to_path_buf(),
            &HashMap::new(),
            true,
            |_| None,
        )
        .expect("context should be captured");

        assert_eq!(context.path_source(), LaunchValueSource::Unavailable);
        assert_eq!(context.pathext_source(), LaunchValueSource::Unavailable);
    }

    #[test]
    fn windows_rejects_ambiguous_explicit_key_case() {
        let explicit_environment = HashMap::from([
            ("Path".to_owned(), "first".to_owned()),
            ("PATH".to_owned(), "second".to_owned()),
        ]);

        assert!(matches!(
            context_for(explicit_environment, true),
            Err(McpStdioLaunchContextError::AmbiguousWindowsEnvironmentKeys)
        ));
    }

    #[test]
    fn windows_rejects_non_ascii_case_equivalent_keys() {
        let explicit_environment = HashMap::from([
            ("Straße".to_owned(), "first".to_owned()),
            ("STRASSE".to_owned(), "second".to_owned()),
        ]);

        assert!(matches!(
            context_for(explicit_environment, true),
            Err(McpStdioLaunchContextError::AmbiguousWindowsEnvironmentKeys)
        ));
    }

    #[test]
    fn rejects_explicit_environment_entries_that_spawn_cannot_accept() {
        for explicit_environment in [
            HashMap::from([("".to_owned(), "value".to_owned())]),
            HashMap::from([("BAD=KEY".to_owned(), "value".to_owned())]),
            HashMap::from([("BAD\0KEY".to_owned(), "value".to_owned())]),
            HashMap::from([("KEY".to_owned(), "bad\0value".to_owned())]),
        ] {
            let error = context_for(explicit_environment, false)
                .err()
                .expect("invalid environment must fail closed");
            assert_eq!(
                error,
                McpStdioLaunchContextError::AmbiguousWindowsEnvironmentKeys
            );
            assert!(!error.to_string().contains("BAD"));
            assert!(!error.to_string().contains("value"));
        }
    }

    #[test]
    fn unix_preserves_case_distinct_explicit_keys() {
        let explicit_environment = HashMap::from([
            ("Path".to_owned(), "mixed-case".to_owned()),
            ("PATH".to_owned(), "upper-case".to_owned()),
        ]);
        let context = context_for(explicit_environment, false).expect("context should be captured");

        assert_eq!(
            context.environment_value("PATH"),
            Some(OsStr::new("upper-case"))
        );
        assert_eq!(
            context.environment.get("Path").map(OsString::as_os_str),
            Some(OsStr::new("mixed-case"))
        );
    }

    #[test]
    fn apply_to_uses_captured_cwd_and_only_captured_environment() {
        let explicit_environment = HashMap::from([("SERVER_FLAG".to_owned(), "on".to_owned())]);
        let context = context_for(explicit_environment, false).expect("context should be captured");
        let mut command = Command::new("unused");

        context.apply_to(&mut command);

        let command = command.as_std();
        assert_eq!(command.get_current_dir(), Some(Path::new("/captured/cwd")));
        let configured_environment = command
            .get_envs()
            .map(|(key, value)| {
                (
                    key.to_string_lossy().into_owned(),
                    value.map(|value| value.to_os_string()),
                )
            })
            .collect::<BTreeMap<_, _>>();
        assert_eq!(
            configured_environment.get("SERVER_FLAG"),
            Some(&Some(OsString::from("on")))
        );
        assert!(!configured_environment.contains_key("OPENAI_API_KEY"));
    }
}
