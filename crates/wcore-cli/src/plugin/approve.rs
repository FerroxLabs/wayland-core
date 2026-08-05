// F25-04: `plugin approve` / `plugin approve --revoke`.
//
// APPROVAL IS A GATE, NOT A PROMPT. This verb only writes the record; the
// enforcement lives in `wcore-agent`'s on-disk loader, which refuses to
// initialise a plugin whose current content digest has no matching approval.
// Both sides call `wcore_config::plugin_governance`, so there is exactly one
// implementation of the verdict — a second one here would be a second answer to
// the same security question.
//
// The record is bound to the CONTENT DIGEST, not the plugin name. That is what
// makes an update invalidate the prior approval instead of inheriting it: new
// bytes are a new thing to consent to.

use std::path::{Path, PathBuf};

use wcore_config::plugin_governance as gov;

use crate::plugin::error::{PluginCliError, Result};
use crate::plugin::verify;

/// An installed plugin located on disk.
pub struct InstalledPlugin {
    /// The name in `plugin.toml` — the identity the LOADER uses, which is what
    /// the approval must be keyed on. The directory name can differ (the
    /// marketplace path installs into `<plugin>@<market>/`).
    pub name: String,
    pub dir: PathBuf,
}

/// Find an installed plugin by its manifest name or its directory name.
///
/// Both are accepted because an operator reads `<plugin>@<market>` off
/// `plugin list` and reasonably types that back in, while the loader knows the
/// plugin by its manifest name. Resolving both and keying the record on the
/// manifest name keeps the two from drifting apart.
pub fn find_installed(plugins_root: &Path, needle: &str) -> Result<InstalledPlugin> {
    for dir in plugin_dirs(plugins_root)? {
        let dir_name = dir
            .file_name()
            .and_then(|s| s.to_str())
            .unwrap_or_default()
            .to_string();
        let manifest = match verify::load_manifest(&dir) {
            Ok(m) => m,
            Err(_) => continue,
        };
        if manifest.plugin.name == needle || dir_name == needle {
            return Ok(InstalledPlugin {
                name: manifest.plugin.name,
                dir,
            });
        }
    }
    Err(PluginCliError::NotInstalled(needle.to_string()))
}

/// Every directory under the plugins root that carries a `plugin.toml`.
pub fn plugin_dirs(plugins_root: &Path) -> Result<Vec<PathBuf>> {
    let mut out = Vec::new();
    if !plugins_root.is_dir() {
        return Ok(out);
    }
    for entry in std::fs::read_dir(plugins_root)? {
        let p = entry?.path();
        if p.is_dir() && p.join("plugin.toml").is_file() {
            out.push(p);
        }
    }
    out.sort();
    Ok(out)
}

/// Make the root governed. Called by every verb that establishes lifecycle
/// state, so a root cannot end up carrying approvals while the loader still
/// treats it as ungoverned.
pub fn ensure_governed(plugins_root: &Path) -> Result<()> {
    if !gov::is_governed(plugins_root) {
        crate::plugin::generations::store(
            plugins_root,
            &crate::plugin::generations::GenerationsFile::default(),
        )?;
    }
    Ok(())
}

/// Record approval of `needle`'s CURRENT bytes.
pub fn approve(plugins_root: &Path, needle: &str, approved_at: String) -> Result<()> {
    let found = find_installed(plugins_root, needle)?;
    ensure_governed(plugins_root)?;
    let digest = gov::content_digest(&found.dir)?;

    let mut store = gov::load_approvals(plugins_root)?;
    // Approving these bytes clears any revocation of these same bytes: the
    // operator is deliberately reversing their own earlier decision, which is
    // different from `recover` silently resurrecting it.
    store
        .revoked
        .retain(|r| !(r.plugin == found.name && r.digest == digest));
    store.approvals.insert(
        found.name.clone(),
        gov::ApprovalRecord {
            plugin: found.name.clone(),
            digest: digest.clone(),
            approved_at,
        },
    );
    gov::store_approvals(plugins_root, &store)?;

    println!("approved {} at digest {}", found.name, gov::short(&digest));
    println!("  {}", found.dir.display());
    for line in verify::describe_permissions(&verify::load_manifest(&found.dir)?) {
        println!("  grants: {line}");
    }
    Ok(())
}

