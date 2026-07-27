//! `wayland-core backup` — archive, verify, restore and recover a Wayland home
//! (F26-03, F26-04).
//!
//! # Why this exists
//!
//! Core shipped no backup, restore or rollback at all. `profile export` /
//! `profile import` copy a home tree with secrets excluded, but there is no
//! archive, no verification, no restore and — the load-bearing gap — no way to
//! undo an operation that died half-way through. This module is that surface.
//!
//! # The contract, and where each half is enforced
//!
//! * [`archive`] — creation with an embedded manifest, no-overwrite
//!   publication, and refusal of an output path inside the tree being archived.
//! * [`restore`] — verification BEFORE the first write, refusal of an occupied
//!   target, and the journalled `--replace` path for the case that actually
//!   matters: restoring over a home that already holds state.
//! * [`journal`] — the write-ahead record that makes an interrupted operation
//!   undoable. Modelled on [`crate::crash_sentinel`]'s settled shape: scoped per
//!   operation AND per process, acted on only when the owning process is dead.
//! * [`remap`] — where each credential backend's secrets actually live, and what
//!   a cross-machine restore must therefore declare or refuse.
//!
//! # What a redacted archive cannot round-trip
//!
//! By default an archive EXCLUDES the secret entries `wcore_config::profile`
//! already classifies (`credentials*`, `oauth/`), and the manifest records them
//! by name as absent. A redacted archive therefore cannot restore those secret
//! VALUES — by construction, not by defect. The round trip is lossless for
//! everything else and lossy for exactly the named set, and
//! [`archive::Manifest::absent_secrets`] is that set. Any claim that a redacted
//! round trip is byte-identical to its source would be false; the test suite
//! asserts the difference is exactly the recorded names and nothing more.

use std::path::{Path, PathBuf};

use clap::Subcommand;

pub mod archive;
pub mod journal;
pub mod remap;
pub mod restore;

/// Errors surfaced by the backup family. `thiserror` because callers (tests and
/// the remap capture) match on the variant, not on a rendered string.
#[derive(Debug, thiserror::Error)]
pub enum BackupError {
    #[error("io error while {context}: {source}")]
    Io {
        context: &'static str,
        #[source]
        source: std::io::Error,
    },

    #[error("source home does not exist or is not a directory: {0}")]
    SourceMissing(PathBuf),

    /// Publication is never a silent replacement — the peer bar refuses too.
    #[error("refusing to overwrite an existing archive at {0}")]
    OutputExists(PathBuf),

    /// An archive written inside the tree it is archiving grows until the disk
    /// does.
    #[error("refusing an output path inside the tree being archived: {0}")]
    OutputInsideSource(PathBuf),

    #[error("archive is not readable as a wayland-core backup: {0}")]
    NotAnArchive(String),

    #[error("archive verification failed: {0}")]
    VerificationFailed(String),

    /// Restore refuses an occupied target rather than replacing state in place.
    #[error(
        "refusing to restore into {0}: the target already holds state. \
         Pass --replace to replace it (the prior state is journalled and can be rolled back), \
         or choose an empty target."
    )]
    TargetOccupied(PathBuf),

    /// The remap refusal. Rendered message is operator-facing and is captured
    /// verbatim by `scripts/portability-remap-capture.sh`.
    #[error("{0}")]
    RemapRefused(String),

    #[error("journal error: {0}")]
    Journal(String),
}

impl BackupError {
    pub(crate) fn io(context: &'static str) -> impl FnOnce(std::io::Error) -> Self {
        move |source| BackupError::Io { context, source }
    }
}

