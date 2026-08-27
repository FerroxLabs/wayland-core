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

    let split = [
        "token=eyJ",
        "hbGciOiJIUzI1NiJ9\n.eyJzdWIiOiJ1c2VyIn0\n.SflKxwRJSMeKKF2QT4fwpMeJf36POk6yJV",
    ]
    .concat();
    let out = s.scrub(&split);
    assert!(out.contains("[REDACTED:JWT]"), "got: {out}");
}

#[test]
fn scrub_bearer_token() {
    let s = PIIScrubber;
    let input = "Authorization: Bearer eyJhbGciOiJSUzI1NiIsInR5cCI6IkpXVCJ9abcdef";
    let out = s.scrub(input);
    assert!(out.contains("[REDACTED:BEARER_TOKEN]"), "got: {out}");

    let split = [
        "Authorization: Bearer \neyJ",
        "hbGciOiJSUzI1NiIsInR5cCI6IkpXVCJ9abcdef",
    ]
    .concat();
    let out = s.scrub(&split);
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

// ── Whitespace-split redaction must not destroy the document (H7) ──────────
//
// `wrapped_base64_candidates()` is greedy and unbounded, and its repetition
// class contains ` `, `\r`, `\n` and `\t`. One secret-shaped token therefore
// reaches to the end of the surrounding whitespace-separated run, and the
// pre-fix code replaced that whole run with a 34-byte marker. These tests pin
// three directions at once: the document survives, every secret the scrubber
// used to remove is still removed, and scrubbing is a fixed point.

/// A GitHub PAT, assembled at run time so the literal never appears in source.
fn planted_pat() -> String {
    ["ghp", "_", "aBCDefGHIjKLmNOPqrSTuvWXyz0123456789ab"].concat()
}

/// An OpenAI key, assembled at run time.
fn planted_openai_key() -> String {
    ["sk", "-", "abcdefghijklmnopqrstuvwxyz0123456789ABCD"].concat()
}

/// ~2.7 KB of ordinary prose. Every character is inside the greedy
/// wrapped-base64 candidate class, so the whole block is one candidate.
fn prose_head() -> String {
    "alpha bravo charlie delta echo foxtrot golf hotel india juliett ".repeat(42)
}

/// ~2.7 KB of different ordinary prose, so containment proves position.
fn prose_tail() -> String {
    "kilo lima mike november oscar papa quebec romeo sierra tango ".repeat(45)
}

const HEAD_PHRASE: &str = "alpha bravo charlie delta echo foxtrot golf hotel india juliett";
const TAIL_PHRASE: &str = "kilo lima mike november oscar papa quebec romeo sierra tango";

#[test]
fn a_split_secret_does_not_swallow_the_surrounding_document() {
    let pat = planted_pat();
    let (head, tail) = pat.split_at(6);
    let input = format!("{}{head}\n{tail} {}", prose_head(), prose_tail());
    assert!(input.len() > 4_000, "fixture must be multi-KB");

    let out = PIIScrubber.scrub(&input);

    // The secret is gone.
    assert!(!out.contains(&pat), "whole key survived");
    assert!(
        !out.contains(tail),
        "key tail survived: {}",
        &out[..200.min(out.len())]
    );
    assert!(out.contains("[REDACTED:"), "nothing was redacted at all");
    // The document is not.
    assert!(
        out.contains(HEAD_PHRASE),
        "prose before the key was destroyed"
    );
    assert!(
        out.contains(TAIL_PHRASE),
        "prose after the key was destroyed"
    );
    assert!(
        out.len() + 200 >= input.len(),
        "scrubber destroyed {} of {} bytes",
        input.len() - out.len(),
        input.len()
    );
}

#[test]
fn a_split_secret_inside_a_punctuated_document_spares_the_prose() {
    // The '.' characters keep the whole-record fast path from firing, so this
    // exercises the greedy wrapped-candidate loop instead.
    let pat = planted_pat();
    let (head, tail) = pat.split_at(6);
    let input = format!(
        "Report follows.\n{}{head}\n{tail} {}\nEnd of report.",
        prose_head(),
        prose_tail()
    );

    let out = PIIScrubber.scrub(&input);

    assert!(!out.contains(&pat), "whole key survived");
    assert!(!out.contains(tail), "key tail survived");
    assert!(out.contains("[REDACTED:"), "nothing was redacted at all");
    assert!(out.starts_with("Report follows."), "head of document lost");
    assert!(
        out.ends_with("End of report."),
        "tail of document lost: {out:?}"
    );
    assert!(
        out.contains(HEAD_PHRASE),
        "prose before the key was destroyed"
    );
    assert!(
        out.contains(TAIL_PHRASE),
        "prose after the key was destroyed"
    );
    assert!(
        out.len() + 200 >= input.len(),
        "scrubber destroyed {} of {} bytes",
        input.len() - out.len(),
        input.len()
    );
}

/// Every case the pre-fix scrubber removed, and what must not survive it.
/// This is the gating arm: shrinking the redaction window must not turn a
/// data-loss bug into a leak.
fn preserved_redactions() -> Vec<(String, Vec<String>)> {
    let pat = planted_pat();
    let key = planted_openai_key();
    let encoded = {
        use base64::Engine as _;
        base64::engine::general_purpose::STANDARD.encode(&key)
    };
    let wrapped_encoded = encoded
        .as_bytes()
        .chunks(9)
        .map(|chunk| std::str::from_utf8(chunk).unwrap())
        .collect::<Vec<_>>()
        .join(" \n\t");

    vec![
        // Unsplit, single token: the pattern is satisfied 24 characters in, so
        // a window that stopped at the shortest match would leak the tail.
        (
            format!("see {pat} here"),
            vec![pat.clone(), pat[24..].to_string()],
        ),
        // Split across a newline mid-token.
        (
            format!("{}\n{}", &pat[..6], &pat[6..]),
            vec![pat.clone(), pat[6..].to_string()],
        ),
        // Split across spaces mid-token.
        (
            format!("{}  {}", &pat[..10], &pat[10..]),
            vec![pat.clone(), pat[10..].to_string()],
        ),
        // The shadowed case: SECRET_ASSIGNMENT is satisfied by "TOKEN=gh"
        // alone, so any fix that stops at the first pattern to fire leaves 38
        // characters of a live PAT behind.
        (
            format!("TOKEN={}\n{}", &pat[..2], &pat[2..]),
            vec![pat.clone(), pat[2..].to_string(), pat[4..].to_string()],
        ),
        // OpenAI key split across a newline.
        (
            format!("{}\n{}", &key[..6], &key[6..]),
            vec![key.clone(), key[6..].to_string()],
        ),
        // AWS access key.
        (
            "key=AKIAIOSFODNN7EXAMPLE and other text".to_string(),
            vec!["AKIAIOSFODNN7EXAMPLE".to_string()],
        ),
        // Whitespace-wrapped base64 record that decodes to a secret.
        (wrapped_encoded.clone(), vec![wrapped_encoded, encoded]),
        // A secret buried in a multi-KB prose run still goes.
        (
            format!(
                "{}{}\n{} {}",
                prose_head(),
                &pat[..6],
                &pat[6..],
                prose_tail()
            ),
            vec![pat.clone(), pat[6..].to_string()],
        ),
    ]
}

#[test]
fn every_secret_the_scrubber_used_to_remove_is_still_removed() {
    for (input, must_vanish) in preserved_redactions() {
        let out = PIIScrubber.scrub(&input);
        assert!(
            out.contains("[REDACTED:"),
            "no redaction for input starting {:?}",
            &input[..40.min(input.len())]
        );
        for needle in &must_vanish {
            assert!(
                !out.contains(needle.as_str()),
                "leaked {:?} — scrubbed output was {:?}",
                &needle[..24.min(needle.len())],
                &out[..240.min(out.len())]
            );
        }
    }
}

#[test]
fn scrubbing_a_split_secret_is_a_fixed_point() {
    let pat = planted_pat();
    let (head, tail) = pat.split_at(6);
    let document = format!("{}{head}\n{tail} {}", prose_head(), prose_tail());

    let mut cases: Vec<String> = preserved_redactions()
        .into_iter()
        .map(|(input, _)| input)
        .collect();
    cases.push(document);

    for input in cases {
        let once = PIIScrubber.scrub(&input).into_owned();
        let twice = PIIScrubber.scrub(&once).into_owned();
        assert_eq!(
            once,
            twice,
            "scrubbing is not idempotent for input starting {:?}",
            &input[..40.min(input.len())]
        );
    }
}

#[test]
fn a_clean_multi_kilobyte_document_is_returned_untouched() {
    // Control for the other direction: the shrink must not become a licence to
    // redact clean prose, and a clean document must come back byte for byte.
    let input = format!("{}{}", prose_head(), prose_tail());
    assert!(input.len() > 4_000);
    let out = PIIScrubber.scrub(&input);
    assert_eq!(out, input, "clean document was modified");
}

#[test]
fn two_split_secrets_in_one_run_are_both_redacted_and_the_prose_between_lives() {
    // Exercises the multi-window path: two secrets, split across CRLF and LF,
    // separated by ordinary prose inside a single greedy candidate run.
    let pat = planted_pat();
    let oauth = ["gh", "s", "_", "ZYXWvutSRQponMLKjihGFEdcba9876543210zz"].concat();
    let middle = " and then some ordinary words in between here ";
    let input = format!(
        "{}{}\r\n{}{middle}{}\n{} {}",
        prose_head(),
        &pat[..6],
        &pat[6..],
        &oauth[..5],
        &oauth[5..],
        prose_tail()
    );

    let out = PIIScrubber.scrub(&input);

    assert!(!out.contains(&pat), "PAT survived");
    assert!(!out.contains(&pat[6..]), "PAT tail survived");
    assert!(!out.contains(&oauth), "OAuth token survived");
    assert!(!out.contains(&oauth[5..]), "OAuth token tail survived");
    assert!(
        out.contains("and then some ordinary words in between here"),
        "prose between the two secrets was destroyed"
    );
    assert!(out.contains(HEAD_PHRASE), "prose before was destroyed");
    assert!(out.contains(TAIL_PHRASE), "prose after was destroyed");
    assert_eq!(
        out.matches("[REDACTED:").count(),
        2,
        "expected exactly two redactions"
    );
    assert!(
        out.len() + 200 >= input.len(),
        "scrubber destroyed {} of {} bytes",
        input.len() - out.len(),
        input.len()
    );
}
