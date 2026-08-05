use serde::{Deserialize, Serialize};
use thiserror::Error;

pub const RUNTIME_DIAGNOSTICS_VERSION: u16 = 1;
pub const RUNTIME_DIAGNOSTICS_REQUEST_ID_MAX_CHARS: usize = 128;

/// Closed, versioned request for the process's effective runtime view.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GetRuntimeDiagnosticsCommand {
    pub diagnostics_version: u16,
    #[serde(deserialize_with = "deserialize_runtime_diagnostics_request_id")]
    pub request_id: String,
}

fn deserialize_runtime_diagnostics_request_id<'de, D>(deserializer: D) -> Result<String, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let request_id = String::deserialize(deserializer)?;
    validate_runtime_diagnostics_request_id(&request_id).map_err(serde::de::Error::custom)?;
    Ok(request_id)
}

/// Fail-closed version negotiation for runtime diagnostics.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Error)]
pub enum RuntimeDiagnosticsVersionError {
    #[error("unsupported runtime diagnostics version: {actual}")]
    UnsupportedVersion { actual: u16 },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Error)]
pub enum RuntimeDiagnosticsRequestIdError {
    #[error("runtime diagnostics request_id must not be empty")]
    Empty,
    #[error("runtime diagnostics request_id exceeds {max_chars} characters")]
    TooLong { max_chars: usize },
}

pub fn validate_runtime_diagnostics_request_id(
    request_id: &str,
) -> Result<(), RuntimeDiagnosticsRequestIdError> {
    let length = request_id.chars().count();
    if length == 0 {
        Err(RuntimeDiagnosticsRequestIdError::Empty)
    } else if length > RUNTIME_DIAGNOSTICS_REQUEST_ID_MAX_CHARS {
        Err(RuntimeDiagnosticsRequestIdError::TooLong {
            max_chars: RUNTIME_DIAGNOSTICS_REQUEST_ID_MAX_CHARS,
        })
    } else {
        Ok(())
    }
}

pub const fn validate_runtime_diagnostics_version(
    actual: u16,
) -> Result<(), RuntimeDiagnosticsVersionError> {
    if actual == RUNTIME_DIAGNOSTICS_VERSION {
        Ok(())
    } else {
        Err(RuntimeDiagnosticsVersionError::UnsupportedVersion { actual })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RuntimeWorkspaceKind {
    Unknown,
    None,
    Project,
    Temporary,
    ProfileHome,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RuntimeEngineMode {
    Unknown,
    Standard,
    Raw,
}

/// Identifies the process binding without exposing environment values.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RuntimeProcessBinding {
    pub profile_binding: RuntimeProfileBinding,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub profile_name: Option<String>,
    pub engine_mode: RuntimeEngineMode,
    pub workspace_kind: RuntimeWorkspaceKind,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RuntimeProfileBinding {
    Unknown,
    DefaultHome,
    ExplicitHome,
    BoundProfile,
    UnboundProfile,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RuntimeDiagnosticsUnavailableReason {
    UnsupportedVersion,
    InvalidRequest,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ConfigSourceRole {
    Global,
    Project,
    Profile,
    Cli,
    Environment,
    CredentialStore,
    DesktopLaunch,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ConfigSourceDisposition {
    Loaded,
    Absent,
    Ignored,
    Unreadable,
    Invalid,
    Overridden,
    Restricted,
}

/// One input to effective configuration. `display_path` is local-host display
/// data and must never be exported as telemetry.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RuntimeConfigSource {
    pub role: ConfigSourceRole,
    pub disposition: ConfigSourceDisposition,
    pub precedence: u16,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub display_path: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub content_digest: Option<String>,
}

/// An override name that the current Core process deliberately did not use.
/// Values are never carried on this wire surface.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct UnsupportedConfigOverride {
    pub name: String,
    pub disposition: ConfigSourceDisposition,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum McpDeclarationOrigin {
    EffectiveConfig,
    GlobalConfig,
    ProjectConfig,
    ProfileConfig,
    RuntimeCommand,
    Plugin,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum McpTransportKind {
    Stdio,
    Sse,
    StreamableHttp,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum McpConnectionState {
    Configured,
    Deferred,
    Connecting,
    Ready,
    Failed,
    TimedOut,
    Skipped,
    Stopping,
    Stopped,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum McpExposureState {
    NotAttempted,
    NotApplicable,
    Exposed,
    ResourceOnly,
    ResourceOnlyUnavailable,
    HiddenNoTools,
    Blocked,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum McpExecutableReadiness {
    NotApplicable,
    Unchecked,
    Resolved,
    MissingEffectivePath,
    NotFound,
    InvalidAbsolutePath,
    InvalidExecutable,
    InvalidEffectiveEnvironment,
    PermissionDenied,
    NotExecutable,
    ProbeTimedOut,
    UnsupportedTransport,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum McpWorkingDirectoryRole {
    InheritedProcess,
    ProjectRoot,
    ProfileHome,
    Explicit,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum McpFailureCode {
    MissingExecutable,
    LaunchFailed,
    ConnectionRefused,
    Timeout,
    ProtocolMismatch,
    AuthenticationRequired,
    AuthorizationDenied,
    InvalidConfiguration,
    TransportClosed,
    Unknown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RuntimeRemediationCode {
    OpenActiveConfig,
    RestartDesktop,
    FixGuiLaunchPath,
    InstallExecutable,
    FixExecutablePermissions,
    ReviewServerConfig,
    RetryConnection,
    RetryDiagnostics,
    CheckAssistantScope,
    RestartToLoadResources,
}

/// Redacted effective state for one configured MCP server.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct McpServerDiagnostic {
    pub name: String,
    pub origin: McpDeclarationOrigin,
    pub transport: McpTransportKind,
    pub connection: McpConnectionState,
    pub exposure: McpExposureState,
    pub deferred: bool,
    pub tool_count: u32,
    pub resources_declared: bool,
    pub resources_exposed: bool,
    pub assistant_scoped: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub executable_basename: Option<String>,
    pub executable_readiness: McpExecutableReadiness,
    pub working_directory: McpWorkingDirectoryRole,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub failure: Option<McpFailureCode>,
    pub remediation: Vec<RuntimeRemediationCode>,
}

/// V1 diagnostics payload. This deliberately has no arbitrary environment,
/// argument, header, stderr, or free-text error maps.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RuntimeDiagnosticsSnapshotV1 {
    pub process: RuntimeProcessBinding,
    pub config_sources: Vec<RuntimeConfigSource>,
    pub unsupported_overrides: Vec<UnsupportedConfigOverride>,
    pub mcp_servers: Vec<McpServerDiagnostic>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn diagnostics_request_id_must_be_non_empty_and_bounded() {
        for request_id in [
            "",
            &"x".repeat(RUNTIME_DIAGNOSTICS_REQUEST_ID_MAX_CHARS + 1),
        ] {
            let value = serde_json::json!({
                "diagnostics_version": RUNTIME_DIAGNOSTICS_VERSION,
                "request_id": request_id,
            });
            assert!(serde_json::from_value::<GetRuntimeDiagnosticsCommand>(value).is_err());
        }

        let value = serde_json::json!({
            "diagnostics_version": RUNTIME_DIAGNOSTICS_VERSION,
            "request_id": "x".repeat(RUNTIME_DIAGNOSTICS_REQUEST_ID_MAX_CHARS),
        });
        assert!(serde_json::from_value::<GetRuntimeDiagnosticsCommand>(value).is_ok());
    }
}
