---
issue: 1243
repo: FerroxLabs/wayland
kind: defect
title: "Two hand-cut authority parsers miss the backslash spelling: the WebFetch prompt calls https://evil.example\\@github.com Trusted, and provider_info.local calls a public endpoint local"
status: closed
last_verified_commit: 93ede3424
criteria:
  - id: c1
    text: "web_fetch_risk(\"https://evil.example\\@github.com/x\") is Risk::External"
    state: met
    evidence: "test:crates/wcore-cli/src/tui/permission/components/webfetch.rs::risk_external_for_every_authority_smuggling_spelling"
    owner: core
    note: "The criterion's exact string is the FIRST row of the list, not a neighbour: `r\"https://evil.example\\@github.com/x\"`. `web_fetch_risk` now takes BOTH the scheme and the host from one `dialed_scheme_host` parse, so the verdict is a function of the address reqwest dials. RED ARM 2026-08-30 (mutation M4): the pre-#1243 hand cut restored inside `web_fetch_risk` -- proven to land on EXECUTABLE code by printing the whole function body before and after the edit (the `strip_prefix(\"https://\")` / `split(['/','?','#'])` / `rsplit('@')` chain, between `pub fn web_fetch_risk` and the ALLOWLIST fold), never a doc comment -- gives verbatim `assertion left == right failed: smuggled authority classified as trusted: https://evil.example\\@github.com/x, left: Trusted, right: External`. Restored with `git checkout --`, touched, re-run green."
  - id: c2
    text: "host_of (webfetch.rs) renders the host the request is actually dialed against, so the prompt title for that URL is not github.com"
    state: met
    evidence: "test:crates/wcore-cli/src/tui/permission/components/webfetch.rs::title_names_the_host_actually_dialed_not_the_smuggled_one"
    owner: core
    note: "Graded through the RENDERED TITLE, not through `host_of`'s return value: the test builds the real permission card, calls `WebFetchComponent.title()`, and asserts the line reads `Fetch evil.example` and explicitly NOT `Fetch github.com` -- the sentence the criterion actually makes. It also asserts the badge on the same card reads `external`, so the two halves of the prompt cannot disagree. RED ARM (mutation M5, `host_of`'s body alone restored to the hand cut, printed lines 157-173 before and after to prove it landed on executable code) is PERFECTLY DISCRIMINATING -- `19 tests run: 18 passed, 1 failed`, exactly this test -- and the failure text is the defect verbatim: `assertion left == right failed: title: Fetch github.com, left: \"Fetch github.com\", right: \"Fetch evil.example\"`. Restored, touched, re-run green."
  - id: c3
    text: "provider_info.local is false for https://evil.example\\@127.0.0.1/v1"
    state: met
    evidence: "test:crates/wcore-protocol/src/events.rs::route_info_is_not_local_for_an_authority_smuggled_loopback"
    owner: core
    note: "The criterion's exact string is the first row. The test grades the PUBLISHED event, `RouteInfo::from_endpoint(...)`, not the private predicate, and asserts two independent things per row: `!route.local`, and that the scrubbed `base_url` published alongside it names the dialed host -- because publishing `https://127.0.0.1/v1` for a request that reached evil.example is the same defect wearing the other hat. TWO separate red arms, landing on two different lines and reddening two different assertions of this test. M6: `is_local_endpoint`'s authority reading replaced by the pre-#1243 hand cut, re-typed through the SAME `match` so only the READING differs -- `public endpoint reported local: https://evil.example\\@127.0.0.1/v1` (events.rs:3436). M7: the `publishable_endpoint` early return deleted from `scrub_base_url`, leaving the hand-cut fallback -- `published base_url must name the dialed host for https://evil.example\\@127.0.0.1/v1: https://127.0.0.1/v1` (events.rs:3412). Both printed before/after to prove they landed on executable code; under each, `route_info_is_still_local_for_a_genuine_loopback_endpoint` stayed GREEN (`9 tests run: 8 passed, 1 failed`), so neither mutation is a blanket break. Restored with `git checkout --` and touched after every mutation AND every restore."
  - id: c4
    text: "All three take the host from a URL parser rather than a hand-cut authority, so the class is closed rather than the one spelling"
    state: met
    evidence: "absent:crates/wcore-cli/src/tui/permission/components/webfetch.rs::rsplit"
    owner: core
    note: "All three now route through ONE module, `crates/wcore-types/src/url_authority.rs` (`dialed_host` / `dialed_host_str` / `dialed_scheme_host` / `publishable_endpoint`), which delegates to `url::Url` -- the WHATWG parser reqwest itself dials with. It lives in wcore-types because that is the lowest crate all three callers already depend on; wcore-protocol's lack of a `url` dep, which the ticket flagged as a dependency decision, is resolved that way rather than by duplicating the parse. `wcore-config::self_hosted::is_self_hosted_base_url` (the #1211 fix) was MIGRATED onto it too, so the unified predicate was extended, not forked -- a third hand-cut parser is exactly what created this bug class. This token re-reads webfetch.rs every run and reds if the cut is resurrected; known-positive control run in the same call: `grep -c rsplit` returns 0 for webfetch.rs and 2 for events.rs, so the query is not silently failing. HONEST REMAINDER, not hidden: those 2 hits in events.rs are `split_endpoint`, retained ONLY as `scrub_base_url`'s redaction fallback for a string that does not parse as a URL at all (nothing reqwest could have dialed either, so there is no dialed host to be wrong about). Its doc comment now says so and forbids anyone asking it which host a request reaches. It is not on the `is_local_endpoint` path any more."
  - id: c5
    text: "A test at each site carries the backslash spelling alongside a wrong-refusal control (an ordinary allowlisted URL still reads Trusted; a genuine loopback endpoint still reads local: true), shown RED against today's hand cut"
    state: met
    evidence: "test:crates/wcore-protocol/src/events.rs::route_info_is_still_local_for_a_genuine_loopback_endpoint"
    owner: core
    note: "Both controls exist and BOTH were shown non-vacuous by their own mutation, which is the point of the criterion -- a classifier that answered External/false for everything would pass the positive tests and quietly destroy the feature. Site B control (this anchor): `http://127.0.0.1:11434/v1`, `localhost`, `[::1]`, `192.168.1.50`, `10.0.0.7`, `ollama.local` still read `local: true`; RED under mutation M8 (`is_local_endpoint`'s Ipv4 arm forced to `false`) -- `http://127.0.0.1:11434/v1 must be reported as a local route`, and it took the pre-existing `route_info_locality_separates_local_from_cloud_on_one_provider_id` down with it, which is the correct blast radius. Site A control: `risk_still_trusted_for_genuinely_allowlisted_urls` in webfetch.rs -- github.com, api.github.com, docs.rs, crates.io, raw.githubusercontent.com, honest `user@github.com` userinfo, and `HTTPS://GitHub.COM/x`. It is RED under mutation M4, and the reason is itself a finding: the old hand cut required a lowercase literal `https://` prefix, so it de-trusted an uppercase scheme. The shared parser also has its own eight-test block; four of them go RED under mutation M9 (`dialed_host` replaced by the pre-#1211/#1243 hand cut), e.g. `https://evil.example\\@github.com/x, left: Some(\"github.com\"), right: Some(\"evil.example\")`. Every mutation restored with `git checkout --` and touched."
