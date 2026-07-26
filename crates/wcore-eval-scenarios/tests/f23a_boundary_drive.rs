#![cfg(feature = "packaged-driver-gate")]
//! F23A-01 — the live execution-boundary proof for Phase 23A Success Criterion 1:
//! *"generated skills cannot execute before governed promotion."*
//!
//! Everything here drives the SHIPPED `wayland-core` binary against a loopback
//! OpenAI-compatible fixture provider. Nothing asserts the return value of an
//! internal guard, because a guard returning `false` is not the security claim.
//! Phase 20A drove two platforms to CI-green without anyone launching the
//! binary and a later live pass found three HIGH defects in that same build;
//! this file exists so that does not happen to this phase's only security
//! criterion.
//!
//! The generated draft is produced by the PRODUCT'S OWN drafting path — three
//! identical turns until `PatternDetector` fires. Hand-seeding a draft is
//! forbidden: a hand-written artifact proves the loader's classifier, not the
//! product.
//!
//! Self-tests, so the controls here are provably not decorative:
//!   * `WAYLAND_EXPECT_SHA` not matching the checkout exits the wrapper with
//!     exactly 3 (asserted in `scripts/f23a-boundary-drive.sh`).
//!   * `WAYLAND_F23A_SELFTEST=refusal` substitutes the user-authored control
//!     skill where the quarantined draft is expected, so the refusal assertion
//!     is handed content that legitimately DOES resolve. The target must then
//!     fail, print `F23A-SELFTEST-TRIPPED: refusal`, and exit nonzero.

use std::fs;
use std::path::{Path, PathBuf};

use wcore_eval_scenarios::assertions::Assertion;
use wcore_eval_scenarios::fixtures::openai::{OpenAiFixtureScript, OpenAiStep};
use wcore_eval_scenarios::providers::{ProviderConfig, ProviderId};
use wcore_eval_scenarios::runner::{discover_binary, run_with_binary_in_paths};
use wcore_eval_scenarios::scenario::{Category, Scenario, Turn};

/// The packaged binary under test. Mirrors `packaged_driver_gate::packaged_core`.
fn packaged_core() -> PathBuf {
    discover_binary().expect("packaged-driver gate requires WCORE_EVAL_BIN or a built binary")
}

fn selftest_refusal() -> bool {
    std::env::var("WAYLAND_F23A_SELFTEST").as_deref() == Ok("refusal")
}

/// Fail the way the self-test contract requires: print the tripped marker so
/// the platform wrapper's gate can read it out of captured stdout, then panic
/// so the process exits nonzero.
fn trip_selftest(detail: &str) -> ! {
    println!("F23A-SELFTEST-TRIPPED: refusal");
    panic!("F23A self-test injected a refusal-control failure: {detail}");
}

/// A tempdir home + project pair with the skills lifecycle enabled on BOTH
/// sources, so the global/project merge resolves it on. Extracted from the
/// `LifecycleMatrixEnv` shape already proven in `packaged_driver_gate.rs`
/// rather than a second, drifting copy of the config strings.
struct BoundaryEnv {
    _root: tempfile::TempDir,
    home: PathBuf,
    project: PathBuf,
}

impl BoundaryEnv {
    fn build() -> Self {
        let root = tempfile::tempdir().expect("boundary env root");
        let home = root.path().join("home");
        let project = root.path().join("project");
        let project_config = project.join(".wayland-core");
        let sessions = home.join("sessions");
        fs::create_dir_all(&home).expect("home");
        fs::create_dir_all(&project_config).expect("project config");
        fs::create_dir_all(&sessions).expect("sessions");

        let session_path = sessions.to_string_lossy().replace('\\', "\\\\");
        fs::write(
            home.join("config.toml"),
            format!(
                "[session]\ndirectory = \"{session_path}\"\n\n\
                 [memory]\nenabled = true\n\n\
                 [observability]\nskills_lifecycle = true\n\n\
                 [provider.openai]\nmodel = \"fixture-chat-v1\"\n"
            ),
        )
        .expect("write global config");
        fs::write(
            project_config.join("config.toml"),
            "[observability]\nskills_lifecycle = true\n",
        )
        .expect("write project config");

        Self {
            _root: root,
            home,
            project,
        }
    }

    /// Seed a genuinely USER-AUTHORED skill. This is the positive control and
    /// it is not a draft: it carries ordinary frontmatter, no `auto-` prefix,
    /// no released-draft body marker and no `auto_drafted` manifest, so the
    /// loader must classify it as user content and leave it model-visible.
    /// Without it the suite cannot tell "generated content is quarantined"
    /// apart from "skills are broken".
    fn seed_user_authored_control(&self, name: &str, needle: &str) {
        let dir = self.home.join("skills").join(name);
        fs::create_dir_all(&dir).expect("control skill dir");
        fs::write(
            dir.join("SKILL.md"),
            format!(
                "---\nname: {name}\ndescription: User-authored control skill {needle}\n\
                 when-to-use: when the boundary drive needs a positive control\n---\n\n\
                 This body is user-authored. Marker {needle}.\n"
            ),
        )
        .expect("write control skill");
    }

