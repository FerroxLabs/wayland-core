//! Phase 30 SR-30-3, the missing half: **bind a verified translation to the harness that declared
//! it, and lower it to fixture steps.**
//!
//! # Why this module exists
//!
//! `dialect.rs` compiles a canonical semantic script into one harness's dialect and digests the
//! result. `dialect_discovery.rs` captures the corpus it compiles against. Before this module,
//! nothing consumed the output: measured on `bin/wayland-scorecard.rs` at
//! `75babf32`, a `TranslationV1` was written by `dialect compile` and read back by exactly one
//! consumer — `dialect verify`, which re-digests it. `TrialsCommand::Run` took no translation
//! argument, and `drive_leg` obtained its script from `protocol["fixture_script"][dimension]`.
//!
//! **So protocol v2 could satisfy all four of its stated execution preconditions and still replay
//! v1's `write_file` script verbatim.** This module is the seam that makes v2 executable at all.
//!
//! # Why binding is a separate concern from compilation
//!
//! [`crate::dialect`] is identity-blind **by type** (G2): [`ToolSchemaCorpusV1`] has no field
//! naming the product it came from, so the compiler cannot branch on whose corpus it holds. That
//! property is load-bearing and this module does not weaken it.
//!
//! But *execution* has the opposite requirement. Running harness A with a dialect compiled for
//! harness B is the original F30-03 defect in a new costume — and it is **invisible to digest
//! verification**, because such a translation verifies perfectly against the corpus it was
//! compiled from. Identity therefore has to be checked somewhere, and the only honest place is
//! here, at the point of execution, against the discovery manifest that recorded which harness
//! declared the corpus.
//!
//! That split is the design: **the compiler must not see identity; the executor must.**
//!
//! # The checks, and which failure each one catches
//!
//! | # | Check | Catches |
//! |---|---|---|
//! | 1 | translation's dimension == the dimension being run | a correctness translation driving a recovery leg |
//! | 2 | the dimension has a canonical script | a typo'd dimension resolving to nothing |
//! | 3 | `TranslationV1::verify` against script + corpus (G4) | a translation hand-tuned after compilation |
//! | 4 | manifest's `corpus_sha256` == the corpus's own digest | a manifest paired with a corpus it does not describe |
//! | 5 | manifest's `tool_label` == the harness being launched | **a dialect compiled for a different harness** |
//!
//! Checks 3 and 5 are independent and neither subsumes the other. 3 is a digest check and passes
//! for a perfectly-compiled translation belonging to somebody else; 5 is an identity check and
//! passes for a hand-edited translation belonging to the right harness. The negative-control suite
//! in `tests/dialect_exec_gate.rs` exercises each one alone.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::dialect::{
    CompiledStepV1, ToolSchemaCorpusV1, TranslationV1, VOCABULARY_VERSION, canonical_script,
};
use crate::dialect_discovery::DiscoveryManifestV1;
use crate::fixtures::openai::OpenAiStep;

/// Dialect provenance stamped onto every trial record driven from a compiled translation.
///
/// **This is what makes a v2 record unpoolable with a v1 one.** A v1 record has no `dialect`
/// field at all, so an assembler cannot silently mix a run that spoke each harness's own dialect
/// with a run that spoke one competitor's.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TrialDialectV1 {
    pub vocabulary_version: String,
    /// Digest of the corpus the harness itself declared on the wire.
    pub corpus_sha256: String,
    /// Digest of the compiled steps actually executed.
    pub translation_sha256: String,
    /// The harness label the discovery manifest bound this corpus to.
    pub bound_tool_label: String,
    /// Every tool name this translation actually calls, in script order. Published so a reader can
    /// see which tool each arm was driven through without re-reading the translation.
    pub resolved_tool_names: Vec<String>,
}

/// A verified, harness-bound translation lowered to fixture steps.
#[derive(Debug, Clone)]
pub struct DialectBindingV1 {
    pub steps: Vec<OpenAiStep>,
    pub provenance: TrialDialectV1,
}

