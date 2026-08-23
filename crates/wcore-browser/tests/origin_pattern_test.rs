//! RED ARM for gh#1075 — an `allowed_origins` / `denied_origins` entry written
//! in URL form can never match, and nothing says so.
//!
//! `origin_matches` (`policy.rs:735`) compares the request's **host**
//! (`x.example`) against the pattern **verbatim**, so `https://x.example`
//! fails `host == pattern` for every request, forever. The field is named
//! `allowed_ORIGINS`, and an origin in the web-platform sense IS
//! scheme + host + port — so the spelling that silently fails is the one the
//! field name asks for.
//!
//! ## Why the fix belongs in `origin_matches`, not in the config loader
//!
//! Counting the construction sites rather than the one the ticket happens to
//! name: `BrowserPolicy` is built from operator config through
//! `wcore-browser/src/adapter.rs:76` (`BrowserPolicy::new`), from the plugin
//! mirror through `wcore-plugin-api/src/browser_spec.rs:65` →
//! `wcore-agent/src/plugins/adapters/browser_adapter.rs:78`, and directly by
//! serde (`allowed_origins` / `denied_origins` are `pub` and the struct is
//! `Deserialize`). Normalising in `BrowserPolicy::new` alone leaves the serde
//! path broken; normalising in `wcore-config` alone leaves both of the others
//! broken. `origin_matches` is the single point every path funnels through —
//! hence `policy_from_serde_normalizes_too` below, which exists specifically
//! to fail against a fix placed in `new()`.
//!
//! ## The deny list is the half that matters
//!
//! A never-matching ALLOW entry fails closed and merely blocks the operator.
//! A never-matching DENY entry fails OPEN: the operator believes a host is
//! blocked and it is not. Both halves are graded below.

use wcore_browser::policy::{BrowserPolicy, PolicyAction, PolicyOutcome};

fn allowed(p: &BrowserPolicy, url: &str) -> bool {
    matches!(p.evaluate(url), PolicyOutcome::Allow)
}

// ---------------------------------------------------------------------------
// Allow list.
// ---------------------------------------------------------------------------

/// RED. The URL spelling of an allow entry must admit its host.
#[test]
fn url_form_allowed_origin_admits_its_host() {
    let p = BrowserPolicy::new(PolicyAction::Deny, vec!["https://x.example".into()], vec![]);
    assert!(
        allowed(&p, "https://x.example/path"),
        "`allowed_origins = [\"https://x.example\"]` never matches any request, \
         and the loader accepts it without a word: {:?}",
        p.evaluate("https://x.example/path")
    );
}

/// NEGATIVE CONTROL — pairs with the test above. Normalising a URL-form entry
/// must not widen it: an unrelated host stays denied. Without this control the
/// test above could be satisfied by making every pattern match everything.
#[test]
fn url_form_allowed_origin_does_not_widen_to_other_hosts() {
    let p = BrowserPolicy::new(PolicyAction::Deny, vec!["https://x.example".into()], vec![]);
    assert!(
        !allowed(&p, "https://y.example/"),
        "an entry for x.example must not admit y.example"
    );
    assert!(
        !allowed(&p, "https://x.example.evil.test/"),
        "suffix confusion: x.example must not admit x.example.evil.test"
    );
}

/// RED. A scheme is not the only decoration an operator writes; a port and a
/// path are just as natural in a field called `allowed_origins`. Per gh#1075's
/// suggested fix, normalising means stripping the scheme, any port and any
/// path down to the host the matcher actually compares.
#[test]
fn url_form_allowed_origin_tolerates_port_path_and_trailing_slash() {
    for pattern in [
        "https://x.example:8443",
        "http://x.example/some/path",
        "https://x.example/",
    ] {
        let p = BrowserPolicy::new(PolicyAction::Deny, vec![pattern.into()], vec![]);
        assert!(
            allowed(&p, "https://x.example/"),
            "pattern {pattern:?} never matches its own host: {:?}",
            p.evaluate("https://x.example/")
        );
    }
}

