//! Explicit secret-source remapping across credential backends (F26-04).
//!
//! # The problem this exists for
//!
//! Where a secret actually lives differs per backend, and two of the four put it
//! somewhere a filesystem archive cannot carry to another machine:
//!
//! | Backend          | Where the secret really is                        | Portable? |
//! |------------------|---------------------------------------------------|-----------|
//! | `plaintext`      | `credentials*` in the home tree                    | yes, if carried |
//! | `auto`           | OS keyring first, `credentials*` as fallback       | partly    |
//! | `keyring`        | OS keychain — NOT in the tree at all               | no        |
//! | `encrypted_file` | files at ABSOLUTE paths the config names           | only if those paths are inside the home |
//!
//! Restoring a `keyring` home onto another machine without saying so produces a
//! config that points at secrets which are not there — a restore that reports
//! success and yields a broken install. Restoring an `encrypted_file` home
//! carrying the SOURCE machine's absolute paths is worse: those paths either
//! resolve to nothing or, on a shared layout, to a different file that happens
//! to exist.
//!
//! So each secret source becomes an explicit decision that is surfaced and
//! recorded, never guessed. Where no honest remap exists the restore REFUSES,
//! naming the backend, how many credential sources will be absent, and the
//! operator's next action.
//!
//! A refusal that names those three things is an honest outcome, not a dead end —
//! which is why "no explicit remap can be defined" is a much narrower condition
//! than "this backend cannot be automatically remapped".

use std::collections::BTreeMap;
use std::fmt;
use std::path::{Path, PathBuf};

use wcore_config::credentials::CredentialsBackend;

use super::BackupError;
use super::archive::{CredentialCapture, Manifest};

/// Where the config declares its credential storage.
const CONFIG_FILE: &str = "config.toml";

/// What the restore decided to do about the source's secrets.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RemapDisposition {
    /// Every secret source was resolved for the target home.
    Remapped,
    /// No honest remap existed and the restore refused rather than emitting a
    /// config pointing at credentials that are not present.
    Refused,
}

impl fmt::Display for RemapDisposition {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            RemapDisposition::Remapped => "remapped",
            RemapDisposition::Refused => "refused",
        })
    }
}

/// A config field whose absolute path must be rewritten for the target home.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PathRewrite {
    /// Dotted config key, e.g. `storage.credentials.backend.encrypted_file.cipher_path`.
    pub key: String,
    pub from: String,
    pub to: String,
}

/// The decision, its operator-facing message, and what it will rewrite.
#[derive(Debug, Clone)]
pub struct RemapPlan {
    pub disposition: RemapDisposition,
    /// Verbatim operator-facing text. Captured per backend by
    /// `scripts/portability-remap-capture.sh` and machine-checked for naming the
    /// backend, the count and an action.
    pub message: String,
    pub rewrites: Vec<PathRewrite>,
    /// Credential sources that will NOT be present after the restore.
    pub absent: Vec<String>,
}

/// Read the source home's declared credential backend and record what an archive
/// of it can and cannot carry.
pub(crate) fn capture_credentials(
    home: &Path,
    include_secrets: bool,
) -> Result<CredentialCapture, BackupError> {
    let backend = read_backend(home)?;
    let mut external_paths = BTreeMap::new();
    let mut secrets_outside_tree = false;

    match &backend {
        Some(CredentialsBackend::Keyring) => {
            // The OS keychain is not a file the archive can reach, at all.
            secrets_outside_tree = true;
        }
        Some(CredentialsBackend::Auto) => {
            // Auto prefers the keyring and falls back to plaintext, so an
            // archive cannot know whether the live secrets were in the tree.
            // Fail toward declaring the gap rather than toward silence.
            secrets_outside_tree = true;
        }
        Some(CredentialsBackend::EncryptedFile {
            cipher_path,
            key_params_path,
        }) => {
            for (key, p) in [
                ("cipher_path", cipher_path),
                ("key_params_path", key_params_path),
            ] {
                external_paths.insert(key.to_string(), p.display().to_string());
                if !path_is_inside(home, p) {
                    secrets_outside_tree = true;
                }
            }
        }
        Some(CredentialsBackend::Plaintext) | None => {}
    }

    Ok(CredentialCapture {
        backend: backend
            .as_ref()
            .map(backend_label)
            .unwrap_or("unknown")
            .to_string(),
        carried: include_secrets,
        external_paths,
        secrets_outside_tree,
    })
}

