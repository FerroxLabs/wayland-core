//! The fresh-machine path: `ensure_ready` must INSTALL the Camoufox sidecar
//! when nothing on the host provides it.
//!
//! `[browser.camoufox_download]` resolves to a self-contained sidecar
//! executable per platform. Upstream publishes no such artifact — it publishes
//! the Camoufox *browser* and the `camofox-browser` *control server*
//! separately — so that path could never be a working default, and the browser
//! tool refused on every machine nobody had prepared by hand.
//!
//! These arms grade the WIRING, not the installer. A unit test of
//! `provision_sidecar_via_npm` cannot tell whether `ensure_ready` ever reaches
//! it; that is exactly the defect this file exists to catch, and it is the same
//! defect `camoufox_provisioning_wiring_test.rs` was written for one path
//! earlier.
//!
//! `npm` is a STUB on `PATH`, so no test here touches the network or a real
//! registry. The stub records its argv, which is what lets the
//! "never `--ignore-scripts`" claim below be measured rather than asserted
//! about source.
//!
//! **Unix only** — the stand-in npm and sidecar are `#!`-scripts and the
//! executable bit is a Unix concept. Windows is a gap here, not a pass.

#![cfg(unix)]

use std::os::unix::fs::PermissionsExt as _;
use std::path::{Path, PathBuf};
use std::sync::{Arc, OnceLock};
use std::time::Duration;

use wcore_browser::supervisor::{BrowserSupervisor, SupervisorConfig};
use wcore_config::browser::{CamoufoxDownloadConfig, SidecarAutoInstall};

/// A sidecar name no PATH entry can satisfy, so a successful `ensure_ready` is
/// attributable to the install and to nothing already on the host.
const PROGRAM: &str = "wcore-npm-sidecar-that-does-not-exist";

