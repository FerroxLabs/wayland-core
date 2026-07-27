//! CLI surface: `wayland-core sandbox` — the platform-containment operator
//! surface (`status` / `exec`).
//!
//! WHY THIS EXISTS. Every shell command the agent runs is executed through
//! this host's platform containment backend (bubblewrap on Linux,
//! `sandbox-exec` on macOS, AppContainer on Windows). Until this surface
//! landed there was **no way — CLI or otherwise — for an operator to obtain
//! positive evidence that the sandbox was actually ACTIVE for an execution.**
//! Availability was reportable (`backend probe` says a backend "probed
//! available"), but availability is a claim about a probe, not about the
//! containment applied to a child. That gap is not academic: this product has
//! a recorded defect class in which the sandbox reports itself available and
//! is silently not applied (the Windows AppContainer lease wedge). A security
//! boundary an operator cannot verify is a boundary an operator cannot trust.
//!
//! `sandbox exec` closes that gap by running a caller-supplied command through
//! the containment path and returning the CHILD'S OWN output, so the caller can
//! compare what the command observes inside the sandbox against what the same
//! command observes outside it. Activeness is then asserted from the
//! DIFFERENCE — a positive observation — rather than from the absence of a
//! violation, which evidences nothing.
//!
//! THE ONE PROPERTY THAT MAKES THIS EVIDENCE RATHER THAN THEATRE, stated here
//! because it is the whole point and a future edit could silently destroy it:
//! this surface does NOT re-implement sandboxed execution. It builds the real
//! [`WorkspacePolicy`], selects the backend through the same fail-closed
//! [`SandboxRegistry::required_for_session`] an agent session uses, and then
//! dispatches through **`wcore_tools::bash::BashTool::execute_with_ctx` — the
//! agent's own shell tool, the same function, not a sibling path.** A
//! regression that stopped routing the agent's shell through containment would
//! therefore break this verb too. If a later change makes this verb construct
//! its own manifest or call a backend directly, the evidence stops being
//! transitive and this surface degrades into a test hook that proves only
//! itself.
//!
//! NOT A BYPASS. The selector is `required_for_session`, which refuses the
//! `none` backend outright (`WAYLAND_SANDBOX=none` is an error, not a
//! downgrade) and falls closed to `FailClosedBackend` when the platform offers
//! no real containment. The child receives strictly LESS authority than the
//! caller's own shell — deny-default filesystem scoped to the workspace, and
//! the `contained` profile's fail-safe network Deny. A caller who can run this
//! verb can already run the same command unsandboxed; this only ever removes
//! authority.

use std::path::PathBuf;
use std::sync::Arc;

use clap::{Args, Subcommand};
use tokio_util::sync::CancellationToken;
use wcore_sandbox::SandboxRegistry;
use wcore_tools::Tool;
use wcore_tools::context::ToolContext;
use wcore_tools::workspace_policy::WorkspacePolicy;

/// Arguments for `wayland-core sandbox`.
#[derive(Args, Debug)]
pub struct SandboxArgs {
    #[command(subcommand)]
    pub cmd: SandboxCmd,
}

#[derive(Subcommand, Debug)]
pub enum SandboxCmd {
    /// Report the platform containment backend selected for this host, and
    /// the containment properties it does and does not provide.
    Status {
        /// Emit a single JSON object instead of human-readable lines.
        #[arg(long)]
        json: bool,
    },
    /// Run a command inside this host's platform sandbox and print the
    /// child's own output.
    ///
    /// The command is executed through the agent's shell tool, so the
    /// containment applied is the containment the agent applies.
    Exec {
        /// Workspace root the sandbox is scoped to. Defaults to the current
        /// directory. The child may read and write here and, by construction
        /// of the `contained` profile, little else.
        #[arg(long)]
        workspace: Option<PathBuf>,

        /// Wall-clock budget for the child, in milliseconds.
        #[arg(long, default_value_t = 120_000)]
        timeout_ms: u64,

        /// The command to run, exactly as the agent's shell tool receives it.
        command: String,
    },
}

/// The containment properties of a selected backend, projected for reporting.
///
/// Every field is read back from the live registry. Nothing here is a
/// constant: a backend that changes its capabilities changes this projection.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SandboxStatus {
    pub backend: String,
    pub available: bool,
    pub bypasses_containment: bool,
    pub enforces_read_deny: bool,
    pub owns_descendants_hard: bool,
    pub binds_cwd_authority: bool,
    pub binds_workspace_authority: bool,
}

