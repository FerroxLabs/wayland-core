//! Quiesced snapshot lease — mechanism tests (wayland#896).
//!
//! Every rejection reason the contract names gets its own test, and every one
//! of those tests carries a positive control in the same run: a refusal that
//! cannot be shown to be absent under the opposite input proves nothing about
//! the guard, only about the harness.
//!
//! The crash arm kills a REAL holder process with SIGKILL/TerminateProcess. A
//! modelled crash — dropping a handle, deleting a file by hand — would exercise
//! the path the code already takes, not the one a dead process leaves behind.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Child, Command};
use std::time::{Duration, Instant};

use serial_test::serial;
use tempfile::TempDir;
use wcore_config::quiesce::{
    self, LeaseRequest, LeaseScope, MIN_LEASE_TTL_MS, ProfileSelector, QuiesceError,
    ReleaseVerdict, RootIdentity,
};

/// Hermetic profile world: its own default home and its own profiles root, so
/// nothing here can see or disturb the developer's real `~/.wayland`.
struct World {
    _dir: TempDir,
    home: PathBuf,
    profiles: PathBuf,
}

impl World {
    fn new(named: &[&str]) -> Self {
        let dir = TempDir::new().expect("tempdir");
        let home = dir.path().join("home");
        let profiles = dir.path().join("profiles");
        fs::create_dir_all(&home).expect("home");
        fs::create_dir_all(&profiles).expect("profiles");
        fs::write(home.join("config.toml"), b"[default]\nmodel = \"x\"\n").expect("config");
        fs::create_dir_all(home.join("oauth")).expect("oauth dir");
        fs::write(home.join("oauth").join("chatgpt.json"), b"{}").expect("oauth");
        for name in named {
            let profile = profiles.join(name);
            fs::create_dir_all(&profile).expect("profile dir");
            fs::write(profile.join("config.toml"), format!("# {name}\n")).expect("profile config");
        }
        unsafe {
            std::env::set_var("WAYLAND_HOME", &home);
            std::env::set_var("WAYLAND_PROFILES_ROOT", &profiles);
        }
        Self {
            _dir: dir,
            home,
            profiles,
        }
    }
}

impl Drop for World {
    fn drop(&mut self) {
        unsafe {
            std::env::remove_var("WAYLAND_HOME");
            std::env::remove_var("WAYLAND_PROFILES_ROOT");
        }
    }
}

fn request(lease_id: &str, scope: LeaseScope) -> LeaseRequest {
    LeaseRequest {
        lease_id: lease_id.to_string(),
        owner: "session-test".to_string(),
        scope,
        ttl_ms: 60_000,
    }
}

fn all_scope() -> LeaseScope {
    LeaseScope {
        include_default: true,
        profiles: ProfileSelector::All,
    }
}

// --- happy path ------------------------------------------------------------

#[test]
#[serial]
fn a_lease_covers_the_default_home_and_every_named_profile() {
    let world = World::new(&["work", "personal"]);
    let grant = quiesce::acquire(&request("lease-1", all_scope())).expect("grant");

    let identities: Vec<RootIdentity> = grant
        .record
        .roots
        .iter()
        .map(|root| root.identity.clone())
        .collect();
    assert_eq!(
        identities,
        vec![
            RootIdentity::Default,
            RootIdentity::Named {
                name: "personal".into()
            },
            RootIdentity::Named {
                name: "work".into()
            },
        ],
        "coverage must enumerate every named profile, not assume there is one"
    );
    assert!(grant.record.epoch.starts_with("sha256:"));
    assert!(!grant.idempotent_replay);
    assert_eq!(grant.record.roots[0].path, world.home);

    let receipt = quiesce::release("lease-1", &grant.record.epoch).expect("release");
    assert_eq!(receipt.verdict, ReleaseVerdict::Clean);
    assert_eq!(receipt.epoch_at_release, grant.record.epoch);
    assert!(
        quiesce::status().expect("status").held.is_none(),
        "release must free the control plane"
    );
}

