// F25-04: retained generations.
//
// This is the state that makes `update`, `rollback` and `recover` real rather
// than cosmetic. Without it `update` would be remove-then-install, which
// destroys exactly the bytes `rollback` has to restore — and a rollback verb
// that cannot restore is a verb that lies.
//
// Layout under the plugins root:
//
//   <root>/generations.json                        the ledger (also the
//                                                  governance marker)
//   <root>/.generations/<plugin>/<digest>/         one retained generation
//   <root>/.generations/<plugin>/.staging-*/       an in-flight copy
//   <root>/<plugin>@<market>/                      the LIVE install dir
//
// The live install dir is a full copy of the live generation, not a symlink:
// symlinks are refused outright by the loader's on-disk discovery (they are a
// containment-escape vector), so a symlinked live pointer would produce a
// plugin that installs fine and then silently never loads.
//
// Commit discipline: a generation is written to a `.staging-*` directory and
// only renamed into place once complete. A process killed mid-write therefore
// leaves a `.staging-*` directory — never a half-written generation that the
// ledger claims is whole.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::plugin::error::{PluginCliError, Result};

/// Directory holding every retained generation. Dot-prefixed so the loader's
/// plugins-root walk skips it (it carries no `plugin.toml` either way, but a
/// dot prefix also keeps it out of `plugin list` style scans).
pub const GENERATIONS_DIR: &str = ".generations";

const STAGING_PREFIX: &str = ".staging-";

/// One retained generation of one plugin.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GenerationRecord {
    /// SHA-256 of the generation's content — also its directory name.
    pub digest: String,
    pub version: String,
    /// RFC3339, supplied by the caller. Lib code never reads the wall clock.
    pub created_at: String,
}

/// Every generation of one plugin plus the pointer to the live one.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct PluginGenerations {
    /// Digest of the generation currently installed at the live path.
    pub live: Option<String>,
    /// Newest last.
    #[serde(default)]
    pub retained: Vec<GenerationRecord>,
    /// The live install directory, relative to the plugins root.
    pub live_path: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct GenerationsFile {
    #[serde(default)]
    pub plugins: BTreeMap<String, PluginGenerations>,
}

pub fn ledger_path(root: &Path) -> PathBuf {
    root.join(wcore_config::plugin_governance::GENERATIONS_FILE)
}

pub fn generations_root(root: &Path, plugin: &str) -> PathBuf {
    root.join(GENERATIONS_DIR).join(sanitize(plugin))
}

pub fn generation_dir(root: &Path, plugin: &str, digest: &str) -> PathBuf {
    generations_root(root, plugin).join(digest)
}

pub fn load(root: &Path) -> Result<GenerationsFile> {
    let p = ledger_path(root);
    if !p.is_file() {
        return Ok(GenerationsFile::default());
    }
    Ok(serde_json::from_str(&std::fs::read_to_string(&p)?)?)
}

pub fn store(root: &Path, f: &GenerationsFile) -> Result<()> {
    std::fs::create_dir_all(root)?;
    let bytes = serde_json::to_vec_pretty(f)?;
    wcore_config::atomic_write(ledger_path(root), &bytes)?;
    Ok(())
}

/// Content digest of a directory — the single shared implementation, so the
/// digest an approval is bound to is the digest the loader recomputes.
pub fn digest_of(dir: &Path) -> Result<String> {
    Ok(wcore_config::plugin_governance::content_digest(dir)?)
}

