//! SEC-05 follow-up — a Deny-network sandbox must not see the host's resolver
//! configuration.
//!
//! `WorkspacePolicy::trusted_local` granted `/etc/resolv.conf` through
//! `trusted_config_and_certificate_reads()`, which CANONICALIZES it. On a
//! systemd-resolved host that resolves to `/run/systemd/resolve/stub-resolv.conf`,
//! and the resolved path went into `fs_read_allow` UNCONDITIONALLY. So the
//! bwrap backend's `NETWORK_RO_ETC` gate — which correctly withholds
//! `/etc/resolv.conf` itself unless the manifest grants a network — was inert
//! for the policy the product actually uses: the child simply read the resolved
//! path instead and got the host's DNS server addresses and its real search
//! domains (on the measured host, a private tailnet domain). Under
//! `NetworkPolicy::Deny` that is pure leak: there is no network for a resolver
//! to serve.
//!
//! Both directions are asserted. Withholding under Deny is the fix; still
//! granting under a real network posture is the POSITIVE CONTROL that proves
//! the Deny assertion measures the gate and not a missing file, and that the
//! narrowing did not break name resolution for the sessions that are allowed to
//! use it.

use std::path::{Path, PathBuf};

use wcore_sandbox::manifest::NetworkPolicy;
use wcore_tools::workspace_policy::WorkspacePolicy;

/// The host's real resolver file, as `trusted_config_and_certificate_reads`
/// would resolve it, plus a marker line that provably exists inside it.
/// Returns `None` when the host has no readable resolver config.
fn host_resolver() -> Option<(PathBuf, String)> {
    let canonical = std::fs::canonicalize("/etc/resolv.conf").ok()?;
    let text = std::fs::read_to_string(&canonical).ok()?;
    // A `search`/`nameserver` line is host-identifying and is the content this
    // test exists to keep out of the sandbox. Derived at run time — never
    // hardcoded, so the assertion cannot silently stop matching.
    let marker = text
        .lines()
        .map(str::trim)
        .find(|l| l.starts_with("search ") || l.starts_with("nameserver "))?
        .to_owned();
    (marker.len() >= 12).then_some((canonical, marker))
}

fn policy_for(root: &Path, network: NetworkPolicy) -> WorkspacePolicy {
    WorkspacePolicy::trusted_local(root).with_network(network)
}

// ---------------------------------------------------------------------------
// Policy level — runs on every platform, no sandbox needed.
// ---------------------------------------------------------------------------

#[test]
fn deny_network_policy_does_not_grant_the_resolver_path() {
    let Some((canonical, _)) = host_resolver() else {
        eprintln!("skip: no readable /etc/resolv.conf on this host");
        return;
    };
    let tmp = tempfile::tempdir().expect("tempdir");

    let allowed = policy_for(tmp.path(), NetworkPolicy::Inherit).readable_roots();
    assert!(
        allowed.iter().any(|p| p == &canonical),
        "POSITIVE CONTROL DID NOT FIRE — a network-granted policy must still \
         grant the resolver config, otherwise the Deny assertion below proves \
         nothing about gating.\n  resolver: {canonical:?}\n  granted: {allowed:#?}"
    );

    let denied = policy_for(tmp.path(), NetworkPolicy::Deny).readable_roots();
    assert!(
        !denied.iter().any(|p| p == &canonical),
        "RESOLVER LEAK — a Deny-network policy granted the host resolver config \
         {canonical:?}, which names the host's DNS servers and search domains \
         and enables nothing (there is no network).\n  granted: {denied:#?}"
    );
}

// ---------------------------------------------------------------------------
// Live level — the real bwrap namespace, driven through the real policy.
// ---------------------------------------------------------------------------

#[cfg(target_os = "linux")]
mod live {
    use super::*;
    use wcore_sandbox::backends::SandboxBackend;
    use wcore_sandbox::backends::bwrap::BubblewrapBackend;
    use wcore_sandbox::{SandboxCommand, SandboxManifest};

