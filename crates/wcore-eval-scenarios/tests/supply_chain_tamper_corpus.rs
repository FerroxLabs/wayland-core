//! The paired supply-chain tamper corpus (F29-03).
//!
//! **Tamper rejection is proved by tampering, never by signing and verifying.**
//! A test that mints a manifest and then verifies it proves the happy path and
//! says nothing whatsoever about refusal. Every claim here is a PAIR: the
//! pristine subject is verified and ACCEPTED, then exactly one field or one
//! byte is mutated and the same verification is run again and REFUSED. Both
//! halves are recorded.
//!
//! The pairing is enforced by the data structure rather than by reviewer
//! discipline. [`TamperCase::new`] builds the pristine subject itself and
//! [`Paired::new`] takes that pristine subject as a mandatory argument, so a
//! case carrying only a mutation cannot be expressed at all. `Paired::new`
//! additionally refuses a mutation that changes nothing, because a no-op
//! "mutation" would make the pair vacuous in the other direction.
//!
//! Why that matters concretely: a corpus that only ever asserts refusals is
//! passed trivially by a verifier that refuses everything. That is the
//! supply-chain shape of a gate that cannot go red, and it is the specific
//! failure this file is built to make impossible.
//!
//! **No case asserts a rendered error string.** Each asserts the INVARIANT that
//! the input was refused. Pinning a message encodes today's failure shape and
//! keeps passing for the wrong reason when the refusal moves to a different —
//! and possibly weaker — cause. The refusal POINT is recorded for the reader,
//! never asserted.
//!
//! Every case drives the same verifiers the release path uses
//! ([`verify_manifest`], [`sbom::sbom_sha256`], the receipt body digest via
//! [`EvidenceReceiptV1::local`]), so a pass here is a statement about the
//! product and not about a test double.
//!
//! Scope, stated rather than implied: the update class is covered at the
//! release-manifest layer, because `wcore-eval-scenarios` has no dependency
//! edge on `wcore-cli` and adding one would be a `Cargo.toml` change outside
//! this plan. The shipped updater's own verification path was driven live by
//! 29-03 against the real binary; see `29-04-TAMPER-RESULTS.md`.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::OnceLock;

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD as BASE64;
use ed25519_dalek::{Signer, SigningKey};
use sha2::{Digest, Sha256};

use wcore_eval_scenarios::receipt::{Evidence, EvidenceReceiptV1, ReceiptBodyV1};
use wcore_eval_scenarios::release_integrity::{
    ArtifactKind, CertificationBindingV1, DependencyPolicyOutcomeV1, PackagedArtifactV1,
    PolicyResult, ReleaseManifestBodyV1, ReleaseManifestV1, ReleaseRevocationV1, ReleaseState,
    ReleaseTrustRootV1, ReproducibilityVerdictV1, RevocationKind, SbomFormat, SbomReferenceV1,
    TrustedKeyV1, VarianceClass, verify_manifest,
};
use wcore_eval_scenarios::sbom;

/// The source commit the packaged eval fixture reports. Not a real release
/// commit; the clean room owns every value in this file.
const FIXTURE_COMMIT: &str = "0123456789abcdef0123456789abcdef01234567";

/// Evaluation instant. Every key in the clean-room trust root is valid here.
const NOW: u64 = 1_800_000_100;

const ARCHIVE_NAME: &str = "wayland-core-x86_64-unknown-linux-gnu.tar.gz";
const SBOM_NAME: &str = "wayland-core-sbom.cdx.json";
const PLUGIN_NAME: &str = "wayland-plugin-ollama-0.12.25.tar.gz";
/// The exact bytes the certification binding's `receipt_body_sha256` covers.
const RECEIPT_BODY_ENTRY: &str = "receipt-body.json";

/// The role that signs a release manifest. `wcore-cli`'s shipped updater pins
/// `release_acceptance` as `RELEASE_MANIFEST_ROLE`, so the corpus verifies
/// under the same role the product does.
const MANIFEST_ROLE: ReleaseState = ReleaseState::ReleaseAcceptance;

// ---------------------------------------------------------------------------
// Outcomes
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Verdict {
    Accepted,
    Refused,
}

