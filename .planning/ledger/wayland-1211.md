---
issue: 1211
repo: FerroxLabs/wayland
kind: defect
title: "is_self_hosted_base_url reads the authority past a query string, so a public host spelled with ?x=@127.0.0.1 is treated as self-hosted"
status: open
last_verified_commit: 9de21aa1
criteria:
  - id: c1
    text: "https://api.openai.com?x=@127.0.0.1 and https://h?a=@10.0.0.1 are classified NOT self-hosted"
    state: not-met
    owner: core
    note: "Filed 2026-08-29 by the 0.13.12 close-sweep (worklist defect D29, found while verifying wayland#1173). Nothing has been done. The measured finding, verbatim: `is_self_hosted_base_url` misclassifies a PUBLIC host as self-hosted when the base URL carries a query string containing `@` followed by a private/loopback literal, because the authority is taken as everything before the first '/' and then `rsplit('@').next()`. The startup credential gate then exempts the endpoint and the CLI dispatches the user's prompt to the public host with no credential instead of refusing."
  - id: c2
    text: "The authority is parsed with a URL parser, or cut at the first of '/', '?' and '#'"
    state: not-met
    owner: core
    note: "Filed 2026-08-29 by the 0.13.12 close-sweep (worklist defect D29). Nothing has been done; the measurement is on c1 and the file:line anchors are in the prose below."
  - id: c3
    text: "Both spellings are added to the_locality_predicate_rejects_public_hosts, which is shown RED against today's rsplit('@')"
    state: not-met
    owner: core
    note: "Filed 2026-08-29 by the 0.13.12 close-sweep (worklist defect D29). Nothing has been done; the measurement is on c1 and the file:line anchors are in the prose below."
  - id: c4
    text: "With every credential variable unset, --base-url 'https://api.openai.com?x=@127.0.0.1' refuses to start; the run is quoted"
    state: not-met
    owner: core
    note: "Filed 2026-08-29 by the 0.13.12 close-sweep (worklist defect D29). Nothing has been done; the measurement is on c1 and the file:line anchors are in the prose below."
---

`is_self_hosted_base_url` misclassifies a PUBLIC host as self-hosted when the base URL carries a query string containing `@` followed by a private/loopback literal, because the authority is taken as everything before the first '/' and then `rsplit('@').next()`. The startup credential gate then exempts the endpoint and the CLI dispatches the user's prompt to the public host with no credential instead of refusing.

**Where.** crates/wcore-config/src/self_hosted.rs:34-35 (`let authority = after_scheme.split('/').next()` / `let host_port = authority.rsplit('@').next()`), consumed by the exemption at crates/wcore-config/src/config.rs:2683-2686 and by `OpenAIProvider::select_key` at crates/wcore-providers/src/openai.rs:157. The control that claims to pin this polarity is crates/wcore-config/tests/keyless_self_hosted_endpoint_test.rs:154 `the_locality_predicate_rejects_public_hosts`.

**Why it matters.** Observed, not modelled: running the shipped code path with `--base-url 'https://api.openai.com?x=@127.0.0.1'` and every credential var unset did NOT emit 'No API key found' -- it started and made a live request to api.openai.com ('API error 421'). Bounded severity (only the constant `wayland-local` placeholder goes on the wire, no real secret leaks, and the user or their project config must supply the URL), but it defeats the exact boundary #1173's negative control exists to establish, and the existing polarity test passes because its public-host list has no query-string spelling. Fix is to parse the authority with a URL parser (or cut at the first of '/', '?', '#') rather than at '/' alone; add `https://api.openai.com?x=@127.0.0.1` and `https://h?a=@10.0.0.1` to the reject list.

Criteria are taken verbatim from the issue's Acceptance section. Nothing has been done: this entry exists so the release gate counts the work rather than anyone having to remember it.