/// RED. The wildcard form has to survive normalisation — an operator writing
/// `https://*.wild.example` must get the same matcher the bare
/// `*.wild.example` gives.
#[test]
fn url_form_wildcard_allowed_origin_still_globs() {
    let p = BrowserPolicy::new(
        PolicyAction::Deny,
        vec!["https://*.wild.example".into()],
        vec![],
    );
    assert!(
        allowed(&p, "https://a.wild.example/"),
        "URL-form wildcard lost its glob: {:?}",
        p.evaluate("https://a.wild.example/")
    );
    assert!(
        allowed(&p, "https://wild.example/"),
        "`*.` matches the apex too (cf. policy_test::explicit_allow_list_permits_origin)"
    );
    // NEGATIVE CONTROL, same test: the glob must not run off its right edge.
    assert!(
        !allowed(&p, "https://wild.example.evil.test/"),
        "wildcard must not admit wild.example.evil.test"
    );
}

/// RED. Guards the fix against being installed only in `BrowserPolicy::new`.
/// `allowed_origins` is a public field on a `Deserialize` struct, so config
/// and plugin-mirror data can reach the matcher without ever passing through
/// the constructor.
#[test]
fn policy_from_serde_normalizes_too() {
    let p: BrowserPolicy = serde_json::from_value(serde_json::json!({
        "default_action": "deny",
        "allowed_origins": ["https://x.example"],
        "denied_origins": []
    }))
    .expect("BrowserPolicy must deserialize from its own operator shape");

    assert!(
        allowed(&p, "https://x.example/"),
        "a serde-constructed policy bypasses BrowserPolicy::new, so a fix that \
         normalises in the constructor leaves this path broken: {:?}",
        p.evaluate("https://x.example/")
    );
    // NEGATIVE CONTROL, same test.
    assert!(
        !allowed(&p, "https://y.example/"),
        "serde path must not widen either"
    );
}

// ---------------------------------------------------------------------------
// Deny list — the fail-OPEN half.
// ---------------------------------------------------------------------------

/// RED. A URL-form deny entry that never matches is a security hole, not an
/// inconvenience: the operator believes the host is blocked and it is not.
#[test]
fn url_form_denied_origin_still_denies() {
    let p = BrowserPolicy::new(
        PolicyAction::Allow,
        vec![],
        vec!["https://evil.example".into()],
    );
    assert!(
        !allowed(&p, "http://evil.example/"),
        "`denied_origins = [\"https://evil.example\"]` fails OPEN — the host is \
         reached while the operator believes it is blocked"
    );
}

/// NEGATIVE CONTROL for the deny half — the bare spelling already works and
/// must keep working, and the deny entry must not swallow unrelated hosts.
#[test]
fn bare_denied_origin_denies_and_stays_narrow() {
    let p = BrowserPolicy::new(PolicyAction::Allow, vec![], vec!["evil.example".into()]);
    assert!(!allowed(&p, "http://evil.example/"), "bare deny must deny");
    assert!(
        allowed(&p, "http://fine.example/"),
        "deny entry must not refuse unrelated hosts"
    );
}

/// NEGATIVE CONTROL for the whole file — the bare host spelling, which is what
/// works today, must be untouched by normalisation. If this ever goes red the
/// fix has regressed the only spelling that currently works.
#[test]
fn bare_host_allowed_origin_is_unchanged() {
    let p = BrowserPolicy::new(PolicyAction::Deny, vec!["x.example".into()], vec![]);
    assert!(allowed(&p, "https://x.example/"));
    assert!(!allowed(&p, "https://y.example/"));
}

/// NEGATIVE CONTROL — normalisation must not resurrect a pattern whose scheme
/// the gate refuses outright. `javascript:` is denied at the scheme allow-list
/// (`policy.rs:301`) before any origin matching happens, and a pattern written
/// with such a scheme must never become a working allow entry.
#[test]
fn a_non_http_scheme_pattern_does_not_become_a_working_allow_entry() {
    let p = BrowserPolicy::new(
        PolicyAction::Deny,
        vec!["javascript://x.example".into()],
        vec![],
    );
    assert!(
        !allowed(&p, "javascript:alert(1)"),
        "the scheme allow-list must still fire first"
    );
}

