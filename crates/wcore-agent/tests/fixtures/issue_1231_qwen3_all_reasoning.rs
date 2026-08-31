//! FerroxLabs/wayland#1231 c1 -- a REAL captured stream, not a hand-authored fixture.
//!
//! Captured on hetzner from `qwen3:8b` served by a local Ollama over the
//! OpenAI-compatible route (`POST /v1/chat/completions`, `stream=true`,
//! `temperature=0`), which is the inline-tag class the #908 fix names. These
//! are the 56 `delta.content` strings from that response, in order, verbatim.
//!
//! c1 refuses a hand-authored `TextDelta`, and this is why the refusal
//! matters: capturing it surfaced a fact no authored fixture would have. Over
//! Ollama's OpenAI shim, `<think>` never reaches a client as content at all --
//! the shim parses it out into a separate `reasoning` field (MEASURED: 797
//! reasoning chars, 0 content chars on the first capture). `<thought>` is NOT
//! parsed and arrives inline. So the reporter's tag is the one that actually
//! reproduces, and it is the tag they reported.
//!
//! The model was asked to keep its whole reply inside one `<thought>` block.
//! It took four temperature-0 attempts to get one that complied -- which is
//! itself the measurement behind the c2 design decision to surface rather than
//! retry: a retry asks the same unreliable instruction-following to go the
//! other way.
//!
//! GENERATED -- do not hand-edit. Editing a delta turns a captured stream back
//! into an authored fixture, which is the substitution c1 exists to refuse.

/// The provider content deltas, in arrival order.
pub const CAPTURED_DELTAS: &[&str] = &[
    "<th",
    "ought",
    ">",
    " To",
    " solve",
    " ",
    "2",
    " +",
    " ",
    "2",
    ",",
    " we",
    " perform",
    " basic",
    " arithmetic",
    " addition",
    ".",
    " Comb",
    "ining",
    " two",
    " groups",
    " of",
    " ",
    "2",
    " items",
    " each",
    " results",
    " in",
    " a",
    " total",
    " of",
    " ",
    "4",
    ".",
    " The",
    " calculation",
    " is",
    " straightforward",
    " and",
    " follows",
    " the",
    " fundamental",
    " rules",
    " of",
    " mathematics",
    ".",
    " Therefore",
    ",",
    " the",
    " answer",
    " is",
    " ",
    "4",
    ".</",
    "thought",
    ">",
];
