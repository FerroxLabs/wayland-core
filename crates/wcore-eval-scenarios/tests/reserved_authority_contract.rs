//! Phase 30 (F30-05) reserved-authority contract suite.
//!
//! **Why these tests exist in this exact form.** An agent on this program invented an extra
//! termination state to dodge an artifact it wrongly believed unobtainable. Inventing a
//! reserved action to route around Sean is the identical move: add an enum member, and the
//! wall is gone. So the defence is not a rule anyone has to remember — it is a type that
//! refuses the invention at DESERIALIZATION, and this file feeds it the literal string that
//! was actually invented.
//!
//! Every refusal below is paired with a pristine control that is ACCEPTED first. A suite that
//! only proves refusals would be passed in full by a verifier that refuses everything, which
//! would say nothing whatsoever about whether an approval can ever be honoured.

use wcore_eval_scenarios::reserved_authority::{
    ALL_RESERVED_ACTIONS, APPROVAL_ROOT_PUBKEY_HEX, ApprovalAuthorityV1, ApprovalRecordV1,
    ApprovalTrustRootV1, PrincipalV1, RESERVED_APPROVAL_SCHEMA, RESERVED_APPROVAL_SCHEMA_VERSION,
    ReservedActionV1, ReservedAuthorityError, RootKindV1, ThrowawayRoot, mint_approval,
};

const SUBJECT: &str = "1111111111111111111111111111111111111111111111111111111111111111";
const OTHER_SUBJECT: &str = "2222222222222222222222222222222222222222222222222222222222222222";

/// A throwaway root plus a frontier-positioning approval that verifies under it.
/// This is the PRISTINE CONTROL every mutation below is derived from.
fn control() -> (ThrowawayRoot, ApprovalRecordV1) {
    let throwaway = ApprovalTrustRootV1::generate_throwaway();
    let approval = mint_approval(
        ReservedActionV1::FrontierPositioning,
        SUBJECT,
        &throwaway.key_id,
        throwaway.seed(),
    )
    .expect("minting under a freshly generated throwaway key must succeed");
    throwaway
        .root
        .verify(&approval)
        .expect("PRISTINE CONTROL: a valid approval must be accepted before any mutation is run");
    (throwaway, approval)
}

// ---------------------------------------------------------------------------
// The closed action enum
// ---------------------------------------------------------------------------

#[test]
fn an_invented_reserved_action_named_termination_state_4_fails_to_deserialize() {
    // PRISTINE CONTROL FIRST: a declared action deserializes.
    let ok: ReservedActionV1 = serde_json::from_str("\"frontier_positioning\"")
        .expect("a declared reserved action must deserialize");
    assert_eq!(ok, ReservedActionV1::FrontierPositioning);

    // THE MUTATION. `termination_state_4` is not a hypothetical: an agent on this program
    // invented exactly that member to route around a wall it wrongly believed impassable.
    // Adding a reserved action to route around Sean is the same move, so the same string is
    // the test input.
    let refused = serde_json::from_str::<ReservedActionV1>("\"termination_state_4\"");
    assert!(
        refused.is_err(),
        "an invented reserved action must fail to DESERIALIZE, not be caught by a later rule"
    );

    // And it must be unroutable through a whole approval record too, not merely as a bare
    // scalar — the shape an agent would actually write.
    let doc = format!(
        r#"{{"schema":"{RESERVED_APPROVAL_SCHEMA}","schema_version":{RESERVED_APPROVAL_SCHEMA_VERSION},"action":"termination_state_4","principal":"sean","subject_sha256":"{SUBJECT}","authority":{{"key_id":"k","signature_base64":"AA=="}}}}"#
    );
    assert!(
        serde_json::from_str::<ApprovalRecordV1>(&doc).is_err(),
        "an approval carrying an invented action must fail to deserialize"
    );
}