// ===========================================================================
// gh#1075, second pass: every OTHER transformation `origin_matches` was blind
// to.
//
// The request side of the comparison is `Url::host_str()`, which has already
// been through the WHATWG host parser. So the generating rule for this whole
// bug class is: *any normalisation that parser performs and the pattern side
// does not is a spelling that can never match.* Enumerated and measured on
// v0.13.5 (`addb4f48`), with the bare-lowercase spelling passing as a control
// in the same run:
//
//   | spelling written by the operator | v0.13.5    |
//   |---------------------------------|------------|
//   | `x.example`            (control) | matched    |
//   | `xn--bcher-kva.example`(control) | matched    |
//   | `x.example:8443` / `:443`        | matched    |
//   | `[2001:db8::1]`        (control) | matched    |
//   | `X.Example` / `https://X.Example`| NEVER      |
//   | `*.Wild.Example`                 | NEVER      |
//   | `x.example.`  (DNS root dot)     | NEVER      |
//   | request `https://x.example./`    | NEVER      |
//   | `bücher.example` (IDN, unicode)  | NEVER      |
//   | `https://user@x.example`         | NEVER      |
//   | `[2001:DB8::1]` / expanded form  | NEVER      |
//   | ` x.example` (stray whitespace)  | NEVER      |
//   | `*` alone                        | NEVER      |
//
// Every "NEVER" row is graded below on BOTH sides, because the two halves are
// not the same defect: an allow entry that never fires locks the operator
// out, a deny entry that never fires is a hole they cannot see.
//
// One of these is worse than a config typo. `Url::host_str()` returns the
// trailing root dot verbatim, and the hard-coded loopback block compares
// `host_lc == "localhost"` — so `http://localhost./` was measured reaching
// `PolicyOutcome::Allow` on v0.13.5 with no rules configured at all. That is
// graded in `root_dot_does_not_walk_past_the_hard_coded_loopback_block`.
// ===========================================================================

fn denied(p: &BrowserPolicy, url: &str) -> bool {
    !allowed(p, url)
}

/// Allow-side policy with one entry; `default_action = Deny`, so `Allow`
/// means the entry fired.
fn with_allow(pattern: &str) -> BrowserPolicy {
    BrowserPolicy::new(PolicyAction::Deny, vec![pattern.into()], vec![])
}

/// Deny-side policy with one entry; `default_action = Allow`, so `Allow`
/// means the entry did NOT fire — i.e. it failed open.
fn with_deny(pattern: &str) -> BrowserPolicy {
    BrowserPolicy::new(PolicyAction::Allow, vec![], vec![pattern.into()])
}

// ---------------------------------------------------------------------------
// 1. ASCII case. The row gh#1075 was filed on.
// ---------------------------------------------------------------------------

#[test]
fn mixed_case_pattern_admits_its_host() {
    for pattern in ["X.Example", "https://X.Example", "HTTPS://X.EXAMPLE/"] {
        assert!(
            allowed(&with_allow(pattern), "https://x.example/"),
            "pattern {pattern:?} never matches its own host: {:?}",
            with_allow(pattern).evaluate("https://x.example/")
        );
    }
}

#[test]
fn mixed_case_deny_pattern_still_denies() {
    for pattern in ["Evil.Example", "https://Evil.Example"] {
        assert!(
            denied(&with_deny(pattern), "http://evil.example/"),
            "`denied_origins = [{pattern:?}]` fails OPEN: the host is reached \
             while the operator believes it is blocked"
        );
    }
}

#[test]
fn mixed_case_request_host_is_matched_too() {
    // The other direction: the operator wrote the pattern in lower case and
    // the URL arrives shouting. `Url` folds it, so this already held on
    // v0.13.5 — pinned so a hand-rolled comparison cannot regress it.
    assert!(allowed(&with_allow("x.example"), "https://X.EXAMPLE/"));
    assert!(denied(&with_deny("evil.example"), "http://EVIL.EXAMPLE/"));
}

