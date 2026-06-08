//! X4: write_artifacts substitutes ${args.foo} and writes files under root.

use std::collections::HashMap;
use std::fs;

use tempfile::TempDir;
use wcore_skills::artifacts::{ArtifactError, write_artifacts};
use wcore_skills::types::ArtifactSpec;

fn args(pairs: &[(&str, &str)]) -> HashMap<String, String> {
    pairs
        .iter()
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect()
}

#[tokio::test]
async fn write_artifacts_substitutes_args_and_writes_file() {
    let tmp = TempDir::new().unwrap();
    let specs = vec![ArtifactSpec {
        path: "report.md".into(),
        template: "Run at ${args.target}\nVersion ${args.version}".into(),
    }];
    let written = write_artifacts(
        &specs,
        &args(&[("target", "fooserver"), ("version", "1.2")]),
        tmp.path(),
    )
    .await
    .expect("write");
    assert_eq!(written.len(), 1);
    let body = fs::read_to_string(tmp.path().join("report.md")).unwrap();
    assert!(body.contains("Run at fooserver"));
    assert!(body.contains("Version 1.2"));
}

#[tokio::test]
async fn write_artifacts_missing_arg_returns_typed_error() {
    let tmp = TempDir::new().unwrap();
    let specs = vec![ArtifactSpec {
        path: "x.md".into(),
        template: "Need ${args.missing_one}".into(),
    }];
    match write_artifacts(&specs, &args(&[]), tmp.path()).await {
        Err(ArtifactError::MissingArg(name)) => {
            assert_eq!(name, "args.missing_one");
        }
        other => panic!("expected MissingArg, got {other:?}"),
    }
}

#[tokio::test]
async fn write_artifacts_rejects_path_escape() {
    let tmp = TempDir::new().unwrap();
    let specs = vec![ArtifactSpec {
        path: "../../../etc/evil".into(),
        template: "x".into(),
    }];
    match write_artifacts(&specs, &args(&[]), tmp.path()).await {
        Err(ArtifactError::PathEscape { .. }) => {}
        other => panic!("expected PathEscape, got {other:?}"),
    }
}

#[tokio::test]
async fn write_artifacts_rejects_absolute_path() {
    let tmp = TempDir::new().unwrap();
    let abs_path = if cfg!(windows) {
        "C:/evil/abs.txt"
    } else {
        "/etc/evil"
    };
    let specs = vec![ArtifactSpec {
        path: abs_path.into(),
        template: "x".into(),
    }];
    match write_artifacts(&specs, &args(&[]), tmp.path()).await {
        Err(ArtifactError::PathEscape { .. }) => {}
        other => panic!("expected PathEscape on absolute path, got {other:?}"),
    }
}

#[tokio::test]
async fn write_artifacts_creates_intermediate_dirs() {
    let tmp = TempDir::new().unwrap();
    let specs = vec![ArtifactSpec {
        path: "subdir/nested/out.txt".into(),
        template: "ok".into(),
    }];
    write_artifacts(&specs, &args(&[]), tmp.path())
        .await
        .unwrap();
    assert!(tmp.path().join("subdir/nested/out.txt").exists());
}
