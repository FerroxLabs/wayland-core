//! `wayland-core gateway support-bundle` — the operator surface over
//! [`wcore_gateway::support_bundle`].
//!
//! # Why this file exists
//!
//! Phase 24 Success Criterion 4 promises that typed authenticated clients
//! *"recover event gaps **and produce useful redacted health/log/support
//! evidence**"*. The recovery half was proven on HTTP/SSE. The support half
//! was **unreachable from the shipped binary**: `wcore_gateway::support_bundle`
//! is 543 lines of collector with a redactor, a manifest, structural elision
//! and a mutation-gated test suite — and it had **zero production call sites
//! and no CLI verb** (`F24-C4-H1`, HIGH). An operator hitting a problem had no
//! way to produce evidence a support engineer could act on.
//!
//! This module is the missing surface and nothing more. All collection policy
//! lives in `wcore_gateway::support_bundle`; this is a caller, not a second
//! implementation.
//!
//! # The two things this surface must get right that the library cannot
//!
//! The library takes *paths* and a *redactor*. Choosing them badly produces a
//! bundle that is confidently wrong, and both traps are live here:
//!
//! 1. **The status file outlives the process that wrote it.** The running
//!    gateway republishes `gateway-status.json` every tick and nothing removes
//!    it when that process dies — which is exactly why
//!    [`super::read_live_projection`] checks `process_is_alive` FIRST and
//!    refuses to return a projection for a dead pid. A bundle that copied that
//!    file verbatim would ship a `Running` claim with a pid that is gone. A
//!    support bundle is created *precisely when the gateway has died*, so this
//!    is not an edge case — it is the modal case, and it would be the first
//!    file a support engineer opens. The status member here is therefore
//!    **derived through the liveness check**, and says so in its own body.
//!
//! 2. **The redactor must learn the config's secrets, not just the
//!    environment's.** `learn_from_environment()` is the only bulk learn the
//!    library had, so a credential living in `config.toml` was never learned,
//!    and a log line quoting that credential shipped it verbatim. This module
//!    also feeds the config and credentials files to
//!    `Redactor::learn_secret_values_from_file`, which was added for it.
//!
//! # Read it before you send it
//!
//! The bundle is a plain directory, not an archive, by the library's design:
//! the operator can read exactly what they are about to hand over. This
//! surface prints the path and the archive command rather than archiving on
//! the operator's behalf.

use std::path::PathBuf;

use anyhow::{Context, Result};

use wcore_gateway::lifecycle::{GatewayState, StatusProjection};
use wcore_gateway::support_bundle::{self, BundleSources, Redactor};

use super::{ScopeArgs, is_registered, read_live_projection, spec};

/// The gateway's own stdout/stderr sink, as written into every generated
/// service unit (`wcore_gateway::service`: `StandardOutPath {home}/gateway.log`).
const LOG_FILE: &str = "gateway.log";

