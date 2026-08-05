// F25-04: `plugin verify` — the compatibility half of the lifecycle, plus the
// manifest/entry-artifact helpers `sign` and `publish` share.
//
// The verdict is a PROCESS EXIT CODE, not a printed warning. An operator who
// cannot get a non-zero exit out of a compatibility check cannot script a gate
// around it, and an unscriptable check is one nobody runs twice.

use std::path::{Path, PathBuf};

use wcore_plugin_api::PluginManifest;

use crate::plugin::error::{PluginCliError, Result};

/// Read and parse `<dir>/plugin.toml`.
pub fn load_manifest(dir: &Path) -> Result<PluginManifest> {
    let path = manifest_path(dir);
    let raw = std::fs::read_to_string(&path).map_err(|e| {
        PluginCliError::Quarantine(format!("no plugin manifest at {}: {e}", path.display()))
    })?;
    PluginManifest::from_toml_str(&raw)
        .map_err(|e| PluginCliError::Quarantine(format!("{}: {e}", path.display())))
}

pub fn manifest_path(dir: &Path) -> PathBuf {
    dir.join("plugin.toml")
}

/// Resolve the manifest-declared entry artifact — the exact bytes the engine's
/// `sig_verifier` checks a detached signature against.
///
/// `Ok(None)` means the manifest declares no entry artifact (a declarative or
/// static plugin). That is a legitimate manifest, not an error; it is the
/// CALLER that decides whether the absence is fatal (`sign` says yes).
///
/// The declared path is resolved under `dir` with the same containment the
/// loader applies: no absolute paths, no `..`. A manifest that could point
/// signing at an arbitrary file on disk would let an author sign one artifact
/// and ship another.
pub fn entry_artifact(dir: &Path, manifest: &PluginManifest) -> Result<Option<PathBuf>> {
    let Some(runtime) = manifest.runtime.as_ref() else {
        return Ok(None);
    };
    let declared = if runtime.kind.eq_ignore_ascii_case("declarative") {
        None
    } else if runtime.kind.eq_ignore_ascii_case("wasm") {
        Some(
            runtime
                .wasm
                .as_ref()
                .and_then(|w| w.component_path.clone())
                .unwrap_or_else(|| "plugin.wasm".to_string()),
        )
    } else {
        runtime
            .subprocess
            .as_ref()
            .and_then(|s| s.binary_path.clone())
    };
    let Some(rel) = declared else {
        return Ok(None);
    };
    Ok(Some(resolve_within(dir, &rel)?))
}

/// Join `rel` under `base`, refusing anything that escapes it.
pub fn resolve_within(base: &Path, rel: &str) -> Result<PathBuf> {
    use std::path::Component;
    let p = Path::new(rel);
    if p.is_absolute() {
        return Err(PluginCliError::PathTraversal(rel.to_string()));
    }
    let bad = p.components().any(|c| {
        matches!(
            c,
            Component::ParentDir | Component::RootDir | Component::Prefix(_)
        )
    });
    if bad {
        return Err(PluginCliError::PathTraversal(rel.to_string()));
    }
    Ok(base.join(p))
}

/// The outcome of a verification pass. Kept as data so the same computation
/// backs `plugin verify` and the compatibility line in `plugin inspect`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VerifyReport {
    pub name: String,
    pub version: String,
    pub declared_api_version: Option<String>,
    pub host_api_version: String,
    pub api_compatible: bool,
    pub api_detail: String,
    pub runtime_kind: String,
    pub entry_artifact: Option<String>,
    pub entry_artifact_present: bool,
    pub signature_present: bool,
    pub permissions: Vec<String>,
    pub content_digest: String,
}

impl VerifyReport {
    /// Fatal means a non-zero exit.
    pub fn is_fatal(&self) -> bool {
        !self.api_compatible || (self.entry_artifact.is_some() && !self.entry_artifact_present)
    }
}

pub fn verify_dir(dir: &Path) -> Result<VerifyReport> {
    let manifest = load_manifest(dir)?;
    let host = wcore_plugin_api::PLUGIN_API_VERSION.to_string();
    let (api_compatible, api_detail) = match manifest.require_api_version(&host) {
        Ok(()) => (true, format!("declared api version matches host {host}")),
        Err(e) => (false, e.to_string()),
    };
    let entry = entry_artifact(dir, &manifest)?;
    let entry_present = entry.as_ref().map(|p| p.is_file()).unwrap_or(false);

    Ok(VerifyReport {
        name: manifest.plugin.name.clone(),
        version: manifest.plugin.version.clone(),
        declared_api_version: manifest.plugin_api_version.clone(),
        host_api_version: host,
        api_compatible,
        api_detail,
        runtime_kind: manifest
            .runtime
            .as_ref()
            .map(|r| r.kind.clone())
            .unwrap_or_else(|| "static".to_string()),
        entry_artifact: entry
            .as_ref()
            .map(|p| p.strip_prefix(dir).unwrap_or(p).display().to_string()),
        entry_artifact_present: entry_present,
        // Beside the entry artifact — the ONLY place the engine's verifier
        // looks. Reporting a root-level signature as present would tell an
        // author their plugin is signed when the loader will refuse it.
        signature_present: entry
            .as_deref()
            .map(|e| crate::plugin::sign::signature_path_for(e).is_file())
            .unwrap_or(false),
        permissions: describe_permissions(&manifest),
        content_digest: wcore_config::plugin_governance::content_digest(dir)?,
    })
}

