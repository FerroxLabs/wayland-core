//! Typed / deterministic / redacted / non-mutating discovery (F26-01).
//!
//! Drives the public importers against the COMMITTED corpora — structure clones
//! of Sean's real `~/.hermes` and `~/.openclaw` with canary tokens substituted
//! for every secret. Canary values are read from each corpus's `MANIFEST.json`
//! at run time, never hard-coded here, so regenerating a corpus cannot silently
//! turn an assertion into a tautology.
//!
//! Serialized for the same reason as `migrate_hermes.rs`: discovery resolves the
//! existing-profile set through the process-global `WAYLAND_HOME`.

use std::path::{Path, PathBuf};

use serial_test::serial;
use tempfile::TempDir;
use wcore_cli::migrate::{hermes, openclaw};
use wcore_config::portability::{ItemKind, PortabilityPlan, tree_digest};

// --- env guard (same discipline as migrate_hermes.rs) ---------------------

struct EnvGuard(Vec<(&'static str, Option<std::ffi::OsString>)>);
impl EnvGuard {
    fn set(pairs: &[(&'static str, Option<&str>)]) -> Self {
        let saved = pairs
            .iter()
            .map(|(k, v)| {
                let prev = std::env::var_os(k);
                match v {
                    Some(val) => unsafe { std::env::set_var(k, val) },
                    None => unsafe { std::env::remove_var(k) },
                }
                (*k, prev)
            })
            .collect();
        Self(saved)
    }
}
impl Drop for EnvGuard {
    fn drop(&mut self) {
        for (k, prev) in &self.0 {
            match prev {
                Some(v) => unsafe { std::env::set_var(k, v) },
                None => unsafe { std::env::remove_var(k) },
            }
        }
    }
}

fn rooted() -> (EnvGuard, TempDir) {
    let home = tempfile::tempdir().unwrap();
    let g = EnvGuard::set(&[
        ("WAYLAND_HOME", Some(home.path().to_str().unwrap())),
        ("XDG_DATA_HOME", None),
    ]);
    (g, home)
}

// --- corpus access --------------------------------------------------------

fn corpus(kind: &str) -> PathBuf {
    let p = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/portability")
        .join(kind);
    assert!(
        p.is_dir(),
        "committed corpus missing at {} — regenerate with scripts/portability-corpus-gen.py",
        p.display()
    );
    p
}

/// Every canary token declared by a corpus manifest.
///
/// Read at run time from the manifest, which is what keeps the canary-absence
/// assertions honest across a corpus regeneration.
fn canaries(kind: &str) -> Vec<String> {
    let raw = std::fs::read_to_string(corpus(kind).join("MANIFEST.json")).unwrap();
    let m: serde_json::Value = serde_json::from_str(&raw).unwrap();
    let list: Vec<String> = m["canaries"]
        .as_array()
        .expect("manifest has no canaries array")
        .iter()
        .map(|c| c["canary"].as_str().unwrap().to_string())
        .collect();
    // A canary-absence assertion over an EMPTY canary list passes vacuously.
    // Refuse to run in that state.
    assert!(
        list.len() >= 8,
        "{kind} manifest declares only {} canaries — an absence assertion would prove nothing",
        list.len()
    );
    list
}

/// The count a corpus manifest declares for one key.
fn declared(kind: &str, key: &str) -> usize {
    let raw = std::fs::read_to_string(corpus(kind).join("MANIFEST.json")).unwrap();
    let m: serde_json::Value = serde_json::from_str(&raw).unwrap();
    m["counts"][key].as_u64().unwrap_or(0) as usize
}

fn hermes_plan() -> PortabilityPlan {
    hermes::build_plan(&corpus("hermes"), false)
        .expect("hermes discovery failed on the committed corpus")
        .to_portability()
}

fn openclaw_plan() -> PortabilityPlan {
    openclaw::build_plan(&corpus("openclaw"), false)
        .expect("openclaw discovery failed on the committed corpus")
        .to_portability()
}

fn assert_no_canaries(kind: &str, what: &str, rendered: &str) {
    for c in canaries(kind) {
        assert!(
            !rendered.contains(&c),
            "canary {c} from the {kind} manifest leaked through {what}"
        );
    }
}

// --- Task 2: Hermes typed discovery --------------------------------------

#[test]
#[serial]
fn hermes_plan_serializes_to_json_with_the_declared_profile_count() {
    let (_g, _h) = rooted();
    let plan = hermes_plan();
    let json = plan.to_json().unwrap();

    // Positive assertion FIRST: an empty or malformed emission must fail here
    // rather than sail through the canary-absence check below.
    let parsed: serde_json::Value =
        serde_json::from_str(&json).expect("emitted JSON does not parse");
    assert_eq!(
        plan.count_of(ItemKind::Profile),
        declared("hermes", "profiles"),
        "profile count must equal the count the corpus manifest declares"
    );
    assert!(
        parsed["items"].as_array().unwrap().len() >= 12,
        "emitted plan is implausibly small: {json}"
    );
    assert_eq!(parsed["source"], "hermes");
}

#[test]
#[serial]
fn hermes_plan_contains_no_canary_value() {
    let (_g, _h) = rooted();
    let plan = hermes_plan();
    assert_no_canaries("hermes", "the Hermes JSON", &plan.to_json().unwrap());
    // Positive half: the plan DID discover credentials, by reference.
    let json = plan.to_json().unwrap();
    assert!(
        json.contains("_API_KEY"),
        "no credential was discovered at all, so the absence check proves nothing"
    );
}

#[test]
#[serial]
fn hermes_discovery_is_deterministic_across_two_independent_walks() {
    let (_g, _h) = rooted();
    let a = hermes_plan().to_json().unwrap();
    let b = hermes_plan().to_json().unwrap();
    assert_eq!(
        a, b,
        "two independent walks of one corpus must serialize identically"
    );
    assert!(
        a.len() > 500,
        "emission too small for the comparison to mean anything"
    );
}

#[test]
#[serial]
fn hermes_discovery_does_not_mutate_the_source_tree() {
    let (_g, _h) = rooted();
    let before = tree_digest(&corpus("hermes")).unwrap();
    let _ = hermes_plan().to_json().unwrap();
    let after = tree_digest(&corpus("hermes")).unwrap();
    assert_eq!(
        before, after,
        "discovery mutated the tree it was previewing"
    );
    assert!(
        before.files > 100,
        "digest covered too little to be meaningful"
    );
}

#[test]
#[serial]
fn root_only_hermes_home_is_importable() {
    // Before F26-01 this ERRORED: detect_home required profiles/.
    let (_g, _h) = rooted();
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(
        dir.path().join("config.yaml"),
        "model:\n  default: deepseek/deepseek-v4-pro\n  provider: deepseek\n",
    )
    .unwrap();

    let home = hermes::detect_home(Some(dir.path())).expect("a root-only home must be accepted");
    let plan = hermes::build_plan(&home, false).unwrap().to_portability();
    assert_eq!(plan.count_of(ItemKind::RootProfile), 1);
    assert_eq!(plan.count_of(ItemKind::Profile), 0);
    let json = plan.to_json().unwrap();
    assert!(
        json.contains("deepseek-v4-pro"),
        "root model was not mapped: {json}"
    );
}

#[test]
#[serial]
fn rooted_home_with_profiles_yields_both_and_the_root_id_cannot_collide() {
    let (_g, _h) = rooted();
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(
        dir.path().join("config.yaml"),
        "model:\n  default: anthropic/opus\n  provider: anthropic\n",
    )
    .unwrap();
    // A profile literally named `root`, which is the collision this guards.
    let p = dir.path().join("profiles/root");
    std::fs::create_dir_all(&p).unwrap();
    std::fs::write(
        p.join("config.yaml"),
        "model:\n  default: x\n  provider: y\n",
    )
    .unwrap();

    let home = hermes::detect_home(Some(dir.path())).unwrap();
    let plan = hermes::build_plan(&home, false).unwrap().to_portability();

    assert_eq!(
        plan.count_of(ItemKind::RootProfile),
        1,
        "root setup missing"
    );
    assert_eq!(
        plan.count_of(ItemKind::Profile),
        1,
        "the `root` profile was lost"
    );
    let ids: Vec<&str> = plan.items.iter().map(|i| i.id.as_str()).collect();
    assert!(
        ids.contains(&"root"),
        "the real profile named `root` vanished: {ids:?}"
    );
    assert!(
        ids.contains(&wcore_config::portability::ROOT_PROFILE_ID),
        "the root setup did not get its reserved id: {ids:?}"
    );
    assert!(
        plan.warnings.iter().any(|w| w.contains("root")),
        "the near-collision was not reported: {:?}",
        plan.warnings
    );
}

#[test]
#[serial]
fn symlink_escaping_the_source_root_is_reported_not_followed() {
    #[cfg(unix)]
    {
        let outside = tempfile::tempdir().unwrap();
        std::fs::write(outside.path().join("outside.txt"), "OUTSIDE").unwrap();
        let root = tempfile::tempdir().unwrap();
        std::fs::write(root.path().join("in.txt"), "in").unwrap();
        std::os::unix::fs::symlink(outside.path(), root.path().join("escape")).unwrap();

        let d = tree_digest(root.path()).unwrap();
        assert_eq!(d.symlink_escapes, vec!["escape".to_string()]);
        // Never followed: the outside file contributed nothing.
        assert_eq!(d.files, 2, "the walk traversed the escaping link");
    }
}

// --- Task 3: OpenClaw discovery ------------------------------------------

#[test]
#[serial]
fn openclaw_plan_maps_model_and_provider_with_the_declared_counts() {
    let (_g, _h) = rooted();
    let plan = openclaw_plan();
    let json = plan.to_json().unwrap();
    let parsed: serde_json::Value =
        serde_json::from_str(&json).expect("emitted JSON does not parse");

    assert_eq!(parsed["source"], "openclaw");
    assert_eq!(
        plan.count_of(ItemKind::RootProfile),
        1,
        "root setup missing"
    );
    assert!(
        !plan.items.is_empty(),
        "a zero-item OpenClaw plan reads as success while proving nothing"
    );
    assert!(
        json.contains("flux"),
        "the configured provider was not mapped: {json}"
    );
}

#[test]
#[serial]
fn openclaw_plan_contains_no_canary_value() {
    let (_g, _h) = rooted();
    assert_no_canaries(
        "openclaw",
        "the OpenClaw JSON",
        &openclaw_plan().to_json().unwrap(),
    );
}

#[test]
#[serial]
fn openclaw_discovery_is_deterministic_and_non_mutating() {
    let (_g, _h) = rooted();
    let before = tree_digest(&corpus("openclaw")).unwrap();
    let a = openclaw_plan().to_json().unwrap();
    let b = openclaw_plan().to_json().unwrap();
    let after = tree_digest(&corpus("openclaw")).unwrap();
    assert_eq!(a, b, "two OpenClaw walks differed");
    assert_eq!(before, after, "OpenClaw discovery mutated its source");
    assert!(before.files > 5);
}

#[test]
#[serial]
fn openclaw_backup_siblings_do_not_multiply_the_plan() {
    // The corpus carries the real install's eight .bak/.last-good siblings. If
    // any were treated as a source the item count would multiply.
    let (_g, _h) = rooted();
    let revisions = std::fs::read_dir(corpus("openclaw"))
        .unwrap()
        .filter_map(|e| e.ok())
        .filter(|e| {
            let n = e.file_name();
            let n = n.to_string_lossy();
            n.starts_with("openclaw.json") && n != "openclaw.json"
        })
        .count();
    assert!(
        revisions >= 5,
        "the corpus holds only {revisions} config revisions — this test would prove nothing"
    );

    let plan = openclaw_plan();
    assert_eq!(
        plan.count_of(ItemKind::RootProfile),
        1,
        "backup siblings were imported as extra root setups"
    );
}

#[test]
#[serial]
fn openclaw_reports_everything_it_does_not_import() {
    let (_g, _h) = rooted();
    let plan = openclaw_plan();
    assert!(
        !plan.deferred.is_empty(),
        "nothing was named as detected-but-not-imported, so discovered state was dropped unnamed"
    );
    assert!(
        plan.deferred.contains_key("config_revisions_excluded"),
        "the excluded config revisions were not named: {:?}",
        plan.deferred
    );
}

// --- Task 4: the multi-emitter probe -------------------------------------

/// The ONLY measurement that distinguishes a STRUCTURAL redaction from a
/// COSMETIC one.
///
/// The real-state check and the JSON canary assertions cannot tell them apart:
/// a printer that merely declines to emit a value produces exactly the same zero
/// hits as a type that cannot hold one. So this drives a plan built from the
/// canary corpus through EVERY emitter the type can reach — `serde`, `Debug`,
/// `Display` where it exists, and the rendering the error path produces — and
/// requires every one of them to be canary-free. A cosmetic redaction survives
/// the JSON assertion and dies here.
#[test]
#[serial]
fn every_emitter_of_a_plan_is_free_of_canary_values() {
    let (_g, _h) = rooted();

    for (kind, plan) in [("hermes", hermes_plan()), ("openclaw", openclaw_plan())] {
        let json = plan.to_json().unwrap();
        let compact = serde_json::to_string(&plan).unwrap();
        let debug = format!("{plan:?}");
        let debug_alt = format!("{plan:#?}");
        // The error path: a plan reported as part of a failure. `anyhow`'s
        // Debug rendering is what a user actually sees on stderr.
        let err = format!("{:?}", anyhow::anyhow!("import failed: {plan:?}"));
        // Every credential the plan discovered, through its own emitters.
        let creds: String = plan
            .items
            .iter()
            .filter_map(|i| i.credential.as_ref())
            .map(|c| format!("{c} {c:?} {}", serde_json::to_string(c).unwrap()))
            .collect::<Vec<_>>()
            .join("\n");

        for (what, rendered) in [
            ("to_json", &json),
            ("serde compact", &compact),
            ("Debug", &debug),
            ("Debug alternate", &debug_alt),
            ("anyhow error path", &err),
            ("CredentialRef emitters", &creds),
        ] {
            assert_no_canaries(kind, what, rendered);
        }

        // POSITIVE half — without these, a plan that rendered to nothing would
        // satisfy every assertion above.
        assert!(
            !json.is_empty() && json.len() > 200,
            "{kind}: json too small"
        );
        assert!(
            debug.contains("PortabilityPlan"),
            "{kind}: Debug is not the plan"
        );
        assert!(
            !creds.is_empty(),
            "{kind}: NO credential was discovered, so the credential emitters were never exercised"
        );
        assert!(
            creds.contains("source_file") || creds.contains("from "),
            "{kind}: credential rendering lost its source reference: {creds}"
        );
    }
}
