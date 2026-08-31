//! The WebFetch permission component (v0.9.2 W4, SPEC §2 #5).
//!
//! Projection for the `WebFetch` tool: a globe header (`🌐`), a `Fetch
//! {host}` title naming the target host, and a body that shows the full URL
//! as plain text plus a single risk-badge line. The badge is `theme.success`
//! `trusted` for an https URL whose host is on the small known-good allowlist
//! (github.com, docs.rs, crates.io, raw.githubusercontent.com), and
//! `theme.warning` `external` for everything else (http, or any host off the
//! allowlist).
//!
//! Risk is decided by the pure helper [`web_fetch_risk`] — scheme + allowlist
//! only, NO live network — so the card is a deterministic, unit-testable
//! function of the URL string.
//!
//! OSC 8 hyperlinking of the URL is Wave 9's job; here the URL is plain text.
//!
//! Classifier-shimmer slot (mirrors Bash): the component renders nothing for
//! an engine classification signal unless one is actually present on the card
//! — it never fabricates a "scanning…" line.
//!
//! Pure over `PermissionContext` — no I/O, no state — so it is unit-tested
//! purely on its title/body/keys text.

use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};

use wcore_types::url_authority::{dialed_host_str, dialed_scheme_host};

use crate::tui::permission::{PermissionComponent, PermissionContext};

/// Permission projection for the `WebFetch` tool.
pub struct WebFetchComponent;

/// The risk class of a fetch target, decided purely from the URL.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Risk {
    /// An https URL whose host is on the known-good allowlist.
    Trusted,
    /// Anything else: http, or an https host off the allowlist.
    External,
}

/// Hosts treated as known-good. Kept small and conservative — documentation
/// and source-hosting origins an agent fetches routinely. An exact host match
/// (or a subdomain of one of these) over https classifies as [`Risk::Trusted`].
const ALLOWLIST: &[&str] = &[
    "github.com",
    "docs.rs",
    "crates.io",
    "raw.githubusercontent.com",
];

/// Classify a fetch target purely from its URL string — scheme + host
/// allowlist, NO live network. An https URL whose host exactly matches (or is
/// a subdomain of) an [`ALLOWLIST`] entry is [`Risk::Trusted`]; an http URL,
/// or any host off the allowlist, is [`Risk::External`].
///
/// The scheme and the host both come from
/// [`wcore_types::url_authority::dialed_scheme_host`] — one parse, the one
/// authority parser. This used to cut the authority out of the string by hand,
/// stopping at `/`, `?` and `#` but not at `\\`, so
/// `https://evil.example\\@github.com/x` read as `github.com` and was badged
/// `trusted`: for a special scheme the WHATWG parser maps `\\` to a path
/// separator, and the fetch goes to `evil.example` (FerroxLabs/wayland#1243).
/// A URL with no host we can vouch for is [`Risk::External`].
pub fn web_fetch_risk(url: &str) -> Risk {
    // Require an explicit https scheme — plain http or a scheme-less string is
    // never trusted.
    let Some((scheme, host)) = dialed_scheme_host(url) else {
        return Risk::External;
    };
    if scheme != "https" || host.is_empty() {
        return Risk::External;
    }
    let trusted = ALLOWLIST
        .iter()
        .any(|allowed| host == *allowed || host.ends_with(&format!(".{allowed}")));
    if trusted {
        Risk::Trusted
    } else {
        Risk::External
    }
}

impl PermissionComponent for WebFetchComponent {
    fn icon(&self) -> &'static str {
        "🌐"
    }

    fn title(&self, ctx: &PermissionContext) -> Line<'static> {
        let url = fetch_url(ctx);
        let host = host_of(&url);
        Line::from(Span::styled(
            format!("Fetch {host}"),
            Style::default()
                .fg(ctx.theme.text)
                .add_modifier(Modifier::BOLD),
        ))
    }

    fn body(&self, ctx: &PermissionContext) -> Vec<Line<'static>> {
        let url = fetch_url(ctx);
        if url.is_empty() {
            // No URL to show — never a raw JSON wall, never a fabricated row.
            return vec![];
        }

        // The full URL as plain text (OSC 8 linkifying is W9's job).
        let mut lines = vec![Line::from(Span::styled(
            url.clone(),
            Style::default().fg(ctx.theme.text_dim),
        ))];

        // The risk badge: `trusted` (success) or `external` (warning).
        let (label, color) = match web_fetch_risk(&url) {
            Risk::Trusted => ("trusted", ctx.theme.success),
            Risk::External => ("external", ctx.theme.warning),
        };
        lines.push(Line::from(Span::styled(label, Style::default().fg(color))));

        lines
    }

    fn keys(&self, ctx: &PermissionContext) -> Line<'static> {
        let always = if ctx.always_allow_available {
            "   [a] always in this workspace"
        } else {
            ""
        };
        Line::from(Span::styled(
            format!("[enter/y] approve{always}   [n] deny   [esc] cancel"),
            Style::default().fg(ctx.theme.text_muted),
        ))
    }
}