/// Copy `src` into a retained generation and record it.
///
/// Returns the generation digest. Re-retaining identical bytes is a no-op that
/// returns the same digest — generations are content-addressed, so there is
/// nothing to duplicate.
pub fn retain(
    root: &Path,
    plugin: &str,
    version: &str,
    live_path: &Path,
    src: &Path,
    created_at: String,
) -> Result<String> {
    let digest = digest_of(src)?;
    let target = generation_dir(root, plugin, &digest);
    if !target.is_dir() {
        let staging = generations_root(root, plugin).join(format!(
            "{STAGING_PREFIX}{}",
            &digest[..digest.len().min(16)]
        ));
        if staging.exists() {
            std::fs::remove_dir_all(&staging)?;
        }
        copy_tree(src, &staging)?;
        // Verify what actually landed, rather than trusting the copy. A short
        // read or a full disk here would otherwise retain a generation whose
        // name promises bytes it does not contain.
        let copied = digest_of(&staging)?;
        if copied != digest {
            std::fs::remove_dir_all(&staging).ok();
            return Err(PluginCliError::Quarantine(format!(
                "generation copy for {plugin} did not reproduce its source \
                 (source {digest}, copy {copied})"
            )));
        }
        if let Some(parent) = target.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::rename(&staging, &target)?;
    }

    let mut f = load(root)?;
    let entry = f.plugins.entry(plugin.to_string()).or_default();
    entry.retained.retain(|g| g.digest != digest);
    entry.retained.push(GenerationRecord {
        digest: digest.clone(),
        version: version.to_string(),
        created_at,
    });
    entry.live_path = Some(rel_to_root(root, live_path));
    store(root, &f)?;
    Ok(digest)
}

/// Point the ledger's live marker at `digest`.
pub fn set_live(root: &Path, plugin: &str, digest: &str) -> Result<()> {
    let mut f = load(root)?;
    let entry = f.plugins.entry(plugin.to_string()).or_default();
    entry.live = Some(digest.to_string());
    store(root, &f)
}

/// The generation immediately before the live one, if any.
pub fn prior_generation(root: &Path, plugin: &str) -> Result<Option<GenerationRecord>> {
    let f = load(root)?;
    let Some(entry) = f.plugins.get(plugin) else {
        return Ok(None);
    };
    let Some(live) = entry.live.as_deref() else {
        return Ok(entry.retained.last().cloned());
    };
    let idx = entry.retained.iter().position(|g| g.digest == live);
    Ok(match idx {
        Some(0) | None => None,
        Some(i) => entry.retained.get(i - 1).cloned(),
    })
}

pub fn get(root: &Path, plugin: &str) -> Result<Option<PluginGenerations>> {
    Ok(load(root)?.plugins.get(plugin).cloned())
}

/// Replace the contents of `dest` with the retained generation `digest`.
///
/// The new tree is assembled beside `dest` and swapped in, so an interruption
/// leaves either the old tree or the new one — never a blend of both.
pub fn restore(root: &Path, plugin: &str, digest: &str, dest: &Path) -> Result<()> {
    let src = generation_dir(root, plugin, digest);
    if !src.is_dir() {
        return Err(PluginCliError::Quarantine(format!(
            "generation {digest} of {plugin} is not on disk at {}",
            src.display()
        )));
    }
    swap_in(&src, dest)
}

/// Copy `src` over `dest` via a sibling staging dir + rename.
pub fn swap_in(src: &Path, dest: &Path) -> Result<()> {
    let parent = dest.parent().ok_or_else(|| {
        PluginCliError::Quarantine(format!("{} has no parent directory", dest.display()))
    })?;
    std::fs::create_dir_all(parent)?;
    let staging = parent.join(format!(
        "{STAGING_PREFIX}swap-{}",
        dest.file_name()
            .and_then(|s| s.to_str())
            .unwrap_or("plugin")
    ));
    if staging.exists() {
        std::fs::remove_dir_all(&staging)?;
    }
    copy_tree(src, &staging)?;
    // Windows cannot rename onto an existing directory, and neither can Unix
    // when the destination is non-empty. Retire the old tree first, then move
    // the new one in. The window between the two is the reason `recover`
    // exists: it is exactly the state `recover` is tested against.
    let retired = parent.join(format!(
        "{STAGING_PREFIX}retired-{}",
        dest.file_name()
            .and_then(|s| s.to_str())
            .unwrap_or("plugin")
    ));
    if retired.exists() {
        std::fs::remove_dir_all(&retired)?;
    }
    if dest.exists() {
        std::fs::rename(dest, &retired)?;
    }
    match std::fs::rename(&staging, dest) {
        Ok(()) => {
            if retired.exists() {
                std::fs::remove_dir_all(&retired).ok();
            }
            Ok(())
        }
        Err(e) => {
            // Put the old tree back rather than leaving nothing live.
            if retired.exists() && !dest.exists() {
                std::fs::rename(&retired, dest).ok();
            }
            Err(e.into())
        }
    }
}

