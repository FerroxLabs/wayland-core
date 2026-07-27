//! The F12 non-regression canary.
//!
//! Phase 22 plan 22-01 measured, cross-binary and single-variable, that the
//! session journal admits additive Goal/Task/Wait records at schema 5 without
//! changing what an existing journal replays to. That measurement was a
//! SNAPSHOT, and its own SUMMARY says so: "The corpus is retained as evidence
//! but no test pins its reduction. The moment someone changes reduction
//! semantics, nothing goes red. The determination is a snapshot, and a snapshot
//! is not a canary."
//!
//! This is the canary. It reduces the REAL 82,367-byte journal that the REAL
//! shipped release binary wrote on `hetzner-dsm` at commit `2ecdfdf5`, and pins
//! the digest of the resulting reduced state.
//!
//! ## Why this file can go red, and how that was verified rather than assumed
//!
//! The pinned digest below was captured by running this exact test against the
//! tree at `cd5b4e9b` — BEFORE the Goal kernel added a `SessionEvent` variant, a
//! reducer arm or a `ReducedSessionState` field. It is therefore a pre-change
//! observation, not a value this test's own author minted after the fact. A
//! canary whose expected value is recomputed from the post-change binary proves
//! only that the binary agrees with itself.
//!
//! It goes RED if a change to reduction semantics alters what this corpus
//! replays to — including the specific way the Goal kernel could have broken it,
//! namely a new `ReducedSessionState` field that is not
//! `#[serde(default, skip_serializing_if = ...)]` and therefore serializes into
//! the digest for every session that has no Goal at all.
//!
//! It deliberately does NOT assert an error string, an error kind or a numeric
//! status: it asserts the reduced state itself, through the same canonical
//! digest the product uses for its own snapshot authority.

use std::path::{Path, PathBuf};

use wcore_agent::session_journal::{SessionJournal, state_payload_digest};

/// Session id of the retained corpus, read from frame 0 of the file itself.
const CORPUS_SESSION_ID: &str = "9aa64ad04744";

/// The reduced-state digest of the retained Linux corpus.
///
/// Captured at `cd5b4e9b` (pre-Goal-kernel). See the module comment for why
/// that provenance is the whole point.
const CORPUS_REDUCED_STATE_DIGEST: &str =
    "4f5713e2a625ee050cc36d7f86fb42fbe1183705f58676186bcb9cff9392f6e2";

fn corpus_path(platform: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .join(".planning/phases/22-supervision-durable-goals-fleet-loops/22-01-EVIDENCE")
        .join(platform)
        .join("session-journal.bin")
}

/// Reduce a retained corpus through a real cold open, exactly as a restarting
/// product would: no snapshot sidecar is copied, so the whole chain is replayed
/// from frame 0 rather than resumed from a shortcut.
fn reduce_corpus(platform: &str) -> String {
    let source = corpus_path(platform);
    assert!(
        source.exists(),
        "the retained {platform} corpus is missing at {}; this canary is worthless without the real bytes",
        source.display()
    );

    let temp = tempfile::tempdir().expect("tempdir");
    let journal_path = temp.path().join("session-journal.bin");
    std::fs::copy(&source, &journal_path).expect("copy the retained corpus");

    let journal = SessionJournal::open(&journal_path, CORPUS_SESSION_ID)
        .expect("the retained corpus opens and replays under the current binary");
    let state = journal.state().expect("reduced state");
    let value = serde_json::to_value(&state).expect("reduced state serializes");
    state_payload_digest(&value).expect("digest")
}

#[test]
fn the_retained_real_binary_corpus_still_reduces_to_the_pinned_state() {
    let digest = reduce_corpus("linux");
    assert_eq!(
        digest, CORPUS_REDUCED_STATE_DIGEST,
        "reduction semantics changed: the retained real-binary journal no longer \
         replays to the state it replayed to at cd5b4e9b. If this change is \
         intended, it is an F12 behavior change and must be authorized \
         explicitly rather than re-pinned silently."
    );
}

#[test]
fn reducing_the_corpus_twice_is_stable() {
    // Guards the canary itself: a digest that varies run to run (map ordering,
    // a timestamp folded into reduced state) would make the pin above flaky and
    // would be quietly disabled by the first person it woke up.
    assert_eq!(reduce_corpus("linux"), reduce_corpus("linux"));
}
