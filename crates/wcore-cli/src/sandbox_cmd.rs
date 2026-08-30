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
//! THE SHELL PRINCIPAL. This verb is reachable from exactly one place —
//! `TopCmd::Sandbox`, parsed from this host's argv in `main.rs`. There is no
//! channel, host-protocol, slash-command or MCP route to it, so the principal
//! driving it IS the local operator, the same principal as the CLI / TUI
//! session beside it. It therefore takes the same local-operator shell
//! carve-out the session takes, through the SAME predicate —
//! `WorkspacePolicy::with_shell_principal` — and not a second copy of the
//! condition. Before that, this verb refused every shell on a backend that
//! cannot enforce OS secret-read-deny (the Windows relaxed default) while the
//! session it is supposed to be evidence ABOUT ran fine, so the containment
//! differential it exists to produce could not be produced on the one platform
//! that most needed it. The administrator's Managed floor is honoured here
//! exactly as it is in a session: without that, this verb would be a
//! one-command way to obtain the shell a Managed policy refuses.
//!
//! NOT A BYPASS. The selector is `required_for_session`, which refuses the
//! `none` backend outright (`WAYLAND_SANDBOX=none` is an error, not a
//! downgrade) and falls closed to `FailClosedBackend` when the platform offers
//! no real containment. A caller who can run this verb can already run the same
//! command unsandboxed; this only ever removes authority, never adds it.
//!
//! HOW MUCH authority it removes is the backend's answer, not this verb's, and
//! `status` reports it field by field rather than as one word. On Linux
//! (bubblewrap) and macOS (`sandbox-exec`) the child gets a deny-default
//! filesystem scoped to the workspace and the `contained` profile's fail-safe
//! network Deny, both enforced by the OS. On the Windows session default
//! (`windows_job_object`) NEITHER is enforced: a Job Object bounds process
//! lifetime and resource use and has no filesystem filter, so a child there can
//! read and write anywhere this user account can. That is why `status` reports
//! `confines_filesystem` separately from `bypasses_containment` — the latter is
//! session authority ("is this the operator's Dangerous launch") and is `false`
//! on Windows while a write still escapes.

use std::path::PathBuf;
use std::sync::Arc;

use clap::{Args, Subcommand};
use tokio_util::sync::CancellationToken;
use wcore_config::config::CliArgs;
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
    ///
    /// Read `confines_filesystem` for "can a command escape my workspace".
    /// `bypasses_containment` does NOT answer that: it reports whether this is
    /// the operator's explicit no-sandbox launch, and is `false` even on a
    /// backend that enforces no filesystem boundary at all.
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
        /// directory. The child may always read and write here; whether it is
        /// stopped from reaching anything ELSE depends on the backend, and is
        /// reported by `sandbox status` as `confines_filesystem`. That is
        /// `false` on the Windows default.
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
    /// Session authority — whether this is the operator's explicit no-sandbox
    /// launch. NOT a statement that a child is confined; see
    /// [`Self::confines_filesystem`].
    pub bypasses_containment: bool,
    /// Whether the OS stops a child writing outside the workspace policy's
    /// granted roots. `false` on the Windows session default.
    pub confines_filesystem: bool,
    pub enforces_read_deny: bool,
    pub owns_descendants_hard: bool,
    pub binds_cwd_authority: bool,
    pub binds_workspace_authority: bool,
    /// Why `available` is `false`, when the backend knows (#369 c2). An
    /// operator must not have to provoke an `execute()` to find out.
    pub unavailable_reason: Option<String>,
    /// What the selected backend is KNOWN not to do (#368, #369).
    pub known_limitations: Vec<String>,
}

impl SandboxStatus {
    /// Project the live registry. Deliberately a pure read of the registry so
    /// the report cannot drift from the backend it describes.
    pub fn project(registry: &SandboxRegistry) -> Self {
        Self {
            backend: registry.backend_name().to_owned(),
            available: registry.is_available(),
            bypasses_containment: registry.bypasses_containment(),
            confines_filesystem: registry.confines_filesystem(),
            enforces_read_deny: registry.enforces_read_deny(),
            owns_descendants_hard: registry.owns_descendants_hard(),
            binds_cwd_authority: registry.binds_cwd_authority(),
            binds_workspace_authority: registry.binds_workspace_authority(),
            unavailable_reason: registry.unavailable_reason(),
            known_limitations: registry
                .known_limitations()
                .into_iter()
                .map(str::to_owned)
                .collect(),
        }
    }

