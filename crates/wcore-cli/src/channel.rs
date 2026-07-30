//! `wayland-core channel` — the operator verb surface over `wcore-channels`.
//!
//! Phase 24, plan 24-03, Success Criterion 3. Four verbs — `list`, `probe`,
//! `health`, `reload` — and the important thing about them is WHERE each one
//! gets its answer, because the three verbs answer questions with three
//! different truth sources and mixing them up is how a status surface starts
//! lying.
//!
//! | verb | source of truth | needs a running gateway |
//! |---|---|---|
//! | `list` | the on-disk config directory | no |
//! | `probe` | the PLATFORM, asked live | no |
//! | `health` | the RUNNING gateway's observations | **yes** |
//! | `reload` | a request the running gateway acts on | **yes** |
//!
//! # `health` refuses rather than reporting a comfortable nothing
//!
//! Health is an OBSERVATION: it is what the poll loops in a running gateway
//! have actually seen. A separate `channel health` process holds no such
//! observations. It would be trivial to make this verb construct its own
//! `ChannelManager`, start it, and print the result — and that output would be
//! a fabrication, describing adapters that have existed for eight
//! milliseconds rather than the ones carrying traffic.
//!
//! So `health` reads the file the running gateway republishes, and when no
//! gateway is running it FAILS with a message saying nothing has been
//! observed. It never prints an empty list, and it never prints "healthy".
//!
//! This is deliberate closure of a shape this program keeps measuring:
//! F24-C-M2 (`gateway status` reporting `deliveries_pending: 0` while nine
//! messages were already at the destination), F24-B-H3 before it, and the
//! Windows orphan scanner in Phase 25. **A zero from a surface that was not
//! looking is not a zero.**
//!
//! # `probe` deliberately does NOT need a gateway
//!
//! The opposite reasoning applies. A probe's answer comes from the platform,
//! not from local state, so an operator must be able to run it while the
//! gateway is DOWN — that is precisely when they are debugging a credential.
//! It constructs adapters, asks, and reports; it never starts a poll loop and
//! never sends a message.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::{Context, Result, bail};
use clap::{Args, Subcommand};

// Reached through the registry rather than a direct dependency edge: adding
// `wcore-channels` to this crate's manifest rewrites `Cargo.lock`, a Phase-24
// shared seam concurrent lanes conflict on.
use wcore_channels::ChannelManager;
use wcore_channels::health::ChannelHealth;
use wcore_channels::probe::ProbeReport;
use wcore_channels_registry::wcore_channels;

/// The file the RUNNING gateway republishes its observed channel health into.
///
/// Separate from `gateway-status.json` on purpose: the gateway projection is
/// about the runtime, this is about the adapters, and a reader that wants one
/// should not have to parse and ignore the other.
pub const CHANNEL_HEALTH_FILE: &str = "channel-health.json";

/// The file `channel reload` creates and `gateway run` consumes.
///
/// A file rather than a signal, matching `gateway.drain`: both Unix families
/// already spend SIGTERM and SIGHUP on other meanings, Windows has neither,
/// and a reload that was indistinguishable from a stop would be worse than no
/// reload at all.
pub const CHANNEL_RELOAD_FILE: &str = "channel.reload";

#[derive(Debug, Clone, Args)]
pub struct ChannelArgs {
    #[command(subcommand)]
    pub cmd: ChannelCmd,
}

