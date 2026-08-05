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
//! 3. **Which config it reads is not a free choice** (`F24-C4-M1`). See
//!    [`config_sources`]: `--home` is a `WAYLAND_HOME` override that is never
//!    exported, so resolving the config path from the process environment
//!    pointed the redactor at the wrong file.
//!
//! # And one thing the library cannot check for itself
//!
//! `collect` scrubs each member as it writes it and reports what it did. That
//! report is the library grading its own work. [`members_still_carrying_a_known_secret`]
//! re-reads the finished bundle and asks the opposite question over the bytes
//! that were actually written; on a hit this verb **refuses with a non-zero
//! exit** instead of printing a path. It is the only check here that can catch
//! a redaction defect in the library rather than inherit one.
//!
//! # Read it before you send it
//!
//! The bundle is a plain directory, not an archive, by the library's design:
//! the operator can read exactly what they are about to hand over. This
//! surface prints the path and the archive command rather than archiving on
//! the operator's behalf.
//!
//! # Reconciliation note
//!
//! Two lanes built this verb concurrently off different bases (`lane/
//! support-bundle` and `lane/24-c4-support`), neither able to see the other.
//! This file is the landed module plus the second lane's three additions: the
//! post-condition above, `config_sources`, and the `channel-health.json`
//! member. The second lane's own layout (an inline `fn` in `gateway.rs`), its
//! raw `deliveries.jsonl` projection and its required `--out` were all
//! DISCARDED in favour of what was already here — the child module, the ledger
//! summary and the timestamped default are better, and are the reason this is
//! a graft rather than a replacement.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};

use wcore_gateway::lifecycle::{GatewayState, StatusProjection};
use wcore_gateway::support_bundle::{self, BundleSources, Redactor};

use super::{ScopeArgs, is_registered, read_live_projection, spec};

/// The gateway's own stdout/stderr sink, as written into every generated
/// service unit (`wcore_gateway::service`: `StandardOutPath {home}/gateway.log`).
const LOG_FILE: &str = "gateway.log";

/// Which `config.toml` / `credentials.toml` this bundle reads.
///
/// **`F24-C4-M1`.** This was `wcore_config::config::global_config_path()`
/// unconditionally, and that is wrong whenever `--home` is used without the
/// matching environment variable. `wayland_config_dir()` resolves from the
/// PROCESS env `WAYLAND_HOME`, while [`ScopeArgs::home`] is a flag that is
/// never exported — so `gateway support-bundle --home /X` read the AMBIENT
/// config, not `/X`'s.
///
/// That is not a cosmetic mismatch, and it is why this is a fix rather than a
/// tidy-up: these two paths feed `Redactor::learn_secret_values_from_file`.
/// Reading the wrong pair means the scrubber learns secrets that are not in
/// play and **fails to learn the ones that are** — so a log line quoting
/// `/X`'s credential would have shipped verbatim, with the bundle reporting a
/// healthy non-zero `known_secrets` the whole time. A redactor armed with the
/// wrong secrets is more dangerous than an empty one, because the manifest no
/// longer flags it.
///
/// `--home` IS a `WAYLAND_HOME` override, so when it is given the config
/// directory is that home; otherwise resolution is deferred to the crate that
/// owns it.
fn config_sources(scope: &ScopeArgs, home: &Path) -> (PathBuf, PathBuf) {
    match &scope.home {
        Some(_) => (home.join("config.toml"), home.join("credentials.toml")),
        None => (
            wcore_config::config::global_config_path(),
            wcore_config::config::credentials_storage_path(),
        ),
    }
}

