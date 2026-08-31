//! The structural redaction boundary for portability plans (F26-01).
//!
//! # Why this type exists
//!
//! A secret that is redacted when PRINTED but still present in the typed value
//! has leaked to every consumer that serializes it — and `migrate --json`
//! creates exactly such a consumer. Withholding the value in one printer is
//! cosmetic: `Debug`, `serde`, a log line and an error formatter each get their
//! own chance to emit it, and every one of them has to remember.
//!
//! So the value is made **unrepresentable** instead. [`CredentialRef`] records
//! only where a credential came from — the variable or key name, and the file
//! relative to the source home. There is deliberately no field, no variant and
//! no accessor capable of carrying the secret itself, so `Debug`, `Display`,
//! `serde` and every error path inherit the redaction from the TYPE rather than
//! each having to implement it.
//!
//! This is a boundary type, not a container: a caller that holds a real secret
//! (the Hermes mapper does, when `--include-credentials` is passed) converts to
//! a `CredentialRef` and the value is dropped at the conversion. There is no
//! inverse — you cannot go from a `CredentialRef` back to a value.

use serde::{Deserialize, Serialize};

/// The longest plausible credential NAME. Env vars and dotted key paths are
/// short; provider secrets are not.
const MAX_NAME_LEN: usize = 128;

/// Force a credential name into an identifier shape.
///
/// A name is `[A-Za-z0-9_.:/-]+` and short. Anything else is not a name, so it
/// is replaced rather than carried — the field reserved for a LOCATION must not
/// become a second channel for a value.
pub fn sanitize_name(raw: &str) -> String {
    let ok = !raw.is_empty()
        && raw.len() <= MAX_NAME_LEN
        && raw
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '.' | ':' | '/' | '-'));
    if ok {
        raw.to_string()
    } else {
        "<invalid-credential-name>".to_string()
    }
}

fn deserialize_name<'de, D>(d: D) -> Result<String, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let raw = String::deserialize(d)?;
    Ok(sanitize_name(&raw))
}

/// A discovered credential, represented by its SOURCE REFERENCE only.
///
/// # Invariant
///
/// This struct has exactly two fields, both of which name a LOCATION. Adding a
/// field that can hold a credential value — or a `From<…>` that stores one —
/// would silently convert every consumer of a portability plan into a secret
/// sink. The multi-emitter probe in `crates/wcore-cli/tests/migrate_typed_dryrun.rs`
/// exists to catch exactly that regression.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct CredentialRef {
    /// The environment variable or configuration key the credential was found
    /// under — e.g. `DEEPSEEK_API_KEY`, or `gateway.auth.token`.
    ///
    /// Constrained to an IDENTIFIER shape by [`sanitize_name`] on every
    /// construction path, including deserialization. A credential name is
    /// always an identifier or a dotted key path; a credential VALUE is not.
    /// Narrowing the field this way is what stops a producer — or a hostile
    /// document — from smuggling a value through the field reserved for a name.
    #[serde(deserialize_with = "deserialize_name")]
    pub name: String,
    /// The file it was found in, relative to the source home — e.g.
    /// `profiles/fred/.env`. Relative so that an absolute path on the
    /// discovering machine never reaches an emitted document.
    pub source_file: String,
}

impl CredentialRef {
    /// Record a credential by reference.
    ///
    /// Note the signature: there is no parameter for the value. A caller that
    /// happens to be holding one cannot pass it in even by accident.
    pub fn new(name: impl Into<String>, source_file: impl Into<String>) -> Self {
        Self {
            name: sanitize_name(&name.into()),
            source_file: source_file.into(),
        }
    }
}

impl std::fmt::Display for CredentialRef {
    /// Renders the reference. There is nothing secret to withhold here — the
    /// type cannot hold a value — so this is safe by construction rather than
    /// by remembering to elide something.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{} (from {})", self.name, self.source_file)
    }
}

/// Configuration key fragments whose VALUE is credential material.
const SECRET_KEY_FRAGMENTS: &[&str] = &[
    "token",
    "secret",
    "apikey",
    "api_key",
    "password",
    "passwd",
    "auth",
    "credential",
    "key",
];

fn is_secret_key(name: &str) -> bool {
    let low = name.to_ascii_lowercase();
    // `max_tokens` / `keywords` / `authorized` are structural look-alikes.
    const ALLOW: &[&str] = &[
        "max_tokens",
        "maxtokens",
        "keywords",
        "authorized",
        "tokenizer",
    ];
    if ALLOW.contains(&low.as_str()) {
        return false;
    }
    SECRET_KEY_FRAGMENTS.iter().any(|f| low.contains(f))
}

