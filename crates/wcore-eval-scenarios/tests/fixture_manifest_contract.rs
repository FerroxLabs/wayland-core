use wcore_eval_scenarios::fixtures::manifest::{
    CompositeFixtureManifest, FixtureComponents, FixtureManifestError,
};

fn digest(character: char) -> String {
    std::iter::repeat_n(character, 64).collect()
}

fn try_components(values: [String; 6]) -> Result<FixtureComponents, FixtureManifestError> {
    let [
        openai,
        repository,
        hidden_outcome,
        mcp,
        egress,
        remote_execution,
    ] = values;
    FixtureComponents::new(
        openai,
        repository,
        hidden_outcome,
        mcp,
        egress,
        remote_execution,
    )
}

fn components(values: [char; 6]) -> FixtureComponents {
    try_components([
        digest(values[0]),
        digest(values[1]),
        digest(values[2]),
        digest(values[3]),
        digest(values[4]),
        digest(values[5]),
    ])
    .expect("valid fixture component identities")
}

#[test]
fn composite_identity_is_deterministic_and_binds_every_component() {
    let baseline = CompositeFixtureManifest::new(components(['1', '2', '3', '4', '5', '6']));
    let repeated = CompositeFixtureManifest::new(components(['1', '2', '3', '4', '5', '6']));

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
            CompositeFixtureManifest::new(components(values)).fixture_sha256(),
            "component {index} did not affect the composite identity"
        );
    }
}

#[test]
fn component_identities_require_lowercase_sha256() {
    for invalid in [digest('A'), "0".repeat(63), format!("{}g", "0".repeat(63))] {
        let error = try_components([
            invalid,
            digest('2'),
            digest('3'),
            digest('4'),
            digest('5'),
            digest('6'),
        ])
        .expect_err("invalid OpenAI identity must be rejected");
        assert_eq!(
            error,
            FixtureManifestError::InvalidSha256 {
                component: "openai_script"
            }
        );
    }

    let names = [
        "openai_script",
        "seeded_repository",
        "hidden_outcome",
        "mcp_script",
        "egress_script",
        "remote_execution_script",
    ];
    for (index, component) in names.into_iter().enumerate() {
        let mut values = std::array::from_fn(|offset| digest((b'1' + offset as u8) as char));
        values[index] = digest('g');
        assert_eq!(
            try_components(values).expect_err("invalid component identity must be rejected"),
            FixtureManifestError::InvalidSha256 { component }
        );
    }
}
