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

/// True when `base_url`'s host is a self-hosted address that is plausibly
/// keyless: loopback (`localhost`, `127.0.0.0/8`, `::1`), unspecified
/// (`0.0.0.0`/`::`), Docker (`host.docker.internal`), mDNS (`*.local`), an
/// RFC1918 private LAN range (`10/8`, `172.16/12`, `192.168/16`), or the
/// Tailscale / CGNAT range (`100.64.0.0/10`). Public hosts return `false`, so a
/// real cloud provider with a missing key still surfaces a clear `MissingApiKey`
/// rather than silently sending a bogus bearer and getting a 401.
#[must_use]
pub fn is_self_hosted_base_url(base_url: &str) -> bool {
    // Host = strip scheme, take the AUTHORITY, drop any `user@`, strip the
    // `:port`. IPv6 literals are bracketed (`[::1]:11434`).
    //
    // The authority ends at the first of `/`, `?`, `#` or `\` — not at `/`
    // alone. Cutting at `/` alone left a query string, a fragment or a
    // backslash-smuggled userinfo inside the "authority", and the `rsplit('@')`
    // below then read whatever private literal had been parked there: the
    // predicate called `https://api.openai.com?x=@127.0.0.1` self-hosted, the
    // startup gate exempted it, and the prompt went to the PUBLIC host with the
    // placeholder bearer instead of being refused. `\` is in the set because
    // reqwest (WHATWG) maps it to `/` for special schemes, so the host actually
    // dialled from `https://api.openai.com\@127.0.0.1` is api.openai.com — the
    // same smuggle `SseTransport::resolve_endpoint` already defends against.
    let after_scheme = base_url
        .split_once("://")
        .map(|(_, r)| r)
        .unwrap_or(base_url);
    let authority = after_scheme
        .split(['/', '?', '#', '\\'])
        .next()
        .unwrap_or("");
    let host_port = authority.rsplit('@').next().unwrap_or(authority);
    let host = if let Some(rest) = host_port.strip_prefix('[') {
        rest.split(']').next().unwrap_or(rest)
    } else {
        host_port.split(':').next().unwrap_or(host_port)
    }
    .trim()
    .to_ascii_lowercase();

    if host.is_empty() {
        return false;
    }
    if host == "localhost"
        || host.ends_with(".localhost")
        || host == "host.docker.internal"
        || host.ends_with(".local")
        || host == "0.0.0.0"
        || host == "::1"
        || host == "::"
    {
        return true;
    }
    // IPv4 loopback / private / CGNAT ranges. Every dotted segment must parse as
    // a u8, so a hostname like `api.openai.com` (a non-numeric segment) yields an
    // empty vec and falls through to `false`.
    let octets: Vec<u8> = host
        .split('.')
        .map(|o| o.parse::<u8>())
        .collect::<Result<Vec<_>, _>>()
        .unwrap_or_default();
    if octets.len() == 4 {
        return matches!(
            (octets[0], octets[1]),
            (127, _) | (10, _) | (192, 168) | (172, 16..=31) | (100, 64..=127)
        );
    }
    false
}
