//! B2 — the real egress policy that the B1 `wcore-egress` chokepoint installs.
//!
//! [`classify`] is the pure decision core (allowlist + exfil-shape); the async
//! policy that resolves Ask/Exfil through the approval bridge and persists
//! "always" allows is wired on top in a later step. Installed process-wide via
//! `wcore_egress::install_global_policy` at bootstrap.

pub mod bridge_doorbell;
pub mod classify;
pub mod consent;
pub mod defaults;
pub mod install;
pub mod policy;

pub use bridge_doorbell::BridgeConsentDoorbell;
pub use classify::{AllowList, EgressVerdict, classify};
pub use consent::{ConsentDecision, ConsentDoorbell};
pub use defaults::build_allowlist;
pub use install::install_egress_policy;
pub use policy::{AgentEgressPolicy, EgressPosture, installed_policy};
