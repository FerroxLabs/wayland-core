//! Exact packaged-binary selection and identity validation for evaluation runs.

use std::ffi::OsStr;
use std::fs::File;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};

use sha2::{Digest, Sha256};
use thiserror::Error;

const PROBE_TIMEOUT: Duration = Duration::from_secs(5);
const MAX_PROBE_OUTPUT: u64 = 64 * 1024;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BinaryArtifact {
    pub path: PathBuf,
    pub sha256: String,
    pub version: String,
    pub source_commit: String,
}

#[derive(Debug, Clone, Copy)]
pub struct ArtifactExpectation<'a> {
    pub version: &'a str,
    pub source_commit: &'a str,
}

#[derive(Debug, Clone)]
pub struct ProbeOutput {
    pub success: bool,
    pub stdout: Vec<u8>,
    pub stderr: Vec<u8>,
}

#[derive(Debug, Error)]
pub enum ArtifactError {
    #[error("no wayland-core binary found")]
    Missing,
    #[error("invalid wayland-core artifact: {0}")]
    Invalid(String),
    #[error("wayland-core provenance mismatch: {0}")]
    Mismatch(String),
    #[error("could not probe wayland-core: {0}")]
    Probe(String),
}

/// Select one candidate without validating it. Validation failure is terminal:
/// callers must not fall through to a lower-precedence candidate.
pub fn select_candidate(
    explicit: Option<&Path>,
    environment: Option<&OsStr>,
    workspace_root: &Path,
) -> Result<PathBuf, ArtifactError> {
    if let Some(path) = explicit {
        return Ok(path.to_path_buf());
    }
    if let Some(path) = environment.filter(|value| !value.is_empty()) {
        return Ok(PathBuf::from(path));
    }

    let binary_name = format!("wayland-core{}", std::env::consts::EXE_SUFFIX);
    for profile in ["release", "debug"] {
        let candidate = workspace_root
            .join("target")
            .join(profile)
            .join(&binary_name);
        if candidate.exists() {
            return Ok(candidate);
        }
    }
    Err(ArtifactError::Missing)
}

/// Validate a selected binary using the production bounded `--build-info`
/// process probe.
pub fn inspect_binary(
    path: &Path,
    expected: ArtifactExpectation<'_>,
) -> Result<BinaryArtifact, ArtifactError> {
    inspect_with_probe(path, expected, probe_build_info)
}

/// Validate a selected artifact using an injected provenance probe. Production
/// uses the bounded process probe; unit tests remain offline and cross-platform.
pub fn inspect_with_probe<F>(
    path: &Path,
    expected: ArtifactExpectation<'_>,
    probe: F,
) -> Result<BinaryArtifact, ArtifactError>
where
    F: FnOnce(&Path) -> Result<ProbeOutput, ArtifactError>,
{
    let path = path.canonicalize().map_err(|error| {
        ArtifactError::Invalid(format!(
            "{} cannot be canonicalized: {error}",
            path.display()
        ))
    })?;
    let metadata = path.metadata().map_err(|error| {
        ArtifactError::Invalid(format!("{} cannot be inspected: {error}", path.display()))
    })?;
    if !metadata.is_file() {
        return Err(ArtifactError::Invalid(format!(
            "{} is not a regular file",
            path.display()
        )));
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if metadata.permissions().mode() & 0o111 == 0 {
            return Err(ArtifactError::Invalid(format!(
                "{} is not executable",
                path.display()
            )));
        }
    }

    validate_commit(expected.source_commit, "expected")?;
    let sha256 = sha256_file(&path)?;
    let output = probe(&path)?;
    if !output.success {
        return Err(ArtifactError::Probe(
            "--build-info exited unsuccessfully (stderr omitted)".into(),
        ));
    }
    let (version, source_commit) = parse_build_info(&output.stdout)?;
    if version != expected.version {
        return Err(ArtifactError::Mismatch(format!(
            "version {version} != expected {}",
            expected.version
        )));
    }
    if source_commit != expected.source_commit {
        return Err(ArtifactError::Mismatch(format!(
            "source {source_commit} != expected {}",
            expected.source_commit
        )));
    }

    Ok(BinaryArtifact {
        path,
        sha256,
        version,
        source_commit,
    })
}

fn sha256_file(path: &Path) -> Result<String, ArtifactError> {
    let mut file = File::open(path).map_err(|error| {
        ArtifactError::Invalid(format!("{} cannot be opened: {error}", path.display()))
    })?;
    let mut hasher = Sha256::new();
    std::io::copy(&mut file, &mut hasher).map_err(|error| {
        ArtifactError::Invalid(format!("{} cannot be hashed: {error}", path.display()))
    })?;
    Ok(format!("{:x}", hasher.finalize()))
}