/// `wayland-core backup <verb>`.
#[derive(Subcommand, Debug)]
pub enum BackupCmd {
    /// Create a verifiable archive of a Wayland home.
    Create {
        /// Home directory to archive.
        #[arg(long, value_name = "DIR")]
        home: PathBuf,
        /// Archive path to write. Must not already exist.
        #[arg(long, value_name = "FILE")]
        out: PathBuf,
        /// Carry in-tree secrets (`credentials*`, `oauth/`). OFF by default:
        /// a default archive is redacted and records the omitted names.
        #[arg(long)]
        include_secrets: bool,
    },
    /// Verify an archive's manifest, payload digests and entry paths.
    Verify {
        /// Archive path to verify.
        #[arg(value_name = "FILE")]
        archive: PathBuf,
    },
    /// Restore an archive into a home directory.
    Restore {
        /// Archive path to restore from.
        #[arg(value_name = "FILE")]
        archive: PathBuf,
        /// Target home directory.
        #[arg(long, value_name = "DIR")]
        home: PathBuf,
        /// Replace an occupied target. The prior state is captured into the
        /// journal's undo store first, so an interrupted replace rolls back to
        /// the exact pre-operation tree.
        #[arg(long)]
        replace: bool,
        /// Proceed when the archive could not carry the source's secrets.
        /// Without this the restore REFUSES rather than emitting a config that
        /// points at credentials which are not present.
        #[arg(long)]
        accept_missing_secrets: bool,
        /// Testing seam for the interruption proof: sleep this long between
        /// payload writes so a kill can land mid-flight. Never used in normal
        /// operation.
        #[arg(long, hide = true, default_value_t = 0)]
        pace_ms: u64,
    },
    /// Roll back any operation whose owning process died mid-flight.
    Recover {
        /// Home directory whose journal should be recovered.
        #[arg(long, value_name = "DIR")]
        home: PathBuf,
    },
    /// Print the tree digest of a home, and the algorithm used.
    ///
    /// Exists so an interruption proof compares pre- and post-recovery state
    /// using the SAME digest the journal itself records, on every platform. A
    /// proof script that reimplemented the digest would be comparing its own
    /// arithmetic across platforms rather than the product's.
    Digest {
        /// Home directory to digest.
        #[arg(long, value_name = "DIR")]
        home: PathBuf,
    },
}

/// Synchronous dispatch, mirroring `TopCmd::Migrate`.
pub fn run(cmd: BackupCmd) -> Result<(), BackupError> {
    match cmd {
        BackupCmd::Create {
            home,
            out,
            include_secrets,
        } => {
            let manifest = archive::create_archive(&home, &out, include_secrets)?;
            println!("archive: {}", out.display());
            println!("payloads: {}", manifest.payloads.len());
            println!("tree_digest: {}", manifest.tree_digest);
            println!("credentials_backend: {}", manifest.credentials.backend);
            println!(
                "secrets_carried: {}",
                if manifest.credentials.carried {
                    "yes"
                } else {
                    "no"
                }
            );
            if !manifest.absent_secrets.is_empty() {
                println!("absent_secrets: {}", manifest.absent_secrets.join(","));
            }
            Ok(())
        }
        BackupCmd::Verify { archive: path } => {
            let manifest = archive::verify_archive(&path)?;
            println!("verified: {}", path.display());
            println!("payloads: {}", manifest.payloads.len());
            println!("tree_digest: {}", manifest.tree_digest);
            Ok(())
        }
        BackupCmd::Restore {
            archive: path,
            home,
            replace,
            accept_missing_secrets,
            pace_ms,
        } => {
            // Arm the uncatchable-kill measurement when the proof asks for it.
            if let Ok(probe) = std::env::var("WAYLAND_BACKUP_KILL_PROBE") {
                arm_kill_handler_probe(PathBuf::from(probe));
            }
            let outcome = restore::restore_archive(
                &path,
                &home,
                restore::RestoreOptions {
                    replace,
                    accept_missing_secrets,
                    pace_ms,
                },
            )?;
            println!("restored: {}", home.display());
            println!("payloads: {}", outcome.written);
            println!("remap_disposition: {}", outcome.remap.disposition);
            if !outcome.remap.message.is_empty() {
                println!("{}", outcome.remap.message);
            }
            Ok(())
        }
        BackupCmd::Recover { home } => {
            let report = journal::recover(&home)?;
            println!("recovered_operations: {}", report.recovered);
            println!("skipped_live_owner: {}", report.skipped_live_owner);
            for op in &report.op_ids {
                println!("rolled_back: {op}");
            }
            Ok(())
        }
        BackupCmd::Digest { home } => {
            println!("DIGEST-ALGO: {}", archive::DIGEST_ALGO);
            println!("DIGEST: {}", journal::target_digest(&home)?);
            Ok(())
        }
    }
}

