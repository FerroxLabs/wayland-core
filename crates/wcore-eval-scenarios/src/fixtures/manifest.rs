//! Content-addressed identity for one complete deterministic fixture bundle.

use serde::Serialize;
use sha2::{Digest, Sha256};
use thiserror::Error;

const MANIFEST_SCHEMA: &str = "wcore-eval-composite-fixture";
const MANIFEST_VERSION: u32 = 1;

/// The six content identities that define an F04 deterministic fixture.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct FixtureComponents {
    openai_script_sha256: String,
    seeded_repository_sha256: String,
    hidden_outcome_sha256: String,
    mcp_script_sha256: String,
    egress_script_sha256: String,
    remote_execution_script_sha256: String,
}

impl FixtureComponents {
    pub fn new(
        openai_script_sha256: impl Into<String>,
        seeded_repository_sha256: impl Into<String>,
        hidden_outcome_sha256: impl Into<String>,
        mcp_script_sha256: impl Into<String>,
        egress_script_sha256: impl Into<String>,
        remote_execution_script_sha256: impl Into<String>,
    ) -> Result<Self, FixtureManifestError> {
        Ok(Self {
            openai_script_sha256: validated("openai_script", openai_script_sha256.into())?,
            seeded_repository_sha256: validated(
                "seeded_repository",
                seeded_repository_sha256.into(),
            )?,
            hidden_outcome_sha256: validated("hidden_outcome", hidden_outcome_sha256.into())?,
            mcp_script_sha256: validated("mcp_script", mcp_script_sha256.into())?,
            egress_script_sha256: validated("egress_script", egress_script_sha256.into())?,
            remote_execution_script_sha256: validated(
                "remote_execution_script",
                remote_execution_script_sha256.into(),
            )?,
        })
    }
}

/// Versioned manifest whose digest changes when any component identity changes.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct CompositeFixtureManifest {
    schema: &'static str,
    schema_version: u32,
    components: FixtureComponents,
    fixture_sha256: String,
}

impl CompositeFixtureManifest {
    pub fn new(components: FixtureComponents) -> Self {
        let canonical = serde_json::to_vec(&CanonicalManifest {
            schema: MANIFEST_SCHEMA,
            schema_version: MANIFEST_VERSION,
            components: &components,
        })
        .expect("fixture manifest contains only infallible JSON values");
        Self {
            schema: MANIFEST_SCHEMA,
            schema_version: MANIFEST_VERSION,
            components,
            fixture_sha256: format!("{:x}", Sha256::digest(canonical)),
        }
    }

    pub fn components(&self) -> &FixtureComponents {
        &self.components
    }

    pub fn fixture_sha256(&self) -> &str {
        &self.fixture_sha256
    }
}

#[derive(Serialize)]
struct CanonicalManifest<'a> {
    schema: &'static str,
    schema_version: u32,
    components: &'a FixtureComponents,
}

fn validated(component: &'static str, digest: String) -> Result<String, FixtureManifestError> {
    if digest.len() == 64
        && digest
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        Ok(digest)
    } else {
        Err(FixtureManifestError::InvalidSha256 { component })
    }
}

#[derive(Debug, Error, PartialEq, Eq)]
pub enum FixtureManifestError {
    #[error("{component} identity must be 64 lowercase hexadecimal characters")]
    InvalidSha256 { component: &'static str },
}
