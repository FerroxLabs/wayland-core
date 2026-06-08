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

use anyhow::{Context, Result, bail};
use clap::Subcommand;
use toml::value::Table;

use crate::provider_keys::{
    Detected, Provider, ValidationOutcome, detect_provider, validate_key_blocking,
};

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
}

/// Production entry point — operates on the global `config.toml`.
pub fn run(cmd: AuthCmd) -> Result<()> {
    let path = wcore_config::config::global_config_path();
    run_with_path(cmd, &path)
}

/// Test-friendly entry point — accepts an explicit config path so unit
/// tests drive the same CRUD against a tempdir-backed file.
pub fn run_with_path(cmd: AuthCmd, config_path: &std::path::Path) -> Result<()> {
    match cmd {
        AuthCmd::List => list_cmd(config_path),
        AuthCmd::Add {
            provider,
            key,
            no_validate,
        } => add_cmd(&provider, &key, no_validate, config_path),
        AuthCmd::Remove { provider } => remove_cmd(&provider, config_path),
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

fn list_cmd(config_path: &std::path::Path) -> Result<()> {
    let doc = load_doc(config_path)?;
    let Some(providers) = providers_table(&doc) else {
        println!("No providers configured. Add one with `wayland-core auth add <provider> <key>`.");
        return Ok(());
    };
    if providers.is_empty() {
        println!("No providers configured. Add one with `wayland-core auth add <provider> <key>`.");
        return Ok(());
    }
    // Sort by slug for stable output.
    let mut rows: Vec<(&String, String)> = providers
        .iter()
        .map(|(slug, tbl)| {
            let key = tbl
                .as_table()
                .and_then(|t| t.get("api_key"))
                .and_then(toml::Value::as_str)
                .map(mask_key)
                .unwrap_or_else(|| "(no api_key set)".to_string());
            (slug, key)
        })
        .collect();
    rows.sort_by(|a, b| a.0.cmp(b.0));
    println!("{:<14} API KEY", "PROVIDER");
    for (slug, masked) in rows {
        println!("{slug:<14} {masked}");
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
    let existed = providers_table(&doc).and_then(|p| p.get(slug)).is_some();
    {
        let providers = providers_table_mut(&mut doc)?;
        let entry = providers
            .entry(slug.to_string())
            .or_insert_with(|| toml::Value::Table(Table::new()));
        let tbl = entry
            .as_table_mut()
            .with_context(|| format!("`providers.{slug}` in config is not a table"))?;
        tbl.insert("api_key".to_string(), toml::Value::String(key.to_string()));
    }
    save_doc(&doc, config_path)?;
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

    let mut doc = load_doc(config_path)?;
    let removed = providers_table_mut(&mut doc)?.remove(slug).is_some();
    if !removed {
        bail!("no API key configured for {} ({slug})", provider.label());
    }
    save_doc(&doc, config_path)?;
    println!("Removed API key for {} ({slug}).", provider.label());
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::tempdir;

    /// Read the stored api_key for a slug straight out of the config
    /// file — the assertion seam for the write paths.
    fn stored_key(config_path: &std::path::Path, slug: &str) -> Option<String> {
        let doc = load_doc(config_path).expect("load config");
        providers_table(&doc)?
            .get(slug)?
            .as_table()?
            .get("api_key")?
            .as_str()
            .map(|s| s.to_string())
    }

    #[test]
    fn add_no_validate_writes_the_provider_key() {
        let dir = tempdir().unwrap();
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
        assert_eq!(
            stored_key(&path, "anthropic").as_deref(),
            Some("sk-ant-test-123")
        );
    }

    #[test]
    fn autodetect_resolves_provider_from_key_prefix() {
        let dir = tempdir().unwrap();
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
    fn add_replaces_an_existing_key_in_place() {
        let dir = tempdir().unwrap();
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
    fn add_preserves_other_config_tables() {
        let dir = tempdir().unwrap();
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
    fn remove_drops_the_provider_table() {
        let dir = tempdir().unwrap();
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
        run_with_path(
            AuthCmd::Remove {
                provider: "xai".to_string(),
            },
            &path,
        )
        .unwrap();
        assert!(stored_key(&path, "xai").is_none());
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
}
