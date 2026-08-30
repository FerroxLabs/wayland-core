//! The one authority parser.
//!
//! Every security- or display-relevant answer to "which host does this URL
//! string actually reach?" comes from here, and from the WHATWG URL parser
//! underneath it — the same parser `reqwest` builds its requests with. Nothing
//! in this repo may cut an authority out of a URL by hand.
//!
//! ## Why this module exists rather than a fourth hand cut
//!
//! FerroxLabs/wayland#1211 and #1243 are one bug appearing four times. A hand
//! cut takes everything up to the first `/` (sometimes `?`, sometimes `#`) and
//! then the last `@`-separated part, and each of those cuts missed a different
//! separator:
//!
//! - `https://api.openai.com?x=@127.0.0.1` — the `@` lives in the QUERY, and a
//!   cut that stops only at `/` reads the whole query as authority. That was
//!   #1211: the startup credential gate classified a public host as loopback
//!   and waived the API key.
//! - `https://evil.example\@github.com/x` — for a SPECIAL scheme (http, https,
//!   ws, wss, ftp, file) the WHATWG parser maps `\` to a path separator, so
//!   this is a request to `evil.example` with the path `/@github.com/x`. A cut
//!   that stops at `/ ? #` still reads it as `evil.example\@github.com` and
//!   takes `github.com`. That is #1243, at both of its sites.
//!
//! Adding `\` to a hand cut would close today's spelling and leave tomorrow's:
//! the WHATWG authority state machine also strips C0 controls and ASCII
//! whitespace (including tab, LF and CR ANYWHERE in the input), applies IDNA
//! to a domain, canonicalises IPv4 in four different radices, and rejects a
//! host with a forbidden code point outright. No hand cut of ours is going to
//! track that, so none of ours gets to try.
//!
//! ## What this module does NOT decide
//!
//! Only "which host". Whether that host is trusted, local, self-hosted, or
//! allowlisted is a POLICY question, and each caller keeps its own answer —
//! the WebFetch allowlist, `provider_info.local`'s address ranges, and the
//! keyless self-hosted ranges are deliberately three different policies over
//! this one host. Unifying the policies is not the fix and would be a bug of
//! its own; unifying the PARSE is.

pub use url::Host;
use url::Url;

/// The host `raw` is actually dialed against, as the URL parser sees it, or
/// `None` when the string carries no host at all.
///
/// `None` covers a scheme-less string, a relative reference, a `data:`/`file:`
/// URL with no authority, and anything unparsable. Every caller must treat
/// `None` as "I do not know", and must fail in whichever direction is safe for
/// it — never as "this host is fine".
///
/// A `Host::Domain` returned here is already lowercased and IDNA-normalised
/// for a special scheme; the ASCII fold at the call sites is belt-and-braces
/// for any other scheme.
#[must_use]
pub fn dialed_host(raw: &str) -> Option<Host<String>> {
    Url::parse(raw.trim()).ok()?.host().map(|host| match host {
        Host::Domain(domain) => Host::Domain(domain.to_ascii_lowercase()),
        Host::Ipv4(addr) => Host::Ipv4(addr),
        Host::Ipv6(addr) => Host::Ipv6(addr),
    })
}

/// [`dialed_host`] rendered for display and for an exact-name comparison:
/// a domain lowercased, an IP literal in its canonical form, an IPv6 address
/// WITHOUT the surrounding brackets.
///
/// Suitable for a prompt title and for an allowlist match. Not suitable for
/// re-assembling a request URL — the brackets an IPv6 authority needs are
/// gone.
#[must_use]
pub fn dialed_host_str(raw: &str) -> Option<String> {
    match dialed_host(raw)? {
        Host::Domain(domain) => Some(domain),
        Host::Ipv4(addr) => Some(addr.to_string()),
        Host::Ipv6(addr) => Some(addr.to_string()),
    }
}

/// The `(scheme, host)` a URL string is actually dialed with, from one parse.
///
/// For a caller whose policy is scheme-sensitive — the WebFetch prompt trusts
/// an allowlisted host only over `https` — so that the scheme and the host can
/// never come from two different readings of the same string.
#[must_use]
pub fn dialed_scheme_host(raw: &str) -> Option<(String, String)> {
    let parsed = Url::parse(raw.trim()).ok()?;
    let scheme = parsed.scheme().to_ascii_lowercase();
    let host = match parsed.host()? {
        Host::Domain(domain) => domain.to_ascii_lowercase(),
        Host::Ipv4(addr) => addr.to_string(),
        Host::Ipv6(addr) => addr.to_string(),
    };
    Some((scheme, host))
}