#[test]
#[serial]
fn the_control_plane_is_never_enumerated_as_a_profile() {
    let world = World::new(&["work"]);
    let grant = quiesce::acquire(&request("lease-1", all_scope())).expect("grant");
    assert!(
        quiesce::control_root().starts_with(&world.profiles),
        "the control plane must live at the profiles root"
    );
    assert!(
        quiesce::control_root().exists(),
        "acquire must have created the control plane"
    );
    // The positive control is `work`: the enumeration DOES see a real profile
    // sitting beside the control directory, so a zero result for `.quiesce`
    // means the filter works rather than that nothing was scanned.
    let names: Vec<String> = grant
        .record
        .roots
        .iter()
        .filter_map(|root| match &root.identity {
            RootIdentity::Named { name } => Some(name.clone()),
            RootIdentity::Default => None,
        })
        .collect();
    assert_eq!(names, vec!["work".to_string()]);
    let _ = quiesce::release("lease-1", &grant.record.epoch);
}

// --- rejection: partial coverage -------------------------------------------

#[test]
#[serial]
fn a_missing_named_profile_is_partial_coverage_not_a_quiet_subset() {
    let _world = World::new(&["work"]);
    let scope = LeaseScope {
        include_default: true,
        profiles: ProfileSelector::Named(vec!["work".into(), "archive".into()]),
    };
    match quiesce::acquire(&request("lease-1", scope)) {
        Err(QuiesceError::PartialCoverage { missing }) => {
            assert_eq!(missing, vec!["profile:archive".to_string()]);
        }
        other => panic!("expected PartialCoverage, got {other:?}"),
    }
    // Positive control: the SAME shape of request over profiles that all exist
    // is granted, so the refusal above is about coverage and not about the
    // request being rejected wholesale.
    let ok_scope = LeaseScope {
        include_default: true,
        profiles: ProfileSelector::Named(vec!["work".into()]),
    };
    let grant = quiesce::acquire(&request("lease-2", ok_scope)).expect("control must be granted");
    let _ = quiesce::release("lease-2", &grant.record.epoch);
}

#[test]
#[serial]
fn a_request_that_covers_nothing_is_refused() {
    let _world = World::new(&[]);
    let scope = LeaseScope {
        include_default: false,
        profiles: ProfileSelector::All,
    };
    // No named profiles exist and the default is excluded, so this covers the
    // empty set. Reporting success would be the fail-open answer.
    match quiesce::acquire(&request("lease-1", scope)) {
        Err(QuiesceError::PartialCoverage { missing }) => {
            assert_eq!(missing, vec!["<request covers no root>".to_string()]);
        }
        other => panic!("expected PartialCoverage, got {other:?}"),
    }
    // Positive control: turning the default back on covers something and is
    // granted, in the same world.
    let grant = quiesce::acquire(&request("lease-2", all_scope())).expect("control granted");
    let _ = quiesce::release("lease-2", &grant.record.epoch);
}

#[test]
#[serial]
fn a_missing_default_home_is_partial_coverage() {
    let world = World::new(&["work"]);
    fs::remove_dir_all(&world.home).expect("remove default home");
    match quiesce::acquire(&request("lease-1", all_scope())) {
        Err(QuiesceError::PartialCoverage { missing }) => {
            assert_eq!(missing, vec!["default".to_string()]);
        }
        other => panic!("expected PartialCoverage, got {other:?}"),
    }
    // Positive control: the named profile alone still resolves, so the world
    // is not simply broken.
    let scope = LeaseScope {
        include_default: false,
        profiles: ProfileSelector::All,
    };
    let grant = quiesce::acquire(&request("lease-2", scope)).expect("control granted");
    let _ = quiesce::release("lease-2", &grant.record.epoch);
}

// --- rejection: concurrent capture ------------------------------------------

#[test]
#[serial]
fn a_second_capture_is_refused_while_a_lease_is_live() {
    let _world = World::new(&["work"]);
    let first = quiesce::acquire(&request("lease-1", all_scope())).expect("first grant");
    match quiesce::acquire(&request("lease-2", all_scope())) {
        Err(QuiesceError::ConcurrentCapture {
            holder_lease_id,
            expires_unix_ms,
        }) => {
            assert_eq!(holder_lease_id, "lease-1");
            assert_eq!(expires_unix_ms, first.record.expires_unix_ms);
        }
        other => panic!("expected ConcurrentCapture, got {other:?}"),
    }
    // Positive control: once the first lease is released the same second
    // request succeeds, so the refusal was the lease and not the request.
    quiesce::release("lease-1", &first.record.epoch).expect("release");
    let second = quiesce::acquire(&request("lease-2", all_scope())).expect("control granted");
    let _ = quiesce::release("lease-2", &second.record.epoch);
}

// --- rejection: stale lease -------------------------------------------------

