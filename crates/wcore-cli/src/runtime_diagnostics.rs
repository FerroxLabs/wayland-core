//! Secret-safe projection of live Core state for local host diagnostics.

use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;

use wcore_agent::bootstrap::PluginMcpDeclaration;
use wcore_agent::mcp_lifecycle::{McpLifecycleCatalog, McpLifecycleSnapshot, McpLifecycleState};
use wcore_config::config::{Config, McpServerConfig, TransportType};
use wcore_config::resolution_provenance::{
    ConfigResolutionProvenance, ConfigSourceDisposition as EvidenceDisposition,
    ConfigSourceRole as EvidenceRole, LaunchBindingEvidence,
};
use wcore_config::shell::LaunchValueSource;
use wcore_mcp::manager::{McpManager, McpServerHealth};
use wcore_mcp::transport::stdio_readiness::{
    McpStdioExecutableReadiness, McpStdioExecutableReadinessStatus,
};
use wcore_protocol::diagnostics::{
    ConfigSourceDisposition, ConfigSourceRole, McpConnectionState, McpDeclarationOrigin,
    McpExecutableReadiness, McpExposureState, McpFailureCode, McpServerDiagnostic,
    McpTransportKind, McpWorkingDirectoryRole, RuntimeConfigSource, RuntimeDiagnosticsSnapshotV1,
    RuntimeEngineMode, RuntimeProcessBinding, RuntimeProfileBinding, RuntimeRemediationCode,
    RuntimeWorkspaceKind, UnsupportedConfigOverride,
};
use wcore_tools::registry::ToolRegistry;

#[derive(Clone, Debug)]
struct McpDeclaration {
    name: String,
    origin: McpDeclarationOrigin,
    transport: McpTransportKind,
    deferred: bool,
    assistant_scoped: bool,
    visible_to_assistant: bool,
    executable_basename: Option<String>,
    executable_readiness: McpExecutableReadiness,
    path_source: LaunchValueSource,
}

/// Immutable launch evidence plus redacted declarations. Arguments,
/// environment values, headers, URLs, and free-text errors are never retained;
/// the configured executable basename remains as a local diagnostic identifier.
pub struct RuntimeDiagnosticsState {
    process: RuntimeProcessBinding,
    config_sources: Vec<RuntimeConfigSource>,
    unsupported_overrides: Vec<UnsupportedConfigOverride>,
    declarations: HashMap<(McpDeclarationOrigin, String), McpDeclaration>,
}

impl RuntimeDiagnosticsState {
    pub fn from_launch(
        config: &Config,
        provenance: &ConfigResolutionProvenance,
        active_assistant: Option<&str>,
        engine_mode: RuntimeEngineMode,
        workspace_kind: RuntimeWorkspaceKind,
    ) -> Self {
        let (profile_binding, profile_name) = match &provenance.launch_binding {
            LaunchBindingEvidence::BoundProfile { name, .. } => {
                (RuntimeProfileBinding::BoundProfile, Some(name.clone()))
            }
            LaunchBindingEvidence::UnboundProfile { name, .. } => {
                (RuntimeProfileBinding::UnboundProfile, Some(name.clone()))
            }
            LaunchBindingEvidence::ExplicitWaylandHome => {
                (RuntimeProfileBinding::ExplicitHome, None)
            }
            LaunchBindingEvidence::DefaultHome => (RuntimeProfileBinding::DefaultHome, None),
            LaunchBindingEvidence::Unavailable => (RuntimeProfileBinding::Unknown, None),
        };

        let mut state = Self {
            process: RuntimeProcessBinding {
                profile_binding,
                profile_name,
                engine_mode,
                workspace_kind,
            },
            config_sources: project_config_sources(provenance),
            unsupported_overrides: project_unsupported_overrides(provenance),
            declarations: HashMap::new(),
        };
        for (name, server) in &config.mcp.servers {
            state.insert_declaration(
                name,
                server,
                McpDeclarationOrigin::EffectiveConfig,
                server.is_visible_to_assistant(active_assistant),
            );
        }
        state
    }

    /// Record a host-added declaration without retaining its args, env, URL,
    /// headers, or free-text errors.
    pub fn record_runtime_declaration(&mut self, name: &str, server: &McpServerConfig) -> bool {
        if self.declarations.values().any(|existing| {
            existing.name == name && existing.origin != McpDeclarationOrigin::RuntimeCommand
        }) {
            return false;
        }
        self.insert_declaration(name, server, McpDeclarationOrigin::RuntimeCommand, true);
        true
    }

    pub fn record_plugin_declarations(&mut self, declarations: &[PluginMcpDeclaration]) {
        for declaration in declarations {
            let (executable_readiness, path_source) = declaration
                .executable_readiness
                .map(|evidence| (project_readiness(evidence.status), evidence.path_source))
                .unwrap_or((
                    McpExecutableReadiness::NotApplicable,
                    LaunchValueSource::Unavailable,
                ));
            let server = McpDeclaration {
                name: declaration.name.clone(),
                origin: McpDeclarationOrigin::Plugin,
                transport: transport_kind(&declaration.transport),
                deferred: false,
                assistant_scoped: false,
                visible_to_assistant: true,
                executable_basename: None,
                executable_readiness,
                path_source,
            };
            self.declarations.insert(
                (McpDeclarationOrigin::Plugin, declaration.name.clone()),
                server,
            );
        }
    }

