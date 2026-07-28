//! F26-02 — import quarantine, provenance, selection, and the PAIRED inertness
//! proof against the real `wayland-core` binary.
//!
//! # Why every absence assertion here is paired
//!
//! Asserting that a quarantined skill's side effect is absent passes vacuously
//! if the skill was never loaded, never discovered, or silently dropped on a
//! parse error. So each negative leg carries a positive control using the SAME
//! payload: `live_negative_*` proves the sentinel is absent while contained,
//! and `live_positive_control_*` proves the SAME payload DOES create it once an
//! operator promotes it. Absence-without-a-positive-control is the self-passing
//! gate shape this phase forbids, and it is exactly what a payload that never
//! loaded would look like.
//!
//! Both sentinels live in a per-run temporary directory, and each leg asserts
//! the sentinel is ABSENT before it begins, so a stale artifact from an earlier
//! run can neither satisfy the positive leg nor contaminate the negative one.

#[path = "support/mod.rs"]
mod support;

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serial_test::serial;
use tempfile::TempDir;
use wcore_cli::migrate::provenance::{
    PROVENANCE_DOMAIN, Provenance, ProvenanceDocument, item_digest, normalize_relative_path,
};
use wcore_cli::migrate::quarantine::{
    self, Classification, ExecutableReason, MAX_QUARANTINE_FILE_BYTES, MAX_QUARANTINE_FILES,
    MAX_QUARANTINE_TOTAL_BYTES, QuarantineRequest, QuarantineStore,
};
use wcore_cli::migrate::select::{Accounting, Outcome, QuarantineReason, SelectError, Selection};
use wcore_cli::migrate::{self, HermesArgs, MigrateCmd, PromoteArgs};
use wcore_config::config::{McpServerConfig, TransportType};
use wcore_skills::types::LoadedFrom;

// ---------------------------------------------------------------------------
// Harness
// ---------------------------------------------------------------------------

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

/// A tempdir `WAYLAND_HOME`, so both `config.toml` and the quarantine store
/// resolve inside the throwaway home.
fn rooted() -> (EnvGuard, TempDir) {
    let home = tempfile::tempdir().unwrap();
    let g = EnvGuard::set(&[
        ("WAYLAND_HOME", Some(home.path().to_str().unwrap())),
        ("XDG_DATA_HOME", None),
    ]);
    (g, home)
}

fn fixtures() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("fixtures")
}

fn exec_fixtures() -> PathBuf {
    fixtures().join("portability-exec")
}

/// Read a committed fixture skill body, substituting the two sentinel tokens.
fn fixture_body(rel: &str, sentinel: &Path, sentinel2: &Path) -> String {
    let raw = std::fs::read_to_string(exec_fixtures().join(rel))
        .unwrap_or_else(|e| panic!("fixture {rel} unreadable: {e}"));
    raw.replace("__SENTINEL__", sentinel.to_str().unwrap())
        .replace("__SENTINEL2__", sentinel2.to_str().unwrap())
}

/// Build a throwaway Hermes home carrying the committed executable fixtures.
///
/// One profile (so the existing profile/MCP vocabulary is exercised too) plus
/// the three skills at the home-level `skills/` root.
fn peer_home_with_fixtures(sentinel: &Path, sentinel2: &Path) -> TempDir {
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path();

    std::fs::write(
        home.join("config.yaml"),
        fixture_body("hermes-config.yaml", sentinel, sentinel2),
    )
    .unwrap();

    let p = home.join("profiles").join("alpha");
    std::fs::create_dir_all(&p).unwrap();
    std::fs::write(
        p.join("config.yaml"),
        "model:\n  default: claude-opus\n  provider: anthropic\n",
    )
    .unwrap();
    std::fs::write(p.join("SOUL.md"), "You are alpha.").unwrap();

    for skill in ["repo-status", "release-notes", "self-promoting"] {
        let d = home.join("skills").join(skill);
        std::fs::create_dir_all(&d).unwrap();
        std::fs::write(
            d.join("SKILL.md"),
            fixture_body(&format!("skills/{skill}/SKILL.md"), sentinel, sentinel2),
        )
        .unwrap();
    }
    // The three self-promotion signals, verbatim from the fixture.
    let sp = home.join("skills").join("self-promoting");
    for f in ["PROMOTE", "manifest.json"] {
        std::fs::copy(
            exec_fixtures().join("skills/self-promoting").join(f),
            sp.join(f),
        )
        .unwrap();
    }
    dir
}

fn args_for(home: &Path) -> HermesArgs {
    HermesArgs {
        home: Some(home.to_path_buf()),
        dry_run: false,
        yes: true,
        include_credentials: false,
        overwrite: false,
        json: false,
        select: Vec::new(),
        exclude: Vec::new(),
    }
}

fn mcp(command: Option<&str>) -> McpServerConfig {
    McpServerConfig {
        transport: TransportType::Stdio,
        command: command.map(str::to_string),
        args: None,
        env: None,
        url: None,
        headers: None,
        deferred: None,
        allow_local: false,
        only_for_assistant: None,
    }
}

/// Every path the agent-facing skill enumeration walks, for the current
/// `WAYLAND_HOME` and a given cwd.
fn agent_facing_skill_roots(cwd: &Path) -> Vec<PathBuf> {
    let mut roots = Vec::new();
    if let Some(d) = wcore_skills::paths::user_skills_dir() {
        roots.push(d);
    }
    if let Some(d) = wcore_skills::paths::user_commands_dir() {
        roots.push(d);
    }
    roots.extend(wcore_skills::paths::wayland_home_skills_dirs());
    roots.extend(wcore_skills::paths::project_skills_dirs(cwd));
    roots.extend(wcore_skills::paths::project_commands_dirs(cwd));
    roots
}

