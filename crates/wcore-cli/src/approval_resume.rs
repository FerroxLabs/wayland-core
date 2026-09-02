//! The `approval_resume` command handler, lifted out of `main.rs`.
//!
//! FerroxLabs/wayland#1180: this is the seam that completes an
//! `ApprovalRequired -> ApprovalResume` loop for a **bridge-backed** approval
//! (a Crucible council card, an egress consent). `main.rs` handles the command
//! in two places — MID-turn, where a bridge-backed approval actually parks, and
//! between turns — with byte-identical bodies. Both were unreachable from a
//! test while they lived inline in a 9000-line `main.rs`: driving the mid-turn
//! arm needs a spawned `--json-stream` binary taken to a live consent verdict.
//!
//! One function, called from both arms, so the mutation named in the ticket —
//! "remove `approval_bridge.resolve(...)`" — is graded.

use wcore_agent::approval::{ApprovalBridge, ApprovalOutcome};
use wcore_protocol::events::ProtocolEvent;
use wcore_protocol::writer::ProtocolEmitter;

/// Route a host's resume decision to the parked approval and echo it.
///
/// Order matters and is load-bearing: the bridge is resolved FIRST, so the
/// awaiting turn is released even if the echo write fails, and the "unknown
/// token" diagnostic is only emitted when nothing was waiting (a stale resume,
/// or a peer guessing ids). The echo is emitted either way so a host UI can
/// clear its pending-approval state.
///
/// Returns whether a pending approval was actually resolved.
pub async fn handle_approval_resume(
    approval_bridge: &ApprovalBridge,
    writer: &dyn ProtocolEmitter,
    resume_token: String,
    approved: bool,
    modifications: Option<serde_json::Value>,
) -> bool {
    let outcome = ApprovalOutcome {
        approved,
        modifications,
        cancellation: None,
    };
    let resolved = approval_bridge.resolve(&resume_token, outcome).await;
    let _ = writer.emit(&ProtocolEvent::ApprovalResume {
        resume_token: resume_token.clone(),
        approved,
    });
    if !resolved {
        let _ = writer.emit(&ProtocolEvent::Info {
            msg_id: String::new(),
            message: format!(
                "approval_resume received for unknown token: {resume_token} (stale resume?)"
            ),
        });
    }
    resolved
}