/// Where verification stopped. Recorded for the reader so distinct cases can be
/// seen to refuse for distinct reasons. **Never asserted by a case** — the case
/// asserts only that the subject was refused.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RefusalPoint {
    /// The real `verify_manifest`: schema, body digest, key id, role, validity
    /// window, signature.
    SignedManifest,
    /// The SBOM document's digest no longer equals the digest the verified
    /// manifest signed.
    SbomDigest,
    /// The certification binding no longer joins to the receipt it names.
    CertificationJoin,
    /// A packaged artifact's bytes no longer digest to what the manifest signed.
    ArtifactBinding,
}

/// Collapse a verification result into the only thing a case is allowed to
/// assert. The refusal POINT is carried alongside for the record.
fn verdict_of(result: &Result<(), RefusalPoint>) -> Verdict {
    match result {
        Ok(()) => Verdict::Accepted,
        Err(_) => Verdict::Refused,
    }
}

impl RefusalPoint {
    fn as_str(self) -> &'static str {
        match self {
            Self::SignedManifest => "signed_manifest",
            Self::SbomDigest => "sbom_digest",
            Self::CertificationJoin => "certification_join",
            Self::ArtifactBinding => "artifact_binding",
        }
    }
}

// ---------------------------------------------------------------------------
// The seven object classes F29-03 names
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum ObjectClass {
    Binary,
    Sbom,
    Update,
    Plugin,
    BackendReceipt,
    Manifest,
    Key,
}

impl ObjectClass {
    const ALL: [ObjectClass; 7] = [
        ObjectClass::Binary,
        ObjectClass::Sbom,
        ObjectClass::Update,
        ObjectClass::Plugin,
        ObjectClass::BackendReceipt,
        ObjectClass::Manifest,
        ObjectClass::Key,
    ];

    fn as_id(self) -> &'static str {
        match self {
            Self::Binary => "BINARY",
            Self::Sbom => "SBOM",
            Self::Update => "UPDATE",
            Self::Plugin => "PLUGIN",
            Self::BackendReceipt => "BACKEND-RECEIPT",
            Self::Manifest => "MANIFEST",
            Self::Key => "KEY",
        }
    }
}

// ---------------------------------------------------------------------------
// The subject: a signed release manifest plus the object store it binds
// ---------------------------------------------------------------------------

/// Everything one verification consumes. The trust root is a FIELD rather than
/// a constant because authority must arrive independently of the document, and
/// because a key-class mutation legitimately targets the root and not the
/// manifest.
#[derive(Debug, Clone)]
struct Subject {
    manifest: ReleaseManifestV1,
    /// name -> bytes: every packaged artifact, the SBOM document, and the
    /// receipt body the certification binding names.
    store: BTreeMap<String, Vec<u8>>,
    trust_root: ReleaseTrustRootV1,
    now: u64,
}

impl Subject {
    /// A cheap identity for this subject. Used only to prove a mutation
    /// actually changed something; deliberately not a full serialization,
    /// because the store holds megabytes of real binary.
    fn fingerprint(&self) -> String {
        let mut hasher = Sha256::new();
        hasher.update(serde_json::to_vec(&self.manifest).expect("manifest serializes"));
        hasher.update(serde_json::to_vec(&self.trust_root).expect("trust root serializes"));
        hasher.update(self.now.to_le_bytes());
        for (name, bytes) in &self.store {
            hasher.update(name.as_bytes());
            hasher.update(Sha256::digest(bytes));
        }
        format!("{:x}", hasher.finalize())
    }
}

// ---------------------------------------------------------------------------
// The verifier every case drives
// ---------------------------------------------------------------------------

