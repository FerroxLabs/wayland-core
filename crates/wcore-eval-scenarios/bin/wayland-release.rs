//! Release manifest and release-state ledger tool.
//!
//! Follows `wayland-receipt`'s shape exactly, including the one rule that
//! matters most: **a signing seed is never an argument and never reaches
//! standard output.** Seeds are written only to files created with owner-only
//! permissions, are read back only from standard input, and are wiped on drop.
//!
//! There is deliberately no subcommand that prints a seed and no subcommand
//! that accepts one as an argument. `trust-root init` prints key ids and PUBLIC
//! keys only.

use std::io::Read as _;
use std::path::{Path, PathBuf};

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD as BASE64;
use clap::{Parser, Subcommand};
use ed25519_dalek::SigningKey;
use rand::RngCore as _;
use sha2::{Digest, Sha256};

use wcore_eval_scenarios::receipt::Evidence;
use wcore_eval_scenarios::release_integrity::{
    ArtifactKind, CANONICAL_RELEASE_STATES, DependencyPolicyOutcomeV1, PackagedArtifactV1,
    PolicyResult, ReleaseManifestBodyV1, ReleaseManifestV1, ReleaseState, ReleaseTrustRootV1,
    ReproducibilityVerdictV1, SbomFormat, SbomReferenceV1, TrustedKeyV1, VarianceClass,
    signing_key_from_seed_base64, verify_manifest, wipe,
};
use wcore_eval_scenarios::release_states::{
    ReleaseStateRecordV1, StateEvidenceV1, verify_state_chain,
};

#[derive(Debug, Parser)]
#[command(name = "wayland-release")]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// Generate one throwaway keypair per release state into a directory.
    /// Prints key ids and PUBLIC keys only — never a seed.
    TrustRootInit {
        #[arg(long)]
        directory: PathBuf,
        #[arg(long, default_value = "0")]
        valid_from: u64,
    },
    /// Transform `cargo metadata --locked --format-version 1` output into a
    /// byte-deterministic CycloneDX SBOM. Pure: no clock, no randomness, no
    /// environment read, so two runs over the same metadata file produce
    /// identical bytes and therefore an identical digest.
    Sbom {
        #[arg(long)]
        metadata: PathBuf,
        #[arg(long)]
        output: PathBuf,
    },
    /// Build an unsigned manifest over a directory of artifacts.
    ///
    /// The three clean-room results are OPTIONAL arguments, and each is
    /// modelled as `Evidence` so its absence is explicit rather than an empty
    /// success: omitting `--sbom` records "no SBOM was produced", never "the
    /// SBOM was fine".
    ManifestBuild {
        #[arg(long)]
        artifacts: PathBuf,
        #[arg(long)]
        output: PathBuf,
        #[arg(long)]
        release_id: String,
        #[arg(long)]
        source_commit: String,
        /// CycloneDX SBOM to bind by digest.
        #[arg(long)]
        sbom: Option<PathBuf>,
        /// Dependency-policy verdict: `pass` or `fail`.
        #[arg(long, value_parser = parse_policy_result)]
        dependency_policy: Option<PolicyResult>,
        /// The policy configuration the verdict was produced against
        /// (deny.toml). Digested into the manifest, because a pass against an
        /// empty policy is not a pass.
        #[arg(long)]
        dependency_policy_config: Option<PathBuf>,
        /// Reproducibility verdict: `reproduced` or `variance`.
        #[arg(long, value_parser = parse_reproducibility)]
        reproducibility: Option<ReproducibilityKind>,
        /// Required with `--reproducibility variance`.
        #[arg(long, value_parser = parse_variance_class)]
        variance_class: Option<VarianceClass>,
        /// The measurement that identified the variance. Digested.
        #[arg(long)]
        variance_evidence: Option<PathBuf>,
    },
    /// Sign a manifest. The base64 32-byte Ed25519 seed is read from stdin.
    ManifestSign {
        #[arg(long)]
        manifest: PathBuf,
        #[arg(long)]
        output: PathBuf,
        #[arg(long)]
        key_id: String,
    },
    /// Verify a manifest against an independently supplied trust root.
    ManifestVerify {
        #[arg(long)]
        manifest: PathBuf,
        #[arg(long)]
        trust_root: PathBuf,
        #[arg(long, value_parser = parse_state)]
        role: ReleaseState,
        #[arg(long, default_value = "0")]
        now: u64,
    },
    /// Append a signed state record to a chain. Seed is read from stdin.
    StateAppend {
        #[arg(long)]
        manifest: PathBuf,
        #[arg(long)]
        chain: PathBuf,
        #[arg(long, value_parser = parse_state)]
        state: ReleaseState,
        #[arg(long)]
        key_id: String,
        /// Repeatable `name=<path>`; each named file is digested as evidence.
        #[arg(long = "evidence", required = true)]
        evidence: Vec<String>,
    },
    /// Verify a chain and report the highest contiguously reached state.
    StateVerify {
        #[arg(long)]
        manifest: PathBuf,
        #[arg(long)]
        chain: PathBuf,
        #[arg(long)]
        trust_root: PathBuf,
        #[arg(long, default_value = "0")]
        now: u64,
    },
}