    /// Every `auto-*` directory under the seeded home's skills dir. Mirrors
    /// `packaged_driver_gate::LifecycleMatrixEnv::generated_artifacts`.
    fn generated_artifacts(&self) -> Vec<PathBuf> {
        let skills = self.home.join("skills");
        let Ok(entries) = fs::read_dir(skills) else {
            return Vec::new();
        };
        entries
            .filter_map(Result::ok)
            .map(|entry| entry.path())
            .filter(|path| {
                path.file_name()
                    .and_then(|name| name.to_str())
                    .is_some_and(|name| name.starts_with("auto-"))
            })
            .collect()
    }
}

fn provider(base_url: &str, key: &str) -> ProviderConfig {
    ProviderConfig::new(ProviderId::OpenAI, "fixture-chat-v1")
        .with_api_key(key)
        .with_known_free_cost()
        .with_base_url(base_url)
}

/// Read every file under a directory tree into one string, so a nonce can be
/// searched for across an artifact set without guessing filenames.
fn read_tree(root: &Path) -> String {
    let mut out = String::new();
    let Ok(entries) = fs::read_dir(root) else {
        return out;
    };
    for entry in entries.filter_map(Result::ok) {
        let path = entry.path();
        if path.is_dir() {
            out.push_str(&read_tree(&path));
        } else if let Ok(text) = fs::read_to_string(&path) {
            out.push_str(&text);
        }
    }
    out
}

// ---------------------------------------------------------------------------
// Probe 1 — a refused `Skill` tool call must not take the session with it.
// ---------------------------------------------------------------------------

/// Success Criterion 1 says a generated skill *cannot execute*. The product's
/// refusal is a `Skill` tool result carrying `is_error: true`. That refusal is
/// only a usable security property if the session SURVIVES it: a refusal that
/// kills the session is a denial-of-service triggered by the security control
/// itself, and it also destroys the operator's ability to observe what was
/// refused (the second half of the same criterion).
///
/// This probe deliberately uses a name that is absent from the catalog rather
/// than a generated draft, so it isolates the "refused Skill tool result"
/// behaviour from the quarantine classifier. Both reach the identical
/// `resolve_for_model` → `NotFound` → error-`ToolResult` path
/// (`skill_tool.rs:187-199`).
#[tokio::test]
async fn refused_skill_tool_call_does_not_kill_the_session() {
    let core = packaged_core();
    let env = BoundaryEnv::build();

    let fixture = OpenAiFixtureScript::new([
        OpenAiStep::tool_call(
            "absent-skill-probe",
            "Skill",
            serde_json::json!({ "skill": "f23a-no-such-skill" }),
        ),
        OpenAiStep::text("SESSION_SURVIVED"),
    ])
    .start()
    .await
    .expect("start absent-skill fixture");

    let scenario = Scenario::new("f23a_refused_skill_survives", Category::Hardening)
        .max_total_time(std::time::Duration::from_secs(45))
        .max_total_cost_usd(0.0)
        .turn(
            Turn::new("use the f23a-no-such-skill skill")
                .assert(Assertion::Contains("SESSION_SURVIVED")),
        );

    let result = run_with_binary_in_paths(
        &scenario,
        &provider(fixture.base_url(), "f23a-absent-key"),
        &core,
        &env.project,
        &env.home,
    )
    .await
    .expect("absent-skill probe ran");
    let _ = fixture.shutdown().await;

    let probe = result
        .trace
        .entries
        .iter()
        .find(|entry| entry.call_id == "absent-skill-probe")
        .expect("the Skill tool call reached the product");
    assert_eq!(probe.tool_name, "Skill");
    assert!(
        probe.is_error,
        "an absent skill must be refused, not executed: {}",
        probe.output
    );
    assert!(
        probe.output.contains("not found"),
        "refusal should say not found, got: {}",
        probe.output
    );

    // The load-bearing clause. If the engine tears the session down after the
    // refusal, the scenario never reaches its second step and the assertion
    // below is what reports it.
    assert!(
        result.passed,
        "a refused Skill tool call must leave the session usable — the turn after \
         the refusal never completed. failures={:?} stderr_tail={}",
        result.failures, result.stderr_tail
    );
}

