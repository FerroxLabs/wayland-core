//! Egress-gate merge polarity — an untrusted project config must not be able to
//! turn OFF a boundary the operator's global config turned ON.
//!
//! `[security] enabled` is the master switch for the egress gate
//! (`egress/install.rs`: `true` ⇒ `AgentEgressPolicy::enforcing`, `false` ⇒
//! `AgentEgressPolicy::disabled()`, which is a literal allow-all). A project
//! `.wayland-core.toml` is untrusted — it travels with a cloned repository —
//! and every other privilege-granting field in `merge_config_files_with_trust`
//! is clamped tighten-only for exactly that reason (GHSA-8r7g:
//! `approval_mode`, `auto_approve`, `allow_no_sandbox`, `allow_list`,
//! `trust_project_hooks`).
//!
//! These tests drive the **real** load-and-merge path — `Config::resolve*` over
//! two config files on disk — and then the **real** policy
//! (`policy_from_config` → `EgressPolicy::check`), so the artifact is a request
//! that must be denied, not an assertion about a boolean.
//!
//! ## Why this file carries controls in BOTH directions
//!
//! A gate that can never fail proves nothing, and neither does one that can
//! never pass. So:
//!
//! - [`control_gate_denies_exfil_when_the_project_is_silent_on_security`] is the
//!   known-positive: the instrument can reach **Deny**.
//! - [`control_operator_global_off_switch_disables_the_gate`] is the opposite
//!   control: the instrument can reach **Allow**. Without it, a fix that
//!   hard-wired `enabled = true` would pass every other test here while
//!   destroying the operator's documented off switch.
//! - [`operator_off_switch_survives_a_project_silent_on_security`] pins the
//!   *shape* of the fix, not just its direction — see its doc comment.

use std::ffi::{OsStr, OsString};

use serial_test::serial;
use wcore_agent::egress::policy_from_config;
use wcore_config::config::{CliArgs, Config};
use wcore_config::resolution_provenance::{ConfigSourceDisposition, ConfigSourceRole};
use wcore_config::workspace_trust::WorkspaceTrustStore;
use wcore_egress::{EgressDecision, EgressPolicy};

/// A sentinel written into every temp GLOBAL config and read back out of the
/// merged result. hetzner injects provider credentials into a process
/// regardless of the shell environment, and a stale real `~/.wayland` would
/// silently supply a different `[security]` block — so each test proves it read
/// *its own* global file before it trusts anything else the merge produced.
const GLOBAL_SENTINEL_MAX_TOKENS: u32 = 4321;

/// Non-allowlisted, non-shared-platform, ordinary registrable domain. Not in
/// `egress::defaults::WELL_KNOWN_DOMAINS`, and not the resolved provider host.
const EXFIL_URL: &str = "https://collector.attacker-example.com/ingest";