fn parse_build_info(bytes: &[u8]) -> Result<(String, String), ArtifactError> {
    let text = std::str::from_utf8(bytes)
        .map_err(|_| ArtifactError::Probe("--build-info was not UTF-8".into()))?;
    let line = text.strip_suffix('\n').unwrap_or(text);
    let line = line.strip_suffix('\r').unwrap_or(line);
    if line.contains('\n') || line.contains('\r') {
        return Err(ArtifactError::Probe(
            "--build-info emitted more than one line".into(),
        ));
    }
    let body = line
        .strip_prefix("wayland-core ")
        .ok_or_else(|| ArtifactError::Probe("malformed --build-info prefix".into()))?;
    let (version, source) = body
        .split_once(" (source ")
        .ok_or_else(|| ArtifactError::Probe("malformed --build-info identity".into()))?;
    let source = source
        .strip_suffix(')')
        .ok_or_else(|| ArtifactError::Probe("malformed --build-info suffix".into()))?;
    if version.is_empty() {
        return Err(ArtifactError::Probe("--build-info version is empty".into()));
    }
    validate_commit(source, "reported")?;
    Ok((version.to_string(), source.to_string()))
}

fn validate_commit(commit: &str, origin: &str) -> Result<(), ArtifactError> {
    if commit.len() != 40
        || !commit
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
    {
        return Err(ArtifactError::Probe(format!(
            "{origin} source identity is not 40 lowercase hexadecimal characters"
        )));
    }
    Ok(())
}

fn probe_build_info(path: &Path) -> Result<ProbeOutput, ArtifactError> {
    let mut child = Command::new(path)
        .arg("--build-info")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|error| ArtifactError::Probe(format!("spawn failed: {error}")))?;
    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| ArtifactError::Probe("stdout pipe unavailable".into()))?;
    let stderr = child
        .stderr
        .take()
        .ok_or_else(|| ArtifactError::Probe("stderr pipe unavailable".into()))?;
    let stdout_reader = thread::spawn(move || read_limited(stdout));
    let stderr_reader = thread::spawn(move || read_limited(stderr));

    let deadline = Instant::now() + PROBE_TIMEOUT;
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) if Instant::now() < deadline => thread::sleep(Duration::from_millis(10)),
            Ok(None) => {
                let _ = child.kill();
                let _ = child.wait();
                let _ = stdout_reader.join();
                let _ = stderr_reader.join();
                return Err(ArtifactError::Probe("--build-info timed out".into()));
            }
            Err(error) => {
                let _ = child.kill();
                let _ = child.wait();
                let _ = stdout_reader.join();
                let _ = stderr_reader.join();
                return Err(ArtifactError::Probe(format!("wait failed: {error}")));
            }
        }
    };

    let stdout = join_reader(stdout_reader, "stdout")?;
    let stderr = join_reader(stderr_reader, "stderr")?;
    Ok(ProbeOutput {
        success: status.success(),
        stdout,
        stderr,
    })
}

fn read_limited(mut reader: impl Read) -> std::io::Result<Vec<u8>> {
    let mut bytes = Vec::new();
    reader
        .by_ref()
        .take(MAX_PROBE_OUTPUT + 1)
        .read_to_end(&mut bytes)?;
    if bytes.len() as u64 > MAX_PROBE_OUTPUT {
        return Err(std::io::Error::other("probe output exceeded 64 KiB"));
    }
    Ok(bytes)
}

fn join_reader(
    reader: thread::JoinHandle<std::io::Result<Vec<u8>>>,
    stream: &str,
) -> Result<Vec<u8>, ArtifactError> {
    reader
        .join()
        .map_err(|_| ArtifactError::Probe(format!("{stream} reader panicked")))?
        .map_err(|error| ArtifactError::Probe(format!("{stream} read failed: {error}")))
}

#[cfg(test)]
mod tests {
    use std::fs;

    use super::*;

    const COMMIT: &str = "0123456789abcdef0123456789abcdef01234567";

    fn candidate(root: &Path, profile: &str) -> PathBuf {
        root.join("target")
            .join(profile)
            .join(format!("wayland-core{}", std::env::consts::EXE_SUFFIX))
    }