#[derive(Debug, Clone, Subcommand)]
pub enum ChannelCmd {
    /// List configured channels from the on-disk config directory.
    ///
    /// Read-only: never constructs an adapter, never touches the network, and
    /// never reads a credential. A channel whose config will not parse is
    /// listed WITH its parse error rather than omitted — an absent row and a
    /// broken row look identical otherwise.
    List {
        #[arg(long)]
        json: bool,
    },
    /// Ask each configured channel's platform whether its setup is complete,
    /// whether its credential authenticates, and what identity it
    /// authenticated as. Sends no message.
    ///
    /// Exits non-zero when any probed channel is not ready, so this verb is
    /// usable as a gate rather than only as a report.
    Probe {
        /// Probe only this channel. Default: every configured channel.
        #[arg(long)]
        name: Option<String>,
        #[arg(long)]
        json: bool,
    },
    /// Show the per-adapter health a RUNNING gateway has observed.
    ///
    /// Fails when no gateway is running. See the module docs: a health
    /// surface that answers when it has observed nothing is the false-zero
    /// shape this phase has measured three times.
    Health {
        #[arg(long)]
        json: bool,
    },
    /// Ask the running gateway to re-read the channel config directory.
    ///
    /// Adapters whose configuration did not change keep their running
    /// instance; changed ones are replaced and deconfigured ones are stopped.
    Reload,
    /// Print the native-action matrix: for each configured channel, whether
    /// edit / delete / react / typing are implemented, permanently absent from
    /// the platform, or simply not built yet.
    ///
    /// Read-only and offline in the same sense as `list`: adapters are
    /// CONSTRUCTED so their own declaration can be asked for, but never
    /// started, and no network call is made. Unlike `probe` this does not
    /// contact the platform at all.
    ///
    /// Exits non-zero when `--require` names an operation some channel cannot
    /// perform, so an operator can gate a deployment on "every channel I have
    /// configured can actually delete".
    Actions {
        /// Report only this channel. Default: every configured channel.
        #[arg(long)]
        name: Option<String>,
        /// Fail unless EVERY reported channel implements this operation.
        /// One of `edit`, `delete`, `react`, `typing`. Repeatable.
        #[arg(long, value_name = "OP")]
        require: Vec<String>,
        #[arg(long)]
        json: bool,
    },
    /// Store, list and remove the credentials a channel config refers to.
    #[command(subcommand)]
    Credential(CredentialCmd),
}

/// `wayland-core channel credential …` — the write side of the credential
/// handle.
///
/// # Why this exists
///
/// A channel config never holds a secret; it holds a *handle*, which is a
/// lookup key into the `CredentialsStore`. Until this verb there was no way
/// to put anything under one: the whole `wcore-cli` tree contained a single
/// `CredentialsStore::put` call and it was a provider OAuth token. `auth` is
/// not that path either — it writes `[providers.<slug>].api_key` into
/// `config.toml`, not into the store. So the documented deployment model
/// ("credentials are handles resolved from the credentials store") had no
/// reachable implementation and the only working route was hand-writing the
/// store file, which no document describes.
///
/// # Why the value is read from stdin and never taken as an argument
///
/// `auth add <provider> <key>` takes the secret in `argv`, where it lands in
/// shell history and is world-readable in `ps` for the life of the process.
/// That is the existing idiom and it is the one part of it not worth copying.
/// There is deliberately no `--value` flag: the only way in is stdin.
#[derive(Debug, Clone, Subcommand)]
pub enum CredentialCmd {
    /// Store a credential under `handle`, reading the value from stdin.
    ///
    /// The value is whatever arrives on stdin with one trailing newline
    /// stripped, so both `printf %s "$tok" | … set h` and
    /// `echo "$tok" | … set h` store the same bytes. An empty stdin is
    /// rejected rather than stored: a channel whose handle resolves to ""
    /// fails later, further from the cause.
    Set {
        /// The credentials-store key, e.g. `slack.acme.bot_token`. Get the
        /// exact spelling from `channel credential list`.
        handle: String,
    },
    /// List every handle the on-disk channel configs refer to and whether
    /// the store currently has a value for it.
    ///
    /// Prints handles and presence only — never a value. Exits non-zero when
    /// any handle is missing, so it is usable as a pre-flight gate and not
    /// only as a report.
    List {
        #[arg(long)]
        json: bool,
    },
    /// Delete the value stored under `handle`.
    Remove {
        /// The credentials-store key to delete.
        handle: String,
    },
}

/// Resolve the home whose `channels/` directory and gateway files we act on.
/// Routed through `wcore_gateway::resolve_home` so this surface and the
/// gateway agree on which directory they are talking about.
fn home() -> Result<PathBuf> {
    wcore_gateway::resolve_home().context("cannot resolve WAYLAND_HOME or $HOME")
}

fn channels_dir(home: &Path) -> PathBuf {
    home.join("channels")
}

pub fn channel_health_path(home: &Path) -> PathBuf {
    home.join(CHANNEL_HEALTH_FILE)
}

pub fn channel_reload_path(home: &Path) -> PathBuf {
    home.join(CHANNEL_RELOAD_FILE)
}

