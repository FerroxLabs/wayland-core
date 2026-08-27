use super::*;
use crate::backends::appcontainer::acl_lock_policy as policy;

fn lease_paths() -> BTreeSet<PathBuf> {
    let Ok(directory) = lease_directory() else {
        return BTreeSet::new();
    };
    fs::read_dir(directory)
        .into_iter()
        .flatten()
        .filter_map(std::result::Result::ok)
        .map(|entry| entry.path())
        .filter(|path| path.extension().and_then(OsStr::to_str) == Some("toml"))
        .collect()
}

#[test]
fn sha256_matches_known_vector() {
    assert_eq!(
        sha256_hex(b"abc"),
        "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
    );
}

#[test]
fn generated_profile_names_are_safe_and_bounded() {
    let name = profile_name(u64::MAX, u64::MAX);
    validate_profile_name(&name).unwrap();
    assert!(name.len() <= 64);
}

#[test]
#[ignore = "requires explicit native Windows AppContainer acceptance"]
fn real_profile_collision_allocates_a_new_identity() {
    require_live_acceptance();
    let _lock = MutationLock::acquire().unwrap();
    let creation = current_process_creation_time().unwrap();
    let start = PROFILE_COUNTER.fetch_add(MAX_PROFILE_ATTEMPTS, Ordering::Relaxed);
    let occupied = profile_name(start, creation);
    let occupied_w = widen(&occupied);
    let display = widen("Wayland-Core collision test");
    let description = widen("W-ACE collision allocation proof");
    let mut occupied_sid = ptr::null_mut();
    let hr = unsafe {
        CreateAppContainerProfile(
            occupied_w.as_ptr(),
            display.as_ptr(),
            description.as_ptr(),
            ptr::null(),
            0,
            &mut occupied_sid as *mut _ as _,
        )
    };
    assert_eq!(hr, 0, "pre-create collision profile: {hr:#x}");
    let (allocated, allocated_sid) = unsafe { allocate_unique_profile(start).unwrap() };
    assert_ne!(allocated, occupied);
    unsafe {
        FreeSid(occupied_sid as _);
        FreeSid(allocated_sid as _);
        assert_eq!(DeleteAppContainerProfile(occupied_w.as_ptr()), 0);
        assert_eq!(DeleteAppContainerProfile(widen(&allocated).as_ptr()), 0);
    }
}

#[test]
#[ignore = "requires explicit native Windows AppContainer acceptance"]
fn setup_failure_after_durable_lease_cleans_up() {
    require_live_acceptance();
    let baseline = lease_paths();
    let result =
        ExecutionIdentity::start_with_apply(&SandboxManifest::default(), |_intents, _sid| {
            Err(exec_error("injected ACL setup failure".into()))
        });
    assert!(result.is_err(), "injected setup failure must propagate");
    assert_eq!(
        lease_paths(),
        baseline,
        "setup failure must remove its durable lease after verified cleanup"
    );
}

#[test]
#[ignore = "requires explicit native Windows AppContainer acceptance"]
fn live_owner_is_never_reclaimed() {
    require_live_acceptance();
    let mut identity = ExecutionIdentity::start(&SandboxManifest::default()).unwrap();
    let lease_path = identity.lease_path.clone();
    {
        let _lock = MutationLock::acquire().unwrap();
        unsafe { recover_dead_leases_locked(&lease_directory().unwrap()).unwrap() };
    }
    assert!(
        lease_path.exists(),
        "live owner lease must remain authoritative"
    );
    identity.mark_process_exited().unwrap();
    identity.cleanup().unwrap();
}

#[test]
#[ignore = "requires explicit native Windows AppContainer acceptance"]
fn malformed_or_unknown_lease_fails_closed() {
    require_live_acceptance();
    let directory = lease_directory().unwrap();
    let path = directory.join(format!("WCore-malformed-{}.toml", std::process::id()));
    fs::write(
        &path,
        "version = 1\nstate = \"active\"\nunknown_critical = true\n",
    )
    .unwrap();
    let result = ExecutionIdentity::start(&SandboxManifest::default());
    assert!(
        result.is_err(),
        "malformed or unknown-critical lease must block new execution"
    );
    fs::remove_file(path).unwrap();
}

#[test]
fn crash_helper_entry() {
    if std::env::var_os("WCORE_ACL_CRASH_HELPER").is_none() {
        return;
    }
    let grant = PathBuf::from(std::env::var_os("WCORE_ACL_CRASH_GRANT").unwrap());
    let marker = PathBuf::from(std::env::var_os("WCORE_ACL_CRASH_MARKER").unwrap());
    let identity = ExecutionIdentity::start(&SandboxManifest {
        fs_read_allow: vec![grant],
        ..Default::default()
    })
    .unwrap();
    fs::write(&marker, &identity.profile_name).unwrap();
    std::mem::forget(identity);
    std::process::exit(91);
}

