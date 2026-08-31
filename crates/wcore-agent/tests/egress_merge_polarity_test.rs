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
//!
//! ## The resource ceiling (`BL-UNTRUSTED-RESOURCE-LIMITS`)
//!
//! The second half of this file covers `[default] max_tokens` / `max_turns`,
//! found by the same sweep and closed later. It is the same defect family — a
//! comment asserting a safety property the code did not implement — and it
//! carries controls in both directions for the same reason: a clamp that cannot
//! deny a raise is absent, and one that cannot honour a lowering is a deletion.
//! Unlike `[security] enabled`, that clamp is deliberately TRUST-GATED; the
//! carve-out and the measurement justifying it are on
//! `a_trusted_project_may_still_raise_the_resource_ceiling_by_design` and
//! `raising_the_ceiling_in_a_trusted_repo_revokes_its_own_trust`.

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
    policy_from_config(config)
        .check(&exfil_request(), wcore_egress::EgressOrigin::Product)
        .await
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

// ── allow_sandboxed_shell_network: operator-owned, EITHER trust state ────────

/// SEC-13. `[security] allow_sandboxed_shell_network` is a WHOLE-HOST-NETWORK
/// grant for the sandboxed shell, so it takes the same shape as
/// `security.enabled`: the merge reads the trusted GLOBAL layer alone.
///
/// Unlike `egress_allow` — which is dropped while untrusted and concatenates
/// once trust is granted — a project file must not mint this one in EITHER
/// trust state. Granting a workspace fingerprint says "this repo's config is
/// mine"; it does not say "this repo may take the shell off the leash",
/// and repo content changes after a grant without the operator re-reading it.
#[test]
#[serial(egress_merge_polarity_env)]
fn a_project_cannot_mint_the_sandboxed_shell_network_grant() {
    for trust in [Trust::Untrusted, Trust::Granted] {
        let loaded = load(
            "[security]\nenabled = true\n",
            "[security]\nallow_sandboxed_shell_network = true\n",
            trust,
        );
        assert!(
            !loaded.config.security.allow_sandboxed_shell_network,
            "a project config must never mint the sandboxed-shell network grant \
             (trust={trust:?})"
        );
    }
}

/// The control for the test above: the operator's own global value IS honoured.
/// Without this, `!allow_sandboxed_shell_network` could be passing because the
/// field is unreachable from config entirely.
#[test]
#[serial(egress_merge_polarity_env)]
fn the_operator_global_sandboxed_shell_network_switch_is_honoured() {
    let loaded = load(
        "[security]\nenabled = true\nallow_sandboxed_shell_network = true\n",
        "",
        Trust::Untrusted,
    );
    assert!(
        loaded.config.security.allow_sandboxed_shell_network,
        "the operator's own global allow_sandboxed_shell_network must survive \
         the merge, or the switch is unreachable and its negative test vacuous"
    );
}

/// A project must not be able to REVOKE the operator's grant either — that is
/// why the merge reads global alone rather than `global && project`. A project
/// silent on `[security]` deserializes to `false`, which is absorbing for `&&`.
#[test]
#[serial(egress_merge_polarity_env)]
fn a_project_cannot_revoke_the_sandboxed_shell_network_grant() {
    let loaded = load(
        "[security]\nenabled = true\nallow_sandboxed_shell_network = true\n",
        "[security]\nenabled = true\n",
        Trust::Granted,
    );
    assert!(
        loaded.config.security.allow_sandboxed_shell_network,
        "a project silent on allow_sandboxed_shell_network must leave the \
         operator's grant alone"
    );
}

// ── The resource ceiling: BL-UNTRUSTED-RESOURCE-LIMITS, now closed ───────────
//
// `restrict_untrusted_project_config` forwarded six `[default]` fields under the
// comment *"Resource limits and read-only/approval requests can only reduce
// power"*. That claim was FALSE for two of them: `max_tokens` merges "project
// wins if non-default" and `max_turns` merges `project.or(global)`, and neither
// compares the two values — so an untrusted project raised both ABOVE the
// operator's global ceiling.
//
// **The test that used to sit here asserted `max_tokens == 999_999` and
// `max_turns == Some(100_000)` as the measured behaviour.** It was written as an
// honest measurement of a defect deliberately left open, not as a "narrowing to
// preserve" the way the `security.enabled` test in this same file had been — but
// it pinned the loosening all the same, and it is now inverted. The clamp lives
// in `restrict_untrusted_project_config`, is TRUST-GATED, and the four tests
// below run the control in both directions: it can deny a raise, and it can
// still honour a lowering.