fn parse_state(value: &str) -> Result<ReleaseState, String> {
    serde_json::from_value::<ReleaseState>(serde_json::Value::String(value.to_string()))
        .map_err(|_| format!("unknown release state: {value}"))
}

/// Whether the two clean-room builds agreed. Deliberately NOT defaultable:
/// there is no "unknown" that reads as success.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ReproducibilityKind {
    Reproduced,
    Variance,
}

fn parse_policy_result(value: &str) -> Result<PolicyResult, String> {
    match value {
        "pass" => Ok(PolicyResult::Pass),
        "fail" => Ok(PolicyResult::Fail),
        other => Err(format!(
            "dependency policy result must be pass|fail: {other}"
        )),
    }
}

fn parse_reproducibility(value: &str) -> Result<ReproducibilityKind, String> {
    match value {
        "reproduced" => Ok(ReproducibilityKind::Reproduced),
        "variance" => Ok(ReproducibilityKind::Variance),
        other => Err(format!(
            "reproducibility must be reproduced|variance: {other}"
        )),
    }
}

fn parse_variance_class(value: &str) -> Result<VarianceClass, String> {
    serde_json::from_value::<VarianceClass>(serde_json::Value::String(value.to_string()))
        .map_err(|_| format!("unknown variance class: {value}"))
}

/// Owns a secret for as long as it is needed and wipes it on drop.
struct SecretBytes(Vec<u8>);

impl Drop for SecretBytes {
    fn drop(&mut self) {
        wipe(&mut self.0);
    }
}

fn main() {
    if let Err(error) = execute(Cli::parse()) {
        eprintln!("wayland-release: {error}");
        std::process::exit(1);
    }
}

fn execute(cli: Cli) -> Result<(), String> {
    match cli.command {
        Command::TrustRootInit {
            directory,
            valid_from,
        } => trust_root_init(&directory, valid_from),
        Command::Sbom { metadata, output } => sbom_generate(&metadata, &output),
        Command::ManifestBuild {
            artifacts,
            output,
            release_id,
            source_commit,
            sbom,
            dependency_policy,
            dependency_policy_config,
            reproducibility,
            variance_class,
            variance_evidence,
        } => manifest_build(
            &artifacts,
            &output,
            &release_id,
            &source_commit,
            CleanRoomInputs {
                sbom: sbom.as_deref(),
                dependency_policy,
                dependency_policy_config: dependency_policy_config.as_deref(),
                reproducibility,
                variance_class,
                variance_evidence: variance_evidence.as_deref(),
            },
        ),
        Command::ManifestSign {
            manifest,
            output,
            key_id,
        } => manifest_sign(&manifest, &output, &key_id),
        Command::ManifestVerify {
            manifest,
            trust_root,
            role,
            now,
        } => manifest_verify(&manifest, &trust_root, role, now),
        Command::StateAppend {
            manifest,
            chain,
            state,
            key_id,
            evidence,
        } => state_append(&manifest, &chain, state, &key_id, &evidence),
        Command::StateVerify {
            manifest,
            chain,
            trust_root,
            now,
        } => state_verify(&manifest, &chain, &trust_root, now),
    }
}

