// F25-04: `plugin publish` — produce a digest-addressed, signed,
// manifest-complete bundle into a target the operator names.
//
// PUBLISH DOES NOT PUSH. PROJECT.md reserves push, merge, release and
// deployment as human-authorized actions, so this verb writes an artifact and
// stops. Its target is a plain local directory laid out as a Claude-Code-shaped
// marketplace, which is what lets the whole lifecycle — publish, add, browse,
// install — be proven end to end without a network service or an authorization
// nobody in this loop can give.
//
// The bundle is deliberately NOT a new archive format. It is the plugin tree
// verbatim plus one `bundle.json` sidecar recording the content digest. The
// authenticity anchor stays `wayland-plugin.sig`, verified by the engine's
// existing trust root; `bundle.json` adds integrity (did these bytes change in
// transit?) on top of it, and is honestly labelled as such.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use wcore_agent::plugins::sig_verifier::PLUGIN_SIG_FILENAME;

use crate::plugin::error::{PluginCliError, Result};
use crate::plugin::{generations, verify};

/// Sidecar written beside a published plugin.
pub const BUNDLE_FILE: &str = "bundle.json";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BundleManifest {
    pub name: String,
    pub version: String,
    /// SHA-256 of the published plugin tree, `bundle.json` itself excluded.
    pub content_digest: String,
    /// Relative path of the artifact the detached signature covers.
    pub signed_artifact: String,
    pub signature_file: String,
    pub published_at: String,
}

/// Where a published plugin lands inside the target marketplace.
pub fn plugin_dir_in(target: &Path, name: &str) -> PathBuf {
    target.join("plugins").join(name)
}

/// Recompute a published tree's digest and compare it with its own sidecar.
///
/// `bundle.json` is written AFTER the digest is taken and is excluded from the
/// recomputation, so this is a genuine comparison rather than a hash of the
/// answer.
pub fn verify_bundle(dir: &Path) -> Result<BundleManifest> {
    let raw = std::fs::read_to_string(dir.join(BUNDLE_FILE))
        .map_err(|e| PluginCliError::Quarantine(format!("{}/{BUNDLE_FILE}: {e}", dir.display())))?;
    let bundle: BundleManifest = serde_json::from_str(&raw)?;
    let actual = digest_excluding_sidecar(dir)?;
    if actual != bundle.content_digest {
        return Err(PluginCliError::Quarantine(format!(
            "bundle integrity check FAILED for {}: {BUNDLE_FILE} records {} but the tree \
             hashes to {} — the published bytes were modified after publication",
            bundle.name, bundle.content_digest, actual
        )));
    }
    Ok(bundle)
}

/// Files that are NOT part of what a publisher signed, because they are written
/// after publication by the installer.
///
/// `bundle.json` is written after the digest is taken (it CONTAINS the digest);
/// `provenance.json` is stamped by the install path to record where the plugin
/// came from. Neither existed when the author published, so hashing them would
/// make every installed bundle read as tampered. The list is short, named and
/// documented on purpose: an exclusion list is how a hash quietly stops
/// covering the file that matters, so it must never grow without a reason
/// written next to it.
pub const POST_PUBLICATION_SIDECARS: &[&str] = &[BUNDLE_FILE, "provenance.json"];

/// Digest of a published tree with the post-publication sidecars removed.
///
/// Implemented by copying to a scratch dir minus those files rather than by
/// teaching the digest function about exclusions: that primitive is shared with
/// the approval gate, where an exclusion would be a hole.
pub fn digest_excluding_sidecar(dir: &Path) -> Result<String> {
    let scratch = tempfile::tempdir()?;
    let staged = scratch.path().join("t");
    generations::copy_tree(dir, &staged)?;
    for name in POST_PUBLICATION_SIDECARS {
        let sidecar = staged.join(name);
        if sidecar.exists() {
            std::fs::remove_file(&sidecar)?;
        }
    }
    generations::digest_of(&staged)
}

