//! Readiness gates: can this bridge actually run, and is it paired.
//!
//! Every gate here is answered from the filesystem — nothing is spawned and
//! nothing is sent. Split out of `bridge.rs` to keep each module inside the
//! project's 1000-line limit; the transport lives in [`super::rpc`] and the
//! adapter in [`super`].

use std::path::{Path, PathBuf};

use super::{WhatsappBackend, WhatsappBridgeConfig};

// ---------------------------------------------------------------------------

/// Why the bridge cannot be launched, in a form an operator can act on.
///
/// Every finding names an ITEM, never a value — this feeds
/// [`ProbeReport::findings`], which is contractually secret-free.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{operator_message}")]
pub struct BridgeUnavailable {
    /// Names of the missing items: `"node_runtime"`, `"bridge_path"`, `"backend"`.
    pub findings: Vec<String>,
    /// One actionable message naming exactly what is missing and how to supply it.
    pub operator_message: String,
}

/// A launch the filesystem says is actually possible.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BridgeLaunch {
    /// Resolved Node interpreter.
    pub node: PathBuf,
    /// Resolved bridge entrypoint.
    pub script: PathBuf,
    /// Backend to request via `--backend`.
    pub backend: WhatsappBackend,
    /// Session directory to request via `--session`, when configured.
    pub session_dir: Option<PathBuf>,
}

/// Resolve a Node interpreter without running anything.
///
/// Platform difference is centralised here (AGENTS.md): Windows needs the
/// `PATHEXT` extensions appended, Unix does not.
fn resolve_node(explicit: Option<&Path>) -> Option<PathBuf> {
    if let Some(p) = explicit {
        return p.is_file().then(|| p.to_path_buf());
    }

    let extensions: Vec<String> = if cfg!(windows) {
        std::env::var("PATHEXT")
            .unwrap_or_else(|_| ".COM;.EXE;.BAT;.CMD".to_string())
            .split(';')
            .filter(|e| !e.is_empty())
            .map(str::to_string)
            .collect()
    } else {
        vec![String::new()]
    };

    let path = std::env::var_os("PATH")?;
    for dir in std::env::split_paths(&path) {
        for ext in &extensions {
            let candidate = dir.join(format!("node{ext}"));
            if candidate.is_file() {
                return Some(candidate);
            }
        }
    }
    None
}

/// Whether the backend's npm package is resolvable from the bridge's directory.
///
/// # Why this check exists
///
/// Driving the real `bridge.js` with no `node_modules` present shows that
/// `health` **still succeeds** — it is answered before any backend is loaded —
/// while the very next `connect` fails with
/// `-32000 Failed to load backend baileys: Cannot find module …`. A readiness
/// verdict taken from the handshake alone would therefore report a green for a
/// bridge that cannot send a single message. This is that gap closed.
///
/// Node resolves `node_modules` by walking up from the importing file, so this
/// walks the same ancestors rather than checking only the sibling directory —
/// a hoisted install must not read as missing.
fn backend_package_installed(bridge_path: &Path, backend: WhatsappBackend) -> bool {
    let Some(pkg) = backend.npm_package() else {
        return true;
    };
    let Some(dir) = bridge_path.parent() else {
        return false;
    };
    dir.ancestors()
        .any(|a| a.join("node_modules").join(pkg).is_dir())
}

/// Resolve the directory the bridge will keep pairing material in.
///
/// Mirrors the bridge's own default (`$HOME/.wayland/whatsapp`) so the answer
/// is the same one the subprocess will compute. Measured from
/// `backends/baileys.js` and `backends/whatsapp-web.js`.
pub(super) fn pairing_dir(cfg: &WhatsappBridgeConfig) -> Option<PathBuf> {
    let (subdir, marker) = cfg.backend.pairing_marker()?;
    let root = match cfg.session_dir.as_ref() {
        Some(d) => d.clone(),
        None => dirs_home()?.join(".wayland").join("whatsapp"),
    };
    Some(root.join(subdir).join(marker))
}

/// Home directory, without taking a dependency for one lookup.
fn dirs_home() -> Option<PathBuf> {
    std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .map(PathBuf::from)
        .filter(|p| !p.as_os_str().is_empty())
}