/// Scope discriminator for the defect Probe 1 exposes.
///
/// If an errored `Read` also kills the session, the defect belongs to the
/// generic tool-dispatch/journal path and is NOT specific to the skills
/// surface — which changes both its owner and its severity. If only `Skill`
/// does, it is this phase's. Recording which one it is, rather than assuming,
/// is the whole point of running it.
#[tokio::test]
async fn refused_read_tool_call_does_not_kill_the_session() {
    let core = packaged_core();
    let env = BoundaryEnv::build();

    let fixture = OpenAiFixtureScript::new([
        OpenAiStep::tool_call(
            "absent-file-probe",
            "Read",
            serde_json::json!({ "file_path": "/f23a/definitely/not/a/real/path.txt" }),
        ),
        OpenAiStep::text("SESSION_SURVIVED"),
    ])
    .start()
    .await
    .expect("start absent-file fixture");

    let scenario = Scenario::new("f23a_refused_read_survives", Category::Hardening)
        .max_total_time(std::time::Duration::from_secs(45))
        .max_total_cost_usd(0.0)
        .turn(Turn::new("read the missing file").assert(Assertion::Contains("SESSION_SURVIVED")));

    let result = run_with_binary_in_paths(
        &scenario,
        &provider(fixture.base_url(), "f23a-read-key"),
        &core,
        &env.project,
        &env.home,
    )
    .await
    .expect("absent-file probe ran");
    let _ = fixture.shutdown().await;

    let probe = result
        .trace
        .entries
        .iter()
        .find(|entry| entry.call_id == "absent-file-probe")
        .expect("the Read tool call reached the product");
    assert!(
        probe.is_error,
        "reading a missing file must be an error: {}",
        probe.output
    );
    assert!(
        result.passed,
        "a refused Read tool call must leave the session usable. \
         failures={:?} stderr_tail={}",
        result.failures, result.stderr_tail
    );
}