/// Scrub credential material that is EMBEDDED inside an otherwise-ordinary
/// string before it is placed in a plan's free-form `details` map.
///
/// # Why this exists
///
/// [`CredentialRef`] closes the path where a credential is a first-class
/// discovered value. It does NOT close the path where a credential is embedded
/// inside a value that is legitimately reported — an MCP server `url` carrying
/// `?token=…`, a `command` line carrying `--api-key …`, or a `base_url` with
/// HTTP userinfo. Those strings come from a peer configuration, which is
/// untrusted input, and they flow into an untyped `BTreeMap<String, String>`
/// that offers the value no resistance.
///
/// This was found by the F26-01 redaction panel: two independent members named
/// `DiscoveredItem::details` as an uncovered channel, so it was fixed and
/// re-measured rather than voted on.
pub fn scrub_detail(value: &str) -> String {
    let mut out = strip_url_userinfo(value);
    out = strip_secret_query_params(&out);
    strip_secret_flags(&out)
}

/// `scheme://user:pass@host/…` ⇒ `scheme://<redacted>@host/…`
fn strip_url_userinfo(v: &str) -> String {
    let Some(scheme_end) = v.find("://") else {
        return v.to_string();
    };
    let rest_start = scheme_end + 3;
    let rest = &v[rest_start..];
    // Userinfo ends at the first `@` that precedes the first `/`.
    let authority_end = rest.find('/').unwrap_or(rest.len());
    let Some(at) = rest[..authority_end].find('@') else {
        return v.to_string();
    };
    format!("{}<redacted>@{}", &v[..rest_start], &rest[at + 1..])
}

/// `?token=abc&x=1` ⇒ `?token=<redacted>&x=1`
fn strip_secret_query_params(v: &str) -> String {
    let Some(q) = v.find('?') else {
        return v.to_string();
    };
    let (head, query) = v.split_at(q + 1);
    let scrubbed: Vec<String> = query
        .split('&')
        .map(|pair| match pair.split_once('=') {
            Some((k, _)) if is_secret_key(k) => format!("{k}=<redacted>"),
            _ => pair.to_string(),
        })
        .collect();
    format!("{head}{}", scrubbed.join("&"))
}

