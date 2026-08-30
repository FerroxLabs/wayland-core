---
issue: 1243
repo: FerroxLabs/wayland
kind: defect
title: "WebFetch approval labels a backslash-@ URL as Trusted and shows the wrong host: https://evil.example\\@github.com reads as github.com"
status: open
last_verified_commit: 65b95a87
criteria:
  - id: c1
    text: "web_fetch_risk(\"https://evil.example\\@github.com/x\") is Risk::External"
    state: not-met
    owner: core
    note: "Filed 2026-08-30 by lane f13-sec-credgate, found by the elimination sweep the wayland#1211 fix required (after replacing the hand-cut authority in is_self_hosted_base_url, every other by-hand authority cut in the repo was grepped). Nothing has been done. MEASURED, not modelled, from a throwaway probe over the real web_fetch_risk / host_of with url::Url::parse in the same call: `PROBE risk=Trusted host_of=\"github.com\" whatwg_host=Some(\"evil.example\") url=https://evil.example\\@github.com/x`. crates/wcore-cli/src/tui/permission/components/webfetch.rs:62 cuts the authority at the first of '/', '?', '#', so #1211's query spelling is already handled there -- the third probe row is that known-negative control: `PROBE risk=External host_of=\"evil.example\" whatwg_host=Some(\"evil.example\") url=https://evil.example?z=@github.com`. A backslash is not in that delimiter set, and for a special scheme the WHATWG parser maps it to '/', so the dialed host is evil.example."
  - id: c2
    text: "host_of renders the host the request is actually dialed against, so the prompt title for that URL is not github.com"
    state: not-met
    owner: core
    note: "Filed 2026-08-30 by lane f13-sec-credgate. Nothing has been done; the measurement is on c1. host_of is webfetch.rs:158-166 and repeats the same cut, so the approval prompt titles the fetch `github.com` while reqwest dials evil.example -- the user approves a host they were never shown."
  - id: c3
    text: "Both take the host from a URL parser rather than a hand-cut authority, so the class is closed rather than the one spelling"
    state: not-met
    owner: core
    note: "Filed 2026-08-30 by lane f13-sec-credgate. Nothing has been done. The house pattern already exists twice: crates/wcore-mcp/src/transport/sse.rs whatwg_tuple_origin (with resolve_endpoint_backslash_at_smuggle_rejected at :864, the same bypass caught in an audit) and, as of this lane, crates/wcore-config/src/self_hosted.rs is_self_hosted_base_url."
  - id: c4
    text: "A test carries the backslash spelling alongside a wrong-refusal control (an ordinary allowlisted URL still reads Trusted), shown RED against today's split(['/', '?', '#'])"
    state: not-met
    owner: core
    note: "Filed 2026-08-30 by lane f13-sec-credgate. Nothing has been done. webfetch.rs already has a polarity block at :223 (`web_fetch_risk: the pure classifier`) whose rows are the natural home for both arms."
---

`web_fetch_risk` and `host_of` cut the WebFetch authority by hand and stop at
`/`, `?` and `#` but not at `\`. For a special scheme the WHATWG parser maps a
backslash to a path separator, so `https://evil.example\@github.com/x` is a
request to `evil.example` that both functions read as `github.com`: the
approval prompt labels it `Risk::Trusted` and titles it with the wrong host.

Bounded -- this is the risk LABEL and the prompt TITLE, not the allow/deny
decision, and the user is still prompted. But the label exists so a user can
skim-approve a trusted host, and model-supplied URLs reach this prompt.

Filed by lane f13-sec-credgate while closing wayland#1211 and #1212. Searched
for an existing carrier first, by symptom (`webfetch authority`,
`web_fetch_risk`, `backslash`, `userinfo`, `host_of allowlist`), by originating
issue (`1211`), and by component (all 132 open FerroxLabs/wayland and 55 open
FerroxLabs/wayland-core issues filtered on webfetch / allowlist / authority /
host / url / permission / spoof / origin). Nothing carried it.