fn sha256_hex(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

/// Verify a release the way the release path does: the signed manifest against
/// an independently supplied trust root FIRST, then the objects that manifest
/// binds by digest.
///
/// Ordering is load-bearing for the record, not for correctness: the SBOM and
/// certification joins are checked before the general artifact binding so a
/// document-specific mutation is reported at its own point rather than being
/// absorbed by the artifact loop.
fn verify_release(subject: &Subject) -> Result<(), RefusalPoint> {
    verify_manifest(
        &subject.manifest,
        &subject.trust_root,
        MANIFEST_ROLE,
        subject.now,
    )
    .map_err(|_| RefusalPoint::SignedManifest)?;

    let body = &subject.manifest.body;

    if let Evidence::Observed { value } = &body.sbom {
        let bytes = subject
            .store
            .get(&value.name)
            .ok_or(RefusalPoint::SbomDigest)?;
        let document = std::str::from_utf8(bytes).map_err(|_| RefusalPoint::SbomDigest)?;
        if sbom::sbom_sha256(document) != value.sha256 {
            return Err(RefusalPoint::SbomDigest);
        }
    }

    if let Evidence::Observed { value } = &body.certification {
        let bytes = subject
            .store
            .get(RECEIPT_BODY_ENTRY)
            .ok_or(RefusalPoint::CertificationJoin)?;
        let receipt_body: ReceiptBodyV1 =
            serde_json::from_slice(bytes).map_err(|_| RefusalPoint::CertificationJoin)?;
        // Recompute through the receipt type's own digest, so the join is the
        // product's computation and not a re-implementation of it.
        let recomputed =
            EvidenceReceiptV1::local(receipt_body).map_err(|_| RefusalPoint::CertificationJoin)?;
        if recomputed.body_sha256 != value.receipt_body_sha256
            || recomputed.body.identity.source_commit != value.source_commit
            || recomputed.body.identity.binary_sha256 != value.binary_sha256
            || recomputed.body.target.os != value.target_os
            || recomputed.body.target.architecture != value.target_architecture
        {
            return Err(RefusalPoint::CertificationJoin);
        }
    }

    for artifact in &body.artifacts {
        let bytes = subject
            .store
            .get(&artifact.name)
            .ok_or(RefusalPoint::ArtifactBinding)?;
        if sha256_hex(bytes) != artifact.sha256 || bytes.len() as u64 != artifact.byte_length {
            return Err(RefusalPoint::ArtifactBinding);
        }
    }

    Ok(())
}

// ---------------------------------------------------------------------------
// The paired case — a case without its pristine control cannot be constructed
// ---------------------------------------------------------------------------

/// A pristine subject and exactly one mutation of it. Both are mandatory
/// fields, and the only constructor takes the pristine subject as its first
/// argument.
#[derive(Debug)]
struct Paired {
    pristine: Subject,
    mutated: Subject,
}

impl Paired {
    fn new(pristine: Subject, mutate: impl FnOnce(&mut Subject)) -> Self {
        let mut mutated = pristine.clone();
        mutate(&mut mutated);
        assert_ne!(
            pristine.fingerprint(),
            mutated.fingerprint(),
            "a mutation that changes nothing is not a mutation, and a pair built from one \
             would assert a refusal of the pristine subject itself"
        );
        Self { pristine, mutated }
    }
}

#[derive(Debug)]
struct TamperCase {
    id: &'static str,
    class: ObjectClass,
    what_was_mutated: &'static str,
    paired: Paired,
}

impl TamperCase {
    /// The only constructor. It builds the pristine subject ITSELF, so no
    /// caller can supply a case that has only a mutation.
    fn new(
        id: &'static str,
        class: ObjectClass,
        what_was_mutated: &'static str,
        mutate: impl FnOnce(&mut Subject),
    ) -> Self {
        Self {
            id,
            class,
            what_was_mutated,
            paired: Paired::new(pristine_subject(), mutate),
        }
    }
}

// ---------------------------------------------------------------------------
// Clean-room material
// ---------------------------------------------------------------------------

/// One throwaway signing key per release state. Deterministic so the corpus is
/// reproducible; these are test keys and nothing here is a credential.
fn role_key(role: ReleaseState) -> SigningKey {
    SigningKey::from_bytes(&[role.ordinal() + 1; 32])
}

fn role_key_id(role: ReleaseState) -> String {
    format!("{}-key", role.as_str().replace('_', "-"))
}

fn clean_room_trust_root() -> ReleaseTrustRootV1 {
    let keys = [
        ReleaseState::Packaging,
        ReleaseState::DeploymentPreparation,
        ReleaseState::RollbackRehearsal,
        ReleaseState::ReleaseAcceptance,
    ]
    .into_iter()
    .map(|role| TrustedKeyV1 {
        key_id: role_key_id(role),
        public_key_base64: BASE64.encode(role_key(role).verifying_key().to_bytes()),
        role,
        valid_from: 0,
        retired_at: None,
    })
    .collect();
    ReleaseTrustRootV1::new(keys)
}

/// The packaged release archive: real bytes of the compiled
/// `wcore-eval-fixture` binary this workspace builds, so the binary class
/// mutates a byte of actual compiled code rather than of a synthetic blob.
///
/// Bounded to one mebibyte on purpose. The corpus holds two subjects per case
/// and each subject owns its own copy of the store; an unbounded debug binary
/// would make the table hold gigabytes and would slow every fingerprint without
/// making a single mutated byte any more real.
fn packaged_archive() -> &'static [u8] {
    static BYTES: OnceLock<Vec<u8>> = OnceLock::new();
    BYTES.get_or_init(|| {
        let path = env!("CARGO_BIN_EXE_wcore-eval-fixture");
        let mut bytes =
            std::fs::read(path).unwrap_or_else(|error| panic!("could not read {path}: {error}"));
        bytes.truncate(1024 * 1024);
        bytes
    })
}