    pub fn to_json(&self) -> serde_json::Value {
        serde_json::json!({
            "backend": self.backend,
            "available": self.available,
            "bypasses_containment": self.bypasses_containment,
            "confines_filesystem": self.confines_filesystem,
            "enforces_read_deny": self.enforces_read_deny,
            "owns_descendants_hard": self.owns_descendants_hard,
            "binds_cwd_authority": self.binds_cwd_authority,
            "binds_workspace_authority": self.binds_workspace_authority,
            "unavailable_reason": self.unavailable_reason,
            "known_limitations": self.known_limitations,
        })
    }
}

/// `sandbox status` is the operator's ONLY read of the containment posture,
/// and until `#368`/`#369` it could not carry a fact nobody had thought to
/// turn into a boolean. These grade the two fields that changed that.
#[cfg(test)]
mod disclosure_tests {
    use super::{SandboxRegistry, SandboxStatus};
    use std::sync::Arc;

    fn bare() -> SandboxStatus {
        SandboxStatus {
            backend: "test".to_owned(),
            available: true,
            bypasses_containment: false,
            confines_filesystem: false,
            enforces_read_deny: false,
            owns_descendants_hard: false,
            binds_cwd_authority: false,
            binds_workspace_authority: false,
            unavailable_reason: None,
            known_limitations: Vec::new(),
        }
    }

    /// Both fields must reach `--json`. A host integration or a script reads
    /// that and nothing else, and a disclosure only the human-readable arm
    /// carries is a disclosure the desktop app cannot surface.
    #[test]
    fn the_json_arm_carries_the_disclosure_a_script_reads() {
        let mut status = bare();
        status.available = false;
        status.unavailable_reason = Some("the probe said so".to_owned());
        status.known_limitations = vec!["it does not do the thing".to_owned()];
        let json = status.to_json();
        assert_eq!(json["unavailable_reason"], "the probe said so");
        assert_eq!(json["known_limitations"][0], "it does not do the thing");
    }

    /// TOTAL over the struct's fields, not over the ones somebody remembered
    /// to serialise.
    ///
    /// # The N+1 this exists to make impossible
    ///
    /// `#368` c6's fix made the disclosure TOTAL over the backends that
    /// declare a limitation. One level up, the same hole is still open: the
    /// operator's read is `SandboxStatus`, and a field can reach the struct
    /// and never reach `--json`, which is the arm a host integration and every
    /// script read. Nothing graded that, and `#400` c1 is about to add exactly
    /// such a field (`blocks_powershell`).
    ///
    /// The question is inverted rather than enumerated: not "did somebody
    /// remember to serialise this field?", which is undecidable over the
    /// fields nobody has added yet, but "is the JSON key set EQUAL to the
    /// struct's field set?", which is decidable and total. A field added and
    /// not serialised reddens here; so does a key serialised under a name no
    /// field has.
    ///
    /// The human arm is NOT made total here and that is a stated gap, not an
    /// oversight: it renders labels (`binds cwd authority`), not field names,
    /// so field-name equality cannot decide it. Making it total needs one
    /// label table driving both arms, which is a bigger change than an RC
    /// wants; the two disclosure fields it turns on are graded above.
    #[test]
    fn every_status_field_reaches_the_json_arm() {
        let src = std::fs::read_to_string(
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src/sandbox_cmd.rs"),
        )
        .expect("this file is readable from its own test");
        let open_brace = src
            .find("pub struct SandboxStatus {")
            .expect("the struct this test is about must be findable")
            + "pub struct SandboxStatus {".len();
        let body = &src[open_brace..];
        let body = &body[..body.find("\n}\n").expect("the struct must close")];
        let mut fields: Vec<String> = body
            .lines()
            .filter_map(|l| l.trim().strip_prefix("pub "))
            .filter_map(|l| l.split(':').next())
            .map(|s| s.trim().to_owned())
            .collect();

        // POSITIVE CONTROL on the scanner. A find that silently drifted would
        // leave an empty field list, and an empty set compares equal to an
        // empty set -- the vacuity this whole file exists to close.
        assert!(
            fields.len() >= 8,
            "the scanner found only {} fields on SandboxStatus; it is not \
             reading the struct: {body:?}",
            fields.len()
        );

        let json = bare().to_json();
        let mut keys: Vec<String> = json
            .as_object()
            .expect("the status must serialise as an object")
            .keys()
            .cloned()
            .collect();
        fields.sort();
        keys.sort();
        assert_eq!(
            fields, keys,
            "every field of `SandboxStatus` must reach the `--json` arm and \
             nothing else may: a host integration reads that and nothing \
             else, so a field the struct carries and the JSON drops is a \
             disclosure the desktop app cannot surface. Add the key in \
             `to_json`, and an arm in this module if an operator has to be \
             able to act on it."
        );
    }