// ===========================================================================
// TASK 1 — classification and containment (behaviors 1-8)
// ===========================================================================

/// Behavior 1: an imported skill whose body carries a shell directive is
/// classified executable, using the EXISTING detector.
#[test]
fn t1_skill_with_a_shell_directive_is_executable_via_the_enforced_detector() {
    let sentinel = Path::new("/tmp/never-created-by-this-test");
    let body = fixture_body("skills/repo-status/SKILL.md", sentinel, sentinel);
    assert!(
        body.contains("```!"),
        "the committed fixture must carry a real block directive; got:\n{body}"
    );
    assert_eq!(
        quarantine::classify_skill_body(&body, LoadedFrom::Skills),
        Classification::Executable(ExecutableReason::SkillShellDirective)
    );
    // The anti-drift claim, asserted rather than commented: the classifier and
    // the predicate the executor/permission-checker enforce agree on this body.
    assert_eq!(
        quarantine::classify_skill_body(&body, LoadedFrom::Skills).is_executable(),
        wcore_skills::shell::contains_shell_commands(&body, LoadedFrom::Skills)
    );
}

/// Behavior 2: an imported skill with no directive is classified
/// non-executable and imports WITHOUT promotion.
#[test]
#[serial]
fn t2_skill_without_a_directive_is_data_and_needs_no_promotion() {
    let sentinel = Path::new("/tmp/never-created-by-this-test");
    let body = fixture_body("skills/release-notes/SKILL.md", sentinel, sentinel);
    assert!(!body.contains("```!"), "fixture must carry NO directive");
    assert_eq!(
        quarantine::classify_skill_body(&body, LoadedFrom::Skills),
        Classification::Data
    );

    // And end to end: it is IMPORTED, not quarantined.
    let (_g, _home) = rooted();
    let peer = peer_home_with_fixtures(sentinel, sentinel);
    let report = migrate::run_import(
        wcore_config::portability::PeerSource::Hermes,
        &args_for(peer.path()),
    )
    .unwrap();
    let store = QuarantineStore::for_current_home();
    assert!(
        !store.contains("skill:skills/release-notes").unwrap(),
        "a directive-free skill must import without ceremony"
    );
    // Positive half: the run DID quarantine the executable siblings, so the
    // absence above is not the absence of any import at all.
    assert!(
        report.quarantined >= 2,
        "expected the two executable fixtures to be contained; report={report:?}"
    );
}

/// Behavior 3: an imported peer MCP definition carrying a launch command is
/// classified executable — a stdio server is a child process.
#[test]
#[serial]
fn t3_peer_mcp_launch_command_is_executable_and_never_lands_live() {
    assert_eq!(
        quarantine::classify_mcp_server(&mcp(Some("/usr/bin/peer-mcp-server"))),
        Classification::Executable(ExecutableReason::McpLaunchCommand)
    );
    // A definition with no command is a setting, not a process.
    assert_eq!(
        quarantine::classify_mcp_server(&mcp(None)),
        Classification::Data
    );

    let sentinel = Path::new("/tmp/never-created-by-this-test");
    let (_g, home) = rooted();
    let peer = peer_home_with_fixtures(sentinel, sentinel);
    migrate::run_import(
        wcore_config::portability::PeerSource::Hermes,
        &args_for(peer.path()),
    )
    .unwrap();

    let toml = std::fs::read_to_string(home.path().join("config.toml")).unwrap_or_default();
    assert!(
        !toml.contains("[mcp.servers.peer-launcher]"),
        "an MCP definition carrying a launch command must NOT reach config.toml, \
         where it is launchable; got:\n{toml}"
    );
    // …and no imported profile may keep a DANGLING reference to it either: a
    // reference that resolves to nothing today would silently resolve to a
    // server of that name defined tomorrow, quietly undoing this containment.
    assert!(
        !toml.contains("\"peer-launcher\""),
        "an imported profile still references the withheld server; got:\n{toml}"
    );
    assert!(
        QuarantineStore::for_current_home()
            .contains("mcp_server:peer-launcher")
            .unwrap(),
        "…and it must be contained rather than dropped"
    );
    // Positive half: the URL-only peer server DID land live, so the absence
    // above is a classification result and not an empty import.
    assert!(
        toml.contains("peer-remote"),
        "a url-only MCP definition is a setting and must still import; got:\n{toml}"
    );
}

/// Behavior 4: an imported hook definition carrying a command is classified
/// executable — the surface GHSA-8r7g closed, reachable by another route.
#[test]
fn t4_hook_command_is_classified_executable() {
    assert_eq!(
        quarantine::classify_hook_command("./on-start.sh --quiet"),
        Classification::Executable(ExecutableReason::HookCommand)
    );
    assert_eq!(
        quarantine::classify_hook_command("   "),
        Classification::Data
    );
    // GHSA-8r7g's own default is not weakened by anything here: a project
    // config still cannot trust its own hooks, and the default is still false.
    assert!(
        !wcore_config::hooks::HooksConfig::default().trust_project_hooks,
        "trust_project_hooks must remain default-false"
    );
}

