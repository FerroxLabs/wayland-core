---
issue: 1243
repo: FerroxLabs/wayland
kind: defect
title: "Two hand-cut authority parsers miss the backslash spelling: the WebFetch prompt calls https://evil.example\\@github.com Trusted, and provider_info.local calls a public endpoint local"
status: open
last_verified_commit: 65b95a87
criteria:
  - id: c1
    text: "web_fetch_risk(\"https://evil.example\\@github.com/x\") is Risk::External"
    state: not-met
    owner: core
    note: "Filed 2026-08-30 by lane f13-sec-credgate, found by the elimination sweep the wayland#1211 fix required (after replacing the hand-cut authority in is_self_hosted_base_url, every other by-hand authority cut in the repo was grepped; wcore-mcp's SSE origin gate was already hardened, these two were not). Nothing has been done. MEASURED, not modelled, from a throwaway probe over the real web_fetch_risk / host_of with url::Url::parse in the same call: `PROBE risk=Trusted host_of=\"github.com\" whatwg_host=Some(\"evil.example\") url=https://evil.example\\@github.com/x`. crates/wcore-cli/src/tui/permission/components/webfetch.rs:62 cuts the authority at the first of '/', '?', '#', so #1211's query spelling is already handled -- the control row in the same probe: `PROBE risk=External host_of=\"evil.example\" whatwg_host=Some(\"evil.example\") url=https://evil.example?z=@github.com`. A backslash is not in that delimiter set, and for a special scheme the WHATWG parser maps it to '/', so the dialed host is evil.example."
  - id: c2
    text: "host_of (webfetch.rs) renders the host the request is actually dialed against, so the prompt title for that URL is not github.com"
    state: not-met
    owner: core
    note: "Filed 2026-08-30 by lane f13-sec-credgate. Nothing has been done; the measurement is on c1 (`host_of=\"github.com\"` against `whatwg_host=Some(\"evil.example\")`). host_of is webfetch.rs:158-166 and repeats the same cut, so the approval prompt titles the fetch `github.com` while reqwest dials evil.example -- the user approves a host they were never shown."
  - id: c3
    text: "provider_info.local is false for https://evil.example\\@127.0.0.1/v1"
    state: not-met
    owner: core
    note: "Filed 2026-08-30 by lane f13-sec-credgate. Nothing has been done. SECOND SITE, and the more dangerous direction: a PUBLIC endpoint reports local: true, i.e. the flag tells the user their prompt never left the machine on the one shape where it did. crates/wcore-protocol/src/events.rs:1948 split_endpoint cuts at ['?','#'] then at '/', consumed by host_of at :1975 and is_local_endpoint at :1999. MEASURED from a probe over the real functions, with three controls in the same call -- the query spelling that IS handled, a genuine loopback, and a plain public host: `PROBE local=true host_of=\"127.0.0.1\" url=https://evil.example\\@127.0.0.1/v1` / `PROBE local=false host_of=\"evil.example\" url=https://evil.example?z=@127.0.0.1` / `PROBE local=true host_of=\"127.0.0.1\" url=http://127.0.0.1:11434/v1` / `PROBE local=false host_of=\"api.openai.com\" url=https://api.openai.com/v1`. The dialed host for row one, from url::Url::parse: `PROBE2 whatwg_host=Some(\"evil.example\") url=https://evil.example\\@127.0.0.1/v1`. docs/json-stream-protocol.md:2174 promises of this flag: 'local is decided against a parsed IP literal, never a string prefix ... this flag is what a user trusts to conclude their prompt never left the machine.' The IP literal IS parsed; the string it is parsed from is not."
  - id: c4
    text: "All three take the host from a URL parser rather than a hand-cut authority, so the class is closed rather than the one spelling"
    state: not-met
    owner: core
    note: "Filed 2026-08-30 by lane f13-sec-credgate. Nothing has been done. The house pattern already exists twice: crates/wcore-mcp/src/transport/sse.rs whatwg_tuple_origin (with resolve_endpoint_backslash_at_smuggle_rejected at :864, the same bypass caught in an audit) and, as of this lane, crates/wcore-config/src/self_hosted.rs is_self_hosted_base_url -- which was measured against this exact string and returns false: `PROBE2 ... self_hosted=false url=https://evil.example\\@127.0.0.1/v1`. Note wcore-protocol deliberately holds no url dependency today, so closing site B is a dependency decision as well as a code change; that is the reason this is its own ticket and not a hunk in the #1211 fix."
  - id: c5
    text: "A test at each site carries the backslash spelling alongside a wrong-refusal control (an ordinary allowlisted URL still reads Trusted; a genuine loopback endpoint still reads local: true), shown RED against today's hand cut"
    state: not-met
    owner: core
    note: "Filed 2026-08-30 by lane f13-sec-credgate. Nothing has been done. Both sites already have a polarity block to extend: webfetch.rs:223 (`web_fetch_risk: the pure classifier`) and the wcore-protocol events tests at events.rs:2297."
---

For a special scheme the WHATWG parser maps `\` to a path separator, so
`https://evil.example\@github.com/x` is a request to `evil.example`. Two
hand-cut authority parsers stop at `/`, `?` and `#` but not at `\`, take the
last `@`-separated part of what is left, and get `github.com`.

#1211's query-string spelling is already handled at both. It is specifically
the backslash spelling that is open.

Bounded at both sites: neither is an allow/deny decision. Site A is the risk
LABEL and the prompt TITLE, and the user is still prompted; site B is a
diagnostic field. But the label exists so a user can skim-approve a trusted
host, and the docs promise site B is exactly what a user trusts about whether
their prompt left the machine.

Filed by lane f13-sec-credgate while closing wayland#1211 and #1212. Searched
for an existing carrier first, by symptom (`webfetch authority`,
`web_fetch_risk`, `backslash`, `userinfo`, `host_of allowlist`), by originating
issue (`1211`), and by component (all 132 open FerroxLabs/wayland and 55 open
FerroxLabs/wayland-core issues filtered on webfetch / allowlist / authority /
host / url / permission / spoof / origin). Nothing carried it. Both sites are
one ticket on purpose -- one defect class, one fix pattern, and splitting them
is how a future lane closes one and leaves the other.
