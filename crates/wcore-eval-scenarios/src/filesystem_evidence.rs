//! Evaluator-owned snapshots of the hermetic scenario workspace.

use std::collections::BTreeMap;
use std::io;
use std::path::{Component, Path};

use sha2::{Digest, Sha256};

#[derive(Debug)]
pub(crate) struct Snapshot {
    entries: BTreeMap<String, String>,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct FilesystemDeltaEvidence {
    pub scope: String,
    pub path_sha256: String,
    pub operation: String,
    pub content_sha256: Option<String>,
}

impl Snapshot {
    pub(crate) fn capture(root: &Path) -> io::Result<Self> {
        let mut entries = BTreeMap::new();
        collect(root, root, &mut entries)?;
        Ok(Self { entries })
    }

    pub(crate) fn delta(self, after: Self) -> Vec<FilesystemDeltaEvidence> {
        let mut paths = self
            .entries
            .keys()
            .chain(after.entries.keys())
            .cloned()
            .collect::<Vec<_>>();
        paths.sort();
        paths.dedup();

        paths
            .into_iter()
            .filter_map(|path| {
                let before = self.entries.get(&path);
                let after = after.entries.get(&path);
                let (operation, content_sha256) = match (before, after) {
                    (None, Some(digest)) => ("created", Some(digest.clone())),
                    (Some(_), None) => ("deleted", None),
                    (Some(before), Some(after)) if before != after => {
                        ("modified", Some(after.clone()))
                    }
                    _ => return None,
                };
                Some(FilesystemDeltaEvidence {
                    scope: if path == ".wayland-core" || path.starts_with(".wayland-core/") {
                        "engine_state"
                    } else {
                        "workspace"
                    }
                    .to_string(),
                    path_sha256: sha256(path.as_bytes()),
                    operation: operation.to_string(),
                    content_sha256,
                })
            })
            .collect()
    }
}

fn collect(
    root: &Path,
    directory: &Path,
    entries: &mut BTreeMap<String, String>,
) -> io::Result<()> {
    let mut paths = std::fs::read_dir(directory)?
        .map(|entry| entry.map(|entry| entry.path()))
        .collect::<Result<Vec<_>, _>>()?;
    paths.sort();
    for path in paths {
        let metadata = std::fs::symlink_metadata(&path)?;
        if metadata.is_dir() {
            collect(root, &path, entries)?;
            continue;
        }
        let relative = normalized_relative(root, &path)?;
        let digest = if metadata.is_file() {
            sha256(&std::fs::read(&path)?)
        } else if metadata.file_type().is_symlink() {
            sha256(std::fs::read_link(&path)?.as_os_str().as_encoded_bytes())
        } else {
            return Err(io::Error::other(format!(
                "unsupported workspace entry: {}",
                path.display()
            )));
        };
        entries.insert(relative, digest);
    }
    Ok(())
}

fn normalized_relative(root: &Path, path: &Path) -> io::Result<String> {
    path.strip_prefix(root)
        .map_err(io::Error::other)?
        .components()
        .map(|component| match component {
            Component::Normal(value) => value
                .to_str()
                .map(str::to_owned)
                .ok_or_else(|| io::Error::other("workspace path is not UTF-8")),
            _ => Err(io::Error::other("workspace path is not normalized")),
        })
        .collect::<Result<Vec<_>, _>>()
        .map(|parts| parts.join("/"))
}

fn sha256(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn records_created_modified_and_deleted_files_without_paths() {
        let root = tempfile::tempdir().unwrap();
        std::fs::write(root.path().join("modified.txt"), b"before").unwrap();
        std::fs::write(root.path().join("deleted.txt"), b"deleted").unwrap();
        let before = Snapshot::capture(root.path()).unwrap();

        std::fs::write(root.path().join("modified.txt"), b"after").unwrap();
        std::fs::remove_file(root.path().join("deleted.txt")).unwrap();
        std::fs::write(root.path().join("created.txt"), b"created").unwrap();
        let deltas = before.delta(Snapshot::capture(root.path()).unwrap());

        assert_eq!(deltas.len(), 3);
        assert_eq!(
            deltas
                .iter()
                .filter(|delta| delta.operation == "created")
                .count(),
            1
        );
        assert_eq!(
            deltas
                .iter()
                .filter(|delta| delta.operation == "modified")
                .count(),
            1
        );
        assert_eq!(
            deltas
                .iter()
                .filter(|delta| delta.operation == "deleted")
                .count(),
            1
        );
        assert!(deltas.iter().all(|delta| delta.path_sha256.len() == 64));
        assert!(deltas.iter().all(|delta| delta.scope == "workspace"));
        assert!(
            deltas
                .iter()
                .all(|delta| !delta.path_sha256.contains(".txt"))
        );
    }

    #[test]
    fn classifies_evaluator_owned_engine_state_separately() {
        let root = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(root.path().join(".wayland-core/sessions")).unwrap();
        let before = Snapshot::capture(root.path()).unwrap();

        std::fs::write(root.path().join(".wayland-core/trace.jsonl"), b"trace").unwrap();
        std::fs::write(
            root.path().join(".wayland-core/sessions/session.json"),
            b"session",
        )
        .unwrap();
        std::fs::write(root.path().join("result.txt"), b"result").unwrap();
        let deltas = before.delta(Snapshot::capture(root.path()).unwrap());

        assert_eq!(
            deltas
                .iter()
                .filter(|delta| delta.scope == "engine_state")
                .count(),
            2
        );
        assert_eq!(
            deltas
                .iter()
                .filter(|delta| delta.scope == "workspace")
                .count(),
            1
        );
    }
}
