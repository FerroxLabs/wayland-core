//! The hostile update-decision corpus, the key-rotation and compromise drill,
//! and the cross-implementation wire-format anti-drift guard.
//!
//! Three disciplines are load-bearing here and each exists because this
//! program has already paid for its absence.
//!
//! 1. **Every refusal carries a pristine control that is ACCEPTED.** A corpus
//!    made only of rejections passes trivially against a verifier that refuses
//!    everything, which would brick every legitimate update while looking
//!    green. `an_offer_newer_than_the_running_version_proceeds` is the corpus
//!    control and every mutation test re-proves its own control before
//!    mutating.
//!
//! 2. **No credential, ever.** Every key in this file is generated at run time
//!    from the OS CSPRNG into memory that dies with the test. No seed is
//!    printed, committed or passed as an argument. The bundled production
//!    trust root holds PUBLIC halves only; its secret halves live on the
//!    minting machine and in one CI secret, and neither is reachable from this
//!    file. Until 2026-07-29 that root shipped EMPTY and this file proved it
//!    was refused; the real root has now been substituted, so the refusal is
//!    proved against INJECTED placeholders instead
//!    (`a_placeholder_trust_root_is_refused_however_it_arrives`) and the
//!    bundled constant has its own guard
//!    (`the_bundled_trust_root_is_real_and_holds_only_the_acceptance_role`).
//!    Both must stay: the first keeps the refusal behaviour gated, the second
//!    catches a regression back to a placeholder.
//!
//! 3. **Hermetic by construction.** The persisted freeze state is addressed by
//!    an explicit path in EVERY test here, so no test can pollute another or
//!    the developer's real installation. The one assertion that has to write
//!    `WAYLAND_HOME` to make its point lives in its own binary,
//!    `tests/self_update_state_path.rs` — a process global written beside
//!    twenty-one siblings running as threads of the same `cargo test` process
//!    is not hermetic, whatever the test itself asserts.

use ed25519_dalek::SigningKey;
use rand::rngs::OsRng;
use tempfile::TempDir;

use wcore_cli::self_update::update_trust::{
    DEFAULT_MAX_MANIFEST_AGE_SECS, FreezeState, RELEASE_MANIFEST_ROLE, RELEASE_TRUST_ROOT_JSON,
    ReleaseVerifier, UpdateDecision, UpdateOffer, UpdateTrustError, VerifiedManifest,
    check_only_report, decide_update, parse_release_version,
};
use wcore_eval_scenarios::receipt::Evidence;
use wcore_eval_scenarios::release_integrity::{
    ArtifactKind, PackagedArtifactV1, ReleaseManifestBodyV1, ReleaseManifestV1,
    ReleaseRevocationV1, ReleaseState, ReleaseTrustRootV1, ReproducibilityVerdictV1,
    RevocationKind, TrustedKeyV1, verify_manifest,
};

// ---------------------------------------------------------------------------
// Harness — the CONSTRUCTION side. Nothing in this section exists in the
// shipped binary; `wcore-eval-scenarios` is a dev-dependency of `wcore-cli`,
// so the signing capability never enters the release artifact.
// ---------------------------------------------------------------------------

const NOW: u64 = 1_800_000_000;

/// A run-time trust root with one active release-acceptance key.
struct Signer {
    key_id: String,
    key: SigningKey,
    root: ReleaseTrustRootV1,
}

impl Signer {
    fn fresh(key_id: &str) -> Self {
        let key = SigningKey::generate(&mut OsRng);
        let root = ReleaseTrustRootV1::new(vec![trusted_key(key_id, &key, 0, None)]);
        Self {
            key_id: key_id.to_string(),
            key,
            root,
        }
    }

    /// Mint a signed manifest and return it as the bytes that travel on the
    /// wire — pretty-printed, exactly as `wayland-release` writes them.
    fn mint(&self, body: ReleaseManifestBodyV1) -> Vec<u8> {
        let manifest = ReleaseManifestV1::unsigned(body)
            .expect("harness body must be valid")
            .sign(&self.key_id, &self.key);
        serde_json::to_vec_pretty(&manifest).expect("manifest must encode")
    }

    fn verifier(&self) -> ReleaseVerifier {
        ReleaseVerifier::with_trust_root_json(
            &serde_json::to_vec(&self.root).expect("trust root must encode"),
        )
        .expect("a freshly generated trust root must be accepted")
    }
}

fn trusted_key(
    key_id: &str,
    key: &SigningKey,
    valid_from: u64,
    retired_at: Option<u64>,
) -> TrustedKeyV1 {
    use base64::Engine as _;
    TrustedKeyV1 {
        key_id: key_id.to_string(),
        public_key_base64: base64::engine::general_purpose::STANDARD
            .encode(key.verifying_key().to_bytes()),
        role: ReleaseState::ReleaseAcceptance,
        valid_from,
        retired_at,
    }
}

fn hexed(ch: char, n: usize) -> String {
    std::iter::repeat_n(ch, n).collect()
}

