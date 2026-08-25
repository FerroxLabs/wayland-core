//! #863 — the Flux loop-ownership anti-collision handshake, client half.
//!
//! ## The one rule
//!
//! **Exactly one ladder per task.** wayland-core's Anvil is a CLIENT-side climb
//! (worktree builders + sandboxed gate + receipt). Flux's Elevation is a
//! SERVER-side climb. A request must never run both: two ladders double the
//! work, converge on each other's output, and produce two receipts each of
//! which is wrong about the other.
//!
//! ## The handshake
//!
//! Request, when the endpoint declares it speaks the contract:
//! - `X-Flux-Loop-Owner: anvil` — Core owns the loop. Flux must not engage
//!   Elevation on this turn regardless of alias, and must not stamp
//!   verification on it (it is mid-loop material, not a finished task).
//! - `X-Flux-Verify: true` — the opposite, explicit per-request opt-in that
//!   lets Flux run Elevation on `flux-auto`. Mutually exclusive with the above
//!   by construction; see [`FluxLoopIntent`].
//! - `metadata.loop_owner` / `metadata.flux_verify` / `metadata.nonce` — the
//!   same three facts in the request body, for OpenAI-wire shapes that carry a
//!   top-level `metadata` object. Flux reads EITHER carrier.
//!
//! Response:
//! - `X-Flux-Loop-Engaged: none | cascade | elevation` — which ladder Flux
//!   actually ran. `elevation` on a turn we marked `ClientOwned` is a hard
//!   fault: see [`is_collision`].
//!
//! ## Why this module exists
//!
//! Three translation paths (OpenAI chat, OpenAI Responses, Anthropic Messages)
//! have to agree on the wire spelling. Duplicating the header names across them
//! is how one path silently stops emitting and nobody notices until a receipt
//! lies. There is one implementation here and every path calls it.

use reqwest::header::{HeaderMap, HeaderName, HeaderValue};
use serde_json::{Value, json};
use wcore_config::compat::ProviderCompat;
use wcore_types::llm::LlmRequest;

/// Request header carrying `loop_owner` (F2).
pub const LOOP_OWNER_HEADER: &str = "x-flux-loop-owner";
/// Request header carrying the explicit Elevation opt-in (F1/F5).
pub const VERIFY_HEADER: &str = "x-flux-verify";
/// Response header echoing which ladder Flux ran (F2).
pub const LOOP_ENGAGED_HEADER: &str = "x-flux-loop-engaged";
/// The `loop_engaged` value that collides with a client-owned loop.
pub const LOOP_ENGAGED_ELEVATION: &str = "elevation";

/// Apply the loop-provenance REQUEST HEADERS.
///
/// Emits nothing at all unless the endpoint declares the handshake via
/// [`ProviderCompat::flux_loop_provenance`], so a non-Flux deployment's request
/// is left byte-for-byte unchanged and Core never leaks its internal loop state
/// to a third-party endpoint that has no contract to honour it.
///
/// This is the carrier that works on EVERY wire shape, including Anthropic
/// Messages, whose `metadata` object accepts only `user_id` and rejects
/// arbitrary keys. Anything that must survive all three translations rides
/// here, not in the body.
pub(crate) fn apply_loop_headers(
    headers: &mut HeaderMap,
    request: &LlmRequest,
    compat: &ProviderCompat,
) {
    if !compat.flux_loop_provenance() {
        return;
    }
    let Some(intent) = request.flux_loop_intent.as_ref() else {
        return;
    };
    match intent.owner() {
        Some(owner) => {
            if let Ok(v) = HeaderValue::from_str(owner) {
                headers.insert(HeaderName::from_static(LOOP_OWNER_HEADER), v);
            }
        }
        None => {
            // `ServerVerify`. Unreachable from driver traffic by construction:
            // an `LlmRequest` cannot hold both arms, so a turn that asked for
            // the server ladder never carried a `loop_owner` to begin with.
            headers.insert(
                HeaderName::from_static(VERIFY_HEADER),
                HeaderValue::from_static("true"),
            );
        }
    }
}

