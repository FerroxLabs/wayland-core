---
issue: 1217
repo: FerroxLabs/wayland
kind: defect
title: "The Anthropic provider builds /v1/v1/messages and //v1/messages -- the defect #1178 fixed on the OpenAI wire, still armed here"
status: closed
last_verified_commit: 93ede3424
criteria:
  - id: c1
    text: "https://api.anthropic.com/v1 and https://api.anthropic.com/ as base_url both produce POST /v1/messages"
    state: met
    evidence: "test:crates/wcore-providers/tests/provider_anthropic_test.rs::anthropic_base_url_spellings_all_post_exactly_one_v1_messages"
    owner: core
    note: "Both named spellings are rows, and so is the CROSS PRODUCT the criterion implies: {no trailing slash, trailing slash} x {bare root, /v1} -- suffixes '', '/', '/v1', '/v1/'. The observable is the path a wiremock server actually RECEIVED (`received[0].url.path() == \"/v1/messages\"`), read out of the request log, not the return value of the joiner; the mock is mounted on `/v1/messages` alone, so a doubled or double-slashed path is a 404 and `stream()` errors before the assertion is reached. That is deliberately the condition that decides rather than the artefact edited: if `messages_url` did nothing, the log would show `//v1/messages` or `/v1/v1/messages`. RED ARM 2026-08-30 (mutation M1): `messages_url`'s body restored to `format!(\"{}/v1/messages\", base_url)` -- verified to land on EXECUTABLE code by printing lines 155-157 before and after (`fn messages_url(base_url: &str) -> String {` / body / `}`), not a doc comment -- gives verbatim `base_url \"http://127.0.0.1:38827/\" must reach /v1/messages, got Api { status: 404, message: \"\" }`. In the same run `anthropic_list_models_spellings_all_get_exactly_one_v1_models` and both Cohere tests stayed GREEN, so the mutation is discriminating. Restored with `git checkout --`, touched, re-run `24 tests run: 24 passed`."
  - id: c2
    text: "The joiner is wcore_config::compat::join_endpoint, the one #1178 already added, not a second bespoke trim"
    state: met
    evidence: "absent:crates/wcore-providers/src/anthropic.rs::{}/v1/messages"
    owner: core
    note: "The FIRST branch, satisfied literally: both Anthropic endpoints call `wcore_config::compat::join_endpoint` (imported at the top of the file alongside `ProviderCompat`), and NO new trim, split or format-based join was added anywhere in this lane. This token re-reads the file every run and reds if the bare `format!` is resurrected; its known-positive control is run in the same call -- `grep -c join_endpoint crates/wcore-providers/src/anthropic.rs` returns 3 while `grep -c '{}/v1/messages'` returns 0, so the query is not silently failing. `join_endpoint` is segment-wise, not substring-wise, which is why the collapse is safe: `crates/wcore-config/src/compat.rs::join_endpoint_does_not_collapse_a_substring_match` already pins `/apiv1` and `/v10`, and this lane re-pins the same property end-to-end on the wire in `anthropic_base_url_path_prefix_is_preserved_not_collapsed`."
  - id: c3
    text: "anthropic.rs:676 (/v1/models) and cohere.rs:136 are fixed in the same pass, or each is stated as out of scope with the reason"
    state: met
    evidence: "test:crates/wcore-providers/tests/provider_cohere_url_test.rs::cohere_base_url_spellings_all_post_exactly_one_chat"
    owner: core
    note: "BOTH fixed, neither deferred. (a) `/v1/models`: `list_models` now calls `models_url` -> `join_endpoint`; `trim_end_matches('/')` alone had closed the `//` half and left `/v1/v1` open. Graded by `anthropic_list_models_spellings_all_get_exactly_one_v1_models` over the same four suffixes, whose observable is again the received path -- necessary because `list_models` falls back to the static alias catalog on ANY failure, so the doubled path was SILENT, which is why it survived. RED ARM (mutation M2, `models_url` body -> `format!(\"{}/v1/models\", base_url)`, printed lines 162-164 before and after to prove it landed on executable code): `assertion left == right failed: base_url \"http://127.0.0.1:33347/\" dialed the wrong path, left: \"//v1/models\"` -- the exact defect path the ticket names. Under M2 the messages and prefix tests stayed GREEN. (b) cohere.rs: `join_endpoint(&self.base_url, \"/chat\")`. The ticket recorded it as UNVERIFIED against a live Cohere host, so this lane added the missing test rather than inheriting that gap; `COHERE_DEFAULT_BASE_URL` is `https://api.cohere.com/v1`, so the trailing-slash spelling built `//chat`. RED ARM (mutation M3, back to `format!(\"{}/chat\", self.base_url)`): `base_url \"http://127.0.0.1:40779/v1/\" must reach /v1/chat, got Api { status: 404 }`, with every Anthropic test GREEN in the same run. All three mutations restored with `git checkout --` and touched afterwards."
  - id: c4
    text: "A test asserts the built URL for both spellings; shown RED against today's format!"
    state: met
    evidence: "test:crates/wcore-providers/tests/provider_anthropic_test.rs::anthropic_base_url_spellings_all_post_exactly_one_v1_messages"
    owner: core
    note: "Same test function as c1 on purpose, and the anchors are honestly shared rather than split onto an easier neighbour: c1 grades the BEHAVIOUR (the path the server received for both spellings) and c4 grades that a TEST pins it and that the test was shown RED. The RED is mutation M1, quoted verbatim in c1. Two things this row does NOT claim: the test asserts a received request path, never a helper's return value, so it cannot pass while the request goes elsewhere; and it is paired with a wrong-collapse control, `anthropic_base_url_path_prefix_is_preserved_not_collapsed`, which pins `/anthropic`, `/anthropic/`, `/anthropic/v1` and `/apiv1` -- the MiniMax-shaped proxy base the ticket names. A fix that forced the path to `/v1/messages`, or that stripped the base's path, would pass c1's test and fail that control; both are also red under M1, which is the correct signal since M1 removes the join entirely."
