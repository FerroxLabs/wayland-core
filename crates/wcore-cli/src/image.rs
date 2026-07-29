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
use wcore_tools::media_cost::{MediaCostRecord, MediaOutcome, MediaRateCard, MediaUnits};

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

    // F27-C3 — this subcommand is a SECOND billable generation surface,
    // separate from the `image_generate` tool, and until now it produced no
    // cost record at all. Measured live on 2026-07-29: one invocation wrote a
    // 249,886-byte JPEG from a real paid account and neither of its output
    // streams contained a single accounting token. `--n` multiplies that
    // silently.
    //
    // The backend identity is the endpoint host plus the model, so a record
    // from here is distinguishable from one produced by the tool path.
    let backend_id = format!("wayland-core image ({})", request.model);
    let rate_card = media_rate_card(&doc);
    let units = match parse_size(args.size.as_deref()) {
        Some((w, h)) => MediaUnits::images_at(args.n, w, h),
        // The provider chooses the size when `--size` is omitted and does not
        // report it back, so the dimensions are genuinely unknown here. The
        // COUNT is still known, and the count is still what gets billed.
        None => MediaUnits::images_of_unknown_size(args.n),
    };

    let client = FluxImageClient::new(&api_key, &base_url);
    let response = match client.generate(&request).await {
        Ok(r) => r,
        Err(e) => {
            // Not free. A refused or errored generation may still have been
            // charged, so it is recorded as billing-unknown rather than as $0
            // or as nothing at all.
            emit_accounting(MediaCostRecord::for_failure(
                "wayland-core image",
                &backend_id,
                &request.model,
                units,
                provider_error_category(&e),
            ));
            bail!("{}", format_provider_error(&e, &request.model, &base_url));
        }
    };

    if response.data.is_empty() {
        emit_accounting(
            MediaCostRecord::for_success(
                "wayland-core image",
                &backend_id,
                &request.model,
                units,
                None,
                &rate_card,
            )
            .with_outcome(MediaOutcome::Failed {
                category: "no_images_returned".to_string(),
            }),
        );
        bail!("image: Flux returned no images");
    }

    // Record the artifacts actually returned, not the number requested — a
    // provider that returns fewer than `--n` must not be recorded as having
    // produced `n`.
    let delivered = u32::try_from(response.data.len()).unwrap_or(u32::MAX);
    let billed_units = MediaUnits {
        images: delivered,
        ..units
    };
    emit_accounting(MediaCostRecord::for_success(
        "wayland-core image",
        &backend_id,
        &request.model,
        billed_units,
        // FluxImageClient discards the HTTP response, so no provider-reported
        // figure is reachable from here. Phase 27 measured that this endpoint
        // returns none in any channel anyway.
        None,
        &rate_card,
    ));

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

/// Print the cost record on stderr so it never contaminates piped image bytes
/// on stdout. Both a human line and the machine-readable JSON, because this
/// surface is scripted as often as it is read.
fn emit_accounting(record: MediaCostRecord) {
    eprintln!("accounting: {}", record.summary_line());
    eprintln!("accounting_json: {}", record.to_json());
}

/// Stable failure class, so a failure here is comparable with the same failure
/// through the `image_generate` tool.
fn provider_error_category(e: &ProviderError) -> &'static str {
    match e {
        ProviderError::PremiumLocked { .. } => "premium_locked",
        ProviderError::UpgradeRequired { .. } => "upgrade_required",
        ProviderError::SpendCeilingUnresolved { .. } => "spend_ceiling_unresolved",
        ProviderError::MissingApiKey => "no_provider_configured",
        ProviderError::Api { status, .. } if *status == 401 => "unauthorized_or_unknown_model",
        ProviderError::Api { status, .. } if *status == 402 => "insufficient_credits",
        ProviderError::Api { status, .. } if *status == 403 => "forbidden",
        _ => "other",
    }
}

/// Parse a `WIDTHxHEIGHT` size string. Returns `None` for absent or
/// unparseable input rather than guessing.
fn parse_size(size: Option<&str>) -> Option<(u32, u32)> {
    let raw = size?;
    let (w, h) = raw.trim().split_once(['x', 'X'])?;
    Some((w.trim().parse().ok()?, h.trim().parse().ok()?))
}