#[test]
#[serial]
fn releasing_with_the_wrong_epoch_is_refused_and_does_not_free_the_lease() {
    let _world = World::new(&["work"]);
    let grant = quiesce::acquire(&request("lease-1", all_scope())).expect("grant");
    match quiesce::release("lease-1", "sha256:notthegrantedepoch") {
        Err(QuiesceError::StaleLease { lease_id, detail }) => {
            assert_eq!(lease_id, "lease-1");
            assert!(detail.contains("epoch echo"), "detail was {detail}");
        }
        other => panic!("expected StaleLease, got {other:?}"),
    }
    // The load-bearing half: a stale actor must not have freed a live lease.
    let held = quiesce::status().expect("status").held.expect("still held");
    assert_eq!(held.lease_id, "lease-1");
    // Positive control: the correct epoch still releases it.
    quiesce::release("lease-1", &grant.record.epoch).expect("control release");
}

#[test]
#[serial]
fn reusing_a_live_lease_id_under_a_different_scope_is_stale_not_a_resize() {
    let _world = World::new(&["work", "personal"]);
    let grant = quiesce::acquire(&request("lease-1", all_scope())).expect("grant");
    let narrower = LeaseScope {
        include_default: true,
        profiles: ProfileSelector::Named(vec!["work".into()]),
    };
    match quiesce::acquire(&request("lease-1", narrower)) {
        Err(QuiesceError::StaleLease { detail, .. }) => {
            assert!(detail.contains("coverage scope"), "detail was {detail}");
        }
        other => panic!("expected StaleLease, got {other:?}"),
    }
    // Positive control: the SAME id with the SAME scope is the idempotent
    // replay, so the refusal above is about the scope change alone.
    let replay = quiesce::acquire(&request("lease-1", all_scope())).expect("replay");
    assert!(replay.idempotent_replay);
    let _ = quiesce::release("lease-1", &grant.record.epoch);
}

#[test]
#[serial]
fn releasing_an_unknown_lease_is_refused() {
    let _world = World::new(&["work"]);
    match quiesce::release("lease-ghost", "sha256:whatever") {
        Err(QuiesceError::UnknownLease { lease_id }) => assert_eq!(lease_id, "lease-ghost"),
        other => panic!("expected UnknownLease, got {other:?}"),
    }
    // Positive control: a lease that IS held releases through the same call.
    let grant = quiesce::acquire(&request("lease-1", all_scope())).expect("grant");
    quiesce::release("lease-1", &grant.record.epoch).expect("control release");
}

#[test]
#[serial]
fn a_lease_held_by_someone_else_cannot_be_released_by_id() {
    let _world = World::new(&["work"]);
    let grant = quiesce::acquire(&request("lease-1", all_scope())).expect("grant");
    match quiesce::release("lease-2", &grant.record.epoch) {
        Err(QuiesceError::UnknownLease { lease_id }) => assert_eq!(lease_id, "lease-2"),
        other => panic!("expected UnknownLease, got {other:?}"),
    }
    assert!(
        quiesce::status().expect("status").held.is_some(),
        "an unrelated id must not free a live lease"
    );
    quiesce::release("lease-1", &grant.record.epoch).expect("control release");
}

// --- rejection: invalid request --------------------------------------------

#[test]
#[serial]
fn an_out_of_range_ttl_is_refused() {
    let _world = World::new(&["work"]);
    for ttl in [0_u64, 24 * 60 * 60 * 1_000] {
        let mut req = request("lease-1", all_scope());
        req.ttl_ms = ttl;
        assert!(
            matches!(quiesce::acquire(&req), Err(QuiesceError::InvalidRequest(_))),
            "ttl {ttl} must be refused"
        );
    }
    // Positive control: a ttl inside the window is granted.
    let mut req = request("lease-1", all_scope());
    req.ttl_ms = MIN_LEASE_TTL_MS;
    let grant = quiesce::acquire(&req).expect("control granted");
    let _ = quiesce::release("lease-1", &grant.record.epoch);
}

#[test]
#[serial]
fn a_hostile_lease_id_is_refused() {
    let _world = World::new(&["work"]);
    for hostile in ["", "../escape", "a/b", "a b", &"x".repeat(129)] {
        assert!(
            matches!(
                quiesce::acquire(&request(hostile, all_scope())),
                Err(QuiesceError::InvalidRequest(_))
            ),
            "lease id {hostile:?} must be refused"
        );
    }
    let grant = quiesce::acquire(&request("lease-1", all_scope())).expect("control granted");
    let _ = quiesce::release("lease-1", &grant.record.epoch);
}