/// Behavior 5: quarantined content lands where the normal discovery path does
/// not look, and a full agent-facing skill enumeration does not include it.
#[tokio::test]
#[serial]
async fn t5_quarantined_content_is_absent_from_what_the_agent_would_load() {
    let sentinel = Path::new("/tmp/never-created-by-this-test");
    let (_g, home) = rooted();
    let peer = peer_home_with_fixtures(sentinel, sentinel);
    migrate::run_import(
        wcore_config::portability::PeerSource::Hermes,
        &args_for(peer.path()),
    )
    .unwrap();

    let store = QuarantineStore::for_current_home();
    assert!(
        store.root().is_dir(),
        "the store must have written something"
    );

    // Structural half: the store root is not INSIDE any root the enumeration
    // walks, so no walk can reach it.
    for root in agent_facing_skill_roots(home.path()) {
        assert!(
            !store.root().starts_with(&root),
            "quarantine root {:?} sits inside an agent-facing skill root {:?}",
            store.root(),
            root
        );
    }

    // Behavioural half: the REAL loader, run over this home, does not list it.
    let refs = wcore_skills::loader::load_catalog(home.path(), &[], false, None).await;
    let names: Vec<String> = refs.iter().map(|r| r.name.clone()).collect();
    assert!(
        !names.iter().any(|n| n == "repo-status"),
        "the quarantined skill must be absent from what the agent would load; got {names:?}"
    );

    // Positive control for the ENUMERATION itself: promote the same item and
    // the same loader now lists it. Without this, "absent" would be equally
    // consistent with a loader that lists nothing at all.
    store
        .promote(
            &["skill:skills/repo-status".to_string()],
            &wcore_config::config::wayland_config_dir().join("skills"),
        )
        .unwrap();
    let refs2 = wcore_skills::loader::load_catalog(home.path(), &[], false, None).await;
    let names2: Vec<String> = refs2.iter().map(|r| r.name.clone()).collect();
    assert!(
        names2.iter().any(|n| n == "repo-status"),
        "after promotion the SAME loader must list it — otherwise the absence \
         above measured a loader that finds nothing, not containment. got {names2:?}"
    );
}

/// Behavior 6: promotion requires an explicit operator action; NOTHING inside
/// the imported content can cause its own promotion.
#[test]
#[serial]
fn t6_nothing_the_imported_content_carries_can_promote_it() {
    let sentinel = Path::new("/tmp/never-created-by-this-test");
    let (_g, home) = rooted();
    let peer = peer_home_with_fixtures(sentinel, sentinel);
    migrate::run_import(
        wcore_config::portability::PeerSource::Hermes,
        &args_for(peer.path()),
    )
    .unwrap();

    let store = QuarantineStore::for_current_home();
    let id = "skill:skills/self-promoting";
    // The payload asserted its own trust five ways in frontmatter, once by a
    // marker FILE, and once in a manifest. It is still contained.
    assert!(
        store.contains(id).unwrap(),
        "content that claims trust in frontmatter, a marker file and a manifest \
         must still be contained — that is the half of GHSA-8r7g that holds"
    );
    let skills_dir = wcore_config::config::wayland_config_dir().join("skills");
    assert!(
        !skills_dir.join("self-promoting").exists(),
        "the self-promoting payload reached the live skills directory"
    );

    // An empty operator selection promotes nothing, regardless of what the
    // content says.
    assert!(store.promote(&[], &skills_dir).unwrap().is_empty());
    assert!(store.contains(id).unwrap());

    // Positive control: the EXPLICIT operator action does promote it, so the
    // refusals above are not a promotion path that never works.
    let promoted = store.promote(&[id.to_string()], &skills_dir).unwrap();
    assert_eq!(promoted, vec![id.to_string()]);
    assert!(skills_dir.join("self-promoting").join("SKILL.md").is_file());
    assert!(!store.contains(id).unwrap());
    let _ = home;
}

/// Behavior 7: the enforcement ceilings hold — a symlinked executable file is
/// refused, an oversized file is refused, and an oversized total surface is
/// refused, mirroring the existing workspace-trust refusals.
#[test]
fn t7_ceilings_refuse_symlink_oversized_file_and_oversized_surface() {
    let store_dir = tempfile::tempdir().unwrap();
    let store = QuarantineStore::new(store_dir.path());
    let src = tempfile::tempdir().unwrap();

    let ok_dir = src.path().join("ok");
    std::fs::create_dir_all(&ok_dir).unwrap();
    std::fs::write(ok_dir.join("SKILL.md"), "---\nname: ok\n---\nbody").unwrap();
    let req = |name: &str, dir: &Path| QuarantineRequest {
        id: format!("skill:{name}"),
        reason: ExecutableReason::SkillShellDirective,
        source_dir: Some(dir.to_path_buf()),
        inline: None,
        source_tool: "hermes".into(),
        source_version: None,
        source_path: format!("skills/{name}"),
        promote_as: name.into(),
    };
    // Pristine control FIRST: a well-formed item is ACCEPTED. Without this a
    // corpus of only-refusals would pass against something that refuses
    // everything.
    store
        .admit(&req("ok", &ok_dir))
        .expect("a clean item must be admitted");

    // (a) symlinked executable file — REFUSED.
    #[cfg(unix)]
    {
        let sl_dir = src.path().join("symlinked");
        std::fs::create_dir_all(&sl_dir).unwrap();
        std::fs::write(sl_dir.join("SKILL.md"), "---\nname: s\n---\nb").unwrap();
        std::os::unix::fs::symlink("/etc/hosts", sl_dir.join("escape.md")).unwrap();
        let err = store.admit(&req("symlinked", &sl_dir)).unwrap_err();
        assert!(
            matches!(err, quarantine::QuarantineError::ExecutableSymlink(_)),
            "a symlink in an executable surface must be REFUSED, got {err:?}"
        );
    }

    // (b) oversized file — REFUSED.
    let big_dir = src.path().join("big");
    std::fs::create_dir_all(&big_dir).unwrap();
    std::fs::write(
        big_dir.join("SKILL.md"),
        vec![b'x'; (MAX_QUARANTINE_FILE_BYTES + 1) as usize],
    )
    .unwrap();
    let err = store.admit(&req("big", &big_dir)).unwrap_err();
    assert!(
        matches!(err, quarantine::QuarantineError::FileTooLarge(_)),
        "an oversized executable file must be REFUSED, got {err:?}"
    );

    // (c) oversized TOTAL surface — REFUSED. Nine 4 MiB files clear the
    // per-file ceiling individually and breach the 32 MiB total together.
    let wide = src.path().join("wide");
    std::fs::create_dir_all(&wide).unwrap();
    for i in 0..9 {
        std::fs::write(
            wide.join(format!("part-{i}.md")),
            vec![b'y'; MAX_QUARANTINE_FILE_BYTES as usize],
        )
        .unwrap();
    }
    let err = store.admit(&req("wide", &wide)).unwrap_err();
    assert!(
        matches!(err, quarantine::QuarantineError::SurfaceTooLarge),
        "an oversized total surface must be REFUSED, got {err:?}"
    );

    // (d) count ceiling — REFUSED above 512 files in one surface.
    let many = src.path().join("many");
    std::fs::create_dir_all(&many).unwrap();
    for i in 0..(MAX_QUARANTINE_FILES + 1) {
        std::fs::write(many.join(format!("f{i}.md")), "x").unwrap();
    }
    let err = store.admit(&req("many", &many)).unwrap_err();
    assert!(
        matches!(err, quarantine::QuarantineError::SurfaceTooLarge),
        "a surface above the file-count ceiling must be REFUSED, got {err:?}"
    );

    // The clean item is still there — the refusals did not roll the store back.
    assert!(store.contains("skill:ok").unwrap());
}

