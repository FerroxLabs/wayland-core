//! Exact packaged-binary selection and identity validation for evaluation runs.

use std::ffi::OsStr;
use std::path::{Path, PathBuf};

use thiserror::Error;

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
    _explicit: Option<&Path>,
    _environment: Option<&OsStr>,
    _workspace_root: &Path,
) -> Result<PathBuf, ArtifactError> {
    Err(ArtifactError::Missing)
}

/// Validate a selected artifact using an injected provenance probe. Production
/// uses the bounded process probe; unit tests remain offline and cross-platform.
pub fn inspect_with_probe<F>(
    _path: &Path,
    _expected: ArtifactExpectation<'_>,
    _probe: F,
) -> Result<BinaryArtifact, ArtifactError>
where
    F: FnOnce(&Path) -> Result<ProbeOutput, ArtifactError>,
{
    Err(ArtifactError::Invalid("not implemented".into()))
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
}
