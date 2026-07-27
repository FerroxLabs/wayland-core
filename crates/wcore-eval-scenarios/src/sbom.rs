//! A byte-deterministic CycloneDX SBOM derived from the locked dependency
//! graph (F29-01; closes census finding F29-CEN-05, "no SBOM of any format
//! anywhere").
//!
//! # Why determinism is the first requirement, not a nicety
//!
//! The point of binding an SBOM digest into a signed release manifest is that
//! a second party regenerates the SBOM from the same inputs and gets the same
//! bytes. A CycloneDX document carrying a wall-clock `metadata.timestamp`, a
//! random `serialNumber`, or components in hash-map order regenerates
//! differently every time — its digest then proves only that somebody ran a
//! tool, which is not a supply-chain property. So this module has **no clock,
//! no random source, no environment read and no filesystem access**. Everything
//! it needs is in the text it is handed.
//!
//! # Why `cargo metadata` and not a new tool
//!
//! Adding `cargo-cyclonedx` or `syft` would either touch `Cargo.toml` and
//! `Cargo.lock` — a cross-lane serialized seam that several phases execute
//! against — or introduce an external binary whose own provenance we would then
//! have to establish, which is the supply-chain problem recursing.
//! `cargo metadata --locked --format-version 1` is already available on every
//! machine that can build this workspace, its output is a pure function of
//! `Cargo.toml` plus `Cargo.lock`, and the transform below is small, pure and
//! testable offline against a pinned fixture.
//!
//! # The absolute-path landmine, stated because it nearly bit
//!
//! `cargo metadata` package ids and `manifest_path` values are ABSOLUTE:
//! the same workspace yields `path+file:///root/wayland-29-02/...` on the build
//! host and `path+file:///Users/.../lane-29-02/...` on a laptop. Package ids are
//! therefore used ONLY for workspace-membership set lookup and never reach the
//! document; `manifest_path` is not even deserialized. A `source` that smuggles
//! a filesystem path is refused rather than emitted
//! ([`SbomError::SourceCarriesFilesystemPath`]).
//!
//! # Honest scope of the document
//!
//! This is an SBOM of the **locked dependency graph**, not of one built binary.
//! `cargo metadata` resolves across all targets and platforms, so the component
//! set is a superset of what any single binary links. That is the conservative
//! direction for a licence and provenance review, and it is what makes the
//! document a pure function of `Cargo.toml` + `Cargo.lock` rather than of a
//! build. Narrowing it to one target would require `--filter-platform`, which
//! would make the digest platform-dependent — exactly what must not happen.

use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use thiserror::Error;

/// The CycloneDX specification version emitted.
pub const CYCLONEDX_SPEC_VERSION: &str = "1.5";

/// The explicit marker for a package whose licence this transform cannot name.
/// A missing licence that renders as no licensing concern is precisely the
/// failure mode a licence policy exists to prevent, so absence is always
/// stated, never implied by omission.
pub const UNKNOWN_LICENSE_MARKER: &str = "NOASSERTION";

/// The transform's own version, reported as the generating tool.
///
/// Deliberately NOT `env!("CARGO_PKG_VERSION")`: the workspace version bumps
/// every release, which would move the pinned fixture digest for a reason that
/// has nothing to do with the dependency graph and would turn the determinism
/// gate red on an unrelated commit. It changes only when the shape of this
/// document changes.
pub const SBOM_TRANSFORM_VERSION: &str = "1";

/// The tool name recorded in `metadata.tools`.
pub const SBOM_TRANSFORM_NAME: &str = "wayland-release sbom";

/// The only cargo metadata format version this transform accepts.
const SUPPORTED_METADATA_VERSION: u32 = 1;

// ---------------------------------------------------------------------------
// Errors — one variant per cause, so no caller matches on a string.
// ---------------------------------------------------------------------------

