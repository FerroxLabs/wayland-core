use super::*;

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

fn require_live_acceptance() {
    assert_eq!(
        std::env::var_os("WAYLAND_SANDBOX_LIVE_WINDOWS").as_deref(),
        Some(OsStr::new("1")),
        "native acceptance must be invoked explicitly with WAYLAND_SANDBOX_LIVE_WINDOWS=1"
    );
}