    fn make_file(path: &Path, bytes: &[u8]) {
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, bytes).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(path, fs::Permissions::from_mode(0o755)).unwrap();
        }
    }

    fn good_probe(_: &Path) -> Result<ProbeOutput, ArtifactError> {
        Ok(ProbeOutput {
            success: true,
            stdout: format!("wayland-core 0.12.25 (source {COMMIT})\n").into_bytes(),
            stderr: Vec::new(),
        })
    }

    #[test]
    fn explicit_then_environment_then_release_then_debug() {
        let dir = tempfile::tempdir().unwrap();
        let explicit = dir.path().join("explicit");
        let environment = dir.path().join("environment");
        let release = candidate(dir.path(), "release");
        let debug = candidate(dir.path(), "debug");
        make_file(&release, b"release");
        make_file(&debug, b"debug");

        assert_eq!(
            select_candidate(Some(&explicit), Some(environment.as_os_str()), dir.path()).unwrap(),
            explicit
        );
        assert_eq!(
            select_candidate(None, Some(environment.as_os_str()), dir.path()).unwrap(),
            environment
        );
        assert_eq!(select_candidate(None, None, dir.path()).unwrap(), release);
        fs::remove_file(&release).unwrap();
        assert_eq!(select_candidate(None, None, dir.path()).unwrap(), debug);
    }

    #[test]
    fn selected_directory_is_rejected() {
        let dir = tempfile::tempdir().unwrap();
        let error = inspect_with_probe(
            dir.path(),
            ArtifactExpectation {
                version: "0.12.25",
                source_commit: COMMIT,
            },
            good_probe,
        )
        .unwrap_err();
        assert!(error.to_string().contains("regular file"));
    }

    #[test]
    fn matching_artifact_records_known_digest_and_identity() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("core");
        make_file(&path, b"abc");

        let artifact = inspect_with_probe(
            &path,
            ArtifactExpectation {
                version: "0.12.25",
                source_commit: COMMIT,
            },
            good_probe,
        )
        .unwrap();

        assert!(artifact.path.is_absolute());
        assert_eq!(
            artifact.sha256,
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
        assert_eq!(artifact.version, "0.12.25");
        assert_eq!(artifact.source_commit, COMMIT);
    }

    #[test]
    fn version_and_source_mismatches_are_rejected() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("core");
        make_file(&path, b"core");

        let wrong_version = inspect_with_probe(
            &path,
            ArtifactExpectation {
                version: "99.0.0",
                source_commit: COMMIT,
            },
            good_probe,
        )
        .unwrap_err();
        assert!(wrong_version.to_string().contains("version"));

        let wrong_commit = inspect_with_probe(
            &path,
            ArtifactExpectation {
                version: "0.12.25",
                source_commit: "ffffffffffffffffffffffffffffffffffffffff",
            },
            good_probe,
        )
        .unwrap_err();
        assert!(wrong_commit.to_string().contains("source"));
    }

    #[test]
    fn malformed_unknown_and_nonzero_probes_are_rejected() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("core");
        make_file(&path, b"core");
        let expectation = ArtifactExpectation {
            version: "0.12.25",
            source_commit: COMMIT,
        };

        for output in [
            ProbeOutput {
                success: true,
                stdout: b"not build info\n".to_vec(),
                stderr: Vec::new(),
            },
            ProbeOutput {
                success: true,
                stdout: b"wayland-core 0.12.25 (source unknown)\n".to_vec(),
                stderr: Vec::new(),
            },
            ProbeOutput {
                success: false,
                stdout: Vec::new(),
                stderr: b"failed".to_vec(),
            },
        ] {
            assert!(
                inspect_with_probe(&path, expectation, |_| Ok(output)).is_err(),
                "invalid probe output must fail closed"
            );
        }
    }

    #[test]
    fn mutation_during_the_identity_probe_is_rejected() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("core");
        make_file(&path, b"original");

        let error = inspect_with_probe(
            &path,
            ArtifactExpectation {
                version: "0.12.25",
                source_commit: COMMIT,
            },
            |probed| {
                make_file(probed, b"replacement");
                good_probe(probed)
            },
        )
        .unwrap_err();

        assert!(error.to_string().contains("changed during inspection"));
    }

    #[test]
    fn execution_artifact_is_a_private_sealed_copy() {
        let dir = tempfile::tempdir().unwrap();
        let source = dir.path().join("core");
        make_file(&source, b"original");
        let source = source.canonicalize().unwrap();

        let sealed = seal_with_probe(
            &source,
            ArtifactExpectation {
                version: "0.12.25",
                source_commit: COMMIT,
            },
            good_probe,
        )
        .unwrap();

        assert_ne!(sealed.path, source);
        assert!(sealed.path.exists());
        assert_eq!(fs::read(&sealed.path).unwrap(), b"original");
    }
}