/// A structurally valid manifest body for release `version`.
fn body(version: &str, sequence: u64, issued_at: u64) -> ReleaseManifestBodyV1 {
    body_with_revocations(version, sequence, issued_at, Vec::new())
}

fn body_with_revocations(
    version: &str,
    sequence: u64,
    issued_at: u64,
    revocations: Vec<ReleaseRevocationV1>,
) -> ReleaseManifestBodyV1 {
    ReleaseManifestBodyV1 {
        release_id: format!("v{version}-wayland-base"),
        source_commit: hexed('a', 40),
        artifacts: vec![PackagedArtifactV1 {
            name: format!("wayland-core-v{version}-x86_64-unknown-linux-gnu.tar.gz"),
            sha256: hexed('b', 64),
            byte_length: 4096,
            kind: ArtifactKind::Archive,
        }],
        sbom: Evidence::Unavailable {
            code: "not_bound_in_this_fixture".to_string(),
        },
        dependency_policy: Evidence::Unavailable {
            code: "not_bound_in_this_fixture".to_string(),
        },
        reproducibility: ReproducibilityVerdictV1::Reproduced,
        certification: Evidence::Unavailable {
            code: "phase_28_certification_binding_not_yet_available".to_string(),
        },
        sequence,
        issued_at,
        revocations,
    }
}

fn revoke_version(version: &str, reason: &str) -> ReleaseRevocationV1 {
    ReleaseRevocationV1 {
        kind: RevocationKind::Version,
        value: version.to_string(),
        reason: reason.to_string(),
    }
}

/// The persisted state a machine that has never installed anything holds.
fn first_run() -> FreezeState {
    FreezeState::first_run()
}

fn offer<'a>(
    running: &'a str,
    offered: &'a str,
    manifest: Option<&'a VerifiedManifest>,
    state: &'a FreezeState,
) -> UpdateOffer<'a> {
    UpdateOffer {
        running_version: running,
        offered_version: offered,
        manifest,
        state,
        now_unix: NOW,
        max_manifest_age_secs: DEFAULT_MAX_MANIFEST_AGE_SECS,
    }
}

/// The control every mutation test re-proves before mutating: a pristine,
/// freshly signed, current manifest for a NEWER version is ACCEPTED.
fn pristine_control(signer: &Signer, running: &str, offered: &str) -> VerifiedManifest {
    let bytes = signer.mint(body(offered, 10, NOW - 60));
    let verified = signer
        .verifier()
        .verify_manifest_json(&bytes, NOW)
        .expect("CONTROL: a pristine manifest must verify");
    let state = first_run();
    let decision = decide_update(&offer(running, offered, Some(&verified), &state));
    assert!(
        decision.proceeds(),
        "CONTROL: a pristine newer offer must proceed, got {decision:?}"
    );
    verified
}

// ---------------------------------------------------------------------------
// Task 1 — the ordered update decision
// ---------------------------------------------------------------------------

#[test]
fn an_offer_older_than_the_running_version_is_refused() {
    let signer = Signer::fresh("release-acceptance-key");
    // Pristine control first: the same machinery accepts a forward move.
    pristine_control(&signer, "0.12.25", "0.13.0");

    let bytes = signer.mint(body("0.11.0", 10, NOW - 60));
    let verified = signer.verifier().verify_manifest_json(&bytes, NOW).unwrap();
    let state = first_run();
    let decision = decide_update(&offer("0.12.25", "0.11.0", Some(&verified), &state));

    assert!(
        matches!(decision, UpdateDecision::RefusedDowngrade { .. }),
        "a lower offer must be refused as a downgrade, got {decision:?}"
    );
    // The refusal names the DIRECTION, not a generic error.
    let message = decision.message();
    assert!(
        message.contains("older") || message.contains("downgrade"),
        "refusal must name the direction: {message}"
    );
    assert!(
        message.contains("0.11.0") && message.contains("0.12.25"),
        "{message}"
    );
}

#[test]
fn an_offer_equal_to_the_running_version_is_a_clean_no_op() {
    let signer = Signer::fresh("release-acceptance-key");
    let bytes = signer.mint(body("0.12.25", 10, NOW - 60));
    let verified = signer.verifier().verify_manifest_json(&bytes, NOW).unwrap();
    let state = first_run();

    let decision = decide_update(&offer("0.12.25", "0.12.25", Some(&verified), &state));
    assert!(
        matches!(decision, UpdateDecision::AlreadyUpToDate { .. }),
        "an equal offer must remain a clean no-op, got {decision:?}"
    );
    assert!(!decision.proceeds(), "a no-op must not install");

    // And it stays a clean no-op with no manifest at all, exactly as today:
    // the equality branch never needed one.
    let bare = decide_update(&offer("0.12.25", "0.12.25", None, &state));
    assert!(
        matches!(bare, UpdateDecision::AlreadyUpToDate { .. }),
        "{bare:?}"
    );
}

