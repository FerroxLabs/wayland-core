//! Redacted executable readiness derived from the real MCP stdio launch context.

use std::collections::HashMap;

use wcore_config::shell::{
    ExecutableReadinessError, LaunchValueSource, McpStdioLaunchContext, McpStdioLaunchContextError,
};

/// Closed readiness result safe to retain in runtime diagnostics.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum McpStdioExecutableReadinessStatus {
    Resolved,
    InvalidExecutable,
    MissingEffectivePath,
    InvalidEffectiveEnvironment,
    PermissionDenied,
    NotExecutable,
    NotFound,
    ProbeTimedOut,
    Unchecked,
}

/// Secret-free readiness evidence for one exact MCP stdio launch environment.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct McpStdioExecutableReadiness {
    pub status: McpStdioExecutableReadinessStatus,
    pub path_source: LaunchValueSource,
    pub pathext_source: LaunchValueSource,
}

impl McpStdioExecutableReadiness {
    fn unavailable(status: McpStdioExecutableReadinessStatus) -> Self {
        Self {
            status,
            path_source: LaunchValueSource::Unavailable,
            pathext_source: LaunchValueSource::Unavailable,
        }
    }
}

/// Inspect an executable without starting it, using the same opaque context
/// builder as [`super::stdio::StdioTransport`]. Readiness is advisory; the
/// transport's real spawn and MCP handshake remain authoritative.
pub async fn inspect_mcp_stdio_executable(
    command: &str,
    environment: &HashMap<String, String>,
) -> McpStdioExecutableReadiness {
    let context = match McpStdioLaunchContext::capture(environment) {
        Ok(context) => context,
        Err(
            McpStdioLaunchContextError::CurrentDirectory { .. }
            | McpStdioLaunchContextError::AmbiguousWindowsEnvironmentKeys,
        ) => {
            return McpStdioExecutableReadiness::unavailable(
                McpStdioExecutableReadinessStatus::InvalidEffectiveEnvironment,
            );
        }
    };
    inspect_mcp_stdio_executable_in_context(command, &context).await
}

/// Inspect an executable against an already-captured launch context.
///
/// The manager uses this entry point before moving the same context into the
/// transport, so retained readiness describes the child that was actually
/// spawned rather than a second ambient-environment snapshot.
pub async fn inspect_mcp_stdio_executable_in_context(
    command: &str,
    context: &McpStdioLaunchContext,
) -> McpStdioExecutableReadiness {
    let path_source = context.path_source();
    let pathext_source = context.pathext_source();
    let status = match context.resolve_executable(command.as_ref()).await {
        Ok(_) => McpStdioExecutableReadinessStatus::Resolved,
        Err(ExecutableReadinessError::InvalidExecutable { .. }) => {
            McpStdioExecutableReadinessStatus::InvalidExecutable
        }
        Err(ExecutableReadinessError::MissingEffectivePath { .. }) => {
            McpStdioExecutableReadinessStatus::MissingEffectivePath
        }
        Err(ExecutableReadinessError::InvalidEffectiveEnvironment { .. }) => {
            McpStdioExecutableReadinessStatus::InvalidEffectiveEnvironment
        }
        Err(ExecutableReadinessError::PermissionDenied { .. }) => {
            McpStdioExecutableReadinessStatus::PermissionDenied
        }
        Err(ExecutableReadinessError::NotExecutable { .. }) => {
            McpStdioExecutableReadinessStatus::NotExecutable
        }
        Err(ExecutableReadinessError::NotFound { .. }) => {
            McpStdioExecutableReadinessStatus::NotFound
        }
        Err(ExecutableReadinessError::ProbeTimedOut { .. }) => {
            McpStdioExecutableReadinessStatus::ProbeTimedOut
        }
        Err(
            ExecutableReadinessError::Io { .. }
            | ExecutableReadinessError::ProbeFailed { .. }
            | ExecutableReadinessError::UncheckedDirectSearch { .. }
            | ExecutableReadinessError::NetworkPathUnsupported { .. }
            | ExecutableReadinessError::EnvironmentLimitExceeded { .. },
        ) => McpStdioExecutableReadinessStatus::Unchecked,
    };

    McpStdioExecutableReadiness {
        status,
        path_source,
        pathext_source,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn explicit_path_source_survives_redaction() {
        let evidence = inspect_mcp_stdio_executable(
            "wayland-missing-mcp-executable",
            &HashMap::from([("PATH".to_owned(), String::new())]),
        )
        .await;

        assert!(matches!(
            evidence.status,
            McpStdioExecutableReadinessStatus::NotFound
                | McpStdioExecutableReadinessStatus::MissingEffectivePath
        ));
        assert_eq!(evidence.path_source, LaunchValueSource::ExplicitServer);
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn readiness_never_executes_the_target() {
        use std::os::unix::fs::PermissionsExt;

        let temp = tempfile::tempdir().unwrap();
        let executable = temp.path().join("marker-mcp");
        let marker = temp.path().join("executed");
        std::fs::write(
            &executable,
            format!("#!/bin/sh\nprintf executed > '{}'\n", marker.display()),
        )
        .unwrap();
        let mut permissions = std::fs::metadata(&executable).unwrap().permissions();
        permissions.set_mode(0o700);
        std::fs::set_permissions(&executable, permissions).unwrap();

        let evidence =
            inspect_mcp_stdio_executable(executable.to_str().unwrap(), &HashMap::new()).await;

        assert_eq!(evidence.status, McpStdioExecutableReadinessStatus::Resolved);
        assert!(
            !marker.exists(),
            "readiness must not execute third-party code"
        );
    }
}