/// Operator rate card from `[tools.media_pricing]` in the global config.
/// Absent or malformed means "price nothing", never "price zero".
fn media_rate_card(doc: &Table) -> MediaRateCard {
    let entries = doc
        .get("tools")
        .and_then(|t| t.as_table())
        .and_then(|t| t.get("media_pricing"))
        .and_then(|t| t.as_table())
        .map(|t| {
            t.iter()
                .filter_map(|(k, v)| {
                    Some((
                        k.clone(),
                        v.as_float().or_else(|| v.as_integer().map(|i| i as f64))?,
                    ))
                })
                .collect()
        })
        .unwrap_or_default();
    MediaRateCard::new(entries)
}

/// Map a [`ProviderError`] to a user-facing message, keeping the typed
/// entitlement distinction (feature lock vs account state) intact.
///
/// `model` and `base_url` are the ones the request actually used, so the 401
/// arm can name the arm that failed and point at a command that resolves the
/// ambiguity.
fn format_provider_error(e: &ProviderError, model: &str, base_url: &str) -> String {
    match e {
        ProviderError::PremiumLocked { .. }
        | ProviderError::UpgradeRequired { .. }
        | ProviderError::SpendCeilingUnresolved { .. } => e.to_string(),
        ProviderError::Api { status, message } if *status == 403 => {
            // Contract §3.6: gpt-image arms require a verified OpenAI org.
            format!("image generation refused (HTTP 403): {message}")
        }
        ProviderError::Api { status, .. } if *status == 401 => {
            // MEASURED (phase 27 lane 27-fixes): this route returns a
            // byte-identical `{"error":{"message":"unauthorized"}}` for THREE
            // distinct causes — an invalid key, a model id that does not exist,
            // and a model the key is not entitled to. Rendering it as a
            // credential verdict sends the user to rotate a key that is fine.
            // The catalogue is key-scoped, so listing it is what actually
            // resolves the ambiguity.
            format!(
                "image generation was rejected with HTTP 401 for model `{model}`.\n\
                 This provider returns an identical 401 for BOTH an invalid API key AND a \
                 model that is unknown or not enabled for your plan, so this status alone \
                 does not tell you which — do not assume your key is bad.\n\
                 Resolve it by listing the models your key can actually use:\n    \
                 curl -sS -H \"Authorization: Bearer $FLUX_API_KEY\" {base_url}/models\n\
                 If `{model}` is absent from that list, pick one that is present:\n    \
                 wayland-core image --model <id> --prompt ...",
            )
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
        let msg = format_provider_error(&e, "flux-image", "https://api.fluxrouter.ai/v1");
        assert!(msg.contains("paid Flux plan"));
    }

    /// MEASURED live (lane 27-fixes): the image route answers with a
    /// byte-identical `{"error":{"message":"unauthorized"}}` HTTP 401 for an
    /// invalid key, for a nonexistent model id, and for the shipped default
    /// arm. So a 401 must never be rendered as a credential verdict.
    #[test]
    fn api_401_does_not_blame_the_credential_and_names_the_model() {
        let e = ProviderError::Api {
            status: 401,
            message: r#"{"error":{"message":"unauthorized"}}"#.into(),
        };
        let msg = format_provider_error(
            &e,
            "flux-image-together-flux",
            "https://api.fluxrouter.ai/v1",
        );

        // 1. Names the arm that actually failed — without it the user cannot
        //    tell which of --model / the default is at fault.
        assert!(
            msg.contains("flux-image-together-flux"),
            "401 message must name the model it used; got: {msg}"
        );
        // 2. Presents BOTH indistinguishable causes rather than asserting one.
        assert!(
            msg.contains("invalid API key") && msg.contains("not enabled for your plan"),
            "401 message must name both causes; got: {msg}"
        );
        // 3. The regression guard: the OLD message was the bare `Display` of
        //    the error, i.e. it surfaced the raw upstream "unauthorized" and
        //    nothing else. Assert we no longer hand the user that verdict bare,
        //    and that we explicitly tell them not to assume the key is bad.
        let old_message = format!("image generation failed: {e}");
        assert_ne!(
            msg, old_message,
            "401 must no longer fall through to the generic arm"
        );
        assert!(
            msg.contains("do not assume your key is bad"),
            "401 message must actively counter the 'unauthorized' reading; got: {msg}"
        );
        // 4. Gives the user the one command that resolves the ambiguity.
        assert!(
            msg.contains("/models"),
            "401 message must point at the key-scoped model catalogue; got: {msg}"
        );
    }
}