/// The fetch target URL: the `url` field from the pretty-printed args when
/// present, else the card `summary` (which previews the args). Empty when
/// neither yields a URL — the body then renders nothing.
fn fetch_url(ctx: &PermissionContext) -> String {
    if let Some(url) = arg_field(&ctx.card.input_pretty, "url")
        && !url.is_empty()
    {
        return url;
    }
    ctx.card.summary.trim().to_string()
}

/// The display host for the title: the host the request is actually dialed
/// against, from the one authority parser. Falls back to the whole string when
/// it does not parse as a URL, and to `a URL` when empty.
///
/// This is the string the user reads before approving, so it must name the
/// host the bytes go to. The hand cut it replaces rendered
/// `Fetch github.com` for `https://evil.example\\@github.com/x`, approving a
/// host the user was never shown (FerroxLabs/wayland#1243).
fn host_of(url: &str) -> String {
    let url = url.trim();
    if url.is_empty() {
        return "a URL".to_string();
    }
    dialed_host_str(url).unwrap_or_else(|| url.to_string())
}

/// Pull a top-level string field out of the pretty-printed args JSON. Returns
/// `None` when the args are not the expected JSON shape.
fn arg_field(input_pretty: &str, key: &str) -> Option<String> {
    serde_json::from_str::<serde_json::Value>(input_pretty)
        .ok()?
        .get(key)?
        .as_str()
        .map(str::to_string)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tui::app::{ToolCardModel, ToolCardStatus};
    use crate::tui::theme::Theme;

    fn card(input_pretty: &str, summary: &str) -> ToolCardModel {
        ToolCardModel {
            call_id: "c1".into(),
            tool_name: "WebFetch".into(),
            summary: summary.into(),
            status: ToolCardStatus::AwaitingApproval,
            output: None,
            edit_preview: None,
            input_pretty: input_pretty.into(),
            approval_reason: String::new(),
            plan_body: None,
            crucible_plan: None,
            path_grant_root: None,
        }
    }

    fn ctx<'a>(c: &'a ToolCardModel, t: &'a Theme) -> PermissionContext<'a> {
        PermissionContext {
            card: c,
            theme: t,
            width: 80,
            always_allow_available: true,
            editable_prefix: None,
            selected_choice: 0,
            expanded: false,
        }
    }

    fn line_text(line: &Line<'_>) -> String {
        line.spans.iter().map(|s| s.content.as_ref()).collect()
    }

    // --- web_fetch_risk: the pure classifier --------------------------------

    #[test]
    fn risk_trusted_for_allowlisted_https_host() {
        assert_eq!(web_fetch_risk("https://github.com/foo/bar"), Risk::Trusted);
        assert_eq!(web_fetch_risk("https://docs.rs/serde"), Risk::Trusted);
        assert_eq!(
            web_fetch_risk("https://crates.io/crates/tokio"),
            Risk::Trusted
        );
        assert_eq!(
            web_fetch_risk("https://raw.githubusercontent.com/o/r/main/x.rs"),
            Risk::Trusted
        );
    }

    #[test]
    fn risk_trusted_for_subdomain_of_allowlisted_host() {
        // A subdomain of an allowlisted origin is still trusted.
        assert_eq!(
            web_fetch_risk("https://api.github.com/repos/o/r"),
            Risk::Trusted
        );
    }

    #[test]
    fn risk_external_for_unknown_https_host() {
        assert_eq!(web_fetch_risk("https://example.com"), Risk::External);
        assert_eq!(web_fetch_risk("https://evil.test/path"), Risk::External);
        // A host that merely *contains* an allowlist entry is not trusted.
        assert_eq!(
            web_fetch_risk("https://github.com.evil.test"),
            Risk::External
        );
    }

    #[test]
    fn risk_external_for_http_even_on_allowlisted_host() {
        // Plain http is never trusted, allowlist host or not.
        assert_eq!(web_fetch_risk("http://github.com/foo"), Risk::External);
        assert_eq!(web_fetch_risk("http://example.com"), Risk::External);
    }

    #[test]
    fn risk_external_for_garbage_or_schemeless() {
        assert_eq!(web_fetch_risk(""), Risk::External);
        assert_eq!(web_fetch_risk("github.com"), Risk::External);
        assert_eq!(web_fetch_risk("ftp://github.com"), Risk::External);
        assert_eq!(web_fetch_risk("https://"), Risk::External);
    }

    // --- #1243 Site A: the authority-smuggling spellings ---------------------

    /// #1243 c1. `https://evil.example\@github.com/x` is a request to
    /// `evil.example` — for a special scheme the WHATWG parser maps `\` to a
    /// path separator, so the path is `/@github.com/x`. The hand cut stopped
    /// at `/`, `?` and `#` but not at `\`, read the authority as
    /// `evil.example\@github.com`, took the last `@`-separated part, and
    /// badged the card `trusted`.
    ///
    /// The set covered here is closed by construction rather than by
    /// enumeration: the risk verdict now comes from the URL parser's own
    /// authority state machine, so every separator that machine honours ends
    /// the authority. The rows are the spellings that have actually been seen
    /// to defeat a hand cut, kept as regressions.
    #[test]
    fn risk_external_for_every_authority_smuggling_spelling() {
        for url in [
            // #1243: the backslash spelling, the one that was open.
            r"https://evil.example\@github.com/x",
            r"https://evil.example\@github.com",
            r"https://evil.example\@docs.rs/x",
            r"https://evil.example\@github.com:443/x",
            // #1211's query spelling, and the fragment sibling.
            "https://evil.example?z=@github.com",
            "https://evil.example#@github.com",
            // Honest userinfo: the host is still not the allowlisted name.
            "https://github.com@evil.example/x",
            "https://user:p@ss@evil.example/x",
            // A tab inside the authority is stripped by the parser, not a
            // separator a hand cut would have had to learn about separately.
            "https://evil.example\t\\@github.com/x",
        ] {
            assert_eq!(
                web_fetch_risk(url),
                Risk::External,
                "smuggled authority classified as trusted: {url}"
            );
        }
    }

    /// The wrong-refusal control for the row above. A classifier that simply
    /// returned `External` for everything would pass that test; these are the
    /// genuinely safe URLs that must still read `Trusted`, so the two together
    /// pin both directions.
    #[test]
    fn risk_still_trusted_for_genuinely_allowlisted_urls() {
        for url in [
            "https://github.com/rust-lang/rust",
            "https://api.github.com/repos/o/r",
            "https://docs.rs/serde/latest/serde/",
            "https://crates.io/crates/tokio",
            "https://raw.githubusercontent.com/o/r/main/x.rs",
            // Userinfo pointing AT an allowlisted host is still that host.
            "https://user@github.com/x",
            // Uppercase scheme and host normalise, they do not de-trust.
            "HTTPS://GitHub.COM/x",
        ] {
            assert_eq!(
                web_fetch_risk(url),
                Risk::Trusted,
                "a genuinely safe URL was refused: {url}"
            );
        }
    }

    /// #1243 c2. The prompt TITLE is what the user reads before approving, so
    /// it must name the host the bytes reach. The hand cut rendered
    /// `Fetch github.com` for a fetch to `evil.example`.
    #[test]
    fn title_names_the_host_actually_dialed_not_the_smuggled_one() {
        let t = Theme::hearth();
        let c = card(r#"{"url":"https://evil.example\\@github.com/x"}"#, "");
        let title = line_text(&WebFetchComponent.title(&ctx(&c, &t)));
        assert_eq!(title, "Fetch evil.example", "title: {title}");
        assert_ne!(title, "Fetch github.com", "title: {title}");

        // And the badge on the same card is `external`, not `trusted`.
        let body = WebFetchComponent.body(&ctx(&c, &t));
        let joined = body.iter().map(line_text).collect::<Vec<_>>().join("\n");
        assert!(joined.contains("external"), "badge: {joined}");
        assert!(!joined.contains("trusted"), "badge: {joined}");
    }

    // --- title: carries the host --------------------------------------------

    #[test]
    fn icon_is_the_globe_glyph() {
        assert_eq!(WebFetchComponent.icon(), "🌐");
    }

    #[test]
    fn title_carries_the_host_from_args() {
        let t = Theme::hearth();
        let c = card(r#"{"url":"https://example.com/path?q=1"}"#, "");
        let title = line_text(&WebFetchComponent.title(&ctx(&c, &t)));
        assert_eq!(title, "Fetch example.com", "title: {title}");
    }

    #[test]
    fn title_falls_back_to_summary_url() {
        let t = Theme::hearth();
        // No parseable args; URL comes from the summary preview.
        let c = card("not json", "https://docs.rs/serde/latest");
        let title = line_text(&WebFetchComponent.title(&ctx(&c, &t)));
        assert_eq!(title, "Fetch docs.rs", "title: {title}");
    }

    #[test]
    fn title_strips_userinfo_and_port_from_host() {
        let t = Theme::hearth();
        let c = card(r#"{"url":"https://user@example.com:8443/x"}"#, "");
        let title = line_text(&WebFetchComponent.title(&ctx(&c, &t)));
        assert_eq!(title, "Fetch example.com", "title: {title}");
    }

    #[test]
    fn title_is_a_url_when_empty() {
        let t = Theme::hearth();
        let c = card("not json", "");
        let title = line_text(&WebFetchComponent.title(&ctx(&c, &t)));
        assert_eq!(title, "Fetch a URL", "title: {title}");
    }

    // --- body: full URL + risk badge ----------------------------------------

    #[test]
    fn body_shows_full_url_then_external_badge() {
        let t = Theme::hearth();
        let c = card(r#"{"url":"https://example.com/deep/path"}"#, "");
        let body = WebFetchComponent.body(&ctx(&c, &t));
        let joined = body.iter().map(line_text).collect::<Vec<_>>().join("\n");
        // The full URL appears verbatim (not just the host).
        assert!(
            joined.contains("https://example.com/deep/path"),
            "full url missing: {joined}"
        );
        // External host → `external` warning badge.
        assert!(joined.contains("external"), "risk badge missing: {joined}");
        assert!(!joined.contains("trusted"), "wrong badge: {joined}");
        // Badge is rendered in the theme's warning color.
        let badge = body.last().unwrap();
        assert_eq!(badge.spans[0].style.fg, Some(t.warning), "badge color");
    }

    #[test]
    fn body_shows_trusted_badge_for_allowlisted_host() {
        let t = Theme::hearth();
        let c = card(r#"{"url":"https://github.com/rust-lang/rust"}"#, "");
        let body = WebFetchComponent.body(&ctx(&c, &t));
        let joined = body.iter().map(line_text).collect::<Vec<_>>().join("\n");
        assert!(
            joined.contains("https://github.com/rust-lang/rust"),
            "full url missing: {joined}"
        );
        assert!(
            joined.contains("trusted"),
            "trusted badge missing: {joined}"
        );
        assert!(!joined.contains("external"), "wrong badge: {joined}");
        // Badge is rendered in the theme's success color.
        let badge = body.last().unwrap();
        assert_eq!(badge.spans[0].style.fg, Some(t.success), "badge color");
    }

    #[test]
    fn body_is_empty_without_a_url() {
        let t = Theme::hearth();
        // No url field, no summary → empty body, never a JSON wall or a
        // fabricated classifier-shimmer line.
        let c = card("not json", "");
        let body = WebFetchComponent.body(&ctx(&c, &t));
        assert!(body.is_empty(), "expected empty body, got {}", body.len());
    }

    // --- keys + default action ----------------------------------------------

    #[test]
    fn keys_offer_approve_always_deny_cancel() {
        let t = Theme::hearth();
        let c = card(r#"{"url":"https://example.com"}"#, "");
        let keys = line_text(&WebFetchComponent.keys(&ctx(&c, &t)));
        assert!(keys.contains("approve"), "keys: {keys}");
        assert!(keys.contains("always"), "keys: {keys}");
        assert!(keys.contains("deny"), "keys: {keys}");
        assert!(keys.contains("cancel"), "keys: {keys}");
    }

    #[test]
    fn keys_hide_always_when_the_gate_is_closed() {
        let t = Theme::hearth();
        let c = card(r#"{"url":"https://example.com"}"#, "");
        let mut context = ctx(&c, &t);
        context.always_allow_available = false;
        let keys = line_text(&WebFetchComponent.keys(&context));
        assert!(!keys.contains("always"), "always must be hidden: {keys}");
        assert!(keys.contains("approve"), "approve still present: {keys}");
    }

    #[test]
    fn default_action_is_approve_once() {
        use crate::tui::permission::ApprovalAction;
        assert_eq!(
            WebFetchComponent.default_action(),
            ApprovalAction::ApproveOnce
        );
    }
}