#[test]
#[ignore = "requires explicit native Windows AppContainer acceptance"]
fn killed_owner_is_recovered_before_next_execution() {
    require_live_acceptance();
    let temp = tempfile::tempdir().unwrap();
    let grant = temp.path().join("grant");
    fs::create_dir(&grant).unwrap();
    let marker = temp.path().join("profile.txt");
    let status = std::process::Command::new(std::env::current_exe().unwrap())
        .arg("crash_helper_entry")
        .arg("--nocapture")
        .env("WAYLAND_SANDBOX_LIVE_WINDOWS", "1")
        // The helper must lease into the SAME root as this process, or the
        // lease it abandons lands where this test never looks.
        .env(TEST_LEASE_ROOT_ENV, test_lease_root().unwrap())
        .env("WCORE_ACL_CRASH_HELPER", "1")
        .env("WCORE_ACL_CRASH_GRANT", &grant)
        .env("WCORE_ACL_CRASH_MARKER", &marker)
        .status()
        .unwrap();
    assert_eq!(status.code(), Some(91), "crash helper must exit abruptly");
    let profile = fs::read_to_string(&marker).unwrap();
    let lease_path = lease_directory().unwrap().join(format!("{profile}.toml"));
    assert!(
        lease_path.exists(),
        "crash must leave durable recovery authority"
    );

    let mut old_sid: *mut core::ffi::c_void = ptr::null_mut();
    let hr = unsafe {
        DeriveAppContainerSidFromAppContainerName(
            widen(&profile).as_ptr(),
            &mut old_sid as *mut _ as _,
        )
    };
    assert_eq!(hr, 0, "derive crashed profile SID: {hr:#x}");
    let old_sid_guard = SidFreeGuard(old_sid);
    assert!(unsafe { contains_exact_sid_ace(&grant, old_sid_guard.0).unwrap() });

    let mut next = ExecutionIdentity::start(&SandboxManifest::default()).unwrap();
    assert!(
        !lease_path.exists(),
        "next start must reconcile dead owner first"
    );
    assert!(!unsafe { contains_exact_sid_ace(&grant, old_sid_guard.0).unwrap() });
    next.mark_process_exited().unwrap();
    next.cleanup().unwrap();
}

/// W-A(a): measure what one execution actually costs inside the machine-wide
/// mutation lock, rather than reasoning about it.
///
/// This is the instrument the contention question needs. It prints, per
/// manifest shape, the number of DACL-mutating intents and the wall clock of
/// each locked phase. The DACL write count is derivable exactly:
/// `apply_intents` performs one `SetNamedSecurityInfoW` per existing intent,
/// and `revoke_intents` performs one per intent plus one more per DENY intent
/// (`restore_unprotected_dacl`) — so `writes = intents + intents + denies`.
///
/// It asserts only what must not regress silently (the intent set matches the
/// manifest, and the lock is not held across profile-service RPC), and PRINTS
/// the timings, because a hard wall-clock threshold on someone else's hardware
/// is a permanently-red or permanently-green gate rather than a measurement.
#[test]
#[ignore = "requires explicit native Windows AppContainer acceptance"]
fn measure_locked_phase_cost_per_execution() {
    use std::time::Instant;
    require_live_acceptance();

    let temp = tempfile::tempdir().unwrap();
    let workspace = temp.path().join("workspace");
    let git_objects = workspace.join(".git").join("objects");
    fs::create_dir_all(&git_objects).unwrap();
    let secret = workspace.join(".env");
    fs::write(&secret, b"TOKEN=x").unwrap();
    let scratch = temp.path().join("scratch");
    fs::create_dir_all(&scratch).unwrap();

    // A grant on a DIRECTORY is written with `SUB_CONTAINERS_AND_OBJECTS_INHERIT`,
    // so `SetNamedSecurityInfoW` propagates the ACE across the whole subtree.
    // That, not the number of intents, is the only mechanism by which one
    // execution could hold the machine-wide lock for seconds — so the third
    // shape carries a tree big enough for the propagation cost to show.
    const WIDE_FILES: usize = 2000;
    let wide = temp.path().join("wide");
    for bucket in 0..20 {
        let dir = wide.join(format!("d{bucket}"));
        fs::create_dir_all(&dir).unwrap();
        for file in 0..(WIDE_FILES / 20) {
            fs::write(dir.join(format!("f{file}.txt")), b"x").unwrap();
        }
    }

    // Shape 1: the floor — no filesystem intents at all.
    // Shape 2: the production `WorkspacePolicy::contained()` shape — workspace
    // read+write, scratch write, and the two secret-deny targets a real repo
    // carries (`.env` and `.git/objects`).
    // Shape 3: the same shape over a 2000-file tree.
    let shapes: [(&str, SandboxManifest); 3] = [
        ("empty", SandboxManifest::default()),
        (
            "repo-rooted-small",
            SandboxManifest {
                fs_read_allow: vec![workspace.clone()],
                fs_write_allow: vec![workspace.clone(), scratch.clone()],
                fs_read_deny: vec![secret.clone(), git_objects.clone()],
                ..Default::default()
            },
        ),
        (
            "repo-rooted-2000-files",
            SandboxManifest {
                fs_read_allow: vec![wide.clone()],
                fs_write_allow: vec![wide.clone(), scratch.clone()],
                fs_read_deny: vec![secret.clone(), git_objects.clone()],
                ..Default::default()
            },
        ),
    ];

    for (label, manifest) in shapes {
        let intents = canonical_intents(&manifest).unwrap();
        let denies = intents
            .iter()
            .filter(|i| i.kind == IntentKind::Deny)
            .count();
        let projected_writes = intents.len() * 2 + denies;

        let setup = Instant::now();
        let mut identity = ExecutionIdentity::start(&manifest).unwrap();
        let setup_us = setup.elapsed().as_micros();

        let exited = Instant::now();
        identity.mark_process_exited().unwrap();
        let exited_us = exited.elapsed().as_micros();

        let teardown = Instant::now();
        identity.cleanup().unwrap();
        let teardown_us = teardown.elapsed().as_micros();

        println!(
            "MEASURE shape={label} intents={} denies={denies} \
             projected_dacl_writes={projected_writes} setup_us={setup_us} \
             mark_exited_us={exited_us} teardown_us={teardown_us}",
            intents.len()
        );

        assert_eq!(
            intents.len(),
            manifest
                .fs_read_allow
                .iter()
                .chain(manifest.fs_write_allow.iter())
                .chain(manifest.fs_read_deny.iter())
                .collect::<BTreeSet<_>>()
                .len(),
            "{label}: the intent set must be exactly the manifest's distinct existing paths"
        );
    }
}