fn f29_fixture(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("CARGO_MANIFEST_DIR is crates/wcore-eval-scenarios, so it has a parent")
        .join("wcore-fixture-harness")
        .join("fixtures")
        .join("f29")
        .join(name)
}

/// A real CycloneDX SBOM produced by the product's own generator from the
/// pinned cargo-metadata fixture.
fn sbom_document() -> &'static str {
    static DOCUMENT: OnceLock<String> = OnceLock::new();
    DOCUMENT.get_or_init(|| {
        let path = f29_fixture("cargo-metadata.json");
        let metadata = std::fs::read_to_string(&path)
            .unwrap_or_else(|error| panic!("could not read {}: {error}", path.display()));
        sbom::cyclonedx_from_cargo_metadata(&metadata).expect("the pinned fixture must transform")
    })
}

/// A receipt the REAL evaluation pipeline produced, in this process, from the
/// packaged fixture binary. The backend-receipt class binds and mutates this
/// rather than a hand-written struct, so the certification join is exercised
/// against an object the product actually emits.
fn real_receipt() -> &'static EvidenceReceiptV1 {
    static RECEIPT: OnceLock<EvidenceReceiptV1> = OnceLock::new();
    RECEIPT.get_or_init(|| {
        let temp = tempfile::tempdir().expect("temp report root");
        let report_root = temp.path().join("reports");
        let mut command = Command::new(env!("CARGO_BIN_EXE_wayland-eval"));
        command.args([
            "--scenario",
            "canary",
            "--provider",
            "deepseek",
            "--binary",
            env!("CARGO_BIN_EXE_wcore-eval-fixture"),
            "--expected-source-commit",
            FIXTURE_COMMIT,
            "--report-dir",
        ]);
        command.arg(&report_root);
        command.env("DEEPSEEK_API_KEY", "fixture-key");
        command.env_remove("WCORE_EVAL_BIN");
        command.env_remove("WCORE_EVAL_PROVIDER");
        command.env_remove("ANTHROPIC_API_KEY");
        command.env_remove("OPENAI_API_KEY");
        let output = command.output().expect("run the fixture evaluation");
        assert!(
            output.status.success(),
            "the fixture evaluation must succeed; status {:?}\nstdout:\n{}\nstderr:\n{}",
            output.status.code(),
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        let cells: Vec<_> = std::fs::read_dir(&report_root)
            .expect("report root")
            .filter_map(Result::ok)
            .map(|entry| entry.path())
            .collect();
        assert_eq!(cells.len(), 1, "exactly one evaluated cell");
        let bytes = std::fs::read(cells[0].join("receipt.json")).expect("receipt JSON");
        let receipt: EvidenceReceiptV1 = serde_json::from_slice(&bytes).expect("versioned receipt");
        // Sign it under a clean-room CI key so the binding's
        // `receipt_signing_key_id` names a key that really did sign it.
        receipt.sign_ci("clean-room-ci-key", &SigningKey::from_bytes(&[9u8; 32]))
    })
}

fn certification_binding(receipt: &EvidenceReceiptV1) -> CertificationBindingV1 {
    CertificationBindingV1 {
        receipt_body_sha256: receipt.body_sha256.clone(),
        receipt_schema: receipt.schema.clone(),
        receipt_schema_version: receipt.schema_version,
        receipt_signing_key_id: "clean-room-ci-key".to_string(),
        source_commit: receipt.body.identity.source_commit.clone(),
        binary_sha256: receipt.body.identity.binary_sha256.clone(),
        target_os: receipt.body.target.os.clone(),
        target_architecture: receipt.body.target.architecture.clone(),
    }
}