/// Apply the loop-provenance REQUEST BODY `metadata` object.
///
/// OpenAI-wire only (chat completions and Responses both accept a top-level
/// `metadata` object). Same endpoint gate as [`apply_loop_headers`]: a
/// non-declaring endpoint keeps a byte-identical body, which also keeps the
/// strict OpenAI-compatible servers (Ollama, llama.cpp, vLLM) from 400-ing on
/// an unknown top-level field.
///
/// Merges into an existing `metadata` object rather than replacing it, so this
/// never clobbers a key some other layer put there.
pub(crate) fn apply_loop_metadata(body: &mut Value, request: &LlmRequest, compat: &ProviderCompat) {
    if !compat.flux_loop_provenance() {
        return;
    }
    let mut pairs: Vec<(&str, Value)> = Vec::new();
    if let Some(intent) = request.flux_loop_intent.as_ref() {
        match intent.owner() {
            Some(owner) => pairs.push(("loop_owner", json!(owner))),
            None => pairs.push(("flux_verify", json!(true))),
        }
    }
    // F3 — per-turn cache variance. Rides with the marking, and only with it:
    // a nonce on unmarked traffic would defeat the semantic cache for every
    // ordinary turn, which is a cost regression, not a fix.
    if request.flux_loop_intent.is_some()
        && let Some(nonce) = request.flux_turn_nonce.as_deref()
        && !nonce.trim().is_empty()
    {
        pairs.push(("nonce", json!(nonce)));
    }
    if pairs.is_empty() {
        return;
    }
    if !body["metadata"].is_object() {
        body["metadata"] = json!({});
    }
    for (k, v) in pairs {
        body["metadata"][k] = v;
    }
}

/// Read the `x-flux-loop-engaged` response echo.
pub fn parse_loop_engaged(headers: &HeaderMap) -> Option<String> {
    headers
        .get(LOOP_ENGAGED_HEADER)
        .and_then(|v| v.to_str().ok())
        .map(str::to_string)
}

/// The runtime collision detector (F2).
///
/// True when Core declared it owns the loop for this turn and Flux ran its
/// server-side Elevation ladder anyway. Both ladders climbed the same task:
/// whatever came back is contaminated mid-loop material and must not be
/// accepted as a candidate.
///
/// Deliberately narrow. `cascade` is NOT a collision — Cascade is a single-tier
/// climb-on-failure, per-request and origin-tier billed, which F1 explicitly
/// permits. `none` is the expected answer. A missing header is not a collision
/// either: a non-Flux endpoint never sends one, and treating silence as a fault
/// would fail every Anthropic turn in the workspace.
pub fn collides(loop_owner: Option<&str>, loop_engaged: Option<&str>) -> bool {
    loop_owner.is_some()
        && loop_engaged.is_some_and(|v| v.eq_ignore_ascii_case(LOOP_ENGAGED_ELEVATION))
}

/// [`collides`], read straight off a request. Same predicate; this is the form
/// a provider uses, `collides` is the form the engine uses once it has already
/// destructured the intent.
pub fn is_collision(request: &LlmRequest, loop_engaged: Option<&str>) -> bool {
    collides(
        request.flux_loop_intent.as_ref().and_then(|i| i.owner()),
        loop_engaged,
    )
}

/// The operator-facing description of a collision, used in the turn error and
/// the seat notes so the two cannot drift.
pub fn collision_message(owner: &str, engaged: &str) -> String {
    format!(
        "flux loop-ownership collision: this turn declared `loop_owner={owner}` \
         (wayland-core owns the climb) but the router replied \
         `x-flux-loop-engaged: {engaged}` — both ladders ran, so the candidate is \
         mid-loop material contaminated by a server-side climb and was dropped"
    )
}
