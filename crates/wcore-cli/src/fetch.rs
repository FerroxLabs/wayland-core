//! CLI surface: `wayland-core fetch` — FluxRouter web_fetch.
//!
//! A thin command wrapper over [`wcore_providers::flux_fetch::FluxFetchClient`]
//! (the dedicated, non-chat web_fetch client). It resolves the Flux Bearer key
//! and base URL, fetches one URL, and prints the returned markdown to stdout.
//!
//! Key/base resolution precedence (highest first), identical to `image`:
//!   key:  `--api-key` → `$FLUX_API_KEY` → `[providers.flux-router].api_key`
//!         (and the `[providers.flux]` alias) in the global `config.toml`.
//!   base: `--base-url` → `[providers.flux-router].base_url`
//!         → [`FLUX_ROUTER_DEFAULT_BASE_URL`]. The endpoint is `{base}/fetch`.
//!
//! Paid-only gating (contract §2/§4.6): a free / paid-but-uncleared key returns
//! `402 upgrade_required`, surfaced as a distinct "requires an upgrade" message
//! via the typed [`ProviderError::UpgradeRequired`] from T1. SSRF (`blocked
//! target`) and social-host (`social_blocked`) refusals come back as `400` and
//! are surfaced verbatim.

use std::io::Write as _;
use std::path::Path;

use anyhow::{Context, Result, bail};
use clap::Args;
use toml::value::Table;

use wcore_providers::ProviderError;
use wcore_providers::flux_fetch::{FetchRequest, FluxFetchClient};
use wcore_providers::flux_router::FLUX_ROUTER_DEFAULT_BASE_URL;

/// `wayland-core fetch` arguments.
#[derive(Args, Debug)]
pub struct FetchArgs {
    /// The URL to fetch (required, non-empty, http(s) only). One URL per call.
    pub url: String,

    /// Use the JS-rendered premium arm (scrape.do, ~$0.02 vs ~$0.005). Needed
    /// for JS-heavy pages the default reader can't render.
    #[arg(long)]
    pub render: bool,

    /// Override the Flux Bearer key (else `$FLUX_API_KEY` / config).
    #[arg(long)]
    pub api_key: Option<String>,

    /// Override the Flux base URL ending in `/v1` (else config / default).
    #[arg(long)]
    pub base_url: Option<String>,
}

/// Production entry point — resolves credentials from the global config.
pub async fn run(args: FetchArgs) -> Result<()> {
    let config_path = wcore_config::config::global_config_path();
    run_with_config_path(args, &config_path).await
}

/// Test-friendly entry: resolve credentials against an explicit config path.
pub async fn run_with_config_path(args: FetchArgs, config_path: &Path) -> Result<()> {
    if args.url.trim().is_empty() {
        bail!("fetch: <url> must be non-empty");
    }

    let doc = load_doc(config_path)?;
    let api_key = resolve_key(&args.api_key, &doc).context(
        "no Flux API key (set --api-key, $FLUX_API_KEY, or [providers.flux-router] in config)",
    )?;
    let base_url = resolve_base_url(&args.base_url, &doc);

    let request = FetchRequest::new(args.url.trim()).with_render(args.render);

    let client = FluxFetchClient::new(&api_key, &base_url);
    let response = match client.fetch(&request).await {
        Ok(r) => r,
        Err(e) => bail!("{}", format_provider_error(&e)),
    };

    // Markdown to stdout (the payload); the echoed URL goes to stderr so it
    // does not contaminate a piped capture of the markdown.
    eprintln!("fetched {} ({})", response.url, response.format);
    std::io::stdout()
        .write_all(response.markdown.as_bytes())
        .context("writing markdown to stdout")?;
    if !response.markdown.ends_with('\n') {
        println!();
    }

    Ok(())
}

/// Map a [`ProviderError`] to a user-facing message, keeping the typed
/// entitlement distinction (feature lock vs account state) intact and
/// surfacing the server's SSRF / social-host `400` reason verbatim.
fn format_provider_error(e: &ProviderError) -> String {
    match e {
        ProviderError::PremiumLocked { .. }
        | ProviderError::UpgradeRequired { .. }
        | ProviderError::SpendCeilingUnresolved { .. } => e.to_string(),
        ProviderError::Api { status, message } if *status == 400 => {
            // Contract §4.6: SSRF (`blocked target:…`) and social-host
            // (`social_blocked`) refusals arrive as 400 with the reason in the
            // body. Surface it verbatim so the user sees WHY.
            format!("web_fetch refused (HTTP 400): {message}")
        }
        other => format!("web_fetch failed: {other}"),
    }
}

