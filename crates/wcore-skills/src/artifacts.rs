//! X4: skill artifact generation. Materialised on activation.

use std::collections::HashMap;
use std::path::{Component, Path, PathBuf};

use thiserror::Error;

use crate::types::ArtifactSpec;

#[derive(Debug, Error)]
pub enum ArtifactError {
    #[error("missing arg for placeholder ${{{0}}}")]
    MissingArg(String),

    #[error("artifact path '{path}' resolves outside skill root: {resolved}")]
    PathEscape { path: String, resolved: String },

    #[error("io error writing artifact {path}: {source}")]
    Io {
        path: String,
        #[source]
        source: std::io::Error,
    },
}

/// Write each artifact to disk, substituting `${args.foo}` placeholders
/// from `args`. Paths are resolved relative to `root` and MUST stay under
/// it (no `..` escapes). Intermediate directories are created on demand.
///
/// Returns the list of written paths on success. On the first error the
/// function returns immediately — partial state is the caller's problem
/// to clean up (SkillTool surfaces logs and continues).
pub async fn write_artifacts(
    specs: &[ArtifactSpec],
    args: &HashMap<String, String>,
    root: &Path,
) -> Result<Vec<PathBuf>, ArtifactError> {
    let mut written = Vec::with_capacity(specs.len());
    for spec in specs {
        let rendered = render_template(&spec.template, args)?;
        let target = resolve_under_root(&spec.path, root)?;
        if let Some(parent) = target.parent() {
            tokio::fs::create_dir_all(parent)
                .await
                .map_err(|source| ArtifactError::Io {
                    path: target.display().to_string(),
                    source,
                })?;
        }
        let target_owned = target.clone();
        let bytes = rendered.into_bytes();
        tokio::task::spawn_blocking(move || wcore_config::atomic_write(&target_owned, &bytes))
            .await
            .map_err(|e| ArtifactError::Io {
                path: target.display().to_string(),
                source: std::io::Error::other(e),
            })?
            .map_err(|source| ArtifactError::Io {
                path: target.display().to_string(),
                source,
            })?;
        written.push(target);
    }
    Ok(written)
}

fn render_template(
    template: &str,
    args: &HashMap<String, String>,
) -> Result<String, ArtifactError> {
    let mut out = String::with_capacity(template.len());
    let mut rest = template;
    while let Some(start) = rest.find("${") {
        out.push_str(&rest[..start]);
        let end = rest[start..]
            .find('}')
            .ok_or_else(|| ArtifactError::MissingArg("unterminated ${{ in template".into()))?;
        let key = &rest[start + 2..start + end];
        // Only args.foo is supported; reject other namespaces clearly.
        let arg_key = key
            .strip_prefix("args.")
            .ok_or_else(|| ArtifactError::MissingArg(key.to_string()))?;
        let value = args
            .get(arg_key)
            .ok_or_else(|| ArtifactError::MissingArg(format!("args.{arg_key}")))?;
        out.push_str(value);
        rest = &rest[start + end + 1..];
    }
    out.push_str(rest);
    Ok(out)
}

fn resolve_under_root(rel: &str, root: &Path) -> Result<PathBuf, ArtifactError> {
    // Reject absolute paths and any ParentDir components in the relative
    // path before joining. Walking the components catches `../foo`,
    // `foo/../bar`, and absolute paths uniformly.
    let candidate = root.join(rel);
    if Path::new(rel).is_absolute() {
        return Err(ArtifactError::PathEscape {
            path: rel.to_string(),
            resolved: candidate.display().to_string(),
        });
    }
    for c in Path::new(rel).components() {
        match c {
            Component::Normal(_) | Component::CurDir => {}
            _ => {
                return Err(ArtifactError::PathEscape {
                    path: rel.to_string(),
                    resolved: candidate.display().to_string(),
                });
            }
        }
    }
    Ok(candidate)
}