/// What a running gateway publishes about its channel adapters.
///
/// # Why this is not just `Vec<ChannelHealth>` — F24-D-H2, found live
///
/// It was, and the first live run on real hardware printed:
///
/// ```text
/// gateway is running and has registered no channels
/// ```
///
/// while TWO channels were configured on disk. The gateway had failed to open
/// the credentials store, registered nothing, and published `[]`. An empty
/// array cannot distinguish "you have no channels" from "I could not load the
/// ones you have", and the message it produced asserted the first.
///
/// That is the same false zero this file's module docs are about — F24-C-M2,
/// F24-B-H3, the Windows orphan scanner — reintroduced by the code written to
/// close it. So the document now carries THREE numbers instead of one list,
/// and two of them come from different places:
///
/// - `configured` is counted by scanning the config DIRECTORY,
/// - `registered` is what the gateway actually constructed,
/// - `registration_error` is why they differ, when they do.
///
/// A reader that sees `configured: 2, registered: 0` cannot mistake it for an
/// empty installation, and `channel health` exits non-zero on the disagreement
/// rather than printing a reassuring line.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ChannelHealthReport {
    /// Channels found in the config directory — counted independently of
    /// whatever the gateway managed to construct.
    pub configured: usize,
    /// Adapters the gateway actually registered.
    pub registered: usize,
    /// Why registration produced fewer adapters than are configured.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub registration_error: Option<String>,
    /// Per-adapter observations.
    #[serde(default)]
    pub channels: Vec<ChannelHealth>,
}

impl ChannelHealthReport {
    /// Whether every configured channel is actually registered and running.
    pub fn is_complete(&self) -> bool {
        self.registration_error.is_none() && self.registered >= self.configured
    }
}

/// Publish observed channel health for a second process to read.
///
/// Written to a same-directory temporary and renamed, so a `channel health`
/// racing a republish reads either the previous set or the next one and never
/// a half-written file. Same discipline as the gateway projection.
pub fn publish_health(home: &Path, report: &ChannelHealthReport) -> Result<()> {
    let final_path = channel_health_path(home);
    let tmp = final_path.with_extension("json.tmp");
    std::fs::write(&tmp, serde_json::to_vec_pretty(report)?)
        .with_context(|| format!("cannot write {}", tmp.display()))?;
    std::fs::rename(&tmp, &final_path)
        .with_context(|| format!("cannot publish {}", final_path.display()))?;
    Ok(())
}

/// Count the channels configured on disk. The INDEPENDENT number the
/// gateway's own registration count is checked against.
pub fn configured_count(home: &Path) -> usize {
    wcore_channels_registry::scan_channel_summaries(&channels_dir(home)).len()
}

/// Read the health a LIVE gateway published, or `None`.
///
/// Liveness is checked FIRST and the file second. A stale
/// `channel-health.json` left by a gateway that has since been killed
/// describes adapters that no longer exist, and reporting it would be exactly
/// the stale-projection failure `gateway status` already guards against.
pub fn read_live_health(home: &Path) -> Option<ChannelHealthReport> {
    let record = wcore_gateway::pidlock::PidLock::read_record(home)?;
    if !wcore_gateway::pidlock::process_is_alive(record.pid) {
        return None;
    }
    let raw = std::fs::read_to_string(channel_health_path(home)).ok()?;
    serde_json::from_str(&raw).ok()
}

/// Resolve configuration for the purpose of opening the CREDENTIALS STORE.
///
/// # F24-D-H1, found on the first live run
///
/// `Config::resolve` fails with [`wcore_config::config::MissingApiKey`] when no
/// LLM provider credential is present. That is entirely correct for a turn, and
/// entirely wrong here: opening the credentials store reads
/// `storage.credentials` and has nothing to do with any provider. Measured on
/// hetzner-dsm against a fresh home:
///
/// ```text
/// wayland-core channel: cannot resolve configuration: No API key found.
/// [gateway] channel reload: credentials store: No API key found.
/// ```
///
/// So `channel probe` was unusable on exactly the host an operator debugs a
/// fresh install on, and — worse — the gateway registered ZERO channels for
/// the same reason and reported it as an empty installation.
///
/// The fix retries resolution with a placeholder provider key ONLY when the
/// failure was specifically `MissingApiKey`. Every other configuration error
/// still propagates. The placeholder never reaches a provider: the only thing
/// taken from the resolved config is `open_credentials_store`, which reads the
/// operator's REAL `storage.credentials` backend. Falling back to a default
/// storage configuration instead would silently open the wrong store — finding
/// no credentials in it and reporting a perfectly configured channel as
/// incomplete.
pub fn resolve_config_for_credentials() -> Result<wcore_config::config::Config> {
    use wcore_config::config::{CliArgs, Config, MissingApiKey};

    match Config::resolve(&CliArgs::default()) {
        Ok(c) => Ok(c),
        Err(e) if e.downcast_ref::<MissingApiKey>().is_some() => Config::resolve(&CliArgs {
            api_key: Some("unused-placeholder-credentials-store-only".to_string()),
            ..CliArgs::default()
        })
        .context("cannot resolve configuration for the credentials store"),
        Err(e) => Err(e).context("cannot resolve configuration"),
    }
}

