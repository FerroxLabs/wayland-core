//! CLI surface: `wayland-core auth` — provider API-key CRUD.
//!
//! Three flag-driven ops against the global `config.toml`'s
//! `[providers.<slug>]` tables:
//!
//!  * `auth list` — show every configured provider and a masked key.
//!  * `auth add <provider|autodetect> <key>` — validate the key against
//!    the provider's endpoint, then write `[providers.<slug>].api_key`.
//!  * `auth remove <provider>` — drop a `[providers.<slug>]` table.
//!
//! This is the lighter-weight sibling of the onboarding flow: it reuses
//! the SAME recognizer ([`crate::provider_keys`]) — `detect_provider`,
//! `validation_endpoint`, `validate_key_blocking` — so the prefix table
//! and per-provider endpoints never drift between the two surfaces.
//!
//! Unlike `engine_bridge::write_onboarding_config` (which renders a fresh
//! config and clobbers), `auth` edits the existing TOML document
//! in-place: every other table (`[default]`, `[memory]`, …) is preserved
//! untouched, and only the targeted `[providers.<slug>]` table is
//! added / changed / removed.

use anyhow::{Context, Result, anyhow, bail};
use clap::Subcommand;
use toml::value::Table;

use crate::provider_keys::{
    Detected, Provider, ValidationOutcome, detect_provider, validate_key_blocking,
};

use wcore_agent::oauth::chatgpt;
use wcore_agent::oauth::{OAuthStorage, OAuthTokens};

#[derive(Subcommand, Debug)]
pub enum AuthCmd {
    /// List every configured provider with a masked API key.
    List,

    /// Add (or replace) a provider API key. The key is validated against
    /// the provider's endpoint before it is written.
    ///
    /// `provider` is either a known provider slug (`anthropic`, `openai`,
    /// …) or the literal `autodetect` — in which case the provider is
    /// inferred from the key's prefix.
    Add {
        /// Provider slug, or `autodetect` to infer it from the key.
        provider: String,
        /// The API key to validate and store.
        key: String,
        /// Skip the live validation request and store the key anyway.
        #[arg(long)]
        no_validate: bool,
    },

    /// Remove a provider's API key from the config.
    Remove {
        /// Provider slug to remove (`anthropic`, `openai`, …).
        provider: String,
    },

    /// Sign in to a subscription provider via OAuth in the browser.
    ///
    /// Currently only `chatgpt` (aliases: `openai-chatgpt`) is wired: it
    /// runs the loopback PKCE flow against OpenAI's Codex client and stores
    /// the tokens encrypted under `~/.wayland/oauth/chatgpt.json`.
    Login {
        /// Subscription provider to sign in to (`chatgpt`).
        provider: String,
        /// Skip the browser flow and import an existing Codex CLI login from
        /// `$CODEX_HOME/auth.json` (default `~/.codex/auth.json`) instead.
        #[arg(long)]
        import_codex: bool,
        /// Use the headless device-code flow (no browser, no loopback): print
        /// a URL + code to enter on any device. Best for remote/SSH sessions.
        #[arg(long)]
        device: bool,
    },

    /// Sign out (delete stored OAuth tokens) for a subscription provider.
    Logout {
        /// Subscription provider to sign out of (`chatgpt`).
        provider: String,
    },

    /// Show OAuth login status (provider, plan, token expiry).
    Status,
}

/// Production entry point — operates on the global `config.toml`.
///
/// Async because the OAuth verbs (`login`/`logout`/`status`) run network
/// round-trips and MUST be awaited on the existing `#[tokio::main]` runtime
/// — spinning a nested `Runtime::new().block_on(..)` here panics (revision
/// B). The API-key CRUD verbs delegate to the synchronous [`run_with_path`].
pub async fn run(cmd: AuthCmd) -> Result<()> {
    match cmd {
        AuthCmd::Login {
            provider,
            import_codex,
            device,
        } => login_cmd(&provider, import_codex, device).await,
        AuthCmd::Logout { provider } => logout_cmd(&provider).await,
        AuthCmd::Status => status_cmd().await,
        // API-key CRUD is synchronous and file-only.
        other => {
            let path = wcore_config::config::global_config_path();
            run_with_path(other, &path)
        }
    }
}

/// Test-friendly entry point for the synchronous API-key CRUD verbs —
/// accepts an explicit config path so unit tests drive the same CRUD against
/// a tempdir-backed file. The OAuth verbs are handled by [`run`] (they need
/// the async runtime + the home-rooted token store), so routing one here is
/// a programmer error.
pub fn run_with_path(cmd: AuthCmd, config_path: &std::path::Path) -> Result<()> {
    match cmd {
        AuthCmd::List => list_cmd(config_path),
        AuthCmd::Add {
            provider,
            key,
            no_validate,
        } => add_cmd(&provider, &key, no_validate, config_path),
        AuthCmd::Remove { provider } => remove_cmd(&provider, config_path),
        AuthCmd::Login { .. } | AuthCmd::Logout { .. } | AuthCmd::Status => {
            bail!("OAuth verbs (login/logout/status) must be dispatched through the async `run`")
        }
    }
}

/// Load the config TOML document. A missing file yields an empty
/// document (so `auth add` works as a first-run path); a present but
/// malformed file is a hard error.
///
/// The body is deserialized straight into a `toml::Table` — the
/// document-level parse. (`toml::Value`'s `FromStr` is the *bare-value*
/// parser and rejects a `[section]` header, so it must not be used to
/// read a whole config file.)
fn load_doc(config_path: &std::path::Path) -> Result<Table> {
    if !config_path.exists() {
        return Ok(Table::new());
    }
    let body = std::fs::read_to_string(config_path)
        .with_context(|| format!("reading config at {}", config_path.display()))?;
    toml::from_str::<Table>(&body)
        .with_context(|| format!("parsing config at {}", config_path.display()))
}