#[test]
fn the_reserved_action_enum_is_closed_at_exactly_nine_with_nine_distinct_domains() {
    assert_eq!(ALL_RESERVED_ACTIONS.len(), 9);

    let mut tokens: Vec<&str> = ALL_RESERVED_ACTIONS.iter().map(|a| a.token()).collect();
    tokens.sort_unstable();
    tokens.dedup();
    assert_eq!(tokens.len(), 9, "nine actions must carry nine distinct tokens");

    let mut domains: Vec<&[u8]> = ALL_RESERVED_ACTIONS
        .iter()
        .map(|a| a.signature_domain())
        .collect();
    domains.sort_unstable();
    domains.dedup();
    assert_eq!(
        domains.len(),
        9,
        "each reserved action must carry its OWN signature domain; a shared domain makes one \
         action's approval replayable as another's"
    );
}

// ---------------------------------------------------------------------------
// The single-member principal enum
// ---------------------------------------------------------------------------

#[test]
fn an_approval_whose_principal_is_the_agent_fails_to_deserialize() {
    // PRISTINE CONTROL FIRST.
    let ok: PrincipalV1 =
        serde_json::from_str("\"sean\"").expect("the one declared principal must deserialize");
    assert_eq!(ok, PrincipalV1::Sean);

    // THE MUTATION. There is no agent principal to write, so this is a value that cannot
    // exist rather than a policy violation to detect after the fact.
    for attempt in ["agent", "Agent", "wayland-core", "system", "automation"] {
        let doc = format!("\"{attempt}\"");
        assert!(
            serde_json::from_str::<PrincipalV1>(&doc).is_err(),
            "principal `{attempt}` must fail to deserialize"
        );
    }
}

#[test]
fn a_self_approved_principal_fails_to_deserialize() {
    // PRISTINE CONTROL FIRST: the real principal still works.
    assert_eq!(
        serde_json::from_str::<PrincipalV1>("\"sean\"").expect("control"),
        PrincipalV1::Sean
    );

    for attempt in ["self", "self_approved", "selfApproved", "SELF_APPROVED"] {
        let doc = format!("\"{attempt}\"");
        assert!(
            serde_json::from_str::<PrincipalV1>(&doc).is_err(),
            "a self-approval principal `{attempt}` must fail to deserialize"
        );
    }
}

#[test]
fn an_approval_carrying_an_unknown_field_is_refused_at_the_trust_boundary() {
    let doc = format!(
        r#"{{"schema":"{RESERVED_APPROVAL_SCHEMA}","schema_version":{RESERVED_APPROVAL_SCHEMA_VERSION},"action":"release","principal":"sean","subject_sha256":"{SUBJECT}","authority":{{"key_id":"k","signature_base64":"AA=="}},"approved_by_agent":true}}"#
    );
    assert!(
        serde_json::from_str::<ApprovalRecordV1>(&doc).is_err(),
        "an unknown field must be refused; a field that was silently ignored reads exactly \
         like a field that was honoured"
    );
}

// ---------------------------------------------------------------------------
// Per-action domains, subject binding and key identity
// ---------------------------------------------------------------------------

#[test]
fn an_approval_minted_for_a_source_push_does_not_verify_as_a_release() {
    let throwaway = ApprovalTrustRootV1::generate_throwaway();

    // PRISTINE CONTROL FIRST: the source-push approval verifies as a source push.
    let push = mint_approval(
        ReservedActionV1::SourcePush,
        SUBJECT,
        &throwaway.key_id,
        throwaway.seed(),
    )
    .expect("control mint");
    throwaway
        .root
        .verify(&push)
        .expect("PRISTINE CONTROL: a source-push approval must verify as a source push");

    // THE MUTATION: relabel the action, keeping the same key, the same subject and the same
    // signature bytes. Approving a documentation push must never be replayable as approving
    // a release.
    let mut replayed = push.clone();
    replayed.action = ReservedActionV1::Release;
    let err = throwaway
        .root
        .verify(&replayed)
        .expect_err("a source-push approval must NOT verify as a release");
    assert!(
        matches!(err, ReservedAuthorityError::InvalidSignature { .. }),
        "expected a signature refusal, got {err}"
    );

    // And the same in the other direction, to the action that matters most here.
    let mut as_positioning = push;
    as_positioning.action = ReservedActionV1::FrontierPositioning;
    assert!(
        throwaway.root.verify(&as_positioning).is_err(),
        "a source-push approval must NOT verify as frontier positioning"
    );
}

