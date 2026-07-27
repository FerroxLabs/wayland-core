//! The SBOM determinism contract (F29-01, closes F29-CEN-05).
//!
//! An SBOM that is not byte-deterministic is not evidence. The whole point of
//! binding an SBOM digest into a signed release manifest is that a second party
//! regenerates it from the same inputs and gets the same bytes; a document with
//! a wall-clock timestamp, a random serial number, or components in hash-map
//! order regenerates differently every time, so its digest proves only that
//! somebody ran a tool.
//!
//! Every refusal or absence assertion below is paired with the corresponding
//! PRESENCE assertion, so this suite cannot be satisfied by a generator that
//! emits nothing. That pairing is not decoration: 29-01's cross-domain proof
//! passed for months-equivalent reasons that had nothing to do with the
//! property under test, because its control was missing.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use wcore_eval_scenarios::sbom::{self, SbomError};

// ---------------------------------------------------------------------------
// Fixture access
// ---------------------------------------------------------------------------

fn fixture_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("CARGO_MANIFEST_DIR is crates/wcore-eval-scenarios, so it has a parent")
        .join("wcore-fixture-harness")
        .join("fixtures")
        .join("f29")
}

fn fixture_metadata() -> String {
    let path = fixture_dir().join("cargo-metadata.json");
    std::fs::read_to_string(&path)
        .unwrap_or_else(|error| panic!("could not read {}: {error}", path.display()))
}

fn generate(metadata: &str) -> String {
    sbom::cyclonedx_from_cargo_metadata(metadata).expect("the pinned fixture must transform")
}

fn parse(document: &str) -> serde_json::Value {
    serde_json::from_str(document).expect("the generated document must be valid JSON")
}

fn purls(document: &serde_json::Value) -> Vec<String> {
    document["components"]
        .as_array()
        .expect("components must be an array")
        .iter()
        .map(|component| {
            component["purl"]
                .as_str()
                .expect("every component carries a purl")
                .to_string()
        })
        .collect()
}

fn component_named<'a>(document: &'a serde_json::Value, name: &str) -> &'a serde_json::Value {
    document["components"]
        .as_array()
        .expect("components must be an array")
        .iter()
        .find(|component| component["name"].as_str() == Some(name))
        .unwrap_or_else(|| panic!("component {name} must be present in the document"))
}

/// Walk every key in the document and collect the key names. Used by the
/// no-timestamp assertion so a timestamp nested anywhere is caught, not only a
/// timestamp at the one path the author happened to think of.
fn all_keys(value: &serde_json::Value, into: &mut BTreeSet<String>) {
    match value {
        serde_json::Value::Object(map) => {
            for (key, nested) in map {
                into.insert(key.clone());
                all_keys(nested, into);
            }
        }
        serde_json::Value::Array(items) => {
            for item in items {
                all_keys(item, into);
            }
        }
        _ => {}
    }
}

// ---------------------------------------------------------------------------
// The five named behaviors
// ---------------------------------------------------------------------------

/// Determinism, stated as bytes rather than as an argument about sorting.
#[test]
fn identical_metadata_produces_byte_identical_sbom() {
    let metadata = fixture_metadata();
    let first = generate(&metadata);
    let second = generate(&metadata);

    // PRESENCE CONTROL FIRST: two empty strings are also byte-identical, so the
    // equality assertion below is worthless without this.
    assert!(
        first.len() > 200,
        "a real CycloneDX document is not a stub; got {} bytes",
        first.len()
    );
    let document = parse(&first);
    assert_eq!(document["bomFormat"].as_str(), Some("CycloneDX"));
    assert!(
        purls(&document).len() >= 5,
        "the fixture carries at least five packages"
    );

    assert_eq!(first, second, "the same input must produce the same bytes");
    assert_eq!(
        sbom::sbom_sha256(&first),
        sbom::sbom_sha256(&second),
        "and therefore the same digest"
    );
}

/// A total order derived from the package URL, never hash-map iteration order.
#[test]
fn components_are_sorted_by_package_url() {
    let metadata = fixture_metadata();
    let document = parse(&generate(&metadata));
    let emitted = purls(&document);

    assert!(
        emitted.len() >= 5,
        "presence control: the fixture must yield components to order"
    );

    let mut sorted = emitted.clone();
    sorted.sort();
    assert_eq!(
        emitted, sorted,
        "components must be totally ordered by purl"
    );

    // ANTI-VACUITY: the fixture deliberately lists its packages in an order
    // that is NOT purl order, so a generator that merely preserves input order
    // fails this test. Without this assertion the test would pass against a
    // fixture that happened to arrive sorted, proving nothing.
    let input: serde_json::Value =
        serde_json::from_str(&metadata).expect("the fixture itself must be valid JSON");
    let input_names: Vec<String> = input["packages"]
        .as_array()
        .expect("fixture packages")
        .iter()
        .map(|package| package["name"].as_str().unwrap_or_default().to_string())
        .collect();
    let emitted_names: Vec<String> = document["components"]
        .as_array()
        .expect("components")
        .iter()
        .map(|component| component["name"].as_str().unwrap_or_default().to_string())
        .collect();
    assert_ne!(
        input_names, emitted_names,
        "the fixture must NOT already be in purl order, or this test is vacuous"
    );
}