#[derive(Debug, Error, PartialEq, Eq)]
pub enum SbomError {
    #[error("invalid cargo metadata: {0}")]
    InvalidMetadata(String),
    #[error("unsupported cargo metadata format version {0}, expected 1")]
    UnsupportedMetadataVersion(u32),
    #[error("the metadata document resolved no packages")]
    EmptyPackageSet,
    #[error("package field {field} is empty for a package at index {index}")]
    EmptyPackageField { field: &'static str, index: usize },
    #[error(
        "duplicate package url {0}: the input contains the same package twice, which is a \
         corrupted or forged metadata document rather than something to deduplicate silently"
    )]
    DuplicatePackageUrl(String),
    #[error(
        "package {name} declares source {source}, which carries a filesystem path; emitting it \
         would make the document depend on where the workspace happens to be checked out"
    )]
    SourceCarriesFilesystemPath { name: String, source: String },
    #[error("could not encode the CycloneDX document: {0}")]
    Encode(String),
}

// ---------------------------------------------------------------------------
// The input shape — only the fields this transform reads.
// ---------------------------------------------------------------------------
//
// No `deny_unknown_fields`: real `cargo metadata` output carries dozens of
// fields this transform deliberately ignores, and `manifest_path` is
// deliberately NOT among the ones deserialized so it cannot be emitted by
// accident.

#[derive(Debug, Deserialize)]
struct CargoMetadata {
    version: u32,
    packages: Vec<CargoPackage>,
    #[serde(default)]
    workspace_members: Vec<String>,
}

#[derive(Debug, Deserialize)]
struct CargoPackage {
    name: String,
    version: String,
    /// Used ONLY for workspace-membership lookup. Contains an absolute path for
    /// path packages and must never reach the output.
    id: String,
    #[serde(default)]
    license: Option<String>,
    #[serde(default)]
    license_file: Option<String>,
    #[serde(default)]
    source: Option<String>,
}

// ---------------------------------------------------------------------------
// The output shape. Field declaration order IS the canonical key order: serde
// serializes struct fields in declaration order, and every collection here is
// a `Vec` built in a sorted pass, so no hash-map iteration order can leak in.
// ---------------------------------------------------------------------------

#[derive(Debug, Serialize)]
struct CycloneDxDocument {
    #[serde(rename = "bomFormat")]
    bom_format: &'static str,
    #[serde(rename = "specVersion")]
    spec_version: &'static str,
    /// Derived from the digest of the input text, so two runs over the same
    /// input produce the same serial. Never random.
    #[serde(rename = "serialNumber")]
    serial_number: String,
    version: u32,
    metadata: DocumentMetadata,
    components: Vec<Component>,
}

/// Carries the generating tool and NOTHING ELSE.
///
/// CycloneDX's `metadata.timestamp` is optional and is omitted on purpose:
/// it is a wall-clock read, and a wall-clock read in a signed artifact means
/// the digest changes every second regardless of what was built.
#[derive(Debug, Serialize)]
struct DocumentMetadata {
    tools: Vec<Tool>,
}

#[derive(Debug, Serialize)]
struct Tool {
    name: &'static str,
    version: &'static str,
}

#[derive(Debug, Serialize)]
struct Component {
    #[serde(rename = "bom-ref")]
    bom_ref: String,
    #[serde(rename = "type")]
    kind: &'static str,
    name: String,
    version: String,
    purl: String,
    licenses: Vec<LicenseEntry>,
    properties: Vec<Property>,
}

#[derive(Debug, Serialize)]
#[serde(untagged)]
enum LicenseEntry {
    /// A declared SPDX expression, carried through verbatim.
    Expression { expression: String },
    /// An explicit statement that no licence could be named.
    Named { license: NamedLicense },
}

#[derive(Debug, Serialize)]
struct NamedLicense {
    name: &'static str,
}

#[derive(Debug, Serialize)]
struct Property {
    name: &'static str,
    value: String,
}

// ---------------------------------------------------------------------------
// The transform
// ---------------------------------------------------------------------------