/// Behavior 8: quarantining is REPORTED, not silent — both the plan and the
/// apply report name how many items were quarantined and why.
#[test]
#[serial]
fn t8_quarantining_is_reported_in_both_the_plan_and_the_apply_report() {
    let sentinel = Path::new("/tmp/never-created-by-this-test");
    let (_g, _home) = rooted();
    let peer = peer_home_with_fixtures(sentinel, sentinel);

    // The PLAN half: the published preview names the executable items and why.
    let published = migrate::published_for(
        wcore_config::portability::PeerSource::Hermes,
        &args_for(peer.path()),
    )
    .unwrap();
    let exec: Vec<&migrate::PublishedItem> = published
        .iter()
        .filter(|p| p.class == "executable")
        .collect();
    assert!(
        exec.len() >= 3,
        "plan must name the two executable skills and the MCP launcher; got {exec:?}"
    );
    for p in &exec {
        assert!(
            p.executable_reason.is_some(),
            "every executable item must carry WHY: {p:?}"
        );
    }
    // Positive half: data items are present too and are NOT flagged, so the
    // preview is not simply flagging everything.
    assert!(
        published.iter().any(|p| p.class == "data"),
        "the preview must also carry data items"
    );

    // The APPLY half: the report carries the count and a reason per item.
    let report = migrate::run_import(
        wcore_config::portability::PeerSource::Hermes,
        &args_for(peer.path()),
    )
    .unwrap();
    assert!(report.quarantined >= 3, "report={report:?}");
    assert_eq!(report.quarantine_notices.len(), report.quarantined);
    assert!(
        report
            .quarantine_notices
            .iter()
            .any(|n| n.contains("shell directive")),
        "notices must say WHAT made each item executable; got {:?}",
        report.quarantine_notices
    );
}

// ===========================================================================
// TASK 2 — provenance, selection, conservation, export (behaviors 1-8)
// ===========================================================================

/// Behavior 1: every imported item carries a provenance record naming the
/// source tool, the source version where declared, the source-relative path, a
/// content digest, and the import time.
#[test]
#[serial]
fn t9_every_contained_item_carries_a_full_provenance_record() {
    let sentinel = Path::new("/tmp/never-created-by-this-test");
    let (_g, _home) = rooted();
    let peer = peer_home_with_fixtures(sentinel, sentinel);
    // Declare a version the SOURCE owns, so the "where the source declares one"
    // clause is exercised rather than assumed absent.
    std::fs::write(peer.path().join("VERSION"), "2026.7.1\n").unwrap();

    migrate::run_import(
        wcore_config::portability::PeerSource::Hermes,
        &args_for(peer.path()),
    )
    .unwrap();

    let entries = QuarantineStore::for_current_home().entries().unwrap();
    assert!(!entries.is_empty(), "nothing was recorded");
    for e in &entries {
        let p = &e.provenance;
        assert_eq!(p.source_tool, "hermes", "{e:?}");
        assert_eq!(p.source_version.as_deref(), Some("2026.7.1"), "{e:?}");
        assert!(!p.source_path.is_empty(), "{e:?}");
        assert_eq!(p.digest.len(), 64, "digest must be a sha256 hex: {e:?}");
        assert!(
            p.imported_at.ends_with('Z') && p.imported_at.len() >= 20,
            "import time must be RFC3339 UTC: {e:?}"
        );
        assert!(
            !p.source_path.contains('\\'),
            "source path must be normalized: {e:?}"
        );
    }
}

/// Behavior 2: the content digest is domain-separated and stable — same bytes
/// at the same relative path digest identically across runs and platforms.
#[test]
fn t10_digest_is_domain_separated_and_platform_stable() {
    let bytes = b"---\nname: a\n---\nbody" as &[u8];
    let a = item_digest("skills/a/SKILL.md", bytes);
    assert_eq!(a, item_digest("skills\\a\\SKILL.md", bytes));
    assert_eq!(a, item_digest("./skills/a/SKILL.md", bytes));
    assert_eq!(a, item_digest("skills/a/SKILL.md", bytes));
    // Positive half: different bytes and different paths DO differ, so the
    // equalities above are not the equality of a constant.
    assert_ne!(a, item_digest("skills/b/SKILL.md", bytes));
    assert_ne!(a, item_digest("skills/a/SKILL.md", b"other"));
    // Domain separation from the workspace-trust surface prefix.
    assert_ne!(
        PROVENANCE_DOMAIN,
        b"wayland-workspace-executable-surface-v1\0"
    );
    assert_eq!(normalize_relative_path("a\\b\\c"), "a/b/c");
}