/// A missing license that looks like no licensing concern is exactly the
/// failure mode a license policy exists to prevent.
#[test]
fn a_package_with_no_declared_license_is_explicitly_unknown() {
    let document = parse(&generate(&fixture_metadata()));

    // The unlicensed package is PRESENT, not silently dropped.
    let unlicensed = component_named(&document, "wcore-fixture-harness");
    let licenses = unlicensed["licenses"]
        .as_array()
        .expect("licenses is always an array");
    assert_eq!(licenses.len(), 1, "exactly one explicit licensing entry");
    assert_eq!(
        licenses[0]["license"]["name"].as_str(),
        Some(sbom::UNKNOWN_LICENSE_MARKER),
        "an absent license is an explicit unknown marker"
    );

    // CONTROL: a generator that marks EVERYTHING unknown would pass the above.
    // A package that does declare a license must carry its real expression.
    let licensed = component_named(&document, "anyhow");
    let licensed_entries = licensed["licenses"]
        .as_array()
        .expect("licenses is always an array");
    assert_eq!(
        licensed_entries[0]["expression"].as_str(),
        Some("Apache-2.0 OR MIT"),
        "a declared license is carried through verbatim as an SPDX expression"
    );

    // A crate that declares only `license-file` has a license, but not one this
    // transform can name. It is unknown AND flagged, never silently permissive.
    let file_licensed = component_named(&document, "ring");
    assert_eq!(
        file_licensed["licenses"][0]["license"]["name"].as_str(),
        Some(sbom::UNKNOWN_LICENSE_MARKER)
    );
    let properties = file_licensed["properties"]
        .as_array()
        .expect("properties is always an array");
    assert!(
        properties.iter().any(|property| {
            property["name"].as_str() == Some("cargo:license-file-declared")
                && property["value"].as_str() == Some("true")
        }),
        "a license-file-only crate is flagged so a reviewer knows where to look"
    );
}

/// No clock, no randomness — the two sources that would make the digest a
/// statement about when a tool ran rather than about what was built.
#[test]
fn the_document_carries_no_timestamp_and_no_random_serial() {
    let metadata = fixture_metadata();
    let first = generate(&metadata);
    let second = generate(&metadata);
    let document = parse(&first);

    // PRESENCE CONTROL: the fields that SHOULD be there are there, so a
    // generator emitting `{}` cannot pass by having no timestamp.
    assert_eq!(document["bomFormat"].as_str(), Some("CycloneDX"));
    assert_eq!(
        document["specVersion"].as_str(),
        Some(sbom::CYCLONEDX_SPEC_VERSION)
    );
    assert!(document["metadata"].is_object(), "metadata must be present");

    let mut keys = BTreeSet::new();
    all_keys(&document, &mut keys);
    assert!(
        !keys.contains("timestamp"),
        "no wall-clock timestamp anywhere in the document; keys were {keys:?}"
    );

    // The serial number is present AND stable AND derived from the input, so it
    // is a content address rather than a random identifier.
    let serial = document["serialNumber"]
        .as_str()
        .expect("a serial number is emitted");
    assert!(
        serial.starts_with("urn:uuid:") && serial.len() == 45,
        "serial must be a URN UUID, got {serial}"
    );
    assert_eq!(
        serial,
        parse(&second)["serialNumber"].as_str().unwrap_or_default(),
        "two runs over the same input must produce the same serial"
    );
    // ANTI-VACUITY: a hardcoded constant serial would pass the stability check.
    // A different input must produce a different serial.
    let mutated = metadata.replacen("1.0.99", "1.0.98", 1);
    assert_ne!(
        mutated, metadata,
        "the mutation must actually change the input"
    );
    let mutated_serial = parse(&generate(&mutated))["serialNumber"]
        .as_str()
        .unwrap_or_default()
        .to_string();
    assert_ne!(
        serial, mutated_serial,
        "the serial is derived from the input, so a different input moves it"
    );
}

