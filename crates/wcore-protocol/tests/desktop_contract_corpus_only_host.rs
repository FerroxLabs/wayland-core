//! What a Desktop host built from the SHIPPED corpus actually does with the
//! frames Core actually emits (C-1).
//!
//! Every other contract gate in this crate is written from inside the producer:
//! it reads `EVENT_SPECS`, or `PRODUCER_EVENT_TYPES`, or both, and therefore
//! shares the producer's own idea of what exists. A real Desktop host has no
//! such access. It has `contracts/desktop/v1/` and nothing else. Whatever is
//! absent from those bytes is, to that host, an unknown event — and the
//! protocol's unknown-event rule is not "ignore it".
//!
//! **This file deliberately does NOT use `HostContractObserver`.** Reusing it
//! is the bug under test: `contract/observation.rs` classifies an event by
//! `PRODUCER_EVENT_TYPES.contains(..)`, and `PRODUCER_EVENT_TYPES` is a
//! producer-side Rust constant that is not shipped in the corpus at all. An
//! observer that consults it can never notice that the corpus under-declares
//! the wire; it will happily accept the seven event types that no shipped
//! artifact mentions. The host we build below is restricted to the corpus, so
//! it fails exactly where a real integrator fails.
//!
//! The unknown-event rule replicated here is the documented one
//! (`docs/json-stream-protocol.md`, and the `adversarial/events/unknown-*`
//! corpus fixtures):
//!
//! * known `type`                -> accept
//! * unknown + `critical: false` -> drop
//! * unknown + `critical: true`  -> hard error
//! * unknown + no `critical`     -> hard error (criticality is unknowable, so
//!   the host cannot prove the frame is safe to ignore)
//!
//! No session transcript is hand-authored anywhere in this file. Both the
//! known-event set and every frame fed to the host are read out of the two
//! generated artifacts, so the test cannot invent a dialect that neither side
//! speaks.

use std::collections::{BTreeMap, BTreeSet};

use serde_json::{Value, json};
use wcore_protocol::contract::{PRODUCER_EVENT_TYPES, generated_artifacts};

/// What the corpus-only host did with one frame.
#[derive(Debug, Clone, PartialEq, Eq)]
enum CorpusHostOutcome {
    /// The corpus declares this `type`; the host can route and validate it.
    Accepted,
    /// Unknown but explicitly non-critical: safe to ignore.
    DroppedUnknownNonCritical,
    /// Unknown and NOT safe to ignore. A real host surfaces this to the user
    /// and/or tears the session down.
    HardError(&'static str),
}

/// A Desktop host whose entire knowledge of the producer wire is the shipped
/// `manifest.json`.
struct CorpusOnlyHost {
    /// `wire type -> fixture path`, straight out of `manifest.json`'s `events`.
    known_events: BTreeMap<String, String>,
}

impl CorpusOnlyHost {
    fn from_shipped_corpus(artifacts: &BTreeMap<String, Vec<u8>>) -> Self {
        let manifest: Value = serde_json::from_slice(
            artifacts
                .get("manifest.json")
                .expect("the corpus must ship a manifest.json"),
        )
        .expect("manifest.json must be JSON");
        let known_events = manifest["events"]
            .as_array()
            .expect("manifest.json must declare an events array")
            .iter()
            .map(|entry| {
                (
                    entry["type"]
                        .as_str()
                        .expect("every manifest event needs a type")
                        .to_owned(),
                    entry["path"]
                        .as_str()
                        .expect("every manifest event needs a fixture path")
                        .to_owned(),
                )
            })
            .collect::<BTreeMap<_, _>>();
        assert!(
            !known_events.is_empty(),
            "a corpus-only host that knows nothing would trivially reject everything"
        );
        Self { known_events }
    }