#[test]
fn an_offer_newer_than_the_running_version_proceeds() {
    // THE CORPUS CONTROL. Without a case that proceeds, every refusal in this
    // file would pass against an updater that refuses everything.
    let signer = Signer::fresh("release-acceptance-key");
    let bytes = signer.mint(body("0.13.0", 11, NOW - 60));
    let verified = signer.verifier().verify_manifest_json(&bytes, NOW).unwrap();
    let state = first_run();

    let decision = decide_update(&offer("0.12.25", "0.13.0", Some(&verified), &state));
    match &decision {
        UpdateDecision::Proceed {
            to_version,
            sequence,
        } => {
            assert_eq!(to_version, "0.13.0");
            assert_eq!(*sequence, 11);
        }
        other => panic!("a pristine newer offer must proceed, got {other:?}"),
    }
    assert!(decision.proceeds());
}

#[test]
fn a_manifest_sequence_at_or_below_the_high_water_mark_is_refused_as_stale() {
    let signer = Signer::fresh("release-acceptance-key");
    pristine_control(&signer, "0.12.25", "0.13.0");

    let temp = TempDir::new().unwrap();
    let path = temp.path().join("release-freeze-state.json");
    // The machine has already accepted sequence 20.
    let state = FreezeState::record_install_at(&path, 20, NOW - 3600).unwrap();
    assert_eq!(state.highest_sequence, 20);

    // A correctly signed, internally consistent, but STALE view: sequence 20
    // is not greater than the high-water mark, so a mirror freezing a user on
    // an old-but-valid manifest is caught.
    for stale_sequence in [1_u64, 19, 20] {
        let bytes = signer.mint(body("0.13.0", stale_sequence, NOW - 60));
        let verified = signer.verifier().verify_manifest_json(&bytes, NOW).unwrap();
        let decision = decide_update(&offer("0.12.25", "0.13.0", Some(&verified), &state));
        assert!(
            matches!(decision, UpdateDecision::RefusedStaleSequence { .. }),
            "sequence {stale_sequence} at or below mark 20 must be refused, got {decision:?}"
        );
        assert!(
            decision.message().contains("stale"),
            "{}",
            decision.message()
        );
    }

    // Control at the same high-water mark: one past it is accepted.
    let bytes = signer.mint(body("0.13.0", 21, NOW - 60));
    let verified = signer.verifier().verify_manifest_json(&bytes, NOW).unwrap();
    let decision = decide_update(&offer("0.12.25", "0.13.0", Some(&verified), &state));
    assert!(
        decision.proceeds(),
        "sequence 21 past mark 20 must proceed: {decision:?}"
    );
}

#[test]
fn a_manifest_older_than_the_maximum_age_is_refused_on_a_first_run() {
    let signer = Signer::fresh("release-acceptance-key");
    pristine_control(&signer, "0.12.25", "0.13.0");

    // No high-water mark exists yet, so age is the ONLY freeze protection
    // available. A mirror that froze this user on a year-old view is caught.
    let state = first_run();
    assert!(state.is_first_run());

    let issued = NOW - DEFAULT_MAX_MANIFEST_AGE_SECS - 1;
    let bytes = signer.mint(body("0.13.0", 11, issued));
    let verified = signer.verifier().verify_manifest_json(&bytes, NOW).unwrap();
    let decision = decide_update(&offer("0.12.25", "0.13.0", Some(&verified), &state));
    assert!(
        matches!(decision, UpdateDecision::RefusedOverAgeManifest { .. }),
        "an over-age manifest must be refused on a first run, got {decision:?}"
    );

    // Control: one second inside the window, same first-run state, accepted.
    let fresh = signer.mint(body("0.13.0", 11, NOW - DEFAULT_MAX_MANIFEST_AGE_SECS + 1));
    let verified = signer.verifier().verify_manifest_json(&fresh, NOW).unwrap();
    let decision = decide_update(&offer("0.12.25", "0.13.0", Some(&verified), &state));
    assert!(
        decision.proceeds(),
        "a manifest inside the window must proceed: {decision:?}"
    );
}

#[test]
fn a_revoked_version_is_refused_and_the_reason_is_surfaced() {
    let signer = Signer::fresh("release-acceptance-key");
    pristine_control(&signer, "0.12.25", "0.13.0");

    let reason = "sandbox escape in the bash tool";
    let bytes = signer.mint(body_with_revocations(
        "0.13.0",
        11,
        NOW - 60,
        vec![revoke_version("0.13.0", reason)],
    ));
    let verified = signer.verifier().verify_manifest_json(&bytes, NOW).unwrap();
    let state = first_run();
    let decision = decide_update(&offer("0.12.25", "0.13.0", Some(&verified), &state));

    assert!(
        matches!(decision, UpdateDecision::RefusedRevokedVersion { .. }),
        "a revoked offer must be refused, got {decision:?}"
    );
    let message = decision.message();
    assert!(
        message.contains(reason),
        "the revocation REASON must reach the user: {message}"
    );
}

