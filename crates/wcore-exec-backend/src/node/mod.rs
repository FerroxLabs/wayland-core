//! F25-03 — the node/device contract.
//!
//! ## A node is not a backend
//!
//! [`crate::contract::ExecutionBackend`] answers **how** work runs: local
//! containment, a container, an SSH transport, a cloud machine. A node answers
//! **where** and **by whose authority**. It is a paired, identified, revocable
//! machine that HOSTS one or more named backends and advertises what each can
//! do. This module therefore sits ABOVE the backend contract in the same
//! crate and refers to backends by name; it does not subclass, wrap or
//! re-declare the trait.
//!
//! Fleet claiming, dependency ordering and work distribution belong to Phase
//! 22 and already exist there. Nothing here schedules, discovers or meshes.
//!
//! ## The clause that actually matters
//!
//! Success Criterion 2 reads "…and handle mixed versions **without losing
//! authority attribution**." Pairing, advertising, revoking and recovering are
//! the visible verbs, but a node that pairs, works, vanishes, returns, and
//! then produces work nobody can tie back to the authority that requested it
//! has failed the criterion even though every verb reported success. So:
//!
//! * node identity is **attested inside the receipt** ([`attribution`]) rather
//!   than carried as a caller-settable string, and
//! * every disruption in this module has a re-verification path that asks the
//!   attribution question *after* the disruption.

pub mod attribution;
pub mod capability;
pub mod pairing;
pub mod registry;
pub mod version;

pub use attribution::{NodeAttribution, verify_node_attribution};
pub use capability::{AdvertisedBackend, NodeAdvertisement};
pub use pairing::{NodeIdentity, PairingChallenge, PairingProof, prove_challenge, verify_proof};
pub use registry::{Liveness, NodeRecord, NodeRegistry, NodeState, SubmissionVerdict};
pub use version::{NODE_CONTRACT_VERSION, VersionVerdict, evaluate_version};