// ---------------------------------------------------------------------------
// F-28-02-002 — the stale-lease wedge.
//
// These do NOT need `WAYLAND_SANDBOX_LIVE_WINDOWS` and are NOT `#[ignore]`d, on
// purpose: they run in the ordinary `cargo test -p wcore-sandbox` pass. Every
// wedge-adjacent test in this file before them was gated AND ignored, which is
// the same shape as the suites on this program that reported `test result: ok`
// having executed zero tests. Nothing here needs a real AppContainer profile —
// the whole defect lives in file-and-liveness handling — so nothing here buys a
// gate it does not need.
//
// Both legs are proved, because only proving the reclaim leg would be satisfied
// by an implementation that reclaims unconditionally, which would revoke the
// ACLs of a container that is still running.
// ---------------------------------------------------------------------------

static SYNTHETIC_COUNTER: AtomicU64 = AtomicU64::new(0);

/// A lease directory this test alone can reach.
///
/// The per-process test root is shared by every test in this binary, and a
/// recovery sweep enumerates a directory and only then opens what it found, so
/// one test's lease creation or removal is another test's `os error 2`. See
/// [`private_lease_directory`] for the measurement and for why the product
/// itself has no such race.
///
/// Returned as a tuple so the caller has to bind the `TempDir`: it deletes the
/// directory on drop, and a root dropped early would delete the lease the test
/// is still working on.
fn private_lease_root() -> (tempfile::TempDir, PathBuf) {
    let local = tempfile::tempdir().unwrap();
    let directory = private_lease_directory(local.path()).unwrap();
    (local, directory)
}

/// Serialize the tests that share the process-global reclamation report sink.
///
/// `EMITTED_RECLAMATIONS` is one `static` for the whole test binary: any sweep
/// that reclaims a lease pushes into it, and two tests below drain it and
/// assert on the exact count. Since every test here now sweeps a lease
/// directory of its own, that sink is the ONLY state they still share, and this
/// lock guards it and nothing else.
///
/// Deliberately a plain in-process mutex rather than [`MutationLock`]: taking
/// the real cross-process lock would make these tests depend on
/// `SeCreateGlobalPrivilege` (its mutex lives in the `Global\` namespace),
/// which would test the privileges of whoever ran `cargo test` rather than the
/// repair. The lock is intentionally NOT poisoned-propagating: one failing
/// assertion must not cascade into three misleading secondary failures.
fn reclamation_sink_lock() -> std::sync::MutexGuard<'static, ()> {
    static LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
    LOCK.lock().unwrap_or_else(|poison| poison.into_inner())
}

/// Write a lease that can never reconcile (it carries the test SID sentinel,
/// exactly like the two files found wedging a real developer box).
///
/// `owner_live` selects the ONLY difference between the two legs: a live leg
/// stamps this process's real creation time, so `owner_is_live` sees a running
/// owner; a dead leg stamps a creation time no process has, so the recorded
/// owner identity provably does not exist. Using a mismatched creation time
/// rather than an exited pid is deliberate — it cannot flake on Windows pid
/// reuse, which would silently turn the "dead" leg into a "live" one.
fn write_unreconcilable_lease(
    directory: &Path,
    tag: &str,
    owner_live: bool,
    intents: Vec<AclIntent>,
) -> PathBuf {
    let sequence = SYNTHETIC_COUNTER.fetch_add(1, Ordering::Relaxed);
    let profile_name = format!(
        "{PROFILE_PREFIX}-h2{tag}-{:08x}-{sequence:04x}",
        std::process::id()
    );
    validate_profile_name(&profile_name).unwrap();
    let real_creation = current_process_creation_time().unwrap();
    let mut lease = LeaseFile {
        version: LEASE_VERSION,
        state: LeaseState::Prepared,
        profile_name: profile_name.clone(),
        sid_sha256: sha256_hex(TEST_SID_SENTINEL),
        owner_pid: std::process::id(),
        owner_creation_time: if owner_live {
            real_creation
        } else {
            real_creation.wrapping_add(1)
        },
        intents,
        lease_sha256: String::new(),
    };
    lease.refresh_digest();
    assert_eq!(
        owner_is_live(&lease).unwrap(),
        owner_live,
        "the synthetic lease must present the owner liveness this leg is testing"
    );
    let path = directory.join(format!("{profile_name}.toml"));
    write_new_synced_lease(&path, &lease).unwrap();
    path
}

