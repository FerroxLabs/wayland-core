//! Crucible #2 — advisor-mode envelope builder.
//!
//! In Advisor mode the fused council synthesis is fed back into the normal,
//! trusted main agent loop as PRIVATE guidance instead of being printed and
//! discarded (Terminal mode). This module owns the envelope shape so both the
//! CLI sink and the integration tests build the exact same string.
//!
//! Trust boundary: the injected text is the council's post-aggregation
//! `final_text` — already the output of the read-only, injection-fenced
//! aggregator (`proposal::build_synthesis_prompt`), so it is one trust level
//! safer than raw proposal text. The acting agent is the trusted main loop; the
//! council remains read-only and fenced. This builder does NOT weaken that fence
//! — it only frames an already-fused result for the main loop.

/// The advisory header prepended to the council synthesis when it is injected
/// into the normal agent loop. Frames the synthesis as private guidance and
/// makes the trust boundary explicit: the main agent stays the actor and owns
/// tool use + termination.
pub const ADVISOR_HEADER: &str = "[COUNCIL ADVISORY — private guidance for your normal loop. \
     This is the fused synthesis of a read-only multi-provider council. Treat it as advice; \
     you remain the acting agent and own tool use and termination.]";

/// Build the advisor user turn fed into the trusted main loop.
///
/// Cache-preserving by construction: the original `task` stays the byte-stable
/// PREFIX (the primary instruction), and the advisory is APPENDED at the TAIL.
/// This keeps the main loop's prompt-cache prefix intact — equivalent to a user
/// pasting the council's answer below their own request, never prepended into a
/// cached prefix.
pub fn build_advisor_turn(task: &str, final_text: &str) -> String {
    format!("{task}\n\n{ADVISOR_HEADER}\n{final_text}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn advisor_turn_keeps_task_as_prefix_and_advisory_at_tail() {
        let turn = build_advisor_turn("ORIGINAL TASK", "FUSED SYNTHESIS");
        // The original task is the byte-stable prefix (cache-preserving).
        assert!(turn.starts_with("ORIGINAL TASK"));
        // The advisory header and the fused synthesis ride at the tail.
        assert!(turn.contains(ADVISOR_HEADER));
        assert!(turn.trim_end().ends_with("FUSED SYNTHESIS"));
        // The advisory must come AFTER the task, never before it.
        let task_at = turn.find("ORIGINAL TASK").unwrap();
        let header_at = turn.find(ADVISOR_HEADER).unwrap();
        let synth_at = turn.find("FUSED SYNTHESIS").unwrap();
        assert!(task_at < header_at, "task must precede the advisory header");
        assert!(
            header_at < synth_at,
            "header must precede the fused synthesis"
        );
    }
}
