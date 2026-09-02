---
issue: 1252
repo: FerroxLabs/wayland
kind: defect
title: "Three more hand-cut authority parsers survive #1243: /doctor suppresses its base-url caveat, a browser origin pattern normalises to a different host, and a redaction renders the smuggled host as the surviving one"
status: closed
last_verified_commit: 488fbbae9
criteria:
  - id: c1
    text: "With base_url = https://evil.example\\@api.openai.com/v1 and provider openai, /doctor PRINTS the base-url caveat naming the vendor host"
    state: met
    evidence: "test:crates/wcore-cli/src/doctor/mod.rs::the_base_url_caveat_is_printed_for_every_host_that_is_not_the_vendor"
    owner: core
    note: "MET. base_url_caveat now asks wcore_types::url_authority::dialed_host_str for BOTH sides of the comparison; the hand cut host_of is deleted. RED ARM, verbatim, with the cut restored and the fix's test kept: `panicked at crates/wcore-cli/src/doctor/mod.rs:1888:9: a base_url that dials evil.example must still print the caveat`. Fail direction is loud: a base_url whose host the parser cannot name (None) is treated as DIFFERENT from the vendor host, so the caveat is printed. Wrong-refusal control in the same test: api.openai.com, API.OpenAI.COM and user:<pw>@api.openai.com all still SUPPRESS it."
  - id: c2
    text: "origin_matches(\"github.com\", r\"https://evil.example\\@github.com\") is false"
    state: met
    evidence: "test:crates/wcore-browser/src/policy.rs::a_smuggled_authority_in_a_pattern_names_the_host_it_dials"
    owner: core
    note: "MET AS WRITTEN. origin_matches('github.com', r'https://evil.example\@github.com') is false, and origin_matches('evil.example', ...) is true, so the pattern is not merely inert. RED ARM, verbatim: `panicked at crates/wcore-browser/src/policy.rs:1780:9: a pattern that PARSES as evil.example must not match github.com`."
  - id: c3
    text: "Both sites take the host from wcore_types::url_authority rather than a hand-cut authority, so the class is closed rather than the one spelling"
    state: met
    evidence: "symbol:crates/wcore-browser/src/policy.rs::strip_pattern_decorations"
    owner: core
    note: "MET. All THREE sites take the host from wcore_types::url_authority: doctor/mod.rs base_url_caveat (dialed_host_str), policy.rs strip_pattern_decorations (dialed_host_str over an assembled URL), and portability/redact.rs strip_url_userinfo (url::Url, and a string with no userinfo returned unchanged). No hand cut remains at any of them. The four out-of-scope split(['/','?','#']) cuts named in the original note are unchanged and still out of scope: none strips userinfo, so none can collapse a smuggled authority onto an allowlisted NAME."
  - id: c4
    text: "A test at each site carries the backslash spelling alongside a wrong-refusal control (a genuinely different configured host still PRINTS the caveat and a matching one still suppresses it; an ordinary *.github.com pattern still matches api.github.com), shown RED against today`s hand cut"
    state: met
    evidence: "test:crates/wcore-cli/src/doctor/mod.rs::an_unparsable_base_url_still_prints_the_caveat"
    owner: core
    note: "MET. A test at each site carries the backslash spelling AND its wrong-refusal control, and each was shown RED against the hand cut before the fix landed (see c1/c2/c5 for the verbatim panics). doctor: the_base_url_caveat_is_printed_for_every_host_that_is_not_the_vendor holds both directions in one test, plus an_unparsable_base_url_still_prints_the_caveat. browser: a_smuggled_authority_in_a_pattern_names_the_host_it_dials with ordinary_origin_patterns_still_match_what_they_always_matched as the named control (an ordinary *.github.com pattern still matches api.github.com). redact: the control is inside the same test."
  - id: c5
    text: "scrub_detail over a smuggled authority does not name the surviving host as the allowlisted one: scrub_detail(r\"https://evil.example\\@github.com/x\") differs from scrub_detail(\"https://user:pw@github.com/x\"), with a test carrying both alongside a control that an ordinary credential-bearing URL is still redacted"
    state: met
    evidence: "test:crates/wcore-config/src/portability/redact.rs::a_smuggled_authority_does_not_redact_into_the_allowlisted_host"
    owner: core
    note: "MET AS WRITTEN. scrub_detail(r'https://evil.example\@github.com/x') != scrub_detail('https://user:pw@github.com/x'): the first is returned UNCHANGED because url::Url says it carries no userinfo, so evil.example survives in the rendered detail. RED ARM, verbatim: `assertion `left != right` failed ... left: \'https://<redacted>@github.com/x\' right: \'https://<redacted>@github.com/x\'`. Controls in the same test: an ordinary credential-bearing URL is still redacted to https://<redacted>@github.com/x, a credential-free URL is untouched, and a URL embedded in a longer detail string is still found."

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
## Independently re-verified 2026-08-31 by lane f13-authority at 488fbbae9