/// Serialize `doc` back to `config_path`, creating the parent directory
/// if needed and tightening the file to `0o600` so the keys it holds are
/// never world-readable.
fn save_doc(doc: &Table, config_path: &std::path::Path) -> Result<()> {
    if let Some(parent) = config_path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("creating config dir {}", parent.display()))?;
    }
    let rendered = toml::to_string_pretty(&toml::Value::Table(doc.clone()))
        .context("serializing config TOML")?;
    std::fs::write(config_path, rendered)
        .with_context(|| format!("writing config to {}", config_path.display()))?;
    // SECURITY: enforce 0o600 — the config holds plaintext API keys.
    wcore_config::credentials::secure_credential_file(config_path)
        .with_context(|| format!("securing {}", config_path.display()))?;
    Ok(())
}

/// Borrow the `[providers]` sub-table from `doc`, if present.
fn providers_table(doc: &Table) -> Option<&Table> {
    doc.get("providers").and_then(toml::Value::as_table)
}

/// Open the credential ladder that sits beside `config_path`.
///
/// Derived from `config_path` rather than from `credentials_storage_path()` for
/// two reasons, and both matter. In production they are the same path —
/// `credentials_storage_path()` is literally `app_config_dir().join(
/// "credentials.toml")`, i.e. the sibling of `config.toml`. In a test that
/// drives [`run_with_path`] against a tempdir they are NOT: an ambient store
/// path would make `cargo test` write into the developer's real credentials
/// store. Deriving it keeps the CRUD verbs scoped to whatever config they were
/// pointed at.
///
/// `[storage.credentials]` is read out of the same document, so `auth add`
/// honours the operator's configured backend — including an explicit
/// `backend = "plaintext"` opt-in.
fn credentials_store(
    config_path: &std::path::Path,
    doc: &Table,
) -> Result<Box<dyn wcore_config::credentials::CredentialsStore>> {
    let storage: wcore_config::credentials::CredentialsStorageConfig = doc
        .get("storage")
        .and_then(toml::Value::as_table)
        .and_then(|s| s.get("credentials"))
        .cloned()
        .map(toml::Value::try_into)
        .transpose()
        .context("parsing [storage.credentials]")?
        .unwrap_or_default();
    let store_path = config_path.with_file_name("credentials.toml");
    wcore_config::credentials::open_store(&storage, &store_path)
        .with_context(|| format!("opening the credentials store at {}", store_path.display()))
}

/// The `providers.<slug>.api_key` store slot for a CLI [`Provider`].
///
/// Every provider the `auth` recognizer knows is a bearer-key provider, so this
/// is total in practice; the `Option` mirrors
/// [`wcore_config::config::credentials_store_key`], which returns `None` for the
/// out-of-band providers (Bedrock/Vertex/ChatGPT) that `auth add` cannot reach.
fn store_slot(provider: Provider) -> Option<String> {
    let provider_type = wcore_config::config::provider_type_from_slug(provider.slug())?;
    wcore_config::config::credentials_store_key(provider_type)
}

/// Read a provider's LEGACY cleartext key out of `[providers.<slug>].api_key`.
///
/// This sink predates the credentials store and still OUTRANKS it in
/// [`resolve_api_key`](wcore_config::config) (cli → config → store → env), so a
/// value here shadows anything `auth add` writes into the ladder. `auth add`
/// therefore removes it after a successful secure write; `auth list` reports it
/// so an operator can see what is still in cleartext.
fn legacy_config_key(doc: &Table, slug: &str) -> Option<String> {
    providers_table(doc)?
        .get(slug)?
        .as_table()?
        .get("api_key")?
        .as_str()
        .map(str::to_string)
}

/// Drop `[providers.<slug>].api_key` from `doc`. Returns whether anything was
/// removed. The rest of the provider table (`base_url`, `model`, …) is
/// preserved — only the secret leaves.
fn strip_legacy_config_key(doc: &mut Table, slug: &str) -> bool {
    let Some(providers) = doc.get_mut("providers").and_then(toml::Value::as_table_mut) else {
        return false;
    };
    let Some(table) = providers.get_mut(slug).and_then(toml::Value::as_table_mut) else {
        return false;
    };
    let removed = table.remove("api_key").is_some();
    if table.is_empty() {
        providers.remove(slug);
    }
    removed
}

/// Get-or-insert the `[providers]` sub-table as mutable.
fn providers_table_mut(doc: &mut Table) -> Result<&mut Table> {
    let entry = doc
        .entry("providers".to_string())
        .or_insert_with(|| toml::Value::Table(Table::new()));
    entry
        .as_table_mut()
        .context("`providers` in config is not a table")
}

/// Mask an API key for display — first 4 and last 4 characters, the
/// middle replaced by a fixed run of bullets. Short keys are fully
/// masked so a tiny key never half-leaks.
fn mask_key(key: &str) -> String {
    let key = key.trim();
    if key.len() <= 8 {
        return "•".repeat(key.len().max(4));
    }
    let head: String = key.chars().take(4).collect();
    let tail: String = key
        .chars()
        .rev()
        .take(4)
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect();
    format!("{head}••••••••{tail}")
}

/// Resolve the `provider` argument to a [`Provider`].
///
/// `autodetect` runs the key through the prefix recognizer; an ambiguous
/// or unrecognized key fails with a message telling the user to name the
/// provider explicitly. A non-`autodetect` argument must be a known slug.
fn resolve_provider(arg: &str, key: &str) -> Result<Provider> {
    if arg.eq_ignore_ascii_case("autodetect") {
        return match detect_provider(key) {
            Detected::One(p) => Ok(p),
            Detected::Ambiguous => bail!(
                "could not autodetect the provider — this key shape is shared by \
                 several providers. Re-run with an explicit provider, e.g. \
                 `wayland-core auth add openai <key>`"
            ),
            Detected::Unknown => bail!(
                "could not autodetect the provider from this key. Re-run with an \
                 explicit provider, e.g. `wayland-core auth add anthropic <key>`"
            ),
        };
    }
    Provider::from_slug(arg).ok_or_else(|| {
        let known: Vec<&str> = Provider::ALL.iter().map(|p| p.slug()).collect();
        anyhow::anyhow!(
            "unknown provider '{arg}'. Known providers: {}. \
             Or pass `autodetect` to infer it from the key.",
            known.join(", ")
        )
    })
}

