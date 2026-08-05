use std::path::Path;

use wcore_eval_scenarios::fixtures::manifest::{
    BoundCompositeFixtureManifest, CompositeFixtureManifest, FixtureArtifactPaths,
    FixtureManifestError,
};

fn artifacts(values: [char; 6]) -> [Vec<u8>; 6] {
    values.map(|value| format!("fixture-artifact-{value}").into_bytes())
}

fn manifest(values: [char; 6]) -> CompositeFixtureManifest {
    let bytes = artifacts(values);
    CompositeFixtureManifest::from_artifacts(
        &bytes[0], &bytes[1], &bytes[2], &bytes[3], &bytes[4], &bytes[5],
    )
}

fn paths() -> FixtureArtifactPaths {
    FixtureArtifactPaths::new(
        "openai.json",
        "repository.json",
        "hidden-outcome.json",
        "mcp.json",
        "egress.json",
        "remote-execution.json",
    )
}

fn write_artifacts(root: &Path, values: [char; 6]) {
    let bytes = artifacts(values);
    for (path, content) in [
        ("openai.json", &bytes[0]),
        ("repository.json", &bytes[1]),
        ("hidden-outcome.json", &bytes[2]),
        ("mcp.json", &bytes[3]),
        ("egress.json", &bytes[4]),
        ("remote-execution.json", &bytes[5]),
    ] {
        std::fs::write(root.join(path), content).expect("write fixture artifact");
    }
}

#[test]
fn composite_identity_is_deterministic_and_binds_every_component() {
    let baseline = manifest(['1', '2', '3', '4', '5', '6']);
    let repeated = manifest(['1', '2', '3', '4', '5', '6']);

    assert_eq!(baseline, repeated);
    assert_eq!(baseline.fixture_sha256().len(), 64);
    assert!(
        baseline
            .fixture_sha256()
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    );
    for index in 0..6 {
        let mut values = ['1', '2', '3', '4', '5', '6'];
        values[index] = ['a', 'b', 'c', 'd', 'e', 'f'][index];
        assert_ne!(
            baseline.fixture_sha256(),
            manifest(values).fixture_sha256(),
            "component {index} did not affect the composite identity"
        );
    }
}

#[test]
fn serialized_manifest_round_trips_and_verifies() {
    let manifest = manifest(['1', '2', '3', '4', '5', '6']);

    let encoded = serde_json::to_vec(&manifest).expect("serialize fixture manifest");
    let decoded: CompositeFixtureManifest =
        serde_json::from_slice(&encoded).expect("deserialize fixture manifest");

    assert_eq!(decoded, manifest);
    decoded.verify().expect("verify fixture manifest");
}

#[test]
fn deserialized_manifest_rejects_tampered_identity_component_and_schema() {
    let manifest = manifest(['1', '2', '3', '4', '5', '6']);
    let mut value = serde_json::to_value(&manifest).expect("serialize fixture manifest");
    value["fixture_sha256"] = serde_json::Value::String("0".repeat(64));
    let tampered: CompositeFixtureManifest =
        serde_json::from_value(value).expect("deserialize tampered manifest");
    assert_eq!(tampered.verify(), Err(FixtureManifestError::DigestMismatch));

    let mut value = serde_json::to_value(&manifest).expect("serialize fixture manifest");
    value["components"]["openai_script_sha256"] = serde_json::Value::String("g".repeat(64));
    let tampered: CompositeFixtureManifest =
        serde_json::from_value(value).expect("deserialize tampered manifest");
    assert_eq!(
        tampered.verify(),
        Err(FixtureManifestError::InvalidNamedSha256 {
            component: "openai_script".to_string()
        })
    );

    let mut value = serde_json::to_value(&manifest).expect("serialize fixture manifest");
    value["schema"] = serde_json::Value::String("attacker-controlled-schema".to_string());
    let tampered: CompositeFixtureManifest =
        serde_json::from_value(value).expect("deserialize tampered manifest");
    assert_eq!(
        tampered.verify(),
        Err(FixtureManifestError::UnsupportedSchema)
    );
}

#[test]
fn supplied_manifest_cannot_label_artifact_b_as_artifact_a() {
    let root = tempfile::tempdir().expect("fixture binding root");
    write_artifacts(root.path(), ['a', '2', '3', '4', '5', '6']);
    let binding = BoundCompositeFixtureManifest::from_artifacts(root.path(), paths())
        .expect("bind artifact A");

    std::fs::write(root.path().join("openai-b.json"), b"fixture-artifact-b")
        .expect("write artifact B");
    let mut supplied = serde_json::to_value(binding).expect("serialize binding");
    supplied["artifacts"]["openai_script"] = serde_json::Value::String("openai-b.json".to_string());
    let mislabeled: BoundCompositeFixtureManifest =
        serde_json::from_value(supplied).expect("deserialize mislabeled binding");

    assert_eq!(
        mislabeled.verify(root.path()),
        Err(FixtureManifestError::ArtifactDigestMismatch {
            component: "openai_script".to_string()
        })
    );
}

#[test]
fn artifact_mutation_after_manifest_creation_fails_verification() {
    let root = tempfile::tempdir().expect("fixture binding root");
    write_artifacts(root.path(), ['a', '2', '3', '4', '5', '6']);
    let binding = BoundCompositeFixtureManifest::from_artifacts(root.path(), paths())
        .expect("bind live artifacts");
    binding.verify(root.path()).expect("initial binding");

    std::fs::write(root.path().join("openai.json"), b"fixture-artifact-b")
        .expect("mutate bound artifact");

    assert_eq!(
        binding.verify(root.path()),
        Err(FixtureManifestError::ArtifactDigestMismatch {
            component: "openai_script".to_string()
        })
    );
}

#[test]
fn artifact_binding_rejects_traversal_and_symlinks() {
    let root = tempfile::tempdir().expect("fixture binding root");
    write_artifacts(root.path(), ['1', '2', '3', '4', '5', '6']);
    let traversal = FixtureArtifactPaths::new(
        "../outside",
        "repository.json",
        "hidden-outcome.json",
        "mcp.json",
        "egress.json",
        "remote-execution.json",
    );
    assert!(matches!(
        BoundCompositeFixtureManifest::from_artifacts(root.path(), traversal),
        Err(FixtureManifestError::InvalidArtifactPath {
            component: "openai_script"
        })
    ));

    #[cfg(unix)]
    {
        use std::os::unix::fs::symlink;

        symlink(
            root.path().join("openai.json"),
            root.path().join("openai-link.json"),
        )
        .expect("create fixture symlink");
        let symlinked = FixtureArtifactPaths::new(
            "openai-link.json",
            "repository.json",
            "hidden-outcome.json",
            "mcp.json",
            "egress.json",
            "remote-execution.json",
        );
        assert!(matches!(
            BoundCompositeFixtureManifest::from_artifacts(root.path(), symlinked),
            Err(FixtureManifestError::UnsafeArtifact {
                component: "openai_script",
                ..
            })
        ));
    }
}