/// FAIL direction — the defect itself. An untrusted project asking for more than
/// the operator allowed gets the operator's value.
#[test]
#[serial(egress_merge_polarity_env)]
fn untrusted_project_cannot_raise_the_resource_ceiling() {
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
        loaded.config.max_tokens, 100,
        "an untrusted project must not raise max_tokens past the operator's 100 \
         (this asserted 999_999 before the clamp)"
    );
    assert_eq!(
        loaded.config.max_turns,
        Some(5),
        "an untrusted project must not raise max_turns past the operator's 5 \
         (this asserted Some(100_000) before the clamp)"
    );

    // The boundary that matters is unaffected — this is a ceiling, not a gate.
    assert!(
        loaded.config.security.enabled,
        "the egress boundary still holds; the resource ceiling is a separate axis"
    );
}

/// PASS direction — the control §3b-iii demands. Without this, a "fix" that
/// simply hard-wired both fields to global would pass the test above while
/// destroying the whole point of forwarding them: a repo tightening its own
/// limits. A clamp that cannot honour a lowering is not a clamp, it is a
/// deletion.
#[test]
#[serial(egress_merge_polarity_env)]
fn untrusted_project_may_still_lower_the_resource_ceiling() {
    let loaded = load_with_global(
        "[default]\nmax_tokens = 100000\nmax_turns = 500\n\n[security]\nenabled = true\n",
        "[default]\nmax_tokens = 4096\nmax_turns = 3\n",
        Trust::Untrusted,
        false,
    );
    assert!(loaded.project_restricted, "untrusted path under test");

    assert_eq!(
        loaded.config.max_tokens, 4096,
        "a project LOWERING max_tokens is a narrowing and must survive the clamp"
    );
    assert_eq!(
        loaded.config.max_turns,
        Some(3),
        "a project LOWERING max_turns is a narrowing and must survive the clamp"
    );
}

/// The ABSENT-value case, and the reason the clamp keeps a presence gate instead
/// of a bare `min`.
///
/// `max_tokens` is `u32` with `#[serde(default = "default_max_tokens")]` =
/// 64000, so a project silent on the field is indistinguishable from one that
/// wrote `64000` — and 64000 is not the identity element for `min`. This test
/// reddens for the natural-looking simplification of moving the comparison to
/// the merge site as `max_tokens: project.min(global)` and dropping the presence
/// gate: that variant returns 64000 here instead of the operator's 200000.
/// Enumerated, it is the *only* shape of the two that differs — which is why the
/// assertion is on a global ABOVE the default, not below it.
///
/// Same family as `operator_off_switch_survives_a_project_silent_on_security`
/// earlier in this file: for a field whose serde default is not neutral, absence
/// is where a merge fix goes wrong.
#[test]
#[serial(egress_merge_polarity_env)]
fn a_project_silent_on_resource_limits_leaves_the_operator_ceiling_alone() {
    let loaded = load_with_global(
        "[default]\nmax_tokens = 200000\nmax_turns = 7\n\n[security]\nenabled = true\n",
        // A real project file that is SILENT on both resource limits.
        "[default]\nsystem_prompt = \"project-body-marker\"\n",
        Trust::Untrusted,
        false,
    );
    assert!(loaded.project_restricted, "untrusted path under test");

    // Instrument-alive check: prove the project file was actually READ and its
    // `[default]` block reached the merge, so the two ceiling assertions below
    // are not passing merely because nothing loaded at all.
    //
    // `system_prompt` is used rather than `model` deliberately: `model` is NOT
    // among the fields `restrict_untrusted_project_config` forwards, so probing
    // with it fails on an untrusted project even when the file loaded perfectly.
    // Measured — the first draft of this test used `model` and this assertion
    // caught it.
    assert_eq!(
        loaded.config.system_prompt.as_deref(),
        Some("project-body-marker"),
        "the project config's [default] block did not reach the merge — the \
         ceiling assertions below would be vacuous"
    );

    assert_eq!(
        loaded.config.max_tokens, 200_000,
        "a project silent on max_tokens must not drag the operator's ceiling down \
         to the 64000 serde default"
    );
    assert_eq!(
        loaded.config.max_turns,
        Some(7),
        "a project silent on max_turns must leave the operator's cap in place"
    );
}