#[test]
fn a_running_version_that_is_revoked_is_reported_on_check_only() {
    let signer = Signer::fresh("release-acceptance-key");
    let reason = "credential disclosure in the MCP transport";
    let bytes = signer.mint(body_with_revocations(
        "0.13.0",
        11,
        NOW - 60,
        vec![revoke_version("0.12.25", reason)],
    ));
    let verified = signer.verifier().verify_manifest_json(&bytes, NOW).unwrap();

    let report = check_only_report(Some(&verified), "0.12.25");
    let joined = report.join("\n");
    assert!(
        joined.contains(reason) && joined.contains("0.12.25"),
        "a revocation the user never learns about protects nobody: {joined}"
    );
    assert!(
        joined.to_ascii_uppercase().contains("REVOKED"),
        "the report must say REVOKED prominently: {joined}"
    );

    // Pristine control: a running version that is NOT revoked produces no
    // revocation line, so this test cannot pass against a reporter that
    // always shouts.
    let clean = check_only_report(Some(&verified), "0.13.0").join("\n");
    assert!(
        !clean.to_ascii_uppercase().contains("REVOKED"),
        "CONTROL: an unrevoked running version must not be reported: {clean}"
    );
}

#[test]
fn an_unorderable_version_string_is_refused_rather_than_guessed() {
    let signer = Signer::fresh("release-acceptance-key");
    pristine_control(&signer, "0.12.25", "0.13.0");
    let state = first_run();

    // A guess here installs something, so every one of these must refuse.
    for bad in [
        "", "latest", "0.13", "0.13.x", "0.13.0.1", "nightly", "v", "0.-1.0",
    ] {
        assert!(
            parse_release_version(bad).is_err(),
            "{bad:?} must not be orderable"
        );
        let decision = decide_update(&offer("0.12.25", bad, None, &state));
        assert!(
            matches!(decision, UpdateDecision::RefusedUnorderableVersion { .. }),
            "offered {bad:?} must be refused, got {decision:?}"
        );
        // ...and an unorderable RUNNING version is equally fatal: it is the
        // other operand of the same comparison.
        let decision = decide_update(&offer(bad, "0.13.0", None, &state));
        assert!(
            matches!(decision, UpdateDecision::RefusedUnorderableVersion { .. }),
            "running {bad:?} must be refused, got {decision:?}"
        );
    }

    // Controls: the shapes that ARE orderable, including the release-tag form
    // the GitHub API hands us and a pre-release ordered below its release.
    for good in ["0.13.0", "v0.13.0", "1.2.3-rc.1", "1.2.3+build.5"] {
        assert!(
            parse_release_version(good).is_ok(),
            "{good:?} must be orderable"
        );
    }
    let pre = parse_release_version("1.2.3-rc.1").unwrap();
    let rel = parse_release_version("1.2.3").unwrap();
    assert!(pre < rel, "a pre-release must order BELOW its own release");
    assert!(parse_release_version("0.9.0").unwrap() < parse_release_version("0.10.0").unwrap());
    assert!(parse_release_version("0.12.25").unwrap() < parse_release_version("0.13.0").unwrap());
}

#[test]
fn the_high_water_mark_advances_only_after_a_successful_install() {
    let temp = TempDir::new().unwrap();
    let path = temp.path().join("release-freeze-state.json");

    // A missing state file is a FIRST RUN, not an error.
    let state = FreezeState::load_from(&path);
    assert!(state.is_first_run());
    assert_eq!(state.highest_sequence, 0);
    assert!(!path.exists(), "merely deciding must not write state");

    // Deciding — even deciding to proceed — writes nothing.
    let signer = Signer::fresh("release-acceptance-key");
    let bytes = signer.mint(body("0.13.0", 11, NOW - 60));
    let verified = signer.verifier().verify_manifest_json(&bytes, NOW).unwrap();
    let decision = decide_update(&offer("0.12.25", "0.13.0", Some(&verified), &state));
    assert!(decision.proceeds());
    assert!(
        !path.exists(),
        "the mark must advance on a successful INSTALL, never on a decision"
    );

    // Only the install records it.
    let advanced = FreezeState::record_install_at(&path, 11, NOW).unwrap();
    assert_eq!(advanced.highest_sequence, 11);
    assert_eq!(FreezeState::load_from(&path).highest_sequence, 11);

    // It is a HIGH-water mark: a lower sequence never lowers it.
    let kept = FreezeState::record_install_at(&path, 5, NOW + 1).unwrap();
    assert_eq!(kept.highest_sequence, 11, "the mark must never regress");

    // An unreadable/garbage state file is a first run, not a hard error —
    // but it is a first run that still enforces the maximum-age rule.
    std::fs::write(&path, b"{ this is not json").unwrap();
    assert!(FreezeState::load_from(&path).is_first_run());
}

// ---------------------------------------------------------------------------
// Task 2 — the bundled trust root, rotation, and the anti-drift guard
// ---------------------------------------------------------------------------

