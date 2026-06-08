//! v0.6.3 manifest parse tests.

use std::path::PathBuf;
use wcore_sandbox::manifest::{NetworkPolicy, SandboxManifest, SyscallPolicy};

#[test]
fn parses_full_manifest() {
    let toml = r#"
        fs_read_allow = ["/usr/lib", "/etc/wayland"]
        fs_write_allow = ["/tmp/work"]
        network = { kind = "allow_hosts", hosts = ["api.openai.com"] }
        syscall_policy = "strict"
        max_memory_bytes = 268435456
        max_cpu_secs = 30
        env = [["FOO", "bar"]]
        image = "ghcr.io/example/img:tag"
    "#;
    let parsed: SandboxManifest = toml::from_str(toml).expect("parse manifest");
    assert_eq!(parsed.fs_read_allow.len(), 2);
    assert_eq!(parsed.fs_read_allow[0], PathBuf::from("/usr/lib"));
    assert_eq!(parsed.fs_write_allow, vec![PathBuf::from("/tmp/work")]);
    assert_eq!(
        parsed.network,
        NetworkPolicy::AllowHosts(vec!["api.openai.com".into()]),
    );
    assert_eq!(parsed.syscall_policy, SyscallPolicy::Strict);
    assert_eq!(parsed.max_memory_bytes, Some(268_435_456));
    assert_eq!(parsed.max_cpu_secs, Some(30));
    assert_eq!(parsed.env, vec![("FOO".into(), "bar".into())]);
    assert_eq!(parsed.image, "ghcr.io/example/img:tag");
}

#[test]
fn defaults_when_all_fields_missing() {
    let parsed: SandboxManifest = toml::from_str("").expect("empty manifest parses");
    assert!(parsed.fs_read_allow.is_empty());
    assert!(parsed.fs_write_allow.is_empty());
    assert_eq!(parsed.network, NetworkPolicy::Inherit);
    assert_eq!(parsed.syscall_policy, SyscallPolicy::Inherit);
    assert!(parsed.max_memory_bytes.is_none());
    assert!(parsed.max_cpu_secs.is_none());
    assert!(parsed.env.is_empty());
    assert_eq!(parsed.image, "ghcr.io/tradecanyon/wcore-sandbox:base");
}

#[test]
fn network_deny_parses() {
    let toml = r#"network = { kind = "deny" }"#;
    let parsed: SandboxManifest = toml::from_str(toml).expect("parse");
    assert_eq!(parsed.network, NetworkPolicy::Deny);
}

#[test]
fn rejects_unknown_network_policy_kind() {
    let toml = r#"network = { kind = "wormhole" }"#;
    let err = toml::from_str::<SandboxManifest>(toml).expect_err("unknown variant");
    let msg = err.to_string();
    assert!(
        msg.contains("wormhole") || msg.contains("unknown variant"),
        "unexpected error: {msg}",
    );
}