/// List every provider that has a key, from BOTH locations.
///
/// Two locations, because there are two: the credential ladder (where `auth
/// add` now writes) and the legacy `[providers.<slug>].api_key` cleartext sink
/// (where it used to, and where an existing install's keys still are). Showing
/// only one would make the other invisible — and the invisible one is the
/// cleartext one, which is exactly backwards.
///
/// The WHERE column is the point of the change: a user has to be able to see
/// which of their keys are still sitting in cleartext.
fn list_cmd(config_path: &std::path::Path) -> Result<()> {
    let doc = load_doc(config_path)?;

    // Every slug that could hold a key, from either side.
    let mut slugs: Vec<String> = providers_table(&doc)
        .map(|providers| providers.keys().cloned().collect())
        .unwrap_or_default();
    for provider in Provider::ALL {
        if !slugs.iter().any(|s| s == provider.slug()) {
            slugs.push(provider.slug().to_string());
        }
    }
    slugs.sort();

    // A store that will not open is reported, not silently treated as empty —
    // "no providers configured" when the store is merely unreachable is the
    // shape that sends a user to re-enter a key they already have.
    let store = match credentials_store(config_path, &doc) {
        Ok(store) => Some(store),
        Err(error) => {
            eprintln!("warning: could not open the credentials store: {error:#}");
            None
        }
    };

    let mut rows: Vec<(String, String, &'static str)> = Vec::new();
    for slug in &slugs {
        let stored = store
            .as_ref()
            .zip(Provider::from_slug(slug).and_then(store_slot))
            .and_then(|(store, slot)| store.get(&slot).ok().flatten());
        if let Some(key) = stored {
            rows.push((slug.clone(), mask_key(&key), "credentials store"));
            continue;
        }
        if let Some(key) = legacy_config_key(&doc, slug) {
            rows.push((slug.clone(), mask_key(&key), "config.toml (CLEARTEXT)"));
        }
    }

    if rows.is_empty() {
        println!("No providers configured. Add one with `wayland-core auth add <provider> <key>`.");
        return Ok(());
    }
    println!("{:<14} {:<24} WHERE", "PROVIDER", "API KEY");
    let mut any_cleartext = false;
    for (slug, masked, where_) in rows {
        any_cleartext |= where_.contains("CLEARTEXT");
        println!("{slug:<14} {masked:<24} {where_}");
    }
    if any_cleartext {
        println!();
        println!(
            "One or more keys are stored UNENCRYPTED in {}. Re-run \
             `wayland-core auth add <provider> <key>` to move them into the credentials store.",
            config_path.display()
        );
    }
    Ok(())
}

fn add_cmd(
    provider_arg: &str,
    key: &str,
    no_validate: bool,
    config_path: &std::path::Path,
) -> Result<()> {
    let key = key.trim();
    if key.is_empty() {
        bail!("the API key is empty");
    }
    let provider = resolve_provider(provider_arg, key)?;

    if !no_validate {
        println!("Validating {} key…", provider.label());
        match validate_key_blocking(provider, key) {
            ValidationOutcome::Ok => println!("Key accepted by {}.", provider.label()),
            ValidationOutcome::Failed(reason) => bail!(
                "{} rejected the key: {reason}. \
                 Re-run with `--no-validate` to store it anyway.",
                provider.label()
            ),
        }
    }

    let mut doc = load_doc(config_path)?;
    let slug = provider.slug();
    let Some(slot) = store_slot(provider) else {
        bail!("{} authenticates out-of-band and has no API key slot", provider.label());
    };

    let store = credentials_store(config_path, &doc)?;
    let existed = store
        .get(&slot)
        .unwrap_or_default()
        .is_some()
        .then_some(true)
        .unwrap_or_else(|| legacy_config_key(&doc, slug).is_some());

    // The ladder, not the config file. `[providers.<slug>].api_key` is a
    // CLEARTEXT sink and it was this command's only destination: `auth add` was
    // the documented way to hand us a key and it wrote it, unencrypted, into
    // config.toml. A refusal here is the ladder failing closed and it is
    // surfaced verbatim — there is no cleartext branch to take instead.
    store
        .put(&slot, key)
        .with_context(|| format!("storing the {} API key", provider.label()))?;

    // NON-VACUITY, at runtime: never report a write we cannot read back.
    match store.get(&slot) {
        Ok(Some(read_back)) if read_back == key => {}
        _ => bail!(
            "the {} API key was accepted by the credentials store but did not read back; \
             refusing to report a save that did not happen",
            provider.label()
        ),
    }

    // Only now remove the legacy cleartext copy. It OUTRANKS the store in
    // `resolve_api_key`, so leaving it would mean the key just stored is
    // shadowed by the old one — a silent no-op from the user's point of view.
    if strip_legacy_config_key(&mut doc, slug) {
        save_doc(&doc, config_path)?;
        println!(
            "Removed the cleartext copy of this key from {} — it is now held only by the \
             credentials store.",
            config_path.display()
        );
    }

    if existed {
        println!("Updated API key for {} ({slug}).", provider.label());
    } else {
        println!("Added API key for {} ({slug}).", provider.label());
    }
    Ok(())
}

fn remove_cmd(provider_arg: &str, config_path: &std::path::Path) -> Result<()> {
    // `remove` never autodetects — it takes an explicit slug.
    let provider = Provider::from_slug(provider_arg).ok_or_else(|| {
        let known: Vec<&str> = Provider::ALL.iter().map(|p| p.slug()).collect();
        anyhow::anyhow!(
            "unknown provider '{provider_arg}'. Known providers: {}",
            known.join(", ")
        )
    })?;
    let slug = provider.slug();

    // BOTH locations. A remove that clears only one leaves the key resolvable
    // from the other, and the one it would leave behind is the cleartext one.
    let mut doc = load_doc(config_path)?;
    let removed_config = providers_table_mut(&mut doc)?.remove(slug).is_some();

    let removed_store = match (credentials_store(config_path, &doc), store_slot(provider)) {
        (Ok(store), Some(slot)) => {
            let had = store.get(&slot).unwrap_or_default().is_some();
            store
                .delete(&slot)
                .with_context(|| format!("removing the {} API key from the store", provider.label()))?;
            had
        }
        (Err(error), _) => {
            // Report rather than swallow: a user told "removed" while the key
            // is still in a store we could not open has been actively misled.
            eprintln!("warning: could not open the credentials store: {error:#}");
            false
        }
        (_, None) => false,
    };

    if !removed_config && !removed_store {
        bail!("no API key configured for {} ({slug})", provider.label());
    }
    if removed_config {
        save_doc(&doc, config_path)?;
    }
    println!("Removed API key for {} ({slug}).", provider.label());
    Ok(())
}

// ── OAuth verbs: login / logout / status (chatgpt) ────────────────────────

/// Normalize the `provider` argument for the OAuth verbs. Only ChatGPT is
/// wired today; `chatgpt` and `openai-chatgpt` both resolve to it.
fn resolve_oauth_provider(arg: &str) -> Result<&'static str> {
    match arg.trim().to_ascii_lowercase().as_str() {
        "chatgpt" | "openai-chatgpt" | "openai_chatgpt" => Ok(chatgpt::PROVIDER),
        other => bail!(
            "unknown OAuth provider '{other}'. The only wired subscription login is \
             `chatgpt` (alias `openai-chatgpt`)."
        ),
    }
}