#[test]
fn a_placeholder_trust_root_is_refused_however_it_arrives() {
    // The single most important assertion in this file, and it now has to be
    // written against INJECTED roots rather than the bundled one: the real
    // FerroxLabs root was substituted on 2026-07-29, so `bundled()` succeeds.
    // The refusal BEHAVIOUR is what must stay gated -- deleting these because
    // the constant changed would retire the guard along with the placeholder.
    let empty = r#"{"schema":"wayland.release.trust-root","schema_version":1,"keys":[]}"#;
    let error = ReleaseVerifier::with_trust_root_json(empty.as_bytes())
        .expect_err("an empty trust root must be REFUSED, never trusted");
    assert!(
        matches!(error, UpdateTrustError::PlaceholderTrustRoot(_)),
        "got {error:?}"
    );
    // RETAINED across the substitution. `PlaceholderTrustRoot`'s `#[error(..)]`
    // string names the constant unconditionally, so this assertion holds for an
    // INJECTED root exactly as it held for the bundled one -- dropping it with
    // the placeholder would have retired a live guard for no reason.
    let message = error.to_string();
    assert!(
        message.contains("RELEASE_TRUST_ROOT_JSON"),
        "the error must name what to replace: {message}"
    );

    // An all-zeros key is the Ed25519 identity point: signatures against it
    // can be forged with no secret. It must be refused exactly as the
    // marketplace index refuses its own placeholder (F-021).
    let zeros = format!(
        r#"{{"schema":"wayland.release.trust-root","schema_version":1,"keys":[{{"key_id":"k","public_key_base64":"{}","role":"release_acceptance","valid_from":0,"retired_at":null}}]}}"#,
        base64_zeros()
    );
    let error = ReleaseVerifier::with_trust_root_json(zeros.as_bytes())
        .expect_err("an all-zeros trust root must be REFUSED");
    assert!(
        matches!(error, UpdateTrustError::PlaceholderTrustRoot(_)),
        "got {error:?}"
    );

    // Pristine control: a trust root holding a real, run-time-generated key
    // constructs, so this test cannot pass against a constructor that refuses
    // everything.
    Signer::fresh("release-acceptance-key").verifier();
}

#[test]
fn the_bundled_trust_root_is_real_and_holds_only_the_acceptance_role() {
    // Substituted 2026-07-29. This is the assertion that would catch a
    // regression to the placeholder, a leaked secret half, or a widened trust
    // surface -- each of which is a different failure and each is checked.
    ReleaseVerifier::bundled().expect("the bundled trust root must now construct");

    let root: serde_json::Value =
        serde_json::from_str(RELEASE_TRUST_ROOT_JSON).expect("bundled root must be valid JSON");
    let keys = root["keys"].as_array().expect("keys must be an array");

    assert_eq!(
        keys.len(),
        1,
        "exactly one key belongs in the bundled root: {RELEASE_TRUST_ROOT_JSON}"
    );
    assert_eq!(
        keys[0]["role"], "release_acceptance",
        "only the role the updater accepts may be bundled -- packaging, \
         deployment_preparation and rollback_rehearsal could never authorise an \
         install and would widen the trust surface for no function"
    );
    assert_eq!(
        keys[0]["valid_from"], 0,
        "valid_from must vouch for the first release it signs, not exclude it"
    );

    assert!(
        keys[0]["retired_at"].is_null(),
        "a bundled key that is already retired would refuse every install"
    );

    // No seed may ride along. A grep for the WORD "seed" is nearly vacuous --
    // it passes on any document that simply spells the field differently -- so
    // the real check is STRUCTURAL: the key object carries EXACTLY the five
    // wire fields and nothing else, and the one key-shaped value present
    // decodes to a well-formed 32-byte Ed25519 point rather than to anything
    // else. `WireTrustedKey` is `deny_unknown_fields`, so a sixth field would
    // also fail `bundled()` above; this asserts it at the document level too.
    assert_eq!(
        field_names(&keys[0]),
        vec![
            "key_id",
            "public_key_base64",
            "retired_at",
            "role",
            "valid_from"
        ],
        "no field beyond the five wire fields may appear on a bundled key"
    );

    let raw = decode_base64(
        keys[0]["public_key_base64"]
            .as_str()
            .expect("must be a string"),
    );
    assert_eq!(raw.len(), 32, "a bundled key must be 32 bytes");
    assert!(
        raw.iter().any(|byte| *byte != 0),
        "the all-zeros identity point is a placeholder, not a key"
    );

    // The field-set check must be ABLE TO FAIL. Same invocation, same
    // extractor, one smuggled field: if this does not differ, the assertion
    // above proves nothing.
    let mut smuggled = keys[0].clone();
    smuggled["private_key_base64"] = serde_json::Value::String("x".to_string());
    assert_ne!(
        field_names(&smuggled),
        field_names(&keys[0]),
        "the field-set instrument is dead: it cannot see a smuggled field"
    );
}

/// Sorted field names of a JSON object, so the assertion does not depend on
/// serialization order.
fn field_names(value: &serde_json::Value) -> Vec<&str> {
    let mut names: Vec<&str> = value
        .as_object()
        .expect("a trusted key must be a JSON object")
        .keys()
        .map(String::as_str)
        .collect();
    names.sort_unstable();
    names
}