/// The pristine release: a signed manifest and the objects it binds, all
/// internally consistent. Every case starts here.
fn pristine_subject() -> Subject {
    let archive = packaged_archive().to_vec();
    let sbom_bytes = sbom_document().as_bytes().to_vec();
    let plugin = b"clean-room plugin payload for the marketplace artifact class\n".repeat(64);
    let receipt = real_receipt();
    let receipt_body = serde_json::to_vec(&receipt.body).expect("receipt body serializes");

    let store: BTreeMap<String, Vec<u8>> = [
        (ARCHIVE_NAME.to_string(), archive),
        (SBOM_NAME.to_string(), sbom_bytes),
        (PLUGIN_NAME.to_string(), plugin),
        (RECEIPT_BODY_ENTRY.to_string(), receipt_body),
    ]
    .into_iter()
    .collect();

    let artifacts: Vec<PackagedArtifactV1> = [
        (ARCHIVE_NAME, ArtifactKind::Archive),
        (SBOM_NAME, ArtifactKind::Sbom),
        (PLUGIN_NAME, ArtifactKind::Archive),
    ]
    .into_iter()
    .map(|(name, kind)| {
        let bytes = &store[name];
        PackagedArtifactV1 {
            name: name.to_string(),
            sha256: sha256_hex(bytes),
            byte_length: bytes.len() as u64,
            kind,
        }
    })
    .collect();

    let body = ReleaseManifestBodyV1 {
        release_id: "v0.12.25-wayland-core-clean-room".to_string(),
        source_commit: FIXTURE_COMMIT.to_string(),
        artifacts,
        sbom: Evidence::Observed {
            value: SbomReferenceV1 {
                name: SBOM_NAME.to_string(),
                sha256: sbom::sbom_sha256(sbom_document()),
                format: SbomFormat::CycloneDxJson,
            },
        },
        dependency_policy: Evidence::Observed {
            value: DependencyPolicyOutcomeV1 {
                tool: "cargo-deny".to_string(),
                policy_sha256: sha256_hex(b"clean-room deny.toml"),
                result: PolicyResult::Pass,
            },
        },
        // 29-02 measured the real release as DOCUMENTED-VARIANCE, class
        // path_prefix. The clean-room manifest carries the same verdict shape
        // rather than a prettier one.
        reproducibility: ReproducibilityVerdictV1::Variance {
            class: VarianceClass::PathPrefix,
            evidence_sha256: sha256_hex(b"clean-room reproducibility measurement"),
        },
        certification: Evidence::Observed {
            value: certification_binding(receipt),
        },
        sequence: 7,
        issued_at: 1_800_000_000,
        revocations: vec![ReleaseRevocationV1 {
            kind: RevocationKind::Version,
            value: "0.12.24".to_string(),
            reason: "superseded by the clean-room release under test".to_string(),
        }],
    };

    let manifest = ReleaseManifestV1::unsigned(body)
        .expect("the pristine manifest body must be well formed")
        .sign(role_key_id(MANIFEST_ROLE), &role_key(MANIFEST_ROLE));

    Subject {
        manifest,
        store,
        trust_root: clean_room_trust_root(),
        now: NOW,
    }
}

// ---------------------------------------------------------------------------
// Mutation helpers — each changes exactly one thing
// ---------------------------------------------------------------------------

/// Change one lowercase-hex character so the value stays well formed and only
/// its VALUE differs. A malformed digest would be refused by shape rather than
/// by binding, which is a different and weaker claim.
fn flip_last_hex(value: &mut String) {
    let replacement = if value.ends_with('0') { '1' } else { '0' };
    value.pop();
    value.push(replacement);
}

fn flip_first_byte(subject: &mut Subject, name: &str) {
    let bytes = subject
        .store
        .get_mut(name)
        .unwrap_or_else(|| panic!("{name} must be in the pristine store"));
    assert!(!bytes.is_empty(), "{name} must not be empty");
    bytes[0] ^= 0x01;
}

fn artifact_mut<'a>(subject: &'a mut Subject, name: &str) -> &'a mut PackagedArtifactV1 {
    subject
        .manifest
        .body
        .artifacts
        .iter_mut()
        .find(|artifact| artifact.name == name)
        .unwrap_or_else(|| panic!("{name} must be a packaged artifact"))
}