/// `plugin publish <dir> --to <target>`.
pub fn run(dir: &Path, target: &Path, published_at: String) -> Result<()> {
    let report = verify::verify_dir(dir)?;
    if report.is_fatal() {
        return Err(PluginCliError::Quarantine(format!(
            "refusing to publish {}: it does not verify ({})",
            report.name, report.api_detail
        )));
    }
    if !report.signature_present {
        return Err(PluginCliError::Quarantine(format!(
            "refusing to publish {} unsigned: no {PLUGIN_SIG_FILENAME} beside its entry \
             artifact in {}. Sign it first: `wayland-core plugin sign {} --key <key>`",
            report.name,
            dir.display(),
            dir.display()
        )));
    }
    let Some(signed_artifact) = report.entry_artifact.clone() else {
        return Err(PluginCliError::Quarantine(format!(
            "refusing to publish {}: it carries a signature but declares no entry \
             artifact, so nothing states WHAT was signed",
            report.name
        )));
    };

    let dest = plugin_dir_in(target, &report.name);
    if dest.exists() {
        std::fs::remove_dir_all(&dest)?;
    }
    generations::copy_tree(dir, &dest)?;
    // Any sidecar carried over from a previous publish must go before the
    // digest is taken, or the new digest would cover the old answer.
    let stale = dest.join(BUNDLE_FILE);
    if stale.exists() {
        std::fs::remove_file(&stale)?;
    }

    // Recorded with the SAME exclusion `verify_bundle` applies, so publishing a
    // tree that already carries an install-time sidecar still verifies later.
    let content_digest = digest_excluding_sidecar(&dest)?;
    let bundle = BundleManifest {
        name: report.name.clone(),
        version: report.version.clone(),
        content_digest: content_digest.clone(),
        signed_artifact,
        signature_file: PLUGIN_SIG_FILENAME.to_string(),
        published_at,
    };
    std::fs::write(dest.join(BUNDLE_FILE), serde_json::to_vec_pretty(&bundle)?)?;

    upsert_catalog(target, &report.name, &report.version)?;

    println!("published {} {}", bundle.name, bundle.version);
    println!("  digest   {}", bundle.content_digest);
    println!(
        "  signed   {} ({})",
        bundle.signed_artifact, bundle.signature_file
    );
    println!("  target   {}", dest.display());
    println!(
        "  install  wayland-core plugin marketplace add {} && \
         wayland-core plugin install {}@{}",
        target.display(),
        bundle.name,
        marketplace_name(target)?
    );
    Ok(())
}

/// Create or extend the target's `.claude-plugin/marketplace.json` so the
/// existing `marketplace add` / `available` / `install` path can consume it
/// unchanged. Reusing the catalog format rather than inventing a publish-only
/// one is what keeps the lifecycle a single pipeline instead of two.
fn upsert_catalog(target: &Path, name: &str, version: &str) -> Result<()> {
    let cdir = target.join(".claude-plugin");
    std::fs::create_dir_all(&cdir)?;
    let cpath = cdir.join("marketplace.json");
    let mut root: Value = if cpath.is_file() {
        serde_json::from_str(&std::fs::read_to_string(&cpath)?)?
    } else {
        json!({
            "name": default_market_name(target),
            "owner": { "name": "local publisher" },
            "plugins": []
        })
    };
    let plugins = root
        .get_mut("plugins")
        .and_then(Value::as_array_mut)
        .ok_or_else(|| {
            PluginCliError::Quarantine(format!(
                "{}: 'plugins' is missing or not an array",
                cpath.display()
            ))
        })?;
    plugins.retain(|p| p.get("name").and_then(Value::as_str) != Some(name));
    plugins.push(json!({
        "name": name,
        "version": version,
        "description": format!("{name} {version}, published locally"),
        "source": format!("./plugins/{name}")
    }));
    std::fs::write(&cpath, serde_json::to_vec_pretty(&root)?)?;
    Ok(())
}

fn default_market_name(target: &Path) -> String {
    target
        .file_name()
        .and_then(|s| s.to_str())
        .filter(|s| !s.is_empty())
        .unwrap_or("local")
        .to_string()
}

