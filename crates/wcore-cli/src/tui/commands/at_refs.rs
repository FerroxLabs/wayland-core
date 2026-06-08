//! `@`-reference expansion (`@file`, `@dir`, `@symbol`, `@diff`, `@url`,
//! `@session`, `@output`).
//!
//! `@` is the inline counterpart to `/`. Where `/` *does* something, `@`
//! *attaches* something to the next message (UX doc §3b). This is the
//! resolution engine behind that surface — a pure logic module, no
//! `Surface` impl. The composer that consumes it is wired in Wave 2.
//!
//! ## Structure
//!
//! W3-B split the engine into four focused submodules — the single
//! `at_refs.rs` had grown past the AGENTS.md 1000-line guideline. This
//! file is now a thin facade: it re-exports the engine's public API so
//! `at_refs::AtRef`, `at_refs::complete`, etc. resolve exactly as before,
//! and external callers (the workspace composer) need no change.
//!
//! | Submodule          | Responsibility |
//! |--------------------|----------------|
//! | [`at_ref_parse`]   | [`AtRef`] / [`AtRefError`] — parse a `@…` token, no I/O |
//! | [`at_ref_guard`]   | [`is_secret_path`] + [`GitIgnore`] — the attach guardrails |
//! | [`at_ref_complete`]| [`Completion`] / [`complete`] — the autocomplete popup model |
//! | [`at_ref_resolve`] | [`AtPayload`] / [`resolve`] — resolve a ref into message content |
//!
//! Three jobs the engine performs:
//!
//! 1. **Parse** a partial `@…` token in the composer into an [`AtRef`].
//! 2. **Complete** it — given `@cra`, list candidate references the user
//!    can insert ([`complete`]).
//! 3. **Resolve** a finished `@`-reference into an [`AtPayload`] — the
//!    content/files a message carries ([`resolve`]) — with a size budget
//!    and the gitignore + secret-denylist guardrails.
//!
//! ## Guardrails
//!
//! - **Size budget.** Every resolved reference reports its byte size and
//!   an estimated token cost ([`AtPayload::tokens`]). An `@dir` whose tree
//!   would blow [`DIR_TOKEN_WARN_BUDGET`] resolves with a
//!   [`AtWarning::OversizedDir`] so the composer can offer a names-only
//!   fallback before the user sends.
//! - **`.gitignore`.** Resolution walks `.gitignore` files and never pulls
//!   an ignored path into a payload.
//! - **Secret denylist.** `.env` and similar key/credential files are
//!   blocked outright ([`is_secret_path`]) — they are never attached even
//!   if not git-ignored, and the block is surfaced as an error, not a
//!   silent omission.

use super::{at_ref_complete, at_ref_guard, at_ref_parse, at_ref_resolve};

// Re-export the engine's public API so `at_refs::*` paths are unchanged
// for every existing consumer (the workspace composer calls
// `at_refs::complete` / `at_refs::Completion`).
//
// The `tui` module deliberately publishes its whole integration surface
// (Wave-0 contract); only `complete`/`Completion` have a Wave-2 caller so
// far, so the remaining re-exports read as unused — `allow` keeps the
// frozen-contract surface intact without a warning, mirroring the
// module-wide `dead_code` allow in `tui/mod.rs`.
#[allow(unused_imports)]
pub use at_ref_complete::{Completion, complete};
#[allow(unused_imports)]
pub use at_ref_guard::{GitIgnore, is_secret_path};
#[allow(unused_imports)]
pub use at_ref_parse::{AtRef, AtRefError};
#[allow(unused_imports)]
pub use at_ref_resolve::{
    AtPayload, AtWarning, DIR_TOKEN_WARN_BUDGET, PayloadKind, ResolvedFile, estimate_tokens,
    resolve,
};

#[cfg(test)]
mod tests {
    //! Cross-module integration tests for the `@`-reference facade.
    //!
    //! The per-submodule unit tests live with their modules; these prove
    //! the facade re-exports compose — parse → resolve and parse →
    //! complete reach across the module boundary as one engine.

    use super::*;
    use std::fs;
    use tempfile::TempDir;

    #[test]
    fn facade_parse_then_resolve_round_trips_a_file() {
        let tmp = TempDir::new().expect("tempdir");
        let root = tmp.path();
        fs::write(root.join("note.txt"), "hello").expect("write");

        // Both ends of the engine are reachable through the facade path.
        let at = AtRef::parse("@note.txt").expect("parse via facade");
        let payload = resolve(&at, root).expect("resolve via facade");
        assert_eq!(payload.kind, PayloadKind::File);
        assert_eq!(payload.files[0].content, "hello");
    }

    #[test]
    fn facade_completion_and_guard_are_reachable() {
        let tmp = TempDir::new().expect("tempdir");
        let root = tmp.path();
        fs::write(root.join("readme.md"), "# r").expect("write");

        let comps = complete("@read", root);
        assert!(comps.iter().any(|c| c.insert == "@readme.md"));
        // The guardrail re-export answers too.
        assert!(is_secret_path(std::path::Path::new(".env")));
    }

    #[test]
    fn facade_exposes_the_dir_token_budget_constant() {
        // The budget constant is part of the public API the composer reads
        // to label an oversized `@dir`. The non-zero bound is a const, so
        // it is checked at compile time.
        const _: () = assert!(DIR_TOKEN_WARN_BUDGET > 0);
        assert_eq!(estimate_tokens("abcd"), 1);
    }
}
