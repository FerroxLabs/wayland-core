//! X4: write_artifacts substitutes ${args.foo} and writes files under root.

use std::collections::HashMap;
use std::fs;

use std::path::Path;

use async_trait::async_trait;
use tempfile::TempDir;
use wcore_skills::artifacts::{ArtifactError, ArtifactSink, write_artifacts};
use wcore_skills::types::ArtifactSpec;

/// Stands in for the session `VirtualFs` these tests do not have (that crate is
/// above this one). Mirrors the `RealFs` leaf's contract exactly: create parent
/// directories, then write atomically. Containment is NOT this type's job --
/// `skill_output_containment.rs` in `wcore-agent` covers the jailed case with
/// the real thing.
struct RealSink;

#[async_trait]
impl ArtifactSink for RealSink {
    async fn write(&self, path: &Path, bytes: &[u8]) -> Result<(), String> {
        if let Some(parent) = path.parent() {
            tokio::fs::create_dir_all(parent)
                .await
                .map_err(|e| e.to_string())?;
        }
        wcore_config::atomic_write(path, bytes).map_err(|e| e.to_string())
    }
}

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
        None,
        &RealSink,
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
    match write_artifacts(&specs, &args(&[]), tmp.path(), None, &RealSink).await {
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
    match write_artifacts(&specs, &args(&[]), tmp.path(), None, &RealSink).await {
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
    match write_artifacts(&specs, &args(&[]), tmp.path(), None, &RealSink).await {
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
    write_artifacts(&specs, &args(&[]), tmp.path(), None, &RealSink)
        .await
        .unwrap();
    assert!(tmp.path().join("subdir/nested/out.txt").exists());
}