fn trust_root_init(directory: &Path, valid_from: u64) -> Result<(), String> {
    std::fs::create_dir_all(directory)
        .map_err(|error| format!("could not create {}: {error}", directory.display()))?;

    let mut keys = Vec::new();
    for state in CANONICAL_RELEASE_STATES {
        let mut seed = SecretBytes(vec![0u8; 32]);
        rand::rngs::OsRng.fill_bytes(&mut seed.0);
        let seed_array = <[u8; 32]>::try_from(seed.0.as_slice())
            .map_err(|_| "generated seed was not 32 bytes".to_string())?;
        let signing_key = SigningKey::from_bytes(&seed_array);
        let key_id = format!("{}-key", state.as_str().replace('_', "-"));

        // The seed goes to a mode-0600 file and NOWHERE else. It is never
        // printed, never logged, and never returned from this function.
        let seed_path = directory.join(format!("{key_id}.seed"));
        write_secret_file(&seed_path, BASE64.encode(seed_array).as_bytes())?;

        keys.push(TrustedKeyV1 {
            key_id: key_id.clone(),
            public_key_base64: BASE64.encode(signing_key.verifying_key().to_bytes()),
            role: state,
            valid_from,
            retired_at: None,
        });
    }

    let trust_root = ReleaseTrustRootV1::new(keys);
    let encoded = serde_json::to_vec_pretty(&trust_root)
        .map_err(|error| format!("could not encode trust root: {error}"))?;
    let trust_root_path = directory.join("trust-root.json");
    std::fs::write(&trust_root_path, &encoded)
        .map_err(|error| format!("could not write trust root: {error}"))?;

    // Public material only.
    println!("TRUST ROOT READY path={}", trust_root_path.display());
    for key in &trust_root.keys {
        println!(
            "KEY key_id={} role={} public_key_base64={}",
            key.key_id,
            key.role.as_str(),
            key.public_key_base64
        );
    }
    Ok(())
}

#[cfg(unix)]
fn write_secret_file(path: &Path, contents: &[u8]) -> Result<(), String> {
    use std::io::Write as _;
    use std::os::unix::fs::OpenOptionsExt as _;
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o600)
        .open(path)
        .map_err(|error| format!("could not create {}: {error}", path.display()))?;
    file.write_all(contents)
        .map_err(|error| format!("could not write {}: {error}", path.display()))
}

#[cfg(not(unix))]
fn write_secret_file(path: &Path, contents: &[u8]) -> Result<(), String> {
    // Windows has no mode bits; the file inherits the (user-private) parent
    // directory ACL. The caller is expected to pass a per-user directory.
    std::fs::write(path, contents)
        .map_err(|error| format!("could not write {}: {error}", path.display()))
}

/// Read a cargo metadata document, emit the CycloneDX SBOM, and print the
/// digest that a release manifest binds. Reads and writes files but performs
/// the transform through the pure library function, so the bytes on disk are a
/// function of the input file alone.
fn sbom_generate(metadata_path: &Path, output: &Path) -> Result<(), String> {
    let metadata_json = std::fs::read_to_string(metadata_path)
        .map_err(|error| format!("could not read {}: {error}", metadata_path.display()))?;
    let document = wcore_eval_scenarios::sbom::cyclonedx_from_cargo_metadata(&metadata_json)
        .map_err(|error| error.to_string())?;
    std::fs::write(output, document.as_bytes())
        .map_err(|error| format!("could not write {}: {error}", output.display()))?;
    println!(
        "SBOM WRITTEN path={} bytes={} sha256={}",
        output.display(),
        document.len(),
        wcore_eval_scenarios::sbom::sbom_sha256(&document)
    );
    Ok(())
}

/// The three clean-room results 29-02 measures, carried together so
/// `manifest_build` keeps one argument list rather than nine.
struct CleanRoomInputs<'a> {
    sbom: Option<&'a Path>,
    dependency_policy: Option<PolicyResult>,
    dependency_policy_config: Option<&'a Path>,
    reproducibility: Option<ReproducibilityKind>,
    variance_class: Option<VarianceClass>,
    variance_evidence: Option<&'a Path>,
}

fn sha256_file(path: &Path) -> Result<String, String> {
    let bytes = std::fs::read(path)
        .map_err(|error| format!("could not read {}: {error}", path.display()))?;
    Ok(format!("{:x}", Sha256::digest(&bytes)))
}

/// Turn the clean-room arguments into the manifest's evidence fields.
///
/// Every refusal here is deliberate. A variance verdict with no class and no
/// evidence would be the "unknown that reads as success" the manifest type was
/// built to make unrepresentable, so it is rejected at the CLI boundary rather
/// than encoded.
fn clean_room_evidence(
    inputs: &CleanRoomInputs<'_>,
) -> Result<
    (
        Evidence<SbomReferenceV1>,
        Evidence<DependencyPolicyOutcomeV1>,
        ReproducibilityVerdictV1,
    ),
    String,