/// Withdraw approval. The revocation is RETAINED, not just deleted, so
/// `recover` can be forbidden from restoring an authority a human withdrew.
pub fn revoke(plugins_root: &Path, needle: &str, revoked_at: String) -> Result<()> {
    let found = find_installed(plugins_root, needle)?;
    ensure_governed(plugins_root)?;

    let mut store = gov::load_approvals(plugins_root)?;
    let previous = store.approvals.remove(&found.name);
    let digest = match &previous {
        Some(rec) => rec.digest.clone(),
        None => gov::content_digest(&found.dir)?,
    };
    store.revoked.push(gov::RevocationRecord {
        plugin: found.name.clone(),
        digest: digest.clone(),
        revoked_at,
    });
    gov::store_approvals(plugins_root, &store)?;

    match previous {
        Some(_) => println!(
            "revoked approval for {} (was approved at digest {})",
            found.name,
            gov::short(&digest)
        ),
        None => println!(
            "{} had no approval to revoke; recorded the revocation anyway so a \
             later recovery cannot grant one",
            found.name
        ),
    }
    println!("it will now be REFUSED at load until re-approved");
    Ok(())
}

/// Was this exact (plugin, digest) pair deliberately revoked?
pub fn is_revoked(plugins_root: &Path, plugin: &str, digest: &str) -> Result<bool> {
    Ok(gov::load_approvals(plugins_root)?
        .revoked
        .iter()
        .any(|r| r.plugin == plugin && r.digest == digest))
}

pub fn run(plugins_root: &Path, name: &str, revoke_flag: bool, now: String) -> Result<()> {
    if revoke_flag {
        revoke(plugins_root, name, now)
    } else {
        approve(plugins_root, name, now)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn install(root: &Path, dir_name: &str, plugin_name: &str, body: &str) -> PathBuf {
        let d = root.join(dir_name);
        std::fs::create_dir_all(&d).unwrap();
        std::fs::write(
            d.join("plugin.toml"),
            format!(
                "plugin_api_version = \"{}\"\n[plugin]\nname = \"{plugin_name}\"\n\
                 version = \"1.0.0\"\ndescription = \"{body}\"\nlicense = \"MIT\"\n\
                 [permissions]\nregister_hooks = true\n[runtime]\nkind = \"declarative\"\n",
                wcore_plugin_api::PLUGIN_API_VERSION
            ),
        )
        .unwrap();
        d
    }

    #[test]
    fn approval_admits_and_revocation_re_arms_the_refusal() {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path();
        let dir = install(root, "demo@local", "demo", "v1");

        // Before any lifecycle state exists the root is ungoverned.
        assert_eq!(
            gov::evaluate(root, "demo", &dir),
            gov::GateVerdict::NotGoverned
        );

        ensure_governed(root).unwrap();
        assert!(matches!(
            gov::evaluate(root, "demo", &dir),
            gov::GateVerdict::Refused { .. }
        ));

        approve(root, "demo", "t0".into()).unwrap();
        assert!(matches!(
            gov::evaluate(root, "demo", &dir),
            gov::GateVerdict::Approved { .. }
        ));

        revoke(root, "demo", "t1".into()).unwrap();
        assert!(matches!(
            gov::evaluate(root, "demo", &dir),
            gov::GateVerdict::Refused { .. }
        ));
    }

    /// The whole point of digest binding: new bytes are a new decision.
    #[test]
    fn changing_the_plugin_invalidates_its_approval() {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path();
        let dir = install(root, "demo@local", "demo", "v1");
        approve(root, "demo", "t0".into()).unwrap();
        assert!(matches!(
            gov::evaluate(root, "demo", &dir),
            gov::GateVerdict::Approved { .. }
        ));

        install(root, "demo@local", "demo", "v2");
        match gov::evaluate(root, "demo", &dir) {
            gov::GateVerdict::Refused { reason } => {
                assert!(reason.contains("does not match"), "{reason}")
            }
            other => panic!("expected refusal after mutation, got {other:?}"),
        }
    }

    #[test]
    fn a_revocation_is_retained_so_recovery_cannot_undo_it() {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path();
        let dir = install(root, "demo@local", "demo", "v1");
        let digest = gov::content_digest(&dir).unwrap();
        approve(root, "demo", "t0".into()).unwrap();
        revoke(root, "demo", "t1".into()).unwrap();
        assert!(is_revoked(root, "demo", &digest).unwrap());
    }

    #[test]
    fn a_plugin_is_findable_by_directory_name_or_manifest_name() {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path();
        install(root, "demo@local", "demo", "v1");
        assert_eq!(find_installed(root, "demo").unwrap().name, "demo");
        assert_eq!(find_installed(root, "demo@local").unwrap().name, "demo");
        assert!(find_installed(root, "absent").is_err());
    }
}