    fn insert_declaration(
        &mut self,
        name: &str,
        server: &McpServerConfig,
        origin: McpDeclarationOrigin,
        visible_to_assistant: bool,
    ) {
        self.declarations.insert(
            (origin, name.to_string()),
            McpDeclaration {
                name: name.to_string(),
                origin,
                transport: transport_kind(&server.transport),
                deferred: server.deferred.unwrap_or(true),
                assistant_scoped: server
                    .only_for_assistant
                    .as_ref()
                    .is_some_and(|names| !names.is_empty()),
                visible_to_assistant,
                executable_basename: server.command.as_deref().and_then(executable_basename),
                executable_readiness: if server.transport == TransportType::Stdio {
                    McpExecutableReadiness::Unchecked
                } else {
                    McpExecutableReadiness::NotApplicable
                },
                path_source: LaunchValueSource::Unavailable,
            },
        );
    }

    /// Attach secret-free readiness evidence produced from the exact launch
    /// environment. The declaration remains authoritative for origin and
    /// transport; unknown names cannot create diagnostic rows.
    pub fn record_executable_readiness(
        &mut self,
        origin: McpDeclarationOrigin,
        name: &str,
        evidence: McpStdioExecutableReadiness,
    ) -> bool {
        let Some(declaration) = self.declarations.get_mut(&(origin, name.to_owned())) else {
            return false;
        };
        if declaration.transport != McpTransportKind::Stdio || !declaration.visible_to_assistant {
            return false;
        }
        declaration.executable_readiness = project_readiness(evidence.status);
        declaration.path_source = evidence.path_source;
        true
    }

    pub fn snapshot(
        &self,
        lifecycle: &McpLifecycleCatalog,
        managers: &[Arc<McpManager>],
        registry: &ToolRegistry,
    ) -> RuntimeDiagnosticsSnapshotV1 {
        let tool_counts = registry.mcp_tool_counts();
        let mut servers: Vec<_> = self
            .declarations
            .values()
            .map(|declaration| {
                let origin_collision = self.declarations.values().any(|other| {
                    other.name == declaration.name && other.origin != declaration.origin
                });
                project_server(
                    declaration,
                    lifecycle.snapshot(&declaration.name),
                    managers,
                    &tool_counts,
                    origin_collision,
                )
            })
            .collect();
        servers.sort_by(|left, right| {
            left.name
                .cmp(&right.name)
                .then_with(|| origin_rank(left.origin).cmp(&origin_rank(right.origin)))
        });

        RuntimeDiagnosticsSnapshotV1 {
            process: self.process.clone(),
            config_sources: self.config_sources.clone(),
            unsupported_overrides: self.unsupported_overrides.clone(),
            mcp_servers: servers,
        }
    }
}

fn origin_rank(origin: McpDeclarationOrigin) -> u8 {
    match origin {
        McpDeclarationOrigin::EffectiveConfig => 0,
        McpDeclarationOrigin::GlobalConfig => 1,
        McpDeclarationOrigin::ProjectConfig => 2,
        McpDeclarationOrigin::ProfileConfig => 3,
        McpDeclarationOrigin::RuntimeCommand => 4,
        McpDeclarationOrigin::Plugin => 5,
    }
}