/// `wayland-core auth login chatgpt [--import-codex] [--device]`.
///
/// Routing (first match wins):
/// - `--import-codex`: import an existing Codex CLI login
///   (`$CODEX_HOME/auth.json`) — no browser, no network.
/// - `--device`: the headless device-code flow (print a URL + code to enter
///   on any device) — no browser, no loopback. Best for remote/SSH.
/// - otherwise: the interactive loopback PKCE flow (opens a browser).
async fn login_cmd(provider_arg: &str, import_codex: bool, device: bool) -> Result<()> {
    resolve_oauth_provider(provider_arg)?;
    if import_codex {
        return import_codex_login();
    }
    if device {
        return login_chatgpt_device().await;
    }
    login_chatgpt().await
}

/// Import a ChatGPT login from the Codex CLI's `auth.json` and store it under
/// our own OAuth store. Shared by `--import-codex` and the auto-import
/// fallback in `status`/`login`. Returns the decoded plan for the success
/// line.
fn import_codex_login() -> Result<()> {
    let storage = OAuthStorage::from_home().map_err(|e| anyhow!("opening token store: {e}"))?;
    let tokens = chatgpt::import_codex_cli_tokens()
        .map_err(|e| anyhow!("importing Codex CLI login: {e}"))?;
    storage
        .store(chatgpt::PROVIDER, &tokens)
        .map_err(|e| anyhow!("persisting imported tokens: {e}"))?;
    let plan = chatgpt::decode_codex_claims(&tokens.access_token)
        .ok()
        .and_then(|c| c.plan_type)
        .unwrap_or_else(|| "unknown".to_string());
    println!(
        "Imported ChatGPT login from the Codex CLI (plan: {plan}). Use `--provider openai-chatgpt`."
    );
    Ok(())
}

/// `wayland-core auth logout chatgpt`.
///
/// C5: removing the on-disk token is not enough — also unlink any
/// `*.json.tmp` orphan left by an interrupted atomic write. A live
/// `ChatGptTokenManager` cache cannot be reached from this short-lived CLI
/// process (there is no live engine), so there is nothing in-memory to clear
/// here; the manager built at the next engine start re-reads the (now
/// missing) file. NotFound on the token file is treated as already-logged-out.
async fn logout_cmd(provider_arg: &str) -> Result<()> {
    let provider = resolve_oauth_provider(provider_arg)?;
    let storage = OAuthStorage::from_home().map_err(|e| anyhow!("opening token store: {e}"))?;
    let path = storage.path_for(provider);

    let removed = match std::fs::remove_file(&path) {
        Ok(()) => true,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => false,
        Err(e) => return Err(anyhow!("removing {}: {e}", path.display())),
    };

    // Unlink any orphaned temp file from an interrupted atomic write so a
    // stale half-written token cannot resurrect a logged-out session.
    let tmp = path.with_extension("json.tmp");
    if let Err(e) = std::fs::remove_file(&tmp)
        && e.kind() != std::io::ErrorKind::NotFound
    {
        // Best-effort: a leftover tmp that can't be removed is not fatal to a
        // logout whose real token file is gone.
        tracing::warn!(error = %e, path = %tmp.display(), "could not remove orphaned oauth tmp file");
    }

    if removed {
        println!("Signed out of ChatGPT. The stored OAuth token was removed.");
    } else {
        println!("Already signed out of ChatGPT (no stored token).");
    }
    Ok(())
}

/// `wayland-core auth status`.
///
/// Loads the stored ChatGPT token, decodes the access-token claims, and
/// prints signed-in + plan + expiry, or a not-signed-in line. When no wayland
/// token exists it tries a Codex CLI import once before reporting logged-out.
async fn status_cmd() -> Result<()> {
    let storage = OAuthStorage::from_home().map_err(|e| anyhow!("opening token store: {e}"))?;

    let tokens = match storage
        .load(chatgpt::PROVIDER)
        .map_err(|e| anyhow!("reading token store: {e}"))?
    {
        Some(t) => Some(t),
        None => {
            // Auto-try a Codex CLI import so a user who logged in via Codex
            // sees signed-in status without an explicit import step.
            match chatgpt::import_codex_cli_tokens() {
                Ok(t) => {
                    let _ = storage.store(chatgpt::PROVIDER, &t);
                    println!("(imported an existing ChatGPT login from the Codex CLI)");
                    Some(t)
                }
                Err(_) => None,
            }
        }
    };

    let Some(tokens) = tokens else {
        println!("ChatGPT: not signed in. Run `wayland-core auth login chatgpt`.");
        return Ok(());
    };

    print_status_line(&tokens);
    Ok(())
}