> {
    let sbom = match inputs.sbom {
        Some(path) => Evidence::Observed {
            value: SbomReferenceV1 {
                name: path
                    .file_name()
                    .and_then(|value| value.to_str())
                    .ok_or_else(|| format!("non-UTF-8 SBOM name at {}", path.display()))?
                    .to_string(),
                sha256: sha256_file(path)?,
                format: SbomFormat::CycloneDxJson,
            },
        },
        None => Evidence::Unavailable {
            code: "sbom_not_produced_by_release_pipeline".to_string(),
        },
    };

    let dependency_policy = match (inputs.dependency_policy, inputs.dependency_policy_config) {
        (Some(result), Some(config)) => Evidence::Observed {
            value: DependencyPolicyOutcomeV1 {
                tool: "cargo-deny".to_string(),
                policy_sha256: sha256_file(config)?,
                result,
            },
        },
        (Some(_), None) => {
            return Err(
                "--dependency-policy requires --dependency-policy-config: a verdict without the \
                 policy it ran against is not a verdict"
                    .to_string(),
            );
        }
        (None, _) => Evidence::Unavailable {
            code: "dependency_policy_never_executed".to_string(),
        },
    };

    let reproducibility = match inputs.reproducibility {
        Some(ReproducibilityKind::Reproduced) => {
            if inputs.variance_class.is_some() || inputs.variance_evidence.is_some() {
                return Err(
                    "--reproducibility reproduced cannot carry a variance class or variance \
                     evidence"
                        .to_string(),
                );
            }
            ReproducibilityVerdictV1::Reproduced
        }
        Some(ReproducibilityKind::Variance) => {
            let class = inputs.variance_class.ok_or_else(|| {
                "--reproducibility variance requires --variance-class: a variance without a named \
                 class is an assertion, not a measurement"
                    .to_string()
            })?;
            let evidence = inputs.variance_evidence.ok_or_else(|| {
                "--reproducibility variance requires --variance-evidence".to_string()
            })?;
            ReproducibilityVerdictV1::Variance {
                class,
                evidence_sha256: sha256_file(evidence)?,
            }
        }
        None => ReproducibilityVerdictV1::Variance {
            class: VarianceClass::Unclassified,
            evidence_sha256: format!("{:x}", Sha256::digest(b"reproducibility-never-measured")),
        },
    };

    Ok((sbom, dependency_policy, reproducibility))
}

fn manifest_build(
    artifacts_dir: &Path,
    output: &Path,
    release_id: &str,
    source_commit: &str,
    inputs: CleanRoomInputs<'_>,
) -> Result<(), String> {
    let mut entries: Vec<_> = std::fs::read_dir(artifacts_dir)
        .map_err(|error| format!("could not read {}: {error}", artifacts_dir.display()))?
        .filter_map(Result::ok)
        .filter(|entry| entry.path().is_file())
        .collect();
    entries.sort_by_key(std::fs::DirEntry::file_name);

    let mut artifacts = Vec::new();
    for entry in entries {
        let path = entry.path();
        let bytes = std::fs::read(&path)
            .map_err(|error| format!("could not read {}: {error}", path.display()))?;
        let name = path
            .file_name()
            .and_then(|value| value.to_str())
            .ok_or_else(|| format!("non-UTF-8 artifact name at {}", path.display()))?
            .to_string();
        let kind = if name.contains("checksums") {
            ArtifactKind::Checksums
        } else if name.contains("sbom") {
            ArtifactKind::Sbom
        } else {
            ArtifactKind::Archive
        };
        artifacts.push(PackagedArtifactV1 {
            sha256: format!("{:x}", Sha256::digest(&bytes)),
            byte_length: bytes.len() as u64,
            name,
            kind,
        });
    }

    let (sbom, dependency_policy, reproducibility) = clean_room_evidence(&inputs)?;

    let body = ReleaseManifestBodyV1 {
        release_id: release_id.to_string(),
        source_commit: source_commit.to_string(),
        artifacts,
        sbom,
        dependency_policy,
        reproducibility,
        certification: Evidence::Unavailable {
            code: "phase_28_certification_binding_not_yet_available".to_string(),
        },
    };
    let manifest = ReleaseManifestV1::unsigned(body).map_err(|error| error.to_string())?;
    write_json(output, &manifest)?;
    println!("MANIFEST BUILT body_sha256={}", manifest.body_sha256);
    Ok(())
}

