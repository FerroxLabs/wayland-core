use std::path::PathBuf;

use thiserror::Error;

#[derive(Debug, Error)]
pub enum PluginSrcError {
    #[error("unrecognized plugin format at {0}")]
    UnknownFormat(PathBuf),
    #[error("marketplace manifest invalid: {0}")]
    MarketplaceManifest(String),
    #[error("plugin manifest invalid: {0}")]
    PluginManifest(String),
    #[error("path traversal rejected: {0}")]
    PathTraversal(String),
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
    #[error("json: {0}")]
    Json(#[from] serde_json::Error),
    #[error("yaml: {0}")]
    Yaml(#[from] serde_yaml::Error),
    #[error("toml serialize: {0}")]
    TomlSer(#[from] toml::ser::Error),
}

pub type Result<T> = std::result::Result<T, PluginSrcError>;