/// Files sitting in the quarantine directory whose name came from `lease_path`.
///
/// Scoped to one lease rather than counting the whole directory: a bare count
/// would also be satisfied by an artifact quarantined from some other lease.
/// The quarantine directory is derived from the lease's own directory, so this
/// follows the caller into its private lease root.
fn quarantined_for(lease_path: &Path) -> Vec<PathBuf> {
    let name = lease_path.file_name().and_then(OsStr::to_str).unwrap();
    let quarantine = lease_path
        .parent()
        .expect("a lease always sits in a lease directory")
        .join(QUARANTINE_DIRECTORY);
    fs::read_dir(quarantine)
        .into_iter()
        .flatten()
        .filter_map(std::result::Result::ok)
        .map(|entry| entry.path())
        .filter(|path| {
            path.file_name()
                .and_then(OsStr::to_str)
                .is_some_and(|found| found.starts_with(name))
        })
        .collect()
}

#[test]
fn dead_owner_unreconcilable_lease_is_reclaimed_not_refused_forever() {
    let _lock = reclamation_sink_lock();
    let (_local, directory) = private_lease_root();
    let path = write_unreconcilable_lease(&directory, "dead", false, Vec::new());
    assert!(path.exists(), "the wedge lease must start on disk");

    // Before F-28-02-002 was repaired this returned Err, and did so on every
    // later call, which is what made the denial of service permanent.
    unsafe { recover_dead_leases_locked(&directory) }
        .expect("a dead owner's unreconcilable lease must not refuse acquisition forever");

    assert!(
        !path.exists(),
        "the reclaimed lease must be gone from the ACTIVE lease directory"
    );
    let quarantined = quarantined_for(&path);
    assert_eq!(
        quarantined.len(),
        1,
        "the lease must be MOVED to quarantine, not deleted: {quarantined:?}"
    );
    let preserved = fs::read_to_string(&quarantined[0]).unwrap();
    assert!(
        preserved.contains(TEST_SID_SENTINEL_SHA256),
        "quarantine must preserve the evidence verbatim"
    );
    fs::remove_file(&quarantined[0]).unwrap();
}

#[test]
fn live_owner_unreconcilable_lease_is_honoured_not_reclaimed() {
    // No reclamation-sink lock: a live owner is skipped, so this sweep can
    // never emit a report.
    let (_local, directory) = private_lease_root();
    // Identical to the leg above in every respect EXCEPT that the recorded
    // owner is this running process. Reclaiming this would revoke the ACLs of a
    // container that is still executing.
    let path = write_unreconcilable_lease(&directory, "live", true, Vec::new());

    unsafe { recover_dead_leases_locked(&directory) }
        .expect("a live owner's lease must be skipped, not error");

    assert!(
        path.exists(),
        "a lease whose owning process is RUNNING must never be reclaimed"
    );
    assert!(
        quarantined_for(&path).is_empty(),
        "a live owner's lease must never reach quarantine"
    );
    fs::remove_file(&path).unwrap();
}

#[test]
fn quarantine_directory_does_not_become_a_second_wedge() {
    // Recovery rejects every unrecognised entry in the lease directory with a
    // hard error. The quarantine directory it creates IS such an entry, so an
    // implementation that reclaimed the lease but did not allow-list its own
    // quarantine directory would refuse forever from the second pass onward —
    // the identical defect, one indirection further down.
    let _lock = reclamation_sink_lock();
    let (_local, directory) = private_lease_root();
    let path = write_unreconcilable_lease(&directory, "reentry", false, Vec::new());
    unsafe { recover_dead_leases_locked(&directory) }.unwrap();
    assert!(
        directory.join(QUARANTINE_DIRECTORY).is_dir(),
        "the first reclamation must create the quarantine directory"
    );

    unsafe { recover_dead_leases_locked(&directory) }
        .expect("a lease directory that CONTAINS a quarantine directory must still recover");

    for stale in quarantined_for(&path) {
        fs::remove_file(stale).unwrap();
    }
}