impl SandboxStatus {
    /// Project the live registry. Deliberately a pure read of the registry so
    /// the report cannot drift from the backend it describes.
    pub fn project(registry: &SandboxRegistry) -> Self {
        Self {
            backend: registry.backend_name().to_owned(),
            available: registry.is_available(),
            bypasses_containment: registry.bypasses_containment(),
            enforces_read_deny: registry.enforces_read_deny(),
            owns_descendants_hard: registry.owns_descendants_hard(),
            binds_cwd_authority: registry.binds_cwd_authority(),
            binds_workspace_authority: registry.binds_workspace_authority(),
        }
    }

    pub fn to_json(&self) -> serde_json::Value {
        serde_json::json!({
            "backend": self.backend,
            "available": self.available,
            "bypasses_containment": self.bypasses_containment,
            "enforces_read_deny": self.enforces_read_deny,
            "owns_descendants_hard": self.owns_descendants_hard,
            "binds_cwd_authority": self.binds_cwd_authority,
            "binds_workspace_authority": self.binds_workspace_authority,
        })
    }
}

/// Resolve the workspace root the sandbox will be scoped to.
///
/// Canonicalized, because the SBPL / bind-mount allowlists are path-prefix
/// matches: an uncanonicalized path that traverses a symlink would produce a
/// profile that does not cover the directory the child actually reaches.
fn resolve_workspace(workspace: Option<PathBuf>) -> anyhow::Result<PathBuf> {
    let raw = match workspace {
        Some(path) => path,
        None => std::env::current_dir()?,
    };
    std::fs::canonicalize(&raw)
        .map_err(|error| anyhow::anyhow!("sandbox workspace {}: {error}", raw.display()))
}

/// Build the tool context the sandboxed child runs under.
///
/// This is the same shape a hosted agent session builds: the strict
/// `contained` workspace profile, plus the registry-owned, containment-required
/// session runtime. Exposed (rather than inlined) so its properties are
/// directly assertable in tests.
pub fn sandbox_context(workspace: &std::path::Path, registry: Arc<SandboxRegistry>) -> ToolContext {
    let policy = Arc::new(WorkspacePolicy::contained(workspace));
    ToolContext::new(
        "sandbox-exec",
        CancellationToken::new(),
        Arc::new(wcore_tools::vfs::RealFs),
        None,
        Arc::new(wcore_tools::NullToolOutputSink),
    )
    .with_workspace(policy)
    .with_sandbox(registry)
}

/// Entry point for `wayland-core sandbox`.
pub async fn run_sandbox(args: SandboxArgs) -> anyhow::Result<()> {
    match args.cmd {
        SandboxCmd::Status { json } => run_status(json),
        SandboxCmd::Exec {
            workspace,
            timeout_ms,
            command,
        } => run_exec(workspace, timeout_ms, command).await,
    }
}

fn run_status(json: bool) -> anyhow::Result<()> {
    // Fail-closed selection, same as a session. A host with no real backend
    // reports `fail_closed` rather than a comforting absence of output.
    let registry = SandboxRegistry::required_for_session(None)
        .map_err(|error| anyhow::anyhow!("sandbox selection: {error}"))?;
    let status = SandboxStatus::project(&registry);
    if json {
        println!("{}", status.to_json());
        return Ok(());
    }
    println!("backend                   {}", status.backend);
    println!("available                 {}", status.available);
    println!("bypasses containment      {}", status.bypasses_containment);
    println!("enforces read deny        {}", status.enforces_read_deny);
    println!("owns descendants hard     {}", status.owns_descendants_hard);
    println!("binds cwd authority       {}", status.binds_cwd_authority);
    println!(
        "binds workspace authority {}",
        status.binds_workspace_authority
    );
    Ok(())
}

