//! Request ORIGIN — who inside this process asked for an outbound request.
//!
//! FerroxLabs/wayland#1264. The egress policy sees a `reqwest::Request` and
//! nothing else, so until this existed it could not tell the agent's own LLM
//! traffic from a request whose URL the MODEL chose. Those two deserve
//! different answers to the same question: allowlisting `github.com` is the
//! operator consenting to the agent reaching GitHub, not to the model picking
//! a query string against it.
//!
//! ## Why an origin marker and not a second policy
//!
//! Two shapes were rejected before this one, both by external review:
//!
//! * **A per-client policy** — give tool clients a stricter `EgressPolicy`.
//!   That makes the boundary depend on WHO CONSTRUCTED THE CLIENT, so any
//!   code path able to build a client is a bypass factory. The policy stays
//!   one policy; only the request is labelled.
//! * **Excluding `-` from the data-bearing token run** — `-` is in the
//!   alphabet of every base64url secret, so dropping it blinds the shape
//!   check to exactly the payload it exists to see.
//!
//! ## How the marker travels, and why it never leaves the process
//!
//! `reqwest::Request::extensions` is `pub(crate)` in reqwest 0.12, so a header
//! is the only per-request channel a caller can write and a policy can read.
//! [`EgressRequestBuilder::send`] REMOVES [`EGRESS_ORIGIN_HEADER`] from the
//! built request after the policy has read it and before the request is
//! dispatched, so the marker is never transmitted. That strip happens at the
//! one seam every outbound request passes through; there is nowhere else for a
//! request to be sent from.
//!
//! ## What an ABSENT marker means
//!
//! [`EgressOrigin::Provider`] — "not tool-originated". This is a positive
//! marker, deliberately: the alternative default would refuse the agent's own
//! provider traffic on every unmarked path, which is the wrong-refusal that
//! breaks an unattended run. Its integrity rests on the tool layer stamping,
//! which is why the stamp is set once on the shared tool client
//! (`build_ssrf_safe_tool_client`) rather than at each call site.

/// The header the origin marker travels in, stripped before dispatch.
///
/// `x-` prefixed and named after this workspace so that, if a future change
/// ever lets one escape, it is unambiguous where it came from.
pub const EGRESS_ORIGIN_HEADER: &str = "x-wayland-egress-origin";

/// Who asked for this outbound request.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EgressOrigin {
    /// The agent's own infrastructure: an LLM provider call, an auth probe, a
    /// telemetry export. The URL is built by this process from configuration.
    Provider,
    /// A tool the model drove. The URL — host, path and query — is chosen,
    /// wholly or in part, by model output.
    Tool,
}

impl EgressOrigin {
    /// The wire spelling carried in [`EGRESS_ORIGIN_HEADER`].
    #[must_use]
    pub const fn as_marker(self) -> &'static str {
        match self {
            Self::Provider => "provider",
            Self::Tool => "tool",
        }
    }

    /// The origin stamped on `request`, or [`EgressOrigin::Provider`] when
    /// none is — see the module header for why absence reads that way.
    #[must_use]
    pub fn of(request: &reqwest::Request) -> Self {
        match request
            .headers()
            .get(EGRESS_ORIGIN_HEADER)
            .and_then(|value| value.to_str().ok())
        {
            Some(marker) if marker == Self::Tool.as_marker() => Self::Tool,
            _ => Self::Provider,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_unmarked_request_reads_as_provider_and_a_marked_one_as_tool() {
        let client = crate::EgressClient::new();

        let plain = client
            .get("https://api.anthropic.com/v1/messages")
            .build_for_test()
            .unwrap();
        assert_eq!(EgressOrigin::of(&plain), EgressOrigin::Provider);

        let stamped = client
            .get("https://github.com/x")
            .origin(EgressOrigin::Tool)
            .build_for_test()
            .unwrap();
        assert_eq!(EgressOrigin::of(&stamped), EgressOrigin::Tool);

        // A request from a client whose DEFAULT origin is Tool is stamped
        // without the call site saying so — the central stamp.
        let tool_client = crate::EgressClient::builder()
            .origin(EgressOrigin::Tool)
            .build()
            .unwrap();
        let from_tool_client = tool_client
            .get("https://github.com/x")
            .build_for_test()
            .unwrap();
        assert_eq!(EgressOrigin::of(&from_tool_client), EgressOrigin::Tool);

        // An UNRECOGNISED marker is not a tool marker. A value this process
        // did not write must never widen anything.
        let forged = client
            .get("https://github.com/x")
            .header(EGRESS_ORIGIN_HEADER, "banana")
            .build_for_test()
            .unwrap();
        assert_eq!(EgressOrigin::of(&forged), EgressOrigin::Provider);
    }
}