/// Behavior 3: selecting a subset by item identity imports exactly that
/// subset, and the identities are the ones the dry-run plan published.
#[test]
#[serial]
fn t11_selecting_a_subset_imports_exactly_that_subset_by_published_identity() {
    let sentinel = Path::new("/tmp/never-created-by-this-test");
    let (_g, _home) = rooted();
    let peer = peer_home_with_fixtures(sentinel, sentinel);

    let published = migrate::published_for(
        wcore_config::portability::PeerSource::Hermes,
        &args_for(peer.path()),
    )
    .unwrap();
    let ids: Vec<String> = published.iter().map(|p| p.identity.clone()).collect();
    assert!(
        ids.contains(&"skill:skills/repo-status".to_string()),
        "{ids:?}"
    );

    let mut args = args_for(peer.path());
    args.select = vec!["skill:skills/repo-status".into()];
    let report = migrate::run_import(wcore_config::portability::PeerSource::Hermes, &args).unwrap();

    assert_eq!(report.discovered, published.len());
    assert_eq!(report.quarantined, 1, "report={report:?}");
    assert_eq!(report.excluded, published.len() - 1);
    assert!(report.balances(), "report={report:?}");

    // A selection naming an identity the plan never published is REFUSED, not
    // silently ignored.
    let mut bad = args_for(peer.path());
    bad.select = vec!["skill:skills/typo".into()];
    let err = migrate::run_import(wcore_config::portability::PeerSource::Hermes, &bad).unwrap_err();
    assert!(
        err.to_string().contains("typo"),
        "an unpublished identity must be refused by name; got {err}"
    );
}

/// Behavior 4: excluding a subset imports everything else.
#[test]
#[serial]
fn t12_excluding_a_subset_imports_everything_else() {
    let sentinel = Path::new("/tmp/never-created-by-this-test");
    let (_g, _home) = rooted();
    let peer = peer_home_with_fixtures(sentinel, sentinel);
    let published = migrate::published_for(
        wcore_config::portability::PeerSource::Hermes,
        &args_for(peer.path()),
    )
    .unwrap();

    let mut args = args_for(peer.path());
    args.exclude = vec!["skill:skills/self-promoting".into()];
    let report = migrate::run_import(wcore_config::portability::PeerSource::Hermes, &args).unwrap();

    assert_eq!(report.excluded, 1, "report={report:?}");
    assert_eq!(report.discovered, published.len());
    assert!(report.balances());
    assert!(
        !QuarantineStore::for_current_home()
            .contains("skill:skills/self-promoting")
            .unwrap(),
        "an excluded item must not be contained either"
    );
    // Positive half: the un-excluded executable sibling WAS contained.
    assert!(
        QuarantineStore::for_current_home()
            .contains("skill:skills/repo-status")
            .unwrap()
    );
}

/// Behavior 5: the conservation invariant holds — imported + quarantined +
/// excluded == discovered, asserted as a NUMBER over the FULL committed
/// Hermes corpus and the FULL committed OpenClaw corpus.
#[test]
#[serial]
fn t13_conservation_holds_over_both_full_committed_corpora() {
    for (source, rel) in [
        (wcore_config::portability::PeerSource::Hermes, "hermes"),
        (wcore_config::portability::PeerSource::OpenClaw, "openclaw"),
    ] {
        let corpus = fixtures().join("portability").join(rel);
        assert!(
            corpus.is_dir(),
            "26-01's committed corpus is missing at {corpus:?} — this assertion \
             would otherwise pass vacuously over an empty tree"
        );
        let (_g, _home) = rooted();
        let report = migrate::run_import(source, &args_for(&corpus)).unwrap();
        assert!(
            report.discovered > 0,
            "{rel}: nothing was discovered, so the balance below would be 0=0"
        );
        assert!(
            report.balances(),
            "{rel}: imported({}) + quarantined({}) + excluded({}) != discovered({})",
            report.imported,
            report.quarantined,
            report.excluded,
            report.discovered
        );
    }
}

/// Behavior 6: an item that cannot be imported for a reason other than user
/// exclusion is a NAMED failure, never a silent drop, and the invariant still
/// balances.
#[test]
fn t14_a_non_exclusion_failure_is_named_and_still_balances() {
    let mut acct = Accounting::over(["a", "b", "c"].map(String::from));
    acct.record("a", Outcome::Imported);
    acct.record("b", Outcome::Excluded);
    // "c" deliberately left without an outcome: the invariant must FAIL.
    assert!(!acct.balances(), "an unaccounted item must not balance");
    assert_eq!(acct.unaccounted(), vec!["c"]);

    acct.record(
        "c",
        Outcome::Quarantined(QuarantineReason::ImportFailed("unreadable source".into())),
    );
    assert!(acct.balances());
    assert_eq!(acct.counts(), (3, 1, 1, 1));
    assert_eq!(acct.failures(), vec![("c", "unreadable source")]);
    // The selection layer refuses an unknown identity by name.
    assert_eq!(
        Selection::including(["nope"])
            .resolve(&["a".to_string()])
            .unwrap_err(),
        SelectError::UnknownIdentity("nope".into())
    );
}

