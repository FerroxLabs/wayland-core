//! Evaluator-owned snapshots of the hermetic scenario workspace.

use std::collections::BTreeMap;
use std::io;
use std::path::{Component, Path, PathBuf};

use sha2::{Digest, Sha256};

#[derive(Debug)]
pub(crate) struct Snapshot {
    entries: BTreeMap<ScopedPath, String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum EvidenceScope {
    Workspace,
    EngineState,
}

impl EvidenceScope {
    fn as_str(self) -> &'static str {
        match self {
            Self::Workspace => "workspace",
            Self::EngineState => "engine_state",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
struct ScopedPath {
    scope: EvidenceScope,
    relative: String,
}

#[derive(Debug, Clone)]
struct EvidenceRoot {
    scope: EvidenceScope,
    path: PathBuf,
}

/// The complete evaluator-owned filesystem evidence boundary for one session.
///
/// Logical roots retain their scope while `scan_roots` collapses equal and
/// nested paths to the smallest set of physical directory walks. A path is
/// classified by the most-specific logical root, with engine state winning an
/// equal-root tie so one physical file cannot be merged across scopes.
#[derive(Debug, Clone)]
pub(crate) struct EvidenceRoots {
    roots: Vec<EvidenceRoot>,
    scan_roots: Vec<PathBuf>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ScopedEvidencePath {
    pub(crate) scope: &'static str,
    pub(crate) relative: PathBuf,
}

impl EvidenceRoots {
    pub(crate) fn new<P>(
        workspace: &Path,
        engine_state_roots: impl IntoIterator<Item = P>,
    ) -> io::Result<Self>
    where
        P: AsRef<Path>,
    {
        let workspace = std::fs::canonicalize(workspace)?;
        let mut roots = vec![EvidenceRoot {
            scope: EvidenceScope::Workspace,
            path: workspace,
        }];
        for root in engine_state_roots {
            let path = std::fs::canonicalize(root.as_ref())?;
            if let Some(existing) = roots.iter_mut().find(|existing| existing.path == path) {
                existing.scope = EvidenceScope::EngineState;
            } else {
                roots.push(EvidenceRoot {
                    scope: EvidenceScope::EngineState,
                    path,
                });
            }
        }
        roots.sort_by(|left, right| {
            left.path
                .components()
                .count()
                .cmp(&right.path.components().count())
                .then_with(|| left.path.cmp(&right.path))
        });

        let mut scan_roots = Vec::new();
        for root in &roots {
            if !scan_roots
                .iter()
                .any(|ancestor: &PathBuf| root.path.starts_with(ancestor))
            {
                scan_roots.push(root.path.clone());
            }
        }
        Ok(Self { roots, scan_roots })
    }

    pub(crate) fn scan_roots(&self) -> &[PathBuf] {
        &self.scan_roots
    }

    pub(crate) fn classify(&self, path: &Path) -> io::Result<ScopedEvidencePath> {
        let root = self
            .roots
            .iter()
            .filter(|root| path.starts_with(&root.path))
            .max_by(|left, right| {
                left.path
                    .components()
                    .count()
                    .cmp(&right.path.components().count())
                    .then_with(|| left.scope.cmp(&right.scope))
            })
            .ok_or_else(|| io::Error::other("evidence path is outside required roots"))?;
        let relative = normalized_relative_path(&root.path, path)?;
        let scope = if root.scope == EvidenceScope::Workspace && is_project_engine_state(&relative)
        {
            EvidenceScope::EngineState
        } else {
            root.scope
        };
        Ok(ScopedEvidencePath {
            scope: scope.as_str(),
            relative,
        })
    }
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct FilesystemDeltaEvidence {
    pub scope: String,
    pub path_sha256: String,
    pub operation: String,
    pub content_sha256: Option<String>,
}

impl Snapshot {
    pub(crate) fn capture(roots: &EvidenceRoots) -> io::Result<Self> {
        let mut entries = BTreeMap::new();
        for root in roots.scan_roots() {
            collect(roots, root, &mut entries)?;
        }
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
                    scope: path.scope.as_str().to_string(),
                    path_sha256: sha256(
                        format!("{}\0{}", path.scope.as_str(), path.relative).as_bytes(),
                    ),
                    operation: operation.to_string(),
                    content_sha256,
                })
            })
            .collect()
    }
}

fn collect(
    roots: &EvidenceRoots,
    directory: &Path,
    entries: &mut BTreeMap<ScopedPath, String>,
) -> io::Result<()> {
    let mut paths = std::fs::read_dir(directory)?
        .map(|entry| entry.map(|entry| entry.path()))
        .collect::<Result<Vec<_>, _>>()?;
    paths.sort();
    for path in paths {
        let metadata = std::fs::symlink_metadata(&path)?;
        if metadata.is_dir() {
            collect(roots, &path, entries)?;
            continue;
        }
        let classified = roots.classify(&path)?;
        let digest = if metadata.is_file() {
            sha256(&std::fs::read(&path)?)
        } else if metadata.file_type().is_symlink() {
            sha256(std::fs::read_link(&path)?.as_os_str().as_encoded_bytes())
        } else {
            return Err(io::Error::other(format!(
                "unsupported evidence entry: {}",
                path.display()
            )));
        };
        entries.insert(
            ScopedPath {
                scope: match classified.scope {
                    "workspace" => EvidenceScope::Workspace,
                    "engine_state" => EvidenceScope::EngineState,
                    _ => unreachable!("EvidenceRoots emits only supported scopes"),
                },
                relative: normalized_relative_string(&classified.relative)?,
            },
            digest,
        );
    }
    Ok(())
}

fn normalized_relative_path(root: &Path, path: &Path) -> io::Result<PathBuf> {
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
        .map(|parts| parts.into_iter().collect())
}

fn normalized_relative_string(path: &Path) -> io::Result<String> {
    path.components()
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

fn is_project_engine_state(relative: &Path) -> bool {
    relative
        .components()
        .next()
        .is_some_and(|component| component.as_os_str() == ".wayland-core")
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
        let roots = EvidenceRoots::new(root.path(), std::iter::empty::<&Path>()).unwrap();
        let before = Snapshot::capture(&roots).unwrap();

        std::fs::write(root.path().join("modified.txt"), b"after").unwrap();
        std::fs::remove_file(root.path().join("deleted.txt")).unwrap();
        std::fs::write(root.path().join("created.txt"), b"created").unwrap();
        let deltas = before.delta(Snapshot::capture(&roots).unwrap());

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
        let roots = EvidenceRoots::new(root.path(), std::iter::empty::<&Path>()).unwrap();
        let before = Snapshot::capture(&roots).unwrap();

        std::fs::write(root.path().join(".wayland-core/trace.jsonl"), b"trace").unwrap();
        std::fs::write(
            root.path().join(".wayland-core/sessions/session.json"),
            b"session",
        )
        .unwrap();
        std::fs::write(root.path().join("result.txt"), b"result").unwrap();
        let deltas = before.delta(Snapshot::capture(&roots).unwrap());

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

    #[test]
    fn observes_disjoint_home_mutation_with_engine_state_scope() {
        let workspace = tempfile::tempdir().unwrap();
        let home = tempfile::tempdir().unwrap();
        std::fs::write(home.path().join("modified.db"), b"before").unwrap();
        std::fs::write(home.path().join("deleted.db"), b"deleted").unwrap();
        let roots = EvidenceRoots::new(workspace.path(), [home.path()]).unwrap();
        let before = Snapshot::capture(&roots).unwrap();

        std::fs::write(home.path().join("created.db"), b"created").unwrap();
        std::fs::write(home.path().join("modified.db"), b"after").unwrap();
        std::fs::remove_file(home.path().join("deleted.db")).unwrap();
        let deltas = before.delta(Snapshot::capture(&roots).unwrap());

        assert_eq!(deltas.len(), 3);
        assert!(deltas.iter().all(|delta| delta.scope == "engine_state"));
        for operation in ["created", "modified", "deleted"] {
            assert_eq!(
                deltas
                    .iter()
                    .filter(|delta| delta.operation == operation)
                    .count(),
                1,
                "missing {operation} engine-state evidence"
            );
        }
    }

    #[test]
    fn disjoint_sibling_roots_cannot_merge_matching_relative_paths() {
        let parent = tempfile::tempdir().unwrap();
        let workspace = parent.path().join("workspace");
        let home = parent.path().join("home");
        std::fs::create_dir_all(&workspace).unwrap();
        std::fs::create_dir_all(&home).unwrap();
        let roots = EvidenceRoots::new(&workspace, [&home]).unwrap();
        let before = Snapshot::capture(&roots).unwrap();

        std::fs::write(workspace.join("state.db"), b"workspace").unwrap();
        std::fs::write(home.join("state.db"), b"engine").unwrap();
        let deltas = before.delta(Snapshot::capture(&roots).unwrap());

        assert_eq!(deltas.len(), 2);
        assert!(deltas.iter().any(|delta| delta.scope == "workspace"));
        assert!(deltas.iter().any(|delta| delta.scope == "engine_state"));
        assert_ne!(deltas[0].path_sha256, deltas[1].path_sha256);
    }

    #[test]
    fn equal_and_nested_roots_are_walked_once_without_omission() {
        let workspace = tempfile::tempdir().unwrap();
        let nested_home = workspace.path().join("home");
        std::fs::create_dir_all(&nested_home).unwrap();
        let nested = EvidenceRoots::new(workspace.path(), [&nested_home]).unwrap();
        assert_eq!(nested.scan_roots.len(), 1);
        let before = Snapshot::capture(&nested).unwrap();
        std::fs::write(workspace.path().join("result.txt"), b"result").unwrap();
        std::fs::write(nested_home.join("memory.db"), b"memory").unwrap();
        let deltas = before.delta(Snapshot::capture(&nested).unwrap());
        assert_eq!(deltas.len(), 2);
        assert!(deltas.iter().any(|delta| delta.scope == "workspace"));
        assert!(deltas.iter().any(|delta| delta.scope == "engine_state"));

        let equal = EvidenceRoots::new(workspace.path(), [workspace.path()]).unwrap();
        assert_eq!(equal.scan_roots.len(), 1);
        let before = Snapshot::capture(&equal).unwrap();
        std::fs::write(workspace.path().join("equal.db"), b"equal").unwrap();
        let deltas = before.delta(Snapshot::capture(&equal).unwrap());
        assert_eq!(deltas.len(), 1);
        assert_eq!(deltas[0].scope, "engine_state");
    }

    #[test]
    fn failure_of_any_required_root_prevents_a_snapshot() {
        let workspace = tempfile::tempdir().unwrap();
        let home = tempfile::tempdir().unwrap();
        let roots = EvidenceRoots::new(workspace.path(), [home.path()]).unwrap();
        std::fs::remove_dir(home.path()).unwrap();

        assert!(Snapshot::capture(&roots).is_err());
    }
}
