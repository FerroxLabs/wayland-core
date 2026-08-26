use std::collections::BTreeSet;
use std::fs;
use std::io;

use serde_json::{Map, Value};

use super::ContractResult;
use super::generate::{
    WireShapeBaseline, all_relative_files, contract_path, enforce_wire_shape_version,
    generated_artifacts,
};

const MANIFEST: &str = "manifest.json";

/// Regenerate in memory and reject missing, extra, or byte-drifted artifacts.
pub fn check_contract() -> ContractResult<()> {
    let root = contract_path();
    let expected = generated_artifacts()?;
    // Before the drift report, because "run `wcore-contract generate`" is the
    // wrong remedy for a moved wire shape and running it used to certify the
    // break as green.
    enforce_wire_shape_version(&expected, WireShapeBaseline::Required)?;
    let expected_paths = expected.keys().cloned().collect::<BTreeSet<_>>();
    let actual_paths = all_relative_files(&root)?;

    let missing = expected_paths
        .difference(&actual_paths)
        .cloned()
        .collect::<Vec<_>>();
    let extra = actual_paths
        .difference(&expected_paths)
        .cloned()
        .collect::<Vec<_>>();
    let mut drifted = Vec::new();
    for path in expected_paths.intersection(&actual_paths) {
        if fs::read(root.join(path))? != expected[path] {
            drifted.push(path.clone());
        }
    }
    if missing.is_empty() && extra.is_empty() && drifted.is_empty() {
        return Ok(());
    }

    // The path list alone cannot tell a source-hash rebase from a moved wire
    // schema, and those two have opposite remedies. The regeneration is
    // already in hand, so say which one this is.
    let diff = ManifestDiff::of(
        fs::read(root.join(MANIFEST)).ok().as_deref(),
        expected.get(MANIFEST).map(Vec::as_slice),
    );
    Err(io::Error::other(format!(
        "Desktop contract corpus drift: missing={missing:?}, extra={extra:?}, drifted={drifted:?}.\n{}",
        diff.report()
    ))
    .into())
}

/// The same key diff `check_contract` reports, computed without writing
/// anything.
///
/// `check` is the gate; this is the diagnostic an author reaches for once the
/// gate has gone red and the question is *what moved*. It regenerates in
/// memory exactly as `check` does, so it answers about the tree in front of the
/// author rather than about the last commit.
pub fn manifest_diff_report() -> ContractResult<String> {
    let root = contract_path();
    let expected = generated_artifacts()?;
    Ok(ManifestDiff::of(
        fs::read(root.join(MANIFEST)).ok().as_deref(),
        expected.get(MANIFEST).map(Vec::as_slice),
    )
    .report())
}

/// Whether the wire schema the corpus publishes moved.
///
/// This is the whole question. `source_inputs_digest` moves whenever any hashed
/// source file moves - a comment, a rename, a rebase - and drags
/// `fixture_digest` with it, because the negotiation fixtures embed the
/// descriptor. None of that changes a byte a pinned Desktop build validates
/// against. `schema_digest` moving does.
enum SchemaVerdict {
    /// Both manifests publish the same `schema_digest`.
    Unchanged(String),
    Moved { from: String, to: String },
    /// One side had no readable manifest, or no `schema_digest` in it.
    Unknown(String),
}

/// What the checked-in `manifest.json` and this tree's regeneration disagree
/// about.
struct ManifestDiff {
    /// Top-level keys present on either side whose values differ. A scalar
    /// carries its rendered value; a structured value is named without being
    /// dumped.
    keys: Vec<(String, Option<String>, Option<String>)>,
    schema: SchemaVerdict,
}

impl ManifestDiff {
    fn of(checked_in: Option<&[u8]>, regenerated: Option<&[u8]>) -> Self {
        let parse = |bytes: Option<&[u8]>| -> Option<Map<String, Value>> {
            match serde_json::from_slice::<Value>(bytes?) {
                Ok(Value::Object(map)) => Some(map),
                _ => None,
            }
        };
        let (Some(checked_in), Some(regenerated)) = (parse(checked_in), parse(regenerated)) else {
            return Self {
                keys: Vec::new(),
                schema: SchemaVerdict::Unknown(format!(
                    "no readable {MANIFEST} object on one or both sides"
                )),
            };
        };
        let digest = |map: &Map<String, Value>| {
            map.get("schema_digest")
                .and_then(Value::as_str)
                .map(str::to_owned)
        };
        let schema = match (digest(&checked_in), digest(&regenerated)) {
            (Some(from), Some(to)) if from == to => SchemaVerdict::Unchanged(from),
            (Some(from), Some(to)) => SchemaVerdict::Moved { from, to },
            _ => SchemaVerdict::Unknown(format!("{MANIFEST} carries no `schema_digest` string")),
        };
        Self {
            keys: key_deltas(&checked_in, &regenerated),
            schema,
        }
    }

