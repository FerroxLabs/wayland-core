//! Renderers that lower `TurnElement` variants to ratatui `Line<'static>`s.
//!
//! v0.9.0 TUI-V1 W2: streaming-safe split-point helper ([`safe_split`]).
//! Further per-element renderers (markdown, reasoning-tag filter, thinking
//! blocks, sources blocks, tool cards) land alongside in W2 sibling commits.

// v0.9.0 TUI-V1 W2 C1: markdown → `Vec<Line<'static>>`. Wired into the
// transcript surface in W2 C3.
pub mod markdown;
// v0.9.0 TUI-V1 W2 C4: streaming reasoning-tag filter. Strips
// `<think>`/`<thinking>`/`<reasoning>` blocks (incl. tags split across
// token-chunk boundaries) before they reach the visible streaming buffer.
// v0.9.2 W9 (S24): OSC 8 wrap-survival hyperlink emission — mailto-strip
// + nested-OSC-8 guard. Pure helpers wired into the markdown link path
// and the Sources block.
pub mod osc8;
pub mod reasoning_filter;
pub mod safe_split;

pub use reasoning_filter::ReasoningFilter;