#[test]
fn an_approval_moved_onto_a_different_subject_digest_is_refused() {
    let (throwaway, approval) = control();

    let mut moved = approval;
    moved.subject_sha256 = OTHER_SUBJECT.to_string();
    let err = throwaway
        .root
        .verify(&moved)
        .expect_err("an approval moved onto a different subject must be refused");
    assert!(
        matches!(err, ReservedAuthorityError::InvalidSignature { .. }),
        "expected a signature refusal, got {err}"
    );
}

#[test]
fn an_unknown_key_id_is_refused_rather_than_trusted() {
    let (throwaway, approval) = control();

    let mut unknown = approval;
    unknown.authority = ApprovalAuthorityV1 {
        key_id: "a-key-this-root-never-declared".to_string(),
        signature_base64: unknown.authority.signature_base64,
    };
    let err = throwaway
        .root
        .verify(&unknown)
        .expect_err("an unknown key id must be refused");
    assert!(
        matches!(err, ReservedAuthorityError::UntrustedKey { .. }),
        "expected an untrusted-key refusal naming the id, got {err}"
    );
    assert!(
        err.to_string().contains("a-key-this-root-never-declared"),
        "the refusal must NAME the key id it refused, got: {err}"
    );
}

#[test]
fn an_approval_signed_by_a_different_root_is_refused() {
    let (mine, _) = control();
    let theirs = ApprovalTrustRootV1::generate_throwaway();

    // Minted under `theirs`, but carrying `mine`'s key id: the id is trusted, the bytes are
    // not. Nothing inside an approval may establish its own authority.
    let forged_bytes = mint_approval(
        ReservedActionV1::FrontierPositioning,
        SUBJECT,
        &theirs.key_id,
        theirs.seed(),
    )
    .expect("mint under the other root");
    let forged = ApprovalRecordV1 {
        authority: ApprovalAuthorityV1 {
            key_id: mine.key_id.clone(),
            signature_base64: forged_bytes.authority.signature_base64,
        },
        ..forged_bytes
    };
    assert!(
        mine.root.verify(&forged).is_err(),
        "a signature from a foreign key must be refused even under a trusted key id"
    );
}

// ---------------------------------------------------------------------------
// The bundled placeholder root — and the positive control that proves the
// mechanism is not merely a verifier that refuses everything
// ---------------------------------------------------------------------------

#[test]
fn frontier_positioning_is_refused_under_the_bundled_placeholder_root() {
    // PRISTINE CONTROL FIRST: this exact approval IS accepted under a real root, so the
    // refusal below is attributable to the placeholder and to nothing else.
    let (_throwaway, approval) = control();

    let bundled = ApprovalTrustRootV1::bundled();
    assert_eq!(bundled.root_kind, RootKindV1::BundledPlaceholder);

    let err = bundled
        .verify(&approval)
        .expect_err("frontier positioning must be REFUSED under the bundled placeholder root");
    assert!(
        matches!(err, ReservedAuthorityError::PlaceholderRoot { .. }),
        "expected a placeholder refusal, got {err}"
    );

    // The refusal must name its own substitution point, so a reader learns what would change
    // it rather than concluding the mechanism is broken.
    let text = err.to_string();
    assert!(text.contains("APPROVAL_ROOT_PUBKEY_HEX"), "got: {text}");
    assert!(
        text.contains("crates/wcore-eval-scenarios/src/reserved_authority.rs"),
        "the refusal must name the exact file the substitution happens in, got: {text}"
    );
    assert!(text.contains("F-030"), "got: {text}");

    // Every one of the nine actions is unreachable under the placeholder, not just this one.
    for action in ALL_RESERVED_ACTIONS {
        let a = ApprovalRecordV1 {
            action,
            ..approval.clone()
        };
        assert!(
            bundled.verify(&a).is_err(),
            "action {} must be unreachable under the placeholder root",
            action.token()
        );
    }
}