/// The stable label used in the manifest and in the capture records.
pub(crate) fn backend_label(b: &CredentialsBackend) -> &'static str {
    match b {
        CredentialsBackend::Auto => "auto",
        CredentialsBackend::Plaintext => "plaintext",
        CredentialsBackend::Keyring => "keyring",
        CredentialsBackend::EncryptedFile { .. } => "encrypted_file",
    }
}

/// Parse `storage.credentials.backend` out of a home's `config.toml`.
///
/// Returns `None` when the home declares no backend — which is not an error: a
/// home with no credential config has nothing to remap.
fn read_backend(home: &Path) -> Result<Option<CredentialsBackend>, BackupError> {
    let path = home.join(CONFIG_FILE);
    let Ok(text) = std::fs::read_to_string(&path) else {
        return Ok(None);
    };
    // `toml::Table`, NOT `toml::Value`: `Value: FromStr` parses a TOML *value*,
    // so a document beginning `[storage.credentials]` is read as an ARRAY and
    // then rejected for trailing content. That misparse is silent if its result
    // is discarded, and it made every home look like it declared no backend.
    let Ok(doc) = text.parse::<toml::Table>() else {
        return Ok(None);
    };
    let Some(value) = doc
        .get("storage")
        .and_then(|s| s.get("credentials"))
        .and_then(|c| c.get("backend"))
    else {
        return Ok(None);
    };
    // Deserialize through the REAL enum rather than string-matching, so this
    // stays correct if a variant is added or renamed.
    //
    // A backend that is DECLARED but unparseable is an error, never `None`:
    // treating it as "no credentials configured" would let the archive present
    // itself as a complete capture of a home whose secrets it could not locate.
    match value.clone().try_into::<CredentialsBackend>() {
        Ok(backend) => Ok(Some(backend)),
        Err(e) => Err(BackupError::Journal(format!(
            "{} declares a credentials backend this build cannot parse ({e}); \
             refusing to archive it as though no credentials were configured",
            path.display()
        ))),
    }
}

fn path_is_inside(home: &Path, candidate: &Path) -> bool {
    let home_c = home.canonicalize().unwrap_or_else(|_| home.to_path_buf());
    let cand_c = candidate
        .canonicalize()
        .unwrap_or_else(|_| candidate.to_path_buf());
    cand_c.starts_with(&home_c)
}

/// Decide what a restore of `manifest` into `target_home` must do about secrets.
///
/// `accept_missing` is the operator's explicit acknowledgement that the install
/// will come up without those credentials. Without it, a gap is a refusal.
pub fn plan_remap(
    manifest: &Manifest,
    target_home: &Path,
    accept_missing: bool,
) -> Result<RemapPlan, BackupError> {
    let cap = &manifest.credentials;
    let mut absent: Vec<String> = Vec::new();
    let mut rewrites: Vec<PathRewrite> = Vec::new();

    // Anything the archive deliberately did not carry is absent by construction.
    // This is exactly what a redacted archive cannot round-trip.
    if !cap.carried {
        absent.extend(manifest.absent_secrets.iter().cloned());
    }

    // The keyring/auto case: the secret was never a file the archive could take.
    if cap.secrets_outside_tree && matches!(cap.backend.as_str(), "keyring" | "auto") {
        absent.push(format!("{}-stored secrets (OS keychain)", cap.backend));
    }

    // The encrypted-file case: the paths are machine-specific and must be
    // rewritten for the target home, whatever else happens. A source absolute
    // path is NEVER carried through verbatim.
    for (key, from) in &cap.external_paths {
        let from_path = PathBuf::from(from);
        let file_name = from_path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| key.clone());
        let to = target_home.join(&file_name);
        rewrites.push(PathRewrite {
            key: format!("storage.credentials.backend.encrypted_file.{key}"),
            from: from.clone(),
            to: to.display().to_string(),
        });
        // The rewritten location only holds a secret if the archive carried it.
        if !carried_file(manifest, &file_name) {
            absent.push(format!("{key} ({file_name})"));
        }
    }

    absent.sort();
    absent.dedup();

    if absent.is_empty() {
        let message = format!(
            "credential remap: backend `{}` — 0 credential source(s) will be absent after restore. \
             action: none required; every secret source resolves inside the restored home.",
            cap.backend
        );
        return Ok(RemapPlan {
            disposition: RemapDisposition::Remapped,
            message,
            rewrites,
            absent,
        });
    }

    let message = format!(
        "credential remap: backend `{}` — {} credential source(s) will NOT be present after restore.\n\
         absent: {}\n\
         action: re-add these credentials on this machine (`wayland-core auth add <provider>`) \
         after the restore, or re-run with --accept-missing-secrets to proceed knowing the \
         restored install starts without them.",
        cap.backend,
        absent.len(),
        absent.join(", ")
    );

    if accept_missing {
        Ok(RemapPlan {
            disposition: RemapDisposition::Remapped,
            message,
            rewrites,
            absent,
        })
    } else {
        // A refusal REFUSES: the caller propagates this as an error and the
        // target is never written. "Warn and continue" is the outcome this
        // whole module exists to make impossible.
        Err(BackupError::RemapRefused(message))
    }
}