/// Render the signed-in status line from a token bundle. Split out so the
/// claim-decode + expiry formatting is unit-testable without a token store.
/// The plan/expiry decode is delegated to
/// [`chatgpt::ChatGptLoginStatus::from_tokens`] so this renderer and the
/// `/provider` precheck + `/config` status row all read the same source.
fn print_status_line(tokens: &OAuthTokens) {
    let status = chatgpt::ChatGptLoginStatus::from_tokens(tokens);
    let plan = status.plan.unwrap_or_else(|| "unknown".to_string());
    let expiry = match status.expires_at_unix_secs {
        Some(exp) => {
            let now = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_secs())
                .unwrap_or(0);
            if exp > now {
                let mins = (exp - now) / 60;
                format!("access token valid for ~{mins} min")
            } else {
                "access token expired (will refresh on next use)".to_string()
            }
        }
        None => "expiry unknown".to_string(),
    };
    println!("ChatGPT: signed in (plan: {plan}); {expiry}.");
}

/// Drive the interactive loopback PKCE round-trip and store the tokens.
/// Mirrors `tui::auth::run_google_meet_connect` but for the ChatGPT Codex
/// flow. Gated behind `remote-registry` because the token exchange uses
/// `wcore_egress::EgressClient` (the same gate the google-meet runner uses).
#[cfg(feature = "remote-registry")]
async fn login_chatgpt() -> Result<()> {
    use wcore_agent::oauth::PkceChallenge;

    let flow = chatgpt::build_chatgpt_flow();

    // 1. Bind the loopback listener (fixed Codex port 1455, dual-stack for
    //    the `localhost` redirect host) and derive the real redirect_uri.
    let (redirect_uri, listener) = flow.bind_callback_listener().await.map_err(|e| {
        anyhow!(
            "could not bind the local callback listener on port {}: {e}. \
             If another process holds the port, close it and retry.",
            chatgpt::CALLBACK_PORT
        )
    })?;

    // 2. Build the authorize URL against the bound redirect_uri.
    let (auth_url, state, pkce) = flow.build_authorize_url(&redirect_uri);

    // 3. Open the browser; a launch failure still leaves a copyable URL.
    let opened = open::that_detached(&auth_url).is_ok();
    if opened {
        println!("Opening your browser to sign in to ChatGPT…");
    } else {
        println!("Could not open a browser automatically. Open this URL to authorize:\n{auth_url}");
    }

    // 4. Wait for the redirect, validating the CSRF state inside.
    let code = flow.wait_for_code(listener, &state).await.map_err(|e| {
        if opened {
            anyhow!("ChatGPT authorization did not complete: {e}")
        } else {
            anyhow!(
                "ChatGPT authorization did not complete: {e}\n\nAuthorize manually:\n{auth_url}"
            )
        }
    })?;

    // 5. Exchange the code (+ PKCE verifier) for tokens.
    let client = wcore_egress::EgressClient::tool();
    let verifier = pkce.as_ref().map(|p: &PkceChallenge| p.verifier.as_str());
    let tokens = flow
        .exchange_code(&client, &code, &redirect_uri, verifier)
        .await
        .map_err(|e| anyhow!("ChatGPT token exchange failed: {e}"))?;

    // Hard-fail if the access token carries no ChatGPT account id — without
    // it the Codex backend rejects every request.
    chatgpt::decode_codex_claims(&tokens.access_token)
        .map_err(|e| anyhow!("ChatGPT login returned a token without an account id: {e}"))?;

    // 6. Persist the bundle to `~/.wayland/oauth/chatgpt.json`.
    let storage = OAuthStorage::from_home().map_err(|e| anyhow!("opening token store: {e}"))?;
    storage
        .store(chatgpt::PROVIDER, &tokens)
        .map_err(|e| anyhow!("persisting the tokens failed: {e}"))?;

    println!("Signed in to ChatGPT. Use `--provider openai-chatgpt`.");
    Ok(())
}

/// Stripped-build variant: with `remote-registry` (and `wcore-egress`)
/// compiled out, the token exchange cannot run. Point the user at the
/// network-backed build or the Codex import path.
#[cfg(not(feature = "remote-registry"))]
#[allow(clippy::unused_async)] // signature must match the remote-registry variant the caller awaits
async fn login_chatgpt() -> Result<()> {
    bail!(
        "ChatGPT login needs the network-backed build (the `remote-registry` feature); \
         this binary was built without it. If you have the Codex CLI installed, run \
         `wayland-core auth login chatgpt --import-codex` instead."
    )
}

/// Drive the headless device-code round-trip and store the tokens. No browser,
/// no loopback listener — the user opens the printed URL on any device and
/// types the printed code. Gated behind `remote-registry` like
/// [`login_chatgpt`] because the device flow uses `wcore_egress::EgressClient`.
#[cfg(feature = "remote-registry")]
async fn login_chatgpt_device() -> Result<()> {
    let client = wcore_egress::EgressClient::tool();

    // Runs steps 1-4 (request code, print, poll, exchange) and returns tokens.
    let tokens = chatgpt::login_device_code(&client)
        .await
        .map_err(|e| anyhow!("ChatGPT device-code sign-in failed: {e}"))?;

    // Hard-fail if the access token carries no ChatGPT account id — without it
    // the Codex backend rejects every request.
    chatgpt::decode_codex_claims(&tokens.access_token)
        .map_err(|e| anyhow!("ChatGPT login returned a token without an account id: {e}"))?;

    // Persist the bundle to `~/.wayland/oauth/chatgpt.json`.
    let storage = OAuthStorage::from_home().map_err(|e| anyhow!("opening token store: {e}"))?;
    storage
        .store(chatgpt::PROVIDER, &tokens)
        .map_err(|e| anyhow!("persisting the tokens failed: {e}"))?;

    println!("Signed in to ChatGPT. Use `--provider openai-chatgpt`.");
    Ok(())
}

