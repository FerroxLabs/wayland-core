//! Canary proof for the support bundle — threat T-24-03-01 (CRITICAL).
//!
//! # Why a canary and not a review of the redaction rules
//!
//! Reading the rules and agreeing they look right is how every redaction bug
//! ships. The rules are read by the person who wrote them, against the sources
//! they thought of. A canary is read by the machine, against the sources that
//! actually exist.
//!
//! # Every canary assertion here carries a POSITIVE CONTROL
//!
//! The sentinel must be proved PRESENT in the seeded inputs before its absence
//! from the output means anything. Without that, a seeding step that silently
//! did nothing produces a triumphantly clean scan — a canary that never
//! travelled the path is trivially absent from the far end, and that is the
//! same false green in a different costume.

use std::path::{Path, PathBuf};

use wcore_gateway::support_bundle::{
    BundleSources, Redactor, bundle_files, collect, name_marks_secret,
};

/// The sentinel. Long and structured so a partial match is still a match, and
/// so it cannot occur by accident in any legitimate byte of a bundle.
const CANARY: &str = "F24D-SUPPORT-BUNDLE-CANARY-0e3b91c7a45d28f6";

/// Read every file in the bundle and return the ones containing `needle`.
///
/// Reads BYTES, not lines, and does not care whether a file is text: a secret
/// that survived into a binary member is still a leak, and a scan that only
/// looked at `.txt` files would miss it.
fn members_containing(root: &Path, needle: &str) -> Vec<PathBuf> {
    bundle_files(root)
        .into_iter()
        .filter(|p| {
            std::fs::read(p)
                .map(|bytes| bytes.windows(needle.len()).any(|w| w == needle.as_bytes()))
                .unwrap_or(false)
        })
        .collect()
}

