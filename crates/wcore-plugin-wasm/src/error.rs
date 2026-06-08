use thiserror::Error;

#[derive(Debug, Error)]
#[non_exhaustive]
pub enum WasmPluginError {
    #[error("wasm component load failed: {0}")]
    LoadFailed(#[source] anyhow::Error),

    #[error("wasm instantiation failed: {0}")]
    InstantiateFailed(#[source] anyhow::Error),

    #[error("wasm execution failed: {0}")]
    ExecuteFailed(#[source] anyhow::Error),

    #[error("wasm plugin timeout (deadline exceeded)")]
    Timeout,

    #[error("wasm plugin exceeded memory limit")]
    MemoryLimitExceeded,

    #[error("wasm plugin exhausted fuel")]
    FuelExhausted,

    #[error("wasm plugin permission denied: {0}")]
    PermissionDenied(String),
}

pub type Result<T> = std::result::Result<T, WasmPluginError>;