/// What a recovery pass repaired. Empty means the store was already sound —
/// which `recover` must be able to report without inventing work.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct RecoveryReport {
    pub repairs: Vec<String>,
    pub unrepairable: Vec<String>,
}

impl RecoveryReport {
    pub fn is_clean(&self) -> bool {
        self.repairs.is_empty() && self.unrepairable.is_empty()
    }
}

/// Repair a half-written store.
///
/// Handles, concretely:
/// 1. leftover `.staging-*` directories from an interrupted write;
/// 2. a live pointer naming a generation that is not on disk;
/// 3. a live generation that exists while the live install directory is gone;
/// 4. a ledger entry for a plugin with no retained generations at all.
///
/// It never resurrects quarantined content and never restores a revoked
/// approval: it touches the generation store only, and approval state is a
/// separate file it does not write.
pub fn recover(root: &Path) -> Result<RecoveryReport> {
    let mut report = RecoveryReport::default();
    if !root.is_dir() {
        return Ok(report);
    }

    // 1. Sweep interrupted writes, wherever they were staged.
    for base in [root.to_path_buf(), root.join(GENERATIONS_DIR)] {
        sweep_staging(&base, &mut report)?;
    }
    let gens_root = root.join(GENERATIONS_DIR);
    if gens_root.is_dir() {
        for entry in std::fs::read_dir(&gens_root)? {
            let p = entry?.path();
            if p.is_dir() {
                sweep_staging(&p, &mut report)?;
            }
        }
    }

    let mut f = load(root)?;
    let mut dirty = false;
    let plugins: Vec<String> = f.plugins.keys().cloned().collect();

    for plugin in plugins {
        let Some(entry) = f.plugins.get_mut(&plugin) else {
            continue;
        };

        // Drop ledger entries for generations whose bytes are gone.
        let before = entry.retained.len();
        entry
            .retained
            .retain(|g| generation_dir(root, &plugin, &g.digest).is_dir());
        if entry.retained.len() != before {
            dirty = true;
            report.repairs.push(format!(
                "{plugin}: dropped {} ledger entr{} whose generation directory was missing",
                before - entry.retained.len(),
                if before - entry.retained.len() == 1 {
                    "y"
                } else {
                    "ies"
                }
            ));
        }

        let live_dir = entry.live_path.as_ref().map(|rel| root.join(rel));

        // 2. Live pointer aimed at a generation that is not on disk.
        let live_missing = match entry.live.as_deref() {
            Some(d) => !generation_dir(root, &plugin, d).is_dir(),
            None => true,
        };
        if live_missing {
            match entry.retained.last().cloned() {
                Some(newest) => {
                    entry.live = Some(newest.digest.clone());
                    dirty = true;
                    report.repairs.push(format!(
                        "{plugin}: live pointer named a generation that is not on disk — \
                         re-pointed at retained generation {}",
                        wcore_config::plugin_governance::short(&newest.digest)
                    ));
                    if let Some(dest) = &live_dir {
                        restore(root, &plugin, &newest.digest, dest)?;
                        report.repairs.push(format!(
                            "{plugin}: restored the live install directory from generation {}",
                            wcore_config::plugin_governance::short(&newest.digest)
                        ));
                    }
                }
                None => {
                    report.unrepairable.push(format!(
                        "{plugin}: no retained generation survives — reinstall it \
                         (`wayland-core plugin install {plugin}`)"
                    ));
                }
            }
            continue;
        }

        // 3. Live generation intact but the install directory is gone or stale.
        if let (Some(digest), Some(dest)) = (entry.live.clone(), live_dir) {
            let needs_restore = if dest.is_dir() {
                digest_of(&dest).map(|d| d != digest).unwrap_or(true)
            } else {
                true
            };
            if needs_restore {
                restore(root, &plugin, &digest, &dest)?;
                report.repairs.push(format!(
                    "{plugin}: install directory did not match live generation {} — restored it",
                    wcore_config::plugin_governance::short(&digest)
                ));
            }
        }
    }

    if dirty {
        store(root, &f)?;
    }
    Ok(report)
}