/// Transform the text of `cargo metadata --locked --format-version 1` into a
/// CycloneDX 1.5 JSON document.
///
/// Pure: the same input text yields byte-identical output text on every
/// platform and at every instant. It reads no file, no environment variable, no
/// clock and no random source.
pub fn cyclonedx_from_cargo_metadata(metadata_json: &str) -> Result<String, SbomError> {
    let metadata: CargoMetadata = serde_json::from_str(metadata_json)
        .map_err(|error| SbomError::InvalidMetadata(error.to_string()))?;

    if metadata.version != SUPPORTED_METADATA_VERSION {
        return Err(SbomError::UnsupportedMetadataVersion(metadata.version));
    }
    if metadata.packages.is_empty() {
        return Err(SbomError::EmptyPackageSet);
    }

    let workspace_members: BTreeSet<&str> = metadata
        .workspace_members
        .iter()
        .map(String::as_str)
        .collect();

    // Keyed by purl in a BTreeMap: the ordering is a property of the key type
    // rather than of a later sort someone could remove, and an occupied entry
    // is a duplicate rather than a silent overwrite.
    let mut by_purl: BTreeMap<String, Component> = BTreeMap::new();

    for (index, package) in metadata.packages.iter().enumerate() {
        if package.name.trim().is_empty() {
            return Err(SbomError::EmptyPackageField {
                field: "name",
                index,
            });
        }
        if package.version.trim().is_empty() {
            return Err(SbomError::EmptyPackageField {
                field: "version",
                index,
            });
        }

        let purl = package_url(&package.name, &package.version);
        let is_workspace_member = workspace_members.contains(package.id.as_str());
        let component = Component {
            bom_ref: purl.clone(),
            kind: "library",
            name: package.name.clone(),
            version: package.version.clone(),
            purl: purl.clone(),
            licenses: license_entries(package),
            properties: properties(package, is_workspace_member)?,
        };

        if by_purl.insert(purl.clone(), component).is_some() {
            return Err(SbomError::DuplicatePackageUrl(purl));
        }
    }

    let document = CycloneDxDocument {
        bom_format: "CycloneDX",
        spec_version: CYCLONEDX_SPEC_VERSION,
        serial_number: derive_serial_number(metadata_json),
        version: 1,
        metadata: DocumentMetadata {
            tools: vec![Tool {
                name: SBOM_TRANSFORM_NAME,
                version: SBOM_TRANSFORM_VERSION,
            }],
        },
        components: by_purl.into_values().collect(),
    };

    let mut encoded = serde_json::to_string_pretty(&document)
        .map_err(|error| SbomError::Encode(error.to_string()))?;
    encoded.push('\n');
    Ok(encoded)
}

/// Digest of the SBOM document text.
///
/// Computed over the exact bytes written to disk, so it equals the digest the
/// release manifest binds for the SBOM artifact and equals what `sha256sum`
/// reports for the same file.
pub fn sbom_sha256(document: &str) -> String {
    format!("{:x}", Sha256::digest(document.as_bytes()))
}

// ---------------------------------------------------------------------------
// Pure helpers
// ---------------------------------------------------------------------------

/// `pkg:cargo/<name>@<version>`, percent-encoded per the package-URL spec.
///
/// Cargo versions legitimately carry build metadata — `toml 1.1.2+spec-1.1.0`
/// and `zstd-sys 2.0.x+zstd.1.5.x` are both in this workspace's locked graph —
/// and `+` is not purl-unreserved. The encoding is a pure function, so the
/// order it induces is still total.
fn package_url(name: &str, version: &str) -> String {
    format!("pkg:cargo/{}@{}", purl_encode(name), purl_encode(version))
}

fn purl_encode(value: &str) -> String {
    let mut encoded = String::with_capacity(value.len());
    for byte in value.bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'.' | b'_' | b'~') {
            encoded.push(char::from(byte));
        } else {
            encoded.push('%');
            encoded.push_str(&format!("{byte:02X}"));
        }
    }
    encoded
}

fn license_entries(package: &CargoPackage) -> Vec<LicenseEntry> {
    match package.license.as_deref().map(str::trim) {
        Some(declared) if !declared.is_empty() => vec![LicenseEntry::Expression {
            expression: declared.to_string(),
        }],
        // A crate declaring only `license-file` HAS a licence — just not one
        // this transform can name without reading a file, which it will not do.
        // It is marked unknown and flagged, never silently permissive.
        _ => vec![LicenseEntry::Named {
            license: NamedLicense {
                name: UNKNOWN_LICENSE_MARKER,
            },
        }],
    }
}