#[test]
fn case_folding_does_not_widen_the_pattern() {
    // NEGATIVE CONTROL for every case-folding test above: folding must not be
    // satisfied by making patterns match more than their own host.
    let p = with_allow("X.Example");
    assert!(
        !allowed(&p, "https://y.example/"),
        "must not admit y.example"
    );
    assert!(
        !allowed(&p, "https://x.example.evil.test/"),
        "suffix confusion: must not admit x.example.evil.test"
    );
    assert!(
        allowed(&with_deny("Evil.Example"), "http://fine.example/"),
        "a deny entry must not start refusing unrelated hosts"
    );
}

// ---------------------------------------------------------------------------
// 2. Case inside the `*.` wildcard suffix.
// ---------------------------------------------------------------------------

#[test]
fn mixed_case_wildcard_still_globs() {
    let p = with_allow("*.Wild.Example");
    assert!(
        allowed(&p, "https://a.wild.example/"),
        "URL-form wildcard lost its glob to case: {:?}",
        p.evaluate("https://a.wild.example/")
    );
    assert!(
        allowed(&p, "https://wild.example/"),
        "`*.` matches the apex"
    );
    // NEGATIVE CONTROL, same test: the glob must not run off its right edge.
    assert!(
        !allowed(&p, "https://wild.example.evil.test/"),
        "wildcard must not admit wild.example.evil.test"
    );
}

#[test]
fn mixed_case_wildcard_deny_still_denies() {
    let p = with_deny("*.Evil.Example");
    assert!(
        denied(&p, "http://a.evil.example/"),
        "wildcard deny failed OPEN"
    );
    assert!(
        denied(&p, "http://evil.example/"),
        "apex must be denied too"
    );
    // NEGATIVE CONTROL, same test.
    assert!(
        allowed(&p, "http://evil.example.fine.test/"),
        "wildcard deny must not run off its right edge"
    );
}

// ---------------------------------------------------------------------------
// 3+4. The DNS root dot — on either side of the comparison.
// ---------------------------------------------------------------------------

#[test]
fn root_dot_in_the_pattern_still_matches() {
    assert!(allowed(&with_allow("x.example."), "https://x.example/"));
    assert!(allowed(
        &with_allow("*.wild.example."),
        "https://a.wild.example/"
    ));
    assert!(denied(&with_deny("evil.example."), "http://evil.example/"));
}

#[test]
fn root_dot_in_the_request_still_matches() {
    // The attacker-facing direction: the operator's rule is written correctly
    // and the REQUEST carries the dot. `evil.example.` and `evil.example`
    // resolve identically, so a deny rule that only catches one spelling is
    // not a deny rule.
    assert!(
        denied(&with_deny("evil.example"), "http://evil.example./"),
        "`denied_origins = [\"evil.example\"]` fails OPEN against the FQDN \
         spelling `http://evil.example./`, which resolves to the same host"
    );
    assert!(
        denied(&with_deny("*.evil.example"), "http://a.evil.example./"),
        "same evasion through the wildcard form"
    );
    assert!(
        allowed(&with_allow("x.example"), "https://x.example./"),
        "the allow side must treat the two spellings as one host as well"
    );
}

#[test]
fn root_dot_handling_does_not_widen() {
    // NEGATIVE CONTROL. Trimming the root label must not trim a real one.
    assert!(
        !allowed(&with_allow("x.example"), "https://x.exampl/"),
        "must not admit a host that is a prefix of the pattern"
    );
    assert!(
        allowed(&with_deny("evil.example"), "http://evil.example.org/"),
        "must not start refusing evil.example.org"
    );
}

/// The severe half of the root-dot finding, and the reason it is fixed in
/// `evaluate` rather than only in `origin_matches`.
///
/// The hard-coded loopback block compares `host_lc == "localhost"`, and
/// `Url::host_str()` hands it `localhost.` verbatim. Measured on v0.13.5:
/// `http://localhost./` and `http://foo.localhost./` returned
/// `PolicyOutcome::Allow` from a policy with NO rules configured — a
/// hard-block bypass, not a config typo. IP literals were never affected
/// (the WHATWG IPv4 parser eats the dot), which is the control below.
#[test]
fn root_dot_does_not_walk_past_the_hard_coded_loopback_block() {
    let p = BrowserPolicy::new(PolicyAction::Allow, vec![], vec![]);
    for url in [
        "http://localhost./",
        "http://LOCALHOST./",
        "http://foo.localhost./",
    ] {
        assert!(
            denied(&p, url),
            "{url} walks past the hard-coded loopback block: {:?}",
            p.evaluate(url)
        );
    }
    // KNOWN-POSITIVE CONTROLS, same run: the dotless spellings and the IP
    // literals were blocked on v0.13.5 too, so this test can distinguish
    // "the fix landed" from "everything is denied now".
    for url in [
        "http://localhost/",
        "http://127.0.0.1./",
        "http://192.168.0.1./",
    ] {
        assert!(denied(&p, url), "{url} must stay blocked");
    }
    assert!(
        allowed(&p, "https://x.example/"),
        "a public host must still pass a rule-free Allow policy — without \
         this the test above passes for the wrong reason"
    );
}