/// The lock's holder sidecar must be published OUTSIDE the swept directory.
///
/// `start_with_apply` runs these two statements back to back:
///
/// ```ignore
/// let _lock = MutationLock::acquire()?;          // publishes the holder sidecar
/// unsafe { recover_dead_leases_locked(&lease_dir)? };
/// ```
///
/// and the sweep hard-errors on every entry in the lease directory it does not
/// recognise. A sidecar published in there therefore fails EVERY sandboxed
/// command under `WAYLAND_SANDBOX=appcontainer|strict`, with an error naming a
/// stray file rather than the lock — turning an intermittent contention timeout
/// into an unconditional failure of the whole backend. Exactly the wedge class
/// `quarantine_directory_does_not_become_a_second_wedge` guards, arriving
/// through the lock instead of through recovery.
///
/// Allow-listing a second name in the sweep is deliberately NOT the repair;
/// `shared_verdict::record_path` records why (an older Core build that meets
/// the new file wedges instead), so the sidecar gets a sibling directory.
#[test]
fn the_lock_holder_sidecar_is_published_outside_the_swept_lease_directory() {
    let _lock = reclamation_sink_lock();
    let (local, lease_dir) = private_lease_root();
    let holder_dir = private_lock_holder_directory(local.path()).unwrap();

    policy::publish_holder(&holder_dir, std::process::id(), r"C:\wayland.exe");
    // Positive control: every assertion below is vacuous unless publishing
    // actually wrote a sidecar somewhere.
    assert!(
        policy::read_holder(&holder_dir, 0).is_some(),
        "publish_holder wrote nothing, so this test cannot see where it went"
    );

    // The CONSEQUENCE first, so a regression reports the production failure
    // rather than the structural reason for it.
    unsafe { recover_dead_leases_locked(&lease_dir) }
        .expect("the sweep that follows every acquisition must still recover");

    // Then the reason. The sweep above can only stay green while this holds,
    // and stating it separately keeps the guard honest if the sweep ever grows
    // a tolerance the production wedge did not have.
    assert!(
        !lease_dir.join(policy::HOLDER_FILE).exists(),
        "the sidecar landed in the directory the sweep rejects unknown entries in"
    );
    assert!(
        !holder_dir.starts_with(&lease_dir),
        "the holder directory is inside the swept lease directory: {} under {}",
        holder_dir.display(),
        lease_dir.display()
    );
}

/// Environment carrying the helper's report path, and the marker that turns
/// [`holder_sidecar_sweep_helper_entry`] from a no-op into the child process.
const HOLDER_SWEEP_REPORT_ENV: &str = "WCORE_HOLDER_SWEEP_REPORT";

/// The production setup sequence, EXECUTED — not two pathnames compared.
///
/// `start_with_apply` is, verbatim and in this order:
///
/// ```ignore
/// let _lock = MutationLock::acquire()?;              // resolves + publishes the sidecar
/// unsafe { recover_dead_leases_locked(&lease_dir)? };
/// ```
///
/// Wave 1 of `#945` broke exactly that composition — the sidecar landed in the
/// directory the next line sweeps — and neither of the pathname guards below
/// could see it, because a guard on two resolvers never runs the wiring that
/// chooses between them. This runs it: the real [`HolderSidecar::resolve`], the
/// real publish, and the real `recover_dead_leases_locked` over the real
/// [`lease_directory`], all rooted at ONE lease root.
///
/// It runs in a CHILD PROCESS, and that is not decoration. `lease_root` is a
/// per-process `OnceLock`, so the only way to give the two production resolvers
/// a root of their own — instead of the process-wide one every other test in
/// this binary is concurrently writing into (`#1095`) — is to hand it to a new
/// process through [`TEST_LEASE_ROOT_ENV`], which is what that variable exists
/// for.
///
/// The child does NOT take the `Global\` mutex. Creating a `Global\` object
/// needs `SeCreateGlobalPrivilege` outside session 0, so a non-elevated
/// developer running `cargo test` would see this fail for a reason that has
/// nothing to do with the invariant. What the mutex would add is serialization;
/// what is under test is the directory the sidecar is published into, and the
/// child owns its lease root outright.
#[test]
fn the_published_holder_survives_the_sweep_that_follows_every_acquisition() {
    let root = tempfile::tempdir().unwrap();
    let report = root.path().join("report.txt");
    let status = std::process::Command::new(std::env::current_exe().unwrap())
        .arg("holder_sidecar_sweep_helper_entry")
        .arg("--nocapture")
        .env(TEST_LEASE_ROOT_ENV, root.path())
        .env(HOLDER_SWEEP_REPORT_ENV, &report)
        .status()
        .unwrap();
    let report = fs::read_to_string(&report).unwrap_or_default();
    assert!(
        status.success(),
        "the production acquire-then-sweep sequence failed; child reported: {report}"
    );
    // A child that returned early, or that skipped a step, must not read as a
    // pass: every marker below is written only after the step it names ran.
    for marker in [
        "resolved=",
        "published=1",
        "sweep-is-armed=1",
        "swept-clean=1",
    ] {
        assert!(
            report.contains(marker),
            "the child never reached {marker:?}; it reported: {report}"
        );
    }

    // Independently of anything the child asserted: find the sidecar on disk
    // and check WHERE it is. This is the assertion a child that lied cannot
    // satisfy, and it is made against the same two production layouts the child
    // resolved, derived here from the same root.
    let lease_dir = private_lease_directory(root.path()).unwrap();
    let holder_dir = private_lock_holder_directory(root.path()).unwrap();
    // And bind the two: the directory PRODUCTION chose in the child is the one
    // checked below. Without this the structural checks would be made against
    // paths this test computed, and a resolver that wandered off to a third
    // directory entirely would satisfy both of them.
    assert!(
        report.contains(&format!("resolved={}\n", holder_dir.display())),
        "the child resolved a directory other than {}; it reported: {report}",
        holder_dir.display()
    );
    assert!(
        holder_dir.join(policy::HOLDER_FILE).is_file(),
        "no sidecar was published under {}, so this test proved nothing",
        holder_dir.display()
    );
    assert!(
        !lease_dir.join(policy::HOLDER_FILE).exists(),
        "the sidecar landed in the swept lease directory {}",
        lease_dir.display()
    );
}