/// Behavior 7: export emits a portable corpus carrying provenance, and still
/// excludes secrets by default exactly as the existing tests require.
#[test]
#[serial]
fn t15_export_carries_provenance_and_still_excludes_secrets_by_default() {
    let root = tempfile::tempdir().unwrap();
    let _g = EnvGuard::set(&[("WAYLAND_PROFILES_ROOT", Some(root.path().to_str().unwrap()))]);
    let dir = wcore_config::profile::create_profile("exp", None).unwrap();
    std::fs::write(dir.join("config.toml"), "x = 1\n").unwrap();
    std::fs::write(
        dir.join("credentials.toml"),
        "token = \"CANARY-SECRET-1\"\n",
    )
    .unwrap();
    std::fs::create_dir_all(dir.join("oauth")).unwrap();
    std::fs::write(dir.join("oauth").join("t.json"), "CANARY-SECRET-2").unwrap();

    let out = tempfile::tempdir().unwrap();
    let dst = out.path().join("corpus");
    wcore_cli::profile::run(wcore_cli::profile::ProfileCmd::Export {
        name: "exp".into(),
        out: Some(dst.clone()),
        include_secrets: false,
        select: Vec::new(),
        exclude: Vec::new(),
    })
    .unwrap();

    // The existing default is intact.
    assert!(
        !dst.join("credentials.toml").exists(),
        "secret file exported"
    );
    assert!(!dst.join("oauth").exists(), "secret dir exported");
    // Positive half: the non-secret file DID export, so the absences are a
    // filter result, not an empty export.
    assert!(dst.join("config.toml").is_file());

    let prov = std::fs::read_to_string(dst.join(wcore_config::profile::EXPORT_PROVENANCE_FILE))
        .expect("export must carry provenance");
    let doc = ProvenanceDocument::from_json(&prov).unwrap();
    assert!(doc.get("config.toml").is_some(), "{prov}");
    assert!(
        doc.get("credentials.toml").is_none(),
        "provenance must not name an entry the export excluded"
    );
    assert!(
        !prov.contains("CANARY-SECRET"),
        "provenance leaked a secret"
    );
}

/// Behavior 8: export followed by import round-trips the non-secret corpus
/// with provenance preserved, and the round-tripped executable content is
/// STILL QUARANTINED rather than arriving promoted.
#[test]
#[serial]
fn t16_export_import_round_trip_preserves_provenance_and_does_not_launder_quarantine() {
    let (_hg, home) = rooted();
    let root = tempfile::tempdir().unwrap();
    let _g = EnvGuard::set(&[("WAYLAND_PROFILES_ROOT", Some(root.path().to_str().unwrap()))]);

    let sentinel = home.path().join("must-not-exist");
    let dir = wcore_config::profile::create_profile("rt", None).unwrap();
    std::fs::write(dir.join("config.toml"), "x = 1\n").unwrap();
    let sk = dir.join("skills").join("repo-status");
    std::fs::create_dir_all(&sk).unwrap();
    std::fs::write(
        sk.join("SKILL.md"),
        fixture_body("skills/repo-status/SKILL.md", &sentinel, &sentinel),
    )
    .unwrap();

    let out = tempfile::tempdir().unwrap();
    let dst = out.path().join("corpus");
    wcore_cli::profile::run(wcore_cli::profile::ProfileCmd::Export {
        name: "rt".into(),
        out: Some(dst.clone()),
        include_secrets: false,
        select: Vec::new(),
        exclude: Vec::new(),
    })
    .unwrap();
    // The corpus carries the executable body — otherwise the round trip below
    // would prove nothing.
    assert!(dst.join("skills/repo-status/SKILL.md").is_file());

    wcore_cli::profile::run(wcore_cli::profile::ProfileCmd::Import {
        path: dst.clone(),
        new_name: Some("rt2".into()),
    })
    .unwrap();

    let landed = wcore_config::profile::profile_dir("rt2").unwrap();
    assert!(
        landed
            .join(wcore_config::profile::EXPORT_PROVENANCE_FILE)
            .is_file(),
        "provenance must survive the round trip"
    );
    assert!(
        !landed.join("skills").join("repo-status").exists(),
        "an export/import round trip must NOT land executable content live — \
         that would launder quarantine into a promotion nobody granted"
    );
    assert!(
        QuarantineStore::for_current_home()
            .entries()
            .unwrap()
            .iter()
            .any(|e| e.id.contains("repo-status")),
        "…and the content must be contained, not merely deleted"
    );
    // Positive half: the non-executable file DID round-trip, so the absence
    // above is containment, not a failed import.
    assert!(landed.join("config.toml").is_file());
    assert!(
        !sentinel.exists(),
        "the payload must not have run at any point"
    );
}

// ===========================================================================
// Drift guard — the mirrored ceilings must equal the originals
// ===========================================================================

/// The quarantine ceilings MIRROR `workspace_trust`'s. This reads that file at
/// test time and fails if the two ever disagree, so "mirrored" is a checked
/// property rather than a comment. It also fails if a ceiling is raised to
/// admit a realistic 540-directory import, which is expressly forbidden.
#[test]
fn t17_quarantine_ceilings_match_workspace_trust_exactly() {
    let src = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("wcore-config")
        .join("src")
        .join("workspace_trust.rs");
    let text = std::fs::read_to_string(&src).unwrap_or_else(|e| {
        panic!("cannot read {src:?}: {e} — this guard must not pass vacuously")
    });
    for (needle, mirrored) in [
        (
            "const MAX_EXECUTABLE_FILES: usize = 512;",
            MAX_QUARANTINE_FILES as u64,
        ),
        (
            "const MAX_EXECUTABLE_FILE_BYTES: u64 = 4 * 1024 * 1024;",
            MAX_QUARANTINE_FILE_BYTES,
        ),
        (
            "const MAX_EXECUTABLE_TOTAL_BYTES: u64 = 32 * 1024 * 1024;",
            MAX_QUARANTINE_TOTAL_BYTES,
        ),
    ] {
        assert!(
            text.contains(needle),
            "workspace_trust.rs no longer declares `{needle}` — either it moved \
             (and this mirror is now unanchored) or a ceiling was RAISED, which \
             this plan forbids. mirrored value here = {mirrored}"
        );
    }
    assert_eq!(MAX_QUARANTINE_FILES, 512);
    assert_eq!(MAX_QUARANTINE_FILE_BYTES, 4 * 1024 * 1024);
    assert_eq!(MAX_QUARANTINE_TOTAL_BYTES, 32 * 1024 * 1024);
}