#[derive(Debug, Error, PartialEq, Eq)]
pub enum DialectExecError {
    #[error(
        "DIALECT_EXEC_DIMENSION_MISMATCH translation={translation} requested={requested} \
         — a translation may only drive the dimension it was compiled for"
    )]
    DimensionMismatch {
        translation: String,
        requested: String,
    },

    #[error("DIALECT_EXEC_UNKNOWN_DIMENSION dimension={dimension}")]
    UnknownDimension { dimension: String },

    /// G4 at execution time. A translation edited after compilation dies here.
    #[error("DIALECT_EXEC_TRANSLATION_UNVERIFIED {detail}")]
    TranslationUnverified { detail: String },

    #[error(
        "DIALECT_EXEC_CORPUS_MANIFEST_MISMATCH manifest_corpus={manifest} actual_corpus={actual} \
         — the discovery manifest does not describe this corpus"
    )]
    CorpusManifestMismatch { manifest: String, actual: String },

    /// The check that catches the original F30-03 defect reproduced deliberately: a dialect
    /// compiled for one harness, executed against another.
    #[error(
        "DIALECT_EXEC_HARNESS_MISMATCH corpus_declared_by={declared_by} launching={launching} \
         — refusing to drive a harness with another harness's dialect"
    )]
    HarnessMismatch {
        declared_by: String,
        launching: String,
    },

    #[error("DIALECT_EXEC_VOCABULARY_MISMATCH expected={expected} found={found}")]
    VocabularyMismatch { expected: String, found: String },
}

/// Verify a translation, bind it to the harness that declared its corpus, and lower it to fixture
/// steps.
///
/// `launching_tool_label` is the label of the harness that is about to be spawned — the caller
/// takes it from the invocation it is about to execute, never from the translation.
pub fn bind_translation(
    requested_dimension: &str,
    launching_tool_label: &str,
    translation: &TranslationV1,
    corpus: &ToolSchemaCorpusV1,
    manifest: &DiscoveryManifestV1,
) -> Result<DialectBindingV1, DialectExecError> {
    // 1 — the translation drives the dimension it was compiled for, and no other.
    if translation.dimension != requested_dimension {
        return Err(DialectExecError::DimensionMismatch {
            translation: translation.dimension.clone(),
            requested: requested_dimension.to_string(),
        });
    }

    // Checked before `verify` so the error names the real problem rather than a digest mismatch.
    if translation.vocabulary_version != VOCABULARY_VERSION {
        return Err(DialectExecError::VocabularyMismatch {
            expected: VOCABULARY_VERSION.to_string(),
            found: translation.vocabulary_version.clone(),
        });
    }

    // 2 — the dimension resolves to a canonical script.
    let script = canonical_script(requested_dimension).ok_or_else(|| {
        DialectExecError::UnknownDimension {
            dimension: requested_dimension.to_string(),
        }
    })?;

    // 3 — G4: both digests recomputed from the material they claim to address.
    translation
        .verify(&script, corpus)
        .map_err(|e| DialectExecError::TranslationUnverified {
            detail: e.to_string(),
        })?;

    // 4 — the manifest describes THIS corpus.
    let actual = corpus
        .sha256()
        .map_err(|e| DialectExecError::TranslationUnverified {
            detail: e.to_string(),
        })?;
    if manifest.corpus_sha256 != actual {
        return Err(DialectExecError::CorpusManifestMismatch {
            manifest: manifest.corpus_sha256.clone(),
            actual,
        });
    }

    // 5 — the corpus was declared by the harness we are about to launch.
    if manifest.tool_label != launching_tool_label {
        return Err(DialectExecError::HarnessMismatch {
            declared_by: manifest.tool_label.clone(),
            launching: launching_tool_label.to_string(),
        });
    }

    let steps = lower_steps(&translation.steps);
    let resolved_tool_names = translation
        .steps
        .iter()
        .filter_map(|s| match s {
            CompiledStepV1::ToolCall(call) => Some(call.tool_name.clone()),
            _ => None,
        })
        .collect();

    Ok(DialectBindingV1 {
        steps,
        provenance: TrialDialectV1 {
            vocabulary_version: translation.vocabulary_version.clone(),
            corpus_sha256: translation.corpus_sha256.clone(),
            translation_sha256: translation.translation_sha256.clone(),
            bound_tool_label: manifest.tool_label.clone(),
            resolved_tool_names,
        },
    })
}

