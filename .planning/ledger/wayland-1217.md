---
issue: 1217
repo: FerroxLabs/wayland
kind: defect
title: "The Anthropic provider builds /v1/v1/messages and //v1/messages -- the defect #1178 fixed on the OpenAI wire, still armed here"
status: open
last_verified_commit: 9de21aa1
criteria:
  - id: c1
    text: "https://api.anthropic.com/v1 and https://api.anthropic.com/ as base_url both produce POST /v1/messages"
    state: not-met
    owner: core
    note: "Filed 2026-08-29 by the 0.13.12 close-sweep (worklist defect D35, found while verifying wayland#1178). Nothing has been done. The measured finding, verbatim: The Anthropic provider has the identical /v1-doubling defect #1178 describes, unfixed and untested. `try_stream` builds the request URL with a bare `format!('{}/v1/messages', base_url)` - no join, and unlike every neighbouring site not even a `trim_end_matches('/')`. `--base-url` / `[providers.anthropic].base_url` flows verbatim into it (config.rs:2526 `let base_url = cli.base_url.clone().or_else(...)` -> lib.rs:442 `AnthropicProvider::new(&config.api_key, &config.base_url, ...)`), so the two spellings a user copies out of Anthropic's own docs both 404. Verified LIVE from hetzner, with the working spelling as the positive control: `POST https://api.anthropic.com/v1/messages -> 401` (routed), `POST https://api.anthropic.com/v1/v1/messages -> 404`, `POST https://api.anthropic.com//v1/messages -> 404`. The trailing-slash arm is strictly worse than the OpenAI case ever was: api.openai.com tolerates the double slash (I measured `https://api.openai.com//v1/chat/completions -> 401`), Anthropic does not. Same failure signature #1178 was filed for: a bare 404 with nothing naming the doubled path. `crates/wcore-providers/src/anthropic.rs:676` (`/v1/models`) carries the /v1 half of the same bug; cohere.rs:136 (`format!('{}/chat', self.base_url)`) has the untrimmed-slash half, unverified against a live Cohere host."
  - id: c2
    text: "The joiner is wcore_config::compat::join_endpoint, the one #1178 already added, not a second bespoke trim"
    state: not-met
    owner: core
    note: "Filed 2026-08-29 by the 0.13.12 close-sweep (worklist defect D35). Nothing has been done; the measurement is on c1 and the file:line anchors are in the prose below."
  - id: c3
    text: "anthropic.rs:676 (/v1/models) and cohere.rs:136 are fixed in the same pass, or each is stated as out of scope with the reason"
    state: not-met
    owner: core
    note: "Filed 2026-08-29 by the 0.13.12 close-sweep (worklist defect D35). Nothing has been done; the measurement is on c1 and the file:line anchors are in the prose below."
  - id: c4
    text: "A test asserts the built URL for both spellings; shown RED against today's format!"
    state: not-met
    owner: core
    note: "Filed 2026-08-29 by the 0.13.12 close-sweep (worklist defect D35). Nothing has been done; the measurement is on c1 and the file:line anchors are in the prose below."
---

The Anthropic provider has the identical /v1-doubling defect #1178 describes, unfixed and untested. `try_stream` builds the request URL with a bare `format!('{}/v1/messages', base_url)` - no join, and unlike every neighbouring site not even a `trim_end_matches('/')`. `--base-url` / `[providers.anthropic].base_url` flows verbatim into it (config.rs:2526 `let base_url = cli.base_url.clone().or_else(...)` -> lib.rs:442 `AnthropicProvider::new(&config.api_key, &config.base_url, ...)`), so the two spellings a user copies out of Anthropic's own docs both 404. Verified LIVE from hetzner, with the working spelling as the positive control: `POST https://api.anthropic.com/v1/messages -> 401` (routed), `POST https://api.anthropic.com/v1/v1/messages -> 404`, `POST https://api.anthropic.com//v1/messages -> 404`. The trailing-slash arm is strictly worse than the OpenAI case ever was: api.openai.com tolerates the double slash (I measured `https://api.openai.com//v1/chat/completions -> 401`), Anthropic does not. Same failure signature #1178 was filed for: a bare 404 with nothing naming the doubled path. `crates/wcore-providers/src/anthropic.rs:676` (`/v1/models`) carries the /v1 half of the same bug; cohere.rs:136 (`format!('{}/chat', self.base_url)`) has the untrimmed-slash half, unverified against a live Cohere host.

**Where.** crates/wcore-providers/src/anthropic.rs:154 (also :676; cohere.rs:136)

**Why it matters.** MiniMax already ships an Anthropic-wire base URL by default (`https://api.minimax.io/anthropic`, config.rs:2299) and every Anthropic-compatible proxy publishes its endpoint with the /v1 in it, so this is the same trap #1178 closed, still armed on the other wire. The one-line fix is the joiner that already exists: `wcore_config::compat::join_endpoint(base_url, '/v1/messages')`. It is out of #1178's stated scope ('every OpenAI-compat endpoint'), so it should not block closing #1178 - it needs its own ticket.

Criteria are taken verbatim from the issue's Acceptance section. Nothing has been done: this entry exists so the release gate counts the work rather than anyone having to remember it.