fn certification_mut(subject: &mut Subject) -> &mut CertificationBindingV1 {
    match &mut subject.manifest.body.certification {
        Evidence::Observed { value } => value,
        Evidence::Unavailable { .. } => {
            panic!("the pristine subject carries an observed certification binding")
        }
    }
}

// ---------------------------------------------------------------------------
// The corpus
// ---------------------------------------------------------------------------

fn corpus() -> Vec<TamperCase> {
    vec![
        // --- binaries -------------------------------------------------------
        TamperCase::new(
            "F29-03-BINARY-1",
            ObjectClass::Binary,
            "one byte of the packaged release archive, so its bytes no longer digest to the \
             value the signed manifest recorded",
            |subject| flip_first_byte(subject, ARCHIVE_NAME),
        ),
        // --- SBOMs ----------------------------------------------------------
        TamperCase::new(
            "F29-03-SBOM-1",
            ObjectClass::Sbom,
            "one byte of the CycloneDX SBOM document, so its digest no longer equals the \
             digest the signed manifest bound",
            |subject| flip_first_byte(subject, SBOM_NAME),
        ),
        // --- updates --------------------------------------------------------
        TamperCase::new(
            "F29-03-UPDATE-1",
            ObjectClass::Update,
            "the manifest sequence lowered by one — the field the updater's rollback and \
             freeze protection reads — without re-signing",
            |subject| subject.manifest.body.sequence -= 1,
        ),
        TamperCase::new(
            "F29-03-UPDATE-2",
            ObjectClass::Update,
            "the manifest issued_at moved 90 days into the past — the staleness field the \
             updater reads on a first run — without re-signing",
            |subject| subject.manifest.body.issued_at -= 90 * 24 * 60 * 60,
        ),
        // --- plugins --------------------------------------------------------
        TamperCase::new(
            "F29-03-PLUGIN-1",
            ObjectClass::Plugin,
            "the plugin artifact entry's recorded sha256 inside the signed release manifest",
            |subject| flip_last_hex(&mut artifact_mut(subject, PLUGIN_NAME).sha256),
        ),
        // --- backend receipts ----------------------------------------------
        TamperCase::new(
            "F29-03-BACKEND-RECEIPT-1",
            ObjectClass::BackendReceipt,
            "the certification binding's receipt_body_sha256 inside the signed manifest",
            |subject| flip_last_hex(&mut certification_mut(subject).receipt_body_sha256),
        ),
        TamperCase::new(
            "F29-03-BACKEND-RECEIPT-2",
            ObjectClass::BackendReceipt,
            "the bound receipt body itself — its identity.binary_sha256 — so the receipt no \
             longer digests to what the untouched manifest signed",
            |subject| {
                let bytes = &subject.store[RECEIPT_BODY_ENTRY];
                let mut body: ReceiptBodyV1 =
                    serde_json::from_slice(bytes).expect("the pristine receipt body parses");
                flip_last_hex(&mut body.identity.binary_sha256);
                subject.store.insert(
                    RECEIPT_BODY_ENTRY.to_string(),
                    serde_json::to_vec(&body).expect("mutated receipt body serializes"),
                );
            },
        ),
        // --- manifests ------------------------------------------------------
        TamperCase::new(
            "F29-03-MANIFEST-1",
            ObjectClass::Manifest,
            "a manifest body field (release_id) changed without re-signing, so the body digest \
             and the signature disagree",
            |subject| subject.manifest.body.release_id.push('x'),
        ),
        TamperCase::new(
            "F29-03-MANIFEST-2",
            ObjectClass::Manifest,
            "the manifest schema_version raised to a version this verifier does not know",
            |subject| subject.manifest.schema_version += 1,
        ),
        // --- keys: three distinct attacks -----------------------------------
        TamperCase::new(
            "F29-03-KEY-1",
            ObjectClass::Key,
            "the authority key id replaced with an id the independently supplied trust root \
             does not know",
            |subject| {
                subject.manifest.authority.key_id =
                    "an-id-the-trust-root-has-never-seen".to_string();
            },
        ),
        TamperCase::new(
            "F29-03-KEY-2",
            ObjectClass::Key,
            "the trust root retires the signing key at an instant before the evaluation \
             instant; the signature stays cryptographically valid",
            |subject| {
                subject.trust_root = subject
                    .trust_root
                    .with_key_retired(&role_key_id(MANIFEST_ROLE), NOW - 1)
                    .expect("the manifest role key is in the clean-room trust root");
            },
        ),
        TamperCase::new(
            "F29-03-KEY-3",
            ObjectClass::Key,
            "the manifest signature replaced with one the SAME key minted over the SAME body \
             digest under the release-state signature domain — a cross-domain replay",
            |subject| {
                let mut message = MANIFEST_ROLE.signature_domain().to_vec();
                message.extend_from_slice(subject.manifest.body_sha256.as_bytes());
                let signature = role_key(MANIFEST_ROLE).sign(&message);
                subject.manifest.authority.signature_base64 = BASE64.encode(signature.to_bytes());
            },
        ),
    ]
}

