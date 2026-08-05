// F25-04: `plugin recover` — repair a half-written plugin store.
//
// This verb REPAIRS; it does not describe. The damage it handles is the damage
// a killed process actually leaves: a staging directory from an interrupted
// write, a live pointer aimed at a generation that is no longer on disk, and an
// install directory that has drifted from (or vanished out from under) the
// generation the ledger says is live.
//
// Two things it must never do, both enforced here rather than merely intended:
// it never resurrects quarantined content, and it never restores an approval a
// human revoked. Recovery restores BYTES; it does not restore AUTHORITY. After
// a repair the restored plugin is re-evaluated against the approval gate and
// reported honestly — including the case where the repair leaves it refused.

use std::path::Path;

use wcore_config::plugin_governance as gov;

use crate::plugin::error::Result;
use crate::plugin::{approve, generations};

pub fn run(plugins_root: &Path) -> Result<()> {
    // Snapshot the approval state BEFORE repairing so the after-check is a real
    // comparison rather than a reading of whatever the repair produced.
    let before = gov::load_approvals(plugins_root)?;

    let report = generations::recover(plugins_root)?;

    if report.is_clean() {
        println!("plugin store is sound — nothing to repair");
    } else {
        println!("repaired {} item(s):", report.repairs.len());
        for line in &report.repairs {
            println!("  + {line}");
        }
    }

    let after = gov::load_approvals(plugins_root)?;
    // A recovery that granted an approval, or that erased a revocation, would
    // be a privilege escalation dressed as a repair. Assert it did neither.
    if after.approvals != before.approvals {
        println!(
            "REFUSING to accept this recovery: it altered the approval store \
             ({} → {} approvals). Approval is operator authority and recovery has \
             no business creating it.",
            before.approvals.len(),
            after.approvals.len()
        );
        return Err(crate::plugin::error::PluginCliError::Quarantine(
            "recovery mutated approval state".into(),
        ));
    }
    if after.revoked.len() < before.revoked.len() {
        return Err(crate::plugin::error::PluginCliError::Quarantine(
            "recovery dropped a revocation record".into(),
        ));
    }

    // Quarantine is containment for untrusted incoming content; recovery has no
    // reason to reach into it and this states that it did not.
    let q = plugins_root.join(".quarantine");
    println!(
        "quarantine    {}",
        if q.is_dir() {
            "left untouched (recovery never promotes quarantined content)"
        } else {
            "absent"
        }
    );

    if !report.unrepairable.is_empty() {
        println!("could NOT repair:");
        for line in &report.unrepairable {
            println!("  ! {line}");
        }
    }

    // Post-repair verdicts, so the operator learns now rather than at boot that
    // a restored plugin still needs approval.
    for dir in approve::plugin_dirs(plugins_root)? {
        let Ok(manifest) = crate::plugin::verify::load_manifest(&dir) else {
            continue;
        };
        let name = manifest.plugin.name;
        match gov::evaluate(plugins_root, &name, &dir) {
            gov::GateVerdict::NotGoverned => {}
            gov::GateVerdict::Approved { digest } => {
                println!("  {name}: approved at {}", gov::short(&digest))
            }
            gov::GateVerdict::Refused { .. } => {
                println!("  {name}: still REFUSED — run `wayland-core plugin approve {name}`")
            }
        }
    }

    if report.unrepairable.is_empty() {
        Ok(())
    } else {
        Err(crate::plugin::error::PluginCliError::Quarantine(format!(
            "{} item(s) could not be repaired",
            report.unrepairable.len()
        )))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn seed(root: &Path, name: &str, body: &str) -> std::path::PathBuf {
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

    /// Recovery restores bytes. It must NOT restore the authority to run them.
    #[test]
    fn recovery_does_not_resurrect_a_revoked_approval() {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path();
        let dir = seed(root, "demo", "1.0.0");
        let d1 = generations::retain(root, "demo", "1.0.0", &dir, &dir, "t1".into()).unwrap();
        generations::set_live(root, "demo", &d1).unwrap();
        approve::approve(root, "demo", "t1".into()).unwrap();
        approve::revoke(root, "demo", "t2".into()).unwrap();

        // Induce damage: delete the live install directory.
        std::fs::remove_dir_all(&dir).unwrap();
        run(root).unwrap();

        // Bytes came back...
        assert!(dir.is_dir());
        assert_eq!(gov::content_digest(&dir).unwrap(), d1);
        // ...authority did not.
        assert!(matches!(
            gov::evaluate(root, "demo", &dir),
            gov::GateVerdict::Refused { .. }
        ));
        assert!(approve::is_revoked(root, "demo", &d1).unwrap());
    }

    #[test]
    fn recovery_of_a_sound_store_repairs_nothing_and_succeeds() {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path();
        let dir = seed(root, "demo", "1.0.0");
        let d1 = generations::retain(root, "demo", "1.0.0", &dir, &dir, "t1".into()).unwrap();
        generations::set_live(root, "demo", &d1).unwrap();
        run(root).unwrap();
        assert!(generations::recover(root).unwrap().is_clean());
    }

    /// A live generation with no bytes anywhere cannot be repaired, and saying
    /// so is the correct answer — inventing a substitute would be worse.
    #[test]
    fn an_unrepairable_store_exits_non_zero() {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path();
        let dir = seed(root, "demo", "1.0.0");
        let d1 = generations::retain(root, "demo", "1.0.0", &dir, &dir, "t1".into()).unwrap();
        generations::set_live(root, "demo", &d1).unwrap();
        std::fs::remove_dir_all(generations::generation_dir(root, "demo", &d1)).unwrap();
        assert!(run(root).is_err());
    }
}
