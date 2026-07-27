// F25-04: `plugin inspect` — the operator's window onto one installed plugin.
//
// Every line here is READ BACK FROM DISK. The approval verdict in particular is
// computed by `wcore_config::plugin_governance::evaluate`, the same call the
// engine's loader makes, rather than by re-deriving "looks approved to me" from
// the store. An inspect that disagrees with the loader would be worse than no
// inspect at all: an operator would trust it and be wrong.
//
// Exits non-zero for a plugin that is not installed, so it is scriptable.

use std::path::Path;

use wcore_config::plugin_governance as gov;

use crate::plugin::error::Result;
use crate::plugin::{approve, generations, lockfile, publish, verify};

pub fn run(plugins_root: &Path, needle: &str) -> Result<()> {
    let found = approve::find_installed(plugins_root, needle)?;
    let report = verify::verify_dir(&found.dir)?;
    let verdict = gov::evaluate(plugins_root, &found.name, &found.dir);

    println!("name          {}", report.name);
    println!("version       {}", report.version);
    println!("path          {}", found.dir.display());
    println!("runtime       {}", report.runtime_kind);
    println!("digest        {}", report.content_digest);

    // Source + provenance, when the marketplace path installed it.
    let record = lockfile::read_lock(plugins_root)?
        .into_iter()
        .find(|r| r.plugin == report.name);
    match &record {
        Some(r) => {
            println!(
                "source        {} (marketplace '{}')",
                r.source, r.marketplace
            );
            println!(
                "resolved sha  {}",
                r.resolved_sha.as_deref().unwrap_or("(none)")
            );
            println!("installed at  {}", r.installed_at);
        }
        None => println!("source        (not recorded in installed.lock.json)"),
    }

    println!(
        "api version   declared={} host={} → {}",
        report.declared_api_version.as_deref().unwrap_or("(absent)"),
        report.host_api_version,
        if report.api_compatible {
            "COMPATIBLE"
        } else {
            "INCOMPATIBLE"
        }
    );
    println!(
        "signature     {}",
        if report.signature_present {
            "present (wayland-plugin.sig)"
        } else {
            "ABSENT"
        }
    );

    // Bundle integrity, when this came from a published bundle.
    if found.dir.join(publish::BUNDLE_FILE).is_file() {
        match publish::verify_bundle(&found.dir) {
            Ok(b) => println!(
                "bundle        intact (published {}, signs {})",
                b.published_at, b.signed_artifact
            ),
            Err(e) => println!("bundle        TAMPERED — {e}"),
        }
    }

    println!(
        "approval      {}",
        match &verdict {
            gov::GateVerdict::NotGoverned =>
                "not governed (this plugins root predates the approval gate)".to_string(),
            gov::GateVerdict::Approved { digest } => format!("APPROVED at {}", gov::short(digest)),
            gov::GateVerdict::Refused { reason } => format!("REFUSED — {reason}"),
        }
    );
    println!(
        "loads         {}",
        match &verdict {
            gov::GateVerdict::Refused { .. } => "NO — the loader will refuse this plugin",
            _ => "yes — the approval gate admits this plugin",
        }
    );

    match generations::get(plugins_root, &found.name)? {
        Some(g) => {
            println!(
                "live gen      {}",
                g.live
                    .as_deref()
                    .map(gov::short)
                    .unwrap_or_else(|| "(none)".into())
            );
            println!("retained      {} generation(s)", g.retained.len());
            for rec in &g.retained {
                let marker = if Some(&rec.digest) == g.live.as_ref() {
                    "* live"
                } else {
                    "       "
                };
                println!(
                    "  {marker} {} v{} created {}",
                    gov::short(&rec.digest),
                    rec.version,
                    rec.created_at
                );
            }
        }
        None => println!("live gen      (no generation history — installed before the lifecycle)"),
    }

    println!("permissions:");
    for line in &report.permissions {
        println!("  - {line}");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn install(root: &Path, name: &str) -> std::path::PathBuf {
        let d = root.join(format!("{name}@local"));
        std::fs::create_dir_all(&d).unwrap();
        std::fs::write(
            d.join("plugin.toml"),
            format!(
                "plugin_api_version = \"{}\"\n[plugin]\nname = \"{name}\"\nversion = \"1.0.0\"\n\
                 description = \"d\"\nlicense = \"MIT\"\n[permissions]\nregister_hooks = true\n\
                 [runtime]\nkind = \"declarative\"\n",
                wcore_plugin_api::PLUGIN_API_VERSION
            ),
        )
        .unwrap();
        d
    }

    #[test]
    fn inspect_of_an_unknown_plugin_is_an_error_not_a_blank_report() {
        let tmp = TempDir::new().unwrap();
        assert!(run(tmp.path(), "nope").is_err());
    }

    #[test]
    fn inspect_reports_an_installed_plugin() {
        let tmp = TempDir::new().unwrap();
        install(tmp.path(), "demo");
        run(tmp.path(), "demo").unwrap();
    }

    /// Inspect's verdict must be the loader's verdict, not an opinion.
    #[test]
    fn inspect_agrees_with_the_gate_before_and_after_approval() {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path();
        let dir = install(root, "demo");
        approve::ensure_governed(root).unwrap();
        assert!(matches!(
            gov::evaluate(root, "demo", &dir),
            gov::GateVerdict::Refused { .. }
        ));
        run(root, "demo").unwrap();
        approve::approve(root, "demo", "t0".into()).unwrap();
        assert!(matches!(
            gov::evaluate(root, "demo", &dir),
            gov::GateVerdict::Approved { .. }
        ));
        run(root, "demo").unwrap();
    }
}
