// F25-04: the governed install / update / rollback path.
//
// `update` is plan-then-commit over retained generations, NOT remove-then-
// install. Remove-then-install destroys the prior bytes, which are precisely
// what `rollback` has to restore — a rollback verb built on top of it would
// print success and restore nothing. So: resolve and stage first, retain the
// new generation, and only then swap the live directory. An update interrupted
// before that final swap leaves the previous generation live and intact.

use std::path::{Path, PathBuf};

use wcore_config::plugin_governance as gov;

use crate::plugin::error::{PluginCliError, Result};
use crate::plugin::{approve, generations, lockfile, marketplace, publish, verify};

/// Commit a written install directory into lifecycle governance: retain it as a
/// generation and point live at it. Returns the install directory and the
/// digest the approval will be bound to.
///
/// The bundle integrity check deliberately does NOT live here. It runs against
/// the PRISTINE acquired tree before anything is copied into the store, so a
/// tampered bundle never lands at all; checking again after installation would
/// also be checking install-time sidecars the publisher never signed.
pub fn adopt_install(
    plugins_root: &Path,
    plugin: &str,
    version: &str,
    install_dir: &Path,
    now: String,
) -> Result<(PathBuf, String)> {
    approve::ensure_governed(plugins_root)?;
    let digest = generations::retain(plugins_root, plugin, version, install_dir, install_dir, now)?;
    generations::set_live(plugins_root, plugin, &digest)?;
    Ok((install_dir.to_path_buf(), digest))
}

/// Is this acquired source tree a WAYLAND-NATIVE plugin rather than a foreign
/// one that has to be lowered?
///
/// Native means it already carries the manifest the runtime reads. Detecting
/// this matters because the lowering pipeline GENERATES a `plugin.toml` from a
/// canonical draft — correct for a Claude Code plugin, and destructive for a
/// native one: it would discard the author's manifest, the entry artifact and
/// the detached signature, so the plugin would arrive with its signature void
/// and its runtime section gone. A signed, digest-addressed bundle has to be
/// installed BYTE FOR BYTE or it is not the thing that was signed.
pub fn is_native_bundle(root: &Path) -> bool {
    root.join("plugin.toml").is_file()
}

/// Install a Wayland-native plugin verbatim.
///
/// This is the branch of `plugin install` that the marketplace lowering path
/// cannot serve. It performs the same containment the lowering path does — the
/// source has already been acquired into the quarantine by
/// `marketplace::resolve_source` — then verifies, copies verbatim, records
/// provenance and the lockfile entry, and adopts the result into the
/// generation ledger.
pub fn native_install(
    plugins_root: &Path,
    market: &str,
    plugin: &str,
    source: &marketplace::ResolvedSource,
    now: String,
) -> Result<(PathBuf, String)> {
    // Verify BEFORE anything is written into the store. A bundle whose bytes
    // changed after publication never reaches the install root at all.
    if source.fetched_root.join(publish::BUNDLE_FILE).is_file() {
        let bundle = publish::verify_bundle(&source.fetched_root)?;
        println!(
            "bundle integrity OK ({} {} @ {})",
            bundle.name,
            bundle.version,
            gov::short(&bundle.content_digest)
        );
    } else {
        println!(
            "note: {plugin}@{market} carries no {} sidecar, so there is no published \
             digest to check these bytes against",
            publish::BUNDLE_FILE
        );
    }
    let report = verify::verify_dir(&source.fetched_root)?;
    if report.is_fatal() {
        return Err(PluginCliError::Quarantine(format!(
            "refusing to install {plugin}@{market}: it does not verify ({})",
            report.api_detail
        )));
    }
    if report.name != plugin {
        return Err(PluginCliError::Quarantine(format!(
            "catalog lists '{plugin}' but its manifest declares '{}' — refusing an \
             install whose identity does not match its listing",
            report.name
        )));
    }

    let dest = plugins_root.join(format!("{plugin}@{market}"));
    generations::swap_in(&source.fetched_root, &dest)?;

    // The provenance sidecar is what `plugin list` and `remove` locate an
    // install by, so a native install has to write one exactly as the lowering
    // path does — otherwise the plugin installs and then cannot be listed or
    // removed.
    let provenance = serde_json::json!({
        "marketplace": market,
        "plugin": plugin,
        "namespace": plugin,
        "version": report.version,
        "grade": "Native",
        "format": "wayland-native",
        "resolved_sha": source.resolved_sha,
    });
    std::fs::write(
        dest.join("provenance.json"),
        serde_json::to_vec_pretty(&provenance)?,
    )?;

    lockfile::record_install(
        plugins_root,
        lockfile::InstallRecord {
            plugin: plugin.to_string(),
            marketplace: market.to_string(),
            source: quarantine_describe(source),
            resolved_sha: source.resolved_sha.clone(),
            version: report.version.clone(),
            grade: "Native".to_string(),
            installed_at: now.clone(),
        },
    )?;

    adopt_install(plugins_root, plugin, &report.version, &dest, now)
}