/// Install a probe that records whether a CATCHABLE termination signal was
/// delivered to this process.
///
/// This is what turns "the kill was uncatchable" from an assertion into a
/// measurement. The interruption proof arms the probe, kills the process with a
/// mechanism that cannot be trapped, and then requires the probe file to be
/// ABSENT — having first shown, in a control run, that the same probe DOES fire
/// for a catchable signal. Without that pair, "no probe file" is equally
/// consistent with a probe that was never installed.
///
/// Runs on its own thread with its own single-threaded runtime, so it is
/// independent of whatever the main runtime is doing while the restore blocks.
fn arm_kill_handler_probe(path: PathBuf) {
    std::thread::spawn(move || {
        let Ok(rt) = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
        else {
            return;
        };
        rt.block_on(async move {
            #[cfg(unix)]
            {
                use tokio::signal::unix::{SignalKind, signal};
                if let Ok(mut sig) = signal(SignalKind::terminate()) {
                    sig.recv().await;
                    let _ = std::fs::write(&path, b"fired");
                }
            }
            #[cfg(windows)]
            {
                if let Ok(mut sig) = tokio::signal::windows::ctrl_break() {
                    sig.recv().await;
                    let _ = std::fs::write(&path, b"fired");
                }
            }
        });
    });
}

/// One file that will be carried in an archive.
#[derive(Debug, Clone)]
pub(crate) struct Payload {
    /// Root-relative, `/`-separated. The single identity used by the manifest,
    /// the tar entry name and the restore path.
    pub(crate) rel: String,
    pub(crate) abs: PathBuf,
}

/// Walk `home` and collect the files an archive will carry.
///
/// Mirrors `profile::copy_tree_filtered`'s discipline exactly rather than
/// growing a second definition:
///
/// * symlinks are never followed and never carried (C6) — a hostile home cannot
///   redirect the archive out of the tree;
/// * Windows reparse points (junctions) are skipped by attribute, because
///   `is_symlink()` does not classify them;
/// * when `skip_secrets`, TOP-LEVEL entries matching
///   [`wcore_config::profile::is_secret_entry`] are omitted, and their names are
///   returned so the manifest can record what it did not carry.
pub(crate) fn collect_payloads(
    home: &Path,
    skip_secrets: bool,
) -> Result<(Vec<Payload>, Vec<String>), BackupError> {
    if !home.is_dir() {
        return Err(BackupError::SourceMissing(home.to_path_buf()));
    }
    let mut payloads = Vec::new();
    let mut omitted = Vec::new();
    walk(home, home, skip_secrets, true, &mut payloads, &mut omitted)?;
    // Total order derived from the data, never from directory iteration order,
    // so two walks of one tree produce byte-identical archives.
    payloads.sort_by(|a, b| a.rel.cmp(&b.rel));
    omitted.sort();
    omitted.dedup();
    Ok((payloads, omitted))
}

fn walk(
    root: &Path,
    dir: &Path,
    skip_secrets: bool,
    is_top_level: bool,
    out: &mut Vec<Payload>,
    omitted: &mut Vec<String>,
) -> Result<(), BackupError> {
    let entries = std::fs::read_dir(dir).map_err(BackupError::io("read source dir"))?;
    for entry in entries {
        let entry = entry.map_err(BackupError::io("read source entry"))?;
        let name = entry.file_name();
        let name_str = name.to_string_lossy().into_owned();

        if is_top_level && skip_secrets && wcore_config::profile::is_secret_entry(&name_str) {
            omitted.push(name_str);
            continue;
        }

        let path = entry.path();
        // symlink_metadata: classify the LINK, never its target.
        let meta = path
            .symlink_metadata()
            .map_err(BackupError::io("stat source entry"))?;
        if meta.file_type().is_symlink() {
            continue;
        }
        #[cfg(windows)]
        {
            use std::os::windows::fs::MetadataExt;
            const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x400;
            if meta.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0 {
                continue;
            }
        }

        if meta.is_dir() {
            walk(root, &path, skip_secrets, false, out, omitted)?;
        } else if meta.is_file()
            && let Some(rel) = rel_path(root, &path)
        {
            out.push(Payload { rel, abs: path });
        }
    }
    Ok(())
}