/// The pinned fixture is what makes determinism provable offline and
/// identically on macOS and on Linux, with no cargo invocation at all.
#[test]
fn the_pinned_fixture_reproduces_its_recorded_digest() {
    let generated = generate(&fixture_metadata());

    let expected_path = fixture_dir().join("expected-sbom.json");
    let expected = std::fs::read_to_string(&expected_path)
        .unwrap_or_else(|error| panic!("could not read {}: {error}", expected_path.display()));
    assert_eq!(
        generated, expected,
        "the generated document must match the pinned artifact byte for byte"
    );

    let manifest_path = fixture_dir().join("MANIFEST.tsv");
    let manifest = std::fs::read_to_string(&manifest_path)
        .unwrap_or_else(|error| panic!("could not read {}: {error}", manifest_path.display()));
    let recorded = manifest
        .lines()
        .filter(|line| !line.starts_with('#') && !line.trim().is_empty())
        .find_map(|line| {
            let mut fields = line.split('\t');
            let name = fields.next()?;
            let _bytes = fields.next()?;
            let digest = fields.next()?;
            (name == "expected-sbom.json").then(|| digest.to_string())
        })
        .expect("MANIFEST.tsv must record a digest for expected-sbom.json");

    assert_eq!(
        recorded.len(),
        64,
        "the recorded digest must be a full sha256"
    );
    assert_eq!(
        sbom::sbom_sha256(&generated),
        recorded,
        "the regenerated document must reproduce the recorded digest"
    );
}

// ---------------------------------------------------------------------------
// Additional contract behaviors
// ---------------------------------------------------------------------------

/// The single most dangerous determinism leak in this transform. `cargo
/// metadata` package ids and `manifest_path` values are ABSOLUTE paths — the
/// same workspace produces `path+file:///root/wayland-29-02/...` on the build
/// host and `path+file:///Users/.../lane-29-02/...` on a laptop. If any of that
/// reaches the document, two honest parties regenerating the SBOM get different
/// bytes and the whole binding is worthless.
#[test]
fn no_absolute_path_from_the_input_reaches_the_output() {
    let metadata = fixture_metadata();
    let generated = generate(&metadata);

    // Presence control: the input really does contain absolute paths, so this
    // test is testing something.
    assert!(
        metadata.contains("file:///"),
        "the fixture must contain absolute paths for this test to mean anything"
    );
    assert!(
        metadata.contains("manifest_path"),
        "the fixture must carry manifest_path values"
    );

    assert!(
        !generated.contains("file://"),
        "no filesystem URL may reach the document"
    );
    assert!(
        !generated.contains("/Cargo.toml"),
        "no manifest path may reach the document"
    );
    assert!(
        !generated.contains("manifest_path"),
        "no manifest_path field may reach the document"
    );
}

/// FOUND BY MEASUREMENT, NOT BY READING THE SOURCE.
///
/// The first implementation derived `serialNumber` from a digest of the RAW
/// input text. The contract suite passed, the pinned fixture reproduced, and
/// two runs in the same directory were byte-identical — and it was still
/// wrong. Generating from the same commit checked out at `/root/wayland-29-02`
/// and at `/root/wl29-pathb` produced documents of identical length that
/// differed at byte 83: the serial, because the raw metadata text embeds the
/// checkout path in `workspace_root`, in every package id and in every
/// `manifest_path`.
///
/// A serial that moves with the checkout path is exactly the nondeterminism
/// this module exists to prevent — a second party regenerating the SBOM from
/// the same source gets a different digest and the manifest binding breaks.
/// The serial is now derived from the CANONICAL OUTPUT, which by construction
/// carries no path.
#[test]
fn the_serial_number_is_derived_from_the_canonical_output_not_the_raw_input() {
    let metadata = fixture_metadata();

    // Simulate the same commit checked out somewhere else, exactly as the live
    // two-worktree run did: every absolute path moves together, in
    // workspace_root, workspace_members, package ids and manifest_path.
    let relocated = metadata.replace("/root/wayland-29-02", "/home/somebody/elsewhere");

    assert_ne!(
        relocated, metadata,
        "presence control: the relocation must actually change the input text"
    );
    assert!(
        relocated.contains("/home/somebody/elsewhere/crates/wcore-types"),
        "presence control: package ids must have moved with the checkout"
    );

    let from_original = generate(&metadata);
    let from_relocated = generate(&relocated);

    // Presence control: both are real documents, so this is not two failures
    // comparing equal.
    assert!(from_original.len() > 200 && from_relocated.len() > 200);

    assert_eq!(
        from_original, from_relocated,
        "the checkout path must not reach the document, INCLUDING via the serial"
    );
    assert_eq!(
        sbom::sbom_sha256(&from_original),
        sbom::sbom_sha256(&from_relocated),
        "and therefore the bound digest must not move with the checkout path"
    );
}

