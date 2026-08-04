//! Collision-free semantic hashing for evidence containing an owned workspace.

use std::path::{Component, Path};

use sha2::{Digest, Sha256};
use thiserror::Error;

const HASH_DOMAIN: &[u8] = b"wayland.eval.workspace-evidence.v1\0";

pub(crate) fn semantic_sha256(
    domain: &[u8],
    evidence: &[u8],
    workspace: &Path,
) -> Result<String, WorkspaceEvidenceError> {
    let workspace_forms = workspace_forms(workspace)?;
    let mut hasher = Sha256::new();
    tagged_bytes(&mut hasher, b'D', HASH_DOMAIN);
    tagged_bytes(&mut hasher, b'D', domain);
    match serde_json::from_slice::<serde_json::Value>(evidence) {
        Ok(value @ (serde_json::Value::Array(_) | serde_json::Value::Object(_))) => {
            hash_json(&mut hasher, &value, &workspace_forms);
        }
        _ => hash_text(&mut hasher, evidence, &workspace_forms),
    }
    Ok(format!("{:x}", hasher.finalize()))
}

fn workspace_forms(workspace: &Path) -> Result<Vec<Vec<u8>>, WorkspaceEvidenceError> {
    if !workspace.is_absolute()
        || workspace.parent().is_none()
        || workspace
            .components()
            .all(|component| !matches!(component, Component::Normal(_)))
    {
        return Err(WorkspaceEvidenceError::UnsafeWorkspace);
    }
    let native = workspace
        .to_str()
        .ok_or(WorkspaceEvidenceError::NonUtf8Workspace)?;
    if native.is_empty() {
        return Err(WorkspaceEvidenceError::UnsafeWorkspace);
    }
    let mut forms = Vec::new();
    push_spellings(&mut forms, native);
    // The SAME directory reaches evidence under more than one spelling, and the
    // caller only holds one of them. `AgentBootstrap` fixes a session's
    // workspace at ingress with `std::fs::canonicalize`, so every path the
    // binary under test emits — the `Working directory:` line of its system
    // prompt above all — carries the RESOLVED form, while the harness that owns
    // the temporary directory still holds the form `TempDir` handed it. On macOS
    // `/var`, `/tmp` and `/etc` are symlinks into `/private`, and on Windows an
    // 8.3 short name aliases its long form, so those two disagree on exactly the
    // platforms CI runs; Linux has no such alias, which is why matching only the
    // caller's form was green there.
    //
    // Left unmatched, the random per-run directory name stays inside the digest
    // and two identical runs never agree — a repeatability gate that cannot pass
    // rather than one that cannot fail, but just as worthless.
    //
    // Fail-soft on a workspace that cannot be resolved (the identity-only roots
    // in `openai_fixture_contract.rs` are never created): an unresolvable path
    // has no second spelling to miss, so the caller's form is already complete.
    if let Ok(canonical) = std::fs::canonicalize(workspace)
        && let Some(canonical) = dunce::simplified(&canonical).to_str()
    {
        push_spellings(&mut forms, canonical);
    }
    Ok(forms)
}

/// Append every spelling of one pathname the platform can produce, skipping
/// duplicates so `next_workspace` never scans the same bytes twice.
fn push_spellings(forms: &mut Vec<Vec<u8>>, path: &str) {
    push_form(forms, path.as_bytes().to_vec());
    // The workspace also reaches evidence from INSIDE a JSON-encoded string,
    // where every `\` is doubled. That is not hypothetical: an OpenAI request
    // carries a tool call's input as `function.arguments` — a whole JSON
    // document stored as a JSON *string*, built by
    // `serde_json::to_string(input)` in `crates/wcore-providers/src/openai.rs`
    // — so from the SECOND request of a turn onward the body embeds
    // `C:\\Users\\…\\.tmpXXXX` while the harness holds `C:\Users\…\.tmpXXXX`.
    //
    // Left unmatched, the random per-run directory name survives into
    // `semantic_body_sha256` and no two runs of the F04 repeatability gate can
    // agree — while every per-LEAF digest still agrees, because
    // `insert_semantic_leaf_hash` re-parses that leaf as JSON and therefore
    // sees the UNescaped path. That asymmetry is exactly the shape the failure
    // took on Windows: request 1 identical, requests 2..n divergent, and
    // `assert_request_leaves_equal` silent throughout.
    //
    // `/`-separated platforms have nothing to escape, so this spelling
    // deduplicates away there and their digests are byte-for-byte unchanged.
    push_form(forms, path.replace('\\', "\\\\").into_bytes());
    #[cfg(windows)]
    {
        push_form(forms, path.replace('\\', "/").into_bytes());
    }
}

fn push_form(forms: &mut Vec<Vec<u8>>, form: Vec<u8>) {
    if !forms.contains(&form) {
        forms.push(form);
    }
}