pub async fn run(args: ChannelArgs) -> Result<()> {
    match args.cmd {
        ChannelCmd::List { json } => list(json),
        ChannelCmd::Probe { name, json } => probe(name.as_deref(), json).await,
        ChannelCmd::Health { json } => health(json),
        ChannelCmd::Reload => reload(),
        ChannelCmd::Actions {
            name,
            require,
            json,
        } => actions(name.as_deref(), &require, json).await,
        ChannelCmd::Credential(cmd) => credential(cmd),
    }
}

// ---------------------------------------------------------------------------
// credential — the write side of the credential handle
// ---------------------------------------------------------------------------

/// Open the credentials store the adapters will read from at `start()`.
///
/// Routed through the same [`resolve_config_for_credentials`] the gateway and
/// `probe` use, so a credential written here is guaranteed to be looked for in
/// the same place. Resolving them independently is how a store gets written in
/// one location and read from another — the F24-D-H2 shape recorded above
/// `ChannelHealthReport`.
fn open_store() -> Result<Arc<dyn wcore_config::credentials::CredentialsStore>> {
    let config = resolve_config_for_credentials()?;
    let store = config
        .open_credentials_store()
        .context("cannot open the credentials store")?;
    Ok(Arc::from(store))
}

/// Every `(channel, option path, handle)` the on-disk configs refer to.
///
/// A config that will not parse contributes nothing and is reported by
/// `channel list`; this surface is about handles, so it stays quiet about
/// parse errors rather than duplicating that report.
fn configured_handles(dir: &Path) -> Vec<(String, String, String)> {
    let mut out = Vec::new();
    for s in wcore_channels_registry::scan_channel_summaries(dir) {
        for (path, handle) in s.credential_handles {
            out.push((s.name.clone(), path, handle));
        }
    }
    out
}

fn credential(cmd: CredentialCmd) -> Result<()> {
    match cmd {
        CredentialCmd::Set { handle } => credential_set(&handle),
        CredentialCmd::List { json } => credential_list(json),
        CredentialCmd::Remove { handle } => credential_remove(&handle),
    }
}

fn credential_set(handle: &str) -> Result<()> {
    use std::io::Read;

    if handle.trim().is_empty() {
        bail!("the credential handle must not be empty");
    }

    let mut value = String::new();
    std::io::stdin()
        .read_to_string(&mut value)
        .context("cannot read the credential value from stdin")?;
    // Strip exactly one trailing newline (and a CR before it) so a value
    // piped with `echo` matches one piped with `printf %s`. Anything more
    // would silently mangle a secret that legitimately ends in whitespace.
    if let Some(v) = value.strip_suffix('\n') {
        value = v.strip_suffix('\r').unwrap_or(v).to_string();
    }
    if value.is_empty() {
        bail!(
            "no credential value arrived on stdin — pipe the secret in, e.g. \
             `printf %s \"$TOKEN\" | wayland-core channel credential set {handle}`. \
             There is deliberately no --value flag: an argument would be visible \
             in shell history and in `ps`."
        );
    }

    let store = open_store()?;
    store
        .put(handle, &value)
        .map_err(|e| anyhow::anyhow!("cannot store the credential: {e}"))?;

    // The handle is echoed; the value never is, and its length is not
    // reported either — a length is a real hint about a secret.
    println!("stored credential under handle {handle:?}");
    Ok(())
}

fn credential_remove(handle: &str) -> Result<()> {
    let store = open_store()?;
    store
        .delete(handle)
        .map_err(|e| anyhow::anyhow!("cannot delete the credential: {e}"))?;
    println!("removed credential handle {handle:?}");
    Ok(())
}