    fn observe(&self, frame: &Value) -> CorpusHostOutcome {
        let object = frame.as_object().expect("a producer frame is an object");
        let event_type = object
            .get("type")
            .and_then(Value::as_str)
            .expect("a producer frame carries a string type");
        if self.known_events.contains_key(event_type) {
            return CorpusHostOutcome::Accepted;
        }
        match object.get("critical").and_then(Value::as_bool) {
            Some(false) => CorpusHostOutcome::DroppedUnknownNonCritical,
            Some(true) => CorpusHostOutcome::HardError("unknown_critical_event"),
            None => CorpusHostOutcome::HardError("unknown_criticality"),
        }
    }
}

/// The frame a corpus-only host would see for `event_type`.
///
/// When the corpus declares the type, that is literally the shipped fixture —
/// the exact bytes the generator produced from the real `ProtocolEvent`
/// serializer. When the corpus does NOT declare it, no fixture exists to read,
/// so we send the smallest frame the wire can carry: the discriminator alone.
/// That is not a guess about the payload — the host's classification rule only
/// ever reads `type` and `critical`, so the body is irrelevant to the outcome,
/// and inventing one would be inventing a dialect.
fn corpus_frame_for(
    artifacts: &BTreeMap<String, Vec<u8>>,
    host: &CorpusOnlyHost,
    event_type: &str,
) -> Value {
    match host.known_events.get(event_type) {
        Some(path) => serde_json::from_slice(
            artifacts
                .get(path)
                .unwrap_or_else(|| panic!("manifest points at missing fixture {path}")),
        )
        .unwrap_or_else(|_| panic!("fixture {path} must be JSON")),
        None => json!({ "type": event_type }),
    }
}

/// Read one shipped single-frame `.jsonl` adversarial artifact.
fn shipped_frame(artifacts: &BTreeMap<String, Vec<u8>>, path: &str) -> Value {
    let bytes = artifacts
        .get(path)
        .unwrap_or_else(|| panic!("the corpus must ship {path}"));
    let text = std::str::from_utf8(bytes).expect("adversarial fixtures are UTF-8");
    serde_json::from_str(text.trim_end()).unwrap_or_else(|_| panic!("{path} must be one JSON line"))
}

/// POSITIVE CASE. Every event type the producer declares it can emit must be
/// survivable by a host that only has the shipped corpus.
///
/// "Survivable" is the weakest possible bar: not "validated", not "rendered" —
/// merely "does not hard error". A host cannot be expected to do anything
/// useful with a frame the corpus never mentions, but it must at least not tear
/// the session down over one. Today it does, and `workspace_policy` arrives on
/// every single session immediately after `ready`.
#[test]
fn corpus_only_host_survives_every_event_the_producer_emits() {
    let artifacts = generated_artifacts().expect("the corpus generator must run");
    let host = CorpusOnlyHost::from_shipped_corpus(&artifacts);

    let mut hard_errors = Vec::new();
    for event_type in PRODUCER_EVENT_TYPES {
        let frame = corpus_frame_for(&artifacts, &host, event_type);
        if let CorpusHostOutcome::HardError(kind) = host.observe(&frame) {
            hard_errors.push(format!("{event_type} ({kind})"));
        }
    }

    assert!(
        hard_errors.is_empty(),
        "a Desktop host built from the shipped corpus HARD ERRORS on {} of the {} \
         event types Core declares it emits: {}. The corpus under-declares the \
         producer wire, so these frames are indistinguishable at the host from a \
         hostile unknown-critical event.",
        hard_errors.len(),
        PRODUCER_EVENT_TYPES.len(),
        hard_errors.join(", ")
    );
}

/// NEGATIVE CONTROL. A genuinely unknown type with no `critical` field must
/// still hard error, or the positive case above degenerates into
/// "accept everything" and measures nothing.
#[test]
fn corpus_only_host_still_rejects_a_genuinely_unknown_event() {
    let artifacts = generated_artifacts().expect("the corpus generator must run");
    let host = CorpusOnlyHost::from_shipped_corpus(&artifacts);

    // Synthetic future type, chosen so it can never be added to the producer.
    assert_eq!(
        host.observe(&json!({ "type": "future_authority" })),
        CorpusHostOutcome::HardError("unknown_criticality"),
        "an unknown type with no criticality signal must not be silently accepted"
    );
    // The same rule, exercised through the corpus's own adversarial fixtures so
    // the control is bound to shipped bytes and not only to a literal here.
    assert_eq!(
        host.observe(&shipped_frame(
            &artifacts,
            "adversarial/events/unknown-criticality.jsonl"
        )),
        CorpusHostOutcome::HardError("unknown_criticality")
    );
    assert_eq!(
        host.observe(&shipped_frame(
            &artifacts,
            "adversarial/events/unknown-critical.jsonl"
        )),
        CorpusHostOutcome::HardError("unknown_critical_event")
    );
}

/// DROP CONTROL. An unknown type that declares itself non-critical must be
/// dropped, not escalated — otherwise the positive case could be "fixed" by
/// making the host reject less, which is the opposite of the contract.
#[test]
fn corpus_only_host_drops_an_unknown_noncritical_event() {
    let artifacts = generated_artifacts().expect("the corpus generator must run");
    let host = CorpusOnlyHost::from_shipped_corpus(&artifacts);

    assert_eq!(
        host.observe(&shipped_frame(
            &artifacts,
            "adversarial/events/unknown-noncritical.jsonl"
        )),
        CorpusHostOutcome::DroppedUnknownNonCritical
    );
    assert_eq!(
        host.observe(&json!({ "critical": false, "type": "future_observation" })),
        CorpusHostOutcome::DroppedUnknownNonCritical
    );
}

/// The declaration gap stated directly, so the diff that closes it is visible
/// as a diff. This is the same fact as the positive case, without the host
/// machinery in the way.
#[test]
fn shipped_manifest_declares_every_producer_event_type() {
    let artifacts = generated_artifacts().expect("the corpus generator must run");
    let host = CorpusOnlyHost::from_shipped_corpus(&artifacts);

    let declared = host.known_events.keys().cloned().collect::<BTreeSet<_>>();
    let produced = PRODUCER_EVENT_TYPES
        .iter()
        .map(|wire| (*wire).to_owned())
        .collect::<BTreeSet<_>>();

    let undeclared = produced.difference(&declared).cloned().collect::<Vec<_>>();
    let phantom = declared.difference(&produced).cloned().collect::<Vec<_>>();

    assert!(
        undeclared.is_empty() && phantom.is_empty(),
        "manifest.json and PRODUCER_EVENT_TYPES disagree — emitted but undeclared: \
         {undeclared:?}; declared but not emitted: {phantom:?}"
    );
}
