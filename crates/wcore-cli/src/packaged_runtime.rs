//! Shared construction seam for the packaged CLI runtime.
//!
//! The binary and packaged-process acceptance helpers must enter the agent
//! through this module. That keeps execution-policy provenance and statically
//! linked plugin inventory identical across both processes.

use wcore_agent::bootstrap::AgentBootstrap;
use wcore_config::config::Config;
use wcore_types::execution_policy::{
    ApprovalPolicy, BaselineExecutionPolicy, DangerousLaunchRequest, DangerousSessionGrant,
    PolicySource, resolve_dangerous_launch,
};

/// The baseline execution authority and optional locally minted dangerous
/// lease selected for one packaged process launch.
pub struct LocalExecutionSelection {
    baseline: BaselineExecutionPolicy,
    dangerous_grant: Option<DangerousSessionGrant>,
}

impl LocalExecutionSelection {
    pub fn approvals(&self) -> ApprovalPolicy {
        if self.dangerous_grant.is_some() {
            ApprovalPolicy::Bypass
        } else {
            self.baseline.approvals()
        }
    }

    pub fn baseline(&self) -> &BaselineExecutionPolicy {
        &self.baseline
    }

    pub fn dangerous_grant(&self) -> Option<&DangerousSessionGrant> {
        self.dangerous_grant.as_ref()
    }

    pub fn apply(self, bootstrap: AgentBootstrap) -> AgentBootstrap {
        let mut bootstrap = bootstrap.with_execution_policy(self.baseline);
        if let Some(grant) = self.dangerous_grant {
            bootstrap = bootstrap.with_dangerous_grant(grant);
        }
        bootstrap
    }
}

/// Keep every plugin compiled into the packaged binary linked when this seam
/// is reached through the `wcore-cli` library by an integration executable.
/// Referencing each submitted factory prevents the linker from discarding the
/// object that owns its `inventory::submit!` record.
fn link_packaged_plugins() {
    std::hint::black_box(&wayland_browser::WaylandBrowserFactory);
    std::hint::black_box(&wayland_cua::WaylandCuaFactory);
    std::hint::black_box(&wayland_honcho::WaylandHonchoFactory);
    std::hint::black_box(&wayland_ollama::WaylandOllamaFactory);
}

pub fn audit_unix_time_millis() -> anyhow::Result<u64> {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_err(|_| anyhow::anyhow!("system clock is before the Unix epoch"))?
        .as_millis()
        .try_into()
        .map_err(|_| anyhow::anyhow!("system clock does not fit execution-policy metadata"))
}

pub fn resolve_local_execution(
    config: &Config,
    approval_bypass: bool,
    dangerous: bool,
    dangerous_ttl_secs: u64,
    desktop_launch: bool,
) -> anyhow::Result<LocalExecutionSelection> {
    link_packaged_plugins();

    let requested = if approval_bypass {
        ApprovalPolicy::Bypass
    } else {
        config.smart_approval_policy()
    };
    let source = if desktop_launch {
        PolicySource::DesktopLocalLaunch
    } else {
        PolicySource::LocalCliLaunch
    };
    let baseline = config
        .execution_policy
        .with_requested_approvals(requested, source);
    let dangerous_grant = if dangerous {
        let activation_id = format!("{}-{}", std::process::id(), uuid::Uuid::now_v7());
        let request = if desktop_launch {
            DangerousLaunchRequest::desktop(dangerous_ttl_secs, activation_id)
        } else {
            DangerousLaunchRequest::cli(dangerous_ttl_secs, activation_id)
        };
        let now = audit_unix_time_millis()?;
        Some(resolve_dangerous_launch(&baseline, request, now)?)
    } else {
        None
    };

    Ok(LocalExecutionSelection {
        baseline,
        dangerous_grant,
    })
}