/// The ceiling COLLISION, measured rather than eyeballed: a real Hermes home
/// carries 540 skill directories, and 540 exceeds the 512-file ceiling. This
/// records the collision as a fact; it is a finding with a severity, never a
/// reason to raise the limit.
#[test]
fn t18_the_512_file_ceiling_refuses_a_realistic_540_directory_surface() {
    const REAL_INSTALL_SKILL_DIRS: usize = 540;
    assert!(
        REAL_INSTALL_SKILL_DIRS > MAX_QUARANTINE_FILES,
        "if this ever passes, the ceiling was raised to admit the import"
    );
    let dir = tempfile::tempdir().unwrap();
    let src = dir.path().join("surface");
    std::fs::create_dir_all(&src).unwrap();
    for i in 0..REAL_INSTALL_SKILL_DIRS {
        std::fs::write(src.join(format!("s{i}.md")), "x").unwrap();
    }
    let store = QuarantineStore::new(dir.path().join("store"));
    let err = store
        .admit(&QuarantineRequest {
            id: "skill:realistic".into(),
            reason: ExecutableReason::SkillShellDirective,
            source_dir: Some(src),
            inline: None,
            source_tool: "hermes".into(),
            source_version: None,
            source_path: "skills".into(),
            promote_as: "realistic".into(),
        })
        .expect_err("a 540-file surface must be refused by the mirrored ceiling");
    assert!(matches!(err, quarantine::QuarantineError::SurfaceTooLarge));
}

// ===========================================================================
// TASK 3 — the PAIRED live inertness proof against the REAL binary
// ===========================================================================

fn binary() -> &'static str {
    env!("CARGO_BIN_EXE_wayland-core")
}

/// Drive the REAL binary through an agent turn that calls the `Skill` tool for
/// `skill_name`, against a scripted mock provider.
///
/// Returns every stdout line the run emitted. The legs assert on the sentinel
/// file (the observable effect) AND on these lines, because a binary that
/// crashed at boot would also leave the sentinel absent — and that would be a
/// negative leg measuring a dead process rather than containment.
///
/// Waits for `sentinel` to appear, up to a bounded deadline, and returns as
/// soon as it does. The negative leg therefore waits the FULL window before
/// concluding absence.
fn drive_skill_turn(home: &Path, skill_name: &str, sentinel: &Path) -> Vec<String> {
    use std::io::{BufRead, BufReader, Write};
    use std::time::{Duration, Instant};

    let rt = tokio::runtime::Runtime::new().expect("tokio runtime");
    let server = rt.block_on(
        support::mock_llm::MockLlm::new()
            .tool_use("Skill", serde_json::json!({ "skill": skill_name }))
            .text("done")
            .start(),
    );

    let mut toml =
        String::from("[default]\nprovider = \"anthropic\"\nmodel = \"claude-sonnet-4-20250514\"\n");
    toml.push_str(
        "\n[providers.anthropic]\napi_key = \"sk-ant-harness-not-real-key-0000000000\"\n",
    );
    toml.push_str(&format!("base_url = \"{}\"\n", server.uri()));
    // OPERATOR-side approval settings. Identical in BOTH legs, so the only
    // thing that differs between them is the promotion.
    toml.push_str("\n[tools]\nauto_approve = true\n");
    toml.push_str(&format!("\n[tools.skills]\nallow = [\"{skill_name}\"]\n"));
    std::fs::write(home.join("config.toml"), toml).expect("write config.toml");

    let mut cmd = std::process::Command::new(binary());
    cmd.args(["--json-stream", "--force", "--provider", "anthropic"])
        .current_dir(home)
        .env("WAYLAND_HOME", home)
        .env("HOME", home)
        .env("TERM", "dumb");
    for key in [
        "API_KEY",
        "ANTHROPIC_API_KEY",
        "OPENAI_API_KEY",
        "GEMINI_API_KEY",
        "GOOGLE_API_KEY",
        "OPENROUTER_API_KEY",
    ] {
        cmd.env_remove(key);
    }
    // The engine refuses to start a turn without confidential recovery storage
    // ("no OS keyring was usable and no encrypted credentials vault is
    // unlocked"), and a hetzner test box has no keyring. The shared support
    // helper hands the child an ephemeral encrypted vault over an inherited
    // descriptor — the same thing every other packaged-binary test in this
    // crate does. Without it the turn dies before reaching the Skill tool, and
    // BOTH legs would then be measuring a dead engine rather than containment.
    let vault = support::vault::configure_process(&mut cmd);
    let child = cmd
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn();
    drop(vault);
    let mut child = child.expect("spawn wayland-core --json-stream");

    let mut stdin = child.stdin.take().expect("stdin");
    let stdout = child.stdout.take().expect("stdout");
    let (tx, rx) = std::sync::mpsc::channel::<String>();
    std::thread::spawn(move || {
        let reader = BufReader::new(stdout);
        for line in reader.lines().map_while(Result::ok) {
            if tx.send(line).is_err() {
                break;
            }
        }
    });
    writeln!(
        stdin,
        "{{\"type\":\"message\",\"msg_id\":\"1\",\"content\":\"use the {skill_name} skill\"}}"
    )
    .expect("write message");

    // Bounded drain: wait for the sentinel to appear, and keep the stdout pipe
    // drained so the child never blocks on a full pipe. Returning early on the
    // sentinel keeps the POSITIVE leg fast; the NEGATIVE leg waits the whole
    // window before concluding absence.
    let deadline = Instant::now() + Duration::from_secs(45);
    let mut lines = Vec::new();
    while Instant::now() < deadline {
        if sentinel.exists() {
            break;
        }
        if let Ok(line) = rx.recv_timeout(Duration::from_millis(200)) {
            lines.push(line);
        }
    }
    let _ = writeln!(stdin, "{{\"type\":\"stop\"}}");
    // Drain whatever is still buffered before tearing the child down.
    while let Ok(line) = rx.recv_timeout(Duration::from_millis(200)) {
        lines.push(line);
    }
    let _ = child.kill();
    let _ = child.wait();
    drop(server);
    drop(rt);
    lines
}

