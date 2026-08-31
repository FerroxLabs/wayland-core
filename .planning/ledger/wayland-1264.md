---
issue: 1264
repo: FerroxLabs/wayland
kind: defect
title: "Egress: an allowlisted apex is admitted on the host match alone, so tool-driven traffic is never shape-checked (split from #1195 c8)"
status: open
last_verified_commit: 488fbbae9
criteria:
  - id: c1
    text: "A decision is recorded in `.planning/DECISIONS.md` with its reasoning: either the allowlist grant is split by traffic origin (provider vs tool-driven), or the current posture is affirmed as intended and the reason is written down where an operator reading the egress policy can see it."
    state: met
    evidence: "file:.planning/DECISIONS.md"
    owner: core
    note: "MET. The decision is SPLIT, recorded in .planning/DECISIONS.md under 'Egress: split the allowlist grant by traffic origin (FerroxLabs/wayland#1264)' with the measurement, both refuted alternatives (a per-client policy is a bypass factory; excluding '-' from the token run blinds the check to base64url secrets), why ToolData is not Ask (resolve_ask fails OPEN with no doorbell, so a shape check resolved through it would be theatre), the redirect gap, and the cooperative-label bound stated rather than implied. Operator-visible half: classify()'s own doc comment carries the reasoning and points at the decision, and the refusal text says plainly that allowlisting permits the agent to REACH a host and not a tool to choose what to send it."
  - id: c2
    text: "If the split is taken: tool-driven egress to an allowlisted host is shape-checked (method, path, query), and a test drives the real `WebFetch` surface against an allowlisted apex carrying a query payload, shown RED against today's `classify.rs:229` early return."
    state: met
    evidence: "test:crates/wcore-agent/tests/egress_tool_origin_test.rs::webfetch_of_an_allowlisted_apex_with_a_query_payload_is_refused"
    owner: core
    note: "MET AS WRITTEN. The split is taken. Tool-driven egress to an allowlisted host is shape-checked on method, path AND query (request_carries_data = body-bearing method OR get_carries_data, which now has a second call site on the allowlisted branch). The test drives the REAL WebFetch surface -- HttpFetchBackend::new() under the real AgentEgressPolicy via with_default_policy_sync -- against github.com carrying a query payload. RED ARM against today's classify.rs:229 early return, measured LIVE and not modelled: with the early return restored the fetch was not refused at all, it was dispatched and answered -- `got Ok { status: 200, content_type: 'text/html; charset=utf-8' ... }` from github.com, and `got HttpError { status: 404 ... 'documentation_url': 'https://docs.github.com/rest' }` from api.github.com. That closes the ticket's own provenance gap: a process has now issued the request and observed the absence of a prompt. Redirects are validated under the same policy: HttpFetchBackend follows hops itself, re-issuing each through EgressRequestBuilder::send, because reqwest follows a hop inside Client::execute where no policy runs."
  - id: c3
    text: "If the split is taken: provider/LLM traffic to the same apex still receives unconditional `Allow`, with a test that fails if the new check is applied to it — the wrong-refusal control."
    state: met
    evidence: "test:crates/wcore-agent/src/egress/classify.rs::provider_traffic_to_the_same_apex_is_still_unconditionally_allowed"
    owner: core
    note: "MET AS WRITTEN. Provider/LLM traffic to the same apex still receives unconditional Allow, and the test fails if the new check is applied to it -- it asserts EgressVerdict::Allow for a provider POST with a body, for a provider GET with a long high-entropy path, and for a provider POST carrying the exact query payload the tool arm is refused for. Both pre-existing pinning tests (classify::post_to_allowlisted_host_is_allowed, policy::allowlisted_post_is_allowed) stay green unchanged. Further wrong-refusal controls: a data-less TOOL read of an allowlisted host is still allowed silently, a data-less GET to a NEW host still fails open exactly as before, and the non-allowlisted branch is unchanged for both origins."
  - id: c4
    text: "If the current posture is affirmed instead: the two pinning tests gain a comment naming this decision, and `wayland#1195` c8 is closed against the recorded decision rather than left blocked."
    state: met
    evidence: "test:crates/wcore-agent/src/egress/policy.rs::allowlisted_post_is_allowed"
    owner: core
    note: "MET, AND THE ANTECEDENT IS FALSE -- recorded plainly rather than quietly. c4 is the ALTERNATIVE branch of c1 ('if the current posture is affirmed instead'); the posture was NOT affirmed, the split was taken, so c2/c3 are the operative pair. Both concrete obligations c4 names were delivered anyway, so nothing it protects is outstanding: (1) the two pinning tests each gained a doc comment naming this decision and pointing at .planning/DECISIONS.md; (2) wayland#1195 c8 is re-graded from `blocked` to `met` against the delivered fix rather than left blocked -- see .planning/ledger/wayland-1195.md."