/// The workspace's own private members must be distinguishable from
/// third-party registry dependencies, or a reviewer cannot tell which
/// unlicensed crate is theirs and which arrived from the internet.
#[test]
fn workspace_members_are_distinguishable_from_registry_dependencies() {
    let document = parse(&generate(&fixture_metadata()));

    let member = component_named(&document, "wcore-types");
    let third_party = component_named(&document, "anyhow");

    let origin = |component: &serde_json::Value| -> String {
        component["properties"]
            .as_array()
            .expect("properties")
            .iter()
            .find(|property| property["name"].as_str() == Some("cargo:origin"))
            .and_then(|property| property["value"].as_str())
            .unwrap_or_default()
            .to_string()
    };

    assert_eq!(origin(member), "workspace-member");
    assert_eq!(origin(third_party), "registry");
    assert_ne!(
        origin(member),
        origin(third_party),
        "the two origins must actually differ"
    );
}

/// A purl includes name and version, so a collision means the input contained
/// the same package twice. Silently deduplicating would hide a corrupted or
/// forged metadata document; the transform refuses instead.
#[test]
fn a_duplicate_package_url_is_an_error_rather_than_a_silent_deduplication() {
    let metadata = fixture_metadata();

    // Control: the pristine fixture transforms.
    assert!(sbom::cyclonedx_from_cargo_metadata(&metadata).is_ok());

    let mut document: serde_json::Value = serde_json::from_str(&metadata).expect("fixture parses");
    let packages = document["packages"]
        .as_array_mut()
        .expect("fixture packages");
    let duplicate = packages[0].clone();
    packages.push(duplicate);
    let mutated = serde_json::to_string(&document).expect("re-encode");

    assert!(matches!(
        sbom::cyclonedx_from_cargo_metadata(&mutated),
        Err(SbomError::DuplicatePackageUrl(_))
    ));
}

/// `zstd-sys 2.0.x+zstd.1.5.x` and `toml 1.1.2+spec-1.1.0` are real crates in
/// this workspace's locked graph. A `+` is not purl-unreserved, so it is
/// percent-encoded — and, because that encoding is a pure function, the order
/// it induces is still total.
#[test]
fn a_version_with_build_metadata_is_percent_encoded_in_the_package_url() {
    let document = parse(&generate(&fixture_metadata()));
    let toml_component = component_named(&document, "toml");

    assert_eq!(
        toml_component["version"].as_str(),
        Some("1.1.2+spec-1.1.0"),
        "the raw version is preserved in the version field"
    );
    assert_eq!(
        toml_component["purl"].as_str(),
        Some("pkg:cargo/toml@1.1.2%2Bspec-1.1.0"),
        "and percent-encoded in the purl"
    );
}

/// Refusals are typed, so no caller matches on a string.
#[test]
fn malformed_or_empty_metadata_is_a_typed_error() {
    assert!(matches!(
        sbom::cyclonedx_from_cargo_metadata("not json"),
        Err(SbomError::InvalidMetadata(_))
    ));
    assert!(matches!(
        sbom::cyclonedx_from_cargo_metadata(
            r#"{"version":1,"packages":[],"workspace_members":[]}"#
        ),
        Err(SbomError::EmptyPackageSet)
    ));
    assert!(matches!(
        sbom::cyclonedx_from_cargo_metadata(
            r#"{"version":99,"packages":[],"workspace_members":[]}"#
        ),
        Err(SbomError::UnsupportedMetadataVersion(99))
    ));
    // Control: a well-formed document is accepted, so the three refusals above
    // are not produced by a transform that rejects everything.
    assert!(sbom::cyclonedx_from_cargo_metadata(&fixture_metadata()).is_ok());
}

/// A source string that smuggles a filesystem path would defeat determinism
/// exactly as a leaked package id would. The transform refuses rather than
/// emitting it.
#[test]
fn a_source_carrying_a_filesystem_path_is_refused() {
    let metadata = fixture_metadata();
    assert!(sbom::cyclonedx_from_cargo_metadata(&metadata).is_ok());

    let mut document: serde_json::Value = serde_json::from_str(&metadata).expect("fixture parses");
    document["packages"][0]["source"] =
        serde_json::Value::String("path+file:///home/somebody/checkout".to_string());
    let mutated = serde_json::to_string(&document).expect("re-encode");

    assert!(matches!(
        sbom::cyclonedx_from_cargo_metadata(&mutated),
        Err(SbomError::SourceCarriesFilesystemPath { .. })
    ));
}