// --- write detection --------------------------------------------------------

#[test]
#[serial]
fn a_write_under_the_lease_lands_on_a_mutated_verdict() {
    let world = World::new(&["work"]);
    let grant = quiesce::acquire(&request("lease-1", all_scope())).expect("grant");
    fs::write(world.home.join("new-state.json"), b"{}").expect("write under lease");
    let receipt = quiesce::release("lease-1", &grant.record.epoch).expect("release");
    assert_eq!(receipt.verdict, ReleaseVerdict::Mutated);
    assert_ne!(receipt.epoch_at_release, receipt.epoch_at_acquire);
}

#[test]
#[serial]
fn a_same_length_rewrite_is_detected() {
    // The case metadata alone cannot see. `config.toml` keeps its byte length,
    // so a size-and-mtime epoch could easily call this clean inside one
    // timestamp tick — and a false clean is the one error this contract may
    // not make.
    let world = World::new(&["work"]);
    let target = world.home.join("config.toml");
    let original = fs::read(&target).expect("read");
    let grant = quiesce::acquire(&request("lease-1", all_scope())).expect("grant");
    let mut rewritten = original.clone();
    let last = rewritten.len() - 2;
    rewritten[last] = if rewritten[last] == b'x' { b'y' } else { b'x' };
    assert_eq!(
        rewritten.len(),
        original.len(),
        "the rewrite must be same-length"
    );
    assert_ne!(rewritten, original, "the rewrite must change content");
    fs::write(&target, &rewritten).expect("rewrite");
    let receipt = quiesce::release("lease-1", &grant.record.epoch).expect("release");
    assert_eq!(receipt.verdict, ReleaseVerdict::Mutated);
}

#[test]
#[serial]
fn rewriting_identical_bytes_is_not_a_mutation() {
    // The converse pin. Rewriting a file with its own bytes moves its mtime and
    // nothing else; a clean verdict here proves the epoch is taken over CONTENT
    // and not over the timestamp, which is what makes the test above meaningful
    // rather than an accident of ordering.
    let world = World::new(&["work"]);
    let target = world.home.join("config.toml");
    let original = fs::read(&target).expect("read");
    let grant = quiesce::acquire(&request("lease-1", all_scope())).expect("grant");
    std::thread::sleep(Duration::from_millis(20));
    fs::write(&target, &original).expect("rewrite identical");
    let receipt = quiesce::release("lease-1", &grant.record.epoch).expect("release");
    assert_eq!(receipt.verdict, ReleaseVerdict::Clean);
}

#[test]
#[serial]
fn a_profile_deleted_under_the_lease_is_a_mutation_not_an_error() {
    let world = World::new(&["work"]);
    let grant = quiesce::acquire(&request("lease-1", all_scope())).expect("grant");
    fs::remove_dir_all(world.profiles.join("work")).expect("delete profile");
    let receipt = quiesce::release("lease-1", &grant.record.epoch).expect("release must succeed");
    assert_eq!(receipt.verdict, ReleaseVerdict::Mutated);
}

// --- idempotency ------------------------------------------------------------

#[test]
#[serial]
fn a_repeated_acquire_returns_the_same_grant() {
    let world = World::new(&["work"]);
    let first = quiesce::acquire(&request("lease-1", all_scope())).expect("first");
    // A write between the two calls is the trap: a retry that recomputed the
    // epoch would answer a different question than the call it repeats, and the
    // host would then release against an epoch its capture never saw.
    fs::write(world.home.join("drift.json"), b"{}").expect("write");
    let second = quiesce::acquire(&request("lease-1", all_scope())).expect("second");
    assert!(second.idempotent_replay);
    assert_eq!(second.record.epoch, first.record.epoch);
    assert_eq!(
        second.record.acquired_unix_ms,
        first.record.acquired_unix_ms
    );
    assert_eq!(second.record.expires_unix_ms, first.record.expires_unix_ms);
    let receipt = quiesce::release("lease-1", &first.record.epoch).expect("release");
    assert_eq!(receipt.verdict, ReleaseVerdict::Mutated);
}

// --- expiry -----------------------------------------------------------------