/// Seed a home whose EVERY input carries the canary.
fn seed_home(dir: &Path) -> BundleSources {
    let config = dir.join("config.toml");
    std::fs::write(
        &config,
        format!(
            "[providers.anthropic]\napi_key = \"{CANARY}\"\nbase_url = \"https://x\"\n\
             [storage]\nnote = \"{CANARY}\"\n"
        ),
    )
    .unwrap();

    let credentials = dir.join("credentials.toml");
    std::fs::write(
        &credentials,
        format!("\"providers.anthropic.api_key\" = \"{CANARY}\"\n"),
    )
    .unwrap();

    let log = dir.join("gateway.log");
    std::fs::write(
        &log,
        format!(
            "[gateway] started\n[gateway] auth failed using token {CANARY}\n\
             [gateway] retrying\n"
        ),
    )
    .unwrap();

    let status = dir.join("gateway-status.json");
    std::fs::write(
        &status,
        format!(r#"{{"state":"running","note":"token {CANARY} rejected"}}"#),
    )
    .unwrap();

    let health = dir.join("channel-health.json");
    std::fs::write(
        &health,
        format!(r#"{{"configured":1,"registered":0,"registration_error":"bad {CANARY}"}}"#),
    )
    .unwrap();

    BundleSources {
        config: Some(config),
        credentials: Some(credentials),
        log: Some(log),
        projections: vec![status, health],
    }
}

#[test]
fn the_canary_appears_in_no_byte_of_any_bundle_member() {
    let dir = tempfile::tempdir().unwrap();
    let sources = seed_home(dir.path());

    // ---- POSITIVE CONTROL. Prove the canary really is in every input first.
    let seeded: Vec<PathBuf> = std::iter::empty()
        .chain(sources.config.clone())
        .chain(sources.credentials.clone())
        .chain(sources.log.clone())
        .chain(sources.projections.iter().cloned())
        .collect();
    assert_eq!(seeded.len(), 5, "five inputs are supposed to be seeded");
    for path in &seeded {
        let raw = std::fs::read_to_string(path).unwrap();
        assert!(
            raw.contains(CANARY),
            "POSITIVE CONTROL FAILED: {} does not contain the canary, so its \
             absence downstream would prove nothing",
            path.display()
        );
    }

    let mut redactor = Redactor::new();
    assert!(
        redactor.learn(CANARY),
        "the scrubber must accept the canary, or the log leg is untested"
    );

    let out = dir.path().join("bundle");
    let manifest = collect(dir.path(), &out, &sources, &redactor).unwrap();

    // The scan covers EVERY file in the tree, including the manifest.
    let leaked = members_containing(&out, CANARY);
    assert!(
        leaked.is_empty(),
        "the support bundle leaked the canary in: {leaked:?}"
    );

    // And the bundle is not empty — a bundle with no members is trivially
    // canary-free and proves nothing.
    assert!(
        !manifest.members.is_empty(),
        "an empty bundle passes a canary scan without protecting anything"
    );
    assert!(
        bundle_files(&out).len() >= 5,
        "expected at least five members plus the manifest, got {:?}",
        bundle_files(&out)
    );
}

#[test]
fn the_scan_itself_can_detect_a_leak() {
    // The scanner is the instrument every other assertion depends on, so it
    // gets its own control: plant the canary in a bundle-shaped directory and
    // confirm the scan FINDS it. Without this, a scanner that always returned
    // an empty list would make every redaction test above pass.
    let dir = tempfile::tempdir().unwrap();
    let out = dir.path().join("bundle");
    std::fs::create_dir_all(out.join("nested")).unwrap();
    std::fs::write(out.join("clean.txt"), "nothing here").unwrap();
    std::fs::write(out.join("nested/leaky.bin"), format!("xx{CANARY}xx")).unwrap();

    let found = members_containing(&out, CANARY);
    assert_eq!(
        found.len(),
        1,
        "the scanner must find a planted leak: {found:?}"
    );
    assert!(
        found[0].ends_with("nested/leaky.bin"),
        "and must recurse into subdirectories: {found:?}"
    );
}

#[test]
fn a_secret_the_scrubber_never_learned_is_still_elided_structurally() {
    // The load-bearing property of the two-layer design. Structural elision
    // must hold for secrets the redactor knows NOTHING about, because in
    // production that is most of them.
    let dir = tempfile::tempdir().unwrap();
    let sources = seed_home(dir.path());
    let out = dir.path().join("bundle");

    // An EMPTY scrubber. Nothing is scrubbed; only elision protects.
    let manifest = collect(dir.path(), &out, &sources, &Redactor::new()).unwrap();
    assert_eq!(manifest.known_secrets, 0, "the scrubber really is empty");
    assert_eq!(manifest.redactions, 0);

    // The config and credentials members must still be clean, because their
    // values were never read.
    for member in ["config-keys.txt", "credential-keys.txt"] {
        let body = std::fs::read_to_string(out.join(member)).unwrap();
        assert!(
            !body.contains(CANARY),
            "{member} leaked a secret the scrubber never learned — structural \
             elision is the only defence that works on unknown secrets"
        );
        assert!(
            !body.is_empty(),
            "{member} is empty, so its cleanliness proves nothing"
        );
    }
    assert!(
        std::fs::read_to_string(out.join("config-keys.txt"))
            .unwrap()
            .contains("api_key"),
        "the KEY NAME must survive — a bundle that elides the names too is \
         useless for support"
    );

    // And the free-text members DO leak without a scrubber, which is exactly
    // why the scrubber exists and why this test does not claim more than it
    // measured.
    let leaked = members_containing(&out, CANARY);
    assert!(
        !leaked.is_empty(),
        "with an empty scrubber the LOG leg is expected to leak; if it does \
         not, this test is no longer measuring what it claims"
    );
    assert!(
        leaked.iter().all(|p| {
            let n = p.file_name().unwrap().to_string_lossy().to_string();
            n == "recent-log.txt" || n.ends_with(".json")
        }),
        "only the free-text and projection members may leak without a \
         scrubber; a key-name member leaking would be an elision bug: {leaked:?}"
    );
}

#[test]
fn the_environment_member_carries_names_and_no_values() {
    // SAFETY: single-threaded within this test; the variable is removed before
    // the test returns.
    let var = "F24D_BUNDLE_CANARY_TOKEN";
    unsafe { std::env::set_var(var, CANARY) };
    assert!(name_marks_secret(var), "the name must mark it as secret");
    assert_eq!(
        std::env::var(var).as_deref(),
        Ok(CANARY),
        "POSITIVE CONTROL: the variable really is set to the canary"
    );

    let dir = tempfile::tempdir().unwrap();
    let out = dir.path().join("bundle");
    let _ = collect(
        dir.path(),
        &out,
        &BundleSources::default(),
        &Redactor::new(),
    )
    .unwrap();
    let body = std::fs::read_to_string(out.join("environment-keys.txt")).unwrap();

    unsafe { std::env::remove_var(var) };

    assert!(body.contains(var), "the NAME must be present: {var}");
    assert!(
        !body.contains(CANARY),
        "the environment member leaked a value"
    );
}

/// The LIVE entry point.
///
/// Takes its inputs from the environment and is driven against a bundle
/// produced by a RUNNING gateway. It FAILS rather than skips when the
/// variables are absent, so a live gate is bound to an exit status instead of
/// to a line of prose in a document the executor wrote.
///
/// Run with:
/// ```text
/// F24_LIVE_BUNDLE=<dir> F24_LIVE_CANARY_FILE=<file> F24_LIVE_SEEDED_DIR=<dir> \
///   cargo test -p wcore-gateway --test support_bundle_redaction -- --ignored live_bundle_canary
/// ```
#[test]
#[ignore = "live: requires a bundle produced by a running gateway"]
fn live_bundle_canary() {
    let bundle = std::env::var("F24_LIVE_BUNDLE")
        .expect("F24_LIVE_BUNDLE must be set; this gate FAILS rather than skips");
    let canary_file = std::env::var("F24_LIVE_CANARY_FILE")
        .expect("F24_LIVE_CANARY_FILE must be set; this gate FAILS rather than skips");
    let seeded_dir = std::env::var("F24_LIVE_SEEDED_DIR")
        .expect("F24_LIVE_SEEDED_DIR must be set; this gate FAILS rather than skips");

    let canary = std::fs::read_to_string(&canary_file)
        .unwrap_or_else(|e| panic!("cannot read canary file {canary_file}: {e}"));
    let canary = canary.trim();
    assert!(
        canary.len() >= 16,
        "a canary shorter than 16 bytes can occur by accident; got {} bytes",
        canary.len()
    );

    // POSITIVE CONTROL: the canary must be somewhere in the seeded inputs.
    let seeded_hits = members_containing(Path::new(&seeded_dir), canary);
    assert!(
        !seeded_hits.is_empty(),
        "POSITIVE CONTROL FAILED: the canary is in none of the seeded inputs \
         under {seeded_dir}, so its absence from the bundle proves nothing"
    );

    let bundle_path = Path::new(&bundle);
    let files = bundle_files(bundle_path);
    assert!(
        !files.is_empty(),
        "the bundle at {bundle} has no members; an empty bundle is trivially \
         canary-free"
    );

    let leaked = members_containing(bundle_path, canary);
    assert!(
        leaked.is_empty(),
        "the LIVE support bundle leaked the canary in: {leaked:?}"
    );
}