fn carried_file(manifest: &Manifest, file_name: &str) -> bool {
    manifest
        .payloads
        .iter()
        .any(|p| p.path == file_name || p.path.ends_with(&format!("/{file_name}")))
}

/// Apply the planned path rewrites to the restored home's `config.toml`.
///
/// Runs AFTER the payloads land, on the restored copy only. If the restored home
/// has no config, there is nothing to rewrite and that is not an error.
pub(crate) fn apply_rewrites(target_home: &Path, plan: &RemapPlan) -> Result<(), BackupError> {
    if plan.rewrites.is_empty() {
        return Ok(());
    }
    let path = target_home.join(CONFIG_FILE);
    let Ok(text) = std::fs::read_to_string(&path) else {
        return Ok(());
    };
    // `Table`, not `Value` — see `read_backend` for why a document must not be
    // parsed through `Value: FromStr`.
    let mut doc: toml::Table = text.parse().map_err(|e| {
        BackupError::Journal(format!("restored config.toml is not valid TOML: {e}"))
    })?;

    let mut changed = false;
    for rewrite in &plan.rewrites {
        let field = rewrite
            .key
            .rsplit('.')
            .next()
            .unwrap_or_default()
            .to_string();
        if let Some(slot) = doc
            .get_mut("storage")
            .and_then(|s| s.get_mut("credentials"))
            .and_then(|c| c.get_mut("backend"))
            .and_then(|b| b.get_mut("encrypted_file"))
            .and_then(|e| e.get_mut(&field))
        {
            *slot = toml::Value::String(rewrite.to.clone());
            changed = true;
        }
    }

    if changed {
        let serialized = toml::to_string_pretty(&doc)
            .map_err(|e| BackupError::Journal(format!("re-serialize config.toml: {e}")))?;
        wcore_config::atomic_io::atomic_write(&path, serialized.as_bytes())
            .map_err(BackupError::io("rewrite restored config"))?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::backup::archive::{FORMAT_ID, FORMAT_VERSION, PayloadEntry};

    fn manifest_with(cap: CredentialCapture, absent: Vec<String>) -> Manifest {
        Manifest {
            format: FORMAT_ID.to_string(),
            version: FORMAT_VERSION,
            created_utc: "1970-01-01T00:00:00Z".to_string(),
            digest_algo: crate::backup::archive::DIGEST_ALGO.to_string(),
            tree_digest: "0".repeat(64),
            payloads: vec![PayloadEntry {
                path: "config.toml".into(),
                bytes: 0,
                sha256: "0".repeat(64),
                mode: None,
            }],
            credentials: cap,
            absent_secrets: absent,
        }
    }

    fn write_config(home: &Path, body: &str) {
        std::fs::create_dir_all(home).unwrap();
        std::fs::write(home.join(CONFIG_FILE), body).unwrap();
    }

    #[test]
    fn each_backend_is_read_from_the_real_enum() {
        let dir = tempfile::tempdir().unwrap();
        for (body, expect) in [
            (
                "[storage.credentials]\nbackend = \"plaintext\"\n",
                "plaintext",
            ),
            ("[storage.credentials]\nbackend = \"keyring\"\n", "keyring"),
            ("[storage.credentials]\nbackend = \"auto\"\n", "auto"),
        ] {
            let home = dir.path().join(expect);
            write_config(&home, body);
            let cap = capture_credentials(&home, false).unwrap();
            assert_eq!(cap.backend, expect);
        }
        // The struct variant, in its externally-tagged TOML spelling.
        let home = dir.path().join("enc");
        write_config(
            &home,
            "[storage.credentials.backend.encrypted_file]\n\
             cipher_path = \"/src/machine/credentials.enc\"\n\
             key_params_path = \"/src/machine/credentials.kdf.json\"\n",
        );
        let cap = capture_credentials(&home, false).unwrap();
        assert_eq!(cap.backend, "encrypted_file");
        assert_eq!(cap.external_paths.len(), 2);
        assert!(
            cap.secrets_outside_tree,
            "paths outside the home are not portable"
        );
    }

    #[test]
    fn a_keyring_home_records_that_its_secrets_are_not_in_the_tree() {
        let dir = tempfile::tempdir().unwrap();
        let home = dir.path().join("h");
        write_config(&home, "[storage.credentials]\nbackend = \"keyring\"\n");
        let cap = capture_credentials(&home, true).unwrap();
        assert!(cap.secrets_outside_tree);

        // Positive control: plaintext with secrets carried is a complete capture,
        // so the flag above measures the backend rather than always being true.
        let plain = dir.path().join("p");
        write_config(&plain, "[storage.credentials]\nbackend = \"plaintext\"\n");
        let cap2 = capture_credentials(&plain, true).unwrap();
        assert!(!cap2.secrets_outside_tree);
    }

    #[test]
    fn a_cross_machine_restore_refuses_rather_than_pointing_at_absent_secrets() {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("target");
        let m = manifest_with(
            CredentialCapture {
                backend: "keyring".into(),
                carried: false,
                external_paths: BTreeMap::new(),
                secrets_outside_tree: true,
            },
            vec!["credentials.toml".into()],
        );
        let err = plan_remap(&m, &target, false).unwrap_err();
        let msg = match err {
            BackupError::RemapRefused(m) => m,
            other => panic!("expected a refusal, got {other:?}"),
        };
        assert!(
            msg.contains("keyring"),
            "message must name the backend: {msg}"
        );
        assert!(
            msg.contains("2 credential source(s)"),
            "message must name the count: {msg}"
        );
        assert!(
            msg.contains("action:"),
            "message must name an action: {msg}"
        );

        // With the explicit acknowledgement it proceeds, and still says so.
        let plan = plan_remap(&m, &target, true).unwrap();
        assert_eq!(plan.disposition, RemapDisposition::Remapped);
        assert_eq!(plan.absent.len(), 2);
    }

    #[test]
    fn no_source_machine_absolute_path_survives_into_the_target() {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("target");
        let mut ext = BTreeMap::new();
        ext.insert("cipher_path".into(), "/src/machine/credentials.enc".into());
        ext.insert(
            "key_params_path".into(),
            "/src/machine/credentials.kdf.json".into(),
        );
        let m = manifest_with(
            CredentialCapture {
                backend: "encrypted_file".into(),
                carried: true,
                external_paths: ext,
                secrets_outside_tree: true,
            },
            vec![],
        );
        let plan = plan_remap(&m, &target, true).unwrap();
        assert_eq!(plan.rewrites.len(), 2);
        for r in &plan.rewrites {
            assert!(
                r.from.starts_with("/src/machine/"),
                "the source path should be recorded verbatim as the FROM side"
            );
            assert!(
                Path::new(&r.to).starts_with(&target),
                "every rewrite must land inside the target home, got {}",
                r.to
            );
        }
    }

    #[test]
    fn apply_rewrites_replaces_the_absolute_paths_in_the_restored_config() {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("t");
        write_config(
            &target,
            "[storage.credentials.backend.encrypted_file]\n\
             cipher_path = \"/src/machine/credentials.enc\"\n\
             key_params_path = \"/src/machine/credentials.kdf.json\"\n",
        );
        let plan = RemapPlan {
            disposition: RemapDisposition::Remapped,
            message: String::new(),
            rewrites: vec![PathRewrite {
                key: "storage.credentials.backend.encrypted_file.cipher_path".into(),
                from: "/src/machine/credentials.enc".into(),
                to: target.join("credentials.enc").display().to_string(),
            }],
            absent: vec![],
        };
        apply_rewrites(&target, &plan).unwrap();
        let after = std::fs::read_to_string(target.join(CONFIG_FILE)).unwrap();
        assert!(
            !after.contains("/src/machine/credentials.enc"),
            "the source machine's absolute path survived: {after}"
        );
        assert!(after.contains(&target.join("credentials.enc").display().to_string()));
    }

    #[test]
    fn a_complete_capture_needs_no_acknowledgement() {
        // Negative control for the refusal tests: a plaintext home whose secrets
        // were carried has nothing absent, so it must NOT refuse.
        let dir = tempfile::tempdir().unwrap();
        let m = manifest_with(
            CredentialCapture {
                backend: "plaintext".into(),
                carried: true,
                external_paths: BTreeMap::new(),
                secrets_outside_tree: false,
            },
            vec![],
        );
        let plan = plan_remap(&m, &dir.path().join("t"), false).unwrap();
        assert_eq!(plan.disposition, RemapDisposition::Remapped);
        assert!(plan.absent.is_empty());
        assert!(plan.message.contains("0 credential source(s)"));
    }
}