pub async fn support_bundle(scope: &ScopeArgs, out: Option<PathBuf>, json: bool) -> Result<()> {
    let home = scope.home()?;
    let profile = scope.profile();

    let out_dir = match out {
        Some(p) => p,
        None => home.join(format!(
            "support-bundle-{}",
            chrono::Utc::now().format("%Y%m%dT%H%M%SZ")
        )),
    };

    // ---- The redactor is built BEFORE any source is read, so that no read
    // path can run against an unarmed scrubber.
    let config = wcore_config::config::global_config_path();
    let credentials = wcore_config::config::credentials_storage_path();
    let mut redactor = Redactor::new();
    redactor.learn_from_environment();
    redactor.learn_secret_values_from_file(&config);
    redactor.learn_secret_values_from_file(&credentials);

    // ---- Derived members are staged OUTSIDE `out_dir`, because `collect`
    // refuses a non-empty output directory — a refusal worth keeping, since it
    // is what stops a bundle shipping whatever was already sitting there.
    let stage = tempfile::tempdir().context("cannot create a staging directory")?;
    let mut projections: Vec<PathBuf> = Vec::new();

    // ---- Status, LIVENESS-CHECKED. See this module's docs, trap 1.
    let live = read_live_projection(&home);
    let running = live.is_some();
    let mut proj = live.unwrap_or_else(|| StatusProjection::stopped(&profile));
    if !running
        && let Ok(spec) = spec(scope)
        && !is_registered(&*wcore_gateway::service::for_this_platform(), &spec).await
    {
        // Distinguish "installed and down" from "never installed": an operator
        // whose service will not start needs the difference, and so does the
        // engineer reading the bundle.
        proj.state = GatewayState::Uninstalled;
    }
    let status_path = stage.path().join("gateway-status.json");
    std::fs::write(
        &status_path,
        serde_json::to_vec_pretty(&serde_json::json!({
            "liveness_checked": true,
            "running": running,
            "note": if running {
                "A live process was found: the pid in gateway.pid is alive and this \
                 projection was read from the file it is currently republishing."
            } else {
                "NO live process. This projection is synthesised, NOT read from \
                 the on-disk gateway-status.json — that file outlives the process \
                 that wrote it and would claim `Running` for a dead pid."
            },
            "projection": proj,
        }))
        .context("cannot serialise the status projection")?,
    )
    .context("cannot stage the status member")?;
    projections.push(status_path);

    // ---- Delivery ledger SUMMARY, not the raw journal: `deliveries.jsonl` is
    // unbounded and projections are copied whole, so the raw file would bloat
    // the bundle without telling a support engineer more than the counts do.
    //
    // Opened only if the journal already exists, because `DeliveryLedger::open`
    // creates `home` — and a diagnostic verb must not create the state it is
    // reporting on.
    let journal = wcore_gateway::ledger::DeliveryLedger::journal_path(&home);
    if journal.exists() {
        let summary = match wcore_gateway::ledger::DeliveryLedger::open(&home) {
            Ok(l) => serde_json::json!({
                "journal": journal.display().to_string(),
                "pending": l.pending_count(),
                "abandoned": l.abandoned_count(),
                "quarantined": l.quarantined(),
                "dropped_past_retention": l.dropped_abandonments(),
            }),
            // Never silent about its own incompleteness: a ledger that will not
            // open is itself a finding, and omitting the member entirely would
            // be indistinguishable from a gateway that never delivered.
            Err(e) => serde_json::json!({
                "journal": journal.display().to_string(),
                "error": format!("the delivery ledger exists but could not be read: {e}"),
            }),
        };
        let p = stage.path().join("ledger-summary.json");
        std::fs::write(
            &p,
            serde_json::to_vec_pretty(&summary).context("cannot serialise the ledger summary")?,
        )
        .context("cannot stage the ledger summary member")?;
        projections.push(p);
    }

    let sources = BundleSources {
        config: Some(config),
        credentials: Some(credentials),
        log: Some(home.join(LOG_FILE)),
        projections,
    };

    let manifest = support_bundle::collect(&home, &out_dir, &sources, &redactor)
        .with_context(|| format!("cannot write a support bundle into {}", out_dir.display()))?;

    if json {
        println!(
            "{}",
            serde_json::to_string_pretty(&serde_json::json!({
                "bundle": out_dir.display().to_string(),
                "manifest": manifest,
            }))?
        );
        return Ok(());
    }

    println!("support bundle written");
    println!("  path:             {}", out_dir.display());
    println!("  members:          {}", manifest.members.len() + 1);
    println!("  os / arch:        {} / {}", manifest.os, manifest.arch);
    println!(
        "  binary version:   {}",
        manifest.binary_version.as_deref().unwrap_or("-")
    );
    println!("  gateway running:  {}", if running { "yes" } else { "no" });
    println!("  known secrets:    {}", manifest.known_secrets);
    println!("  redactions made:  {}", manifest.redactions);

    if manifest.known_secrets == 0 {
        // The manifest records this for a reader; an operator deserves to be
        // told at the point of creation. A bundle whose scrubber knew nothing
        // has had structural elision applied and nothing else.
        println!();
        println!(
            "NOTE: the scrubber knew ZERO secret values, so the log member has had no \
             exact-secret redaction applied — only structural elision protected this \
             bundle. That is expected if no credential is set in the environment or in \
             {}.",
            wcore_config::config::global_config_path().display()
        );
    }

    if !manifest.absent_sources.is_empty() {
        println!();
        println!("  sources that were expected and absent:");
        for a in &manifest.absent_sources {
            println!("    - {a}");
        }
    }

    println!();
    println!("READ IT BEFORE YOU SEND IT. It is a plain directory:");
    println!("    ls -la {}", out_dir.display());
    println!("Archive it once you are satisfied:");
    println!(
        "    tar czf support-bundle.tar.gz -C {} .",
        out_dir.display()
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The verb must be reachable through clap, not merely present on the
    /// enum. Read off the built command, mirroring `verb_names()` in the
    /// parent module — a `#[command(skip)]` or a rename changes this answer.
    #[test]
    fn the_verb_is_reachable_through_clap() {
        use clap::Subcommand as _;
        let names: Vec<String> =
            super::super::GatewayCmd::augment_subcommands(clap::Command::new("gateway"))
                .get_subcommands()
                .map(|c| c.get_name().to_string())
                .collect();
        assert!(
            names.iter().any(|n| n == "support-bundle"),
            "F24-C4-H1: the support bundle must have an operator verb, got {names:?}"
        );
    }

    /// A bundle must be producible with NO gateway running and NO config on
    /// disk — the state an operator is actually in when they need one.
    #[tokio::test]
    async fn a_bundle_is_produced_against_a_dead_gateway_and_an_empty_home() {
        let home = tempfile::tempdir().unwrap();
        let out = home.path().join("bundle");
        let scope = ScopeArgs {
            profile: Some("default".into()),
            home: Some(home.path().to_path_buf()),
        };
        support_bundle(&scope, Some(out.clone()), false)
            .await
            .expect("a bundle must be producible with nothing running");

        let raw = std::fs::read_to_string(out.join("manifest.json")).unwrap();
        let manifest: support_bundle::BundleManifest = serde_json::from_str(&raw).unwrap();

        // Every declared member is really there.
        for m in &manifest.members {
            assert!(out.join(m).exists(), "declared member {m} is missing");
        }
        // The status member exists and does NOT claim the gateway is running.
        let status: serde_json::Value = serde_json::from_str(
            &std::fs::read_to_string(out.join("gateway-status.json")).unwrap(),
        )
        .unwrap();
        assert_eq!(status["running"], serde_json::json!(false));
        assert_eq!(status["liveness_checked"], serde_json::json!(true));
        // No ledger existed, so no ledger member was invented.
        assert!(!out.join("ledger-summary.json").exists());
    }

    /// The refusal that stops a bundle shipping somebody else's files must
    /// survive this surface, not just the library.
    #[tokio::test]
    async fn a_populated_output_directory_is_refused_through_the_verb() {
        let home = tempfile::tempdir().unwrap();
        let out = home.path().join("bundle");
        std::fs::create_dir_all(&out).unwrap();
        std::fs::write(out.join("someone-elses-private-file"), "private").unwrap();
        let scope = ScopeArgs {
            profile: Some("default".into()),
            home: Some(home.path().to_path_buf()),
        };
        let err = support_bundle(&scope, Some(out.clone()), false)
            .await
            .expect_err("a populated output directory must be refused");
        assert!(
            format!("{err:#}").contains("not empty"),
            "the refusal must name the reason: {err:#}"
        );
        // And it must not have clobbered what was already there.
        assert!(out.join("someone-elses-private-file").exists());
    }
}
