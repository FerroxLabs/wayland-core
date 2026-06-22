//! CLI surface: `wayland-core image` — FluxRouter image generation.
//!
//! A thin command wrapper over [`wcore_providers::flux_image::FluxImageClient`]
//! (the dedicated, non-chat image client). It resolves the Flux Bearer key and
//! base URL, builds the request, calls the live endpoint, decodes the returned
//! base64 image, and writes it to `--out` (or stdout when piped). The SynthID
//! watermark notice (Gemini arms) is surfaced on stderr.
//!
//! Key/base resolution precedence (highest first):
//!   key:  `--api-key` → `$FLUX_API_KEY` → `[providers.flux-router].api_key`
//!         (and the `[providers.flux]` alias) in the global `config.toml`.
//!   base: `--base-url` → `[providers.flux-router].base_url`
//!         → [`FLUX_ROUTER_DEFAULT_BASE_URL`].
//!
//! Paid-only gating (contract §2/§3.6): a free / paid-but-uncleared key returns
//! `402 premium_locked`, surfaced as a distinct "requires a paid Flux plan"
//! message via the typed [`ProviderError::PremiumLocked`] from T1.

use std::io::Write as _;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use clap::Args;
use toml::value::Table;

use wcore_providers::ProviderError;
use wcore_providers::flux_image::{FluxImageClient, ImageRequest};
use wcore_providers::flux_router::FLUX_ROUTER_DEFAULT_BASE_URL;

/// `wayland-core image` arguments.
#[derive(Args, Debug)]
pub struct ImageArgs {
    /// The image prompt (required, non-empty).
    #[arg(long)]
    pub prompt: String,

    /// Image arm / provider (e.g. `flux-image-together-flux`, `nano-banana`,
    /// `gpt-image-high`). Omit for the cheapest default (together-flux).
    #[arg(long)]
    pub model: Option<String>,

    /// Number of images to generate. Defaults to 1; keep at 1 for premium arms
    /// (they can exceed the ~60s sync timeout otherwise).
    #[arg(long, default_value_t = 1)]
    pub n: u32,

    /// Image size (honored only by together-flux; other arms use a fixed size).
    #[arg(long)]
    pub size: Option<String>,

    /// Output file path. With `--n > 1` an index is inserted before the
    /// extension (`out.png` → `out-1.png`, `out-2.png`, …). When omitted, the
    /// single image is written to stdout (only valid for `--n 1`).
    #[arg(long)]
    pub out: Option<PathBuf>,

    /// USD price ceiling. If the final (post-PAYG) price exceeds this, Flux
    /// returns 402 and does NOT charge.
    #[arg(long)]
    pub max_price: Option<f64>,

    /// Override the Flux Bearer key (else `$FLUX_API_KEY` / config).
    #[arg(long)]
    pub api_key: Option<String>,

    /// Override the Flux base URL ending in `/v1` (else config / default).
    #[arg(long)]
    pub base_url: Option<String>,
}

/// Production entry point — resolves credentials from the global config.
pub async fn run(args: ImageArgs) -> Result<()> {
    let config_path = wcore_config::config::global_config_path();
    run_with_config_path(args, &config_path).await
}

/// Test-friendly entry: resolve credentials against an explicit config path.
pub async fn run_with_config_path(args: ImageArgs, config_path: &Path) -> Result<()> {
    if args.prompt.trim().is_empty() {
        bail!("image: --prompt must be non-empty");
    }
    if args.out.is_none() && args.n > 1 {
        bail!("image: --out is required when --n > 1 (cannot write multiple images to stdout)");
    }

    let doc = load_doc(config_path)?;
    let api_key = resolve_key(&args.api_key, &doc).context(
        "no Flux API key (set --api-key, $FLUX_API_KEY, or [providers.flux-router] in config)",
    )?;
    let base_url = resolve_base_url(&args.base_url, &doc);

    let request = ImageRequest::new(&args.prompt)
        .with_model(args.model.as_deref())
        .with_n(args.n)
        .with_size(args.size.clone())
        .with_max_price(args.max_price);

    let client = FluxImageClient::new(&api_key, &base_url);
    let response = match client.generate(&request).await {
        Ok(r) => r,
        Err(e) => bail!("{}", format_provider_error(&e)),
    };

    if response.data.is_empty() {
        bail!("image: Flux returned no images");
    }

    // Surface the SynthID watermark notice (Gemini arms) on stderr so it does
    // not contaminate piped image bytes on stdout.
    if let Some(notice) = response.synthid_notice() {
        eprintln!("note: {notice}");
    }

    for index in 0..response.data.len() {
        let bytes = response
            .image_bytes(index)
            .with_context(|| format!("decoding image {}", index + 1))?;
        match &args.out {
            Some(path) => {
                let target = numbered_path(path, index, response.data.len());
                std::fs::write(&target, &bytes)
                    .with_context(|| format!("writing image to {}", target.display()))?;
                eprintln!("wrote {} ({} bytes)", target.display(), bytes.len());
            }
            None => {
                // Single image (n==1 guaranteed by the guard above) → stdout.
                std::io::stdout()
                    .write_all(&bytes)
                    .context("writing image to stdout")?;
            }
        }
    }

    Ok(())
}