fn project_server(
    declaration: &McpDeclaration,
    lifecycle: Option<McpLifecycleSnapshot>,
    managers: &[Arc<McpManager>],
    tool_counts: &HashMap<String, u32>,
    origin_collision: bool,
) -> McpServerDiagnostic {
    let (health, manager_readiness) = managers
        .iter()
        .filter_map(|manager| {
            manager.health().get(&declaration.name).map(|health| {
                (
                    health,
                    manager
                        .executable_readiness()
                        .get(&declaration.name)
                        .copied(),
                )
            })
        })
        .max_by_key(|(health, _)| health_rank(health))
        .map_or((None, None), |(health, readiness)| {
            (Some(health), readiness)
        });
    let resources_declared = managers
        .iter()
        .any(|manager| manager.server_supports_resources(&declaration.name));
    // Deferred and runtime-added JSON-stream MCP resources are not loaded into
    // the active skill catalog after bootstrap. A server capability is not
    // evidence that its resources are exposed to this running agent.
    let resources_exposed = false;
    let tool_count = tool_counts.get(&declaration.name).copied().unwrap_or(0);

    let (connection, mut failure) = if origin_collision {
        (
            McpConnectionState::Skipped,
            Some(McpFailureCode::InvalidConfiguration),
        )
    } else if !declaration.visible_to_assistant {
        (McpConnectionState::Skipped, None)
    } else if let Some(snapshot) = lifecycle {
        match snapshot.state {
            McpLifecycleState::Connecting => (McpConnectionState::Connecting, None),
            McpLifecycleState::Ready => (McpConnectionState::Ready, None),
            McpLifecycleState::Failed { .. } => {
                (McpConnectionState::Failed, Some(McpFailureCode::Unknown))
            }
            McpLifecycleState::Stopping => (McpConnectionState::Stopping, None),
        }
    } else {
        match health {
            Some(McpServerHealth::Ready { .. }) => (McpConnectionState::Ready, None),
            Some(McpServerHealth::TimedOut { .. }) => {
                (McpConnectionState::TimedOut, Some(McpFailureCode::Timeout))
            }
            Some(McpServerHealth::Failed { .. }) => {
                (McpConnectionState::Failed, Some(McpFailureCode::Unknown))
            }
            Some(McpServerHealth::Skipped { .. }) => (
                McpConnectionState::Skipped,
                Some(McpFailureCode::InvalidConfiguration),
            ),
            None => (McpConnectionState::Configured, None),
        }
    };

    let exposure = if !declaration.visible_to_assistant {
        McpExposureState::Blocked
    } else if connection == McpConnectionState::Ready {
        if tool_count > 0 {
            McpExposureState::Exposed
        } else if resources_declared && resources_exposed {
            McpExposureState::ResourceOnly
        } else if resources_declared {
            McpExposureState::ResourceOnlyUnavailable
        } else {
            McpExposureState::HiddenNoTools
        }
    } else if matches!(
        connection,
        McpConnectionState::Failed | McpConnectionState::TimedOut | McpConnectionState::Skipped
    ) {
        McpExposureState::Blocked
    } else {
        McpExposureState::NotAttempted
    };

    // A completed MCP handshake is stronger evidence than the advisory,
    // TOCTOU-sensitive filesystem readiness probe. Never tell an operator to
    // install or repair an executable that this process already launched and
    // negotiated successfully.
    let advisory_readiness = manager_readiness
        .map(|evidence| project_readiness(evidence.status))
        .unwrap_or(declaration.executable_readiness);
    let path_source = manager_readiness
        .map(|evidence| evidence.path_source)
        .unwrap_or(declaration.path_source);
    let executable_readiness = if connection == McpConnectionState::Ready
        && declaration.transport == McpTransportKind::Stdio
    {
        McpExecutableReadiness::Resolved
    } else {
        advisory_readiness
    };

    if failure.is_none()
        && connection != McpConnectionState::Ready
        && declaration.visible_to_assistant
    {
        failure = readiness_failure(executable_readiness);
    }

    let mut remediation = Vec::new();
    if origin_collision {
        remediation.push(RuntimeRemediationCode::ReviewServerConfig);
    } else if !declaration.visible_to_assistant {
        remediation.push(RuntimeRemediationCode::CheckAssistantScope);
    } else {
        match connection {
            McpConnectionState::Failed | McpConnectionState::TimedOut => {
                remediation.push(RuntimeRemediationCode::RetryConnection);
                remediation.push(RuntimeRemediationCode::ReviewServerConfig);
            }
            McpConnectionState::Skipped => {
                remediation.push(RuntimeRemediationCode::ReviewServerConfig);
            }
            _ => {}
        }
    }
    if exposure == McpExposureState::ResourceOnlyUnavailable {
        remediation.push(RuntimeRemediationCode::RestartToLoadResources);
    }
    append_readiness_remediation(&mut remediation, executable_readiness, path_source);
    let mut deduplicated = Vec::with_capacity(remediation.len());
    for code in remediation {
        if !deduplicated.contains(&code) {
            deduplicated.push(code);
        }
    }

    McpServerDiagnostic {
        name: declaration.name.clone(),
        origin: declaration.origin,
        transport: declaration.transport,
        connection,
        exposure,
        deferred: declaration.deferred,
        tool_count,
        resources_declared,
        resources_exposed,
        assistant_scoped: declaration.assistant_scoped,
        executable_basename: declaration.executable_basename.clone(),
        executable_readiness,
        working_directory: McpWorkingDirectoryRole::InheritedProcess,
        failure,
        remediation: deduplicated,
    }
}

fn project_readiness(status: McpStdioExecutableReadinessStatus) -> McpExecutableReadiness {
    match status {
        McpStdioExecutableReadinessStatus::Resolved => McpExecutableReadiness::Resolved,
        McpStdioExecutableReadinessStatus::InvalidExecutable => {
            McpExecutableReadiness::InvalidExecutable
        }
        McpStdioExecutableReadinessStatus::MissingEffectivePath => {
            McpExecutableReadiness::MissingEffectivePath
        }
        McpStdioExecutableReadinessStatus::InvalidEffectiveEnvironment => {
            McpExecutableReadiness::InvalidEffectiveEnvironment
        }
        McpStdioExecutableReadinessStatus::PermissionDenied => {
            McpExecutableReadiness::PermissionDenied
        }
        McpStdioExecutableReadinessStatus::NotExecutable => McpExecutableReadiness::NotExecutable,
        McpStdioExecutableReadinessStatus::NotFound => McpExecutableReadiness::NotFound,
        McpStdioExecutableReadinessStatus::ProbeTimedOut => McpExecutableReadiness::ProbeTimedOut,
        McpStdioExecutableReadinessStatus::Unchecked => McpExecutableReadiness::Unchecked,
    }
}