// ---------------------------------------------------------------------------
// Probe 2 — the product's own draft, refused at every model-facing route,
// with a user-authored positive control that must succeed.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn generated_draft_is_refused_at_every_route_while_user_content_is_not() {
    let core = packaged_core();
    let env = BoundaryEnv::build();
    // A run-time nonce. A hardcoded needle can be matched by a stale artifact
    // from an earlier run, which makes every absence assertion vacuous.
    let nonce = format!(
        "F23ANONCE{}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("clock")
            .as_nanos()
    );
    env.seed_user_authored_control("f23a-control", &nonce);

    // --- Stage 1: provoke the PRODUCT's own drafting path. -----------------
    let gen_fixture = OpenAiFixtureScript::new([
        OpenAiStep::text("ACK"),
        OpenAiStep::text("ACK"),
        OpenAiStep::text("ACK"),
    ])
    .start()
    .await
    .expect("start generation fixture");

    let generation = Scenario::new("f23a_boundary_generation", Category::Hardening)
        .max_total_time(std::time::Duration::from_secs(60))
        .max_total_cost_usd(0.0)
        .turn(Turn::new("Repeat this exact safe operation").assert(Assertion::Contains("ACK")))
        .turn(Turn::new("Repeat this exact safe operation").assert(Assertion::Contains("ACK")))
        .turn(Turn::new("Repeat this exact safe operation").assert(Assertion::Contains("ACK")));

    let generated = run_with_binary_in_paths(
        &generation,
        &provider(gen_fixture.base_url(), "f23a-generation-key"),
        &core,
        &env.project,
        &env.home,
    )
    .await
    .expect("generation run");
    let _ = gen_fixture.shutdown().await;
    assert!(
        generated.passed,
        "the drafting run failed before a draft could exist: {:?}",
        generated.failures
    );

    let drafts = env.generated_artifacts();
    assert_eq!(
        drafts.len(),
        1,
        "the product's own drafting path must produce exactly one draft; \
         hand-seeding one instead is forbidden. found: {drafts:?}"
    );
    let manifest: serde_json::Value =
        serde_json::from_slice(&fs::read(drafts[0].join("manifest.json")).expect("read manifest"))
            .expect("parse manifest");
    assert_eq!(manifest["auto_drafted"], true);
    assert_eq!(manifest["needs_review"], true);
    let draft_name = manifest["name"].as_str().expect("draft name").to_string();

    // The self-test substitutes the user-authored control where the draft is
    // expected. Every refusal assertion below is then handed content that
    // legitimately DOES resolve, so a driver whose refusal assertions never
    // actually fire will stay green here — and that is the failure this
    // switch exists to expose.
    let probed_name = if selftest_refusal() {
        "f23a-control".to_string()
    } else {
        draft_name.clone()
    };

    // --- Stage 2: drive every model-facing route at the draft. -------------
    let catalog_fixture = OpenAiFixtureScript::new([
        OpenAiStep::tool_call(
            "quarantined-skill-probe",
            "Skill",
            serde_json::json!({ "skill": probed_name.clone() }),
        ),
        OpenAiStep::tool_call(
            "control-skill-probe",
            "Skill",
            serde_json::json!({ "skill": "f23a-control" }),
        ),
        OpenAiStep::text("ROUTES_DRIVEN"),
    ])
    .start()
    .await
    .expect("start catalog fixture");

    let routes = Scenario::new("f23a_boundary_routes", Category::Hardening)
        .max_total_time(std::time::Duration::from_secs(60))
        .max_total_cost_usd(0.0)
        .turn(Turn::new(format!("/skill run {probed_name}")))
        .turn(Turn::new("/skill list"))
        .turn(Turn::new(format!("/skill show {probed_name}")))
        .turn(Turn::new("drive the skill routes").assert(Assertion::Contains("ROUTES_DRIVEN")));

    let driven = run_with_binary_in_paths(
        &routes,
        &provider(catalog_fixture.base_url(), "f23a-routes-key"),
        &core,
        &env.project,
        &env.home,
    )
    .await
    .expect("route drive ran");
    let _ = catalog_fixture.shutdown().await;

    let info = driven.info_events.join("\n");

    // Route: /skill run — an explicit refusal the operator can read.
    let run_refused = info.contains("quarantined and cannot be run");
    if !run_refused && selftest_refusal() {
        trip_selftest("/skill run refusal did not fire for the substituted control");
    }
    assert!(
        run_refused,
        "/skill run on a quarantined draft must refuse in words the operator can read. \
         info_events=\n{info}"
    );

    // Route: /skill list — present, tagged hidden. The operator must still be
    // able to SEE what is quarantined; that is the other half of Criterion 1.
    let listed_hidden = info.contains(&probed_name) && info.contains("(hidden)");
    if !listed_hidden && selftest_refusal() {
        trip_selftest("/skill list did not tag the substituted control hidden");
    }
    assert!(
        listed_hidden,
        "/skill list must show the draft AND tag it hidden. info_events=\n{info}"
    );

    // Route: /skill show — metadata visible, body NOT disclosed.
    let shown_hidden = info.contains("visibility: hidden from model");
    if !shown_hidden && selftest_refusal() {
        trip_selftest("/skill show did not report the substituted control hidden");
    }
    assert!(
        shown_hidden,
        "/skill show must report the draft hidden from the model. info_events=\n{info}"
    );

    // Route: Skill tool call — not-found to the model, and no body disclosure.
    let probe = driven
        .trace
        .entries
        .iter()
        .find(|entry| entry.call_id == "quarantined-skill-probe")
        .expect("the quarantined Skill tool call reached the product");
    if !probe.is_error && selftest_refusal() {
        trip_selftest("the Skill tool executed the substituted control as if quarantined");
    }
    assert!(
        probe.is_error,
        "a quarantined draft must not execute through the Skill tool: {}",
        probe.output
    );
    assert!(
        !probe.output.contains("Repeat this exact safe operation"),
        "the refusal disclosed the draft body it exists to withhold: {}",
        probe.output
    );

    // The draft on disk carries the interpolated user text; the refusal path
    // must not have leaked it, and the control's nonce must not have travelled
    // into the quarantined artifact.
    let draft_body = fs::read_to_string(drafts[0].join("SKILL.md")).expect("read draft body");
    assert!(
        !probe.output.contains(draft_body.trim()),
        "the refusal echoed the whole draft body: {}",
        probe.output
    );

    // --- The positive control. --------------------------------------------
    // Without this, a build in which every skill is broken would look
    // identical to a build in which quarantine works.
    let control = driven
        .trace
        .entries
        .iter()
        .find(|entry| entry.call_id == "control-skill-probe")
        .expect("the control Skill tool call reached the product");
    assert!(
        !control.is_error,
        "the user-authored control skill must RESOLVE — a run where everything is \
         refused is indistinguishable from a broken build: {}",
        control.output
    );
    assert!(
        control.output.contains(&nonce),
        "the control skill's body must reach the model, nonce absent from: {}",
        control.output
    );

    assert!(
        driven.passed,
        "the route drive did not complete: failures={:?} stderr_tail={}",
        driven.failures, driven.stderr_tail
    );

    // A last absence check across the whole seeded home: the nonce belongs to
    // the user-authored control only, and must not have been absorbed into the
    // generated artifact.
    let generated_tree = read_tree(&drafts[0]);
    assert!(
        !generated_tree.contains(&nonce),
        "the control's nonce was absorbed into the generated draft artifact"
    );
}