/// Map a [`ProviderError`] to a user-facing message, keeping the typed
/// entitlement distinction (feature lock vs account state) intact.
fn format_provider_error(e: &ProviderError) -> String {
    match e {
        ProviderError::PremiumLocked { .. }
        | ProviderError::UpgradeRequired { .. }
        | ProviderError::SpendCeilingUnresolved { .. } => e.to_string(),
        ProviderError::Api { status, message } if *status == 403 => {
            // Contract §3.6: gpt-image arms require a verified OpenAI org.
            format!("image generation refused (HTTP 403): {message}")
        }
        other => format!("image generation failed: {other}"),
    }
}

/// `out.png`, index 0, total 1 → `out.png` (no suffix when a single image).
/// `out.png`, index 0, total 3 → `out-1.png`; index 1 → `out-2.png`.
fn numbered_path(base: &Path, index: usize, total: usize) -> PathBuf {
    if total <= 1 {
        return base.to_path_buf();
    }
    let stem = base.file_stem().map(|s| s.to_string_lossy().into_owned());
    let ext = base.extension().map(|s| s.to_string_lossy().into_owned());
    let n = index + 1;
    let file = match (stem, ext) {
        (Some(stem), Some(ext)) => format!("{stem}-{n}.{ext}"),
        (Some(stem), None) => format!("{stem}-{n}"),
        (None, Some(ext)) => format!("image-{n}.{ext}"),
        (None, None) => format!("image-{n}"),
    };
    match base.parent() {
        Some(parent) if !parent.as_os_str().is_empty() => parent.join(file),
        _ => PathBuf::from(file),
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
        // No flag, no env (env is process-global; the flag/config branches are
        // checked first so this is deterministic only when FLUX_API_KEY unset —
        // we assert the config value is returned when the flag is None).
        let key = resolve_key(&None, &doc);
        // If the test environment has FLUX_API_KEY set this would return that;
        // guard the assertion on the env being absent.
        if std::env::var("FLUX_API_KEY").is_err() {
            assert_eq!(key.unwrap(), "sk-config");
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
    fn resolve_key_errors_when_absent() {
        let doc = Table::new();
        if std::env::var("FLUX_API_KEY").is_err() {
            assert!(resolve_key(&None, &doc).is_err());
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

    #[test]
    fn numbered_path_single_image_has_no_suffix() {
        let p = numbered_path(Path::new("out.png"), 0, 1);
        assert_eq!(p, PathBuf::from("out.png"));
    }

    #[test]
    fn numbered_path_multi_image_inserts_index() {
        assert_eq!(
            numbered_path(Path::new("out.png"), 0, 3),
            PathBuf::from("out-1.png")
        );
        assert_eq!(
            numbered_path(Path::new("out.png"), 2, 3),
            PathBuf::from("out-3.png")
        );
    }

    #[test]
    fn numbered_path_preserves_parent_dir() {
        let p = numbered_path(Path::new("imgs/out.png"), 1, 2);
        assert_eq!(p, PathBuf::from("imgs/out-2.png"));
    }

    #[test]
    fn numbered_path_no_extension() {
        assert_eq!(
            numbered_path(Path::new("out"), 0, 2),
            PathBuf::from("out-1")
        );
    }

    #[tokio::test]
    async fn empty_prompt_is_rejected() {
        let args = ImageArgs {
            prompt: "   ".into(),
            model: None,
            n: 1,
            size: None,
            out: None,
            max_price: None,
            api_key: Some("k".into()),
            base_url: None,
        };
        let err = run_with_config_path(args, Path::new("/nonexistent/config.toml"))
            .await
            .unwrap_err();
        assert!(err.to_string().contains("--prompt"));
    }

    #[tokio::test]
    async fn multi_image_to_stdout_is_rejected() {
        let args = ImageArgs {
            prompt: "a cat".into(),
            model: None,
            n: 2,
            size: None,
            out: None,
            max_price: None,
            api_key: Some("k".into()),
            base_url: None,
        };
        let err = run_with_config_path(args, Path::new("/nonexistent/config.toml"))
            .await
            .unwrap_err();
        assert!(err.to_string().contains("--out is required"));
    }

    #[test]
    fn premium_locked_renders_paid_plan_message() {
        let e = ProviderError::PremiumLocked {
            capability: "image generation".into(),
            message: "image generation requires a paid plan".into(),
        };
        let msg = format_provider_error(&e);
        assert!(msg.contains("paid Flux plan"));
    }
}
