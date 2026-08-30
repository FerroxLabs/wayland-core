//! Is this endpoint one the user is hosting themselves?
//!
//! Two layers need the same answer and must not disagree about it:
//!
//! - `wcore-providers`' OpenAI-wire provider, which sends a benign placeholder
//!   bearer instead of failing with `MissingApiKey` when no key is configured.
//! - `wcore-config`'s credential resolution, which must stop refusing to start
//!   before that path is ever reached (FerroxLabs/wayland#1173).
//!
//! It lives here, in the lower crate, because `wcore-providers` already depends
//! on `wcore-config` and a second copy of this predicate is exactly how the two
//! layers would drift into disagreeing about which endpoints are keyless.
//!
//! This answers "is the address one a user plausibly runs a local model on" —
//! it is NOT an authorization decision on its own. Nothing here relaxes what
//! counts as a credential; the only thing a `true` may unlock is the
//! *requirement* for one on an endpoint the user explicitly pointed us at.

use url::{Host, Url};

use crate::compat::ProviderCompat;

/// True when `base_url`'s host is a self-hosted address that is plausibly
/// keyless: loopback (`localhost`, `127.0.0.0/8`, `::1`), unspecified
/// (`0.0.0.0`/`::`), Docker (`host.docker.internal`), mDNS (`*.local`), an
/// RFC1918 private LAN range (`10/8`, `172.16/12`, `192.168/16`), or the
/// Tailscale / CGNAT range (`100.64.0.0/10`). Public hosts return `false`, so a
/// real cloud provider with a missing key still surfaces a clear `MissingApiKey`
/// rather than silently sending a bogus bearer and getting a 401.
///
/// The host comes from the same URL parser the HTTP client itself is built on,
/// so this predicate cannot disagree with the address the request actually goes
/// to. Hand-cutting the authority is what FerroxLabs/wayland#1211 was: taking
/// everything before the first `/` and then the LAST `@`-separated part read
/// `https://api.openai.com?x=@127.0.0.1` as loopback, because the `@` lived in
/// the QUERY and the cut never stopped at `?`. Also cutting at `?` and `#`
/// would still have been wrong — for a special scheme the WHATWG parser treats
/// `\` as a path separator, so `https://api.openai.com\@127.0.0.1` is a request
/// to `api.openai.com` that a hand-rolled cut reads as loopback too.
///
/// A string with no host — scheme-less, relative, or unparsable — is NOT
/// self-hosted. The only thing this predicate can unlock is the waiver of a
/// credential requirement, so an address we cannot resolve fails closed.
#[must_use]
pub fn is_self_hosted_base_url(base_url: &str) -> bool {
    let Ok(parsed) = Url::parse(base_url) else {
        return false;
    };
    match parsed.host() {
        // The parser lowercases and IDNA-normalises the host of a special
        // scheme; the extra fold keeps the comparison honest for any other.
        Some(Host::Domain(domain)) => {
            let host = domain.to_ascii_lowercase();
            host == "localhost"
                || host.ends_with(".localhost")
                || host == "host.docker.internal"
                || host.ends_with(".local")
        }
        Some(Host::Ipv4(addr)) => {
            let [a, b, _, _] = addr.octets();
            addr.is_unspecified()
                || matches!(
                    (a, b),
                    (127, _) | (10, _) | (192, 168) | (172, 16..=31) | (100, 64..=127)
                )
        }
        Some(Host::Ipv6(addr)) => addr.is_loopback() || addr.is_unspecified(),
        None => false,
    }
}

/// The #1173 keyless self-hosted exemption — the WHOLE of it, in one place.
///
/// Every credential gate that means to honour the exemption calls this and
/// nothing else. It exists because the exemption was originally spelled out
/// inline at `Config::resolve` while `resolve_council_provider` re-implemented
/// the surrounding chain without it, so the two gates made opposite decisions
/// on identical config and a keyless local council member was dropped
/// (FerroxLabs/wayland#1212).
///
/// Three conditions, ALL required, and none of them widens what COUNTS as a
/// credential — only whether one is *required*:
///
/// 1. `user_declared_base_url` — the user named this endpoint themselves
///    (`--base-url`, or `[providers.<name>] base_url`). A provider's own
///    default endpoint never qualifies, so this can only fire on an address the
///    user typed.
/// 2. the provider's compat declares its wire HAS a keyless path
///    ([`ProviderCompat::keyless_self_hosted`]). A provider without one keeps
///    its clear startup refusal rather than trading it for an opaque 401
///    several seconds into the first turn.
/// 3. that endpoint is genuinely self-hosted ([`is_self_hosted_base_url`]), so
///    nothing is sent to a public host without a real key.
#[must_use]
pub fn declared_keyless_self_hosted_endpoint(
    user_declared_base_url: bool,
    compat: &ProviderCompat,
    base_url: &str,
) -> bool {
    user_declared_base_url && compat.keyless_self_hosted() && is_self_hosted_base_url(base_url)
}
