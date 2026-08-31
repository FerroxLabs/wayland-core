//! S2 independent instrument probe for wayland#1252 c2/c4 (SITE B).
//!
//! WRONG-REFUSAL CONTROL the lane did not write: every pattern an operator
//! plausibly writes must still admit the host it names, through the PRODUCTION
//! `BrowserPolicy::check_url` gate rather than through the private
//! `origin_matches` helper the criterion names.
//!
//! RESULT: no wrong refusal. 21 ordinary spellings all still admit their own
//! host after the rewrite.
//!
//! The eight LOOPBACK spellings (`localhost`, `127.0.0.1`, `[::1]`, `::1`, and
//! their `:port` forms) are deliberately NOT in the corpus below: they are
//! refused, but they are refused by `check_url`'s pre-existing loopback
//! hard-block, which sits in front of the allow-list and is untouched by this
//! lane. Measured both ways in the same session on 2026-08-31 — with the
//! rewrite in place, and with a faithful restoration of the pre-fix
//! `strip_pattern_decorations` hand cut — and the refused set was BYTE
//! IDENTICAL. Not a regression; not this lane's.
use wcore_browser::policy::{BrowserPolicy, PolicyAction};

fn admits(pattern: &str, url: &str) -> bool {
    BrowserPolicy::new(PolicyAction::Deny, vec![pattern.to_string()], Vec::new())
        .check_url(url)
        .is_ok()
}

#[test]
fn s2_probe_ordinary_allow_patterns_still_admit_their_own_host() {
    let cases: &[(&str, &str)] = &[
        ("github.com", "https://github.com/x"),
        ("*.github.com", "https://api.github.com/x"),
        ("*.github.com", "https://github.com/x"),
        ("https://github.com", "https://github.com/x"),
        ("https://github.com/", "https://github.com/x"),
        ("http://github.com", "https://github.com/x"),
        ("github.com/", "https://github.com/x"),
        ("https://github.com/some/path", "https://github.com/x"),
        ("github.com:8443", "https://github.com:8443/x"),
        ("https://github.com:8443/x", "https://github.com/x"),
        ("EXAMPLE.COM", "https://example.com/x"),
        ("*.EXAMPLE.COM", "https://api.example.com/x"),
        ("example.com.", "https://example.com/x"),
        ("*.example.co.uk", "https://a.example.co.uk/x"),
        ("https://user:pw@example.com", "https://example.com/x"),
        ("internal.corp", "https://internal.corp/x"),
        ("*.internal.corp", "https://svc.internal.corp/x"),
        ("my-host_1.example", "https://my-host_1.example/x"),
        ("  github.com  ", "https://github.com/x"),
        ("https://example.com?a=b", "https://example.com/x"),
        ("https://example.com#f", "https://example.com/x"),
    ];
    let mut refused = Vec::new();
    for (pattern, url) in cases {
        if !admits(pattern, url) {
            refused.push((*pattern, *url));
        }
    }
    // Known-positive control: the query itself discriminates, so an empty
    // `refused` is a real zero and not a silently inert probe.
    assert!(
        !admits("other.example", "https://github.com/x"),
        "control failed: an unrelated pattern admitted github.com, so this \
         probe cannot distinguish anything"
    );
    assert!(
        refused.is_empty(),
        "ordinary operator patterns stopped admitting their own host: {refused:#?}"
    );
}