#[test]
#[serial]
fn a_lapsed_lease_is_reclaimed_and_reported() {
    let _world = World::new(&["work"]);
    let mut req = request("lease-1", all_scope());
    req.ttl_ms = MIN_LEASE_TTL_MS;
    let first = quiesce::acquire(&req).expect("grant");

    // Before expiry the control plane is genuinely held — the positive control
    // for the reclaim below.
    assert!(matches!(
        quiesce::acquire(&request("lease-2", all_scope())),
        Err(QuiesceError::ConcurrentCapture { .. })
    ));

    std::thread::sleep(Duration::from_millis(MIN_LEASE_TTL_MS + 250));
    let second = quiesce::acquire(&request("lease-2", all_scope())).expect("reclaimed grant");
    let reclaimed = second.reclaimed.expect("the expiry must be reported");
    assert_eq!(reclaimed.lease_id, "lease-1");
    assert_eq!(reclaimed.epoch, first.record.epoch);
    let _ = quiesce::release("lease-2", &second.record.epoch);
}

#[test]
#[serial]
fn releasing_after_expiry_is_refused_rather_than_certified_clean() {
    let _world = World::new(&["work"]);
    let mut req = request("lease-1", all_scope());
    req.ttl_ms = MIN_LEASE_TTL_MS;
    let grant = quiesce::acquire(&req).expect("grant");
    std::thread::sleep(Duration::from_millis(MIN_LEASE_TTL_MS + 250));
    match quiesce::release("lease-1", &grant.record.epoch) {
        Err(QuiesceError::StaleLease { detail, .. }) => {
            assert!(detail.contains("expired"), "detail was {detail}");
        }
        other => panic!("expected StaleLease, got {other:?}"),
    }
    // And the control plane is free again, not wedged by the lapsed record.
    let next = quiesce::acquire(&request("lease-2", all_scope())).expect("control granted");
    let _ = quiesce::release("lease-2", &next.record.epoch);
}

#[test]
#[serial]
fn an_unparsable_lease_record_does_not_wedge_the_control_plane() {
    let _world = World::new(&["work"]);
    let control = quiesce::control_root();
    fs::create_dir_all(&control).expect("control dir");
    fs::write(control.join("lease.json"), b"{ this is not a lease").expect("corrupt");
    // Such a record has no expiry to reach, so leaving it would wedge capture
    // forever.
    let grant = quiesce::acquire(&request("lease-1", all_scope())).expect("must reclaim");
    let reclaimed = grant.reclaimed.expect("the reclaim must be reported");
    assert_eq!(reclaimed.lease_id, "<unparsable>");
    let _ = quiesce::release("lease-1", &grant.record.epoch);
}

// --- crash arm --------------------------------------------------------------

/// The lease holder for [`a_killed_holder_leaves_a_reclaimable_lease`].
///
/// Inert unless the parent asks for it by env var, so a normal test run does
/// not park a process forever.
#[test]
fn crash_arm_lease_holder() {
    let Ok(marker) = std::env::var("WL_QUIESCE_HOLDER_MARKER") else {
        return;
    };
    let request = LeaseRequest {
        lease_id: "lease-holder".to_string(),
        owner: "session-holder".to_string(),
        scope: LeaseScope {
            include_default: true,
            profiles: ProfileSelector::All,
        },
        ttl_ms: MIN_LEASE_TTL_MS,
    };
    let grant = quiesce::acquire(&request).expect("holder must acquire");
    fs::write(&marker, grant.record.epoch.as_bytes()).expect("marker");
    // Park. The parent kills this process uncatchably; nothing below runs, and
    // in particular no destructor gets a chance to release the lease.
    loop {
        std::thread::sleep(Duration::from_secs(3_600));
    }
}

fn wait_for(path: &Path, limit: Duration) -> bool {
    let deadline = Instant::now() + limit;
    while Instant::now() < deadline {
        if path.exists() {
            return true;
        }
        std::thread::sleep(Duration::from_millis(25));
    }
    false
}

fn reap(mut child: Child) {
    let _ = child.kill();
    let _ = child.wait();
}

