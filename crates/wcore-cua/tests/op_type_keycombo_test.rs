//! Wave SC SECURITY MAJOR fix — `CuaOp::Type` now routes through the
//! same `check_op` gate as `CuaOp::Key`, refusing:
//!   - null bytes
//!   - ANSI escape sequences
//!   - C0 control chars (except newline + tab)
//!   - forbidden key combos embedded as literal text (Cmd+Q, ⌘Q,
//!     command-q, ^Q, etc.)
//!
//! Closes the audit finding: previously the LLM could bypass the
//! forbidden-key-combo gate by submitting `CuaOp::Type` with the combo
//! as text, which `check_action` skipped entirely.

use wcore_cua::{CuaOp, CuaPolicy, CuaPolicyOutcome};

fn quitter_policy() -> CuaPolicy {
    let mut p = CuaPolicy::permissive();
    p.forbidden_key_combos = vec!["cmd+q".into(), "ctrl+alt+del".into()];
    p
}

fn ty(text: &str) -> CuaOp {
    CuaOp::Type {
        text: text.to_string(),
    }
}

#[test]
fn type_with_null_byte_is_rejected() {
    let p = quitter_policy();
    let r = p.check_op(&ty("hello\0world"), "Finder");
    assert!(
        matches!(r, CuaPolicyOutcome::Reject { .. }),
        "null byte must be rejected, got {r:?}"
    );
}

#[test]
fn type_with_ansi_escape_is_rejected() {
    let p = quitter_policy();
    // ESC [2J — clear-screen ANSI sequence. ESC is U+001B = `\x1b`.
    let r = p.check_op(&ty("\x1b[2Jhello"), "Terminal");
    assert!(
        matches!(r, CuaPolicyOutcome::Reject { .. }),
        "ANSI escape must be rejected, got {r:?}"
    );
}

#[test]
fn type_with_bell_control_char_is_rejected() {
    let p = quitter_policy();
    let r = p.check_op(&ty("\x07alert"), "Terminal");
    assert!(
        matches!(r, CuaPolicyOutcome::Reject { .. }),
        "BEL control char must be rejected, got {r:?}"
    );
}

#[test]
fn type_with_unicode_cmd_glyph_is_rejected() {
    let p = quitter_policy();
    let r = p.check_op(&ty("press ⌘Q to quit"), "TextEdit");
    assert!(
        matches!(r, CuaPolicyOutcome::Reject { .. }),
        "⌘Q embedded in text must be rejected, got {r:?}"
    );
}

#[test]
fn type_with_long_form_cmd_word_is_rejected() {
    let p = quitter_policy();
    let r = p.check_op(&ty("hit Cmd+Q now"), "TextEdit");
    assert!(
        matches!(r, CuaPolicyOutcome::Reject { .. }),
        "Cmd+Q literal text must be rejected, got {r:?}"
    );
}

#[test]
fn type_with_command_dash_q_is_rejected() {
    let p = quitter_policy();
    let r = p.check_op(&ty("then command-q"), "TextEdit");
    assert!(
        matches!(r, CuaPolicyOutcome::Reject { .. }),
        "command-q literal text must be rejected, got {r:?}"
    );
}

#[test]
fn type_with_caret_q_normalizes_to_ctrl_q() {
    // ^Q normalizes to ctrl+q; rejected only when the policy bans ctrl+q.
    let mut p2 = CuaPolicy::permissive();
    p2.forbidden_key_combos = vec!["ctrl+q".into()];
    let r = p2.check_op(&ty("press ^Q"), "TextEdit");
    assert!(
        matches!(r, CuaPolicyOutcome::Reject { .. }),
        "^Q with ctrl+q in forbidden list must be rejected, got {r:?}"
    );
}

#[test]
fn type_with_plain_text_is_allowed() {
    let p = quitter_policy();
    let r = p.check_op(&ty("hello world\nsecond line\ttabbed"), "TextEdit");
    assert_eq!(
        r,
        CuaPolicyOutcome::Allow,
        "plain text with newline + tab should be allowed"
    );
}

#[test]
fn type_with_unicode_letters_is_allowed() {
    let p = quitter_policy();
    let r = p.check_op(&ty("こんにちは 你好 emoji 🚀"), "TextEdit");
    assert_eq!(
        r,
        CuaPolicyOutcome::Allow,
        "non-ASCII letters/emoji must be allowed"
    );
}

#[test]
fn key_op_still_gated_after_lift_to_check_op() {
    let p = quitter_policy();
    let op = CuaOp::Key {
        keys: "cmd+q".into(),
        mods: Default::default(),
    };
    let r = p.check_op(&op, "Finder");
    assert!(
        matches!(r, CuaPolicyOutcome::Reject { .. }),
        "Key cmd+q must still be rejected, got {r:?}"
    );
}

#[test]
fn key_op_with_unicode_glyph_is_rejected() {
    let mut p = CuaPolicy::permissive();
    p.forbidden_key_combos = vec!["cmd+q".into()];
    let op = CuaOp::Key {
        keys: "⌘Q".into(),
        mods: Default::default(),
    };
    let r = p.check_op(&op, "Finder");
    assert!(
        matches!(r, CuaPolicyOutcome::Reject { .. }),
        "Key ⌘Q must normalize and be rejected, got {r:?}"
    );
}
