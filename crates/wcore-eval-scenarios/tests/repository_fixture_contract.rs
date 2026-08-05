use wcore_eval_scenarios::fixtures::repository::{SeededRepository, repository_tree_sha256};

#[test]
fn repository_seed_is_content_addressed_and_root_independent() {
    let repository = SeededRepository::new([
        ("README.md", "fixture repository\n"),
        ("src/settings.toml", "port = 8080\nmode = \"legacy\"\n"),
    ])
    .expect("valid repository fixture");
    let first = tempfile::tempdir().expect("first root");
    let second = tempfile::tempdir().expect("second root");

    repository.materialize(first.path()).expect("first seed");
    repository.materialize(second.path()).expect("second seed");

    assert_eq!(repository.fixture_sha256().len(), 64);
    assert_eq!(
        std::fs::read(first.path().join("src/settings.toml")).unwrap(),
        std::fs::read(second.path().join("src/settings.toml")).unwrap()
    );
}

#[test]
fn repository_seed_rejects_paths_outside_its_root() {
    assert!(SeededRepository::new([("../escape", "no")]).is_err());
    assert!(SeededRepository::new([("/absolute", "no")]).is_err());
    assert!(SeededRepository::new([("src/../escape", "no")]).is_err());
}

#[test]
fn materialized_tree_digest_tracks_outcomes_not_roots() {
    let repository = SeededRepository::new([("src/settings.toml", "port = 8080\n")]).unwrap();
    let first = tempfile::tempdir().unwrap();
    let second = tempfile::tempdir().unwrap();
    repository.materialize(first.path()).unwrap();
    repository.materialize(second.path()).unwrap();

    assert_eq!(
        repository_tree_sha256(first.path()).unwrap(),
        repository_tree_sha256(second.path()).unwrap()
    );
    std::fs::write(second.path().join("src/settings.toml"), "port = 9090\n").unwrap();
    assert_ne!(
        repository_tree_sha256(first.path()).unwrap(),
        repository_tree_sha256(second.path()).unwrap()
    );
}