    /// A backend with nothing to disclose must emit JSON `null` and an EMPTY
    /// list, never an empty string or a reassuring placeholder. A consumer
    /// that sees `""` cannot tell "no cause recorded" from "cause is blank",
    /// and #369's whole harm was a cause that existed and could not be read.
    #[test]
    fn nothing_recorded_serialises_as_null_and_empty_not_as_reassurance() {
        let json = bare().to_json();
        assert!(json["unavailable_reason"].is_null());
        assert_eq!(json["known_limitations"].as_array().map(Vec::len), Some(0));
    }

    /// The human arm must print every declared limitation, not merely hold
    /// them in the struct. Graded with a sentinel so it is the RENDER under
    /// test and not any particular backend's wording.
    #[test]
    fn the_human_arm_prints_every_declared_limitation() {
        let mut status = bare();
        status.known_limitations = vec![
            "SENTINEL-ONE does not do the first thing".to_owned(),
            "SENTINEL-TWO does not do the second thing".to_owned(),
        ];
        let human = super::render_status_human(&status);
        assert!(
            human.contains("KNOWN LIMITATIONS"),
            "the block must be headed, or the lines read as prose: {human}"
        );
        for text in &status.known_limitations {
            assert!(
                human.contains(text.as_str()),
                "the human arm dropped {text:?}; an operator at a terminal \
                 reads this and nothing else: {human}"
            );
        }
    }

    /// A backend with nothing declared must not print an empty heading — an
    /// empty `KNOWN LIMITATIONS:` block reads as a clean bill of health.
    #[test]
    fn the_human_arm_says_nothing_when_nothing_is_declared() {
        assert!(!super::render_status_human(&bare()).contains("KNOWN LIMITATIONS"));
    }

    /// Answers `is_available()` without probing, and delegates everything this
    /// criterion is about to the real backend underneath.
    ///
    /// `SandboxStatus::project` reads `is_available()`, and AppContainer's is a
    /// guarded REAL SPAWN with a 15 s wall-clock guard. If the disclosure grade
    /// depended on it, the one arm that catches a deleted disclosure would be
    /// slow, side-effecting, and skipped on exactly the hosts where the
    /// disclosure matters most. NOTHING ELSE is substituted: `name()` and
    /// `known_limitations()` come from the real backend, so deleting a real
    /// backend's declaration still reddens.
    struct AvailabilityStub(Arc<dyn wcore_sandbox::backends::SandboxBackend>);