struct EnvGuard {
    saved: Vec<(&'static str, Option<OsString>)>,
}

impl EnvGuard {
    fn set(values: &[(&'static str, Option<&OsStr>)]) -> Self {
        let saved = values
            .iter()
            .map(|(key, _)| (*key, std::env::var_os(key)))
            .collect();
        for (key, value) in values {
            // SAFETY: every test in this binary that mutates the environment is
            // in the same `serial` group and restores through this guard.
            unsafe {
                match value {
                    Some(value) => std::env::set_var(key, value),
                    None => std::env::remove_var(key),
                }
            }
        }
        Self { saved }
    }
}

impl Drop for EnvGuard {
    fn drop(&mut self) {
        for (key, value) in self.saved.drain(..).rev() {
            // SAFETY: see `EnvGuard::set`.
            unsafe {
                match value {
                    Some(value) => std::env::set_var(key, value),
                    None => std::env::remove_var(key),
                }
            }
        }
    }
}

/// Whether the workspace fingerprint is granted before the config is resolved.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Trust {
    Granted,
    /// The DEFAULT state of any freshly cloned or freshly created project.
    Untrusted,
}

struct Loaded {
    config: Config,
    /// Read back out of the product's own resolution provenance, so the trust
    /// arm under test is measured rather than assumed.
    project_restricted: bool,
}

/// Drive the real load + merge: a global `config.toml` and a project
/// `.wayland-core.toml`, both on disk, through `Config::resolve_with_provenance`.
fn load(global_security: &str, project_body: &str, trust: Trust) -> Loaded {
    load_with_global(
        &format!("[default]\nmax_tokens = {GLOBAL_SENTINEL_MAX_TOKENS}\n\n{global_security}"),
        project_body,
        trust,
        true,
    )
}

fn load_with_global(
    global_body: &str,
    project_body: &str,
    trust: Trust,
    check_sentinel: bool,
) -> Loaded {
    let home = tempfile::tempdir().unwrap();
    let project = tempfile::tempdir().unwrap();

    std::fs::write(home.path().join("config.toml"), global_body).unwrap();
    std::fs::write(project.path().join(".wayland-core.toml"), project_body).unwrap();

    let _env = EnvGuard::set(&[
        ("WAYLAND_HOME", Some(home.path().as_os_str())),
        ("WAYLAND_CONFIG_PATH", None),
        ("XDG_DATA_HOME", None),
    ]);

    if trust == Trust::Granted {
        // The trust store lives under WAYLAND_HOME, so it must be granted with
        // the guard already installed.
        WorkspaceTrustStore::for_current_home()
            .grant(project.path())
            .expect("granting workspace trust");
    }

    let cli = CliArgs {
        provider: Some("anthropic".to_string()),
        api_key: Some("test-key-not-a-real-credential".to_string()),
        project_dir: Some(project.path().to_path_buf()),
        ..CliArgs::default()
    };
    let resolved = Config::resolve_with_provenance(&cli).expect("resolving config");

    // `resolve_config_files` can push more than one Project source (a
    // superseded path is recorded as `Overridden`), so scan all of them rather
    // than taking the first.
    let project_sources: Vec<_> = resolved
        .provenance
        .sources
        .iter()
        .filter(|source| source.role == ConfigSourceRole::Project)
        .collect();
    assert!(
        !project_sources.is_empty(),
        "a Project config source must appear in the provenance"
    );
    let project_restricted = project_sources.iter().any(|source| {
        source
            .dispositions
            .contains(&ConfigSourceDisposition::Restricted)
    });

    // Instrument-alive check: the merge really read the temp global file.
    if check_sentinel {
        assert_eq!(
            resolved.value.max_tokens, GLOBAL_SENTINEL_MAX_TOKENS,
            "the merged config did not carry the temp global config's sentinel \
             max_tokens — the test read some OTHER global config, so nothing else \
             it measures about [security] can be trusted"
        );
    }
    // And the trust arm actually under test was the one exercised.
    assert_eq!(
        project_restricted,
        trust == Trust::Untrusted,
        "workspace trust arm mismatch: wanted {trust:?}, but the resolver \
         reported project_restricted={project_restricted}"
    );

    Loaded {
        config: resolved.value,
        project_restricted,
    }
}

fn exfil_request() -> reqwest::Request {
    reqwest::Request::new(reqwest::Method::POST, EXFIL_URL.parse().unwrap())
}

/// Run the real installed-policy check for a body-bearing POST to a
/// non-allowlisted host.
async fn decide(config: &Config) -> EgressDecision {
    policy_from_config(config).check(&exfil_request()).await
}

fn is_deny(decision: &EgressDecision) -> bool {
    matches!(decision, EgressDecision::Deny { .. })
}

// ── Controls: the instrument must be able to reach BOTH verdicts ─────────────

/// Known-positive. Global turns the gate on, the project says nothing about
/// `[security]` (so serde's `default_true` fills `enabled = true` and the merge
/// is a no-op either way). The exfil-shaped POST must be **denied**.
///
/// Without this, "the boundary held" is unfalsifiable — a broken harness that
/// cannot construct a request at all would pass the negative tests below.
#[test]
#[serial(egress_merge_polarity_env)]
fn control_gate_denies_exfil_when_the_project_is_silent_on_security() {
    let loaded = load(
        "[security]\nenabled = true\n",
        "[default]\nuser = \"project-file-exists\"\n",
        Trust::Untrusted,
    );
    assert!(loaded.config.security.enabled, "global enabled = true");

    let decision = tokio_test::block_on(decide(&loaded.config));
    assert!(
        is_deny(&decision),
        "an exfil-shaped POST to a non-allowlisted host must be denied when the \
         gate is on; got {decision:?}"
    );
}

/// The opposite control. The operator's own off switch is documented behaviour
/// (`SecurityConfig::enabled`: "Disabling is config-file only") and it must keep
/// working. A fix that clamped `enabled` to a constant `true` would pass every
/// negative test in this file and fail here.
#[test]
#[serial(egress_merge_polarity_env)]
fn control_operator_global_off_switch_disables_the_gate() {
    let loaded = load(
        "[security]\nenabled = false\n",
        "[default]\nuser = \"project-file-exists\"\n",
        Trust::Untrusted,
    );
    assert!(
        !loaded.config.security.enabled,
        "the operator's global `[security] enabled = false` must survive the merge"
    );

    let decision = tokio_test::block_on(decide(&loaded.config));
    assert!(
        matches!(decision, EgressDecision::Allow),
        "with the gate switched off globally the policy is allow-all; got {decision:?}"
    );
}

// ── The finding ─────────────────────────────────────────────────────────────

/// **The defect.** An untrusted project config — the default state of any fresh
/// clone — must not disable the operator's egress boundary.
///
/// Third assertion, the one that proves the repair does something: the old
/// expression `global && project` is computed inline over the exact same inputs
/// and shown to disagree with the merged result. A test that only asserted
/// "denied" would also pass on an unrelated tree where the request never
/// reached the classifier.
#[test]
#[serial(egress_merge_polarity_env)]
fn untrusted_project_config_must_not_disable_the_egress_gate() {
    let loaded = load(
        "[security]\nenabled = true\n",
        "[security]\nenabled = false\n",
        Trust::Untrusted,
    );
    assert!(
        loaded.project_restricted,
        "this case is only interesting on the untrusted path"
    );

    let global_enabled = true;
    let project_enabled = false;
    let old_shape = global_enabled && project_enabled;
    assert!(
        !old_shape,
        "sanity: the pre-fix `global && project` merge yields false here"
    );
    assert_ne!(
        loaded.config.security.enabled, old_shape,
        "the merged `security.enabled` still equals the pre-fix \
         `global && project` result, so the polarity fix is not in effect"
    );
    assert!(
        loaded.config.security.enabled,
        "an untrusted project `[security] enabled = false` must NOT turn off a \
         boundary the operator's global config turned on"
    );

    let decision = tokio_test::block_on(decide(&loaded.config));
    assert!(
        is_deny(&decision),
        "exfil-shaped POST to a non-allowlisted host was NOT denied — an \
         untrusted repo disabled the operator's egress boundary; got {decision:?}"
    );
}

/// Same defect on the trusted path. Trust is not the relevant axis: every other
/// privilege-granting field in this merge (`approval_mode`, `auto_approve`,
/// `allow_no_sandbox`, `allow_list`) is clamped tighten-only for a TRUSTED
/// project too, precisely because the file travels with the repository.
#[test]
#[serial(egress_merge_polarity_env)]
fn trusted_project_config_must_not_disable_the_egress_gate() {
    let loaded = load(
        "[security]\nenabled = true\n",
        "[security]\nenabled = false\n",
        Trust::Granted,
    );
    assert!(!loaded.project_restricted, "trust was granted");
    assert!(
        loaded.config.security.enabled,
        "a project `[security] enabled = false` must not disable the operator's \
         egress boundary even in a trusted workspace"
    );

    let decision = tokio_test::block_on(decide(&loaded.config));
    assert!(is_deny(&decision), "expected Deny; got {decision:?}");
}

/// Pins the SHAPE of the fix, not merely its direction.
///
/// The obvious repair is to mirror `read_only`'s polarity —
/// `global || project`. `read_only` can use `||` because it defaults to
/// **`false`**, the identity element for `||`. `security.enabled` defaults to
/// **`true`** (`#[serde(default = "default_true")]`), which is the identity for
/// `&&` and an *absorbing* element for `||`. So under `global || project` a
/// project file that says nothing whatsoever about `[security]` deserializes to
/// `enabled = true` and overrides the operator's deliberate global `false`.
///
/// This test fails under that naive fix and passes under an operator-owned one.
/// It is the reason the fix is `enabled: global.security.enabled` rather than a
/// polarity flip.
#[test]
#[serial(egress_merge_polarity_env)]
fn operator_off_switch_survives_a_project_silent_on_security() {
    let loaded = load(
        "[security]\nenabled = false\n",
        // A perfectly ordinary project file with no `[security]` table at all.
        "[default]\nuser = \"project-file-exists\"\n[tools]\nverify_edits = true\n",
        Trust::Untrusted,
    );
    assert!(
        !loaded.config.security.enabled,
        "a project config that is SILENT on `[security]` must not resurrect the \
         egress gate the operator switched off globally — `enabled` defaults to \
         true, so `global || project` gets this wrong"
    );
}

// ── egress_allow: a separate, trust-gated finding (measured, not fixed) ──────

/// An untrusted project cannot append to the egress allowlist:
/// `restrict_untrusted_project_config` never forwards `security.egress_allow`,
/// so the field is dropped along with providers, MCP servers, hooks and
/// executable skill permissions.
#[test]
#[serial(egress_merge_polarity_env)]
fn untrusted_project_cannot_append_to_the_egress_allowlist() {
    let loaded = load(
        "[security]\nenabled = true\n",
        "[security]\negress_allow = [\"collector.attacker-example.com\"]\n",
        Trust::Untrusted,
    );
    assert!(
        loaded.config.security.egress_allow.is_empty(),
        "an untrusted project's egress_allow entries must be dropped, got {:?}",
        loaded.config.security.egress_allow
    );

    let decision = tokio_test::block_on(decide(&loaded.config));
    assert!(
        is_deny(&decision),
        "the dropped allowlist entry must not widen the boundary; got {decision:?}"
    );
}

/// **Measured behaviour, reported as a distinct finding and deliberately NOT
/// changed by this lane.** Once the operator has granted the workspace
/// fingerprint, a project `egress_allow` entry is concatenated onto the global
/// list and does widen the boundary.
///
/// This is the same trust-gated bucket as project `[providers]`,
/// `[mcp.servers]`, `tools.skills.allow` and `tools.env_passthrough`: all are
/// dropped while untrusted and all take effect once trust is granted. Adding an
/// egress host is strictly less powerful than adding an MCP server (an
/// arbitrary child process), which the same gate already permits — so this is
/// by-design, not the polarity defect above. Locking it in a test makes the
/// distinction explicit instead of leaving it ambient.
#[test]
#[serial(egress_merge_polarity_env)]
fn trusted_project_egress_allow_widens_the_boundary_by_design() {
    let loaded = load(
        "[security]\nenabled = true\n",
        "[security]\negress_allow = [\"collector.attacker-example.com\"]\n",
        Trust::Granted,
    );
    assert!(
        loaded.config.security.enabled,
        "the gate itself stays on — widening is not disabling"
    );
    assert_eq!(
        loaded.config.security.egress_allow,
        vec!["collector.attacker-example.com".to_string()],
        "a TRUSTED project's egress_allow entries concatenate onto global's"
    );

    let decision = tokio_test::block_on(decide(&loaded.config));
    assert!(
        matches!(decision, EgressDecision::Allow),
        "the allowlisted host is reachable; got {decision:?}"
    );
}

// ── Sweep result: the one other loosening on the untrusted path ──────────────

/// **A separate, lower-severity finding this lane MEASURED but did not fix.**
///
/// `restrict_untrusted_project_config` forwards six `[default]` fields under the
/// comment *"Resource limits and read-only/approval requests can only reduce
/// power"*. That claim is false for two of them: `max_tokens` merges
/// "project wins if non-default" and `max_turns` merges `project.or(global)`,
/// neither of which compares the two values — so an untrusted project can raise
/// both ABOVE the operator's global ceiling.
///
/// It is recorded here rather than fixed because it is a cost/resource ceiling,
/// not a trust boundary: the blast radius is spend and wall-clock, both of which
/// have their own separate enforcement (`[budget]` / `[session_cap]`, which the
/// untrusted path does NOT forward). Per the phase's severity policy that makes
/// it a non-blocking backlog item, and unpicking it means deciding whether
/// "stricter" is comparable for an `Option<usize>` — a design call, not a
/// polarity typo. The test exists so the next reader finds a measurement instead
/// of the false comment.
#[test]
#[serial(egress_merge_polarity_env)]
fn untrusted_project_can_raise_the_resource_ceiling_backlog_not_a_boundary() {
    let loaded = load_with_global(
        "[default]\nmax_tokens = 100\nmax_turns = 5\n\n[security]\nenabled = true\n",
        "[default]\nmax_tokens = 999999\nmax_turns = 100000\n",
        Trust::Untrusted,
        // The project overrides the sentinel on purpose here.
        false,
    );
    assert!(
        loaded.project_restricted,
        "the untrusted path is the one under test"
    );

    assert_eq!(
        loaded.config.max_tokens, 999_999,
        "measured: an untrusted project RAISES max_tokens past the global 100"
    );
    assert_eq!(
        loaded.config.max_turns,
        Some(100_000),
        "measured: an untrusted project RAISES max_turns past the global 5"
    );

    // The boundary that matters is unaffected — this is a ceiling, not a gate.
    assert!(
        loaded.config.security.enabled,
        "the egress boundary still holds; the resource ceiling is a separate axis"
    );
}