/// `--api-key SECRET` / `--token=SECRET` ⇒ the value replaced.
fn strip_secret_flags(v: &str) -> String {
    let toks: Vec<&str> = v.split_whitespace().collect();
    let mut out: Vec<String> = Vec::with_capacity(toks.len());
    let mut redact_next = false;
    for t in toks {
        if redact_next {
            out.push("<redacted>".to_string());
            redact_next = false;
            continue;
        }
        if let Some(flag) = t.strip_prefix("--")
            && let Some((name, _)) = flag.split_once('=')
            && is_secret_key(name)
        {
            out.push(format!("--{name}=<redacted>"));
            continue;
        }
        if let Some(flag) = t.strip_prefix("--")
            && is_secret_key(flag)
        {
            out.push(t.to_string());
            redact_next = true;
            continue;
        }
        out.push(t.to_string());
    }
    // Preserve the original when there was no whitespace splitting to do.
    if out.len() == 1 && !v.contains(char::is_whitespace) {
        return out.into_iter().next().unwrap_or_default();
    }
    out.join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scrub_detail_removes_embedded_credentials_but_keeps_the_shape() {
        // URL userinfo.
        assert_eq!(
            scrub_detail("https://alice:hunter2@example.com/mcp"),
            "https://<redacted>@example.com/mcp"
        );
        // Secret-named query parameter, non-secret ones preserved.
        assert_eq!(
            scrub_detail("https://example.com/mcp?token=SEKRIT&mode=fast"),
            "https://example.com/mcp?token=<redacted>&mode=fast"
        );
        // Command flags, both spellings.
        assert_eq!(
            scrub_detail("srv --api-key=SEKRIT --verbose"),
            "srv --api-key=<redacted> --verbose"
        );
        assert_eq!(
            scrub_detail("srv --token SEKRIT --port 80"),
            "srv --token <redacted> --port 80"
        );
        // NEGATIVE controls: an ordinary value must survive untouched, or the
        // plan's details map would become useless.
        assert_eq!(
            scrub_detail("https://api.deepseek.com/v1"),
            "https://api.deepseek.com/v1"
        );
        assert_eq!(scrub_detail("deepseek-v4-pro"), "deepseek-v4-pro");
        assert_eq!(
            scrub_detail("srv --max_tokens 8192"),
            "srv --max_tokens 8192",
            "max_tokens is a structural look-alike, not a secret"
        );
    }

    /// wayland#1252 c5, SITE C. `strip_url_userinfo` cut the authority with
    /// `find("://")` + `rest.find('/')` + `find('@')`, so a URL that dials
    /// `evil.example` (path `/@github.com/x`) and a genuinely
    /// credential-bearing `github.com` URL rendered to the SAME string — a
    /// reader of the redacted detail could not tell the first one apart from
    /// the second.
    #[test]
    fn a_smuggled_authority_is_not_rendered_as_the_surviving_host() {
        let smuggled = scrub_detail(r"https://evil.example\@github.com/x");
        let credentialed = scrub_detail("https://user:pw@github.com/x");
        assert_ne!(
            smuggled, credentialed,
            "a URL that dials evil.example renders identically to a \
             credential-bearing github.com URL"
        );
        assert!(
            !smuggled.contains("<redacted>@github.com"),
            "the smuggled URL still names github.com as the surviving host: \
             {smuggled}"
        );

        // CONTROL: an ordinary credential-bearing URL is still redacted, so
        // the fix above is not bought by giving up the redaction.
        assert_eq!(
            credentialed, "https://<redacted>@github.com/x",
            "an ordinary credential-bearing URL stopped being redacted"
        );
        for leaked in ["user", "pw@"] {
            assert!(
                !credentialed.contains(leaked),
                "credential material {leaked:?} survived: {credentialed}"
            );
        }
        // CONTROL: a URL with no credential at all is untouched.
        assert_eq!(
            scrub_detail("https://github.com/x"),
            "https://github.com/x",
            "an ordinary URL was rewritten"
        );
    }

    #[test]
    fn credential_ref_carries_no_value_through_any_emitter() {
        // The canonical failure this type prevents: a caller holds a real
        // secret and records the credential. The value must not survive into
        // any rendering, and the type must give it nowhere to live.
        let secret = "sk-live-THIS-MUST-NEVER-APPEAR-0123456789";
        let c = CredentialRef::new("DEEPSEEK_API_KEY", "profiles/fred/.env");

        let json = serde_json::to_string(&c).unwrap();
        let debug = format!("{c:?}");
        let display = format!("{c}");
        // The error path is a real emitter: a plan is often reported as part of
        // a failure, and `anyhow`'s Debug rendering is what a user sees.
        let err = format!("{:?}", anyhow::anyhow!("import failed for {c}"));

        for (what, rendered) in [
            ("json", &json),
            ("debug", &debug),
            ("display", &display),
            ("error", &err),
        ] {
            assert!(
                !rendered.contains(secret),
                "credential value leaked through the {what} emitter: {rendered}"
            );
        }

        // Positive half — without it a type that rendered to the empty string
        // would pass the assertions above vacuously.
        assert!(
            json.contains("DEEPSEEK_API_KEY"),
            "json lost the name: {json}"
        );
        assert!(
            json.contains("profiles/fred/.env"),
            "json lost the source file: {json}"
        );
        assert!(
            debug.contains("DEEPSEEK_API_KEY"),
            "debug is empty: {debug}"
        );
    }

    #[test]
    fn a_secret_cannot_be_smuggled_through_the_name_field() {
        // The round-3 panel objection: `name` is a public deserializable String,
        // so a producer or a hostile document could put a VALUE there. A name is
        // an identifier; a secret is not, so the field is narrowed to that shape.
        let secret = "sk-live-abcdefghijklmnop!!/QQ==+longvalue+with+padding+and+more+entropy+here";
        let c = CredentialRef::new(secret, ".env");
        assert!(
            !format!("{c:?} {c} {}", serde_json::to_string(&c).unwrap()).contains(secret),
            "a secret survived in the name field"
        );

        // Hostile deserialization must be narrowed too.
        let hostile = format!(r#"{{"name":"{secret}","source_file":".env"}}"#);
        let parsed: CredentialRef = serde_json::from_str(&hostile).unwrap();
        assert!(
            !serde_json::to_string(&parsed).unwrap().contains(secret),
            "deserialization bypassed the name narrowing"
        );

        // NEGATIVE controls: real names must survive untouched, or discovery
        // output becomes useless.
        assert_eq!(
            CredentialRef::new("DEEPSEEK_API_KEY", "x").name,
            "DEEPSEEK_API_KEY"
        );
        assert_eq!(
            CredentialRef::new("gateway.auth.token", "x").name,
            "gateway.auth.token"
        );
        assert_eq!(
            CredentialRef::new("models.providers.flux.apiKey", "x").name,
            "models.providers.flux.apiKey"
        );
    }

    #[test]
    fn credential_ref_json_shape_has_exactly_two_location_fields() {
        // Guards the invariant directly: if someone adds a value-bearing field,
        // this fails rather than waiting for a leak to be observed downstream.
        let c = CredentialRef::new("OPENROUTER_API_KEY", ".env");
        let v: serde_json::Value = serde_json::to_value(&c).unwrap();
        let obj = v
            .as_object()
            .expect("CredentialRef must serialize as an object");
        let mut keys: Vec<&str> = obj.keys().map(String::as_str).collect();
        keys.sort_unstable();
        assert_eq!(
            keys,
            vec!["name", "source_file"],
            "CredentialRef gained a field; if it can hold a credential value the \
             structural redaction contract is broken"
        );
    }

    #[test]
    fn credential_ref_ordering_is_total_and_by_location() {
        // Discovery sorts by this ordering, so it must be total and derived
        // from the data rather than from walk order.
        let a = CredentialRef::new("A_KEY", "a/.env");
        let b = CredentialRef::new("B_KEY", "a/.env");
        let c = CredentialRef::new("A_KEY", "b/.env");
        assert!(a < b, "same file, name orders");
        assert!(a < c, "same name, source_file orders");
        // `name` is the FIRST field, so it dominates: c (A_KEY) sorts before
        // b (B_KEY) even though c's file sorts later.
        assert!(c < b, "name must dominate source_file in the ordering");
    }
}
