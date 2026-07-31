//! Integration tests for `wayland-core migrate grok`.
//!
//! The grok source shipped with six unit tests and **zero** integration tests,
//! while `hermes`, `quarantine` and `typed_dryrun` each carry a suite. The unit
//! tests all stop at [`migrate::grok::build_plan`] — nothing exercised the APPLY
//! path, so every claim the module makes about what reaches the user's
//! `config.toml` was mock-proven and never driven. This file drives it.
//!
//! Structure and discipline are taken from `migrate_hermes.rs`: the same env
//! guard, the same `rooted()` tempdir `WAYLAND_HOME`, and `#[serial]` on every
//! test because the importer resolves the existing-profile set through the
//! process-global `WAYLAND_HOME`.
//!
//! # Two corpora, deliberately
//!
//! * A **synthetic fixture** ([`fixture_grok`]) for the apply-path behaviours,
//!   because those need a home shaped to hit each branch (a disabled server, an
//!   executable server, a credential).
//! * The **real-install corpus** at `tests/fixtures/portability/grok`, cloned
//!   from a real `~/.grok` v0.2.103 by `scripts/portability-corpus-gen.py`.
//!   `config.toml` in that corpus is BYTE-IDENTICAL to the real install's;
//!   `version.json` is semantically identical (re-serialized sorted);
//!   `auth.json` is a canary placeholder because the importer provably never
//!   opens it — a claim [`the_importer_never_opens_the_session_store`] proves
//!   rather than asserts. Everything else is a directory marker preserving the
//!   real subdirectory counts.
//!
//!   A hand-written fixture can only contain what its author thought of. The
//!   corpus is the shape a real install actually has, and it is the only reason
//!   this file can say anything about grok installs that exist.

use std::path::{Path, PathBuf};

use serial_test::serial;
use tempfile::TempDir;
use wcore_cli::migrate::quarantine::QuarantineStore;
use wcore_cli::migrate::{self, HermesArgs};
use wcore_config::portability::{GROK_ROOT_PROFILE_ID, PeerSource};

/// The OIDC session value planted in the fixture's `auth.json`. grok's own
/// resolution order makes this a short-lived session token, not a provider API
/// key, and the module header commits to never promoting it into a profile.
/// Distinctive so a recursive search for it cannot match anything incidental.
const SESSION_SENTINEL: &str = "grok-oidc-session-NEVER-PROMOTE-8f21c4";

// --- env guard (identical discipline to migrate_hermes.rs) ----------------