async fn run_exec(
    workspace: Option<PathBuf>,
    timeout_ms: u64,
    command: String,
) -> anyhow::Result<()> {
    let workspace = resolve_workspace(workspace)?;
    // The bypass is refused HERE, before anything runs: `required_for_session`
    // returns an error for an explicit `none` selection rather than quietly
    // handing back an uncontained runtime.
    let registry = Arc::new(
        SandboxRegistry::required_for_session(None)
            .map_err(|error| anyhow::anyhow!("sandbox selection: {error}"))?,
    );
    let ctx = sandbox_context(&workspace, registry);

    // THE agent shell tool, not a copy of it. See the module docs.
    let result = wcore_tools::bash::BashTool
        .execute_with_ctx(
            serde_json::json!({ "command": command, "timeout": timeout_ms }),
            &ctx,
        )
        .await;

    // The child's own bytes, verbatim. A caller forming a containment
    // differential needs what the child observed, not a summary of it.
    print!("{}", result.content);
    if !result.content.ends_with('\n') && !result.content.is_empty() {
        println!();
    }
    if result.is_error {
        anyhow::bail!("sandboxed command reported an error");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The selector refuses an explicit no-sandbox request rather than
    /// downgrading to an uncontained runtime. This is the property that keeps
    /// `sandbox exec` from being usable as a bypass.
    #[test]
    fn exec_selector_refuses_an_explicit_no_sandbox_selection() {
        let error = SandboxRegistry::required_for_session(Some("none"))
            .expect_err("an explicit `none` selection must not yield a runtime");
        // Fails as an unsafe bypass source, not as an unknown backend.
        assert!(
            format!("{error}").to_lowercase().contains("bypass"),
            "unexpected refusal reason: {error}"
        );
    }

    /// An unknown backend name is refused too, so a typo cannot silently
    /// select something weaker.
    #[test]
    fn exec_selector_refuses_an_unknown_backend() {
        assert!(SandboxRegistry::required_for_session(Some("definitely-not-a-backend")).is_err());
    }

    /// The context handed to the shell tool carries the STRICT production
    /// workspace profile rooted at the caller's workspace, and the registry
    /// selected for it — not the fail-closed placeholder `ToolContext::new`
    /// starts from.
    #[test]
    fn sandbox_context_carries_the_contained_profile_and_the_selected_registry() {
        let dir = tempfile::tempdir().expect("tempdir");
        let root = std::fs::canonicalize(dir.path()).expect("canonicalize");
        let registry =
            Arc::new(SandboxRegistry::required_for_session(None).expect("select a backend"));
        let expected_backend = registry.backend_name().to_owned();

        let ctx = sandbox_context(&root, registry);

        let policy = ctx.workspace.as_deref().expect("workspace policy attached");
        assert_eq!(policy.root(), root.as_path());
        assert!(
            policy.writable_roots().iter().any(|p| p == &root),
            "the workspace must be writable by the child: {:?}",
            policy.writable_roots()
        );
        // The fail-safe network posture: a sandboxed child gets no egress
        // unless the operator opted in. This is what makes a DNS reachability
        // difference a usable containment signal.
        assert!(
            matches!(policy.network(), wcore_sandbox::NetworkPolicy::Deny)
                || std::env::var("WAYLAND_BASH_ALLOW_NETWORK").is_ok(),
            "contained profile must default to network Deny"
        );
        assert_eq!(ctx.sandbox.backend_name(), expected_backend);
        assert_ne!(
            ctx.sandbox.backend_name(),
            "fail_closed",
            "the placeholder registry must have been replaced by the selected one"
        );
        assert!(!ctx.sandbox.bypasses_containment());
    }

    /// The status projection is a read of the live registry, so it cannot
    /// report a backend other than the one selected.
    #[test]
    fn status_projects_the_live_registry() {
        let registry = SandboxRegistry::required_for_session(None).expect("select a backend");
        let status = SandboxStatus::project(&registry);
        assert_eq!(status.backend, registry.backend_name());
        assert_eq!(status.available, registry.is_available());
        assert_eq!(status.bypasses_containment, registry.bypasses_containment());
        assert!(!status.bypasses_containment);
        let json = status.to_json();
        assert_eq!(json["backend"], serde_json::json!(status.backend));
        assert_eq!(
            json["owns_descendants_hard"],
            serde_json::json!(status.owns_descendants_hard)
        );
    }

    /// A workspace that does not exist is refused with the path named, rather
    /// than silently falling back to the current directory — which would scope
    /// the sandbox somewhere the caller did not ask for.
    #[test]
    fn resolve_workspace_refuses_a_missing_directory() {
        let dir = tempfile::tempdir().expect("tempdir");
        let missing = dir.path().join("nope");
        let error = resolve_workspace(Some(missing.clone())).expect_err("missing dir must refuse");
        assert!(format!("{error}").contains("nope"), "{error}");
    }

    #[test]
    fn resolve_workspace_defaults_to_the_current_directory() {
        let resolved = resolve_workspace(None).expect("cwd resolves");
        let cwd = std::fs::canonicalize(std::env::current_dir().expect("cwd")).expect("canon");
        assert_eq!(resolved, cwd);
    }
}
