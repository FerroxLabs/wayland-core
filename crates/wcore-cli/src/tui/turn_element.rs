//! Typed building blocks of a completed conversation turn.
//!
//! `TurnView` used to store a single flat `text: String`. v0.9.0 TUI-V1 W1
//! replaces that with `elements: Vec<TurnElement>`, a list of typed pieces
//! the renderer walks per element. This is the data-model precondition for
//! markdown rendering (W2), per-tool formatting, and the Sources block.
//!
//! ## Why a lean shape
//!
//! Per the M2 RECON (`.planning/recon/2026-05-26-current-tui-inventory.md`)
//! tool calls already live on `session.tool_cards: Vec<ToolCardModel>`,
//! which is its own typed/decoupled stream from turns. Folding tools into
//! `TurnElement` would force every tool consumer to walk turn elements —
//! the opposite of decoupling. So the enum stays minimal: just the kinds
//! of content that flow *into the assistant transcript itself*.
//!
//! ## Variants
//!
//! - `Markdown(String)` — assistant body text (the common case). Rendered
//!   with the markdown widget in W2. Pre-W2, the renderer falls back to
//!   line-iteration semantics identical to the old `turn.text` path.
//! - `Thinking { body, secs, tokens }` — persisted reasoning text + the
//!   stream duration + output token count for the turn that produced it.
//!   v0.9.3 extended this from `Thinking(String)` (N-BLOCK-2 closure) so
//!   the collapsed projection can render `▶ Thought: <title> · Ns · N tok`
//!   without parsing meta back out of the body. The live streaming buffer
//!   (`SessionView::thinking`) stays as it is; this variant is for
//!   *persisted* thinking the renderer can scroll back to.
//! - `Sources(Vec<String>)` — a list of citation URLs / file paths the
//!   assistant referenced for this turn. Rendered as a footer block.

/// One typed element of a conversation turn. See module docs for shape
/// rationale.
#[derive(Debug, Clone)]
pub enum TurnElement {
    /// Markdown body text. The common case for assistant turns; user and
    /// system turns also flow through this variant (pre-W2 the renderer
    /// treats it as plain text + line iteration).
    Markdown(String),
    /// v0.9.3 — extended from `Thinking(String)` per N-BLOCK-2 closure.
    /// Persisted reasoning text + the stream duration + token count for the
    /// turn that produced it. The collapsed projection is rendered via
    /// [`crate::tui::render::reasoning_filter::reasoning_collapsed_lines_themed`]
    /// (body, secs, tokens, expanded, theme).
    Thinking {
        body: String,
        secs: u64,
        tokens: u64,
    },
    /// A list of citation URLs / file paths the assistant referenced.
    Sources(Vec<String>),
    /// D037: the project-relative paths the agent touched THIS turn, rendered
    /// as a compact post-turn "Files changed" review card.
    FilesChanged(Vec<String>),
    /// v0.9.1.2 F12: a tool call placeholder. The string is the engine
    /// `call_id` correlating to a [`ToolCardModel`] in
    /// `SessionView::tool_cards`. The renderer looks the card up at this
    /// position in the turn's element vector so tool cards interleave
    /// with assistant text in document order (heading → tool → next
    /// heading → tool, etc.) instead of piling up at the end of the
    /// transcript.
    ToolCard(String),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn turn_element_markdown_preserves_source_text() {
        let body = "# Hello\n\nWorld\n";
        let elem = TurnElement::Markdown(body.to_string());
        if let TurnElement::Markdown(s) = elem {
            assert_eq!(s, body);
        } else {
            panic!("variant mismatch");
        }
    }
}