/// Lower compiled steps to fixture steps.
///
/// Text and transport-fault steps carry no dialect and are copied through verbatim, which is what
/// keeps the recovery leg's injected 503 identical to v1's.
///
/// **Deliberately `pub`**: the negative-control suite needs to execute a mis-compiled dialect
/// end-to-end, which means bypassing [`bind_translation`]'s guards on purpose. A guard that can
/// only be tested by its own refusal is a guard whose *consequence* is never measured — the point
/// of the negative control is that the trial itself goes red, not merely that the binder said no.
pub fn lower_steps(steps: &[CompiledStepV1]) -> Vec<OpenAiStep> {
    steps
        .iter()
        .map(|step| match step {
            CompiledStepV1::Text { text } => OpenAiStep::Text { text: text.clone() },
            CompiledStepV1::HttpError { status } => OpenAiStep::HttpError { status: *status },
            CompiledStepV1::ToolCall(call) => OpenAiStep::ToolCall {
                id: call.id.clone(),
                name: call.tool_name.clone(),
                arguments: arguments_to_json(&call.arguments),
            },
        })
        .collect()
}

fn arguments_to_json(arguments: &BTreeMap<String, String>) -> serde_json::Value {
    serde_json::Value::Object(
        arguments
            .iter()
            .map(|(k, v)| (k.clone(), serde_json::Value::String(v.clone())))
            .collect(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dialect::{DeclaredToolV1, compile_script};

    fn manifest_for(label: &str, corpus: &ToolSchemaCorpusV1) -> DiscoveryManifestV1 {
        DiscoveryManifestV1 {
            tool_label: label.to_string(),
            tool_version: None,
            captured_at_utc: "2026-07-29T00:00:00Z".to_string(),
            corpus_sha256: corpus.sha256().expect("corpus digest"),
            requests_observed: 1,
            model_requested: None,
            notes: vec![],
        }
    }

    fn string_param(names: &[&str]) -> serde_json::Value {
        let props: serde_json::Map<String, serde_json::Value> = names
            .iter()
            .map(|n| {
                (
                    (*n).to_string(),
                    serde_json::json!({ "type": "string" }),
                )
            })
            .collect();
        serde_json::json!({
            "type": "object",
            "properties": props,
            "required": names,
        })
    }

    /// PascalCase surface, as `wayland-core` declares on the wire.
    fn pascal_corpus() -> ToolSchemaCorpusV1 {
        ToolSchemaCorpusV1::new(vec![
            DeclaredToolV1 {
                name: "Write".to_string(),
                description: String::new(),
                parameters: string_param(&["file_path", "content"]),
            },
            DeclaredToolV1 {
                name: "Read".to_string(),
                description: String::new(),
                parameters: string_param(&["file_path"]),
            },
        ])
    }

    /// snake_case surface, the flavour v1's frozen script assumed.
    fn snake_corpus() -> ToolSchemaCorpusV1 {
        ToolSchemaCorpusV1::new(vec![
            DeclaredToolV1 {
                name: "write_file".to_string(),
                description: String::new(),
                parameters: string_param(&["path", "content"]),
            },
            DeclaredToolV1 {
                name: "read_file".to_string(),
                description: String::new(),
                parameters: string_param(&["path"]),
            },
        ])
    }

    fn compile(dimension: &str, corpus: &ToolSchemaCorpusV1) -> TranslationV1 {
        let script = canonical_script(dimension).expect("canonical script");
        compile_script(&script, corpus).expect("compiles")
    }

    #[test]
    fn a_matched_translation_binds_and_lowers_to_the_harnesss_own_tool_name() {
        let corpus = pascal_corpus();
        let translation = compile("correctness", &corpus);
        let manifest = manifest_for("wayland", &corpus);

        let bound = bind_translation("correctness", "wayland", &translation, &corpus, &manifest)
            .expect("binds");

        let called: Vec<&str> = bound
            .steps
            .iter()
            .filter_map(|s| match s {
                OpenAiStep::ToolCall { name, .. } => Some(name.as_str()),
                _ => None,
            })
            .collect();
        assert_eq!(
            called,
            vec!["Write"],
            "the lowered fixture step must name the tool the harness declared"
        );
        assert_eq!(bound.provenance.bound_tool_label, "wayland");
        assert_eq!(bound.provenance.resolved_tool_names, vec!["Write"]);
    }

    /// The whole point of the module. A perfectly-compiled, digest-clean translation belonging to
    /// somebody else must not be executable against this harness.
    #[test]
    fn a_translation_compiled_for_another_harness_is_refused() {
        let peer_corpus = snake_corpus();
        let peer_translation = compile("correctness", &peer_corpus);
        let peer_manifest = manifest_for("hermes", &peer_corpus);

        // It verifies cleanly against its own corpus — digests alone cannot catch this.
        let script = canonical_script("correctness").unwrap();
        assert!(
            peer_translation.verify(&script, &peer_corpus).is_ok(),
            "precondition: the mis-targeted translation is itself digest-clean"
        );

        let err = bind_translation(
            "correctness",
            "wayland",
            &peer_translation,
            &peer_corpus,
            &peer_manifest,
        )
        .expect_err("must refuse another harness's dialect");

        assert_eq!(
            err,
            DialectExecError::HarnessMismatch {
                declared_by: "hermes".to_string(),
                launching: "wayland".to_string(),
            }
        );
    }

    #[test]
    fn a_manifest_that_does_not_describe_the_corpus_is_refused() {
        let corpus = pascal_corpus();
        let translation = compile("correctness", &corpus);
        // Right label, wrong corpus digest.
        let mut manifest = manifest_for("wayland", &corpus);
        manifest.corpus_sha256 = "0".repeat(64);

        let err = bind_translation("correctness", "wayland", &translation, &corpus, &manifest)
            .expect_err("must refuse an unbound manifest");
        assert!(
            matches!(err, DialectExecError::CorpusManifestMismatch { .. }),
            "got {err}"
        );
    }

    #[test]
    fn a_hand_edited_translation_is_refused_even_for_the_right_harness() {
        let corpus = pascal_corpus();
        let mut translation = compile("correctness", &corpus);
        let manifest = manifest_for("wayland", &corpus);

        // Tune the mapping by hand, exactly as a vendor wanting a favourable arm would.
        if let Some(CompiledStepV1::ToolCall(call)) = translation
            .steps
            .iter_mut()
            .find(|s| matches!(s, CompiledStepV1::ToolCall(_)))
        {
            call.tool_name = "Write".to_string();
            call.arguments.insert("extra".to_string(), "x".to_string());
        }

        let err = bind_translation("correctness", "wayland", &translation, &corpus, &manifest)
            .expect_err("must refuse a hand-edited translation");
        assert!(
            matches!(err, DialectExecError::TranslationUnverified { .. }),
            "got {err}"
        );
    }

    #[test]
    fn a_translation_may_not_drive_a_dimension_it_was_not_compiled_for() {
        let corpus = pascal_corpus();
        let translation = compile("correctness", &corpus);
        let manifest = manifest_for("wayland", &corpus);

        let err = bind_translation("recovery", "wayland", &translation, &corpus, &manifest)
            .expect_err("must refuse a cross-dimension translation");
        assert_eq!(
            err,
            DialectExecError::DimensionMismatch {
                translation: "correctness".to_string(),
                requested: "recovery".to_string(),
            }
        );
    }

    /// The dialect-free steps must survive lowering byte-for-byte, or v2's recovery leg would not
    /// be measuring the same fault v1 measured.
    #[test]
    fn the_injected_fault_step_survives_lowering_unchanged() {
        let corpus = pascal_corpus();
        let translation = compile("recovery", &corpus);
        let manifest = manifest_for("wayland", &corpus);
        let bound =
            bind_translation("recovery", "wayland", &translation, &corpus, &manifest).unwrap();

        let faults: Vec<u16> = bound
            .steps
            .iter()
            .filter_map(|s| match s {
                OpenAiStep::HttpError { status } => Some(*status),
                _ => None,
            })
            .collect();
        assert_eq!(
            faults,
            vec![503],
            "the 503 is dialect-free and must be copied through verbatim"
        );
        assert!(
            matches!(bound.steps.first(), Some(OpenAiStep::HttpError { .. })),
            "the fault must remain step 1, or the harness meets it at the wrong point"
        );
    }

    /// Arguments must be lowered under the HARNESS's parameter names, not the canonical slots.
    /// If this regresses, every arm would be driven with `path`/`content` again and the confound
    /// returns silently.
    #[test]
    fn arguments_are_lowered_under_the_harnesss_own_parameter_names() {
        let corpus = pascal_corpus();
        let translation = compile("correctness", &corpus);
        let manifest = manifest_for("wayland", &corpus);
        let bound =
            bind_translation("correctness", "wayland", &translation, &corpus, &manifest).unwrap();

        let args = bound
            .steps
            .iter()
            .find_map(|s| match s {
                OpenAiStep::ToolCall { arguments, .. } => Some(arguments.clone()),
                _ => None,
            })
            .expect("a tool call");
        let obj = args.as_object().expect("arguments lower to a JSON object");
        assert!(
            obj.contains_key("file_path"),
            "expected the harness's own `file_path`, got {:?}",
            obj.keys().collect::<Vec<_>>()
        );
        assert!(
            !obj.contains_key("path"),
            "canonical slot name leaked into the wire payload: {:?}",
            obj.keys().collect::<Vec<_>>()
        );
    }

    /// Two harnesses with genuinely different dialects must be driven to the SAME semantic work:
    /// same argument VALUES, under each one's own parameter NAMES.
    #[test]
    fn semantically_identical_scripts_compile_to_semantically_identical_work() {
        let pascal = pascal_corpus();
        let snake = snake_corpus();

        let a = bind_translation(
            "correctness",
            "wayland",
            &compile("correctness", &pascal),
            &pascal,
            &manifest_for("wayland", &pascal),
        )
        .expect("arm A binds");
        let b = bind_translation(
            "correctness",
            "hermes",
            &compile("correctness", &snake),
            &snake,
            &manifest_for("hermes", &snake),
        )
        .expect("arm B binds");

        let values = |bound: &DialectBindingV1| -> Vec<String> {
            bound
                .steps
                .iter()
                .filter_map(|s| match s {
                    OpenAiStep::ToolCall { arguments, .. } => Some(
                        arguments
                            .as_object()
                            .expect("object")
                            .values()
                            .map(|v| v.as_str().unwrap_or_default().to_string())
                            .collect::<Vec<_>>(),
                    ),
                    _ => None,
                })
                .flatten()
                .collect()
        };

        // Different tool NAMES...
        assert_ne!(
            a.provenance.resolved_tool_names, b.provenance.resolved_tool_names,
            "precondition: the two arms must really speak different dialects, \
             else this test proves nothing"
        );
        // ...identical semantic PAYLOAD.
        let mut va = values(&a);
        let mut vb = values(&b);
        va.sort();
        vb.sort();
        assert_eq!(
            va, vb,
            "the same canonical script must carry the same values into both dialects"
        );
    }
}