#[test]
fn the_bundled_approval_root_is_an_all_zeros_placeholder() {
    assert!(
        APPROVAL_ROOT_PUBKEY_HEX.bytes().all(|b| b == b'0'),
        "the committed approval root must be the all-zeros placeholder; a real key here \
         would make every reserved action reachable from this repository"
    );
    assert_eq!(APPROVAL_ROOT_PUBKEY_HEX.len(), 64);
}

#[test]
fn frontier_positioning_verifies_under_a_throwaway_root_generated_at_run_time() {
    // THE MANDATORY POSITIVE CONTROL. Without it every other test in this file would also
    // pass against a verifier that refuses unconditionally, and the suite would prove
    // nothing about whether an approval can EVER be honoured.
    let throwaway = ApprovalTrustRootV1::generate_throwaway();
    assert_eq!(
        throwaway.root.root_kind,
        RootKindV1::ThrowawayGeneratedAtRunTime,
        "a run-time root must declare itself throwaway so its acceptance can never be \
         mistaken for Sean's approval"
    );

    let approval = mint_approval(
        ReservedActionV1::FrontierPositioning,
        SUBJECT,
        &throwaway.key_id,
        throwaway.seed(),
    )
    .expect("mint");

    let verified = throwaway
        .root
        .verify(&approval)
        .expect("the mechanism MUST be able to accept a valid approval");
    assert_eq!(verified.action, ReservedActionV1::FrontierPositioning);
    assert_eq!(verified.principal, PrincipalV1::Sean);
    assert_eq!(verified.subject_sha256, SUBJECT);
    assert_eq!(
        verified.root_kind,
        RootKindV1::ThrowawayGeneratedAtRunTime,
        "the verified result must carry the root kind forward; an acceptance that does not \
         say WHICH root honoured it is exactly how a clean-room proof gets quoted as authority"
    );

    // And every one of the nine actions is reachable under a real root, so the closure of
    // the enum is not doing the refusing anywhere above.
    for action in ALL_RESERVED_ACTIONS {
        let a = mint_approval(action, SUBJECT, &throwaway.key_id, throwaway.seed())
            .expect("mint for each action");
        throwaway
            .root
            .verify(&a)
            .unwrap_or_else(|e| panic!("action {} must verify under a real root: {e}", action.token()));
    }
}

#[test]
fn an_approval_outside_the_roots_validity_window_is_refused() {
    let (throwaway, approval) = control();

    // PRISTINE CONTROL was already accepted inside `control()`.
    let mut expired = throwaway.root.clone();
    expired.not_after = "2000-01-01T00:00:00Z".to_string();
    let err = expired
        .verify(&approval)
        .expect_err("an approval checked against an expired root must be refused");
    assert!(
        matches!(err, ReservedAuthorityError::OutsideValidityWindow { .. }),
        "expected a validity-window refusal, got {err}"
    );
}

#[test]
fn a_trust_root_carrying_an_unknown_field_is_refused() {
    let raw = serde_json::to_string(&ApprovalTrustRootV1::bundled()).expect("encode");
    // PRISTINE CONTROL FIRST: the pristine document round-trips.
    serde_json::from_str::<ApprovalTrustRootV1>(&raw).expect("the pristine root must parse");

    let mutated = raw.replace("{\"schema\"", "{\"trusted_by_default\":true,\"schema\"");
    assert!(
        serde_json::from_str::<ApprovalTrustRootV1>(&mutated).is_err(),
        "an unknown field on the trust root must be refused"
    );
}