/// A project `Some(n)` against a global `None` is a NARROWING and must be
/// honoured; a clamp that naively required both sides to be `Some` in order to
/// compare would silently drop it.
#[test]
#[serial(egress_merge_polarity_env)]
fn untrusted_project_may_add_a_turn_cap_when_the_operator_has_none() {
    let loaded = load_with_global(
        "[default]\nmax_tokens = 100000\n\n[security]\nenabled = true\n",
        "[default]\nmax_turns = 12\n",
        Trust::Untrusted,
        false,
    );
    assert!(loaded.project_restricted, "untrusted path under test");

    assert_eq!(
        loaded.config.max_turns,
        Some(12),
        "global has no cap, so the project's cap is strictly narrowing and stands"
    );
}

/// The residual hole the first draft of this clamp left open, and the reason the
/// `(Some, None)` arm is not a pass-through.
///
/// An absent global `max_turns` does NOT mean "unlimited": `Config::resolve`
/// finishes the field as
/// `cli.max_turns.or(merged.default.max_turns).unwrap_or(SMART_MAX_TURNS)`, so
/// an operator who configures no cap still has an EFFECTIVE ceiling of 512. A
/// clamp that compared only `(Some, Some)` and passed `(Some(p), None)` through
/// therefore still let an untrusted project raise the effective ceiling from 512
/// to 100000 — the defect surviving inside its own fix.
///
/// This is the sharper of the two `max_turns` controls: the test above proves the
/// clamp can be permissive, this one proves it is not permissive by accident.
#[test]
#[serial(egress_merge_polarity_env)]
fn untrusted_project_cannot_raise_past_the_backstop_when_the_operator_has_no_cap() {
    let loaded = load_with_global(
        "[default]\nmax_tokens = 100000\n\n[security]\nenabled = true\n",
        "[default]\nmax_turns = 100000\n",
        Trust::Untrusted,
        false,
    );
    assert!(loaded.project_restricted, "untrusted path under test");

    assert_eq!(
        loaded.config.max_turns,
        Some(512),
        "with no operator cap the effective ceiling is the SMART_MAX_TURNS \
         backstop (512); an untrusted project must not raise past it"
    );
}

/// The clamp is deliberately TRUST-GATED, and this test says so out loud so the
/// carve-out is a recorded decision rather than an oversight someone later
/// "fixes" without knowing why it was chosen.
///
/// Rationale (panel 3/3 — codex `gpt-5.6-sol`, gemini `3.1-pro-preview`, kimi
/// K3): `[budget]` (`max_cost_usd`, `max_wall_time_secs`) and `[session_cap]`
/// are strictly MORE powerful resource ceilings — they are denominated in
/// dollars — and both merge project-wins unclamped on the trusted path while
/// being dropped entirely on the untrusted one. A trusted workspace can also
/// already register `[mcp.servers]` and `[providers]`, i.e. arbitrary tool
/// execution. Clamping a token count there buys no security and breaks the
/// legitimate monorepo that needs a larger window than the shipped default.
#[test]
#[serial(egress_merge_polarity_env)]
fn a_trusted_project_may_still_raise_the_resource_ceiling_by_design() {
    let loaded = load_with_global(
        "[default]\nmax_tokens = 100\nmax_turns = 5\n\n[security]\nenabled = true\n",
        "[default]\nmax_tokens = 200000\nmax_turns = 900\n",
        Trust::Granted,
        false,
    );
    assert!(
        !loaded.project_restricted,
        "the TRUSTED path is the one under test"
    );

    assert_eq!(
        loaded.config.max_tokens, 200_000,
        "a trusted project's window request is honoured — see this test's doc for why"
    );
    assert_eq!(
        loaded.config.max_turns,
        Some(900),
        "a trusted project's turn budget is honoured — see this test's doc for why"
    );
}

