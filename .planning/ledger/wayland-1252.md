---
issue: 1252
repo: FerroxLabs/wayland
kind: defect
title: "Three more hand-cut authority parsers survive #1243: /doctor suppresses its base-url caveat, a browser origin pattern normalises to a different host, and a redaction renders the smuggled host as the surviving one"
status: open
last_verified_commit: 1775bc762
criteria:
  - id: c1
    text: "With base_url = https://evil.example\\@api.openai.com/v1 and provider openai, /doctor PRINTS the base-url caveat naming the vendor host"
    state: not-met
    owner: core
    note: "Filed 2026-08-30 by lane f13-w2-provider-url while closing wayland#1243, by the elimination sweep that ticket's c4 demanded. Nothing has been done. SITE A, and the meaningful one. crates/wcore-cli/src/doctor/mod.rs:1011-1021 (host_of), consumed at :999-1000 as `vendor_host == host_of(&cfg.base_url)`. That configured base_url cuts to `api.openai.com`, which EQUALS vendor_host, so `base_url_caveat` returns an empty Vec and the caveat is NOT printed -- while reqwest dials evil.example. The function's own doc comment names wayland#1079 and says the point is that a diagnostic must not answer a question the user did not ask; the backslash spelling voids exactly that. Bounded honestly: a diagnostic, not an allow/deny gate, and base_url is the user's own config rather than model-supplied."
  - id: c2
    text: "origin_matches(\"github.com\", r\"https://evil.example\\@github.com\") is false"
    state: not-met
    owner: core
    note: "Filed 2026-08-30 by lane f13-w2-provider-url while closing wayland#1243, by the elimination sweep that ticket's c4 demanded. Nothing has been done. SITE B, lower severity and the bound matters. crates/wcore-browser/src/policy.rs:1149-1170 (strip_pattern_decorations, via normalize_origin_pattern -> origin_matches) normalises an operator-supplied ALLOWLIST PATTERN. The URL being EVALUATED is parsed properly (Url::parse at policy.rs:411, :613, :682), so this is not a bypass driven by a navigated URL -- it is a pattern that silently allows a host other than the one written. Listed with Site A because it is the same cut, and splitting them is how a future lane closes one and leaves the other."
  - id: c3
    text: "Both sites take the host from wcore_types::url_authority rather than a hand-cut authority, so the class is closed rather than the one spelling"
    state: not-met
    owner: core
    note: "Filed 2026-08-30 by lane f13-w2-provider-url while closing wayland#1243, by the elimination sweep that ticket's c4 demanded. Nothing has been done. The fix already exists: crates/wcore-types/src/url_authority.rs (dialed_host / dialed_host_str / dialed_scheme_host / publishable_endpoint), added by #1243 in the lowest crate every caller already depends on. A third hand cut is what created this class; do not add a fourth. NOT in scope, and stated so the next lane does not re-derive it: four further split(['/','?','#']) cuts exist (tui/widgets/sources_block.rs:92, tui/commands/at_ref_parse.rs:178, tui/tool_formatters/web_fetch.rs:105, tui/tool_formatters/web.rs:214) but NONE strips userinfo, so none can collapse a smuggled authority onto an allowlisted NAME; wcore-channel-email/src/smtp.rs:327 and wcore-types/src/model_aliases.rs:515 split an email address and a model id, not a URL authority; wcore-protocol/src/events.rs:1988 (split_endpoint) is retained by design as scrub_base_url's redaction-only fallback and its doc comment forbids asking it which host a request reaches."
  - id: c4
    text: "A test at each site carries the backslash spelling alongside a wrong-refusal control (a genuinely different configured host still PRINTS the caveat and a matching one still suppresses it; an ordinary *.github.com pattern still matches api.github.com), shown RED against today`s hand cut"
    state: not-met
    owner: core
    note: "Filed 2026-08-30 by lane f13-w2-provider-url while closing wayland#1243, by the elimination sweep that ticket's c4 demanded. Nothing has been done. The controls are named in the criterion on purpose: #1243`s own red arm showed that a mutation which simply refuses everything passes the positive test and destroys the feature (mutation M8 there reddened the genuine-loopback control), so each site needs both directions. doctor/mod.rs:1856-1863 already has a host_of test block to extend; policy.rs has its allowlist tests in the same file."
  - id: c5
    text: "scrub_detail over a smuggled authority does not name the surviving host as the allowlisted one: scrub_detail(r\"https://evil.example\\@github.com/x\") differs from scrub_detail(\"https://user:pw@github.com/x\"), with a test carrying both alongside a control that an ordinary credential-bearing URL is still redacted"
    state: not-met
    owner: core
    note: "Added 2026-08-30 by lane f13-w2-provider-url after an independent verifier swept with a third instrument this lane had not used. Nothing has been done. SITE C, the lowest of the three. crates/wcore-config/src/portability/redact.rs:157 (strip_url_userinfo, reached via scrub_detail on DiscoveredItem::details) cuts with find(\"://\") + rest.find(\\'/\\') + find(\\'@\\'). Measured by faithful transcription: r\"https://evil.example\\@github.com/x\" and \"https://user:pw@github.com/x\" BOTH emit \"https://<redacted>@github.com/x\" -- identical, so the reader cannot tell the first dials evil.example -- while the control \"https://github.com/x\" is returned unchanged. Bounded: redaction only, it over-redacts rather than under-redacts, and it is a display path rather than a gate. RECORDED BECAUSE THE MISS IS THE FINDING: this lane\\'s sweep enumerated cutting IDIOMS (rsplit_once(\\'@\\'), split([\\'/\\',\\'?\\',\\'#\\'])) and find(\"://\") is not in that alphabet. Enumerating idioms over an open alphabet cannot terminate; the decidable question is the inverted one -- which functions return a host- or authority-shaped value WITHOUT going through url::Url -- and that is what c3 should be graded against."
---

Found while closing wayland#1243, by the elimination sweep that ticket`s c4
demanded. A hand cut takes everything before the first `/` (sometimes `?`, `#`)
and then the LAST `@`-separated part. For a special scheme the WHATWG parser
maps `\` to a path separator, so `https://evil.example\@github.com/x` is a
request to `evil.example` -- and the cut reads `github.com`. That is #1211 and
#1243. These two sites still read it that way, and neither is inside #1243`s
stated scope, which is why this is its own ticket.

Searched for an existing carrier first, by symptom (`authority`, `backslash`,
`doctor host_of`, `url parser spoof host`, `BrowserPolicy allowlist origin`),
by originating issue (`1211`, `1243`), and by component across open
FerroxLabs/wayland + FerroxLabs/wayland-core. The only hits were #1211 and
#1243 themselves. Control run in the same session: the query `sandbox`
returned 12 open issues, so the search was not silently returning nothing.