fn credential_list(json: bool) -> Result<()> {
    let home = home()?;
    let dir = channels_dir(&home);
    let handles = configured_handles(&dir);
    let store = open_store()?;

    // Presence, never the value. A store error is NOT folded into "absent":
    // an unreadable store and an empty one are different problems and the
    // false zero of conflating them is this phase's most-measured defect.
    let mut rows = Vec::new();
    for (channel, option, handle) in &handles {
        let present = match store.get(handle) {
            Ok(Some(v)) => Some(!v.is_empty()),
            Ok(None) => Some(false),
            Err(_) => None,
        };
        rows.push((channel, option, handle, present));
    }
    let missing = rows.iter().filter(|r| r.3 != Some(true)).count();

    if json {
        let out: Vec<serde_json::Value> = rows
            .iter()
            .map(|(channel, option, handle, present)| {
                serde_json::json!({
                    "channel": channel,
                    "option": option,
                    "handle": handle,
                    "stored": present,
                })
            })
            .collect();
        println!("{}", serde_json::to_string_pretty(&out)?);
    } else if rows.is_empty() {
        println!(
            "no channel config in {} refers to a credential handle",
            dir.display()
        );
    } else {
        println!("credential handles referenced by {}:", dir.display());
        for (channel, option, handle, present) in &rows {
            let state = match present {
                Some(true) => "stored",
                Some(false) => "MISSING",
                None => "UNREADABLE",
            };
            println!("  {channel:<16} {option:<34} {handle:<32} {state}");
        }
        if missing > 0 {
            println!(
                "\n{missing} handle(s) have no value. Store each with:\n  \
                 printf %s \"$SECRET\" | wayland-core channel credential set <handle>"
            );
        }
    }

    if missing > 0 {
        std::process::exit(1);
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// actions — the native-action matrix
// ---------------------------------------------------------------------------

/// The four operations, in the order [`wcore_channels::NativeActions::entries`]
/// yields them, so the CLI table and the conformance matrix agree on order.
const ACTION_OPS: [&str; 4] = ["edit", "delete", "react", "typing"];

/// Print each configured channel's DECLARED native-action surface.
///
/// # Where the answer comes from, and why this verb is offline
///
/// Following the truth-source discipline in this module's docs: the answer is
/// a property of the ADAPTER TYPE, so it needs neither a running gateway (like
/// `health`) nor a platform round trip (like `probe`). Adapters are
/// constructed so their own declaration can be read, and nothing is started.
///
/// The distinction the table exists to draw is the one an operator otherwise
/// cannot make: `no (platform)` means the platform has no such API and no
/// amount of work closes it, while `NOT BUILT` is our backlog. Before this
/// verb, both surfaced identically — as an `Unsupported` error, and only if
/// you made the call.
async fn actions(only: Option<&str>, require: &[String], json: bool) -> Result<()> {
    for op in require {
        if !ACTION_OPS.contains(&op.as_str()) {
            bail!(
                "--require {op:?} is not an operation; expected one of {}",
                ACTION_OPS.join(", ")
            );
        }
    }

    let home = home()?;
    let dir = channels_dir(&home);

    let config = resolve_config_for_credentials()?;
    let store = config
        .open_credentials_store()
        .context("cannot open the credentials store")?;
    let creds: Arc<dyn wcore_config::credentials::CredentialsStore> = Arc::from(store);

    let mut mgr = ChannelManager::new();
    let registered = wcore_channels_registry::auto_register_from_dir(&mut mgr, &dir, creds)
        .await
        .with_context(|| format!("cannot load channels from {}", dir.display()))?;

    // Same refusal as `probe`. An empty table would read as "no channel can do
    // anything", which is a different claim from "no channel could be built" —
    // the false-zero shape this module's docs are about.
    if registered == 0 {
        bail!(
            "no channels could be constructed from {} — nothing to report",
            dir.display()
        );
    }

    let mut rows = mgr.native_action_matrix().await;
    if let Some(want) = only {
        rows.retain(|(n, _, _)| n == want);
        if rows.is_empty() {
            bail!("no configured channel named {want:?}");
        }
    }

    if json {
        let out: Vec<serde_json::Value> = rows
            .iter()
            .map(|(name, platform, a)| {
                serde_json::json!({
                    "name": name,
                    "platform": platform,
                    "edit": a.edit.as_str(),
                    "delete": a.delete.as_str(),
                    "react": a.react.as_str(),
                    "typing": a.typing.as_str(),
                    "note": a.note,
                })
            })
            .collect();
        println!("{}", serde_json::to_string_pretty(&out)?);
    } else {
        println!(
            "{:<18} {:<10} {:<14} {:<14} {:<14} {:<14}",
            "CHANNEL", "PLATFORM", "EDIT", "DELETE", "REACT", "TYPING"
        );
        for (name, platform, a) in &rows {
            let cell = |s: wcore_channels::ActionSupport| match s {
                wcore_channels::ActionSupport::Implemented => "yes",
                wcore_channels::ActionSupport::PlatformHasNoApi => "no (platform)",
                wcore_channels::ActionSupport::NotImplemented => "NOT BUILT",
            };
            println!(
                "{:<18} {:<10} {:<14} {:<14} {:<14} {:<14}",
                name,
                platform,
                cell(a.edit),
                cell(a.delete),
                cell(a.react),
                cell(a.typing)
            );
            if !a.note.is_empty() {
                println!("    note: {}", a.note);
            }
        }
    }

    // `--require` turns the report into a gate. Every unmet requirement is
    // listed, not just the first: an operator fixing one at a time and
    // re-running is how a five-minute check becomes five round trips.
    let mut unmet: Vec<String> = Vec::new();
    for op in require {
        for (name, platform, a) in &rows {
            let support = match op.as_str() {
                "edit" => a.edit,
                "delete" => a.delete,
                "react" => a.react,
                "typing" => a.typing,
                _ => unreachable!("validated above"),
            };
            if !support.is_implemented() {
                unmet.push(format!("{name} ({platform}) {op}: {}", support.as_str()));
            }
        }
    }
    if !unmet.is_empty() {
        for u in &unmet {
            eprintln!("REQUIRED BUT UNAVAILABLE: {u}");
        }
        bail!(
            "{} of the required operations are unavailable on the configured channels",
            unmet.len()
        );
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// list
// ---------------------------------------------------------------------------

fn list(json: bool) -> Result<()> {
    let home = home()?;
    let dir = channels_dir(&home);
    let summaries = wcore_channels_registry::scan_channel_summaries(&dir);

    if json {
        let rows: Vec<serde_json::Value> = summaries
            .iter()
            .map(|s| {
                serde_json::json!({
                    "name": s.name,
                    "platform": s.platform,
                    "enabled": s.enabled,
                    "known_platform": s.known_platform,
                    "option_keys": s.option_keys,
                    "credential_handles": s.credential_handles
                        .iter()
                        .map(|(path, handle)| serde_json::json!({
                            "option": path,
                            "handle": handle,
                        }))
                        .collect::<Vec<_>>(),
                    "parse_error": s.parse_error,
                })
            })
            .collect();
        println!("{}", serde_json::to_string_pretty(&rows)?);
        return Ok(());
    }

    if summaries.is_empty() {
        println!("no channels configured in {}", dir.display());
        return Ok(());
    }
    println!("channels in {}:", dir.display());
    for s in &summaries {
        let state = if !s.known_platform {
            "UNKNOWN PLATFORM"
        } else if s.enabled {
            "enabled"
        } else {
            "disabled"
        };
        println!("  {:<20} {:<12} {}", s.name, s.platform, state);
        if let Some(err) = &s.parse_error {
            println!("      config error: {err}");
        }
        // The handles a config expects are the other half of "why isn't this
        // working": the config can be perfect and the channel still dead
        // because nothing was ever stored under them. Printing them here is
        // what makes `channel credential set` discoverable from the verb an
        // operator already runs first.
        for (path, handle) in &s.credential_handles {
            println!("      credential: {path} -> {handle}");
        }
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// probe
// ---------------------------------------------------------------------------

async fn probe(only: Option<&str>, json: bool) -> Result<()> {
    let home = home()?;
    let dir = channels_dir(&home);

    let config = resolve_config_for_credentials()?;
    let store = config
        .open_credentials_store()
        .context("cannot open the credentials store")?;
    let creds: Arc<dyn wcore_config::credentials::CredentialsStore> = Arc::from(store);

    // Adapters are CONSTRUCTED but never started: `probe` takes `&self` and
    // asks the platform directly. Starting them would open gateways and poll
    // loops that this short-lived process is about to drop, which is traffic
    // on a production surface for no reason.
    let mut mgr = ChannelManager::new();
    let registered = wcore_channels_registry::auto_register_from_dir(&mut mgr, &dir, creds)
        .await
        .with_context(|| format!("cannot load channels from {}", dir.display()))?;

    if registered == 0 {
        bail!(
            "no channels could be constructed from {} — nothing to probe",
            dir.display()
        );
    }

    let reports: Vec<ProbeReport> = match only {
        Some(name) => vec![
            mgr.probe_one(name)
                .await
                .with_context(|| format!("cannot probe channel {name:?}"))?,
        ],
        None => mgr.probe_all().await,
    };

    if json {
        println!("{}", serde_json::to_string_pretty(&reports)?);
    } else {
        for r in &reports {
            println!("{} ({})", r.channel, r.platform);
            println!("  outcome:  {:?}", r.outcome);
            println!(
                "  config:   {}",
                if r.config_complete {
                    "complete"
                } else {
                    "INCOMPLETE"
                }
            );
            println!(
                "  auth:     {}",
                if r.authenticated {
                    "authenticated"
                } else {
                    "NOT authenticated"
                }
            );
            println!("  identity: {}", r.identity.as_deref().unwrap_or("-"));
            for f in &r.findings {
                println!("  finding:  {f}");
            }
        }
    }

    // Non-zero when anything is not ready, so this is a GATE and not just a
    // report. `Unsupported` counts as not ready — an adapter that declined to
    // check has not established that it is fine.
    let not_ready: Vec<&ProbeReport> = reports.iter().filter(|r| !r.outcome.is_ready()).collect();
    if !not_ready.is_empty() {
        let names: Vec<&str> = not_ready.iter().map(|r| r.channel.as_str()).collect();
        bail!(
            "{} of {} channels are not ready: {}",
            not_ready.len(),
            reports.len(),
            names.join(", ")
        );
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// health
// ---------------------------------------------------------------------------

fn health(json: bool) -> Result<()> {
    let home = home()?;
    let Some(report) = read_live_health(&home) else {
        // The refusal is the feature. See the module docs.
        bail!(
            "no running gateway for {} — channel health is an OBSERVATION and \
             nothing has observed anything. Start one with `wayland-core \
             gateway start`, or use `wayland-core channel probe`, which asks \
             the platform directly and needs no gateway.",
            home.display()
        );
    };

    if json {
        println!("{}", serde_json::to_string_pretty(&report)?);
    } else {
        println!(
            "configured: {}   registered: {}",
            report.configured, report.registered
        );
        for h in &report.channels {
            println!("{} ({})", h.channel, h.platform);
            println!("  state:      {:?}", h.state);
            println!("  reason:     {}", h.reason.as_deref().unwrap_or("-"));
            println!("  errors:     {}", h.consecutive_errors);
            println!("  reconnects: {}", h.reconnects);
        }
    }

    // F24-D-H2. An incomplete registration must not be reported as a healthy
    // empty installation. `configured` is counted from the config DIRECTORY and
    // `registered` from what the gateway built, so a disagreement between two
    // independently-sourced numbers is what trips this — not the gateway's own
    // opinion of itself.
    if !report.is_complete() {
        bail!(
            "{} of {} configured channels are NOT registered in the running \
             gateway{}",
            report.configured.saturating_sub(report.registered),
            report.configured,
            report
                .registration_error
                .as_ref()
                .map(|e| format!(": {e}"))
                .unwrap_or_default()
        );
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// reload
// ---------------------------------------------------------------------------

fn reload() -> Result<()> {
    let home = home()?;
    let record = wcore_gateway::pidlock::PidLock::read_record(&home);
    match record {
        Some(r) if wcore_gateway::pidlock::process_is_alive(r.pid) => {
            std::fs::write(channel_reload_path(&home), "1").with_context(|| {
                format!("cannot write {}", channel_reload_path(&home).display())
            })?;
            println!(
                "reload requested; gateway pid {} will re-read {}",
                r.pid,
                channels_dir(&home).display()
            );
            Ok(())
        }
        // Writing a request nobody will read, and reporting success for it, is
        // the same lie as an unobserved health report.
        _ => bail!(
            "no running gateway for {} — nothing would act on a reload request",
            home.display()
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use wcore_channels::health::HealthState;
    use wcore_channels::probe::ProbeOutcome;

    fn sample() -> ChannelHealthReport {
        ChannelHealthReport {
            configured: 1,
            registered: 1,
            registration_error: None,
            channels: vec![ChannelHealth {
                channel: "acme".into(),
                platform: "discord".into(),
                state: HealthState::Degraded,
                reason: Some("supervised reconnect in progress".into()),
                consecutive_errors: 5,
                reconnects: 2,
            }],
        }
    }

    #[test]
    fn published_health_round_trips_through_the_file() {
        let dir = tempfile::tempdir().unwrap();
        publish_health(dir.path(), &sample()).unwrap();
        let raw = std::fs::read_to_string(channel_health_path(dir.path())).unwrap();
        let back: ChannelHealthReport = serde_json::from_str(&raw).unwrap();
        assert_eq!(back, sample());
    }

    #[test]
    fn publishing_leaves_no_temporary_behind() {
        // A leftover `.json.tmp` is how a reader eventually finds a
        // half-written file and parses it as truth.
        let dir = tempfile::tempdir().unwrap();
        publish_health(dir.path(), &sample()).unwrap();
        let tmp = channel_health_path(dir.path()).with_extension("json.tmp");
        assert!(
            !tmp.exists(),
            "temporary {} survived the rename",
            tmp.display()
        );
    }

    #[test]
    fn health_is_not_reported_when_no_gateway_is_running() {
        // THE case this verb exists to get right. A published file with no
        // live process must produce NOTHING, not a stale all-clear.
        let dir = tempfile::tempdir().unwrap();
        publish_health(dir.path(), &sample()).unwrap();
        assert!(
            read_live_health(dir.path()).is_none(),
            "health was reported from a file with no live gateway behind it"
        );
    }

    #[test]
    fn health_is_not_reported_from_a_stale_pid_record() {
        // Positive control for the case above: the file IS parseable and the
        // record IS present, so the only thing withholding the report is the
        // liveness check. Without this, the previous test would also pass if
        // `read_live_health` were simply broken.
        let dir = tempfile::tempdir().unwrap();
        publish_health(dir.path(), &sample()).unwrap();
        // A pid that cannot be alive. `process_is_alive(0)` used to return
        // TRUE on Unix — see F24-D-M1 in `wcore_gateway::pidlock`.
        wcore_gateway::pidlock::PidLock::write_stale_record_for_test(dir.path(), 0);
        assert!(
            wcore_gateway::pidlock::PidLock::read_record(dir.path()).is_some(),
            "positive control: the record really is readable"
        );
        assert!(
            std::fs::read_to_string(channel_health_path(dir.path()))
                .is_ok_and(|s| s.contains("acme")),
            "positive control: the published file really is present and parseable"
        );
        assert!(
            read_live_health(dir.path()).is_none(),
            "a dead pid must withhold the report"
        );
    }

    #[test]
    fn health_reads_back_when_the_recorded_process_is_this_one() {
        // The mirror case: with a LIVE pid the report comes through. Without
        // it, `read_live_health` returning None unconditionally would pass
        // every test above.
        let dir = tempfile::tempdir().unwrap();
        publish_health(dir.path(), &sample()).unwrap();
        wcore_gateway::pidlock::PidLock::write_stale_record_for_test(
            dir.path(),
            std::process::id(),
        );
        assert_eq!(
            read_live_health(dir.path()),
            Some(sample()),
            "a live pid plus a published file must produce the report"
        );
    }

    #[test]
    fn a_registration_failure_is_not_reportable_as_an_empty_installation() {
        // F24-D-H2, found on real hardware. The gateway could not open the
        // credentials store, registered nothing, published an empty list, and
        // `channel health` rendered it as "you have no channels" — a false
        // zero produced by the code written to close false zeros.
        let failed = ChannelHealthReport {
            configured: 2,
            registered: 0,
            registration_error: Some("credentials store unavailable".into()),
            channels: Vec::new(),
        };
        assert!(
            !failed.is_complete(),
            "two configured and none registered must NOT read as complete"
        );

        // The error alone is not what trips it: a silent shortfall must too,
        // because a registration that drops one adapter without erroring is
        // the quieter version of the same bug.
        let silent_shortfall = ChannelHealthReport {
            configured: 2,
            registered: 1,
            registration_error: None,
            channels: Vec::new(),
        };
        assert!(!silent_shortfall.is_complete());

        // Positive control: a genuinely empty installation IS complete, so the
        // check above is not simply always-false.
        let genuinely_empty = ChannelHealthReport {
            configured: 0,
            registered: 0,
            registration_error: None,
            channels: Vec::new(),
        };
        assert!(
            genuinely_empty.is_complete(),
            "an operator with no channels configured is not in an error state"
        );
        assert!(sample().is_complete());
    }

    #[test]
    fn probe_readiness_gate_treats_unsupported_as_not_ready() {
        // The exit-status rule `probe` gates on. An adapter that implements no
        // probe must not satisfy a readiness gate.
        assert!(!ProbeReport::unsupported("c", "p").outcome.is_ready());
        assert!(ProbeOutcome::Ok.is_ready());
    }
}