---

The Anthropic provider had the identical /v1-doubling defect #1178 describes.
`try_stream` built the request URL with a bare `format!` -- no join, and unlike
every neighbouring site not even a `trim_end_matches('/')` -- so the two
spellings a user copies out of Anthropic's own docs both 404. Measured live by
the filing sweep, with the working spelling as the positive control:
`POST https://api.anthropic.com/v1/messages -> 401` (routed),
`POST https://api.anthropic.com/v1/v1/messages -> 404`,
`POST https://api.anthropic.com//v1/messages -> 404`.

Closed 2026-08-30 by lane f13-w2-provider-url. All four criteria met as
written; the fix is the joiner #1178 already added, applied at both Anthropic
endpoints and at cohere.rs, plus three neighbouring sites whose local
`trim_end_matches` was the same defect half-closed (flux_fetch.rs,
flux_image.rs, gemini.rs `/v1beta/models`, openai_chatgpt.rs `responses_url`).

COMPLETENESS, stated as a bounded claim rather than an implied one. Within
`crates/wcore-providers/src/`, `grep -rn 'format!("{}/'` now returns ZERO
endpoint joins over a configurable `base_url`: the only hits are
`bedrock.rs:230`/`:243`, whose `endpoint_override` is a test-only wiremock knob
(its own doc comment says so) and never user config, and `resilient.rs:502`,
which is an error-message `provider/model` label, not a URL. The known-positive
control for that grep, run in the same call: `grep -rln join_endpoint crates/`
lists seven provider files plus compat.rs. OUTSIDE wcore-providers the set is
NOT closed and this is an allowlist with named gaps:
`wcore-agent/src/tool_backends/homeassistant.rs:84`, `firecrawl_web.rs:40` and
`gemini_vision.rs:86` each build an endpoint from a configurable base with a
bare `format!`. They are tool backends, not LLM providers, and are outside this
ticket's stated scope ("the Anthropic provider ... anthropic.rs:676;
cohere.rs:136"); they are recorded here rather than silently fixed or silently
dropped.