// ---------------------------------------------------------------------------
// 5. IDN. `Url` reduces the request host to punycode; the pattern was not.
// ---------------------------------------------------------------------------

#[test]
fn unicode_idn_pattern_matches_its_punycode_host() {
    // `bücher.example` → `xn--bcher-kva.example`. An operator types the name
    // the way it is written, and the matcher only ever sees the A-label.
    assert!(
        allowed(&with_allow("bücher.example"), "https://bücher.example/"),
        "a unicode IDN allow entry never matches its own host"
    );
    assert!(
        denied(&with_deny("bücher.example"), "http://bücher.example/"),
        "a unicode IDN deny entry fails OPEN"
    );
    assert!(
        allowed(
            &with_allow("https://*.bücher.example"),
            "https://a.bücher.example/"
        ),
        "…including through a URL-form wildcard"
    );
}

#[test]
fn punycode_pattern_keeps_working() {
    // KNOWN-POSITIVE CONTROL — the A-label spelling already worked on
    // v0.13.5 and must not regress.
    assert!(allowed(
        &with_allow("xn--bcher-kva.example"),
        "https://bücher.example/"
    ));
    assert!(denied(
        &with_deny("xn--bcher-kva.example"),
        "http://bücher.example/"
    ));
}

#[test]
fn idn_normalisation_does_not_widen() {
    // NEGATIVE CONTROL: the ASCII lookalike is a different host.
    assert!(!allowed(
        &with_allow("bücher.example"),
        "https://bucher.example/"
    ));
    assert!(allowed(
        &with_deny("bücher.example"),
        "http://bucher.example/"
    ));
}

// ---------------------------------------------------------------------------
// 6. Userinfo. Natural to paste out of a browser bar.
// ---------------------------------------------------------------------------

#[test]
fn userinfo_in_the_pattern_is_discarded() {
    for pattern in ["https://user@x.example", "https://u:p@x.example"] {
        assert!(
            allowed(&with_allow(pattern), "https://x.example/"),
            "pattern {pattern:?} never matches its own host"
        );
    }
    assert!(denied(
        &with_deny("https://user@evil.example"),
        "http://evil.example/"
    ));
}

#[test]
fn userinfo_stripping_takes_the_last_at_sign() {
    // NEGATIVE CONTROL against the classic confusion: the host is what
    // follows the LAST `@`, exactly as WHATWG parses it. A pattern whose
    // userinfo itself contains `@` must not resolve to the wrong host.
    let p = with_allow("https://user@evil.example@x.example");
    assert!(
        allowed(&p, "https://x.example/"),
        "host is after the last @"
    );
    assert!(
        !allowed(&p, "https://evil.example/"),
        "the userinfo field must never become the matched host"
    );
}

// ---------------------------------------------------------------------------
// 7. Ports written out. These already worked — pinned as controls.
// ---------------------------------------------------------------------------

#[test]
fn explicit_ports_including_the_default_are_dropped() {
    // KNOWN-POSITIVE CONTROLS from the v0.13.5 measurement. Origin matching
    // here is host-granular by design, so a port never narrows an entry.
    for pattern in [
        "https://x.example:443",
        "http://x.example:80",
        "x.example:8443",
    ] {
        assert!(
            allowed(&with_allow(pattern), "https://x.example/"),
            "pattern {pattern:?} stopped matching its host"
        );
    }
    assert!(denied(
        &with_deny("https://evil.example:443"),
        "http://evil.example:8443/"
    ));
}

