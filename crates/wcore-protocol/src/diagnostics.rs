use serde::{Deserialize, Serialize};
use thiserror::Error;

pub const RUNTIME_DIAGNOSTICS_VERSION: u16 = 1;

/// Closed, versioned request for the process's effective runtime view.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GetRuntimeDiagnosticsCommand {
    pub diagnostics_version: u16,
    pub request_id: String,
}

/// Fail-closed version negotiation for runtime diagnostics.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Error)]
pub enum RuntimeDiagnosticsVersionError {
    #[error("unsupported runtime diagnostics version: {actual}")]
    UnsupportedVersion { actual: u16 },
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

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RuntimeWorkspaceKind {
    None,
    Project,
    Temporary,
    ProfileHome,
}

/// Identifies the process binding without exposing environment values.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RuntimeProcessBinding {
    pub profile_bound: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub profile_name: Option<String>,
    pub raw_engine_mode: bool,
    pub workspace_kind: RuntimeWorkspaceKind,
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

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum McpDeclarationOrigin {
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
    Stopped,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum McpExposureState {
    NotAttempted,
    NotApplicable,
    Exposed,
    HiddenNoTools,
    Blocked,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum McpExecutableReadiness {
    NotApplicable,
    Resolved,
    MissingEffectivePath,
    NotFound,
    InvalidAbsolutePath,
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
    ReviewServerConfig,
    RetryConnection,
    CheckAssistantScope,
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