struct EnvGuard(Vec<(&'static str, Option<std::ffi::OsString>)>);
impl EnvGuard {
    fn set(pairs: &[(&'static str, Option<&str>)]) -> Self {
        let saved = pairs
            .iter()
            .map(|(k, v)| {
                let prev = std::env::var_os(k);
                match v {
                    Some(val) => unsafe { std::env::set_var(k, val) },
                    None => unsafe { std::env::remove_var(k) },
                }
                (*k, prev)
            })
            .collect();
        Self(saved)
    }
}
impl Drop for EnvGuard {
    fn drop(&mut self) {
        for (k, prev) in &self.0 {
            match prev {
                Some(v) => unsafe { std::env::set_var(k, v) },
                None => unsafe { std::env::remove_var(k) },
            }
        }
    }
}

/// A tempdir `WAYLAND_HOME` with `XDG_DATA_HOME` and `GROK_HOME` both cleared,
/// so neither can shadow the override and no test can accidentally reach a real
/// `~/.grok` on the machine running it.
fn rooted() -> (EnvGuard, TempDir) {
    let home = tempfile::tempdir().unwrap();
    let g = EnvGuard::set(&[
        ("WAYLAND_HOME", Some(home.path().to_str().unwrap())),
        ("XDG_DATA_HOME", None),
        ("GROK_HOME", None),
    ]);
    (g, home)
}

// --- synthetic fixture ----------------------------------------------------

/// A grok home exercising every apply-path branch at once:
/// a `[models] default`, one launchable MCP server, one DISABLED MCP server, an
/// OIDC session store, a user skill, a persona and a memory note, plus the two
/// vendor catalogs that must never be imported.
fn fixture_grok() -> TempDir {
    let dir = tempfile::tempdir().unwrap();
    let h = dir.path();
    std::fs::write(
        h.join("config.toml"),
        "[models]\ndefault = \"grok-code-fast-1\"\n\n\
         [mcp_servers.live]\ncommand = \"/usr/bin/grok-srv\"\nargs = [\"--serve\"]\n\n\
         [mcp_servers.off]\ncommand = \"/usr/bin/disabled-srv\"\nenabled = false\n\n\
         [ui]\nscreen_mode = \"minimal\"\n",
    )
    .unwrap();
    std::fs::write(
        h.join("auth.json"),
        format!("{{\"https://auth.x.ai::acct\":{{\"key\":\"{SESSION_SENTINEL}\"}}}}"),
    )
    .unwrap();
    std::fs::write(h.join("version.json"), "{\"version\": \"0.2.103\"}\n").unwrap();

    std::fs::create_dir_all(h.join("skills/mine")).unwrap();
    std::fs::write(
        h.join("skills/mine/SKILL.md"),
        "---\nname: mine\n---\nprose only",
    )
    .unwrap();
    std::fs::create_dir_all(h.join("personas")).unwrap();
    std::fs::write(h.join("personas/reviewer.toml"), "name = \"reviewer\"\n").unwrap();
    std::fs::create_dir_all(h.join("memory")).unwrap();
    std::fs::write(h.join("memory/note.md"), "a memory").unwrap();
    std::fs::write(h.join("memory/MEMORY.md"), "entrypoint").unwrap();
    // The vendor's and the server's shipped catalogs.
    for c in ["bundled/help", "server-skills/s"] {
        std::fs::create_dir_all(h.join(c)).unwrap();
        std::fs::write(h.join(c).join("SKILL.md"), "vendor prose").unwrap();
    }
    dir
}

fn grok_args(home: &Path, include_credentials: bool, overwrite: bool) -> HermesArgs {
    HermesArgs {
        home: Some(home.to_path_buf()),
        dry_run: false,
        yes: true,
        include_credentials,
        overwrite,
        json: false,
        select: Vec::new(),
        exclude: Vec::new(),
    }
}

fn config_toml(home: &Path) -> String {
    std::fs::read_to_string(home.join("config.toml")).unwrap_or_default()
}

/// Every regular file under a directory, read as bytes. Used to prove a token
/// reached NO file in the Wayland home rather than merely no `config.toml` —
/// "it isn't in the file I looked at" is not the claim these tests make.
fn all_files(root: &Path) -> Vec<(PathBuf, Vec<u8>)> {
    let mut out = Vec::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(d) = stack.pop() {
        let Ok(rd) = std::fs::read_dir(&d) else {
            continue;
        };
        for e in rd.filter_map(Result::ok) {
            let p = e.path();
            if p.is_dir() {
                stack.push(p);
            } else if let Ok(b) = std::fs::read(&p) {
                out.push((p, b));
            }
        }
    }
    out
}

fn assert_absent_from_home(home: &Path, needle: &str, what: &str) {
    let files = all_files(home);
    // A search over an EMPTY tree passes vacuously. Refuse to run in that state.
    assert!(
        !files.is_empty(),
        "{what}: the Wayland home holds no files at all, so this absence proves nothing"
    );
    for (p, bytes) in &files {
        assert!(
            !String::from_utf8_lossy(bytes).contains(needle),
            "{what}: {needle} reached {}",
            p.display()
        );
    }
}

// --- real-install corpus access -------------------------------------------

fn corpus() -> PathBuf {
    let p = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/portability/grok");
    assert!(
        p.is_dir(),
        "committed corpus missing at {} — regenerate with \
         scripts/portability-corpus-gen.py --source ~/.grok --kind grok",
        p.display()
    );
    p
}

fn manifest() -> serde_json::Value {
    let raw = std::fs::read_to_string(corpus().join("MANIFEST.json")).unwrap();
    serde_json::from_str(&raw).unwrap()
}

/// The canary tokens the corpus declares, read at run time so regenerating the
/// corpus cannot silently turn an assertion into a tautology.
fn corpus_canaries() -> Vec<String> {
    let m = manifest();
    let list: Vec<String> = m["canaries"]
        .as_array()
        .expect("manifest declares no canaries array")
        .iter()
        .map(|c| c["canary"].as_str().unwrap().to_string())
        .collect();
    // grok's real install carries exactly one secret-bearing document, so this
    // list is short by construction — but an EMPTY list would make every
    // absence assertion below vacuous.
    assert!(
        !list.is_empty(),
        "the grok manifest declares no canary — an absence assertion would prove nothing"
    );
    list
}

fn declared(key: &str) -> u64 {
    manifest()["counts"][key]
        .as_u64()
        .unwrap_or_else(|| panic!("manifest declares no count for {key}"))
}

// =========================================================================
// Apply path — the surface no unit test reaches
// =========================================================================

/// The root setup reaches the live config, and nothing the peer never had does.
#[test]
#[serial]
fn apply_writes_the_grok_root_profile() {
    let (_g, home) = rooted();
    let grok = fixture_grok();

    let report = migrate::run_import(PeerSource::Grok, &grok_args(grok.path(), false, false))
        .expect("grok import failed");

    assert_eq!(report.profiles_added, 1, "{report:?}");
    let names: Vec<String> = wcore_config::config::global_profiles()
        .into_iter()
        .map(|(n, _, _)| n)
        .collect();
    assert!(
        names.contains(&GROK_ROOT_PROFILE_ID.to_string()),
        "the root setup did not land: {names:?}"
    );

    let toml = config_toml(home.path());
    assert!(toml.contains("grok-code-fast-1"), "{toml}");
    assert!(toml.contains("xai"), "{toml}");
    // Known-negative: grok has ONE user-global setup. Its `personas/` are
    // subagent instruction bodies, not provider bindings, so inventing a
    // profile per persona would publish a profile the peer never had.
    assert!(
        !names.contains(&"reviewer".to_string()),
        "a persona was published as a profile: {names:?}"
    );
    assert_eq!(names.len(), 1, "exactly one grok profile: {names:?}");
}

/// The module's central security claim, and the one with no coverage at all:
/// grok's only credential is an OIDC session, so `--include-credentials` must
/// NOT promote it into a `ProfileConfig::api_key`. Driven with the flag ON,
/// because the claim is worthless if only tested with the flag off.
#[test]
#[serial]
fn the_oidc_session_is_never_promoted_even_with_include_credentials() {
    let (_g, home) = rooted();
    let grok = fixture_grok();

    let report = migrate::run_import(PeerSource::Grok, &grok_args(grok.path(), true, false))
        .expect("grok import failed");

    assert_eq!(
        report.credentials_written, 0,
        "a session token was written as a credential: {report:?}"
    );
    assert_absent_from_home(
        home.path(),
        SESSION_SENTINEL,
        "--include-credentials promoted the OIDC session",
    );
    assert!(
        !config_toml(home.path()).contains("api_key"),
        "an api_key field was created for a peer that has no API key"
    );

    // Positive half — without it, an import that did nothing would pass.
    let plan = migrate::grok::build_plan(grok.path(), true).unwrap();
    let root = &plan.profiles[0];
    assert!(
        root.has_credential,
        "the session store was not even noticed"
    );
    assert_eq!(
        root.credential_env_var.as_deref(),
        Some("auth.json.key"),
        "the credential must still be recorded BY NAME"
    );
    assert!(
        root.config.api_key.is_none(),
        "the value reached the plan itself"
    );
    assert!(config_toml(home.path()).contains("grok-code-fast-1"));
}

/// T-26-02-03 for grok. A peer MCP entry carrying a launch command is a
/// child-process surface fed by peer-controlled strings; writing it into
/// `config.toml` makes it spawnable. `migrate_hermes.rs` pins this for Hermes.
/// Nothing pinned it for grok.
#[test]
#[serial]
fn an_executable_grok_mcp_definition_is_contained_not_written_live() {
    let (_g, home) = rooted();
    let grok = fixture_grok();

    let report = migrate::run_import(PeerSource::Grok, &grok_args(grok.path(), false, false))
        .expect("grok import failed");

    let toml = config_toml(home.path());
    assert!(
        !toml.contains("grok-srv"),
        "a launchable peer MCP command landed in config.toml:\n{toml}"
    );
    assert!(
        QuarantineStore::for_current_home()
            .contains("mcp_server:live")
            .unwrap(),
        "…and it was silently dropped rather than contained: {report:?}"
    );
    // Positive half: the import is not simply inert.
    assert!(toml.contains("grok-code-fast-1"), "{toml}");
    assert!(report.quarantined >= 1, "{report:?}");
}

/// `enabled = false` is the user's own statement. The unit test proves the PLAN
/// omits it; this proves the apply path does not re-enable it by another route
/// — it must be in neither the live config nor the quarantine store, because
/// "contained" would still leave it promotable.
#[test]
#[serial]
fn a_disabled_grok_mcp_server_is_absent_from_every_destination() {
    let (_g, home) = rooted();
    let grok = fixture_grok();

    migrate::run_import(PeerSource::Grok, &grok_args(grok.path(), false, false)).unwrap();

    assert_absent_from_home(
        home.path(),
        "disabled-srv",
        "a server the user turned off was carried across",
    );
    assert!(
        !QuarantineStore::for_current_home()
            .contains("mcp_server:off")
            .unwrap(),
        "a disabled server became a promotable quarantine entry"
    );
    // Positive half: the ENABLED sibling did travel, so the absence above is a
    // decision about `enabled = false` and not a wholesale MCP failure.
    assert!(
        QuarantineStore::for_current_home()
            .contains("mcp_server:live")
            .unwrap()
    );
}

#[test]
#[serial]
fn the_grok_import_is_idempotent_without_overwrite() {
    let (_g, home) = rooted();
    let grok = fixture_grok();

    migrate::run_import(PeerSource::Grok, &grok_args(grok.path(), false, false)).unwrap();
    let first = config_toml(home.path());
    migrate::run_import(PeerSource::Grok, &grok_args(grok.path(), false, false)).unwrap();
    let second = config_toml(home.path());

    assert_eq!(
        first, second,
        "re-importing without --overwrite changed the config"
    );
    assert_eq!(
        second.matches(GROK_ROOT_PROFILE_ID).count(),
        1,
        "the root setup was duplicated:\n{second}"
    );
    // Positive half: there was something to be idempotent ABOUT.
    assert!(first.contains("grok-code-fast-1"), "{first}");
}

/// grok records its installed version in `version.json`, not in a plain
/// `VERSION` file — a branch `migrate::peer_version` carries specifically for
/// this peer and which no test reached. A provenance record that silently omits
/// the source version reads as "the source declared none", which is a
/// fabrication by omission.
#[test]
#[serial]
fn version_json_reaches_the_provenance_record() {
    let (_g, _home) = rooted();
    let grok = fixture_grok();

    migrate::run_import(PeerSource::Grok, &grok_args(grok.path(), false, false)).unwrap();

    let entries = QuarantineStore::for_current_home().entries().unwrap();
    assert!(!entries.is_empty(), "nothing carried provenance at all");
    for e in &entries {
        assert_eq!(e.provenance.source_tool, "grok", "{e:?}");
        assert_eq!(
            e.provenance.source_version.as_deref(),
            Some("0.2.103"),
            "version.json did not reach the record: {e:?}"
        );
    }

    // Known-negative: with NO version.json the record must say `None`, never a
    // guessed version. Same fixture minus the one file.
    let (_g2, _home2) = rooted();
    let bare = fixture_grok();
    std::fs::remove_file(bare.path().join("version.json")).unwrap();
    migrate::run_import(PeerSource::Grok, &grok_args(bare.path(), false, false)).unwrap();
    for e in QuarantineStore::for_current_home().entries().unwrap() {
        assert_eq!(
            e.provenance.source_version, None,
            "a version was invented for a home that declares none: {e:?}"
        );
    }
}

#[test]
#[serial]
fn missing_grok_home_errors() {
    let _g = rooted().0;
    let missing: PathBuf = tempfile::tempdir().unwrap().path().join("nope");
    let err = migrate::grok::detect_home(Some(&missing)).unwrap_err();
    assert!(
        err.to_string().contains("no grok setup found"),
        "unexpected error text: {err}"
    );
    // Known-positive on the same instrument: an auth-only home IS acceptable,
    // so the guard above is discriminating and not a blanket refusal.
    let ok = tempfile::tempdir().unwrap();
    std::fs::write(ok.path().join("auth.json"), "{}").unwrap();
    assert!(migrate::grok::detect_home(Some(ok.path())).is_ok());
}

// =========================================================================
// The real install — `~/.grok` v0.2.103, via the committed structure clone
// =========================================================================

/// The importer, driven end to end against the shape a real grok install has.
///
/// This is the leg the module never had: every prior proof used a home written
/// by the same person who wrote the parser.
#[test]
#[serial]
fn the_real_install_corpus_imports_and_leaks_no_canary() {
    let (_g, home) = rooted();

    let report = migrate::run_import(PeerSource::Grok, &grok_args(&corpus(), true, false))
        .expect("the real-install corpus failed to import");

    // The real install is logged in, so the credential is SEEN…
    let plan = migrate::grok::build_plan(&corpus(), true).unwrap();
    assert!(
        plan.profiles[0].has_credential,
        "the real install's auth.json was not detected"
    );
    // …and still never written, with the flag ON.
    assert_eq!(report.credentials_written, 0, "{report:?}");
    for c in corpus_canaries() {
        assert_absent_from_home(home.path(), &c, "a corpus canary reached the Wayland home");
    }

    // The measured shape of the real install, recorded rather than inferred:
    // its `config.toml` declares no `[models] default` and no `[mcp_servers]`,
    // so the imported root setup is a credential reference and nothing else.
    // Asserted so that a future change to the real-world default is visible as
    // a failure here instead of being discovered by a user.
    assert_eq!(
        plan.profiles[0].config.model, None,
        "the real install's config.toml gained a [models] default"
    );
    assert!(plan.mcp_servers.is_empty(), "{:?}", plan.mcp_servers);
    assert_eq!(report.profiles_added, 1, "{report:?}");
}

/// The "credential is a REFERENCE, never a value" claim, proved DIFFERENTIALLY
/// instead of asserted: replace the session store's contents wholesale and the
/// plan must not move by one byte. If any code path ever starts reading it,
/// this goes red — which is also what makes the corpus's canary placeholder an
/// honest substitute for the real file's bytes.
#[test]
#[serial]
fn the_importer_never_opens_the_session_store() {
    let (_g, _home) = rooted();

    let base = migrate::grok::build_plan(&corpus(), true).unwrap();

    // Copy the corpus (never mutate a committed fixture) and rewrite auth.json.
    let scratch = tempfile::tempdir().unwrap();
    let dst = scratch.path().join("grok");
    copy_tree(&corpus(), &dst);
    std::fs::write(
        dst.join("auth.json"),
        "\u{0}\u{1}not json at all — 8f21c4 \u{ff}\u{fe}",
    )
    .unwrap();

    let mutated = migrate::grok::build_plan(&dst, true).unwrap();

    assert_eq!(
        format!("{:?}", base.profiles[0].config),
        format!("{:?}", mutated.profiles[0].config),
        "the session store's CONTENTS changed the imported profile"
    );
    assert_eq!(base.warnings, mutated.warnings);
    assert!(
        mutated.profiles[0].has_credential,
        "…and the file's PRESENCE must still register"
    );

    // Known-positive on the same instrument: the plan is not simply insensitive
    // to its inputs. Removing the file DOES move it.
    std::fs::remove_file(dst.join("auth.json")).unwrap();
    let removed = migrate::grok::build_plan(&dst, true).unwrap();
    assert!(
        !removed.profiles[0].has_credential,
        "build_plan is inert — the comparison above proved nothing"
    );
}

/// The deferred inventory, checked against the counts the corpus generator
/// recorded off the real tree rather than against numbers written here.
#[test]
#[serial]
fn the_real_install_deferred_inventory_matches_the_manifest() {
    let (_g, _home) = rooted();
    let plan = migrate::grok::build_plan(&corpus(), false).unwrap();

    assert_eq!(plan.deferred.skills as u64, declared("skills"));
    for dir in ["bundled", "marketplace-cache", "sessions"] {
        let n = declared(dir);
        assert!(
            n > 0,
            "manifest count for {dir} is zero — assertion vacuous"
        );
        assert_eq!(
            plan.deferred_other.get(dir).copied().unwrap_or(0) as u64,
            n,
            "deferred inventory disagrees with the real tree for {dir}: {:?}",
            plan.deferred_other
        );
    }
    // `vendor/` exists in the real install with zero subdirectories, so it must
    // be ABSENT from the report rather than present as a zero — a reported zero
    // and an unreported directory are different statements.
    assert_eq!(declared("vendor"), 0);
    assert!(!plan.deferred_other.contains_key("vendor"));
}

// =========================================================================
// The live drive — a real `~/.grok`, untouched
// =========================================================================

/// Drive the importer at a REAL grok install, and prove it left no mark.
///
/// `#[ignore]`d and env-gated on purpose: CI has no grok install, and a test
/// that silently no-ops when the environment is absent is the shape that let
/// two exactly-once claims be falsified elsewhere in this program — it would
/// report a green having measured nothing. Refusing to run is honest; passing
/// vacuously is not.
///
/// ```text
/// WL_GROK_LIVE_HOME=$HOME/.grok \
///   cargo test -p wcore-cli --test migrate_grok -- --ignored --exact \
///   live_drive_against_a_real_grok_install
/// ```
///
/// The non-mutation claim is MEASURED, not assumed: a content-addressed
/// `tree_digest` of the peer home is taken before and after, and the test fails
/// if it moves by one byte.
#[test]
#[serial]
#[ignore = "requires a real grok install; set WL_GROK_LIVE_HOME"]
fn live_drive_against_a_real_grok_install() {
    let Ok(raw) = std::env::var("WL_GROK_LIVE_HOME") else {
        panic!("WL_GROK_LIVE_HOME is unset — refusing to report a green having measured nothing");
    };
    let peer = PathBuf::from(raw);
    assert!(peer.is_dir(), "{} is not a directory", peer.display());

    let before = wcore_config::portability::tree_digest(&peer).unwrap();
    assert!(
        before.files > 0,
        "the peer home is empty — a non-mutation claim over it would be vacuous"
    );

    let (_g, home) = rooted();
    let plan = migrate::grok::build_plan(&peer, true).expect("real install failed to plan");
    let report = migrate::run_import(PeerSource::Grok, &grok_args(&peer, true, false))
        .expect("real install failed to import");

    let after = wcore_config::portability::tree_digest(&peer).unwrap();
    assert_eq!(
        before.digest, after.digest,
        "THE IMPORTER MUTATED A REAL PEER HOME ({} files before, {} after)",
        before.files, after.files
    );

    // What the run must have achieved for the digest comparison to mean
    // anything: an import that failed early would also leave the home untouched.
    assert_eq!(report.profiles_added, 1, "{report:?}");
    assert_eq!(
        plan.profiles[0].name, GROK_ROOT_PROFILE_ID,
        "{:?}",
        plan.profiles
    );
    assert_eq!(
        report.credentials_written, 0,
        "a real OIDC session was written as a credential: {report:?}"
    );
    assert!(
        !config_toml(home.path()).is_empty(),
        "nothing was written into the Wayland home, so nothing was driven"
    );

    // Printed rather than asserted: these are MEASUREMENTS of one install, not
    // properties of grok. An assertion here would pin one machine's state.
    println!(
        "LIVE-GROK peer={} files={} version={:?} model={:?} provider={:?} \
         has_credential={} mcp={} skills={} personas={} memory={} deferred_other={:?} \
         warnings={:?} report={{profiles_added:{}, quarantined:{}, discovered:{}, \
         imported:{}, excluded:{}, files_written:{}}}",
        peer.display(),
        before.files,
        std::fs::read_to_string(peer.join("version.json")).ok(),
        plan.profiles[0].config.model,
        plan.profiles[0].config.provider,
        plan.profiles[0].has_credential,
        plan.mcp_servers.len(),
        plan.deferred.skills,
        plan.deferred.personas,
        plan.deferred.memory_files,
        plan.deferred_other,
        plan.warnings,
        report.profiles_added,
        report.quarantined,
        report.discovered,
        report.imported,
        report.excluded,
        report.files_written,
    );
}

/// Recursive copy. `std::fs` has no directory copy and the corpus is small.
fn copy_tree(src: &Path, dst: &Path) {
    std::fs::create_dir_all(dst).unwrap();
    for e in std::fs::read_dir(src).unwrap().filter_map(Result::ok) {
        let p = e.path();
        let target = dst.join(e.file_name());
        if p.is_dir() {
            copy_tree(&p, &target);
        } else {
            std::fs::copy(&p, &target).unwrap();
        }
    }
}
