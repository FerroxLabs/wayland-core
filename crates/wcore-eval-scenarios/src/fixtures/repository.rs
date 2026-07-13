//! Content-addressed repository trees for deterministic agent scenarios.

use std::collections::BTreeMap;
use std::path::{Component, Path, PathBuf};

use serde::Serialize;
use sha2::{Digest, Sha256};
use thiserror::Error;

const FIXTURE_PROTOCOL_VERSION: u32 = 1;
const MAX_FILES: usize = 512;
const MAX_TOTAL_BYTES: usize = 4 * 1024 * 1024;

/// A deterministic repository tree whose identity is independent of the
/// temporary directory where a scenario materializes it.
#[derive(Debug, Clone)]
pub struct SeededRepository {
    files: BTreeMap<String, Vec<u8>>,
    fixture_sha256: String,
}

impl SeededRepository {
    pub fn new<P, C>(files: impl IntoIterator<Item = (P, C)>) -> Result<Self, RepositoryError>
    where
        P: AsRef<str>,
        C: AsRef<[u8]>,
    {
        let mut normalized = BTreeMap::new();
        let mut total_bytes = 0_usize;
        for (path, content) in files {
            let path = path.as_ref();
            validate_relative_path(path)?;
            let content = content.as_ref().to_vec();
            total_bytes = total_bytes.saturating_add(content.len());
            if total_bytes > MAX_TOTAL_BYTES {
                return Err(RepositoryError::TooLarge {
                    limit: MAX_TOTAL_BYTES,
                });
            }
            if normalized.insert(path.to_string(), content).is_some() {
                return Err(RepositoryError::DuplicatePath(path.to_string()));
            }
            if normalized.len() > MAX_FILES {
                return Err(RepositoryError::TooManyFiles { limit: MAX_FILES });
            }
        }
        if normalized.is_empty() {
            return Err(RepositoryError::Empty);
        }

        let canonical = serde_json::to_vec(&CanonicalRepository {
            protocol_version: FIXTURE_PROTOCOL_VERSION,
            files: &normalized,
        })?;
        Ok(Self {
            files: normalized,
            fixture_sha256: format!("{:x}", Sha256::digest(canonical)),
        })
    }

    pub fn fixture_sha256(&self) -> &str {
        &self.fixture_sha256
    }

    /// Write the seed without following pre-existing symlinks below `root`.
    pub fn materialize(&self, root: &Path) -> Result<(), RepositoryError> {
        std::fs::create_dir_all(root).map_err(|source| RepositoryError::Io {
            path: root.to_path_buf(),
            source,
        })?;
        for (relative, content) in &self.files {
            let path = prepare_target(root, relative)?;
            std::fs::write(&path, content).map_err(|source| RepositoryError::Io {
                path: path.clone(),
                source,
            })?;
        }
        Ok(())
    }
}

/// Hash a materialized repository tree independently of its absolute root.
/// Symlinks and non-regular entries are rejected rather than followed.
pub fn repository_tree_sha256(root: &Path) -> Result<String, RepositoryError> {
    let mut files = BTreeMap::new();
    let mut total_bytes = 0_usize;
    collect_tree(root, root, &mut files, &mut total_bytes)?;
    if files.is_empty() {
        return Err(RepositoryError::Empty);
    }
    let canonical = serde_json::to_vec(&CanonicalRepository {
        protocol_version: FIXTURE_PROTOCOL_VERSION,
        files: &files,
    })?;
    Ok(format!("{:x}", Sha256::digest(canonical)))
}