fn decode_base64(value: &str) -> Vec<u8> {
    use base64::Engine as _;
    base64::engine::general_purpose::STANDARD
        .decode(value)
        .expect("a bundled key must be valid standard base64")
}

fn base64_zeros() -> String {
    use base64::Engine as _;
    base64::engine::general_purpose::STANDARD.encode([0u8; 32])
}

#[test]
fn a_manifest_signed_by_an_unknown_key_id_is_refused() {
    let signer = Signer::fresh("release-acceptance-key");
    let control = signer.mint(body("0.13.0", 11, NOW - 60));
    signer
        .verifier()
        .verify_manifest_json(&control, NOW)
        .expect("CONTROL: the pristine manifest must verify");

    // Same key material, a key id the trust root has never heard of.
    let stranger = Signer::fresh("some-other-key-id");
    let bytes = stranger.mint(body("0.13.0", 11, NOW - 60));
    let error = signer
        .verifier()
        .verify_manifest_json(&bytes, NOW)
        .expect_err("an unknown key id must be refused");
    assert!(
        matches!(error, UpdateTrustError::UnknownKeyId(_)),
        "got {error:?}"
    );
}

#[test]
fn a_manifest_signed_by_a_retired_key_is_refused() {
    let signer = Signer::fresh("release-acceptance-key");
    let bytes = signer.mint(body("0.13.0", 11, NOW - 60));

    // Control: while the key is active the very same bytes verify.
    signer
        .verifier()
        .verify_manifest_json(&bytes, NOW)
        .expect("CONTROL: an active key must verify");

    // Retire it. The signature is still cryptographically valid — that is the
    // whole point: retirement must be ENFORCED at verification, not merely
    // recorded somewhere.
    let retired = ReleaseTrustRootV1::new(vec![trusted_key(
        &signer.key_id,
        &signer.key,
        0,
        Some(NOW - 1),
    )]);
    let verifier =
        ReleaseVerifier::with_trust_root_json(&serde_json::to_vec(&retired).unwrap()).unwrap();
    let error = verifier
        .verify_manifest_json(&bytes, NOW)
        .expect_err("a retired key must be refused");
    assert!(
        matches!(error, UpdateTrustError::RetiredKey(_)),
        "got {error:?}"
    );

    // A key that is not yet valid is equally refused.
    let future = ReleaseTrustRootV1::new(vec![trusted_key(
        &signer.key_id,
        &signer.key,
        NOW + 1,
        None,
    )]);
    let verifier =
        ReleaseVerifier::with_trust_root_json(&serde_json::to_vec(&future).unwrap()).unwrap();
    let error = verifier
        .verify_manifest_json(&bytes, NOW)
        .expect_err("not yet valid");
    assert!(
        matches!(error, UpdateTrustError::KeyNotYetValid(_)),
        "got {error:?}"
    );
}

#[test]
fn a_manifest_signed_by_a_newly_rotated_key_is_accepted() {
    // Rotation must be a real operation, not a redeploy: a key added to the
    // trust root works immediately.
    let key_a = SigningKey::generate(&mut OsRng);
    let key_b = SigningKey::generate(&mut OsRng);

    let rotated = ReleaseTrustRootV1::new(vec![
        trusted_key("release-key-a", &key_a, 0, Some(NOW - 1)),
        trusted_key("release-key-b", &key_b, NOW - 10, None),
    ]);
    let verifier =
        ReleaseVerifier::with_trust_root_json(&serde_json::to_vec(&rotated).unwrap()).unwrap();

    let signed_by_b = ReleaseManifestV1::unsigned(body("0.13.0", 11, NOW - 60))
        .unwrap()
        .sign("release-key-b", &key_b);
    let bytes = serde_json::to_vec_pretty(&signed_by_b).unwrap();
    verifier
        .verify_manifest_json(&bytes, NOW)
        .expect("a newly rotated key must be accepted immediately");

    // And the retired predecessor is refused against the SAME root, so this
    // test cannot pass against a verifier that accepts everything.
    let signed_by_a = ReleaseManifestV1::unsigned(body("0.13.0", 11, NOW - 60))
        .unwrap()
        .sign("release-key-a", &key_a);
    let bytes = serde_json::to_vec_pretty(&signed_by_a).unwrap();
    let error = verifier
        .verify_manifest_json(&bytes, NOW)
        .expect_err("retired");
    assert!(
        matches!(error, UpdateTrustError::RetiredKey(_)),
        "got {error:?}"
    );
}