/// Every known secret, re-scanned across the FINISHED bundle.
///
/// The post-condition, and it is not decoration. `collect` scrubs each member
/// as it writes it; this asks the opposite question afterwards — *is any known
/// secret still present anywhere under the bundle root?* — over the bytes that
/// were actually written, including members no scrub path touched. It is the
/// only check in this surface that can catch a redaction defect in the library
/// itself rather than trusting the library's own report.
///
/// Reads bytes and converts lossily rather than `read_to_string`, because a
/// member that is not valid UTF-8 would otherwise be skipped silently — and a
/// secret that survived into a non-text member is still a leak.
fn members_still_carrying_a_known_secret(root: &Path, redactor: &Redactor) -> Vec<PathBuf> {
    support_bundle::bundle_files(root)
        .into_iter()
        .filter(|p| match std::fs::read(p) {
            Ok(bytes) => redactor.scrub(&String::from_utf8_lossy(&bytes)).1 > 0,
            Err(_) => false,
        })
        .collect()
}

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
    let (config, credentials) = config_sources(scope, &home);
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

    // ---- Channel health, copied verbatim after scrubbing when a gateway has
    // published it. Criterion 4 names "redacted **health**/log/support
    // evidence" and this file is the gateway's health publication, so a bundle
    // without it is missing a third of what the clause asks for. Pushed
    // unconditionally: `collect` NAMES an absent projection in
    // `absent_sources`, which is strictly better than omitting it, because a
    // member that was never collected and a gateway that published no health
    // are otherwise indistinguishable to the engineer reading the bundle.
    projections.push(crate::channel::channel_health_path(&home));

    let sources = BundleSources {
        config: Some(config),
        credentials: Some(credentials),
        log: Some(home.join(LOG_FILE)),
        projections,
    };

    let manifest = support_bundle::collect(&home, &out_dir, &sources, &redactor)
        .with_context(|| format!("cannot write a support bundle into {}", out_dir.display()))?;

    // ---- THE POST-CONDITION. Everything above trusts `collect` to have
    // scrubbed what it wrote; this is the only step that checks. A hit here is
    // a redaction defect in the library, and the right response is to refuse
    // rather than to print a path an operator will attach to a ticket.
    //
    // The directory is NOT deleted. Deleting it destroys the only evidence of
    // the defect, and a bundle sitting on the operator's own disk has crossed
    // no boundary — the hazard is sending it, and the refusal is what stops
    // that. The exit status is non-zero so a script cannot proceed either.
    let leaked = members_still_carrying_a_known_secret(&out_dir, &redactor);
    if !leaked.is_empty() {
        bail!(
            "REDACTION FAILURE — do NOT send this bundle. {} member(s) under {} \
             still contain a known secret value: {}. The directory has been left \
             in place deliberately, as evidence; delete it once reported.",
            leaked.len(),
            out_dir.display(),
            leaked
                .iter()
                .map(|p| p.display().to_string())
                .collect::<Vec<_>>()
                .join(", ")
        );
    }

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

    // -----------------------------------------------------------------------
    // Grafted from lane `24-c4-support`. Each of these was mutation-proved on
    // that lane and is re-proved on this reconciled code.
    // -----------------------------------------------------------------------

    #[test]
    fn the_leak_detector_finds_a_planted_secret_and_is_quiet_on_a_clean_tree() {
        // The post-condition is a NEGATIVE claim — "no member still carries a
        // known secret" — and a negative passes for free on a dead instrument.
        // Both directions are asserted, and the positive one is what makes the
        // negative worth anything.
        const PLANT: &str = "F24C4-LEAK-DETECTOR-PROBE-9a71fe30";

        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("b");
        std::fs::create_dir_all(root.join("nested")).unwrap();
        std::fs::write(root.join("clean.txt"), "nothing here").unwrap();

        let mut redactor = Redactor::new();
        assert!(redactor.learn(PLANT), "the probe must be learnable");

        // KNOWN-NEGATIVE first, so a detector that always reports a hit fails.
        assert!(
            members_still_carrying_a_known_secret(&root, &redactor).is_empty(),
            "a clean tree must produce no findings"
        );

        // KNOWN-POSITIVE, in a NON-UTF8 member: `read_to_string` would skip
        // this file silently and report the tree clean.
        let mut bytes = vec![0xff, 0xfe];
        bytes.extend_from_slice(PLANT.as_bytes());
        std::fs::write(root.join("nested/leaky.bin"), &bytes).unwrap();

        let found = members_still_carrying_a_known_secret(&root, &redactor);
        assert_eq!(
            found.len(),
            1,
            "the planted secret must be found: {found:?}"
        );
        assert!(
            found[0].ends_with("nested/leaky.bin"),
            "and the scan must recurse: {found:?}"
        );
    }

    #[test]
    fn an_explicit_home_redirects_the_config_sources_with_it() {
        // F24-C4-M1. `--home` is a WAYLAND_HOME override that is never
        // exported, so resolving the config path from the process environment
        // pointed the REDACTOR at the wrong file — it would learn secrets that
        // are not in play and miss the ones that are.
        let dir = tempfile::tempdir().unwrap();
        let home = dir.path().join("explicit-home");
        let scope = ScopeArgs {
            profile: None,
            home: Some(home.clone()),
        };
        let (config, credentials) = config_sources(&scope, &home);
        assert_eq!(config, home.join("config.toml"));
        assert_eq!(credentials, home.join("credentials.toml"));

        // With no `--home`, resolution is deferred to the crate that owns it
        // rather than reinvented here.
        let ambient = ScopeArgs {
            profile: None,
            home: None,
        };
        let (c2, _) = config_sources(&ambient, &home);
        assert_eq!(c2, wcore_config::config::global_config_path());
        assert_ne!(
            c2,
            home.join("config.toml"),
            "the two arms must be distinguishable, or this test cannot fail"
        );
    }

    #[tokio::test]
    async fn a_credential_in_the_named_home_is_learned_and_scrubbed_from_the_log() {
        // The end-to-end consequence of F24-C4-M1, and the reason it is a fix
        // rather than a tidy-up. A secret that lives ONLY in the `--home`
        // config must reach the scrubber, or the log member ships it verbatim.
        const SECRET: &str = "F24C4-HOME-SCOPED-SECRET-4b1d77e0";

        let dir = tempfile::tempdir().unwrap();
        let home = dir.path().join("home");
        std::fs::create_dir_all(&home).unwrap();
        std::fs::write(
            home.join("config.toml"),
            format!("[providers.anthropic]\napi_key = \"{SECRET}\"\n"),
        )
        .unwrap();
        std::fs::write(
            home.join(LOG_FILE),
            format!("[gateway] auth rejected using token {SECRET}\n"),
        )
        .unwrap();

        // POSITIVE CONTROL: the secret really is in the log before collection,
        // or its absence afterwards proves nothing.
        assert!(
            std::fs::read_to_string(home.join(LOG_FILE))
                .unwrap()
                .contains(SECRET)
        );

        let out = dir.path().join("bundle");
        let scope = ScopeArgs {
            profile: Some("t".into()),
            home: Some(home.clone()),
        };
        support_bundle(&scope, Some(out.clone()), false)
            .await
            .expect("the bundle must be produced");

        let log = std::fs::read_to_string(out.join("recent-log.txt")).unwrap();
        assert!(
            !log.contains(SECRET),
            "the log member leaked a secret that lived in the --home config: {log}"
        );
        assert!(
            log.contains(support_bundle::REDACTED),
            "and it must have been replaced rather than dropped: {log}"
        );
    }

    #[tokio::test]
    async fn channel_health_is_collected_when_a_gateway_published_it() {
        // Criterion 4 names "redacted HEALTH/log/support evidence". The health
        // publication is a real file; a bundle without it is missing a third of
        // the clause.
        let dir = tempfile::tempdir().unwrap();
        let home = dir.path().join("home");
        std::fs::create_dir_all(&home).unwrap();
        std::fs::write(
            crate::channel::channel_health_path(&home),
            r#"{"configured":2,"registered":2}"#,
        )
        .unwrap();

        let out = dir.path().join("bundle");
        let scope = ScopeArgs {
            profile: Some("t".into()),
            home: Some(home.clone()),
        };
        support_bundle(&scope, Some(out.clone()), false)
            .await
            .unwrap();

        let raw = std::fs::read_to_string(out.join("manifest.json")).unwrap();
        let manifest: support_bundle::BundleManifest = serde_json::from_str(&raw).unwrap();
        assert!(
            manifest.members.iter().any(|m| m == "channel-health.json"),
            "channel health must be a declared member: {:?}",
            manifest.members
        );
        assert!(out.join("channel-health.json").exists());

        // KNOWN-NEGATIVE, so the assertion above is not satisfied by a member
        // that is always present: with no health file the absence must be
        // NAMED rather than silently skipped.
        let home2 = dir.path().join("home2");
        std::fs::create_dir_all(&home2).unwrap();
        let out2 = dir.path().join("bundle2");
        let scope2 = ScopeArgs {
            profile: Some("t".into()),
            home: Some(home2.clone()),
        };
        support_bundle(&scope2, Some(out2.clone()), false)
            .await
            .unwrap();
        let m2: support_bundle::BundleManifest =
            serde_json::from_str(&std::fs::read_to_string(out2.join("manifest.json")).unwrap())
                .unwrap();
        assert!(
            !m2.members.iter().any(|m| m == "channel-health.json"),
            "no health file existed, so no member may be invented"
        );
        assert!(
            m2.absent_sources
                .iter()
                .any(|a| a.contains("channel-health.json")),
            "and the absence must be NAMED: {:?}",
            m2.absent_sources
        );
    }
}