/// Declared permissions in plain language. An operator approving a plugin is
/// approving THESE, so they are spelled out rather than dumped as a struct.
pub fn describe_permissions(m: &PluginManifest) -> Vec<String> {
    let p = &m.permissions;
    let mut out = Vec::new();
    let flags: [(bool, &str); 8] = [
        (p.register_tools, "register tools the model can call"),
        (p.register_hooks, "run hooks around the agent lifecycle"),
        (p.register_providers, "register LLM providers"),
        (p.register_agents, "register sub-agents"),
        (p.register_skills, "register skills"),
        (p.register_rules, "register rules"),
        (p.register_mcp_server, "declare an MCP server"),
        (p.register_user_models, "register user-visible models"),
    ];
    for (on, text) in flags {
        if on {
            out.push(text.to_string());
        }
    }
    if let Some(ns) = &p.tool_namespace {
        out.push(format!("claim the tool namespace '{ns}'"));
    }
    for part in &p.memory_partitions_writable {
        out.push(format!("WRITE memory partition '{part}'"));
    }
    for part in &p.memory_partitions_readable {
        out.push(format!("read memory partition '{part}'"));
    }
    let c = &m.capabilities;
    for cap in &c.required {
        out.push(format!("require host capability '{cap}'"));
    }
    if out.is_empty() {
        out.push("no privileged surfaces declared".to_string());
    }
    out
}

/// `plugin verify <dir>` — print the report; non-zero exit when fatal.
pub fn run(dir: &Path) -> Result<()> {
    let r = verify_dir(dir)?;
    println!("plugin        {} {}", r.name, r.version);
    println!("runtime       {}", r.runtime_kind);
    println!("digest        {}", r.content_digest);
    println!(
        "api version   declared={} host={} → {}",
        r.declared_api_version.as_deref().unwrap_or("(absent)"),
        r.host_api_version,
        if r.api_compatible {
            "COMPATIBLE"
        } else {
            "INCOMPATIBLE"
        }
    );
    if !r.api_compatible {
        println!("              {}", r.api_detail);
    }
    match (&r.entry_artifact, r.entry_artifact_present) {
        (Some(p), true) => println!("entry         {p} (present)"),
        (Some(p), false) => println!("entry         {p} (MISSING)"),
        (None, _) => println!("entry         (none declared — no binary to verify or sign)"),
    }
    println!(
        "signature     {}",
        if r.signature_present {
            wcore_agent::plugins::sig_verifier::PLUGIN_SIG_FILENAME
        } else {
            "(unsigned)"
        }
    );
    println!("permissions:");
    for line in &r.permissions {
        println!("  - {line}");
    }

    if r.is_fatal() {
        return Err(PluginCliError::Quarantine(format!(
            "verification failed for {}: {}",
            r.name,
            if !r.api_compatible {
                r.api_detail
            } else {
                format!(
                    "declared entry artifact {} is missing",
                    r.entry_artifact.unwrap_or_default()
                )
            }
        )));
    }
    println!("VERIFIED");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn write_plugin(dir: &Path, api: &str, extra: &str) {
        std::fs::create_dir_all(dir).unwrap();
        std::fs::write(
            dir.join("plugin.toml"),
            format!(
                "plugin_api_version = \"{api}\"\n\
                 [plugin]\nname = \"demo\"\nversion = \"1.0.0\"\n\
                 description = \"d\"\nlicense = \"MIT\"\n\
                 [permissions]\nregister_hooks = true\n{extra}"
            ),
        )
        .unwrap();
    }

    #[test]
    fn incompatible_api_version_is_fatal() {
        let tmp = TempDir::new().unwrap();
        write_plugin(tmp.path(), "0.0.1-nope", "");
        let r = verify_dir(tmp.path()).unwrap();
        assert!(!r.api_compatible);
        assert!(r.is_fatal());
        assert!(run(tmp.path()).is_err());
    }

    #[test]
    fn matching_api_version_verifies() {
        let tmp = TempDir::new().unwrap();
        write_plugin(tmp.path(), wcore_plugin_api::PLUGIN_API_VERSION, "");
        let r = verify_dir(tmp.path()).unwrap();
        assert!(r.api_compatible, "{}", r.api_detail);
        assert!(!r.is_fatal());
        assert!(r.permissions.iter().any(|p| p.contains("hooks")));
    }

    #[test]
    fn a_declared_but_missing_entry_artifact_is_fatal() {
        let tmp = TempDir::new().unwrap();
        write_plugin(
            tmp.path(),
            wcore_plugin_api::PLUGIN_API_VERSION,
            "\n[runtime]\nkind = \"subprocess\"\n[runtime.subprocess]\nbinary_path = \"bin/run\"\n",
        );
        let r = verify_dir(tmp.path()).unwrap();
        assert!(r.entry_artifact.is_some());
        assert!(!r.entry_artifact_present);
        assert!(r.is_fatal());
    }

    #[test]
    fn a_traversing_entry_path_is_refused_outright() {
        let tmp = TempDir::new().unwrap();
        write_plugin(
            tmp.path(),
            wcore_plugin_api::PLUGIN_API_VERSION,
            "\n[runtime]\nkind = \"subprocess\"\n[runtime.subprocess]\nbinary_path = \"../../etc/passwd\"\n",
        );
        let err = verify_dir(tmp.path()).unwrap_err();
        assert!(matches!(err, PluginCliError::PathTraversal(_)), "{err:?}");
    }
}
