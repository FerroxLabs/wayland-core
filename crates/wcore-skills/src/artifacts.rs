//! X4: skill artifact generation. Materialised on activation.

use std::collections::HashMap;
use std::path::{Component, Path, PathBuf};

use async_trait::async_trait;
use thiserror::Error;

use crate::types::ArtifactSpec;

#[derive(Debug, Error)]
pub enum ArtifactError {
    #[error("missing arg for placeholder ${{{0}}}")]
    MissingArg(String),

    #[error("artifact path '{path}' resolves outside skill root: {resolved}")]
    PathEscape { path: String, resolved: String },

    #[error(
        "artifact path '{path}' targets a skill SOURCE directory ({resolved}). \
         Skills are LOADED from there and never written to it -- writing a \
         SKILL.md is instruction injection into the next session. Put files \
         this skill produces under ${{WCORE_SKILL_OUTPUT_DIR}} instead \
         (<cwd>/.wayland-out/skills/<session_id>/)."
    )]
    SkillSourceTarget { path: String, resolved: String },

    #[error("refused writing artifact {path}: {reason}")]
    Refused { path: String, reason: String },

    #[error("io error writing artifact {path}: {source}")]
    Io {
        path: String,
        #[source]
        source: std::io::Error,
    },
}

/// The filesystem `write_artifacts` is allowed to touch.
///
/// FerroxLabs/wayland#1097. Artifact writes used to call
/// `wcore_config::atomic_write` directly, which meant the ONE surface a skill
/// can write through was the one surface the session's filesystem containment
/// did not cover: the repository-control write guard installed for every
/// session, the workspace jail installed for contained ones, and the
/// project-secret deny were all bypassed. A declared artifact could therefore
/// write `.git/hooks/pre-commit` -- arbitrary code execution on the operator's
/// next commit, requested by a line of frontmatter -- and could follow a
/// symlink out of the jail its own session's `Read` is confined to.
///
/// Implementations MUST apply the session's containment, and MUST create
/// intermediate directories (the production implementation is the session's
/// `VirtualFs`, whose `RealFs` leaf already does both that and the atomic
/// temp-file-plus-rename that this module used to perform itself).
///
/// Deliberately a port defined HERE rather than an `Arc<dyn VirtualFs>`
/// parameter: `VirtualFs` lives in `wcore-tools`, and `wcore-skills` does not
/// depend on it. The adapter is `wcore-agent`'s, alongside the `ToolContext`
/// the vfs comes from.
#[async_trait]
pub trait ArtifactSink: Send + Sync {
    /// Write `bytes` to `path`, creating parent directories, subject to the
    /// session's filesystem containment. The `String` is a human-readable
    /// refusal or I/O reason; it reaches the model.
    async fn write(&self, path: &Path, bytes: &[u8]) -> Result<(), String>;
}