fn quarantine_describe(source: &marketplace::ResolvedSource) -> String {
    crate::plugin::quarantine::describe_source(&source.entry.kind)
}

/// `plugin update <name>` — re-resolve from the recorded marketplace, retain
/// the incoming generation, then swap it live.
pub fn run_update(plugins_root: &Path, needle: &str, now: String) -> Result<()> {
    let found = approve::find_installed(plugins_root, needle)?;
    let record = lockfile::read_lock(plugins_root)?
        .into_iter()
        .find(|r| r.plugin == found.name)
        .ok_or_else(|| {
            PluginCliError::Quarantine(format!(
                "{} has no marketplace record in installed.lock.json, so there is \
                 nothing to update it FROM. Reinstall it through \
                 `plugin install <name>@<marketplace>` first.",
                found.name
            ))
        })?;

    let before_digest = gov::content_digest(&found.dir)?;
    println!(
        "updating {} from marketplace '{}' (current digest {})",
        found.name,
        record.marketplace,
        gov::short(&before_digest)
    );

    // PLAN: acquire into the quarantine. Nothing under the plugins root has
    // been touched at this point, so a failure here is a no-op update — the
    // previous generation is still live and intact.
    let quarantine_root = plugins_root.join(".quarantine");
    let source = marketplace::resolve_source(
        plugins_root,
        &quarantine_root,
        &record.marketplace,
        &found.name,
    )?;

    // COMMIT: write the new tree, retain it, then point live at it. The prior
    // generation was retained at ITS install, so it survives all of this.
    let after_digest = if is_native_bundle(&source.fetched_root) {
        native_install(plugins_root, &record.marketplace, &found.name, &source, now)?.1
    } else {
        let planned = marketplace::resolve_and_plan(
            plugins_root,
            &quarantine_root,
            &record.marketplace,
            &found.name,
        )?;
        println!("{}", planned.plan.render());
        let dir = marketplace::commit_install(plugins_root, &planned, now.clone())?;
        let version = manifest_version(&dir);
        adopt_install(plugins_root, &found.name, &version, &dir, now)?.1
    };

    if after_digest == before_digest {
        println!(
            "{} is already at digest {} — nothing changed",
            found.name,
            gov::short(&after_digest)
        );
        return Ok(());
    }
    println!(
        "updated {} {} → {}",
        found.name,
        gov::short(&before_digest),
        gov::short(&after_digest)
    );
    // Approval is bound to the digest, so it did not survive this. Say so
    // rather than letting the operator discover it at the next session boot.
    println!(
        "NOTE: the approval for the previous digest does not carry over. \
         Run `wayland-core plugin approve {}` to admit the new bytes.",
        found.name
    );
    Ok(())
}

/// `plugin rollback <name>` — restore the exact prior generation.
///
/// The restore is proved by recomputing the installed directory's digest and
/// comparing it with the retained generation's, not by trusting the copy.
pub fn run_rollback(plugins_root: &Path, needle: &str) -> Result<()> {
    let found = approve::find_installed(plugins_root, needle)?;
    let prior = generations::prior_generation(plugins_root, &found.name)?.ok_or_else(|| {
        PluginCliError::Quarantine(format!(
            "{} has no retained generation before the live one — there is nothing to \
             roll back to",
            found.name
        ))
    })?;
    let before = gov::content_digest(&found.dir)?;

    generations::restore(plugins_root, &found.name, &prior.digest, &found.dir)?;
    generations::set_live(plugins_root, &found.name, &prior.digest)?;

    let after = gov::content_digest(&found.dir)?;
    if after != prior.digest {
        return Err(PluginCliError::Quarantine(format!(
            "rollback of {} did NOT restore the retained bytes: expected {}, got {}",
            found.name, prior.digest, after
        )));
    }
    println!(
        "rolled back {} {} → {} (v{})",
        found.name,
        gov::short(&before),
        gov::short(&after),
        prior.version
    );
    println!("  restored digest equals retained generation digest: {after}");
    match gov::evaluate(plugins_root, &found.name, &found.dir) {
        gov::GateVerdict::Approved { .. } => {
            println!("  the restored bytes are already approved and will load")
        }
        gov::GateVerdict::NotGoverned => {}
        gov::GateVerdict::Refused { .. } => println!(
            "  the restored bytes are NOT approved — run `wayland-core plugin approve {}`",
            found.name
        ),
    }
    Ok(())
}