/// NEGATIVE LEG. After importing the peer corpus containing the payload,
/// driving the REAL binary through an agent turn that would use skills leaves
/// the sentinel ABSENT.
///
/// Three assertions TOGETHER, because any one alone is vacuous: the import
/// reported a non-zero item count, the payload item is reported quarantined,
/// and the sentinel does not exist. The first two are what stop "nothing was
/// imported" and "the item vanished" from reading as containment.
#[test]
#[serial]
fn t19_live_negative_leg_quarantined_payload_does_not_execute() {
    let (_g, home) = rooted();
    // The sentinel lives INSIDE the per-run home, which is both the session
    // workspace and a tempdir unique to this run. Inside the workspace because
    // the real binary runs its shell under the sandbox, which correctly refuses
    // writes outside the workspace — a sentinel out there would be blocked by
    // the SANDBOX, and the positive control would then be measuring the sandbox
    // rather than the quarantine boundary.
    let sentinel = home.path().join("negative-sentinel");
    assert!(
        !sentinel.exists(),
        "the per-run sentinel must be absent BEFORE the leg begins"
    );

    let peer = peer_home_with_fixtures(&sentinel, &sentinel);
    let report = migrate::run_import(
        wcore_config::portability::PeerSource::Hermes,
        &args_for(peer.path()),
    )
    .unwrap();

    // (1) the import actually happened.
    assert!(
        report.discovered > 0,
        "an import that discovered nothing cannot pass as containment: {report:?}"
    );
    // (2) the payload item is reported QUARANTINED.
    assert!(
        QuarantineStore::for_current_home()
            .contains("skill:skills/repo-status")
            .unwrap(),
        "the payload must be reported quarantined, not vanished: {report:?}"
    );

    let lines = drive_skill_turn(home.path(), "repo-status", &sentinel);

    // (3) the TURN ACTUALLY RAN and reached the Skill tool. Without this, a
    // binary that crashed at boot would leave the sentinel absent too, and the
    // leg would be measuring a dead process rather than containment.
    let stream = lines.join("\n");
    assert!(
        stream.contains("repo-status"),
        "the driven turn never reached the Skill tool for the payload, so the \
         absence below would measure a turn that never happened. stream:\n{stream}"
    );
    assert!(
        stream.contains("not found") || stream.contains("Available skills"),
        "the Skill tool must report the quarantined skill as unavailable — that \
         is what containment looks like from the agent's side. stream:\n{stream}"
    );

    // (4) the sentinel is ABSENT.
    assert!(
        !sentinel.exists(),
        "the quarantined payload EXECUTED during a real agent turn — containment failed"
    );
}

/// POSITIVE CONTROL, same payload, same driven turn, differing ONLY by an
/// explicit operator promotion. Without this leg the negative leg carries no
/// information, because absence would be equally consistent with the payload
/// never being loaded, never being parsed, or never being discovered.
#[test]
#[serial]
fn t20_live_positive_control_same_payload_executes_once_promoted() {
    let (_g, home) = rooted();
    // Same placement rule as the negative leg — see there.
    let sentinel = home.path().join("positive-sentinel");
    assert!(
        !sentinel.exists(),
        "the per-run sentinel must be absent BEFORE the leg begins"
    );

    let peer = peer_home_with_fixtures(&sentinel, &sentinel);
    migrate::run_import(
        wcore_config::portability::PeerSource::Hermes,
        &args_for(peer.path()),
    )
    .unwrap();

    // THE EXPLICIT OPERATOR ACTION — the one and only difference from the
    // negative leg. Driven through the real CLI verb, not the library.
    migrate::run(MigrateCmd::Promote(PromoteArgs {
        ids: vec!["skill:skills/repo-status".to_string()],
        all: false,
    }))
    .unwrap();
    assert!(
        wcore_config::config::wayland_config_dir()
            .join("skills")
            .join("repo-status")
            .join("SKILL.md")
            .is_file(),
        "promotion must have placed the payload on the load path"
    );

    let lines = drive_skill_turn(home.path(), "repo-status", &sentinel);

    assert!(
        sentinel.exists(),
        "the PROMOTED payload did NOT execute — so the negative leg measured a \
         payload that never loads rather than containment, and proves nothing. \
         sentinel={} stream:\n{}",
        sentinel.display(),
        lines.join("\n")
    );
}

// ===========================================================================
// Provenance document behaviour used by the scale measurement
// ===========================================================================

#[test]
fn t21_provenance_document_is_deterministic_and_key_ordered() {
    let mut doc = ProvenanceDocument::new();
    let mut expected = BTreeMap::new();
    for id in ["skill:z", "skill:a", "skill:m"] {
        let p = Provenance::with_time("hermes", None, id, "d", "2026-01-01T00:00:00Z");
        expected.insert(id.to_string(), p.clone());
        doc.insert(id, p);
    }
    let a = doc.to_json().unwrap();
    assert_eq!(a, doc.to_json().unwrap());
    assert!(a.find("skill:a").unwrap() < a.find("skill:m").unwrap());
    assert!(a.find("skill:m").unwrap() < a.find("skill:z").unwrap());
    assert_eq!(doc.len(), 3);
}