#[test]
fn the_shipped_crate_exposes_no_manifest_signing_entry_point() {
    // A release binary that can mint a release-manifest signature is a
    // key-custody problem delivered to every user. The release-update surface
    // is `self_update.rs` + `update_trust.rs`; neither may contain an Ed25519
    // signing type or a signing call.
    //
    // Scope note, stated rather than hidden: `plugin/sign.rs` DOES carry a
    // signing key loader. That is the plugin-author signing surface on Phase
    // 25's lifecycle, it cannot sign a release manifest (different domain
    // separator, different trust root), and it is out of this plan's scope.
    let src = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    for file in ["self_update.rs", "update_trust.rs"] {
        let text =
            std::fs::read_to_string(src.join(file)).unwrap_or_else(|e| panic!("read {file}: {e}"));
        for forbidden in ["SigningKey", ".sign(", "Signer"] {
            assert!(
                !text.contains(forbidden),
                "{file} must contain no `{forbidden}` — the shipped updater VERIFIES, never signs"
            );
        }
    }

    // Control: the construction side really does exist, in the DEV-dependency
    // harness that never enters the shipped artifact.
    let harness = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../wcore-eval-scenarios/src/release_integrity.rs");
    let text = std::fs::read_to_string(harness).expect("read the harness");
    assert!(
        text.contains("SigningKey"),
        "CONTROL: the harness must be where signing lives"
    );
}

#[test]
fn a_harness_minted_manifest_verifies_under_the_shipped_verifier() {
    // THE ANTI-DRIFT GUARD. Two independent verifiers are a feature only if a
    // test pins the wire format they meet at. Both directions are proved:
    // what the harness accepts, the shipped verifier accepts; what the harness
    // rejects, the shipped verifier rejects.
    let signer = Signer::fresh("release-acceptance-key");
    let bytes = signer.mint(body("0.13.0", 11, NOW - 60));

    // Direction 1 — ACCEPT. The harness's own verifier accepts it...
    let harness_manifest: ReleaseManifestV1 = serde_json::from_slice(&bytes).unwrap();
    verify_manifest(
        &harness_manifest,
        &signer.root,
        ReleaseState::ReleaseAcceptance,
        NOW,
    )
    .expect("the harness must accept its own manifest");
    // ...and so does the shipped one, over the identical bytes.
    let shipped = signer
        .verifier()
        .verify_manifest_json(&bytes, NOW)
        .expect("the shipped verifier must accept a harness-minted manifest");
    assert_eq!(shipped.body_sha256(), harness_manifest.body_sha256);
    assert_eq!(shipped.sequence(), 11);
    assert_eq!(shipped.version_string(), "0.13.0");

    // Direction 2 — REJECT. Flip one byte of the signed body. The harness
    // rejects it (its digest no longer matches) and so must the shipped
    // verifier, independently.
    let mutated = String::from_utf8(bytes.clone())
        .unwrap()
        .replace("\"byte_length\": 4096", "\"byte_length\": 4097");
    assert_ne!(
        mutated.as_bytes(),
        bytes.as_slice(),
        "the mutation must actually change the document"
    );
    let harness_mutated: ReleaseManifestV1 = serde_json::from_slice(mutated.as_bytes()).unwrap();
    assert!(
        verify_manifest(
            &harness_mutated,
            &signer.root,
            ReleaseState::ReleaseAcceptance,
            NOW
        )
        .is_err(),
        "the harness must reject a mutated body"
    );
    let error = signer
        .verifier()
        .verify_manifest_json(mutated.as_bytes(), NOW)
        .expect_err("the shipped verifier must reject a mutated body");
    assert!(
        matches!(error, UpdateTrustError::BodyDigestMismatch),
        "got {error:?}"
    );

    // Direction 2b — a body swapped wholesale while keeping the signed digest
    // and signature. Without an independent digest recomputation the shipped
    // verifier would read sequence, age and revocations out of an
    // UNAUTHENTICATED body.
    let smuggled = String::from_utf8(bytes.clone())
        .unwrap()
        .replace("\"sequence\": 11", "\"sequence\": 99999");
    let error = signer
        .verifier()
        .verify_manifest_json(smuggled.as_bytes(), NOW)
        .expect_err("a swapped body under a valid signature must be refused");
    assert!(
        matches!(error, UpdateTrustError::BodyDigestMismatch),
        "got {error:?}"
    );
}

#[test]
fn a_signature_minted_for_another_domain_does_not_verify_as_a_manifest() {
    // Domain separation is load-bearing: nothing may verify across domains.
    // A signature over the SAME body digest but under a release-STATE domain
    // must not verify as a manifest signature.
    use base64::Engine as _;
    use ed25519_dalek::Signer as _;

    let signer = Signer::fresh("release-acceptance-key");
    let manifest = ReleaseManifestV1::unsigned(body("0.13.0", 11, NOW - 60)).unwrap();

    // The pristine control: the manifest domain verifies.
    let control =
        serde_json::to_vec_pretty(&manifest.clone().sign(&signer.key_id, &signer.key)).unwrap();
    signer
        .verifier()
        .verify_manifest_json(&control, NOW)
        .expect("CONTROL: the manifest domain must verify");

    // Same key, same body digest, a DIFFERENT domain separator.
    let mut message = ReleaseState::ReleaseAcceptance.signature_domain().to_vec();
    message.extend_from_slice(manifest.body_sha256.as_bytes());
    let foreign = signer.key.sign(&message);

    let mut cross = manifest;
    cross.authority.key_id = signer.key_id.clone();
    cross.authority.signature_base64 =
        base64::engine::general_purpose::STANDARD.encode(foreign.to_bytes());
    let bytes = serde_json::to_vec_pretty(&cross).unwrap();

    let error = signer
        .verifier()
        .verify_manifest_json(&bytes, NOW)
        .expect_err("a cross-domain signature must NOT verify as a manifest");
    assert!(
        matches!(error, UpdateTrustError::InvalidSignature),
        "got {error:?}"
    );
}