fn manifest_sign(manifest_path: &Path, output: &Path, key_id: &str) -> Result<(), String> {
    let manifest: ReleaseManifestV1 = read_json(manifest_path)?;
    let secret = read_seed_from_stdin()?;
    let signing_key = signing_key_from_seed_base64(&secret.0).map_err(|error| error.to_string())?;
    let signed = manifest.sign(key_id, &signing_key);
    write_json(output, &signed)?;
    println!("MANIFEST SIGNED body_sha256={}", signed.body_sha256);
    Ok(())
}

fn manifest_verify(
    manifest_path: &Path,
    trust_root_path: &Path,
    role: ReleaseState,
    now: u64,
) -> Result<(), String> {
    let manifest: ReleaseManifestV1 = read_json(manifest_path)?;
    let trust_root: ReleaseTrustRootV1 = read_json(trust_root_path)?;
    verify_manifest(&manifest, &trust_root, role, now).map_err(|error| error.to_string())?;
    println!("MANIFEST VERIFIED body_sha256={}", manifest.body_sha256);
    Ok(())
}

fn state_append(
    manifest_path: &Path,
    chain_path: &Path,
    state: ReleaseState,
    key_id: &str,
    evidence_specs: &[String],
) -> Result<(), String> {
    let manifest: ReleaseManifestV1 = read_json(manifest_path)?;
    let mut chain: Vec<ReleaseStateRecordV1> = if chain_path.exists() {
        read_json(chain_path)?
    } else {
        Vec::new()
    };

    let mut evidence = Vec::new();
    for spec in evidence_specs {
        let (name, path) = spec
            .split_once('=')
            .ok_or_else(|| format!("--evidence must be name=path, got {spec}"))?;
        let bytes =
            std::fs::read(path).map_err(|error| format!("could not read {path}: {error}"))?;
        evidence.push(StateEvidenceV1 {
            name: name.to_string(),
            sha256: format!("{:x}", Sha256::digest(&bytes)),
        });
    }

    let previous = chain.last().map(|record| record.body_sha256.clone());
    let record = ReleaseStateRecordV1::unsigned(state, &manifest.body_sha256, previous, evidence)
        .map_err(|error| error.to_string())?;
    let secret = read_seed_from_stdin()?;
    let signing_key = signing_key_from_seed_base64(&secret.0).map_err(|error| error.to_string())?;
    let signed = record.sign(key_id, &signing_key);
    println!(
        "STATE APPENDED state={} body_sha256={}",
        signed.body.state.as_str(),
        signed.body_sha256
    );
    chain.push(signed);
    write_json(chain_path, &chain)
}

fn state_verify(
    manifest_path: &Path,
    chain_path: &Path,
    trust_root_path: &Path,
    now: u64,
) -> Result<(), String> {
    let manifest: ReleaseManifestV1 = read_json(manifest_path)?;
    let trust_root: ReleaseTrustRootV1 = read_json(trust_root_path)?;
    let chain: Vec<ReleaseStateRecordV1> = read_json(chain_path)?;
    let progress = verify_state_chain(&chain, &manifest, &trust_root, now)
        .map_err(|error| error.to_string())?;
    let highest = progress.highest_state.map_or("none", ReleaseState::as_str);
    println!(
        "CHAIN VERIFIED highest_state={} records={} accepted={}",
        highest,
        progress.records_verified,
        progress.is_accepted()
    );
    Ok(())
}

/// Read a base64 seed from standard input. Never from an argument, never
/// echoed. Bounded exactly as `wayland-receipt sign` bounds its own read.
fn read_seed_from_stdin() -> Result<SecretBytes, String> {
    let mut secret = SecretBytes(Vec::new());
    std::io::stdin()
        .take(4097)
        .read_to_end(&mut secret.0)
        .map_err(|error| format!("could not read signing key from stdin: {error}"))?;
    if secret.0.len() > 4096 {
        return Err("signing key input exceeds 4096 bytes".to_string());
    }
    Ok(secret)
}

fn read_json<T: serde::de::DeserializeOwned>(path: &Path) -> Result<T, String> {
    let bytes = std::fs::read(path)
        .map_err(|error| format!("could not read {}: {error}", path.display()))?;
    serde_json::from_slice(&bytes)
        .map_err(|error| format!("invalid JSON in {}: {error}", path.display()))
}

fn write_json(path: &Path, value: &impl serde::Serialize) -> Result<(), String> {
    let encoded = serde_json::to_vec_pretty(value)
        .map_err(|error| format!("could not encode {}: {error}", path.display()))?;
    std::fs::write(path, &encoded)
        .map_err(|error| format!("could not write {}: {error}", path.display()))
}