/// The declared name of the catalog at `target` — what `install <p>@<name>`
/// needs. Read back from disk rather than recomputed, so the printed command
/// is the one that actually works.
pub fn marketplace_name(target: &Path) -> Result<String> {
    let cpath = target.join(".claude-plugin").join("marketplace.json");
    let root: Value = serde_json::from_str(&std::fs::read_to_string(&cpath)?)?;
    Ok(root
        .get("name")
        .and_then(Value::as_str)
        .unwrap_or("local")
        .to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn signed_plugin(dir: &Path, body: &[u8]) -> PathBuf {
        std::fs::create_dir_all(dir.join("bin")).unwrap();
        std::fs::write(dir.join("bin").join("run"), body).unwrap();
        std::fs::write(
            dir.join("plugin.toml"),
            format!(
                "plugin_api_version = \"{}\"\n[plugin]\nname = \"demo\"\nversion = \"1.0.0\"\n\
                 description = \"d\"\nlicense = \"MIT\"\n\
                 [permissions]\nregister_tools = true\ntool_namespace = \"Demo\"\n\
                 [runtime]\nkind = \"subprocess\"\n\
                 [runtime.subprocess]\nbinary_path = \"bin/run\"\n",
                wcore_plugin_api::PLUGIN_API_VERSION
            ),
        )
        .unwrap();
        dir.to_path_buf()
    }

    fn sign(dir: &Path, tmp: &Path) {
        let k = tmp.join("k.key");
        if !k.exists() {
            crate::plugin::sign::generate_key(&k).unwrap();
        }
        crate::plugin::sign::sign_dir(dir, &k).unwrap();
    }

    #[test]
    fn publishing_unsigned_material_is_refused() {
        let tmp = TempDir::new().unwrap();
        let p = tmp.path().join("p");
        signed_plugin(&p, b"x");
        let err = run(&p, &tmp.path().join("market"), "t".into()).unwrap_err();
        assert!(err.to_string().contains("unsigned"), "{err:?}");
    }

    #[test]
    fn published_bundle_verifies_and_a_mutated_byte_breaks_it() {
        let tmp = TempDir::new().unwrap();
        let p = tmp.path().join("p");
        signed_plugin(&p, b"entry");
        sign(&p, tmp.path());
        let market = tmp.path().join("market");
        run(&p, &market, "t".into()).unwrap();

        let published = plugin_dir_in(&market, "demo");
        let bundle = verify_bundle(&published).unwrap();
        assert_eq!(bundle.name, "demo");
        assert_eq!(bundle.signed_artifact, "bin/run");

        std::fs::write(published.join("bin").join("run"), b"entrY").unwrap();
        let err = verify_bundle(&published).unwrap_err();
        assert!(
            err.to_string().contains("integrity check FAILED"),
            "{err:?}"
        );
    }

    #[test]
    fn publish_writes_a_catalog_the_existing_marketplace_parser_accepts() {
        let tmp = TempDir::new().unwrap();
        let p = tmp.path().join("p");
        signed_plugin(&p, b"entry");
        sign(&p, tmp.path());
        let market = tmp.path().join("market");
        run(&p, &market, "t".into()).unwrap();

        let raw = std::fs::read_to_string(market.join(".claude-plugin").join("marketplace.json"))
            .unwrap();
        let (meta, entries) = crate::plugin::marketplace::parse_marketplace(&raw).unwrap();
        assert_eq!(meta.name, "market");
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].name, "demo");
    }

    /// Republishing a second version must replace the catalog entry, not
    /// accumulate a duplicate the resolver would then pick between at random.
    #[test]
    fn republishing_replaces_the_catalog_entry() {
        let tmp = TempDir::new().unwrap();
        let p = tmp.path().join("p");
        signed_plugin(&p, b"entry");
        sign(&p, tmp.path());
        let market = tmp.path().join("market");
        run(&p, &market, "t1".into()).unwrap();

        let toml = std::fs::read_to_string(p.join("plugin.toml")).unwrap();
        std::fs::write(
            p.join("plugin.toml"),
            toml.replace("version = \"1.0.0\"", "version = \"2.0.0\""),
        )
        .unwrap();
        run(&p, &market, "t2".into()).unwrap();

        let raw = std::fs::read_to_string(market.join(".claude-plugin").join("marketplace.json"))
            .unwrap();
        let (_, entries) = crate::plugin::marketplace::parse_marketplace(&raw).unwrap();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].declared_version.as_deref(), Some("2.0.0"));
    }
}