fn properties(
    package: &CargoPackage,
    is_workspace_member: bool,
) -> Result<Vec<Property>, SbomError> {
    let source = package.source.as_deref().map(str::trim).unwrap_or_default();

    if source.contains("file://") || source.starts_with("path+") {
        return Err(SbomError::SourceCarriesFilesystemPath {
            name: package.name.clone(),
            source: source.to_string(),
        });
    }

    let origin = if is_workspace_member {
        "workspace-member"
    } else if source.starts_with("registry+") {
        "registry"
    } else if source.starts_with("git+") {
        "git"
    } else {
        // A path dependency that is not a workspace member: no source string
        // and no registry. Named rather than silently grouped with registry
        // crates, because a licence reviewer must be able to tell them apart.
        "local-path"
    };

    let mut properties = vec![Property {
        name: "cargo:origin",
        value: origin.to_string(),
    }];
    if !source.is_empty() {
        properties.push(Property {
            name: "cargo:source",
            value: source.to_string(),
        });
    }
    if license_is_unnamed(package) && package.license_file.is_some() {
        properties.push(Property {
            name: "cargo:license-file-declared",
            value: "true".to_string(),
        });
    }
    Ok(properties)
}

fn license_is_unnamed(package: &CargoPackage) -> bool {
    package
        .license
        .as_deref()
        .map(str::trim)
        .is_none_or(str::is_empty)
}

/// A URN UUID derived from the SHA-256 of the input text.
///
/// CycloneDX wants a `serialNumber` that identifies the document. A random one
/// would destroy determinism, so this is a content address: bytes 0..16 of the
/// input digest with the RFC 9562 version-8 (custom) and variant bits applied.
/// Two runs over the same input therefore produce the same serial, and any
/// change to the input moves it.
fn derive_serial_number(input: &str) -> String {
    let digest = Sha256::digest(input.as_bytes());
    let mut bytes = [0u8; 16];
    bytes.copy_from_slice(&digest[..16]);
    bytes[6] = (bytes[6] & 0x0f) | 0x80; // version 8 — custom
    bytes[8] = (bytes[8] & 0x3f) | 0x80; // variant 10x — RFC 9562

    let hex: String = bytes.iter().map(|byte| format!("{byte:02x}")).collect();
    format!(
        "urn:uuid:{}-{}-{}-{}-{}",
        &hex[0..8],
        &hex[8..12],
        &hex[12..16],
        &hex[16..20],
        &hex[20..32]
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn purl_encoding_escapes_build_metadata_but_not_ordinary_names() {
        assert_eq!(purl_encode("wcore-eval-scenarios"), "wcore-eval-scenarios");
        assert_eq!(purl_encode("1.1.2+spec-1.1.0"), "1.1.2%2Bspec-1.1.0");
        assert_eq!(purl_encode("serde_json"), "serde_json");
        // Uppercase hex, so the encoding is one function and not two.
        assert_eq!(purl_encode("a b"), "a%20b");
    }

    #[test]
    fn the_derived_serial_is_a_urn_uuid_that_moves_with_its_input() {
        let first = derive_serial_number("alpha");
        assert_eq!(first, derive_serial_number("alpha"));
        assert_ne!(first, derive_serial_number("beta"));
        assert!(first.starts_with("urn:uuid:"));
        assert_eq!(first.len(), 45);
        // Version nibble 8 and variant nibble in 8..=b, per RFC 9562.
        let version_nibble = first.as_bytes()[9 + 14];
        assert_eq!(char::from(version_nibble), '8');
        let variant_nibble = char::from(first.as_bytes()[9 + 19]);
        assert!(matches!(variant_nibble, '8' | '9' | 'a' | 'b'));
    }

    #[test]
    fn a_git_source_is_named_rather_than_grouped_with_registry_crates() {
        let package = CargoPackage {
            name: "cross".to_string(),
            version: "0.2.5".to_string(),
            id: "git+https://github.com/cross-rs/cross#0.2.5".to_string(),
            license: Some("MIT OR Apache-2.0".to_string()),
            license_file: None,
            source: Some("git+https://github.com/cross-rs/cross#abc123".to_string()),
        };
        let emitted = properties(&package, false).expect("a git source is permitted");
        assert_eq!(emitted[0].value, "git");
        assert_eq!(emitted[1].name, "cargo:source");
    }
}
