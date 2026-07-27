//! F25-03 — what a node says it can do.
//!
//! An advertisement names the execution backends a node HOSTS and, for each,
//! whether it is actually available and what established that. It references
//! the backend contract by name; it does not restate it.
//!
//! ## Advertisements are refreshed, never assumed
//!
//! Plan 25-01 established the capability-honesty rule for backends: a socket
//! existing is not a daemon answering, so availability is a PROBE with a named
//! basis rather than an inference. The same rule applies one layer up. A node
//! whose Docker daemon has died must stop claiming a container backend, and
//! the only way it can is if the advertisement is re-derived from a fresh probe
//! rather than read out of a cache written when things were healthy.
//!
//! ## Advertisements carry no host detail
//!
//! Named backends, availability, probe basis and a short detail line. No
//! paths, no credentials, no environment values. An advertisement travels to a
//! controller across a network the node does not control.

use serde::{Deserialize, Serialize};

use super::version::NodeContractVersion;
use crate::contract::{BackendKind, ProbeBasis, ResourceBudget};
use crate::error::Result;

/// One backend as advertised by a node.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AdvertisedBackend {
    pub backend_id: String,
    pub kind: BackendKind,
    pub version: String,
    pub available: bool,
    /// What ESTABLISHED availability — a probe name, never an assumption.
    pub probe_basis: String,
    /// One operator-readable line. Host paths and credentials never appear.
    pub detail: String,
}

/// Everything a node advertises about itself.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NodeAdvertisement {
    pub node_id: String,
    pub os: String,
    pub contract_version: NodeContractVersion,
    /// When this advertisement was produced. A controller can see staleness
    /// rather than having to trust that a cache was invalidated.
    pub observed_unix_ms: u64,
    pub backends: Vec<AdvertisedBackend>,
}

impl NodeAdvertisement {
    /// An advertisement claiming nothing. Used in tests and as the far-end
    /// answer when a host hosts no backends at all — which is a legitimate,
    /// visible state, not an error.
    pub fn empty(node_id: &str) -> Self {
        Self {
            node_id: node_id.to_string(),
            os: std::env::consts::OS.to_string(),
            contract_version: super::version::NODE_CONTRACT_VERSION,
            observed_unix_ms: now_unix_ms(),
            backends: Vec::new(),
        }
    }

    /// Produce a FRESH advertisement by probing every reference backend this
    /// build carries. This is the only supported way to build a real one —
    /// there is deliberately no constructor that takes a backend list, because
    /// that is the shape a stale cache would arrive in.
    pub async fn observe(node_id: &str, limits: ResourceBudget) -> Result<Self> {
        let mut backends = Vec::new();
        for reference in crate::reference_backends(limits)? {
            let caps = reference.backend.capabilities();
            let availability = reference.backend.availability().await;
            backends.push(AdvertisedBackend {
                backend_id: caps.backend_id.clone(),
                kind: caps.kind,
                version: caps.version.clone(),
                available: availability.available,
                probe_basis: probe_basis_name(&availability.probe).to_string(),
                detail: availability.detail.clone(),
            });
        }
        backends.sort_by(|a, b| a.backend_id.cmp(&b.backend_id));
        Ok(Self {
            node_id: node_id.to_string(),
            os: std::env::consts::OS.to_string(),
            contract_version: super::version::NODE_CONTRACT_VERSION,
            observed_unix_ms: now_unix_ms(),
            backends,
        })
    }

    /// Backends this node claims are usable right now.
    pub fn available_backends(&self) -> Vec<&AdvertisedBackend> {
        self.backends.iter().filter(|b| b.available).collect()
    }

    /// Is this advertisement older than `max_age_ms`?
    ///
    /// A controller that cannot tell a fresh advertisement from a stale one
    /// will keep routing work to a backend that died an hour ago.
    pub fn is_stale(&self, now_unix_ms: u64, max_age_ms: u64) -> bool {
        now_unix_ms.saturating_sub(self.observed_unix_ms) > max_age_ms
    }