/// The measurement that kills the objection to trust-gating.
///
/// All three panel members, unprompted, raised the same counter-argument: trust
/// is STICKY while repo content is not, so the trusted path is a post-trust
/// escalation channel — a workspace trusted today, then a hostile commit raises
/// `max_turns` tomorrow.
///
/// **That premise is false in this codebase, and this test is the proof.**
/// `fingerprint_workspace` hashes the CONTENT of `.wayland-core.toml` into the
/// trust digest, and `WorkspaceTrustStore::resolve` re-derives and compares it on
/// every resolve. So the edit that would exploit the trusted path is the very
/// edit that invalidates the grant: the workspace reverts to UNTRUSTED and the
/// clamp applies. The escalation channel does not exist.
///
/// Note this asserts BOTH halves — that trust was really granted and effective
/// on the original content, and that it lapsed after the edit. Asserting only
/// the second half would pass if the grant had never worked at all.
#[test]
#[serial(egress_merge_polarity_env)]
fn raising_the_ceiling_in_a_trusted_repo_revokes_its_own_trust() {
    let home = tempfile::tempdir().unwrap();
    let project = tempfile::tempdir().unwrap();
    let project_config = project.path().join(".wayland-core.toml");

    std::fs::write(
        home.path().join("config.toml"),
        "[default]\nmax_tokens = 100\nmax_turns = 5\n",
    )
    .unwrap();
    // The content the operator inspected and trusted.
    std::fs::write(&project_config, "[default]\nmodel = \"reviewed-model\"\n").unwrap();

    let _env = EnvGuard::set(&[
        ("WAYLAND_HOME", Some(home.path().as_os_str())),
        ("WAYLAND_CONFIG_PATH", None),
        ("XDG_DATA_HOME", None),
    ]);

    let store = WorkspaceTrustStore::for_current_home();
    store.grant(project.path()).expect("granting trust");

    let cli = CliArgs {
        provider: Some("anthropic".to_string()),
        api_key: Some("test-key-not-a-real-credential".to_string()),
        project_dir: Some(project.path().to_path_buf()),
        ..CliArgs::default()
    };
    let restricted_of = |cli: &CliArgs| -> (bool, u32, Option<usize>) {
        let resolved = Config::resolve_with_provenance(cli).expect("resolving config");
        let restricted = resolved
            .provenance
            .sources
            .iter()
            .filter(|source| source.role == ConfigSourceRole::Project)
            .any(|source| {
                source
                    .dispositions
                    .contains(&ConfigSourceDisposition::Restricted)
            });
        (
            restricted,
            resolved.value.max_tokens,
            resolved.value.max_turns,
        )
    };

    // Half one: the grant really took effect on the reviewed content. Without
    // this, the assertion below would pass even if `grant` had done nothing.
    let (restricted_before, _, _) = restricted_of(&cli);
    assert!(
        !restricted_before,
        "the grant did not take effect on the reviewed content, so this test \
         cannot say anything about what happens when that content changes"
    );

    // The hostile commit: raise the ceiling past the operator's.
    std::fs::write(
        &project_config,
        "[default]\nmodel = \"reviewed-model\"\nmax_tokens = 999999\nmax_turns = 100000\n",
    )
    .unwrap();

    let (restricted_after, max_tokens_after, max_turns_after) = restricted_of(&cli);
    assert!(
        restricted_after,
        "editing .wayland-core.toml must invalidate the content-bound trust digest \
         and route the config back through restrict_untrusted_project_config"
    );
    assert_eq!(
        max_tokens_after, 100,
        "and the clamp then applies, so the raise does not land"
    );
    assert_eq!(
        max_turns_after,
        Some(5),
        "and the clamp then applies, so the raise does not land"
    );
}