// ---------------------------------------------------------------------------
// The runner
// ---------------------------------------------------------------------------

/// Verify the pristine subject and assert ACCEPTANCE, then verify the mutated
/// subject and assert REFUSAL — in that order, for every case, with both halves
/// printed.
#[test]
fn every_tamper_case_is_accepted_pristine_then_refused_after_one_mutation() {
    let cases = corpus();
    assert!(
        cases.len() >= 10,
        "the corpus floor is ten cases: seven object classes with three attacks in the key class"
    );

    for case in &cases {
        let control = verify_release(&case.paired.pristine);
        assert_eq!(
            verdict_of(&control),
            Verdict::Accepted,
            "{}: the PRISTINE control must be ACCEPTED. A corpus whose controls are refused is \
             passed trivially by a verifier that refuses everything, which is the whole hazard \
             this file exists to remove. Re-examine the fixture, never the verifier.",
            case.id
        );

        let mutated = verify_release(&case.paired.mutated);
        assert_eq!(
            verdict_of(&mutated),
            Verdict::Refused,
            "{}: the single mutation ({}) must be REFUSED",
            case.id,
            case.what_was_mutated
        );
        let point = mutated.err().map_or("none", RefusalPoint::as_str);

        println!(
            "TAMPER-CASE {}::control=ACCEPTED::mutated=REFUSED::class={}::point={}::mutation={}",
            case.id,
            case.class.as_id(),
            point,
            case.what_was_mutated
        );
    }
    println!("TAMPER-CORPUS cases={} all_paired=true", cases.len());
}

/// The meta-assertion that makes the corpus non-trivial: EVERY case has a
/// pristine control and every one of those controls is accepted.
#[test]
fn every_tamper_case_has_a_pristine_control_that_is_accepted() {
    let cases = corpus();
    assert!(!cases.is_empty(), "an empty corpus proves nothing");
    let mut accepted = 0usize;
    for case in &cases {
        assert!(
            verify_release(&case.paired.pristine).is_ok(),
            "{}: its pristine control must be accepted",
            case.id
        );
        accepted += 1;
    }
    assert_eq!(
        accepted,
        cases.len(),
        "every case must contribute exactly one accepted control"
    );
    println!(
        "TAMPER-META controls_accepted={accepted} of {}",
        cases.len()
    );
}

/// The meta-assertion that stops an object class quietly disappearing when
/// somebody edits the table.
#[test]
fn the_corpus_covers_every_f29_03_object_class() {
    let cases = corpus();
    let covered: BTreeSet<ObjectClass> = cases.iter().map(|case| case.class).collect();
    let expected: BTreeSet<ObjectClass> = ObjectClass::ALL.into_iter().collect();
    assert_eq!(
        covered, expected,
        "F29-03 names seven object classes and all seven must be represented"
    );

    let key_cases = cases
        .iter()
        .filter(|case| case.class == ObjectClass::Key)
        .count();
    assert!(
        key_cases >= 3,
        "the key class needs three distinct attacks — unknown id, retired key, cross-domain \
         replay — but found {key_cases}"
    );

    for case in &cases {
        let prefix = format!("F29-03-{}-", case.class.as_id());
        assert!(
            case.id.starts_with(&prefix),
            "{} must be identified as {prefix}<n> so the ledger can index it",
            case.id
        );
    }
    println!(
        "TAMPER-META classes_covered={} key_class_attacks={key_cases}",
        covered.len()
    );
}