Every red arm below was RE-RUN by a second pass rather than taken on the first
pass's word, and each mutation was confirmed to COMPILE (`cargo check -p
<crate> --tests`, RC=0) before its arm was believed.

* **c1** — the hand cut restored inside `base_url_caveat` (both sides cut by
  `split(['/','?','#'])` then `rsplit_once('@')`):

      panicked at crates/wcore-cli/src/doctor/mod.rs:1886:9:
      a base_url that dials evil.example must still print the caveat

* **c2** — the hand cut restored inside `strip_pattern_decorations`:

      panicked at crates/wcore-browser/src/policy.rs:1784:9:
      a pattern that PARSES as evil.example must not match github.com

* **c5** — the hand cut restored AHEAD of the parser in `strip_url_userinfo`:

      panicked at crates/wcore-config/src/portability/redact.rs:336:9:
      assertion `left != right` failed: the smuggled URL and a
      credential-bearing github.com URL must not render identically -- that is
      the whole defect
        left: "https://<redacted>@github.com/x"
       right: "https://<redacted>@github.com/x"

  Under that SAME mutation `scrub_detail_removes_embedded_credentials_but_keeps_the_shape`
  stayed GREEN, so the arm is specific to the smuggled spelling and is not a
  general break of the redaction.

**c3 graded against the INVERTED question the body asks for**, not against the
idiom list that missed Site C. The decidable total set used is every production
`.rs` line under `crates/` carrying the scheme-separator literal `"://"` -- a
superset of every cutting idiom, and the one that DOES catch `find("://")`.
24 hits, all read:

* IN CLASS AND FIXED -- `doctor/mod.rs`, `wcore-browser/src/policy.rs`,
  `portability/redact.rs`: this ticket's three sites, all now answering through
  `wcore_types::url_authority` or `url::Url`.
* ALREADY CORRECT -- `wcore-tools/src/website_policy.rs:155` (`Url::parse` +
  `host_str()`).
* OUT OF CLASS, each for a stated reason -- `sources_block.rs:91`,
  `tool_formatters/web_fetch.rs:103`, `tool_formatters/web.rs:212` render the
  whole authority and strip no userinfo (the body's own disposition);
  `events.rs:2022` is `split_endpoint`, redaction-only and forbidden by its doc
  comment from answering which host is reached; `discord/gateway.rs:713`
  (`ensure_path`) inserts a missing `/` into the process's own gateway URL and
  returns no host; `compat.rs:1403` (`split_authority`) joins a path onto the
  operator's own `base_url` and makes no name comparison;
  `video_analyze.rs:266` reads the SCHEME only, to require https, and a `\`
  cannot forge a scheme; `monitor.rs:285` builds a log-noise fingerprint for
  dedup, never a rendered host; `egress_proxy.rs:218`, `bash/policy.rs:314` and
  `:540`, `marketplace.rs:515`, `retry.rs:728`, `website_policy.rs:411` and
  `skills/mcp.rs:217` are `contains("://")` shape tests that return no host at
  all.
* TEST-ONLY -- `limits.rs:1317` and `:1427` DO cut a host and compare it against
  `VENDOR_API_DOMAINS`, which is the class shape; both sit inside `#[cfg(test)]`
  and their input is the bundled `providers.toml` this repo ships, not model
  output or user config.

**Stated bound, so this is not over-read.** The sweep above is a MEASUREMENT,
not a standing gate. Nothing fails when a FOURTH hand cut is added, so a future
one is caught by the next sweep rather than on arrival. c3 as written is a
property of the two named sites and it holds; the class-closure gate that would
make it permanent is filed as FerroxLabs/wayland#1276 rather than claimed here.