fn readiness_failure(readiness: McpExecutableReadiness) -> Option<McpFailureCode> {
    match readiness {
        McpExecutableReadiness::MissingEffectivePath | McpExecutableReadiness::NotFound => {
            Some(McpFailureCode::MissingExecutable)
        }
        McpExecutableReadiness::InvalidAbsolutePath
        | McpExecutableReadiness::InvalidExecutable
        | McpExecutableReadiness::InvalidEffectiveEnvironment => {
            Some(McpFailureCode::InvalidConfiguration)
        }
        McpExecutableReadiness::PermissionDenied | McpExecutableReadiness::NotExecutable => {
            Some(McpFailureCode::LaunchFailed)
        }
        McpExecutableReadiness::NotApplicable
        | McpExecutableReadiness::Unchecked
        | McpExecutableReadiness::Resolved
        | McpExecutableReadiness::ProbeTimedOut
        | McpExecutableReadiness::UnsupportedTransport => None,
    }
}

fn append_readiness_remediation(
    remediation: &mut Vec<RuntimeRemediationCode>,
    readiness: McpExecutableReadiness,
    path_source: LaunchValueSource,
) {
    match readiness {
        McpExecutableReadiness::MissingEffectivePath => match path_source {
            LaunchValueSource::ExplicitServer => {
                remediation.push(RuntimeRemediationCode::OpenActiveConfig);
                remediation.push(RuntimeRemediationCode::ReviewServerConfig);
            }
            LaunchValueSource::InheritedAllowlist | LaunchValueSource::Unavailable => {
                remediation.push(RuntimeRemediationCode::FixGuiLaunchPath);
                remediation.push(RuntimeRemediationCode::RestartDesktop);
            }
        },
        McpExecutableReadiness::NotFound => {
            remediation.push(RuntimeRemediationCode::InstallExecutable);
            match path_source {
                LaunchValueSource::ExplicitServer => {
                    remediation.push(RuntimeRemediationCode::OpenActiveConfig);
                    remediation.push(RuntimeRemediationCode::ReviewServerConfig);
                }
                LaunchValueSource::InheritedAllowlist | LaunchValueSource::Unavailable => {
                    remediation.push(RuntimeRemediationCode::FixGuiLaunchPath);
                    remediation.push(RuntimeRemediationCode::RestartDesktop);
                }
            }
        }
        McpExecutableReadiness::InvalidAbsolutePath
        | McpExecutableReadiness::InvalidExecutable
        | McpExecutableReadiness::InvalidEffectiveEnvironment => {
            remediation.push(RuntimeRemediationCode::OpenActiveConfig);
            remediation.push(RuntimeRemediationCode::ReviewServerConfig);
        }
        McpExecutableReadiness::PermissionDenied | McpExecutableReadiness::NotExecutable => {
            remediation.push(RuntimeRemediationCode::FixExecutablePermissions);
            remediation.push(RuntimeRemediationCode::ReviewServerConfig);
        }
        McpExecutableReadiness::ProbeTimedOut => {
            remediation.push(RuntimeRemediationCode::RetryDiagnostics);
        }
        McpExecutableReadiness::NotApplicable
        | McpExecutableReadiness::Unchecked
        | McpExecutableReadiness::Resolved
        | McpExecutableReadiness::UnsupportedTransport => {}
    }
}

fn health_rank(health: &McpServerHealth) -> u8 {
    match health {
        McpServerHealth::Ready { .. } => 4,
        McpServerHealth::TimedOut { .. } => 3,
        McpServerHealth::Failed { .. } => 2,
        McpServerHealth::Skipped { .. } => 1,
    }
}

fn executable_basename(command: &str) -> Option<String> {
    let basename = command
        .rsplit(['/', '\\'])
        .next()
        .filter(|part| !part.is_empty())?;
    // `command` is operator configuration, not a trusted argv token. A common
    // malformed form puts inline args or credentials in this field; retaining
    // that text would violate the diagnostics redaction contract. Publish only
    // a conservative executable identifier and fail closed for everything else.
    if basename.len() > 255
        || !basename
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-' | b'+'))
    {
        return None;
    }
    Path::new(basename)
        .file_name()
        .and_then(|name| name.to_str())
        .map(str::to_owned)
}

fn transport_kind(transport: &TransportType) -> McpTransportKind {
    match transport {
        TransportType::Stdio => McpTransportKind::Stdio,
        TransportType::Sse => McpTransportKind::Sse,
        TransportType::StreamableHttp => McpTransportKind::StreamableHttp,
    }
}

fn project_config_sources(provenance: &ConfigResolutionProvenance) -> Vec<RuntimeConfigSource> {
    let mut sources = Vec::new();
    for source in &provenance.sources {
        for disposition in &source.dispositions {
            sources.push(RuntimeConfigSource {
                role: source_role(&source.role),
                disposition: source_disposition(*disposition),
                precedence: source.precedence,
                display_path: source.path.as_ref().map(|path| path.display().to_string()),
                content_digest: None,
            });
        }
    }
    sources.sort_by(|left, right| {
        left.precedence
            .cmp(&right.precedence)
            .then_with(|| left.display_path.cmp(&right.display_path))
            .then_with(|| {
                format!("{:?}", left.disposition).cmp(&format!("{:?}", right.disposition))
            })
    });
    sources
}