    #[async_trait::async_trait]
    impl wcore_sandbox::backends::SandboxBackend for AvailabilityStub {
        fn name(&self) -> &'static str {
            self.0.name()
        }
        fn is_available(&self) -> bool {
            true
        }
        fn known_limitations(&self) -> Vec<&'static str> {
            self.0.known_limitations()
        }
        async fn execute(
            &self,
            _manifest: &wcore_sandbox::SandboxManifest,
            _cmd: wcore_sandbox::SandboxCommand,
        ) -> wcore_sandbox::Result<wcore_sandbox::SandboxOutput> {
            unreachable!("the disclosure grade never executes a command")
        }
    }

    /// THE assertion `#368` c6 is actually about: what a backend declares must
    /// survive the whole path an operator reads it through — backend →
    /// `SandboxRegistry` → `SandboxStatus` → BOTH arms of `sandbox status`.
    ///
    /// Grading the constants instead of this path is what let
    /// `AppContainerBackend::known_limitations` be replaced with `Vec::new()`
    /// while `sandbox status --json` on real Windows returned
    /// `"known_limitations":[]` and every test stayed green.
    fn assert_declared_limitations_reach_the_operator(
        backend: Arc<dyn wcore_sandbox::backends::SandboxBackend>,
    ) {
        let name = backend.name();
        let declared: Vec<String> = backend
            .known_limitations()
            .into_iter()
            .map(str::to_owned)
            .collect();
        assert!(
            !declared.is_empty(),
            "backend `{name}` is registered as declaring a known limitation \
             and declared none; an empty list is what an operator reads as a \
             clean bill of health"
        );

        let registry = SandboxRegistry::new(Arc::new(AvailabilityStub(backend)));
        let status = SandboxStatus::project(&registry);
        let json = status.to_json();
        let human = super::render_status_human(&status);
        for text in &declared {
            assert!(
                json["known_limitations"]
                    .as_array()
                    .is_some_and(|entries| entries.iter().any(|v| v == text)),
                "backend `{name}` declares {text:?} and the --json arm a host \
                 integration reads does not carry it: {json}"
            );
            assert!(
                human.contains(text.as_str()),
                "backend `{name}` declares {text:?} and the human arm an \
                 operator reads does not print it: {human}"
            );
        }
    }

    /// TOTAL over the declaring backends, not over the ones somebody
    /// remembered. The table is scanned against this crate's source by
    /// `wcore-sandbox/tests/declared_limitations_are_registered.rs`, so a
    /// backend that declares a limitation cannot reach here unlisted, and an
    /// unrecognised row panics rather than being skipped.
    #[test]
    fn every_declaring_backends_disclosure_reaches_both_operator_arms() {
        use wcore_sandbox::backends::BACKENDS_THAT_DECLARE_LIMITATIONS;
        use wcore_sandbox::backends::windows_job_object::WindowsJobObjectBackend;

        assert!(
            !BACKENDS_THAT_DECLARE_LIMITATIONS.is_empty(),
            "positive control: an empty table would make this loop vacuous"
        );
        let mut graded = 0usize;
        for row in BACKENDS_THAT_DECLARE_LIMITATIONS {
            match row.name {
                "windows_job_object" => {
                    assert_declared_limitations_reach_the_operator(Arc::new(
                        WindowsJobObjectBackend::new(),
                    ));
                    graded += 1;
                }
                #[cfg(windows)]
                "appcontainer" => {
                    assert_declared_limitations_reach_the_operator(Arc::new(
                        wcore_sandbox::backends::appcontainer::AppContainerBackend::new(),
                    ));
                    graded += 1;
                }
                // Off Windows the type in that row does not exist, and the row
                // says so. Asserted rather than skipped: a row that is NOT
                // windows-only and has no arm here must fail, not vanish.
                #[cfg(not(windows))]
                "appcontainer" => assert!(
                    row.windows_only,
                    "row `{}` is not windows-only and has no arm on this target",
                    row.name
                ),
                other => panic!(
                    "backend `{other}` declares known limitations and nothing \
                     here projects them to the operator. Add an arm — this \
                     panic IS the guard: #368 c6 was graded `met` while \
                     exactly one backend's declaration was unread, and \
                     deleting it left every test green."
                ),
            }
        }
        #[cfg(windows)]
        assert_eq!(graded, BACKENDS_THAT_DECLARE_LIMITATIONS.len());
        #[cfg(not(windows))]
        assert!(
            graded >= 1,
            "at least one declaring backend must be constructible on every \
             target, or this test proves nothing off Windows"
        );
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

/// Build the workspace policy the sandboxed child runs under.
///
/// The same strict `contained` profile a hosted session builds, carrying the
/// same shell-principal decision — see the module docs. `channel_posture_present`
/// is passed as a literal `false` because it is structurally false for this
/// verb, not because it was forgotten: argv is the only route in.
///
/// Exposed so `sandbox_exec_principal_parity` can compare it against the policy
/// the production bootstrap installs for the same inputs.
pub fn sandbox_policy(
    workspace: &std::path::Path,
    managed_execution_floor: bool,
) -> WorkspacePolicy {
    WorkspacePolicy::contained(workspace).with_shell_principal(false, managed_execution_floor)
}

/// Build the tool context the sandboxed child runs under.
///
/// This is the same shape a hosted agent session builds: the strict
/// `contained` workspace profile, plus the registry-owned, containment-required
/// session runtime. Exposed (rather than inlined) so its properties are
/// directly assertable in tests.
pub fn sandbox_context(
    workspace: &std::path::Path,
    registry: Arc<SandboxRegistry>,
    managed_execution_floor: bool,
) -> ToolContext {
    let policy = Arc::new(sandbox_policy(workspace, managed_execution_floor));
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
    print!("{}", render_status_human(&status));
    Ok(())
}

/// The human arm of `sandbox status`, rendered to a `String` rather than
/// straight to stdout.
///
/// # Why this is a function and not a run of `println!`
///
/// It was a run of `println!`, and that made the operator's ACTUAL read
/// ungradeable: every test on this surface asserted the `SandboxStatus`
/// struct or its JSON, so a disclosure could reach the struct and never reach
/// the screen and nothing would notice. `#368` c6 asks for the opposite
/// property — that the product STATES a defect *where an operator reads the
/// containment posture* — and this is that place. Returning the text is what
/// lets `disclosure_tests` grade the read instead of the constant behind it.
fn render_status_human(status: &SandboxStatus) -> String {
    use std::fmt::Write as _;
    let mut out = String::new();
    let _ = writeln!(out, "backend                   {}", status.backend);
    let _ = writeln!(out, "available                 {}", status.available);
    let _ = writeln!(
        out,
        "bypasses containment      {}",
        status.bypasses_containment
    );
    let _ = writeln!(
        out,
        "confines filesystem       {}",
        status.confines_filesystem
    );
    let _ = writeln!(
        out,
        "enforces read deny        {}",
        status.enforces_read_deny
    );
    let _ = writeln!(
        out,
        "owns descendants hard     {}",
        status.owns_descendants_hard
    );
    let _ = writeln!(
        out,
        "binds cwd authority       {}",
        status.binds_cwd_authority
    );
    let _ = writeln!(
        out,
        "binds workspace authority {}",
        status.binds_workspace_authority
    );
    if let Some(why) = &status.unavailable_reason {
        let _ = writeln!(out);
        let _ = writeln!(out, "UNAVAILABLE, and the backend knows why:");
        let _ = writeln!(out, "      {why}");
    }
    if !status.known_limitations.is_empty() {
        let _ = writeln!(out);
        let _ = writeln!(
            out,
            "KNOWN LIMITATIONS of backend `{}` — measured, open, and NOT fixed:",
            status.backend
        );
        for l in &status.known_limitations {
            let _ = writeln!(out, "      - {l}");
        }
    }
    // A row of booleans is not readable as a security posture. Say the
    // consequence of the one that decides whether a command can leave the
    // workspace, naming the mechanism so an operator can act on it.
    if status.available && !status.bypasses_containment && !status.confines_filesystem {
        let _ = writeln!(out);
        let _ = writeln!(
            out,
            "NOTE: backend `{}` does NOT confine the filesystem.",
            status.backend
        );
        let _ = writeln!(
            out,
            "      A command run through this sandbox — including the agent's Bash tool —"
        );
        let _ = writeln!(
            out,
            "      can read and write anywhere this user account can."
        );
        let _ = writeln!(
            out,
            "      `bypasses containment false` means a real backend was selected, NOT"
        );
        let _ = writeln!(out, "      that a write cannot escape the workspace.");
        if cfg!(windows) {
            let _ = writeln!(
                out,
                "      In force:     kill-on-close Job Object process-tree ownership;"
            );
            let _ = writeln!(
                out,
                "                    child environment scrubbed to the manifest."
            );
            let _ = writeln!(
                out,
                "      NOT in force: OS filesystem confinement, OS network denial,"
            );
            let _ = writeln!(out, "                    OS secret read-deny.");
            let _ = writeln!(
                out,
                "      `WAYLAND_SANDBOX=appcontainer` selects the STRICT backend that"
            );
            let _ = writeln!(out, "      enforces them.");
        }
    }
    out
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
    // Read the administrator's Managed floor from the merged config files. Not
    // `Config::resolve` — this verb runs no model and must work on a host that
    // has never been onboarded. An unreadable config is refused rather than
    // treated as "unmanaged".
    let managed_execution_floor =
        wcore_config::config::Config::resolve_managed_execution_floor(&CliArgs::default())
            .map_err(|error| anyhow::anyhow!("execution policy: {error}"))?;
    let ctx = sandbox_context(&workspace, registry, managed_execution_floor);

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
        // `SandboxRegistry` is deliberately not `Debug`, so unwrap the Result
        // by hand rather than through `expect_err`.
        let error = match SandboxRegistry::required_for_session(Some("none")) {
            Ok(registry) => panic!(
                "an explicit `none` selection must not yield a runtime; got backend {}",
                registry.backend_name()
            ),
            Err(error) => error,
        };
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

        let ctx = sandbox_context(&root, registry, false);

        let policy = ctx.workspace.as_deref().expect("workspace policy attached");
        assert_eq!(policy.root(), root.as_path());
        assert!(
            policy.writable_roots().iter().any(|p| p == &root),
            "the workspace must be writable by the child: {:?}",
            policy.writable_roots()
        );
        // The fail-safe network posture: a sandboxed child gets no egress.
        // This is what makes a DNS reachability difference a usable
        // containment signal. SEC-11 — the assertion used to carry an
        // `|| env::var("WAYLAND_BASH_ALLOW_NETWORK").is_ok()` escape clause;
        // that env lever is gone, so the posture here is unconditional.
        assert!(
            matches!(policy.network(), wcore_sandbox::NetworkPolicy::Deny),
            "contained profile must default to network Deny, got {:?}",
            policy.network()
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
        assert_eq!(status.confines_filesystem, registry.confines_filesystem());
        let json = status.to_json();
        assert_eq!(json["backend"], serde_json::json!(status.backend));
        assert_eq!(
            json["owns_descendants_hard"],
            serde_json::json!(status.owns_descendants_hard)
        );
        // The two containment words must both reach the wire. A consumer that
        // sees only `bypasses_containment` reads `false` as "contained", which
        // is what the Windows default made untrue.
        assert!(json["bypasses_containment"].is_boolean());
        assert_eq!(
            json["confines_filesystem"],
            serde_json::json!(status.confines_filesystem)
        );
    }

    /// The two fields answer different questions, and on the Windows default
    /// they answer them differently. Asserted against the real backends rather
    /// than the live host's, so every platform runs the check.
    #[test]
    fn filesystem_confinement_is_reported_independently_of_session_authority() {
        use wcore_sandbox::backends::bwrap::BubblewrapBackend;
        use wcore_sandbox::backends::windows_job_object::WindowsJobObjectBackend;

        let relaxed = SandboxRegistry::new(Arc::new(WindowsJobObjectBackend::new()));
        let relaxed = SandboxStatus::project(&relaxed);
        assert!(
            !relaxed.bypasses_containment && !relaxed.confines_filesystem,
            "the Windows default is not a bypass AND does not confine the \
             filesystem; a surface that reports only the first tells an \
             operator a write cannot escape when it can: {relaxed:?}"
        );

        let confining = SandboxRegistry::new(Arc::new(BubblewrapBackend::new()));
        let confining = SandboxStatus::project(&confining);
        assert!(
            confining.confines_filesystem,
            "positive control: bwrap confines the filesystem, or the row above \
             proves nothing: {confining:?}"
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