---

Created 2026-08-31 to close a COVERAGE gap; GRADED 2026-08-31 by lane
f13-authority, which took the work and the decision. The decision is SPLIT and
it is recorded in `.planning/DECISIONS.md`.

`scripts/check-criteria-ledger.py` scopes every open `area:core` issue on
wayland and EVERY open issue on wayland-core. This issue was in scope from
the moment it was filed and had no ledger file, so
`scripts/check-release-readiness.py` -- which reads ledger files and nothing
else -- could not count it. CI runs the coverage gate with `--offline`, the
arm that would have reported the gap, so nothing said so for two days.

Criteria are transcribed from the issue body without edit. Where the body's
wording is loose it is LEFT loose rather than tightened here: sharpening a
criterion inside the ledger is how a criterion quietly becomes an easier
adjacent property. Whoever takes this restates it on the ISSUE first.
## Independently re-verified 2026-08-31 by lane f13-authority at 488fbbae9

c2's red arm was RE-RUN LIVE rather than taken on the first pass's word. The
split was neutered in place (`if false && origin == EgressOrigin::Tool &&
request_carries_data(method, url)`), `cargo check -p wcore-agent --tests`
returned RC=0 so the mutation genuinely compiled, and the REAL `WebFetch`
surface then dispatched the payload and was answered:

    panicked at crates/wcore-agent/tests/egress_tool_origin_test.rs:71:9:
    a tool-driven payload to an allowlisted apex must be refused, got
    Ok { status: 200, content_type: "text/html; charset=utf-8", text: "..." }

    panicked at crates/wcore-agent/tests/egress_tool_origin_test.rs:96:5:
    a high-entropy path payload to an allowlisted apex must be refused, got
    HttpError { status: 404, message: "HTTP 404 -- {\"message\": \"Not Found\",
    \"documentation_url\": \"https://docs.github.com/rest\", \"status\":
    \"404\"}" }

github.com answered both. That is the defect measured end to end on the shipped
surface, not modelled. With the split restored both arms refuse BEFORE dispatch
and `a_data_less_webfetch_still_reaches_the_origin_under_the_same_policy` stays
green, so the fix did not simply break `WebFetch`.

The mandated direction was checked against the code rather than the commit
message: origin is stamped centrally (`EgressClientBuilder::origin`, set once on
`build_ssrf_safe_tool_client`), it is a LABEL and not a second policy
(`AgentEgressPolicy::check` reads `EgressOrigin::of(request)` and passes it to
the ONE `classify`), the marker is stripped in `EgressRequestBuilder::send`
before dispatch, provider semantics are untouched (absent marker reads as
`Provider`), and the fail-open at the doorbell is not relied on -- `ToolData`
resolves to DENY with no doorbell while `Ask` keeps its deliberate Allow, so no
legitimate unattended provider traffic is blanket-denied. Redirects are followed
by `HttpFetchBackend` itself and re-issued through the gate per hop.

One operator-facing defect was found in this lane's OWN commit and fixed at
`488fbbae9`: the `ToolData` refusal reason carried a 22-space run where a line
continuation was meant, so the message read "...data the model
<22 spaces> chose...". Text only; the verdict and every test are unchanged.