fn hash_json(hasher: &mut Sha256, value: &serde_json::Value, workspace_forms: &[Vec<u8>]) {
    match value {
        serde_json::Value::Null => hasher.update(b"N"),
        serde_json::Value::Bool(value) => hasher.update(if *value { b"T" } else { b"F" }),
        serde_json::Value::Number(value) => {
            tagged_bytes(hasher, b'#', value.to_string().as_bytes())
        }
        serde_json::Value::String(value) => {
            hasher.update(b"S");
            hash_text(hasher, value.as_bytes(), workspace_forms);
        }
        serde_json::Value::Array(values) => {
            hasher.update(b"A");
            hasher.update((values.len() as u64).to_be_bytes());
            for value in values {
                hash_json(hasher, value, workspace_forms);
            }
        }
        serde_json::Value::Object(values) => {
            hasher.update(b"O");
            hasher.update((values.len() as u64).to_be_bytes());
            let mut entries = values.iter().collect::<Vec<_>>();
            entries.sort_by_key(|(key, _)| *key);
            for (key, value) in entries {
                hash_text(hasher, key.as_bytes(), workspace_forms);
                hash_json(hasher, value, workspace_forms);
            }
        }
    }
}

fn hash_text(hasher: &mut Sha256, text: &[u8], workspace_forms: &[Vec<u8>]) {
    let mut cursor = 0;
    while let Some((start, end)) = next_workspace(text, cursor, workspace_forms) {
        tagged_bytes(hasher, b'L', &text[cursor..start]);
        hasher.update(b"W");
        cursor = end;
    }
    tagged_bytes(hasher, b'L', &text[cursor..]);
}

fn next_workspace(
    text: &[u8],
    cursor: usize,
    workspace_forms: &[Vec<u8>],
) -> Option<(usize, usize)> {
    let mut best = None;
    for workspace in workspace_forms {
        if workspace.is_empty() || workspace.len() > text.len().saturating_sub(cursor) {
            continue;
        }
        for start in cursor..=text.len() - workspace.len() {
            let end = start + workspace.len();
            if &text[start..end] == workspace
                && prefix_boundary(text, start)
                && suffix_boundary(text, end)
                && best.is_none_or(|(best_start, _)| start < best_start)
            {
                best = Some((start, end));
                break;
            }
        }
    }
    best
}

fn prefix_boundary(text: &[u8], start: usize) -> bool {
    start == 0
        || text[start - 1].is_ascii_whitespace()
        || matches!(
            text[start - 1],
            b'"' | b'\'' | b'`' | b'(' | b'[' | b'{' | b'=' | b':' | b','
        )
}

fn suffix_boundary(text: &[u8], end: usize) -> bool {
    end == text.len() || text[end].is_ascii_whitespace() || matches!(text[end], b'/' | b'\\')
}

fn tagged_bytes(hasher: &mut Sha256, tag: u8, bytes: &[u8]) {
    hasher.update([tag]);
    hasher.update((bytes.len() as u64).to_be_bytes());
    hasher.update(bytes);
}

#[derive(Debug, Error, PartialEq, Eq)]
pub(crate) enum WorkspaceEvidenceError {
    #[error("workspace must be an absolute non-root path")]
    UnsafeWorkspace,
    #[error("workspace must be valid UTF-8")]
    NonUtf8Workspace,
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::semantic_sha256;

    #[test]
    fn markdown_code_span_is_a_workspace_boundary() {
        let first = tempfile::tempdir().unwrap();
        let second = tempfile::tempdir().unwrap();
        let first_evidence = format!(
            "Persistent memory lives at `{}/.wayland-core/memory`.",
            first.path().display()
        );
        let second_evidence = format!(
            "Persistent memory lives at `{}/.wayland-core/memory`.",
            second.path().display()
        );

        assert_eq!(
            semantic_sha256(b"test", first_evidence.as_bytes(), first.path()).unwrap(),
            semantic_sha256(b"test", second_evidence.as_bytes(), second.path()).unwrap()
        );
    }