#[test]
#[serial]
fn a_killed_holder_leaves_a_reclaimable_lease() {
    let world = World::new(&["work"]);
    let marker = world
        .profiles
        .parent()
        .expect("tmp root")
        .join("held.marker");

    let mut child = Command::new(std::env::current_exe().expect("test binary"))
        .args(["crash_arm_lease_holder", "--exact", "--nocapture"])
        .env("WL_QUIESCE_HOLDER_MARKER", &marker)
        .env("WAYLAND_HOME", &world.home)
        .env("WAYLAND_PROFILES_ROOT", &world.profiles)
        .spawn()
        .expect("spawn holder");

    if !wait_for(&marker, Duration::from_secs(60)) {
        reap(child);
        panic!("the holder never acquired a lease");
    }

    // The holder really is holding it — without this the kill below would prove
    // nothing, because an unheld lease is trivially reclaimable.
    assert!(
        matches!(
            quiesce::acquire(&request("lease-observer", all_scope())),
            Err(QuiesceError::ConcurrentCapture { .. })
        ),
        "a live holder must exclude a second capture"
    );

    // Real, uncatchable death: SIGKILL on unix, TerminateProcess on Windows. No
    // Drop runs, no unwinding, no cleanup of any kind.
    child.kill().expect("kill holder");
    let status = child.wait().expect("reap holder");
    assert!(!status.success(), "the holder must have died, not exited");

    // The record survives the holder, which is the state a crash actually
    // leaves — and it must not wedge capture.
    assert!(
        quiesce::control_root().join("lease.json").exists(),
        "a killed holder leaves its record behind"
    );
    assert!(
        matches!(
            quiesce::acquire(&request("lease-observer", all_scope())),
            Err(QuiesceError::ConcurrentCapture { .. })
        ),
        "the lease is still in force until it lapses"
    );

    std::thread::sleep(Duration::from_millis(MIN_LEASE_TTL_MS + 250));
    let grant = quiesce::acquire(&request("lease-observer", all_scope()))
        .expect("a dead holder's lease must become reclaimable");
    let reclaimed = grant
        .reclaimed
        .expect("reclaiming a dead holder's lease must be reported");
    assert_eq!(reclaimed.lease_id, "lease-holder");
    let _ = quiesce::release("lease-observer", &grant.record.epoch);
}

// --- handle symmetry --------------------------------------------------------

#[test]
#[serial]
fn dropping_a_handle_releases_only_its_own_lease() {
    let _world = World::new(&["work"]);
    let grant = quiesce::acquire(&request("lease-1", all_scope())).expect("grant");
    {
        let _handle = quiesce::LeaseHandle::adopt(&grant);
    }
    assert!(
        quiesce::status().expect("status").held.is_none(),
        "Drop must release the lease it owns"
    );

    // The asymmetric case: a handle for a lease the control plane no longer
    // records must not remove someone else's record.
    let mine = quiesce::acquire(&request("lease-1", all_scope())).expect("mine");
    let stale_handle = quiesce::LeaseHandle::adopt(
        &quiesce::acquire(&request("lease-1", all_scope())).expect("replay"),
    );
    drop(stale_handle);
    // Same id, so that one legitimately released. Now prove the negative with a
    // handle whose id differs from the record on disk.
    let other = quiesce::acquire(&request("lease-2", all_scope())).expect("other");
    let impostor = quiesce::LeaseHandle::adopt(&mine);
    assert_eq!(impostor.lease_id(), "lease-1");
    drop(impostor);
    let held = quiesce::status()
        .expect("status")
        .held
        .expect("lease-2 must survive an impostor handle's Drop");
    assert_eq!(held.lease_id, "lease-2");
    let _ = quiesce::release("lease-2", &other.record.epoch);
}

#[test]
#[serial]
fn an_explicit_release_makes_drop_a_no_op() {
    let _world = World::new(&["work"]);
    let grant = quiesce::acquire(&request("lease-1", all_scope())).expect("grant");
    let mut handle = quiesce::LeaseHandle::adopt(&grant);
    let receipt = handle.release().expect("release").expect("a receipt");
    assert_eq!(receipt.verdict, ReleaseVerdict::Clean);
    assert!(
        handle.release().expect("second release").is_none(),
        "a second release must be a no-op, not a refusal"
    );

    // A different lease taken after the explicit release must survive this
    // handle's Drop — the double-free the journal lock actually shipped.
    let next = quiesce::acquire(&request("lease-2", all_scope())).expect("next");
    drop(handle);
    let held = quiesce::status()
        .expect("status")
        .held
        .expect("lease-2 must survive the released handle's Drop");
    assert_eq!(held.lease_id, "lease-2");
    let _ = quiesce::release("lease-2", &next.record.epoch);
}