// ---------------------------------------------------------------------------
// 8. IPv6 literals. `Url` compresses and case-folds; the operator does not.
// ---------------------------------------------------------------------------

#[test]
fn ipv6_pattern_spellings_all_reach_the_same_host() {
    for pattern in [
        "[2001:db8::1]",              // control: already matched on v0.13.5
        "[2001:DB8::1]",              // case
        "[2001:db8:0:0:0:0:0:1]",     // uncompressed
        "2001:db8::1",                // no brackets, as an operator writes it
        "https://[2001:db8::1]:8443", // full URL form
    ] {
        assert!(
            allowed(&with_allow(pattern), "https://[2001:db8::1]/"),
            "IPv6 pattern {pattern:?} never matches its own host: {:?}",
            with_allow(pattern).evaluate("https://[2001:db8::1]/")
        );
    }
    assert!(denied(&with_deny("[2001:DB8::1]"), "http://[2001:db8::1]/"));
}

#[test]
fn ipv6_normalisation_does_not_widen() {
    // NEGATIVE CONTROL.
    assert!(!allowed(
        &with_allow("[2001:db8::1]"),
        "https://[2001:db8::2]/"
    ));
}

// ---------------------------------------------------------------------------
// 9. Stray whitespace — a TOML list split across lines.
// ---------------------------------------------------------------------------

#[test]
fn surrounding_whitespace_is_trimmed() {
    assert!(allowed(&with_allow(" x.example "), "https://x.example/"));
    assert!(denied(
        &with_deny("\tevil.example\n"),
        "http://evil.example/"
    ));
}

// ---------------------------------------------------------------------------
// 10. The residual: what still cannot fire, pinned deliberately.
// ---------------------------------------------------------------------------

/// `*` on its own is NOT a match-all here, and this pins that on purpose.
///
/// It is the one entry left in the enumeration that can never fire. Giving it
/// match-all semantics would be a new policy feature, and on the allow side it
/// would convert today's fail-CLOSED miss into an allow-everything rule — the
/// wrong direction for a security control to be changed in by a bug fix. It is
/// recorded here so the next reader finds it stated rather than discovers it.
#[test]
fn a_bare_star_matches_nothing_and_that_is_deliberate() {
    assert!(
        !allowed(&with_allow("*"), "https://x.example/"),
        "`*` is not a wildcard in this matcher"
    );
    assert!(
        allowed(&with_deny("*"), "http://evil.example/"),
        "`denied_origins = [\"*\"]` does not deny everything — the residual \
         never-match case, pinned"
    );
    // KNOWN-POSITIVE CONTROL, same test: `*.` IS a wildcard, so this test
    // fails for the right reason if the matcher ever stops globbing at all.
    assert!(allowed(&with_allow("*.example"), "https://x.example/"));
}

/// A pattern that is not a host in any spelling matches nothing, on both
/// sides. Together with the test above this is the complete residual set.
#[test]
fn non_host_patterns_match_nothing() {
    for pattern in ["", "   ", "*.", "data:text/html,<script>"] {
        assert!(
            !allowed(&with_allow(pattern), "https://x.example/"),
            "pattern {pattern:?} must not admit anything"
        );
        assert!(
            allowed(&with_deny(pattern), "http://x.example/"),
            "pattern {pattern:?} is not a host, so it cannot refuse one"
        );
    }
}

/// NEGATIVE CONTROL for the whole normalisation, extending the existing
/// `a_non_http_scheme_pattern_does_not_become_a_working_allow_entry`: an entry
/// written with a scheme the gate refuses outright must not become a working
/// allow entry for the host inside it, whatever request the URL is compared
/// against.
#[test]
fn a_refused_scheme_pattern_never_admits_its_host() {
    for pattern in [
        "javascript://x.example",
        "file://x.example",
        "ftp://x.example",
        "blob://x.example",
    ] {
        let p = with_allow(pattern);
        assert!(
            !allowed(&p, "https://x.example/"),
            "pattern {pattern:?} was resurrected into a working allow entry \
             for https://x.example/"
        );
    }
}