/// `raw` rebuilt with userinfo, query string and fragment REMOVED, so it can
/// be published in an event or a log. Scheme, host, port and path survive.
///
/// Both removed positions can carry an API key (`https://user:KEY@host/v1`,
/// `https://host/v1?api_key=KEY`), which is why this exists at all. It is
/// built from the parse rather than from a cut so the host it publishes is the
/// host the request went to — publishing `127.0.0.1` for a request that
/// reached `evil.example` is the same defect as calling it local.
///
/// `None` when the string carries no host; the caller decides what a
/// non-URL is worth publishing.
#[must_use]
pub fn publishable_endpoint(raw: &str) -> Option<String> {
    let mut parsed = Url::parse(raw.trim()).ok()?;
    parsed.host()?;
    // set_username / set_password return Err only for a cannot-be-a-base URL,
    // which cannot have reached here: it would have no host.
    let _ = parsed.set_username("");
    let _ = parsed.set_password(None);
    parsed.set_query(None);
    parsed.set_fragment(None);
    Some(parsed.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The two spellings that defeated the hand cuts, side by side with the
    /// plain URL they were disguised as. `\` and `?` are read by the parser as
    /// the start of the path and the query — the authority ends before both.
    #[test]
    fn the_smuggling_spellings_resolve_to_the_host_actually_dialed() {
        for raw in [
            r"https://evil.example\@github.com/x",
            r"https://evil.example\@github.com",
            r"https://evil.example\@127.0.0.1/v1",
            "https://evil.example?z=@github.com",
            "https://evil.example?z=@127.0.0.1",
            "https://evil.example#@github.com",
            // Userinfo spelled honestly is still userinfo, not the host.
            "https://github.com@evil.example/x",
            "https://user:p@ss@evil.example/x",
        ] {
            assert_eq!(
                dialed_host_str(raw).as_deref(),
                Some("evil.example"),
                "{raw}"
            );
        }
    }

    /// The wrong-answer control: the honest spellings of the SAME hosts must
    /// still resolve to themselves, or a predicate built on this would simply
    /// refuse everything and look correct.
    #[test]
    fn ordinary_urls_still_resolve_to_their_own_host() {
        for (raw, expect) in [
            ("https://github.com/x", "github.com"),
            ("https://GitHub.COM/x", "github.com"),
            ("https://api.github.com/repos/o/r", "api.github.com"),
            ("http://127.0.0.1:11434/v1", "127.0.0.1"),
            ("http://localhost:11434", "localhost"),
            ("http://[::1]:11434/v1", "::1"),
            ("https://api.anthropic.com/v1", "api.anthropic.com"),
            ("http://ollama.local:11434", "ollama.local"),
            (
                "https://127.0.0.1.evil.example.com/v1",
                "127.0.0.1.evil.example.com",
            ),
        ] {
            assert_eq!(dialed_host_str(raw).as_deref(), Some(expect), "{raw}");
        }
    }

    /// A host that does not exist is `None`, never an empty string and never a
    /// fragment of the input. Callers key their fail-closed branch on this.
    #[test]
    fn a_string_with_no_host_is_none() {
        for raw in [
            "",
            "   ",
            "github.com",
            "https://",
            "/v1/messages",
            "not a url at all",
            "data:text/plain,hello",
        ] {
            assert_eq!(dialed_host_str(raw), None, "{raw:?}");
        }
    }

    /// Whitespace and C0 controls are stripped by the parser wherever they
    /// appear, so a host cannot be hidden behind an embedded tab or newline —
    /// the class of trick a hand cut would need a separate rule for.
    #[test]
    fn embedded_whitespace_does_not_hide_the_host() {
        assert_eq!(
            dialed_host_str("https://evil.example\t\\@github.com/x").as_deref(),
            Some("evil.example")
        );
        assert_eq!(
            dialed_host_str("https://evil.exa\nmple/x").as_deref(),
            Some("evil.example")
        );
    }

    /// The scheme and the host come from ONE parse, so a scheme-sensitive
    /// policy cannot be handed a host read out of a different interpretation.
    #[test]
    fn the_scheme_and_host_come_from_one_reading() {
        assert_eq!(
            dialed_scheme_host(r"https://evil.example\@github.com/x"),
            Some(("https".into(), "evil.example".into()))
        );
        assert_eq!(
            dialed_scheme_host("HTTPS://GitHub.com/x"),
            Some(("https".into(), "github.com".into()))
        );
        assert_eq!(
            dialed_scheme_host("http://github.com/x"),
            Some(("http".into(), "github.com".into()))
        );
        assert_eq!(dialed_scheme_host("github.com"), None);
        assert_eq!(dialed_scheme_host("https://"), None);
    }

    /// A published endpoint keeps the diagnostic host and path and loses both
    /// credential positions — including a password that itself contains `@`.
    #[test]
    fn a_publishable_endpoint_drops_both_credential_positions() {
        for raw in [
            "https://user:s3cr3t@gateway.example.com/v1",
            "https://gateway.example.com/v1?api_key=s3cr3t",
            "https://gateway.example.com/v1#s3cr3t",
            "https://user:s3cr3t@gateway.example.com/v1?api_key=s3cr3t",
            "https://user:p@ss3cr3t@gateway.example.com/v1",
        ] {
            let out = publishable_endpoint(raw).expect(raw);
            assert!(!out.contains("s3cr3t"), "{raw} -> {out}");
            assert!(out.contains("gateway.example.com/v1"), "{raw} -> {out}");
        }
    }

    /// And it publishes the host the request actually reaches, not the one the
    /// smuggled spelling displays.
    #[test]
    fn a_publishable_endpoint_names_the_host_actually_dialed() {
        let out = publishable_endpoint(r"https://evil.example\@127.0.0.1/v1").expect("parses");
        assert!(out.starts_with("https://evil.example/"), "{out}");
        assert!(!out.starts_with("https://127.0.0.1"), "{out}");
        assert_eq!(publishable_endpoint("127.0.0.1:11434"), None);
    }

    /// The typed form keeps the IP/domain distinction a caller needs to decide
    /// an address range without re-parsing the string it just produced.
    #[test]
    fn the_typed_host_separates_an_ip_literal_from_a_name() {
        assert!(matches!(
            dialed_host("http://127.0.0.1:11434/v1"),
            Some(Host::Ipv4(_))
        ));
        assert!(matches!(
            dialed_host("http://[fe80::1]:11434"),
            Some(Host::Ipv6(_))
        ));
        assert!(matches!(
            dialed_host("https://127.0.0.1.evil.example.com/v1"),
            Some(Host::Domain(_))
        ));
    }
}
