//! `wayland-core#354 c7` — `--doctor --probe-mcp` must launch under the
//! operator's chosen malware-gate mode.
//!
//! `--doctor` returns at `main.rs` BEFORE config/OAuth/engine bootstrap, and
//! `AgentBootstrap::build` was the only caller of
//! `wcore_mcp::malware_gate::install_mode`. So the probe — which spawns real
//! stdio transports — reached `StdioTransport::spawn` with the mode
//! uninstalled and silently took the permissive default. Under `strict`, the
//! one command an operator runs to ASK whether the gate is on was the command
//! that did not honour it.
//!
//! ## Why this is an integration test and not a unit test
//!
//! `install_mode` is a one-shot `OnceLock`, deliberately: a plugin or a late
//! code path must not swap a security posture out from under a session that
//! already launched servers under it. One process can therefore observe
//! exactly one installed mode, so this file is its own test binary and holds
//! exactly one mode-installing test.
//!
//! ## Why it cannot pass vacuously
//!
//! The uninstalled default IS `Permissive`, which is also the shipped default
//! — so "mode() == Strict at the end" is only meaningful next to "mode() ==
//! Permissive at the start". The pre-assertion is the defect state, asserted
//! in this very process, and it doubles as the permissive control: had the
//! call not installed anything, the process would have stayed exactly as the
//! bug leaves it. The `[mcp] malware_gate` key is written into the GLOBAL
//! config, because c7 is about the OPERATOR's choice; an untrusted project
//! file is narrowed by an allowlist and would prove something else.

use std::ffi::OsString;

use wcore_config::config::{CliArgs, McpMalwareGateMode};

/// Restores every environment variable it touched, so a failure here cannot
/// leak a temp `WAYLAND_HOME` into whatever runs next in this binary.
struct EnvGuard {
    saved: Vec<(&'static str, Option<OsString>)>,
}

impl EnvGuard {
    fn set(values: &[(&'static str, Option<&std::ffi::OsStr>)]) -> Self {
        let saved = values
            .iter()
            .map(|(key, _)| (*key, std::env::var_os(key)))
            .collect();
        for (key, value) in values {
            // SAFETY: this binary holds a single test, so nothing else is
            // reading the environment concurrently.
            unsafe {
                match value {
                    Some(value) => std::env::set_var(key, value),
                    None => std::env::remove_var(key),
                }
            }
        }
        Self { saved }
    }
}

impl Drop for EnvGuard {
    fn drop(&mut self) {
        for (key, value) in &self.saved {
            // SAFETY: as above.
            unsafe {
                match value {
                    Some(value) => std::env::set_var(key, value),
                    None => std::env::remove_var(key),
                }
            }
        }
    }
}

#[tokio::test]
async fn doctor_probe_mcp_launches_under_the_operators_gate_mode() {
    let home = tempfile::tempdir().expect("home tempdir");
    let project = tempfile::tempdir().expect("project tempdir");

    // The operator's own config: strict, plus a declared stdio server so the
    // probe below really enters the launch block rather than short-circuiting
    // on an empty server map. The command does not exist, so the spawn fails
    // immediately -- this test grades the POSTURE the launch path runs under,
    // not whether a server came up.
    std::fs::write(
        home.path().join("config.toml"),
        "[mcp]\n\
         malware_gate = \"strict\"\n\
         \n\
         [mcp.servers.probe-target]\n\
         transport = \"stdio\"\n\
         command = \"wl354-no-such-mcp-server\"\n",
    )
    .expect("write global config");

    let _env = EnvGuard::set(&[
        ("WAYLAND_HOME", Some(home.path().as_os_str())),
        ("WAYLAND_CONFIG_PATH", None),
        ("XDG_DATA_HOME", None),
    ]);

    // Credentials on the command line, the way `--doctor` itself passes them
    // through (`doctor_args` in `main.rs`): this host has no keyring and a
    // `[default] api_key` key is not a recognised config setting, so a config
    // file cannot satisfy the resolver here.
    let cli = CliArgs {
        provider: Some("anthropic".to_string()),
        api_key: Some("test-key-not-a-real-credential".to_string()),
        project_dir: Some(project.path().to_path_buf()),
        ..CliArgs::default()
    };

    // Instrument-alive: the operator's strict really is what this command line
    // resolves to. Without this, a config that failed to load would leave the
    // process permissive and the test would be grading its own fixture.
    let resolved = wcore_config::config::Config::resolve(&cli).expect("resolving config");
    assert_eq!(
        resolved.mcp.malware_gate,
        McpMalwareGateMode::Strict,
        "fixture is not strict; the rest of this test would be vacuous"
    );

    // The defect state, asserted in this process before anything runs: nothing
    // has installed a mode, so the gate reads the permissive default.
    assert_eq!(
        wcore_mcp::malware_gate::mode(),
        McpMalwareGateMode::Permissive,
        "the uninstalled default must be permissive; if it is not, the \
         post-assertion below proves nothing about who installed it"
    );

    let _exit = wcore_cli::doctor::run(true, false, &cli).await;

    assert_eq!(
        wcore_mcp::malware_gate::mode(),
        McpMalwareGateMode::Strict,
        "--doctor --probe-mcp reached the MCP launch path with the operator's \
         strict mode uninstalled, so it probed under the permissive default \
         (FerroxLabs/wayland-core#354 c7)"
    );
}
