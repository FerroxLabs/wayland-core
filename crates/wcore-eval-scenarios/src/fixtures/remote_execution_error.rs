use thiserror::Error;

#[derive(Debug, Error, PartialEq, Eq)]
pub enum RemoteFixtureError {
    #[error("invalid fixture identifier: {field}")]
    InvalidIdentifier { field: &'static str },
    #[error("invalid SHA-256 digest: {field}")]
    InvalidDigest { field: &'static str },
    #[error("fixture resource budgets must be non-zero")]
    InvalidResourceBudget,
    #[error("fixture input exceeds {limit} bytes")]
    InputTooLarge { limit: usize },
    #[error("fixture artifact exceeds {limit} bytes")]
    ArtifactTooLarge { limit: usize },
    #[error("fixture artifact name is not a portable normalized relative path")]
    InvalidArtifactName,
    #[error("fixture terminal reason is not a bounded stable code")]
    InvalidReason,
    #[error("fixture script exceeds {limit} output events")]
    TooManyEvents { limit: usize },
    #[error("fixture event sequence mismatch: expected {expected}, observed {observed}")]
    InvalidEventSequence { expected: u64, observed: u64 },
    #[error("fixture event text exceeds {limit} bytes")]
    EventTextTooLarge { limit: usize },
    #[error("fixture total event text exceeds {limit} bytes")]
    TotalEventTextTooLarge { limit: usize },
    #[error("fixture serialization failed: {0}")]
    Serialize(String),
    #[error("unsupported remote fixture receipt schema")]
    UnsupportedReceiptSchema,
    #[error("unsupported remote fixture protocol")]
    UnsupportedFixtureProtocol,
    #[error("remote fixture receipt backend identity mismatch")]
    BackendIdentityMismatch,
    #[error("remote fixture receipt body digest mismatch")]
    ReceiptDigestMismatch,
    #[error("remote fixture receipt event digest mismatch")]
    EventDigestMismatch,
    #[error("remote fixture receipt has invalid event or terminal semantics")]
    InvalidReceiptSemantics,
    #[error("remote fixture attestation signature is malformed")]
    MalformedSignature,
    #[error("remote fixture attestation signature verification failed")]
    InvalidSignature,
}
