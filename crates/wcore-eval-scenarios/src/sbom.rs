//! RED BASELINE — the naive CycloneDX transform, kept only long enough to
//! record which contract behaviors it fails. Replaced in the GREEN commit.
//!
//! This is deliberately the implementation a careless author writes: it stamps
//! a wall-clock timestamp, emits components in whatever order the input had,
//! drops packages that declare no license, and copies the cargo package id
//! (which embeds an absolute filesystem path) straight into the document.

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use thiserror::Error;

pub const CYCLONEDX_SPEC_VERSION: &str = "1.5";
pub const UNKNOWN_LICENSE_MARKER: &str = "NOASSERTION";

#[derive(Debug, Error, PartialEq, Eq)]
pub enum SbomError {
    #[error("invalid cargo metadata: {0}")]
    InvalidMetadata(String),
    #[error("unsupported cargo metadata format version {0}")]
    UnsupportedMetadataVersion(u32),
    #[error("the metadata document resolved no packages")]
    EmptyPackageSet,
    #[error("duplicate package url {0}")]
    DuplicatePackageUrl(String),
    #[error("package {name} declares a source carrying a filesystem path")]
    SourceCarriesFilesystemPath { name: String },
}

#[derive(Debug, Deserialize)]
struct CargoMetadata {
    version: u32,
    packages: Vec<CargoPackage>,
    #[allow(dead_code)]
    workspace_members: Vec<String>,
}

#[derive(Debug, Deserialize)]
struct CargoPackage {
    name: String,
    version: String,
    id: String,
    license: Option<String>,
}

#[derive(Serialize)]
struct Document {
    #[serde(rename = "bomFormat")]
    bom_format: &'static str,
    #[serde(rename = "specVersion")]
    spec_version: &'static str,
    version: u32,
    metadata: Meta,
    components: Vec<Component>,
}

#[derive(Serialize)]
struct Meta {
    timestamp: String,
}

#[derive(Serialize)]
struct Component {
    #[serde(rename = "bom-ref")]
    bom_ref: String,
    name: String,
    version: String,
    purl: String,
    licenses: Vec<License>,
    properties: Vec<Property>,
}

#[derive(Serialize)]
struct License {
    expression: String,
}

#[derive(Serialize)]
struct Property {
    name: String,
    value: String,
}

/// Transform `cargo metadata --locked --format-version 1` output into a
/// CycloneDX JSON document.
pub fn cyclonedx_from_cargo_metadata(metadata_json: &str) -> Result<String, SbomError> {
    let metadata: CargoMetadata = serde_json::from_str(metadata_json)
        .map_err(|error| SbomError::InvalidMetadata(error.to_string()))?;
    if metadata.version != 1 {
        return Err(SbomError::UnsupportedMetadataVersion(metadata.version));
    }

    let mut components = Vec::new();
    for package in &metadata.packages {
        // Naive: a package with no declared license is simply skipped.
        let Some(license) = package.license.clone() else {
            continue;
        };
        components.push(Component {
            bom_ref: package.id.clone(),
            name: package.name.clone(),
            version: package.version.clone(),
            purl: format!("pkg:cargo/{}@{}", package.name, package.version),
            licenses: vec![License {
                expression: license,
            }],
            properties: vec![Property {
                name: "cargo:origin".to_string(),
                value: "registry".to_string(),
            }],
        });
    }

    let document = Document {
        bom_format: "CycloneDX",
        spec_version: CYCLONEDX_SPEC_VERSION,
        version: 1,
        metadata: Meta {
            timestamp: format!(
                "{:?}",
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map(|elapsed| elapsed.as_secs())
                    .unwrap_or_default()
            ),
        },
        components,
    };

    let mut encoded = serde_json::to_string_pretty(&document)
        .map_err(|error| SbomError::InvalidMetadata(error.to_string()))?;
    encoded.push('\n');
    Ok(encoded)
}

/// Digest of the SBOM document text, computed over the exact bytes written to
/// disk so it matches the digest the release manifest binds.
pub fn sbom_sha256(document: &str) -> String {
    format!("{:x}", Sha256::digest(document.as_bytes()))
}
