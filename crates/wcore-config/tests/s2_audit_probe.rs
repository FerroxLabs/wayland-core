//! S2 independent instrument probe for wayland#1252 c5 (SITE C).
//!
//! **THIS TEST IS EXPECTED RED on lane/f13-s2-authority-parsers.** It is the
//! refutation of c5's control clause — "with a test carrying both alongside a
//! control that an ordinary credential-bearing URL is still redacted" — in the
//! same red-arm form the lane used for its own criteria.
//!
//! The c5 fix rewrote `strip_url_userinfo` to split the detail into
//! WHITESPACE-DELIMITED tokens and hand each token to `Url::parse`. Any token
//! that does not parse as a URL on its own is returned byte for byte. A URL
//! attached to a flag with `=`, wrapped in quotes, brackets or JSON — every
//! shape short of a bare token — therefore stops being redacted at all, and
//! the userinfo password survives into `DiscoveredItem::details`, the very
//! channel `scrub_detail` exists to close.
//!
//! Measured 2026-08-31 on hetzner, both arms in one run: with the lane's
//! rewrite 8 of these 12 shapes leak; with a faithful restoration of the
//! pre-fix hand cut, 0 of 12 leak and this test PASSES. The regression is the
//! fix's, not a pre-existing gap.
use wcore_config::portability::scrub_detail;

/// Faithful transcription of the PRE-FIX `strip_url_userinfo` (base ca15a48bf).
fn old_strip_url_userinfo(v: &str) -> String {
    let Some(scheme_end) = v.find("://") else {
        return v.to_string();
    };
    let rest_start = scheme_end + 3;
    let rest = &v[rest_start..];
    let authority_end = rest.find('/').unwrap_or(rest.len());
    let Some(at) = rest[..authority_end].find('@') else {
        return v.to_string();
    };
    format!("{}<redacted>@{}", &v[..rest_start], &rest[at + 1..])
}

#[test]
fn s2_regression_c5_leaks_credentials_the_old_cut_redacted() {
    let cases = [
        "https://u:pw@h.example/x",
        "srv --url https://u:pw@h.example/x",
        "srv --url=https://u:pw@h.example/x",
        "--endpoint=https://u:pw@h.example/x",
        "url=https://u:pw@h.example/x",
        "\"https://u:pw@h.example/x\"",
        "(https://u:pw@h.example/x)",
        "<https://u:pw@h.example/x>",
        "postgres://u:pw@db.example:5432/app",
        "DATABASE_URL=postgres://u:pw@db.example:5432/app",
        "{\"url\":\"https://u:pw@h.example/x\"}",
        "connect https://u:pw@h.example/x; retry",
    ];
    let leaks = |s: &str| s.contains("pw@") || s.contains("u:pw");
    let mut regressed = Vec::new();
    for c in cases {
        let new = scrub_detail(c);
        let old = old_strip_url_userinfo(c);
        println!("INPUT: {c}\n  OLD: {old}\n  NEW: {new}");
        if leaks(&new) && !leaks(&old) {
            regressed.push(c);
        }
    }
    // Known-positive control: the leak detector fires on an unredacted string,
    // so an empty `regressed` would be a real zero rather than a dead query.
    assert!(
        leaks("https://u:pw@h.example/x"),
        "control failed: the leak detector does not detect a leak"
    );
    assert!(
        regressed.is_empty(),
        "scrub_detail now leaks credential material that the hand cut it \
         replaced redacted, in {} of {} shapes: {regressed:#?}",
        regressed.len(),
        cases.len()
    );
}