/// Drop a removed plugin's lifecycle state.
///
/// Generations and the live pointer go, because the bytes they describe are
/// gone. The APPROVAL goes too — an approval is consent to run a specific set
/// of bytes, and leaving it behind would silently pre-approve whatever a later
/// reinstall put at the same digest. The REVOCATION list is deliberately kept:
/// a withdrawn authority must not be recoverable by uninstalling and
/// reinstalling.
pub fn forget(plugins_root: &Path, plugin: &str) -> Result<()> {
    let mut ledger = generations::load(plugins_root)?;
    if let Some(entry) = ledger.plugins.remove(plugin) {
        for rec in &entry.retained {
            let dir = generations::generation_dir(plugins_root, plugin, &rec.digest);
            if dir.is_dir() {
                std::fs::remove_dir_all(&dir)?;
            }
        }
        let root = generations::generations_root(plugins_root, plugin);
        if root.is_dir() {
            std::fs::remove_dir_all(&root).ok();
        }
        generations::store(plugins_root, &ledger)?;
    }
    let mut store = gov::load_approvals(plugins_root)?;
    if store.approvals.remove(plugin).is_some() {
        gov::store_approvals(plugins_root, &store)?;
    }
    Ok(())
}

/// Version string for the ledger, taken from the plugin's own manifest so the
/// generation record says what the plugin says about itself.
pub fn manifest_version(dir: &Path) -> String {
    verify::load_manifest(dir)
        .map(|m| m.plugin.version)
        .unwrap_or_else(|_| "unknown".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn seed(root: &Path, name: &str, body: &str) -> PathBuf {
        let d = root.join(format!("{name}@local"));
        std::fs::create_dir_all(&d).unwrap();
        std::fs::write(
            d.join("plugin.toml"),
            format!(
                "plugin_api_version = \"{}\"\n[plugin]\nname = \"{name}\"\nversion = \"{body}\"\n\
                 description = \"d\"\nlicense = \"MIT\"\n[permissions]\nregister_hooks = true\n\
                 [runtime]\nkind = \"declarative\"\n",
                wcore_plugin_api::PLUGIN_API_VERSION
            ),
        )
        .unwrap();
        d
    }

    #[test]
    fn rollback_restores_byte_identical_prior_content() {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path();
        let dir = seed(root, "demo", "1.0.0");
        let (_, d1) = adopt_install(root, "demo", "1.0.0", &dir, "t1".into()).unwrap();
        let v1_bytes = std::fs::read(dir.join("plugin.toml")).unwrap();

        seed(root, "demo", "2.0.0");
        let (_, d2) = adopt_install(root, "demo", "2.0.0", &dir, "t2".into()).unwrap();
        assert_ne!(d1, d2);

        run_rollback(root, "demo").unwrap();
        assert_eq!(std::fs::read(dir.join("plugin.toml")).unwrap(), v1_bytes);
        assert_eq!(gov::content_digest(&dir).unwrap(), d1);
    }

    #[test]
    fn rollback_with_no_prior_generation_is_an_error_not_a_no_op_success() {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path();
        let dir = seed(root, "demo", "1.0.0");
        adopt_install(root, "demo", "1.0.0", &dir, "t1".into()).unwrap();
        let err = run_rollback(root, "demo").unwrap_err();
        assert!(err.to_string().contains("nothing to roll back"), "{err:?}");
    }

    /// The security property that makes `update` safe: consent does not travel
    /// with the plugin name across a change of bytes.
    #[test]
    fn an_update_invalidates_the_prior_approval() {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path();
        let dir = seed(root, "demo", "1.0.0");
        adopt_install(root, "demo", "1.0.0", &dir, "t1".into()).unwrap();
        approve::approve(root, "demo", "t1".into()).unwrap();
        assert!(matches!(
            gov::evaluate(root, "demo", &dir),
            gov::GateVerdict::Approved { .. }
        ));

        seed(root, "demo", "2.0.0");
        adopt_install(root, "demo", "2.0.0", &dir, "t2".into()).unwrap();
        assert!(matches!(
            gov::evaluate(root, "demo", &dir),
            gov::GateVerdict::Refused { .. }
        ));
    }

    /// An interrupted update leaves the PREVIOUS generation live: retention
    /// happens before the live pointer moves, so a crash between the two loses
    /// nothing.
    #[test]
    fn a_retained_generation_survives_an_update_that_never_commits() {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path();
        let dir = seed(root, "demo", "1.0.0");
        let (_, d1) = adopt_install(root, "demo", "1.0.0", &dir, "t1".into()).unwrap();

        // Simulate the crash: new bytes are staged into a generation, but the
        // process dies before `set_live`.
        seed(root, "demo", "2.0.0");
        let d2 = generations::retain(root, "demo", "2.0.0", &dir, &dir, "t2".into()).unwrap();
        assert_ne!(d1, d2);
        assert_eq!(
            generations::get(root, "demo")
                .unwrap()
                .unwrap()
                .live
                .unwrap(),
            d1,
            "the live pointer must still name the previous generation"
        );

        // And recovery puts the install directory back to that live generation.
        let report = generations::recover(root).unwrap();
        assert!(report.unrepairable.is_empty(), "{report:?}");
        assert_eq!(gov::content_digest(&dir).unwrap(), d1);
    }
}