fn sweep_staging(dir: &Path, report: &mut RecoveryReport) -> Result<()> {
    if !dir.is_dir() {
        return Ok(());
    }
    for entry in std::fs::read_dir(dir)? {
        let p = entry?.path();
        let name = p
            .file_name()
            .and_then(|s| s.to_str())
            .unwrap_or_default()
            .to_string();
        if !name.starts_with(STAGING_PREFIX) {
            continue;
        }
        if p.is_dir() {
            std::fs::remove_dir_all(&p)?;
        } else {
            std::fs::remove_file(&p)?;
        }
        report
            .repairs
            .push(format!("removed interrupted staging directory {name}"));
    }
    Ok(())
}

/// Recursive copy. Symlinks are refused rather than followed: the plugins root
/// is a security boundary and a followed symlink would pull bytes from outside
/// it into a generation the digest then blesses.
pub fn copy_tree(src: &Path, dest: &Path) -> Result<()> {
    std::fs::create_dir_all(dest)?;
    for entry in std::fs::read_dir(src)? {
        let entry = entry?;
        let ty = entry.file_type()?;
        let from = entry.path();
        let to = dest.join(entry.file_name());
        if ty.is_symlink() {
            return Err(PluginCliError::PathTraversal(format!(
                "{} is a symlink; plugin trees are copied by value, never by reference",
                from.display()
            )));
        }
        if ty.is_dir() {
            copy_tree(&from, &to)?;
        } else {
            std::fs::copy(&from, &to)?;
            copy_permissions(&from, &to)?;
        }
    }
    Ok(())
}

/// Carry the executable bit across. A plugin whose entry binary loses `+x` in
/// a rollback is a plugin that "restored" into something that cannot run.
fn copy_permissions(from: &Path, to: &Path) -> Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = std::fs::metadata(from)?.permissions().mode();
        std::fs::set_permissions(to, std::fs::Permissions::from_mode(mode))?;
    }
    #[cfg(not(unix))]
    {
        let _ = (from, to);
    }
    Ok(())
}

fn rel_to_root(root: &Path, path: &Path) -> String {
    path.strip_prefix(root)
        .unwrap_or(path)
        .to_string_lossy()
        .replace('\\', "/")
}