/// A stub `npm`, prepended to `PATH` ONCE for this test binary.
///
/// Set once and never mutated, so the arms below cannot race each other
/// through the process-global environment. Each arm isolates itself by
/// install root instead: the stub derives every path it touches from the
/// `--prefix` it was handed.
fn stub_npm_on_path() -> &'static Path {
    static DIR: OnceLock<PathBuf> = OnceLock::new();
    DIR.get_or_init(|| {
        let dir = std::env::temp_dir().join(format!("wcore-stub-npm-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let npm = dir.join("npm");
        std::fs::write(
            &npm,
            format!(
                r#"#!/usr/bin/env python3
import sys, pathlib, os
argv = sys.argv[1:]
prefix = None
for i, a in enumerate(argv):
    if a == "--prefix" and i + 1 < len(argv):
        prefix = argv[i + 1]
if prefix is None:
    sys.exit("stub npm: no --prefix in " + repr(argv))
prefix = pathlib.Path(prefix)
root = prefix.parent
root.mkdir(parents=True, exist_ok=True)
# Evidence of the call, and of exactly what argv reached npm.
(root / "npm-argv.log").write_text("\0".join(argv))
port = (root / "port").read_text().strip()
bindir = prefix / "bin"
bindir.mkdir(parents=True, exist_ok=True)
shim = bindir / "{program}"
shim.write_text('''#!/usr/bin/env python3
import http.server, socketserver
class H(http.server.BaseHTTPRequestHandler):
    def do_GET(self):
        self.send_response(200); self.send_header("Content-Length","2"); self.end_headers()
        self.wfile.write(b"ok")
    def log_message(self, *a): pass
socketserver.TCPServer.allow_reuse_address = True
socketserver.TCPServer(("127.0.0.1", ''' + port + '''), H).serve_forever()
''')
os.chmod(shim, 0o755)
"#,
                program = PROGRAM
            ),
        )
        .unwrap();
        std::fs::set_permissions(&npm, std::fs::Permissions::from_mode(0o755)).unwrap();
        let old = std::env::var("PATH").unwrap_or_default();
        unsafe {
            std::env::set_var("PATH", format!("{}:{old}", dir.display()));
        }
        dir
    })
    .as_path()
}

fn free_port() -> u16 {
    let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    l.local_addr().unwrap().port()
}

fn cfg(port: u16, root: &Path, auto: SidecarAutoInstall) -> SupervisorConfig {
    SupervisorConfig {
        pid_dir: root.join("pids"),
        reaper_interval: Duration::from_millis(200),
        healthcheck_interval: Duration::from_secs(30),
        healthcheck_url: format!("http://127.0.0.1:{port}/health"),
        sidecar_program: Some(PROGRAM.to_string()),
        startup_timeout: Duration::from_secs(20),
        // OFF, so nothing here can be satisfied by the pinned-artifact path.
        camoufox_download: CamoufoxDownloadConfig::default(),
        sidecar_auto_install: auto,
        binary_install_root: root.join("bin"),
        ..SupervisorConfig::default()
    }
}

/// Precondition for both arms. Without it, ARM 1 could be satisfied by a host
/// that already had the sidecar and ARM 2 would prove nothing.
#[test]
fn configured_program_is_genuinely_unresolvable() {
    stub_npm_on_path();
    assert!(
        which::which(PROGRAM).is_err(),
        "{PROGRAM} resolves on this host; both arms below would be vacuous"
    );
}

/// ARM 1 — nothing on the host provides the sidecar, so `ensure_ready` installs
/// it and spawns what it installed.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn ensure_ready_installs_the_sidecar_and_spawns_what_it_installed() {
    stub_npm_on_path();
    let port = free_port();
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("bin");
    std::fs::create_dir_all(&root).unwrap();
    std::fs::write(root.join("port"), port.to_string()).unwrap();

    let sup = Arc::new(BrowserSupervisor::with_config(cfg(
        port,
        tmp.path(),
        SidecarAutoInstall::default(),
    )));

    sup.ensure_ready().await.expect(
        "ensure_ready must install the sidecar; the configured program is not on PATH and the \
         pinned-artifact path is disabled, so this can only succeed via the npm path",
    );

    assert_eq!(
        sup.live_sessions().len(),
        1,
        "exactly one sidecar session must be tracked"
    );

    let shim = root.join("node").join("bin").join(PROGRAM);
    assert!(
        shim.is_file(),
        "installed sidecar missing at {}",
        shim.display()
    );
    assert_ne!(
        std::fs::metadata(&shim).unwrap().permissions().mode() & 0o111,
        0,
        "installed sidecar is not executable"
    );

    // What actually reached npm. This is the arm's substantive claim.
    let argv = std::fs::read_to_string(root.join("npm-argv.log"))
        .expect("npm was never invoked, so ensure_ready did not reach the install path");
    let args: Vec<&str> = argv.split('\0').collect();

    for want in ["install", "-g", "--prefix", "@askjo/camofox-browser"] {
        assert!(args.contains(&want), "npm argv missing {want:?}: {args:?}");
    }
    assert!(
        args.contains(&root.join("node").to_str().unwrap()),
        "npm must install into the Core-owned prefix, got {args:?}"
    );

    // The whole point. `--ignore-scripts` skips the package's postinstall,
    // which is what fetches the Camoufox browser — the published tarball is
    // 181 KB and contains no browser. Passing it would install a control
    // server with nothing behind it and move the failure from install time to
    // first navigation, which is the two-wall shape this path removes.
    assert!(
        !args.contains(&"--ignore-scripts"),
        "npm must NOT skip the postinstall that fetches the browser: {args:?}"
    );
}

/// ARM 2 (control) — the SAME setup with the switch off. Nothing is installed,
/// and the operator keeps the pre-existing actionable refusal. Without this
/// arm, ARM 1 is satisfied by an implementation that installs unconditionally
/// and ignores the operator's opt-out.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn nothing_is_installed_when_auto_install_is_disabled() {
    stub_npm_on_path();
    let port = free_port();
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("bin");
    std::fs::create_dir_all(&root).unwrap();
    std::fs::write(root.join("port"), port.to_string()).unwrap();

    let sup = Arc::new(BrowserSupervisor::with_config(cfg(
        port,
        tmp.path(),
        SidecarAutoInstall {
            enabled: false,
            ..SidecarAutoInstall::default()
        },
    )));

    let err = sup
        .ensure_ready()
        .await
        .expect_err("with auto-install off and no sidecar on PATH, ensure_ready must refuse");

    assert!(
        !root.join("npm-argv.log").exists(),
        "npm ran despite the operator turning auto-install off"
    );
    assert!(
        !root.join("node").join("bin").join(PROGRAM).exists(),
        "a sidecar was installed despite the operator turning auto-install off"
    );
    // The refusal still has to tell the operator what to do about it.
    let msg = err.to_string();
    assert!(
        msg.contains(PROGRAM) || msg.to_lowercase().contains("camofox"),
        "the refusal must name the sidecar it could not start; got: {msg}"
    );
    assert!(sup.live_sessions().is_empty());
}