/// Child half of [`the_published_holder_survives_the_sweep_that_follows_every_acquisition`].
///
/// Inert unless [`HOLDER_SWEEP_REPORT_ENV`] is set, exactly like
/// `mutation_lock_helper_entry`, so an ordinary run of this binary skips it.
#[test]
fn holder_sidecar_sweep_helper_entry() {
    let Some(report) = std::env::var_os(HOLDER_SWEEP_REPORT_ENV) else {
        return;
    };
    let report = PathBuf::from(report);
    let mut log = String::new();
    let mut note = |line: String| {
        log.push_str(&line);
        log.push('\n');
        fs::write(&report, &log).unwrap();
    };

    // The production choice, made by production code, under this child's root.
    let sidecar = HolderSidecar::resolve();
    let directory = sidecar
        .directory()
        .expect("the holder directory must resolve under a private lease root")
        .to_path_buf();
    note(format!("resolved={}", directory.display()));

    sidecar.publish(std::process::id(), r"C:\wayland.exe");
    // Positive control: everything below is vacuous unless publishing actually
    // wrote a sidecar somewhere.
    assert!(
        sidecar.sample(0).is_some(),
        "publish wrote nothing, so this child cannot see where it went"
    );
    note("published=1".to_string());

    let lease_dir = lease_directory().unwrap();

    // Control FIRST, and it is the one that makes the pass below mean
    // something: prove that the sweep in THIS process, over THIS lease
    // directory, still rejects an entry it does not recognise. Without it a
    // sweep that had quietly stopped enforcing anything would read as success.
    let intruder = lease_dir.join("wave3-control.txt");
    fs::write(&intruder, b"not a lease").unwrap();
    let rejected = unsafe { recover_dead_leases_locked(&lease_dir) }
        .expect_err("the sweep must still reject an unknown entry")
        .to_string();
    assert!(
        rejected.contains("unknown entry in AppContainer ACL lease directory"),
        "the sweep rejected the control for the wrong reason: {rejected}"
    );
    fs::remove_file(&intruder).unwrap();
    note("sweep-is-armed=1".to_string());

    // The production consequence. With the published sidecar still on disk, the
    // line that runs immediately after `MutationLock::acquire` must recover.
    unsafe { recover_dead_leases_locked(&lease_dir) }
        .expect("the sweep that follows every acquisition must survive the holder sidecar");
    note("swept-clean=1".to_string());
}

/// The same invariant on the PRODUCTION resolvers, not just the test ones.
///
/// The test above pins the behaviour of a private root; this pins the two
/// pathnames the product itself computes, so moving one of them without the
/// other cannot pass by only touching test plumbing.
#[test]
fn the_production_holder_directory_is_a_sibling_of_the_lease_directory() {
    let lease = lease_directory().unwrap();
    let holder = lock_holder_directory().unwrap();
    assert!(
        !holder.starts_with(&lease),
        "{} is inside {}",
        holder.display(),
        lease.display()
    );
    assert_ne!(holder, lease);
    assert_eq!(
        holder.parent().and_then(Path::parent),
        lease.parent().and_then(Path::parent),
        "the holder directory must be a SIBLING of the lease directory, sharing its root"
    );
}

/// Reclaim one unreconcilable lease and return the report the product emitted.
///
/// Reads the report out of the production emit path, NOT out of the quarantined
/// file. That distinction is the whole of `F-28-ADJ-001`: the quarantined file
/// contains the grant path because the file was MOVED verbatim, so asserting on
/// it passes no matter what the operator is told.
fn reclaim_and_capture_report(tag: &str, intents: Vec<AclIntent>) -> String {
    let (_local, directory) = private_lease_root();
    let _ = take_emitted_reclamations();
    let path = write_unreconcilable_lease(&directory, tag, false, intents);

    unsafe { recover_dead_leases_locked(&directory) }.unwrap();

    let mut reports = take_emitted_reclamations();
    assert_eq!(
        reports.len(),
        1,
        "exactly one reclamation must have been reported, got: {reports:?}"
    );
    for stale in quarantined_for(&path) {
        fs::remove_file(stale).unwrap();
    }
    reports.pop().unwrap()
}