fn sanitize(s: &str) -> String {
    s.chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.' | '@') {
                c
            } else {
                '_'
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn seed(root: &Path, name: &str, body: &[u8]) -> PathBuf {
        let dir = root.join(name);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("plugin.toml"), body).unwrap();
        dir
    }

    #[test]
    fn retain_then_rollback_restores_exact_bytes() {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path();
        let live = seed(root, "demo", b"v1");

        let d1 = retain(root, "demo", "1.0.0", &live, &live, "t1".into()).unwrap();
        set_live(root, "demo", &d1).unwrap();

        std::fs::write(live.join("plugin.toml"), b"v2").unwrap();
        let d2 = retain(root, "demo", "2.0.0", &live, &live, "t2".into()).unwrap();
        set_live(root, "demo", &d2).unwrap();
        assert_ne!(d1, d2);

        let prior = prior_generation(root, "demo").unwrap().expect("prior");
        assert_eq!(prior.digest, d1);
        restore(root, "demo", &prior.digest, &live).unwrap();
        set_live(root, "demo", &prior.digest).unwrap();

        assert_eq!(std::fs::read(live.join("plugin.toml")).unwrap(), b"v1");
        assert_eq!(digest_of(&live).unwrap(), d1);
    }

    #[test]
    fn recover_repairs_a_live_pointer_aimed_at_a_missing_generation() {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path();
        let live = seed(root, "demo", b"v1");
        let d1 = retain(root, "demo", "1.0.0", &live, &live, "t1".into()).unwrap();
        set_live(root, "demo", &d1).unwrap();
        std::fs::write(live.join("plugin.toml"), b"v2").unwrap();
        let d2 = retain(root, "demo", "2.0.0", &live, &live, "t2".into()).unwrap();
        set_live(root, "demo", &d2).unwrap();

        // Induce the damage: delete the live generation out from under the ledger.
        std::fs::remove_dir_all(generation_dir(root, "demo", &d2)).unwrap();

        let report = recover(root).unwrap();
        assert!(!report.is_clean(), "recover reported nothing to repair");
        assert!(report.unrepairable.is_empty(), "{report:?}");
        assert_eq!(get(root, "demo").unwrap().unwrap().live.unwrap(), d1);
        assert_eq!(std::fs::read(live.join("plugin.toml")).unwrap(), b"v1");
    }

    #[test]
    fn recover_sweeps_an_interrupted_staging_directory() {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path();
        let live = seed(root, "demo", b"v1");
        let d1 = retain(root, "demo", "1.0.0", &live, &live, "t1".into()).unwrap();
        set_live(root, "demo", &d1).unwrap();

        let partial = generations_root(root, "demo").join(format!("{STAGING_PREFIX}deadbeef"));
        std::fs::create_dir_all(&partial).unwrap();
        std::fs::write(partial.join("half.txt"), b"..").unwrap();

        let report = recover(root).unwrap();
        assert!(!partial.exists(), "staging dir survived recovery");
        assert!(
            report.repairs.iter().any(|r| r.contains("interrupted")),
            "{report:?}"
        );
    }

    #[test]
    fn recover_restores_an_install_directory_deleted_out_from_under_the_ledger() {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path();
        let live = seed(root, "demo", b"v1");
        let d1 = retain(root, "demo", "1.0.0", &live, &live, "t1".into()).unwrap();
        set_live(root, "demo", &d1).unwrap();

        std::fs::remove_dir_all(&live).unwrap();
        let report = recover(root).unwrap();
        assert!(live.is_dir(), "install dir not restored: {report:?}");
        assert_eq!(digest_of(&live).unwrap(), d1);
    }

    /// A sound store must recover to "nothing to do" — otherwise the verb
    /// manufactures repairs and its report proves nothing.
    #[test]
    fn recover_on_a_sound_store_reports_no_repairs() {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path();
        let live = seed(root, "demo", b"v1");
        let d1 = retain(root, "demo", "1.0.0", &live, &live, "t1".into()).unwrap();
        set_live(root, "demo", &d1).unwrap();

        let report = recover(root).unwrap();
        assert!(report.is_clean(), "expected a clean report, got {report:?}");
    }

    #[test]
    fn retaining_identical_bytes_is_content_addressed_not_duplicated() {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path();
        let live = seed(root, "demo", b"v1");
        let a = retain(root, "demo", "1.0.0", &live, &live, "t1".into()).unwrap();
        let b = retain(root, "demo", "1.0.1", &live, &live, "t2".into()).unwrap();
        assert_eq!(a, b);
        assert_eq!(get(root, "demo").unwrap().unwrap().retained.len(), 1);
    }

    #[test]
    fn copy_tree_refuses_a_symlink_rather_than_following_it() {
        let tmp = TempDir::new().unwrap();
        let src = tmp.path().join("src");
        std::fs::create_dir_all(&src).unwrap();
        std::fs::write(tmp.path().join("outside.txt"), b"secret").unwrap();
        #[cfg(unix)]
        std::os::unix::fs::symlink(tmp.path().join("outside.txt"), src.join("link.txt")).unwrap();
        #[cfg(windows)]
        {
            if std::os::windows::fs::symlink_file(
                tmp.path().join("outside.txt"),
                src.join("link.txt"),
            )
            .is_err()
            {
                // Unprivileged Windows cannot create symlinks; nothing to prove.
                return;
            }
        }
        let err = copy_tree(&src, &tmp.path().join("dest")).unwrap_err();
        assert!(
            matches!(err, PluginCliError::PathTraversal(_)),
            "expected a traversal refusal, got {err:?}"
        );
    }
}