    /// Build the manifest the way production does — see
    /// `wcore_tools::bash::build_sandbox_pieces`, which sets `fs_write_allow`
    /// from `writable_roots()`, `fs_read_allow` from `readable_roots()` and
    /// `network` from `network()`.
    fn manifest_from(policy: &WorkspacePolicy) -> SandboxManifest {
        SandboxManifest {
            fs_write_allow: policy.writable_roots(),
            fs_read_allow: policy.readable_roots(),
            network: policy.network(),
            env: vec![("PATH".into(), "/usr/bin:/bin".into())],
            ..Default::default()
        }
    }

    /// Read both spellings the child could use, with a proof-of-execution
    /// sentinel either side. Without the sentinels an assertion that the
    /// resolver content is ABSENT is satisfied by a child that never ran —
    /// the exact vacuity that let a broken bwrap namespace report PASS on the
    /// SEC-05/07/10 containment suite.
    async fn read_resolver_in_sandbox(
        policy: &WorkspacePolicy,
        canonical: &Path,
        nonce: &str,
    ) -> String {
        let backend = BubblewrapBackend::new();
        let body = format!(
            "cat /etc/resolv.conf 2>&1; cat {} 2>&1",
            canonical.display()
        );
        let out = backend
            .execute(
                &manifest_from(policy),
                SandboxCommand {
                    argv: vec![
                        "/bin/sh".into(),
                        "-c".into(),
                        format!("echo {nonce}-START; {body}; echo {nonce}-END=$?"),
                    ],
                    cwd: Some(policy.root().to_path_buf()),
                },
            )
            .await
            .expect("bwrap execute");
        let observed = format!(
            "{}{}",
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        );
        assert!(
            observed.contains(&format!("{nonce}-START"))
                && observed.contains(&format!("{nonce}-END=")),
            "POSITIVE CONTROL DID NOT FIRE — the sandboxed child did not run to \
             completion, so any absence assertion against its output is VACUOUS.\
             \n  child output: {observed:?}"
        );
        observed
    }

    fn nonce(tag: &str) -> String {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("clock after epoch")
            .as_nanos();
        format!("WLRESOLV{tag}{}{nanos:x}", std::process::id())
    }

    #[tokio::test]
    async fn deny_network_sandbox_cannot_read_the_host_resolver() {
        if !BubblewrapBackend::new().is_available() {
            eprintln!("skip: bwrap not available");
            return;
        }
        let Some((canonical, marker)) = host_resolver() else {
            eprintln!("skip: no readable /etc/resolv.conf on this host");
            return;
        };
        let tmp = tempfile::tempdir().expect("tempdir");
        let root = std::fs::canonicalize(tmp.path()).expect("canonicalize");

        // POSITIVE CONTROL: with a network granted, the resolver IS readable.
        // This proves the probe can observe the file at all, and that DNS
        // configuration still reaches the sessions entitled to it.
        let allowed = policy_for(&root, NetworkPolicy::Inherit);
        let seen = read_resolver_in_sandbox(&allowed, &canonical, &nonce("allow")).await;
        assert!(
            seen.contains(&marker),
            "POSITIVE CONTROL DID NOT FIRE — a network-granted sandbox could not \
             read the resolver config, so the Deny assertion below would be \
             vacuous.\n  marker: {marker:?}\n  child output: {seen:?}"
        );

        // The assertion under test.
        let denied = policy_for(&root, NetworkPolicy::Deny);
        let observed = read_resolver_in_sandbox(&denied, &canonical, &nonce("deny")).await;
        assert!(
            !observed.contains(&marker),
            "RESOLVER LEAK — a Deny-network sandbox read the host's resolver \
             configuration, exposing its DNS servers and search domains.\n  \
             marker: {marker:?}\n  child output: {observed:?}"
        );
    }
}