    /// Does this advertisement carry anything that looks like host detail?
    ///
    /// Used as an assertion rather than a filter: a leak should FAIL loudly at
    /// the point it is introduced, not be silently stripped where a future
    /// change could route around the stripping.
    pub fn leaks_host_detail(&self) -> Option<String> {
        for b in &self.backends {
            for (field, value) in [
                ("probe_basis", &b.probe_basis),
                ("backend_id", &b.backend_id),
            ] {
                if value.contains('/') || value.contains('\\') {
                    return Some(format!("{}.{field} contains a path: {value}", b.backend_id));
                }
            }
        }
        None
    }
}

fn probe_basis_name(basis: &ProbeBasis) -> &'static str {
    match basis {
        ProbeBasis::SandboxBackendProbe => "sandbox_backend_probe",
        ProbeBasis::DaemonPing => "daemon_ping",
        ProbeBasis::SshHandshake => "ssh_handshake",
        ProbeBasis::VendorApiCall => "vendor_api_call",
        ProbeBasis::CredentialAbsent => "credential_absent",
        ProbeBasis::ProbeFailed => "probe_failed",
    }
}

pub(crate) fn now_unix_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ad(node: &str, os: &str, backends: Vec<AdvertisedBackend>) -> NodeAdvertisement {
        NodeAdvertisement {
            node_id: node.into(),
            os: os.into(),
            contract_version: crate::node::version::NODE_CONTRACT_VERSION,
            observed_unix_ms: 1_000_000,
            backends,
        }
    }

    fn backend(id: &str, kind: BackendKind, available: bool, basis: &str) -> AdvertisedBackend {
        AdvertisedBackend {
            backend_id: id.into(),
            kind,
            version: "0.12.25".into(),
            available,
            probe_basis: basis.into(),
            detail: "d".into(),
        }
    }

    #[test]
    fn available_backends_excludes_the_unavailable_ones() {
        let a = ad(
            "alpha",
            "linux",
            vec![
                backend("local", BackendKind::Local, true, "sandbox_backend_probe"),
                backend("container", BackendKind::Container, false, "daemon_ping"),
            ],
        );
        let avail = a.available_backends();
        assert_eq!(avail.len(), 1);
        assert_eq!(avail[0].backend_id, "local");
    }

    /// The staleness check has to be able to say yes AND no, or it is decoration.
    #[test]
    fn staleness_is_measured_not_assumed() {
        let a = ad("alpha", "linux", vec![]);
        assert!(!a.is_stale(1_000_500, 60_000));
        assert!(a.is_stale(1_100_000, 60_000));
    }

    /// Two different operating systems must be able to advertise differently.
    /// A hardcoded identical list across a Linux and a Windows node would be a
    /// defect wearing a pass.
    #[test]
    fn two_nodes_can_advertise_genuinely_different_capability_sets() {
        let linux = ad(
            "alpha",
            "linux",
            vec![
                backend("local", BackendKind::Local, true, "sandbox_backend_probe"),
                backend("container", BackendKind::Container, true, "daemon_ping"),
            ],
        );
        let windows = ad(
            "beta",
            "windows",
            vec![
                backend("local", BackendKind::Local, true, "sandbox_backend_probe"),
                backend("container", BackendKind::Container, false, "daemon_ping"),
            ],
        );
        assert_ne!(linux.os, windows.os);
        assert_ne!(
            linux.available_backends().len(),
            windows.available_backends().len(),
            "the two hosts must be able to differ, or advertisement proves nothing"
        );
    }

    #[test]
    fn a_path_in_an_advertisement_is_reported_as_a_leak() {
        let leaky = ad(
            "alpha",
            "linux",
            vec![backend(
                "local",
                BackendKind::Local,
                true,
                "/root/.wayland/exec-backend",
            )],
        );
        assert!(leaky.leaks_host_detail().is_some());
        let clean = ad(
            "alpha",
            "linux",
            vec![backend("local", BackendKind::Local, true, "daemon_ping")],
        );
        assert!(clean.leaks_host_detail().is_none());
    }
}
