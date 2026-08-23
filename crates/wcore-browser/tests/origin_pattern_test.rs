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