    /// The caller holds the workspace by its SYMLINKED spelling; the binary
    /// under test prints the RESOLVED one, because `AgentBootstrap` canonicalizes
    /// at ingress. Both must normalize to the same marker, or the random
    /// per-run directory name survives into the digest and no two runs of the
    /// F04 repeatability gate can ever agree.
    ///
    /// This is the macOS condition exactly (`/var` -> `/private/var`),
    /// reproduced here with an explicit symlink so it is checked on every unix
    /// runner rather than only where the platform happens to supply the alias.
    #[cfg(unix)]
    #[test]
    fn a_symlinked_workspace_matches_the_resolved_spelling_the_binary_prints() {
        let root = tempfile::tempdir().unwrap();
        let mut resolved = Vec::new();
        let mut links = Vec::new();
        for name in ["a", "b"] {
            let real = root.path().join(format!("real-{name}"));
            std::fs::create_dir(&real).unwrap();
            let link = root.path().join(format!("link-{name}"));
            std::os::unix::fs::symlink(&real, &link).unwrap();
            resolved.push(std::fs::canonicalize(&real).unwrap());
            links.push(link);
        }

        // Positive control on the setup: the two spellings really do differ, so
        // a match below is the canonical form doing work rather than the two
        // sides being the same string all along.
        assert_ne!(
            links[0].to_str().unwrap(),
            resolved[0].to_str().unwrap(),
            "the symlink and its target have the same spelling; this test would \
             pass without the canonical form and proves nothing"
        );

        let evidence = |index: usize| format!("Working directory: {}\n", resolved[index].display());
        assert_eq!(
            semantic_sha256(b"test", evidence(0).as_bytes(), &links[0]).unwrap(),
            semantic_sha256(b"test", evidence(1).as_bytes(), &links[1]).unwrap()
        );

        // Known-negative: recognising more spellings must not make unrelated
        // evidence agree. Same workspace, different tail.
        let deeper = format!("Working directory: {}/sub\n", resolved[0].display());
        assert_ne!(
            semantic_sha256(b"test", evidence(0).as_bytes(), &links[0]).unwrap(),
            semantic_sha256(b"test", deeper.as_bytes(), &links[0]).unwrap()
        );
    }

    /// The workspace reaches an OpenAI request body under a spelling the
    /// harness never holds: JSON-ESCAPED, because `function.arguments` is a
    /// whole JSON document stored as a JSON *string*. On Windows that doubles
    /// every separator, the plain spelling stops matching, and the random
    /// per-run directory name survives into `semantic_body_sha256` — the F04
    /// repeatability gate then diverges from its SECOND request onward while
    /// its per-leaf assertion stays silent (leaves are re-parsed as JSON, so
    /// they see the unescaped path).
    ///
    /// JSON escapes only `\`, so on `/`-separated platforms the workspace
    /// directory is given a name that CONTAINS one — legal on unix, and the
    /// same code path Windows takes for every path it produces. Without that
    /// this test would be green on Linux whether or not the bug is fixed.
    #[test]
    fn a_json_encoded_workspace_path_normalizes() {
        #[cfg(unix)]
        const NAMES: [&str; 2] = ["run\\a", "run\\b"];
        #[cfg(not(unix))]
        const NAMES: [&str; 2] = ["run-a", "run-b"];

        let root = tempfile::tempdir().unwrap();
        let workspaces = NAMES.map(|name| {
            let directory = root.path().join(name);
            std::fs::create_dir(&directory).unwrap();
            directory
        });

        // Positive control on the setup: the escaped spelling must actually
        // differ from the plain one, or nothing below exercises the escaped
        // form and the assertions pass unfixed.
        let native = workspaces[0].to_str().unwrap();
        assert_ne!(
            native,
            native.replace('\\', "\\\\"),
            "this platform's workspace path has no escapable separator; the \
             assertions below would prove nothing"
        );

        // Exactly the shape an OpenAI request carries from its second call
        // onward: an assistant tool call whose `arguments` is a JSON document
        // encoded into a string.
        let body = |workspace: &Path, file: &str| {
            let arguments = serde_json::to_string(&serde_json::json!({
                "file_path": workspace.join("src").join(file),
            }))
            .unwrap();
            serde_json::to_vec(&serde_json::json!({
                "messages": [{
                    "role": "assistant",
                    "tool_calls": [{
                        "id": "call-edit",
                        "type": "function",
                        "function": {"name": "Edit", "arguments": arguments},
                    }],
                }],
            }))
            .unwrap()
        };

        let first = body(&workspaces[0], "settings.toml");
        let second = body(&workspaces[1], "settings.toml");
        assert_ne!(first, second, "the two runs must differ on the wire");
        assert_eq!(
            semantic_sha256(b"test", &first, &workspaces[0]).unwrap(),
            semantic_sha256(b"test", &second, &workspaces[1]).unwrap()
        );

        // Known-negative: recognising the escaped spelling must not swallow the
        // rest of the encoded document, or the gate could no longer fail.
        assert_ne!(
            semantic_sha256(b"test", &first, &workspaces[0]).unwrap(),
            semantic_sha256(b"test", &body(&workspaces[0], "other.toml"), &workspaces[0]).unwrap()
        );
    }

    #[test]
    fn line_ending_is_a_workspace_boundary() {
        let first = tempfile::tempdir().unwrap();
        let second = tempfile::tempdir().unwrap();
        let first_evidence = format!(
            "Working directory: {}\nNext section",
            first.path().display()
        );
        let second_evidence = format!(
            "Working directory: {}\nNext section",
            second.path().display()
        );

        assert_eq!(
            semantic_sha256(b"test", first_evidence.as_bytes(), first.path()).unwrap(),
            semantic_sha256(b"test", second_evidence.as_bytes(), second.path()).unwrap()
        );
    }
}
