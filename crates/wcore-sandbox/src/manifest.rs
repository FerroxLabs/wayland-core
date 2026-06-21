//! `SandboxManifest` — the TOML schema a plugin declares to request
//! sandboxed execution of a tool. Parsed from the plugin manifest fragment
//! and passed into `SandboxBackend::execute` at run time.
//!
//! v0.6.3 schema (BREAKING — replaces the v0.6.2 schema):
//! - Path-based filesystem allowlists (`fs_read_allow` / `fs_write_allow`)
//!   replace the old `allow_mounts` + Docker-bind concept. Backends translate
//!   these to bind mounts (Docker), bwrap `--ro-bind` / `--bind` (Linux),
//!   sandbox-exec rules (macOS), or AppContainer ACLs (Windows).
//! - `network` now carries an inline `AllowHosts(Vec<String>)` variant that
//!   replaces the separate `allow_hosts` field; backends without a DNS gate
//!   return `PolicyNotSupported` rather than silently downgrading.
//! - `syscall_policy` (Linux-only) and `timeout` are first-class.
//! - Resource limits are advisory; see `ResourceLimitEnforcement` in the
//!   crate root for what each backend can actually enforce.

use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::time::Duration;

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "kind", content = "hosts")]
pub enum NetworkPolicy {
    /// Inherit host network (default — pre-sandbox parity).
    #[default]
    Inherit,
    /// Block all network access.
    Deny,
    /// Egress to listed hostnames only. Backend may emit
    /// `PolicyNotSupported` if no DNS gate is available (e.g. Docker today).
    AllowHosts(Vec<String>),
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SyscallPolicy {
    /// No syscall filter beyond what the host kernel enforces.
    #[default]
    Inherit,
    /// Strict seccomp filter (Linux-only via libseccomp; ignored elsewhere).
    Strict,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SandboxManifest {
    /// Read-allowed paths (absolute, on host). Backend translates to
    /// bind/profile rules.
    #[serde(default)]
    pub fs_read_allow: Vec<PathBuf>,
    /// Write-allowed paths.
    #[serde(default)]
    pub fs_write_allow: Vec<PathBuf>,
    /// Read-DENIED paths (absolute, canonicalized). Backends deny reads even
    /// under an `fs_read_allow` subtree. Empty = today's behavior.
    #[serde(default)]
    pub fs_read_deny: Vec<PathBuf>,
    /// Network policy.
    #[serde(default)]
    pub network: NetworkPolicy,
    /// Syscall policy (Linux only, ignored elsewhere).
    #[serde(default)]
    pub syscall_policy: SyscallPolicy,
    /// Wall-clock timeout for the child process. Optional; backends pick a
    /// sane default if None. TOML encoding uses serde's default struct form
    /// (`{ secs = N, nanos = M }`); programmatic callers pass a `Duration`
    /// directly.
    #[serde(default)]
    pub timeout: Option<Duration>,
    /// Max RSS bytes for the child. Enforced where the backend can;
    /// best-effort elsewhere (see `ResourceLimitEnforcement`).
    #[serde(default)]
    pub max_memory_bytes: Option<u64>,
    /// Max CPU seconds.
    #[serde(default)]
    pub max_cpu_secs: Option<u64>,
    /// Explicit env injected into the child. Backends ALWAYS scrub host env
    /// first (`env -i` style).
    #[serde(default)]
    pub env: Vec<(String, String)>,
    /// Docker-only: container image. Default
    /// `ghcr.io/tradecanyon/wcore-sandbox:base`. Other backends ignore this
    /// field.
    #[serde(default = "default_image")]
    pub image: String,
}

fn default_image() -> String {
    "ghcr.io/tradecanyon/wcore-sandbox:base".into()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fs_read_deny_defaults_empty() {
        let m = SandboxManifest::default();
        assert!(
            m.fs_read_deny.is_empty(),
            "fs_read_deny must default to empty (today's behavior)"
        );
    }

    #[test]
    fn fs_read_deny_roundtrips_toml() {
        let m = SandboxManifest {
            fs_read_deny: vec![
                PathBuf::from("/tmp/secret.env"),
                PathBuf::from("/home/user/.ssh/id_rsa"),
            ],
            ..Default::default()
        };
        let serialized = toml::to_string(&m).expect("serialize");
        let back: SandboxManifest = toml::from_str(&serialized).expect("deserialize");
        assert_eq!(m.fs_read_deny, back.fs_read_deny);
    }

    #[test]
    fn fs_read_deny_missing_from_toml_defaults_empty() {
        // A serialized manifest without the field (old schema) must still
        // deserialize cleanly with an empty deny list.
        let toml_str = "";
        let m: SandboxManifest = toml::from_str(toml_str).expect("deserialize");
        assert!(
            m.fs_read_deny.is_empty(),
            "missing fs_read_deny in old schema must default to empty"
        );
    }
}