    fn report(&self) -> String {
        let mut out = match &self.schema {
            SchemaVerdict::Unchanged(digest) => format!(
                "schema_digest is UNCHANGED ({digest}): no wire schema moved. This is a \
                 source-hash rebase - the same wire surface described from different source \
                 bytes - and regenerating is the correct remedy.\n\
                 Remedy: run `wcore-contract generate`, then confirm in the diff that \
                 schema_digest is still {digest} before committing.\n"
            ),
            SchemaVerdict::Moved { from, to } => format!(
                "schema_digest MOVED: {from} -> {to}. The wire schema a pinned Desktop build \
                 validates against is NOT the one this tree produces, so `wcore-contract \
                 generate` is NOT a safe remedy here: it would restamp the digest and turn this \
                 red green with nobody having decided the change is compatible.\n\
                 Remedy: decide the contract version first - CONTRACT_MINOR for a new wire type \
                 or a new optional field, CONTRACT_MAJOR for a field renamed, removed, retyped \
                 or newly required - and regenerate only after that decision is in the tree.\n"
            ),
            SchemaVerdict::Unknown(reason) => format!(
                "schema_digest could not be compared ({reason}), so whether a wire schema moved \
                 is UNKNOWN. Do not treat `wcore-contract generate` as a safe remedy until that \
                 is established by hand - restore {MANIFEST} from git first.\n"
            ),
        };
        if self.keys.is_empty() {
            out.push_str(&format!(
                "No {MANIFEST} key moved. If the corpus is nonetheless reported as drifted, that \
                 drift is in the corpus files themselves - a hand-edit or a partial commit - not \
                 a regeneration gap.\n"
            ));
        } else {
            out.push_str(&format!("{MANIFEST} keys that moved:\n"));
            for (key, from, to) in &self.keys {
                match (from, to) {
                    (Some(from), Some(to)) => out.push_str(&format!("  {key}: {from} -> {to}\n")),
                    _ => out.push_str(&format!("  {key}: changed\n")),
                }
            }
        }
        out.push_str("Run `wcore-contract diff` to see this key diff without writing anything.\n");
        out
    }
}

/// Top-level keys present on either side whose values differ.
fn key_deltas(
    checked_in: &Map<String, Value>,
    regenerated: &Map<String, Value>,
) -> Vec<(String, Option<String>, Option<String>)> {
    checked_in
        .keys()
        .chain(regenerated.keys())
        .collect::<BTreeSet<_>>()
        .into_iter()
        .filter(|key| checked_in.get(*key) != regenerated.get(*key))
        .map(|key| {
            (
                key.clone(),
                render(checked_in.get(key)),
                render(regenerated.get(key)),
            )
        })
        .collect()
}

/// A scalar renders its value; anything structured renders as `None` so the
/// caller names it as changed instead of dumping a wire inventory into an
/// error message.
fn render(value: Option<&Value>) -> Option<String> {
    match value {
        None => Some("(absent)".to_owned()),
        Some(Value::String(text)) => Some(text.clone()),
        Some(scalar @ (Value::Number(_) | Value::Bool(_) | Value::Null)) => {
            Some(scalar.to_string())
        }
        Some(Value::Array(_) | Value::Object(_)) => None,
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn manifest(schema: &str, source: &str, shapes: Value) -> Vec<u8> {
        let mut map = Map::new();
        map.insert("schema_digest".into(), Value::String(schema.into()));
        map.insert("source_inputs_digest".into(), Value::String(source.into()));
        map.insert("wire_shapes".into(), shapes);
        serde_json::to_vec(&Value::Object(map)).expect("serialize the test manifest")
    }

    /// The rebase arm: only the source hash moved, so regenerating is safe and
    /// the message has to say so rather than leave the author hand-diffing
    /// digests.
    #[test]
    fn an_unchanged_schema_digest_reports_a_source_hash_rebase() {
        let a = manifest("sha256:aaa", "sha256:111", json!({"x": "1"}));
        let b = manifest("sha256:aaa", "sha256:222", json!({"x": "1"}));
        let report = ManifestDiff::of(Some(&a), Some(&b)).report();
        assert!(report.contains("schema_digest is UNCHANGED"), "{report}");
        assert!(report.contains("source-hash rebase"), "{report}");
        assert!(
            report.contains("source_inputs_digest: sha256:111 -> sha256:222"),
            "{report}"
        );
        assert!(!report.contains("MOVED"), "{report}");
    }

    /// The arm that matters: a moved schema must never be presented as a
    /// regenerate-and-go.
    #[test]
    fn a_moved_schema_digest_refuses_to_offer_regeneration_as_the_remedy() {
        let a = manifest("sha256:aaa", "sha256:111", json!({"x": "1"}));
        let b = manifest("sha256:bbb", "sha256:222", json!({"x": "2"}));
        let report = ManifestDiff::of(Some(&a), Some(&b)).report();
        assert!(
            report.contains("schema_digest MOVED: sha256:aaa -> sha256:bbb"),
            "{report}"
        );
        assert!(report.contains("is NOT a safe remedy"), "{report}");
        assert!(report.contains("CONTRACT_MAJOR"), "{report}");
        assert!(!report.contains("UNCHANGED"), "{report}");
        // A structured value is named as changed, never dumped.
        assert!(report.contains("wire_shapes: changed"), "{report}");
    }

    /// Drift with an identical manifest is a hand-edited or partially
    /// committed corpus, and reads as neither of the two above.
    #[test]
    fn an_identical_manifest_points_at_the_corpus_files_instead() {
        let a = manifest("sha256:aaa", "sha256:111", json!({"x": "1"}));
        let report = ManifestDiff::of(Some(&a), Some(&a)).report();
        assert!(report.contains("No manifest.json key moved"), "{report}");
        assert!(report.contains("hand-edit or a partial commit"), "{report}");
    }

    /// An unreadable side must fail loud rather than fall back to the
    /// reassuring arm.
    #[test]
    fn an_unreadable_manifest_is_unknown_not_reassuring() {
        let a = manifest("sha256:aaa", "sha256:111", json!({"x": "1"}));
        let report = ManifestDiff::of(Some(b"not json"), Some(&a)).report();
        assert!(report.contains("could not be compared"), "{report}");
        assert!(report.contains("Do not treat"), "{report}");
        assert!(!report.contains("UNCHANGED"), "{report}");

        let absent = ManifestDiff::of(None, Some(&a)).report();
        assert!(absent.contains("could not be compared"), "{absent}");
    }
}