fn project_unsupported_overrides(
    provenance: &ConfigResolutionProvenance,
) -> Vec<UnsupportedConfigOverride> {
    let mut overrides: Vec<_> = provenance
        .sources
        .iter()
        .filter_map(|source| {
            let EvidenceRole::EnvironmentOverride { variable } = &source.role else {
                return None;
            };
            let disposition = source
                .dispositions
                .iter()
                .find(|disposition| {
                    matches!(
                        disposition,
                        EvidenceDisposition::Ignored | EvidenceDisposition::Restricted
                    )
                })
                .copied()?;
            Some(UnsupportedConfigOverride {
                name: variable.clone(),
                disposition: source_disposition(disposition),
            })
        })
        .collect();
    overrides.sort_by(|left, right| left.name.cmp(&right.name));
    overrides
}

fn source_role(role: &EvidenceRole) -> ConfigSourceRole {
    match role {
        EvidenceRole::Global => ConfigSourceRole::Global,
        EvidenceRole::Project => ConfigSourceRole::Project,
        EvidenceRole::Profile => ConfigSourceRole::Profile,
        EvidenceRole::CliOverrides => ConfigSourceRole::Cli,
        EvidenceRole::EnvironmentOverride { .. } => ConfigSourceRole::Environment,
    }
}