/// Stripped-build variant: with `remote-registry` (and `wcore-egress`)
/// compiled out, the device-code exchange cannot run. Point the user at the
/// network-backed build or the Codex import path.
#[cfg(not(feature = "remote-registry"))]
#[allow(clippy::unused_async)] // signature must match the remote-registry variant the caller awaits
async fn login_chatgpt_device() -> Result<()> {
    bail!(
        "ChatGPT device-code login needs the network-backed build (the `remote-registry` \
         feature); this binary was built without it. If you have the Codex CLI installed, run \
         `wayland-core auth login chatgpt --import-codex` instead."
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::tempdir;

    /// Read the stored api_key for a slug back out of the CREDENTIALS STORE —
    /// the assertion seam for the write paths, and it moved.
    ///
    /// It used to read `[providers.<slug>].api_key` out of `config.toml`,
    /// because that is where `auth add` wrote it: in cleartext, outranking the
    /// credentials store in `resolve_api_key`. Asserting there now would assert
    /// the defect.
    fn stored_key(config_path: &std::path::Path, slug: &str) -> Option<String> {
        let doc = load_doc(config_path).expect("load config");
        let store = credentials_store(config_path, &doc).expect("open store");
        store.get(&store_slot(Provider::from_slug(slug)?)?).ok()?
    }

    /// Scope the ladder for a test: `WAYLAND_HOME` forces the isolated-profile
    /// path (so the OS keyring is never touched — a test that wrote into the
    /// developer's real Keychain would be a defect in itself), and a passphrase
    /// mounts the in-home encrypted vault so there IS a secure tier to write to.
    struct LadderEnv {
        saved: Vec<(&'static str, Option<std::ffi::OsString>)>,
    }

    impl LadderEnv {
        fn scoped(home: &std::path::Path) -> Self {
            let pairs: [(&'static str, Option<String>); 3] = [
                ("WAYLAND_HOME", Some(home.display().to_string())),
                ("WAYLAND_VAULT_PASSPHRASE", Some("auth-test-passphrase".into())),
                ("WAYLAND_VAULT_PASSPHRASE_FD", None),
            ];
            let saved = pairs
                .iter()
                .map(|(key, value)| {
                    let prior = std::env::var_os(key);
                    unsafe {
                        match value {
                            Some(value) => std::env::set_var(key, value),
                            None => std::env::remove_var(key),
                        }
                    }
                    (*key, prior)
                })
                .collect();
            Self { saved }
        }
    }

    impl Drop for LadderEnv {
        fn drop(&mut self) {
            for (key, prior) in &self.saved {
                unsafe {
                    match prior {
                        Some(value) => std::env::set_var(key, value),
                        None => std::env::remove_var(key),
                    }
                }
            }
        }
    }

    #[test]
    #[serial_test::serial(auth_credentials_env)]
    fn add_no_validate_stores_the_provider_key_and_never_writes_it_in_cleartext() {
        let dir = tempdir().unwrap();
        let _env = LadderEnv::scoped(dir.path());
        let path = dir.path().join("config.toml");
        run_with_path(
            AuthCmd::Add {
                provider: "anthropic".to_string(),
                key: "sk-ant-test-123".to_string(),
                no_validate: true,
            },
            &path,
        )
        .unwrap();
        // NON-VACUITY: readable back from the store it should have landed in.
        assert_eq!(
            stored_key(&path, "anthropic").as_deref(),
            Some("sk-ant-test-123")
        );
        // And nowhere in cleartext. `auth add` used to write exactly here.
        let config_body = fs::read_to_string(&path).unwrap_or_default();
        assert!(
            !config_body.contains("sk-ant-test-123"),
            "auth add wrote the key into config.toml in cleartext: {config_body}"
        );
        let cleartext_store = dir.path().join("credentials.toml");
        assert!(
            !cleartext_store.exists(),
            "auth add must not create a cleartext credentials file"
        );
        assert!(
            dir.path().join("credentials.enc").exists(),
            "the key must have landed in the encrypted vault"
        );
    }

    /// `auth add` MIGRATES a pre-existing cleartext key rather than leaving it
    /// to shadow the new one: `[providers.<slug>].api_key` outranks the store in
    /// `resolve_api_key`, so a stale cleartext copy would make the secure write
    /// a silent no-op from the user's point of view.
    #[test]
    #[serial_test::serial(auth_credentials_env)]
    fn add_strips_a_pre_existing_cleartext_key_that_would_shadow_the_store() {
        let dir = tempdir().unwrap();
        let _env = LadderEnv::scoped(dir.path());
        let path = dir.path().join("config.toml");
        fs::write(
            &path,
            "[providers.anthropic]\napi_key = \"sk-ant-OLD-cleartext\"\nbase_url = \"https://x\"\n",
        )
        .unwrap();

        run_with_path(
            AuthCmd::Add {
                provider: "anthropic".to_string(),
                key: "sk-ant-NEW".to_string(),
                no_validate: true,
            },
            &path,
        )
        .unwrap();

        assert_eq!(stored_key(&path, "anthropic").as_deref(), Some("sk-ant-NEW"));
        let body = fs::read_to_string(&path).unwrap();
        assert!(
            !body.contains("sk-ant-OLD-cleartext"),
            "the shadowing cleartext key must be removed: {body}"
        );
        assert!(
            body.contains("https://x"),
            "only the secret leaves — the rest of the provider table stays: {body}"
        );
    }

    /// Fail-closed reaches the CLI. With no keyring (isolated) and no vault
    /// passphrase there is no secure tier, and `auth add` must refuse with the
    /// actionable message rather than falling back to cleartext.
    #[test]
    #[serial_test::serial(auth_credentials_env)]
    fn add_refuses_rather_than_writing_cleartext_when_no_secure_tier_exists() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("config.toml");
        let saved: Vec<(&str, Option<std::ffi::OsString>)> =
            ["WAYLAND_HOME", "WAYLAND_VAULT_PASSPHRASE", "WAYLAND_VAULT_PASSPHRASE_FD"]
                .iter()
                .map(|key| {
                    let prior = std::env::var_os(key);
                    unsafe {
                        if *key == "WAYLAND_HOME" {
                            std::env::set_var(key, dir.path());
                        } else {
                            std::env::remove_var(key);
                        }
                    }
                    (*key, prior)
                })
                .collect();

        let err = run_with_path(
            AuthCmd::Add {
                provider: "anthropic".to_string(),
                key: "sk-ant-refused".to_string(),
                no_validate: true,
            },
            &path,
        )
        .unwrap_err();

        for (key, prior) in saved {
            unsafe {
                match prior {
                    Some(value) => std::env::set_var(key, value),
                    None => std::env::remove_var(key),
                }
            }
        }

        let rendered = format!("{err:#}");
        assert!(
            rendered.contains("WAYLAND_VAULT_PASSPHRASE"),
            "the refusal must be actionable: {rendered}"
        );
        assert!(
            !path.exists() || !fs::read_to_string(&path).unwrap().contains("sk-ant-refused"),
            "a refused add must not write the key into config.toml"
        );
        assert!(
            !dir.path().join("credentials.toml").exists(),
            "a refused add must not create a cleartext credentials file"
        );
    }

    #[test]
    #[serial_test::serial(auth_credentials_env)]
    fn autodetect_resolves_provider_from_key_prefix() {
        let dir = tempdir().unwrap();
        let _env = LadderEnv::scoped(dir.path());
        let path = dir.path().join("config.toml");
        run_with_path(
            AuthCmd::Add {
                provider: "autodetect".to_string(),
                key: "sk-or-v1-routerkey".to_string(),
                no_validate: true,
            },
            &path,
        )
        .unwrap();
        // `sk-or-v1-` is OpenRouter — never OpenAI.
        assert_eq!(
            stored_key(&path, "openrouter").as_deref(),
            Some("sk-or-v1-routerkey")
        );
        assert!(stored_key(&path, "openai").is_none());
    }

    #[test]
    fn autodetect_rejects_an_ambiguous_key() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("config.toml");
        let err = run_with_path(
            AuthCmd::Add {
                provider: "autodetect".to_string(),
                key: "sk-plainbarekey".to_string(),
                no_validate: true,
            },
            &path,
        )
        .unwrap_err();
        assert!(
            err.to_string().contains("could not autodetect"),
            "expected an autodetect failure, got: {err}"
        );
    }

    #[test]
    fn add_rejects_an_unknown_provider_slug() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("config.toml");
        let err = run_with_path(
            AuthCmd::Add {
                provider: "not-a-provider".to_string(),
                key: "whatever".to_string(),
                no_validate: true,
            },
            &path,
        )
        .unwrap_err();
        assert!(err.to_string().contains("unknown provider"), "got: {err}");
    }

    #[test]
    #[serial_test::serial(auth_credentials_env)]
    fn add_replaces_an_existing_key_in_place() {
        let dir = tempdir().unwrap();
        let _env = LadderEnv::scoped(dir.path());
        let path = dir.path().join("config.toml");
        let add = |key: &str| {
            run_with_path(
                AuthCmd::Add {
                    provider: "openai".to_string(),
                    key: key.to_string(),
                    no_validate: true,
                },
                &path,
            )
            .unwrap();
        };
        add("sk-proj-first");
        add("sk-proj-second");
        assert_eq!(
            stored_key(&path, "openai").as_deref(),
            Some("sk-proj-second")
        );
    }

    #[test]
    #[serial_test::serial(auth_credentials_env)]
    fn add_preserves_other_config_tables() {
        let dir = tempdir().unwrap();
        let _env = LadderEnv::scoped(dir.path());
        let path = dir.path().join("config.toml");
        // Seed a config with an unrelated table and a default section.
        fs::write(
            &path,
            "[default]\nprovider = \"anthropic\"\nuser = \"Sean\"\n\n[memory]\nenabled = true\n",
        )
        .unwrap();
        run_with_path(
            AuthCmd::Add {
                provider: "groq".to_string(),
                key: "gsk_testkey".to_string(),
                no_validate: true,
            },
            &path,
        )
        .unwrap();
        let doc = load_doc(&path).unwrap();
        // The new provider landed.
        assert_eq!(stored_key(&path, "groq").as_deref(), Some("gsk_testkey"));
        // The pre-existing tables survived untouched.
        let default = doc.get("default").and_then(toml::Value::as_table).unwrap();
        assert_eq!(
            default.get("user").and_then(toml::Value::as_str),
            Some("Sean")
        );
        let memory = doc.get("memory").and_then(toml::Value::as_table).unwrap();
        assert_eq!(
            memory.get("enabled").and_then(toml::Value::as_bool),
            Some(true)
        );
    }

    #[test]
    #[serial_test::serial(auth_credentials_env)]
    fn remove_clears_the_key_from_both_the_store_and_the_legacy_config_table() {
        let dir = tempdir().unwrap();
        let _env = LadderEnv::scoped(dir.path());
        let path = dir.path().join("config.toml");
        run_with_path(
            AuthCmd::Add {
                provider: "xai".to_string(),
                key: "xai-testkey".to_string(),
                no_validate: true,
            },
            &path,
        )
        .unwrap();
        assert!(stored_key(&path, "xai").is_some());

        // Reintroduce a legacy cleartext copy by hand, so the removal has to
        // clear BOTH locations. A remove that cleared only the store would
        // leave the cleartext one resolvable — and outranking.
        let mut doc = load_doc(&path).unwrap();
        providers_table_mut(&mut doc)
            .unwrap()
            .entry("xai".to_string())
            .or_insert_with(|| toml::Value::Table(Table::new()))
            .as_table_mut()
            .unwrap()
            .insert(
                "api_key".to_string(),
                toml::Value::String("xai-legacy".into()),
            );
        save_doc(&doc, &path).unwrap();

        run_with_path(
            AuthCmd::Remove {
                provider: "xai".to_string(),
            },
            &path,
        )
        .unwrap();
        assert!(stored_key(&path, "xai").is_none(), "store copy must be gone");
        assert!(
            legacy_config_key(&load_doc(&path).unwrap(), "xai").is_none(),
            "legacy cleartext copy must be gone too"
        );
    }

    #[test]
    fn remove_errors_when_the_provider_is_not_configured() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("config.toml");
        let err = run_with_path(
            AuthCmd::Remove {
                provider: "mistral".to_string(),
            },
            &path,
        )
        .unwrap_err();
        assert!(
            err.to_string().contains("no API key configured"),
            "got: {err}"
        );
    }

    #[test]
    fn list_on_a_missing_config_does_not_error() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("config.toml");
        run_with_path(AuthCmd::List, &path).unwrap();
    }

    #[test]
    fn mask_key_hides_the_middle_and_keeps_the_ends() {
        let masked = mask_key("sk-ant-api03-abcdefghijklmnop");
        assert!(masked.starts_with("sk-a"), "head not preserved: {masked}");
        assert!(masked.ends_with("mnop"), "tail not preserved: {masked}");
        assert!(masked.contains('•'), "key not masked: {masked}");
        assert!(
            !masked.contains("api03"),
            "key middle leaked into mask: {masked}"
        );
    }

    #[test]
    fn mask_key_fully_masks_a_short_key() {
        let masked = mask_key("sk-12");
        assert!(
            masked.chars().all(|c| c == '•'),
            "short key leaked: {masked}"
        );
    }

    // ── Task 5.2: OAuth verbs (login / logout / status) ──────────────

    #[test]
    fn resolve_oauth_provider_accepts_chatgpt_aliases() {
        assert_eq!(
            resolve_oauth_provider("chatgpt").unwrap(),
            chatgpt::PROVIDER
        );
        assert_eq!(
            resolve_oauth_provider("openai-chatgpt").unwrap(),
            chatgpt::PROVIDER
        );
        assert_eq!(
            resolve_oauth_provider("OpenAI-ChatGPT").unwrap(),
            chatgpt::PROVIDER
        );
    }

    #[test]
    fn resolve_oauth_provider_rejects_unknown() {
        let err = resolve_oauth_provider("anthropic").unwrap_err();
        assert!(err.to_string().contains("unknown OAuth provider"), "{err}");
    }

    /// The sync CRUD entry point must refuse the OAuth verbs — they require
    /// the async runtime + the home-rooted token store and are routed through
    /// the async `run`.
    #[test]
    fn run_with_path_refuses_oauth_verbs() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("config.toml");
        let err = run_with_path(AuthCmd::Status, &path).unwrap_err();
        assert!(err.to_string().contains("async"), "{err}");
        let err = run_with_path(
            AuthCmd::Logout {
                provider: "chatgpt".into(),
            },
            &path,
        )
        .unwrap_err();
        assert!(err.to_string().contains("async"), "{err}");
    }

    /// A 3-segment JWT whose payload carries the account id + plan, so the
    /// status line decode resolves a real plan.
    fn jwt_with_plan(account_id: &str, plan: &str) -> String {
        use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
        let payload = serde_json::json!({
            "https://api.openai.com/auth": {
                "chatgpt_account_id": account_id,
                "chatgpt_plan_type": plan,
            }
        });
        let seg = URL_SAFE_NO_PAD.encode(serde_json::to_vec(&payload).unwrap());
        format!("hdr.{seg}.sig")
    }

    /// `print_status_line` does not panic and the decoded plan + a future
    /// expiry are reflected. (It prints to stdout; we assert it runs cleanly
    /// over a well-formed token — the decode/expiry math is the logic under
    /// test.)
    #[test]
    fn print_status_line_handles_signed_in_token() {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);
        let tokens = OAuthTokens {
            access_token: jwt_with_plan("acct_s", "pro"),
            refresh_token: Some("rt".into()),
            expires_at_unix_secs: Some(now + 3600),
            token_type: "Bearer".into(),
            scope: None,
            id_token: None,
        };
        // The plan must be extractable from the access token.
        let plan = chatgpt::decode_codex_claims(&tokens.access_token)
            .unwrap()
            .plan_type;
        assert_eq!(plan.as_deref(), Some("pro"));
        // Smoke: rendering the line must not panic.
        print_status_line(&tokens);
    }

    /// Login with `--import-codex` round-trips a fake `$CODEX_HOME/auth.json`
    /// through the importer. We drive `chatgpt::import_codex_cli_tokens`
    /// directly (the CLI wrapper only adds the home-rooted store, which is not
    /// test-injectable) to prove the verb's import path is correctly wired to
    /// a real Codex auth shape.
    #[test]
    #[serial_test::serial]
    fn import_codex_verb_reads_codex_auth_json() {
        use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
        let dir = tempdir().unwrap();
        let home = dir.path().join("codex");
        std::fs::create_dir_all(&home).unwrap();
        let exp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0)
            + 3600;
        let payload = serde_json::json!({
            "exp": exp,
            "https://api.openai.com/auth": { "chatgpt_account_id": "acct_imp" }
        });
        let access = format!(
            "hdr.{}.sig",
            URL_SAFE_NO_PAD.encode(serde_json::to_vec(&payload).unwrap())
        );
        std::fs::write(
            home.join("auth.json"),
            serde_json::to_vec(&serde_json::json!({
                "tokens": { "access_token": access, "refresh_token": "rt-c" }
            }))
            .unwrap(),
        )
        .unwrap();

        let saved = std::env::var_os("CODEX_HOME");
        unsafe { std::env::set_var("CODEX_HOME", &home) };
        let result = chatgpt::import_codex_cli_tokens();
        match saved {
            Some(v) => unsafe { std::env::set_var("CODEX_HOME", v) },
            None => unsafe { std::env::remove_var("CODEX_HOME") },
        }

        let tokens = result.expect("import");
        assert_eq!(tokens.access_token, access);
        assert_eq!(tokens.refresh_token.as_deref(), Some("rt-c"));
    }
}