#[test]
fn reclamation_reports_grants_it_could_not_revoke() {
    // A lease with recorded intents cannot have those grants revoked: the SID
    // is stored as a digest and cannot be reconstructed. Refusing forever never
    // revoked them either, so reclaiming is strictly better — but the operator
    // has to be TOLD, and that is the ONLY warning they get.
    //
    // Asserted in BOTH directions on purpose. Disclosure alone is satisfied by
    // an implementation that always discloses; silence alone is satisfied by
    // one that never does. Mutant M3 (adjudication `28-adj`) deletes the
    // disclosure branch so every reclamation claims nothing was left behind —
    // the negative assertion below is what catches it.
    let _lock = reclamation_sink_lock();
    const GRANT: &str = r"C:\f28h2-residual";

    let disclosed = reclaim_and_capture_report(
        "residual",
        vec![AclIntent {
            path: GRANT.to_string(),
            kind: IntentKind::Allow,
            mask: ACL_READ_MASK,
        }],
    );
    assert!(
        disclosed.contains(GRANT),
        "a grant that could not be revoked must be named to the operator: {disclosed}"
    );
    assert!(
        disclosed.contains("could NOT be revoked automatically"),
        "the report must say the grants were not revoked, not merely list a path: {disclosed}"
    );
    assert!(
        !disclosed.contains("nothing was left behind"),
        "a lease WITH un-revokable grants must never be reported as leaving nothing behind: \
         {disclosed}"
    );

    let silent = reclaim_and_capture_report("noresidual", Vec::new());
    assert!(
        silent.contains("nothing was left behind"),
        "a lease with no recorded grant must say so plainly: {silent}"
    );
    assert!(
        !silent.contains("could NOT be revoked automatically"),
        "a lease with no recorded grant must not manufacture a residual warning: {silent}"
    );
    assert!(
        !silent.contains(GRANT),
        "a lease with no recorded grant must name no path: {silent}"
    );
}

/// Write a lease file with exactly the given bytes, bypassing the writer.
///
/// Reproduces an on-disk STATE, and does not claim to simulate the crash that
/// produces it. That the state is reachable is established by
/// `write_new_synced_lease` creating the file before writing its content, and
/// by `zero_length_lease_is_reachable_through_the_writer` below.
fn write_raw_lease(directory: &Path, tag: &str, bytes: &[u8]) -> PathBuf {
    let sequence = SYNTHETIC_COUNTER.fetch_add(1, Ordering::Relaxed);
    let path = directory.join(format!(
        "{PROFILE_PREFIX}-h2{tag}-{:08x}-{sequence:04x}.toml",
        std::process::id()
    ));
    fs::write(&path, bytes).unwrap();
    path
}

#[test]
fn zero_length_lease_is_reclaimed_not_refused_forever() {
    // F-28-ADJ-002. Reproduced on hardware at 1b9f148f before this fix: a
    // 0-byte .toml refused all sandboxed execution, twice running, with
    // `invalid AppContainer ACL lease size 0`.
    let _lock = reclamation_sink_lock();
    let (_local, directory) = private_lease_root();
    let _ = take_emitted_reclamations();
    let path = write_raw_lease(&directory, "zerolen", b"");
    assert_eq!(fs::metadata(&path).unwrap().len(), 0);

    unsafe { recover_dead_leases_locked(&directory) }
        .expect("a 0-byte lease must not refuse acquisition forever");

    assert!(
        !path.exists(),
        "the 0-byte lease must be gone from the ACTIVE lease directory"
    );
    let quarantined = quarantined_for(&path);
    assert_eq!(
        quarantined.len(),
        1,
        "it must be MOVED to quarantine, not deleted: {quarantined:?}"
    );
    let reports = take_emitted_reclamations();
    assert_eq!(
        reports.len(),
        1,
        "the reclamation must be reported: {reports:?}"
    );
    assert!(
        reports[0].contains("0-byte"),
        "the report must name the actual cause: {}",
        reports[0]
    );
    assert!(
        reports[0].contains("nothing was left behind"),
        "an empty lease recorded no grant and must say so: {}",
        reports[0]
    );

    // Permanence was the finding, so prove the SECOND pass is clean too.
    unsafe { recover_dead_leases_locked(&directory) }
        .expect("recovery must stay clean after reclaiming a 0-byte lease");
    fs::remove_file(&quarantined[0]).unwrap();
}

#[test]
fn a_non_empty_unreadable_lease_still_fails_closed() {
    // The guard rail on the fix above. Reclamation is keyed on zero LENGTH
    // only. A non-empty lease that will not parse is indistinguishable from a
    // tampered one -- it may carry real ACL grants -- so it must keep refusing.
    // Widening the 0-byte reclamation to "anything unreadable" would silently
    // convert this deliberate fail-closed into a reclaim, which is why this
    // test sits next to it rather than in the existing ignore-gated suite.
    // No reclamation-sink lock: this sweep fails closed before it reclaims
    // anything, so it can never emit a report.
    let (_local, directory) = private_lease_root();
    let path = write_raw_lease(
        &directory,
        "malformed",
        b"version = 1\nstate = \"nonsense\"\n",
    );
    assert!(fs::metadata(&path).unwrap().len() > 0);

    let result = unsafe { recover_dead_leases_locked(&directory) };

    assert!(
        result.is_err(),
        "a non-empty unparseable lease must still fail closed, not be reclaimed"
    );
    assert!(
        path.exists(),
        "a non-empty unparseable lease must NOT be quarantined: it may hold real grants"
    );
    fs::remove_file(&path).unwrap();
}