fn source_disposition(disposition: EvidenceDisposition) -> ConfigSourceDisposition {
    match disposition {
        EvidenceDisposition::Loaded => ConfigSourceDisposition::Loaded,
        EvidenceDisposition::Absent => ConfigSourceDisposition::Absent,
        EvidenceDisposition::Ignored => ConfigSourceDisposition::Ignored,
        EvidenceDisposition::Unreadable => ConfigSourceDisposition::Unreadable,
        EvidenceDisposition::Invalid => ConfigSourceDisposition::Invalid,
        EvidenceDisposition::Overridden => ConfigSourceDisposition::Overridden,
        EvidenceDisposition::Restricted => ConfigSourceDisposition::Restricted,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use async_trait::async_trait;
    use serde_json::{Value, json};
    use wcore_config::resolution_provenance::ConfigSourceEvidence;
    use wcore_mcp::protocol::{JsonRpcRequest, JsonRpcResponse};
    use wcore_mcp::transport::{McpError, McpTransport};
    use wcore_protocol::events::ToolCategory;
    use wcore_tools::Tool;
    use wcore_types::tool::{JsonSchema, ToolResult};

    fn stdio_server(command: &str, scope: Option<Vec<String>>) -> McpServerConfig {
        McpServerConfig {
            transport: TransportType::Stdio,
            command: Some(command.to_string()),
            args: Some(vec!["--token=ARG_SECRET".to_string()]),
            env: Some(HashMap::from([(
                "TOKEN".to_string(),
                "ENV_SECRET".to_string(),
            )])),
            url: None,
            headers: Some(HashMap::from([(
                "Authorization".to_string(),
                "HEADER_SECRET".to_string(),
            )])),
            deferred: Some(false),
            allow_local: false,
            only_for_assistant: scope,
        }
    }

    fn http_server(url: &str) -> McpServerConfig {
        McpServerConfig {
            transport: TransportType::StreamableHttp,
            command: None,
            args: None,
            env: None,
            url: Some(url.to_string()),
            headers: Some(HashMap::from([(
                "X-Token".to_string(),
                "HTTP_HEADER_SECRET".to_string(),
            )])),
            deferred: Some(true),
            allow_local: false,
            only_for_assistant: None,
        }
    }

    fn provenance() -> ConfigResolutionProvenance {
        ConfigResolutionProvenance {
            sources: vec![ConfigSourceEvidence {
                role: EvidenceRole::Project,
                path: Some("/workspace/.wayland-core.toml".into()),
                precedence: 20,
                dispositions: vec![EvidenceDisposition::Loaded, EvidenceDisposition::Restricted],
            }],
            launch_binding: LaunchBindingEvidence::DefaultHome,
        }
    }

    fn state_with_servers(
        servers: impl IntoIterator<Item = (String, McpServerConfig)>,
        assistant: Option<&str>,
    ) -> RuntimeDiagnosticsState {
        let mut config = Config::default();
        config.mcp.servers = servers.into_iter().collect();
        RuntimeDiagnosticsState::from_launch(
            &config,
            &provenance(),
            assistant,
            RuntimeEngineMode::Unknown,
            RuntimeWorkspaceKind::Unknown,
        )
    }

    #[test]
    fn snapshot_is_secret_safe_and_keeps_scoped_servers_visible() {
        let state = state_with_servers(
            [
                (
                    "global".to_string(),
                    stdio_server("/usr/local/bin/global-mcp", None),
                ),
                (
                    "scoped".to_string(),
                    stdio_server(
                        "C:\\tools\\scoped-mcp.exe",
                        Some(vec!["concierge".to_string()]),
                    ),
                ),
                (
                    "remote".to_string(),
                    http_server("https://example.invalid/mcp?token=URL_SECRET"),
                ),
            ],
            Some("other"),
        );
        let snapshot = state.snapshot(&McpLifecycleCatalog::new(), &[], &ToolRegistry::new());
        let encoded = serde_json::to_string(&snapshot).unwrap();

        for secret in [
            "ARG_SECRET",
            "ENV_SECRET",
            "HEADER_SECRET",
            "HTTP_HEADER_SECRET",
            "URL_SECRET",
            "Authorization",
        ] {
            assert!(!encoded.contains(secret), "leaked {secret}: {encoded}");
        }
        assert_eq!(snapshot.config_sources.len(), 2);
        assert_eq!(snapshot.mcp_servers[0].name, "global");
        assert_eq!(
            snapshot.mcp_servers[2].connection,
            McpConnectionState::Skipped
        );
        assert_eq!(snapshot.mcp_servers[2].exposure, McpExposureState::Blocked);
        assert_eq!(
            snapshot.mcp_servers[2].remediation,
            vec![RuntimeRemediationCode::CheckAssistantScope]
        );
        assert_eq!(
            snapshot.mcp_servers[2].executable_basename.as_deref(),
            Some("scoped-mcp.exe")
        );
    }

    struct NoopTransport;

    #[async_trait]
    impl McpTransport for NoopTransport {
        async fn request(&self, _req: &JsonRpcRequest) -> Result<JsonRpcResponse, McpError> {
            unreachable!("runtime diagnostics must not probe transports")
        }

        async fn notify(&self, _req: &JsonRpcRequest) -> Result<(), McpError> {
            unreachable!("runtime diagnostics must not probe transports")
        }

        async fn close(&self) -> Result<(), McpError> {
            unreachable!("runtime diagnostics must not mutate transports")
        }
    }

    #[test]
    fn declared_resources_are_not_falsely_reported_as_exposed() {
        let state = state_with_servers(
            [
                ("resources".to_string(), stdio_server("resource-mcp", None)),
                ("stopping".to_string(), stdio_server("stop-mcp", None)),
            ],
            None,
        );
        let manager = Arc::new(McpManager::new_for_test(vec![(
            "resources",
            true,
            Box::new(NoopTransport),
        )]));
        let lifecycle = McpLifecycleCatalog::new();
        lifecycle.seed_ready(
            "stopping",
            wcore_agent::mcp_lifecycle::McpConfigIdentity::UNKNOWN,
        );
        assert!(lifecycle.mark_stopping("stopping"));

        let snapshot = state.snapshot(&lifecycle, &[manager], &ToolRegistry::new());
        assert_eq!(snapshot.mcp_servers[0].name, "resources");
        assert_eq!(
            snapshot.mcp_servers[0].connection,
            McpConnectionState::Ready
        );
        assert_eq!(
            snapshot.mcp_servers[0].exposure,
            McpExposureState::ResourceOnlyUnavailable
        );
        assert!(snapshot.mcp_servers[0].resources_declared);
        assert!(!snapshot.mcp_servers[0].resources_exposed);
        assert_eq!(
            snapshot.mcp_servers[1].connection,
            McpConnectionState::Stopping
        );
    }

    struct FakeMcpTool;

    #[async_trait]
    impl Tool for FakeMcpTool {
        fn name(&self) -> &str {
            "registered"
        }

        fn description(&self) -> &str {
            "test"
        }

        fn input_schema(&self) -> JsonSchema {
            json!({"type": "object"})
        }

        fn is_concurrency_safe(&self, _input: &Value) -> bool {
            true
        }

        async fn execute(&self, _input: Value) -> ToolResult {
            ToolResult {
                content: String::new(),
                is_error: false,
            }
        }

        fn category(&self) -> ToolCategory {
            ToolCategory::Mcp
        }

        fn mcp_server(&self) -> Option<&str> {
            Some("server")
        }
    }

    #[test]
    fn registered_tool_count_wins_over_discovery_count_and_failure_text_is_redacted() {
        let state = state_with_servers(
            [
                ("server".to_string(), stdio_server("server-mcp", None)),
                ("failed".to_string(), stdio_server("failed-mcp", None)),
            ],
            None,
        );
        let manager = Arc::new(McpManager::new_for_test_with_health(vec![
            ("server", McpServerHealth::Ready { tool_count: 99 }),
            (
                "failed",
                McpServerHealth::Failed {
                    reason: "FAILURE_SECRET".to_string(),
                },
            ),
        ]));
        let mut registry = ToolRegistry::new();
        registry.register(Box::new(FakeMcpTool));

        let snapshot = state.snapshot(&McpLifecycleCatalog::new(), &[manager], &registry);
        let server = snapshot
            .mcp_servers
            .iter()
            .find(|server| server.name == "server")
            .unwrap();
        assert_eq!(server.tool_count, 1);
        let failed = snapshot
            .mcp_servers
            .iter()
            .find(|server| server.name == "failed")
            .unwrap();
        assert_eq!(failed.failure, Some(McpFailureCode::Unknown));
        assert!(
            !serde_json::to_string(&snapshot)
                .unwrap()
                .contains("FAILURE_SECRET")
        );
    }

    #[test]
    fn declaration_insertion_order_does_not_change_serialized_snapshot() {
        let first = state_with_servers(
            [
                ("zeta".to_string(), stdio_server("zeta", None)),
                ("alpha".to_string(), stdio_server("alpha", None)),
            ],
            None,
        );
        let second = state_with_servers(
            [
                ("alpha".to_string(), stdio_server("alpha", None)),
                ("zeta".to_string(), stdio_server("zeta", None)),
            ],
            None,
        );
        let lifecycle = McpLifecycleCatalog::new();
        let registry = ToolRegistry::new();
        let first = serde_json::to_vec(&first.snapshot(&lifecycle, &[], &registry)).unwrap();
        let second = serde_json::to_vec(&second.snapshot(&lifecycle, &[], &registry)).unwrap();
        assert_eq!(first, second);
    }

    #[test]
    fn runtime_declaration_cannot_overwrite_effective_config_origin() {
        let mut state = state_with_servers(
            [("same".to_string(), stdio_server("configured", None))],
            None,
        );
        assert!(!state.record_runtime_declaration("same", &stdio_server("runtime", None)));
        assert!(state.record_runtime_declaration("new", &stdio_server("runtime", None)));

        let snapshot = state.snapshot(&McpLifecycleCatalog::new(), &[], &ToolRegistry::new());
        let configured = snapshot
            .mcp_servers
            .iter()
            .find(|server| server.name == "same")
            .unwrap();
        assert_eq!(configured.origin, McpDeclarationOrigin::EffectiveConfig);
        let runtime = snapshot
            .mcp_servers
            .iter()
            .find(|server| server.name == "new")
            .unwrap();
        assert_eq!(runtime.origin, McpDeclarationOrigin::RuntimeCommand);
    }

    #[test]
    fn readiness_mapping_and_remediation_are_exhaustive_and_secret_safe() {
        use McpExecutableReadiness as Projected;
        use McpFailureCode as Failure;
        use McpStdioExecutableReadinessStatus as Inspected;
        use RuntimeRemediationCode as Remediation;

        let cases = [
            (
                "resolved",
                Inspected::Resolved,
                LaunchValueSource::InheritedAllowlist,
                Projected::Resolved,
                None,
                vec![],
            ),
            (
                "invalid-executable",
                Inspected::InvalidExecutable,
                LaunchValueSource::InheritedAllowlist,
                Projected::InvalidExecutable,
                Some(Failure::InvalidConfiguration),
                vec![
                    Remediation::OpenActiveConfig,
                    Remediation::ReviewServerConfig,
                ],
            ),
            (
                "missing-inherited-path",
                Inspected::MissingEffectivePath,
                LaunchValueSource::InheritedAllowlist,
                Projected::MissingEffectivePath,
                Some(Failure::MissingExecutable),
                vec![Remediation::FixGuiLaunchPath, Remediation::RestartDesktop],
            ),
            (
                "missing-explicit-path",
                Inspected::MissingEffectivePath,
                LaunchValueSource::ExplicitServer,
                Projected::MissingEffectivePath,
                Some(Failure::MissingExecutable),
                vec![
                    Remediation::OpenActiveConfig,
                    Remediation::ReviewServerConfig,
                ],
            ),
            (
                "invalid-environment",
                Inspected::InvalidEffectiveEnvironment,
                LaunchValueSource::InheritedAllowlist,
                Projected::InvalidEffectiveEnvironment,
                Some(Failure::InvalidConfiguration),
                vec![
                    Remediation::OpenActiveConfig,
                    Remediation::ReviewServerConfig,
                ],
            ),
            (
                "permission-denied",
                Inspected::PermissionDenied,
                LaunchValueSource::InheritedAllowlist,
                Projected::PermissionDenied,
                Some(Failure::LaunchFailed),
                vec![
                    Remediation::FixExecutablePermissions,
                    Remediation::ReviewServerConfig,
                ],
            ),
            (
                "not-executable",
                Inspected::NotExecutable,
                LaunchValueSource::InheritedAllowlist,
                Projected::NotExecutable,
                Some(Failure::LaunchFailed),
                vec![
                    Remediation::FixExecutablePermissions,
                    Remediation::ReviewServerConfig,
                ],
            ),
            (
                "not-found-inherited",
                Inspected::NotFound,
                LaunchValueSource::InheritedAllowlist,
                Projected::NotFound,
                Some(Failure::MissingExecutable),
                vec![
                    Remediation::InstallExecutable,
                    Remediation::FixGuiLaunchPath,
                    Remediation::RestartDesktop,
                ],
            ),
            (
                "not-found-explicit",
                Inspected::NotFound,
                LaunchValueSource::ExplicitServer,
                Projected::NotFound,
                Some(Failure::MissingExecutable),
                vec![
                    Remediation::InstallExecutable,
                    Remediation::OpenActiveConfig,
                    Remediation::ReviewServerConfig,
                ],
            ),
            (
                "probe-timed-out",
                Inspected::ProbeTimedOut,
                LaunchValueSource::InheritedAllowlist,
                Projected::ProbeTimedOut,
                None,
                vec![Remediation::RetryDiagnostics],
            ),
            (
                "unchecked",
                Inspected::Unchecked,
                LaunchValueSource::Unavailable,
                Projected::Unchecked,
                None,
                vec![],
            ),
        ];

        for (label, inspected, path_source, projected, failure, remediation) in cases {
            let mut state = state_with_servers(
                [("server".to_string(), stdio_server("server-mcp", None))],
                None,
            );
            assert!(state.record_executable_readiness(
                McpDeclarationOrigin::EffectiveConfig,
                "server",
                McpStdioExecutableReadiness {
                    status: inspected,
                    path_source,
                    pathext_source: LaunchValueSource::Unavailable,
                },
            ));

            let snapshot = state.snapshot(&McpLifecycleCatalog::new(), &[], &ToolRegistry::new());
            let server = &snapshot.mcp_servers[0];
            assert_eq!(server.executable_readiness, projected, "{label}");
            assert_eq!(server.failure, failure, "{label}");
            assert_eq!(server.remediation, remediation, "{label}");
            assert!(
                !serde_json::to_string(&snapshot)
                    .unwrap()
                    .contains("ENV_SECRET"),
                "{label} retained launch environment"
            );
        }

        for (label, readiness, failure, remediation) in [
            ("not-applicable", Projected::NotApplicable, None, vec![]),
            (
                "invalid-absolute-path",
                Projected::InvalidAbsolutePath,
                Some(Failure::InvalidConfiguration),
                vec![
                    Remediation::OpenActiveConfig,
                    Remediation::ReviewServerConfig,
                ],
            ),
            (
                "unsupported-transport",
                Projected::UnsupportedTransport,
                None,
                vec![],
            ),
        ] {
            let mut actual_remediation = Vec::new();
            append_readiness_remediation(
                &mut actual_remediation,
                readiness,
                LaunchValueSource::Unavailable,
            );
            assert_eq!(readiness_failure(readiness), failure, "{label}");
            assert_eq!(actual_remediation, remediation, "{label}");
        }
    }

    #[test]
    fn successful_handshake_dominates_stale_advisory_readiness() {
        let mut state = state_with_servers(
            [("ready".to_string(), stdio_server("ready-mcp", None))],
            None,
        );
        assert!(state.record_executable_readiness(
            McpDeclarationOrigin::EffectiveConfig,
            "ready",
            McpStdioExecutableReadiness {
                status: McpStdioExecutableReadinessStatus::NotFound,
                path_source: LaunchValueSource::InheritedAllowlist,
                pathext_source: LaunchValueSource::Unavailable,
            },
        ));
        let manager = Arc::new(McpManager::new_for_test(vec![(
            "ready",
            false,
            Box::new(NoopTransport),
        )]));

        let snapshot = state.snapshot(
            &McpLifecycleCatalog::new(),
            &[manager],
            &ToolRegistry::new(),
        );
        let server = &snapshot.mcp_servers[0];
        assert_eq!(server.connection, McpConnectionState::Ready);
        assert_eq!(
            server.executable_readiness,
            McpExecutableReadiness::Resolved
        );
        assert_eq!(server.failure, None);
        assert!(server.remediation.is_empty());
    }

    #[test]
    fn executable_basename_rejects_inline_command_data() {
        let state = state_with_servers(
            [
                (
                    "inline".to_string(),
                    stdio_server("node --token=INLINE_COMMAND_SECRET", None),
                ),
                (
                    "shellish".to_string(),
                    stdio_server("node;INLINE_COMMAND_SECRET", None),
                ),
                (
                    "safe".to_string(),
                    stdio_server("/Applications/Wayland Tools/bin/safe-mcp.exe", None),
                ),
            ],
            None,
        );

        let snapshot = state.snapshot(&McpLifecycleCatalog::new(), &[], &ToolRegistry::new());
        let inline = snapshot
            .mcp_servers
            .iter()
            .find(|server| server.name == "inline")
            .unwrap();
        let shellish = snapshot
            .mcp_servers
            .iter()
            .find(|server| server.name == "shellish")
            .unwrap();
        let safe = snapshot
            .mcp_servers
            .iter()
            .find(|server| server.name == "safe")
            .unwrap();
        assert_eq!(inline.executable_basename, None);
        assert_eq!(shellish.executable_basename, None);
        assert_eq!(safe.executable_basename.as_deref(), Some("safe-mcp.exe"));
        assert!(
            !serde_json::to_string(&snapshot)
                .unwrap()
                .contains("INLINE_COMMAND_SECRET")
        );
    }

    #[test]
    fn plugin_declarations_are_visible_and_name_collisions_fail_closed() {
        let mut state = state_with_servers(
            [("same".to_string(), stdio_server("configured", None))],
            None,
        );
        state.record_plugin_declarations(&[
            PluginMcpDeclaration {
                name: "plugin-only".into(),
                transport: TransportType::StreamableHttp,
                executable_readiness: None,
            },
            PluginMcpDeclaration {
                name: "same".into(),
                transport: TransportType::Stdio,
                executable_readiness: None,
            },
        ]);

        let snapshot = state.snapshot(&McpLifecycleCatalog::new(), &[], &ToolRegistry::new());
        let plugin = snapshot
            .mcp_servers
            .iter()
            .find(|server| server.name == "plugin-only")
            .unwrap();
        assert_eq!(plugin.origin, McpDeclarationOrigin::Plugin);
        assert_eq!(plugin.transport, McpTransportKind::StreamableHttp);

        let collisions: Vec<_> = snapshot
            .mcp_servers
            .iter()
            .filter(|server| server.name == "same")
            .collect();
        assert_eq!(collisions.len(), 2);
        assert_eq!(collisions[0].origin, McpDeclarationOrigin::EffectiveConfig);
        assert_eq!(collisions[1].origin, McpDeclarationOrigin::Plugin);
        assert!(collisions.iter().all(|server| {
            server.connection == McpConnectionState::Skipped
                && server.exposure == McpExposureState::Blocked
                && server.failure == Some(McpFailureCode::InvalidConfiguration)
        }));
    }
}