fn collect_tree(
    root: &Path,
    directory: &Path,
    files: &mut BTreeMap<String, Vec<u8>>,
    total_bytes: &mut usize,
) -> Result<(), RepositoryError> {
    let entries = std::fs::read_dir(directory).map_err(|source| RepositoryError::Io {
        path: directory.to_path_buf(),
        source,
    })?;
    let mut paths = entries
        .map(|entry| {
            entry
                .map(|entry| entry.path())
                .map_err(|source| RepositoryError::Io {
                    path: directory.to_path_buf(),
                    source,
                })
        })
        .collect::<Result<Vec<_>, _>>()?;
    paths.sort();

    for path in paths {
        let metadata = std::fs::symlink_metadata(&path).map_err(|source| RepositoryError::Io {
            path: path.clone(),
            source,
        })?;
        if metadata.file_type().is_symlink() {
            return Err(RepositoryError::UnsafeExistingPath(path));
        }
        if metadata.is_dir() {
            collect_tree(root, &path, files, total_bytes)?;
            continue;
        }
        if !metadata.is_file() {
            return Err(RepositoryError::UnsafeExistingPath(path));
        }
        let relative = path
            .strip_prefix(root)
            .map_err(|_| RepositoryError::InvalidPath(path.display().to_string()))?;
        let relative = relative
            .components()
            .map(|component| match component {
                Component::Normal(value) => value.to_str().map(str::to_string),
                _ => None,
            })
            .collect::<Option<Vec<_>>>()
            .ok_or_else(|| RepositoryError::InvalidPath(relative.display().to_string()))?
            .join("/");
        let content = std::fs::read(&path).map_err(|source| RepositoryError::Io {
            path: path.clone(),
            source,
        })?;
        *total_bytes = total_bytes.saturating_add(content.len());
        if *total_bytes > MAX_TOTAL_BYTES {
            return Err(RepositoryError::TooLarge {
                limit: MAX_TOTAL_BYTES,
            });
        }
        files.insert(relative, content);
        if files.len() > MAX_FILES {
            return Err(RepositoryError::TooManyFiles { limit: MAX_FILES });
        }
    }
    Ok(())
}

#[derive(Serialize)]
struct CanonicalRepository<'a> {
    protocol_version: u32,
    files: &'a BTreeMap<String, Vec<u8>>,
}

fn validate_relative_path(path: &str) -> Result<(), RepositoryError> {
    let parsed = Path::new(path);
    if path.is_empty()
        || parsed.is_absolute()
        || !parsed
            .components()
            .all(|component| matches!(component, Component::Normal(_)))
    {
        return Err(RepositoryError::InvalidPath(path.to_string()));
    }
    Ok(())
}

fn prepare_target(root: &Path, relative: &str) -> Result<PathBuf, RepositoryError> {
    let parsed = Path::new(relative);
    let mut target = root.to_path_buf();
    let mut components = parsed.components().peekable();
    while let Some(Component::Normal(segment)) = components.next() {
        target.push(segment);
        if components.peek().is_none() {
            if let Ok(metadata) = std::fs::symlink_metadata(&target)
                && (metadata.file_type().is_symlink() || metadata.is_dir())
            {
                return Err(RepositoryError::UnsafeExistingPath(target));
            }
            return Ok(target);
        }

        match std::fs::symlink_metadata(&target) {
            Ok(metadata) if metadata.file_type().is_symlink() || !metadata.is_dir() => {
                return Err(RepositoryError::UnsafeExistingPath(target));
            }
            Ok(_) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                std::fs::create_dir(&target).map_err(|source| RepositoryError::Io {
                    path: target.clone(),
                    source,
                })?;
            }
            Err(source) => {
                return Err(RepositoryError::Io {
                    path: target,
                    source,
                });
            }
        }
    }
    Err(RepositoryError::InvalidPath(relative.to_string()))
}

#[derive(Debug, Error)]
pub enum RepositoryError {
    #[error("repository fixture must contain at least one file")]
    Empty,
    #[error("repository fixture path must be relative and normalized: {0}")]
    InvalidPath(String),
    #[error("repository fixture contains duplicate path: {0}")]
    DuplicatePath(String),
    #[error("repository fixture exceeds {limit} files")]
    TooManyFiles { limit: usize },
    #[error("repository fixture exceeds {limit} bytes")]
    TooLarge { limit: usize },
    #[error("repository fixture refuses existing symlink or directory target: {0}")]
    UnsafeExistingPath(PathBuf),
    #[error("repository fixture I/O at {path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("repository fixture serialization failed: {0}")]
    Serialize(#[from] serde_json::Error),
}