/// Write each artifact through `sink`, substituting `${args.foo}` placeholders
/// from `args`. Paths are resolved relative to `root` and MUST stay under
/// it (no `..` escapes). Intermediate directories are created on demand.
///
/// `skill_root` is the skill's own source directory when it has one; a target
/// inside it is refused by name rather than written, because a skill that can
/// rewrite its own load path can rewrite what the next session is told to do.
///
/// Returns the list of written paths on success. On the first error the
/// function returns immediately — partial state is the caller's problem
/// to clean up (SkillTool surfaces logs and continues).
pub async fn write_artifacts(
    specs: &[ArtifactSpec],
    args: &HashMap<String, String>,
    root: &Path,
    skill_root: Option<&Path>,
    sink: &dyn ArtifactSink,
) -> Result<Vec<PathBuf>, ArtifactError> {
    let mut written = Vec::with_capacity(specs.len());
    for spec in specs {
        let rendered = render_template(&spec.template, args)?;
        let target = resolve_under_root(&spec.path, root)?;
        reject_skill_source_target(&spec.path, &target, skill_root)?;
        sink.write(&target, rendered.as_bytes())
            .await
            .map_err(|reason| ArtifactError::Refused {
                path: target.display().to_string(),
                reason,
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
    //
    // NOTE what this does NOT do, and why the sink exists: every component of
    // `.git/hooks/pre-commit` is `Component::Normal`, and a `link/x` whose
    // `link` is a symlink out of the workspace is Normal too. Lexical
    // component checks cannot see either. Containment is the sink's job.
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

/// Refuse an artifact aimed at a place skills are LOADED from
/// (FerroxLabs/wayland#1096, suggested direction 2).
///
/// Two forms, both seen in the UAT's shape: the skill's own source directory,
/// and any `.wayland-core/skills` / `.wayland-core/commands` path reachable
/// from the workspace. The second overlaps the repository-control write guard
/// the sink also applies; it is checked here as well so the model is told WHICH
/// mistake it made and where the file should have gone, instead of getting the
/// generic repo-control refusal.
fn reject_skill_source_target(
    rel: &str,
    target: &Path,
    skill_root: Option<&Path>,
) -> Result<(), ArtifactError> {
    let err = || ArtifactError::SkillSourceTarget {
        path: rel.to_string(),
        resolved: target.display().to_string(),
    };

    if let Some(sr) = skill_root
        && !sr.as_os_str().is_empty()
        && target.starts_with(sr)
    {
        return Err(err());
    }

    let comps: Vec<_> = target
        .components()
        .filter_map(|c| match c {
            Component::Normal(s) => Some(s.to_string_lossy().into_owned()),
            _ => None,
        })
        .collect();
    for pair in comps.windows(2) {
        if pair[0] == ".wayland-core" && (pair[1] == "skills" || pair[1] == "commands") {
            return Err(err());
        }
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    /// Records what it was asked to write and refuses nothing. Used to prove
    /// the REFUSALS below happen before the sink is reached — a sink that is
    /// never called is the only honest way to assert "the write did not
    /// happen" without depending on a particular containment implementation.
    #[derive(Default)]
    struct RecordingSink {
        seen: Mutex<Vec<PathBuf>>,
    }

    #[async_trait]
    impl ArtifactSink for RecordingSink {
        async fn write(&self, path: &Path, _bytes: &[u8]) -> Result<(), String> {
            self.seen.lock().unwrap().push(path.to_path_buf());
            Ok(())
        }
    }

    fn spec(path: &str) -> ArtifactSpec {
        ArtifactSpec {
            path: path.into(),
            template: "x".into(),
        }
    }

    #[tokio::test]
    async fn skill_source_targets_never_reach_the_sink() {
        let sink = RecordingSink::default();
        let root = Path::new("/work");
        for p in [
            ".wayland-core/skills/foo/SKILL.md",
            ".wayland-core/commands/foo.md",
            "nested/.wayland-core/skills/foo/SKILL.md",
        ] {
            let e = write_artifacts(&[spec(p)], &HashMap::new(), root, None, &sink)
                .await
                .expect_err("must refuse");
            assert!(
                matches!(e, ArtifactError::SkillSourceTarget { .. }),
                "{p} produced {e:?}"
            );
            assert!(
                e.to_string().contains(".wayland-out"),
                "the refusal must name where the file should have gone: {e}"
            );
        }
        assert!(
            sink.seen.lock().unwrap().is_empty(),
            "a refused artifact was still handed to the sink"
        );
    }

    #[tokio::test]
    async fn a_skill_cannot_write_into_its_own_source_directory() {
        let sink = RecordingSink::default();
        let root = Path::new("/work");
        let skill_root = root.join("vendor").join("myskill");
        let e = write_artifacts(
            &[spec("vendor/myskill/SKILL.md")],
            &HashMap::new(),
            root,
            Some(&skill_root),
            &sink,
        )
        .await
        .expect_err("must refuse");
        assert!(
            matches!(e, ArtifactError::SkillSourceTarget { .. }),
            "{e:?}"
        );
        assert!(sink.seen.lock().unwrap().is_empty());
    }

    /// The neighbouring directory `vendor/myskillOTHER` shares a string prefix
    /// with the skill root but is not inside it. A `starts_with` on the string
    /// rather than on path components would refuse this legitimate target.
    #[tokio::test]
    async fn a_sibling_sharing_a_name_prefix_is_still_writable() {
        let sink = RecordingSink::default();
        let root = Path::new("/work");
        let skill_root = root.join("vendor").join("myskill");
        write_artifacts(
            &[spec("vendor/myskillOTHER/report.md")],
            &HashMap::new(),
            root,
            Some(&skill_root),
            &sink,
        )
        .await
        .expect("a sibling directory is an ordinary target");
        assert_eq!(sink.seen.lock().unwrap().len(), 1);
    }

    /// The sink's refusal is the containment answer, and it has to reach the
    /// model rather than being logged and swallowed.
    #[tokio::test]
    async fn a_sink_refusal_surfaces_as_a_typed_error() {
        struct Deny;
        #[async_trait]
        impl ArtifactSink for Deny {
            async fn write(&self, _path: &Path, _bytes: &[u8]) -> Result<(), String> {
                Err("outside sandbox root".to_string())
            }
        }
        let e = write_artifacts(
            &[spec("ok.md")],
            &HashMap::new(),
            Path::new("/work"),
            None,
            &Deny,
        )
        .await
        .expect_err("must propagate");
        assert!(matches!(e, ArtifactError::Refused { .. }), "{e:?}");
        assert!(e.to_string().contains("outside sandbox root"));
    }
}