/// Load the global config TOML. A missing file is fine (env/flag may carry the
/// key); a malformed file is a hard error.
fn load_doc(config_path: &Path) -> Result<Table> {
    if !config_path.exists() {
        return Ok(Table::new());
    }
    let body = std::fs::read_to_string(config_path)
        .with_context(|| format!("reading config at {}", config_path.display()))?;
    toml::from_str::<Table>(&body)
        .with_context(|| format!("parsing config at {}", config_path.display()))
}

/// Read `[providers.<slug>].<field>` as a string from the parsed doc.
fn provider_field<'a>(doc: &'a Table, slug: &str, field: &str) -> Option<&'a str> {
    doc.get("providers")?
        .as_table()?
        .get(slug)?
        .as_table()?
        .get(field)?
        .as_str()
}

/// Resolve the Flux Bearer key: flag → `$FLUX_API_KEY` → config table
/// (`flux-router`, then the `flux` alias).
fn resolve_key(flag: &Option<String>, doc: &Table) -> Result<String> {
    if let Some(k) = flag
        && !k.trim().is_empty()
    {
        return Ok(k.trim().to_string());
    }
    if let Ok(k) = std::env::var("FLUX_API_KEY")
        && !k.trim().is_empty()
    {
        return Ok(k);
    }
    for slug in ["flux-router", "flux"] {
        if let Some(k) = provider_field(doc, slug, "api_key")
            && !k.trim().is_empty()
        {
            return Ok(k.to_string());
        }
    }
    bail!("no Flux API key found")
}

/// Resolve the base URL: flag → config (`flux-router`, then `flux`) →
/// the canonical default.
fn resolve_base_url(flag: &Option<String>, doc: &Table) -> String {
    if let Some(b) = flag
        && !b.trim().is_empty()
    {
        return b.trim().to_string();
    }
    for slug in ["flux-router", "flux"] {
        if let Some(b) = provider_field(doc, slug, "base_url")
            && !b.trim().is_empty()
        {
            return b.to_string();
        }
    }
    FLUX_ROUTER_DEFAULT_BASE_URL.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn doc_from(toml_str: &str) -> Table {
        toml::from_str::<Table>(toml_str).expect("valid toml")
    }

    #[test]
    fn resolve_key_prefers_flag() {
        let doc = doc_from("[providers.flux-router]\napi_key = \"from-config\"\n");
        let key = resolve_key(&Some("from-flag".into()), &doc).unwrap();
        assert_eq!(key, "from-flag");
    }

    #[test]
    fn resolve_key_falls_back_to_config_table() {
        let doc = doc_from("[providers.flux-router]\napi_key = \"sk-config\"\n");
        if std::env::var("FLUX_API_KEY").is_err() {
            assert_eq!(resolve_key(&None, &doc).unwrap(), "sk-config");
        }
    }

    #[test]
    fn resolve_key_uses_flux_alias_table() {
        let doc = doc_from("[providers.flux]\napi_key = \"sk-alias\"\n");
        if std::env::var("FLUX_API_KEY").is_err() {
            assert_eq!(resolve_key(&None, &doc).unwrap(), "sk-alias");
        }
    }

    #[test]
    fn resolve_base_url_defaults_to_flux_v1() {
        let doc = Table::new();
        assert_eq!(resolve_base_url(&None, &doc), FLUX_ROUTER_DEFAULT_BASE_URL);
    }

    #[test]
    fn resolve_base_url_prefers_flag_then_config() {
        let doc = doc_from("[providers.flux-router]\nbase_url = \"https://cfg/v1\"\n");
        assert_eq!(
            resolve_base_url(&Some("https://flag/v1".into()), &doc),
            "https://flag/v1"
        );
        assert_eq!(resolve_base_url(&None, &doc), "https://cfg/v1");
    }

    #[tokio::test]
    async fn empty_url_is_rejected() {
        let args = FetchArgs {
            url: "   ".into(),
            render: false,
            api_key: Some("k".into()),
            base_url: None,
        };
        let err = run_with_config_path(args, Path::new("/nonexistent/config.toml"))
            .await
            .unwrap_err();
        assert!(err.to_string().contains("<url>"));
    }

    #[test]
    fn upgrade_required_renders_upgrade_message() {
        let e = ProviderError::UpgradeRequired {
            message: "web_fetch is a paid capability; upgrade or clear a charge".into(),
        };
        let msg = format_provider_error(&e);
        assert!(msg.contains("requires an upgrade"));
    }

    #[test]
    fn ssrf_400_is_surfaced_verbatim() {
        // Contract §4.6: a `blocked target:…` SSRF refusal arrives as a 400 and
        // must reach the user with its reason, NOT be masked as an upgrade.
        let e = ProviderError::Api {
            status: 400,
            message: "{\"error\":\"blocked target: 169.254.169.254\"}".into(),
        };
        let msg = format_provider_error(&e);
        assert!(msg.contains("HTTP 400"));
        assert!(msg.contains("blocked target"));
    }
}
