use std::borrow::Cow;
use wcore_safety::{CheckSet, OutputValidator, PIIScrubber, ValidationFailure};

// ── PIIScrubber tests ──────────────────────────────────────────────────────

#[test]
fn scrub_aws_access_key() {
    let s = PIIScrubber;
    let input = "key=AKIAIOSFODNN7EXAMPLE and other text";
    let out = s.scrub(input);
    assert!(out.contains("[REDACTED:AWS_ACCESS_KEY]"), "got: {out}");
    assert!(!out.contains("AKIAIOSFODNN7EXAMPLE"));
}

#[test]
fn scrub_openai_key() {
    let s = PIIScrubber;
    let input = "Authorization: sk-abcdefghijklmnopqrstuvwxyzABCDEF12";
    let out = s.scrub(input);
    assert!(out.contains("[REDACTED:OPENAI_API_KEY]"), "got: {out}");
    assert!(!out.contains("sk-abcdefghijklmnopqrstuvwxyzABCDEF12"));
}

#[test]
fn scrub_anthropic_key() {
    let s = PIIScrubber;
    let input = "Using key sk-ant-api03-abc123XYZ-def456";
    let out = s.scrub(input);
    assert!(out.contains("[REDACTED:ANTHROPIC_API_KEY]"), "got: {out}");
}

#[test]
fn scrub_jwt() {
    let s = PIIScrubber;
    // Minimal valid-looking JWT (header.payload.signature)
    let input = "token=eyJhbGciOiJIUzI1NiJ9.eyJzdWIiOiJ1c2VyIn0.SflKxwRJSMeKKF2QT4fwpMeJf36POk6yJV";
    let out = s.scrub(input);
    assert!(out.contains("[REDACTED:JWT]"), "got: {out}");
}

#[test]
fn scrub_bearer_token() {
    let s = PIIScrubber;
    let input = "Authorization: Bearer eyJhbGciOiJSUzI1NiIsInR5cCI6IkpXVCJ9abcdef";
    let out = s.scrub(input);
    assert!(out.contains("[REDACTED:BEARER_TOKEN]"), "got: {out}");
}

#[test]
fn scrub_aws_secret_key() {
    let s = PIIScrubber;
    let input = "aws_secret_access_key=wJalrXUtnFEMI/K7MDENG/bPxRfiCYEXAMPLEKEY";
    let out = s.scrub(input);
    assert!(out.contains("[REDACTED:AWS_SECRET_KEY]"), "got: {out}");
}

#[test]
fn scrub_clean_input_borrows() {
    let s = PIIScrubber;
    let input = "Hello, this is a normal log line with no secrets.";
    let out = s.scrub(input);
    // No allocation when nothing matches.
    assert!(
        matches!(out, Cow::Borrowed(_)),
        "expected Borrowed, got Owned"
    );
    assert_eq!(out, input);
}

#[test]
fn scrub_multiple_secrets_in_one_string() {
    let s = PIIScrubber;
    let input = "key=AKIAIOSFODNN7EXAMPLE token=sk-abcdefghijklmnopqrstuvwxyzABCDEF12";
    let out = s.scrub(input);
    assert!(out.contains("[REDACTED:AWS_ACCESS_KEY]"), "got: {out}");
    assert!(out.contains("[REDACTED:OPENAI_API_KEY]"), "got: {out}");
}

// ── OutputValidator tests ──────────────────────────────────────────────────

#[test]
fn validator_clean_output_passes() {
    let v = OutputValidator::new(CheckSet::all());
    assert!(
        v.validate("The task is complete. Here is the result.")
            .is_ok()
    );
}

#[test]
fn validator_detects_refusal() {
    let v = OutputValidator::new(CheckSet::all());
    let err = v
        .validate("I cannot help you with that request.")
        .unwrap_err();
    assert!(matches!(err, ValidationFailure::Refusal { .. }));
    assert!(err.is_warning());
}

#[test]
fn validator_detects_as_an_ai_refusal() {
    let v = OutputValidator::new(CheckSet::all());
    let err = v
        .validate("As an AI, I don't have opinions on that.")
        .unwrap_err();
    assert!(matches!(err, ValidationFailure::Refusal { .. }));
}

#[test]
fn validator_detects_credential_leak() {
    let v = OutputValidator::new(CheckSet::all());
    let err = v
        .validate("The user's key is sk-abcdefghijklmnopqrstuvwxyz1234567")
        .unwrap_err();
    assert!(matches!(err, ValidationFailure::CredentialLeak));
    assert!(!err.is_warning());
}

#[test]
fn validator_format_check_pass() {
    let checks = CheckSet::all().with_format(r"^\{.*\}$");
    let v = OutputValidator::new(checks);
    assert!(v.validate(r#"{"result": "ok"}"#).is_ok());
}

#[test]
fn validator_format_check_fail() {
    let checks = CheckSet::all().with_format(r"^\{.*\}$");
    let v = OutputValidator::new(checks);
    let err = v.validate("plain text, not JSON").unwrap_err();
    assert!(matches!(err, ValidationFailure::FormatMismatch { .. }));
    assert!(!err.is_warning());
}

#[test]
fn validator_credential_leak_takes_priority_over_refusal() {
    // Output has both a credential AND a refusal phrase.
    // Format check absent; credential leak should win over refusal (hard before warning).
    let v = OutputValidator::new(CheckSet::all());
    let err = v
        .validate("I cannot do that. Also my key is sk-abcdefghijklmnopqrstuvwxyz1234567")
        .unwrap_err();
    assert!(
        matches!(err, ValidationFailure::CredentialLeak),
        "expected CredentialLeak, got {err:?}"
    );
}

#[test]
fn validator_refusal_only_check_ignores_credentials() {
    let checks = CheckSet {
        refusal: true,
        credential_leak: false,
        format_regex: None,
    };
    let v = OutputValidator::new(checks);
    // Has a credential but refusal check only — should pass.
    assert!(
        v.validate("Here is your key: sk-abcdefghijklmnopqrstuvwxyz1234567")
            .is_ok()
    );
}