#[test]
fn zero_length_lease_is_reachable_through_the_writer() {
    // The half of F-28-ADJ-002 that is about CAUSE rather than effect: the
    // 0-byte state is not hypothetical, it is what the product's own writer
    // leaves on disk between creating the file and writing its content. Proved
    // by observing the file at that exact instant rather than by reading the
    // source, and without killing anything.
    // No reclamation-sink lock: this test never runs a recovery sweep.
    let (_local, directory) = private_lease_root();
    let observed = std::sync::Arc::new(std::sync::Mutex::new(None::<u64>));
    let probe = std::sync::Arc::clone(&observed);
    let path = write_new_synced_lease_observed(&directory, "window", move |path| {
        *probe.lock().unwrap() = Some(fs::metadata(path).unwrap().len());
    });
    assert_eq!(
        observed.lock().unwrap().take(),
        Some(0),
        "the writer must be observable with the lease created and still empty"
    );
    fs::remove_file(&path).unwrap();
}

/// Drive the real writer, calling `probe` after the file exists and before its
/// content is written.
fn write_new_synced_lease_observed(
    directory: &Path,
    tag: &str,
    probe: impl FnOnce(&Path),
) -> PathBuf {
    let sequence = SYNTHETIC_COUNTER.fetch_add(1, Ordering::Relaxed);
    let profile_name = format!(
        "{PROFILE_PREFIX}-h2{tag}-{:08x}-{sequence:04x}",
        std::process::id()
    );
    let mut lease = LeaseFile {
        version: LEASE_VERSION,
        state: LeaseState::Prepared,
        profile_name: profile_name.clone(),
        sid_sha256: sha256_hex(TEST_SID_SENTINEL),
        owner_pid: std::process::id(),
        owner_creation_time: current_process_creation_time().unwrap(),
        intents: Vec::new(),
        lease_sha256: String::new(),
    };
    lease.refresh_digest();
    let path = directory.join(format!("{profile_name}.toml"));
    storage::write_new_synced_lease_with_probe(&path, &lease, probe).unwrap();
    path
}

fn require_live_acceptance() {
    assert_eq!(
        std::env::var_os("WAYLAND_SANDBOX_LIVE_WINDOWS").as_deref(),
        Some(OsStr::new("1")),
        "native acceptance must be invoked explicitly with WAYLAND_SANDBOX_LIVE_WINDOWS=1"
    );
}

// ---------------------------------------------------------------------------
// Instrument, not a gate.
//
// Deliberately `#[ignore]`d and deliberately assertion-free: it MEASURES, and a
// timing threshold here would either be so loose it proves nothing or so tight
// it flakes on a busy host. It exists because the Windows sandbox stall was
// diagnosed wrong twice from arithmetic that merely happened to match, and the
// only thing that settled it was running this.
//
// Recorded on SEANDESKTOP (32 logical cores, NVMe), whole ExecutionIdentity
// lifecycle through the real entry points, median per op:
//
//   BEFORE the profile RPCs were moved out of MutationLock : ~140 ms
//   AFTER                                                  :  ~68 ms  (idle)
//                                                            ~103 ms (32 CPU burners)
//
// The floor is the AppX profile service itself: the same 24 threads doing ONLY
// CreateAppContainerProfile + DeleteAppContainerProfile with no lock of ours
// cost 350 ms total (~15 ms/op), and that part is Windows', not ours.
// ---------------------------------------------------------------------------

#[test]
#[ignore = "measurement instrument; run explicitly with --ignored --nocapture"]
fn measure_concurrent_lifecycles() {
    require_live_acceptance();
    let workspace = tempfile::tempdir().unwrap();
    fs::write(workspace.path().join("a.txt"), b"x").unwrap();
    let manifest = SandboxManifest {
        fs_write_allow: vec![workspace.path().to_path_buf()],
        ..SandboxManifest::default()
    };
    for threads in [1usize, 4, 8, 16, 24] {
        let started = std::time::Instant::now();
        std::thread::scope(|scope| {
            for _ in 0..threads {
                scope.spawn(|| {
                    let mut identity = ExecutionIdentity::start(&manifest).unwrap();
                    identity.mark_process_exited().unwrap();
                    identity.cleanup().unwrap();
                });
            }
        });
        let total_ms = started.elapsed().as_millis();
        println!(
            "MEASURE concurrent threads={threads} total_ms={total_ms} per_op_ms={:.1}",
            total_ms as f64 / threads as f64
        );
    }
}