#[test]
fn a_manifest_that_does_not_describe_the_offered_release_is_refused() {
    // A correctly signed, current, in-sequence manifest for a DIFFERENT
    // release must not authorise this offer — otherwise one good manifest
    // launders every archive.
    let signer = Signer::fresh("release-acceptance-key");
    pristine_control(&signer, "0.12.25", "0.13.0");

    let bytes = signer.mint(body("0.14.0", 11, NOW - 60));
    let verified = signer.verifier().verify_manifest_json(&bytes, NOW).unwrap();
    let state = first_run();
    let decision = decide_update(&offer("0.12.25", "0.13.0", Some(&verified), &state));
    assert!(
        matches!(
            decision,
            UpdateDecision::RefusedManifestDoesNotDescribeOffer { .. }
        ),
        "got {decision:?}"
    );
}

#[test]
fn a_newer_offer_with_no_manifest_at_all_is_refused_fail_closed() {
    // The install path requires a signed manifest. This is a DELIBERATE
    // fail-closed posture: until release manifests are published and the
    // bundled trust root holds Sean's real keys, `self-update` refuses to
    // install and directs the user to the provenance-backed npm route. The
    // refusal must say so.
    let state = first_run();
    let decision = decide_update(&offer("0.12.25", "0.13.0", None, &state));
    assert!(
        matches!(decision, UpdateDecision::RefusedMissingManifest { .. }),
        "got {decision:?}"
    );
    let message = decision.message();
    assert!(
        message.contains("npm"),
        "the refusal must name the working alternative: {message}"
    );

    // Control: the SAME offer with a manifest proceeds, so this is not a
    // verifier that refuses everything.
    let signer = Signer::fresh("release-acceptance-key");
    pristine_control(&signer, "0.12.25", "0.13.0");
}

#[test]
fn the_downloaded_archive_must_match_the_digest_the_manifest_signed() {
    // Without this the manifest's artifact digests are decorative: a correctly
    // signed manifest would sit beside whatever archive the source handed over.
    let signer = Signer::fresh("release-acceptance-key");
    let bytes = signer.mint(body("0.13.0", 11, NOW - 60));
    let verified = signer.verifier().verify_manifest_json(&bytes, NOW).unwrap();
    let name = "wayland-core-v0.13.0-x86_64-unknown-linux-gnu.tar.gz";

    // Pristine control: the digest and length the manifest names are accepted.
    verified
        .check_archive(name, &hexed('b', 64), 4096)
        .expect("CONTROL: the artifact the manifest names must be accepted");

    // A different digest at the right length.
    assert!(matches!(
        verified.check_archive(name, &hexed('c', 64), 4096),
        Err(UpdateTrustError::ArtifactDigestMismatch { .. })
    ));
    // The right digest at a different length.
    assert!(matches!(
        verified.check_archive(name, &hexed('b', 64), 4097),
        Err(UpdateTrustError::ArtifactDigestMismatch { .. })
    ));
    // An archive the manifest never names at all.
    assert!(matches!(
        verified.check_archive("something-else.tar.gz", &hexed('b', 64), 4096),
        Err(UpdateTrustError::ArtifactNotInManifest(_))
    ));
}

#[test]
fn the_release_manifest_role_is_pinned_to_release_acceptance() {
    // A manifest signed by a key bound to any EARLIER release state must not
    // authorise a shipped install: reaching packaging is not reaching
    // acceptance.
    let key = SigningKey::generate(&mut OsRng);
    assert_eq!(RELEASE_MANIFEST_ROLE, "release_acceptance");

    let root = ReleaseTrustRootV1::new(vec![TrustedKeyV1 {
        key_id: "packaging-key".to_string(),
        public_key_base64: {
            use base64::Engine as _;
            base64::engine::general_purpose::STANDARD.encode(key.verifying_key().to_bytes())
        },
        role: ReleaseState::Packaging,
        valid_from: 0,
        retired_at: None,
    }]);
    let verifier =
        ReleaseVerifier::with_trust_root_json(&serde_json::to_vec(&root).unwrap()).unwrap();
    let signed = ReleaseManifestV1::unsigned(body("0.13.0", 11, NOW - 60))
        .unwrap()
        .sign("packaging-key", &key);
    let bytes = serde_json::to_vec_pretty(&signed).unwrap();
    let error = verifier
        .verify_manifest_json(&bytes, NOW)
        .expect_err("a packaging key must not authorise an install");
    assert!(
        matches!(error, UpdateTrustError::RoleMismatch { .. }),
        "got {error:?}"
    );
}