/// Root-relative, `/`-separated path. `None` for anything not genuinely under
/// `root`, and for anything carrying a surviving `..` component.
pub(crate) fn rel_path(root: &Path, path: &Path) -> Option<String> {
    use std::path::Component;
    let rel = path.strip_prefix(root).ok()?;
    if rel.components().any(|c| matches!(c, Component::ParentDir)) {
        return None;
    }
    Some(
        rel.components()
            .filter_map(|c| match c {
                Component::Normal(s) => Some(s.to_string_lossy().into_owned()),
                _ => None,
            })
            .collect::<Vec<_>>()
            .join("/"),
    )
}

/// Hex SHA-256 of `bytes`.
pub(crate) fn sha256_hex(bytes: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    let mut h = Sha256::new();
    h.update(bytes);
    let out = h.finalize();
    hex(&out)
}

pub(crate) fn hex(bytes: &[u8]) -> String {
    use std::fmt::Write as _;
    bytes.iter().fold(String::new(), |mut s, b| {
        let _ = write!(s, "{b:02x}");
        s
    })
}

/// True when a directory exists and contains at least one entry.
pub(crate) fn dir_holds_state(dir: &Path) -> bool {
    match std::fs::read_dir(dir) {
        Ok(mut it) => it.next().is_some(),
        Err(_) => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn collect_payloads_omits_top_level_secrets_and_names_them() {
        let dir = tempfile::tempdir().unwrap();
        let home = dir.path();
        std::fs::write(home.join("config.toml"), "a = 1").unwrap();
        std::fs::write(home.join("credentials.toml"), "SECRET").unwrap();
        std::fs::create_dir_all(home.join("oauth")).unwrap();
        std::fs::write(home.join("oauth/token.json"), "SECRET").unwrap();
        std::fs::create_dir_all(home.join("skills")).unwrap();
        std::fs::write(home.join("skills/SKILL.md"), "body").unwrap();

        let (payloads, omitted) = collect_payloads(home, true).unwrap();
        let rels: Vec<&str> = payloads.iter().map(|p| p.rel.as_str()).collect();
        assert_eq!(rels, vec!["config.toml", "skills/SKILL.md"]);
        assert_eq!(
            omitted,
            vec!["credentials.toml".to_string(), "oauth".to_string()]
        );

        // Positive control: the SAME tree with secrets included carries them,
        // so the omission above measures the filter rather than an empty tree.
        let (all, none_omitted) = collect_payloads(home, false).unwrap();
        assert!(none_omitted.is_empty());
        assert!(all.iter().any(|p| p.rel == "credentials.toml"));
        assert!(all.iter().any(|p| p.rel == "oauth/token.json"));
    }

    #[test]
    fn collect_payloads_rejects_a_missing_home() {
        let dir = tempfile::tempdir().unwrap();
        let err = collect_payloads(&dir.path().join("nope"), true).unwrap_err();
        assert!(matches!(err, BackupError::SourceMissing(_)), "{err:?}");
    }

    #[cfg(unix)]
    #[test]
    fn collect_payloads_never_follows_a_symlink_out_of_the_tree() {
        let outside = tempfile::tempdir().unwrap();
        std::fs::write(outside.path().join("secret.txt"), "OUTSIDE").unwrap();
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("real.txt"), "inside").unwrap();
        std::os::unix::fs::symlink(outside.path(), dir.path().join("escape")).unwrap();

        let (payloads, _) = collect_payloads(dir.path(), true).unwrap();
        let rels: Vec<&str> = payloads.iter().map(|p| p.rel.as_str()).collect();
        assert_eq!(rels, vec!["real.txt"], "a symlink was traversed or carried");
    }

    #[test]
    fn rel_path_is_slash_separated_and_rejects_escapes() {
        let root = Path::new("/tmp/home");
        assert_eq!(
            rel_path(root, Path::new("/tmp/home/a/b.txt")).as_deref(),
            Some("a/b.txt")
        );
        assert_eq!(rel_path(root, Path::new("/tmp/other/x")), None);
    }

    #[test]
    fn dir_holds_state_distinguishes_empty_from_populated() {
        let dir = tempfile::tempdir().unwrap();
        assert!(!dir_holds_state(dir.path()));
        std::fs::write(dir.path().join("x"), "1").unwrap();
        assert!(dir_holds_state(dir.path()));
        assert!(!dir_holds_state(&dir.path().join("absent")));
    }
}