---

For a special scheme the WHATWG parser maps `\` to a path separator, so
`https://evil.example\@github.com/x` is a request to `evil.example`. Two
hand-cut authority parsers stopped at `/`, `?` and `#` but not at `\`, took
the last `@`-separated part of what was left, and got `github.com`.

Closed 2026-08-30 by lane f13-w2-provider-url. All five criteria met as
written. The fix EXTENDS the unified predicate #1211/#1212 created rather than
adding a third hand cut: the parse moved down into
`crates/wcore-types/src/url_authority.rs` and `is_self_hosted_base_url` was
migrated onto it in the same pass.

SPELLINGS COVERED, and why that set is CLOSED. The tests carry the backslash
spelling (with and without a path, with a port, and against a second allowlist
entry), #1211's query spelling, the `#` fragment sibling, honest userinfo
pointed the other way (`https://github.com@evil.example/x`), a password
containing `@`, and an embedded tab inside the authority. The set is closed by
CONSTRUCTION, not by enumeration: the host is no longer chosen by a delimiter
list of ours at all -- it comes from the WHATWG authority state machine, which
also strips C0 controls and ASCII whitespace anywhere in the input, applies
IDNA, canonicalises IPv4 in four radices, and rejects forbidden code points.
Those rows are regressions, not the mechanism.

SITES: NOT closed, and this is an allowlist with named gaps. The class is "a
hand cut of an authority followed by a last-`@` selection, used as a security
or display oracle for a URL that will be dialed". After this lane, `grep -rn`
for `rsplit_once('@')` / `rsplit('@')` plus a `split(['/','?','#'])` cut leaves
TWO such sites in the tree, both outside this ticket's stated scope:
`crates/wcore-cli/src/doctor/mod.rs:1011-1021` (`host_of`, a `/doctor`
diagnostic over the user's own configured base_url) and
`crates/wcore-browser/src/policy.rs:1155-1170`
(`strip_pattern_decorations`, which normalises an operator-supplied ALLOWLIST
PATTERN -- the URL being matched against it already comes from
`Url::host_str()`). Four further `split(['/','?','#'])` cuts exist
(`tui/widgets/sources_block.rs:92`, `tui/commands/at_ref_parse.rs:178`,
`tui/tool_formatters/web_fetch.rs:105`, `tui/tool_formatters/web.rs:214`) but
none of them strips userinfo, so they cannot collapse a smuggled authority onto
an allowlisted NAME -- they display the whole `evil.example\@github.com`
string. `crates/wcore-channel-email/src/smtp.rs:327` and
`crates/wcore-types/src/model_aliases.rs:515` split an email address and a
model id, not a URL authority, and are out of class. Known-positive control for
that sweep, in the same call: the query matched events.rs:1988 and sse.rs:909,
both real occurrences.

handoff: FerroxLabs/wayland#1252
