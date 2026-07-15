//! Deterministic Wayland Desktop producer-contract corpus.
//!
//! The corpus records the current serialized protocol surface without adding
//! authority or changing any wire variant. Generated artifacts live under
//! `contracts/desktop/v1` and are checked byte-for-byte in CI.

mod canonical;
mod check;
mod fixtures_support;
mod generate;
mod observation;
mod spec;

pub use canonical::{canonical_json, digest_named_bytes};
pub use check::check_contract;
pub use generate::{
    CONTRACT_NAME, CONTRACT_ROOT, GENERATOR_VERSION, generated_artifacts, manifest_digests,
    write_contract,
};
pub use observation::{
    ContractCapabilityStatus, ContractDescriptor, HostContractObserver, HostObservation,
    HostObservationError, producer_contract_descriptor,
};
pub use spec::{
    COMMAND_SPECS, ContractCriticality, EVENT_SPECS, SOURCE_INPUTS, WireSpec,
    command_fixture_values, event_fixture_values,
};

pub type ContractResult<T> = Result<T, Box<dyn std::error::Error + Send + Sync>>;
