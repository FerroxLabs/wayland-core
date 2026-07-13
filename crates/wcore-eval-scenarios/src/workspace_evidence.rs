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
    let native_form = native.as_bytes().to_vec();
    #[cfg(windows)]
    {
        let mut forms = vec![native_form];
        let slash = native.replace('\\', "/").into_bytes();
        if slash != forms[0] {
            forms.push(slash);
        }
        Ok(forms)
    }
    #[cfg(not(windows))]
    {
        Ok(vec![native_form])
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
            b'"' | b'\'' | b'(' | b'[' | b'{' | b'=' | b':' | b','
        )
}

fn suffix_boundary(text: &[u8], end: usize) -> bool {
    end == text.len() || matches!(text[end], b'/' | b'\\')
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