/// Decide whether the bridge can be launched, from the filesystem alone.
///
/// Spawns nothing and sends nothing. Every missing item is collected, so an
/// operator who is missing two things is told about both rather than
/// discovering the second after fixing the first.
pub fn preflight(cfg: &WhatsappBridgeConfig) -> Result<BridgeLaunch, BridgeUnavailable> {
    let mut findings = Vec::new();
    let mut lines = Vec::new();

    if !cfg.backend.is_bridged() {
        findings.push("backend".to_string());
        lines.push(format!(
            "backend {:?} is not driven through the bridge — use the Cloud API adapter for it, \
             or set backend to one of: {}",
            cfg.backend.wire_name(),
            ["baileys", "whatsapp-web"].join(", ")
        ));
    }

    let node = resolve_node(cfg.node_path.as_deref());
    if node.is_none() {
        findings.push("node_runtime".to_string());
        lines.push(match cfg.node_path.as_deref() {
            Some(p) => format!(
                "node_path points at {} which is not a file — install Node 18+ or correct node_path",
                p.display()
            ),
            None => "no `node` on PATH — the bridge is a Node program; install Node 18+ or set \
                     node_path in the channel config"
                .to_string(),
        });
    }

    if !cfg.bridge_path.is_file() {
        findings.push("bridge_path".to_string());
        lines.push(format!(
            "bridge_path {} is not a file — wayland-core does not ship the bridge; point \
             bridge_path at the bridge.js of a Wayland Desktop install (or a checkout of it) \
             whose dependencies are installed",
            cfg.bridge_path.display()
        ));
    } else if !backend_package_installed(&cfg.bridge_path, cfg.backend) {
        // Only meaningful when the script exists — otherwise this would just
        // restate the finding above.
        findings.push("bridge_dependencies".to_string());
        lines.push(format!(
            "the bridge at {} has no resolvable {} — its `health` will still answer, but the \
             first connect fails with `Cannot find module`; run `npm install` (or `bun install`) \
             in the bridge's directory",
            cfg.bridge_path.display(),
            cfg.backend.npm_package().unwrap_or("backend package"),
        ));
    }

    if findings.is_empty() {
        // Both unwraps are guarded by the checks above.
        return Ok(BridgeLaunch {
            node: node.expect("node resolved: findings is empty"),
            script: cfg.bridge_path.clone(),
            backend: cfg.backend,
            session_dir: cfg.session_dir.clone(),
        });
    }

    Err(BridgeUnavailable {
        findings,
        operator_message: format!(
            "whatsapp backend {} is not available: {}",
            cfg.backend.wire_name(),
            lines.join("; ")
        ),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bridge::testing::{cfg, installed_bridge};

    // -- preflight: both directions ----------------------------------------

    #[test]
    fn preflight_fails_closed_naming_a_missing_bridge_script() {
        let c = cfg(
            WhatsappBackend::Baileys,
            PathBuf::from("/definitely/not/here/bridge.js"),
        );
        let err = preflight(&c).unwrap_err();
        assert!(
            err.findings.contains(&"bridge_path".to_string()),
            "findings must NAME the missing item, got {:?}",
            err.findings
        );
        assert!(
            err.operator_message.contains("does not ship the bridge"),
            "the message must tell the operator what to do: {}",
            err.operator_message
        );
    }

    #[test]
    fn preflight_fails_closed_naming_a_missing_node_runtime() {
        // node_path pointing at a nonexistent file is the deterministic way to
        // express "no Node" — emptying PATH would be process-global and would
        // race every other test in this binary. The bridge is a fully INSTALLED
        // one so that `node_runtime` is genuinely the only thing missing.
        let (_dir, script) = installed_bridge(WhatsappBackend::Baileys);
        let mut c = cfg(WhatsappBackend::Baileys, script);
        c.node_path = Some(PathBuf::from("/definitely/not/here/node"));

        let err = preflight(&c).unwrap_err();
        assert_eq!(
            err.findings,
            vec!["node_runtime".to_string()],
            "only Node is missing — the script and its dependencies exist"
        );
        assert!(err.operator_message.contains("install Node"));
    }

    #[test]
    fn preflight_names_every_missing_item_at_once() {
        let mut c = cfg(
            WhatsappBackend::Baileys,
            PathBuf::from("/definitely/not/here/bridge.js"),
        );
        c.node_path = Some(PathBuf::from("/definitely/not/here/node"));
        let err = preflight(&c).unwrap_err();
        assert!(err.findings.contains(&"node_runtime".to_string()));
        assert!(err.findings.contains(&"bridge_path".to_string()));
        assert_eq!(err.findings.len(), 2);
    }

    #[test]
    fn preflight_refuses_meta_business_because_it_is_not_a_bridged_backend() {
        // A config that asks the bridge for the Cloud API is a routing mistake,
        // and must be named rather than silently spawning Node to do something
        // Core already does natively over HTTPS.
        let script = tempfile::NamedTempFile::new().unwrap();
        let c = cfg(WhatsappBackend::MetaBusiness, script.path().to_path_buf());
        let err = preflight(&c).unwrap_err();
        assert!(err.findings.contains(&"backend".to_string()));
    }

    #[test]
    fn preflight_passes_when_node_the_script_and_the_backend_package_all_exist() {
        // CAN PASS — the control proving the failures above are about missing
        // items and not about preflight being unable to succeed. A real file
        // stands in for Node: preflight checks the path, it runs nothing.
        let (_dir, script) = installed_bridge(WhatsappBackend::Baileys);
        let fake_node = tempfile::NamedTempFile::new().unwrap();
        let mut c = cfg(WhatsappBackend::Baileys, script.clone());
        c.node_path = Some(fake_node.path().to_path_buf());

        let launch = preflight(&c).expect("preflight must be able to succeed");
        assert_eq!(launch.backend, WhatsappBackend::Baileys);
        assert_eq!(launch.script, script);
        assert_eq!(launch.node, fake_node.path());
    }

    #[test]
    fn preflight_names_bridge_dependencies_when_the_backend_package_is_absent() {
        // The gate that closes the measured gap: the real bridge answers
        // `health` with no node_modules and only fails at the first `connect`,
        // so a handshake-only verdict would hand out an unearned green.
        let dir = tempfile::tempdir().unwrap();
        let script = dir.path().join("bridge.js");
        std::fs::write(&script, "// stand-in\n").unwrap();
        let fake_node = tempfile::NamedTempFile::new().unwrap();
        let mut c = cfg(WhatsappBackend::Baileys, script);
        c.node_path = Some(fake_node.path().to_path_buf());

        let err = preflight(&c).unwrap_err();
        assert_eq!(err.findings, vec!["bridge_dependencies".to_string()]);
        assert!(
            err.operator_message.contains("@whiskeysockets/baileys"),
            "the message must name the package to install: {}",
            err.operator_message
        );
    }

    #[test]
    fn a_hoisted_node_modules_above_the_bridge_counts_as_installed() {
        // Node resolves node_modules by walking up, so checking only the
        // sibling directory would report a false red on a hoisted install.
        let root = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(
            root.path()
                .join("node_modules")
                .join("@whiskeysockets/baileys"),
        )
        .unwrap();
        let nested = root.path().join("packages").join("bridge");
        std::fs::create_dir_all(&nested).unwrap();
        let script = nested.join("bridge.js");
        std::fs::write(&script, "// stand-in\n").unwrap();

        assert!(
            backend_package_installed(&script, WhatsappBackend::Baileys),
            "known-positive: a hoisted install must resolve"
        );
        assert!(
            !backend_package_installed(&script, WhatsappBackend::WhatsappWeb),
            "known-negative: a package that is NOT installed must not resolve"
        );
    }

    #[test]
    fn each_bridged_backend_checks_for_its_own_package() {
        // Guards against a check that passes for any backend once one package
        // happens to be installed.
        let (_d1, baileys_script) = installed_bridge(WhatsappBackend::Baileys);
        assert!(backend_package_installed(
            &baileys_script,
            WhatsappBackend::Baileys
        ));
        assert!(!backend_package_installed(
            &baileys_script,
            WhatsappBackend::WhatsappWeb
        ));

        let (_d2, www_script) = installed_bridge(WhatsappBackend::WhatsappWeb);
        assert!(backend_package_installed(
            &www_script,
            WhatsappBackend::WhatsappWeb
        ));
        assert!(!backend_package_installed(
            &www_script,
            WhatsappBackend::Baileys
        ));
    }

    #[test]
    fn pairing_marker_paths_match_the_layout_each_backend_actually_writes() {
        // Measured from backends/baileys.js (useMultiFileAuthState under
        // <session>/baileys, creds.json) and backends/whatsapp-web.js
        // (LocalAuth dataPath <session>/whatsapp-web, clientId "wayland").
        let session = PathBuf::from("/srv/wa");
        let mut c = cfg(WhatsappBackend::Baileys, PathBuf::from("/x/bridge.js"));
        c.session_dir = Some(session.clone());
        assert_eq!(
            pairing_dir(&c),
            Some(session.join("baileys").join("creds.json"))
        );

        c.backend = WhatsappBackend::WhatsappWeb;
        assert_eq!(
            pairing_dir(&c),
            Some(session.join("whatsapp-web").join("session-wayland"))
        );

        // meta-business has no bridge pairing at all.
        c.backend = WhatsappBackend::MetaBusiness;
        assert_eq!(pairing_dir(&c), None);
    }

    #[test]
    fn resolve_node_finds_a_real_file_and_rejects_a_missing_one() {
        // Instrument control for `resolve_node`'s explicit-path arm: a
        // known-positive and a known-negative in the same test, so a resolver
        // that always returned None could not pass.
        let real = tempfile::NamedTempFile::new().unwrap();
        assert_eq!(
            resolve_node(Some(real.path())).as_deref(),
            Some(real.path()),
            "known-positive: an existing file must resolve"
        );
        assert_eq!(
            resolve_node(Some(Path::new("/definitely/not/here/node"))),
            None,
            "known-negative: a missing file must not resolve"
        );
    }
}
