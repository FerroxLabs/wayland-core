//! Focused executable-readiness contract tests.

use super::*;

fn resolve_for_unix(
    program: &OsStr,
    effective_cwd: &Path,
    effective_path: Option<&OsStr>,
) -> Result<ResolvedExecutable, ExecutableReadinessError> {
    resolve_executable_for(
        program,
        effective_cwd,
        effective_path,
        None,
        ExecutablePlatform::Unix,
        McpStdioLaunchStrategy::CommandShell,
        WindowsResolutionContext {
            direct_roots: None,
            cwd_search_suppressed: true,
        },
    )
}

fn resolve_for_windows(
    program: &OsStr,
    effective_cwd: &Path,
    effective_path: Option<&OsStr>,
    effective_pathext: Option<&OsStr>,
) -> Result<ResolvedExecutable, ExecutableReadinessError> {
    resolve_for_windows_with_environment(
        program,
        effective_cwd,
        effective_path,
        effective_pathext,
        &[],
    )
}

fn resolve_for_windows_with_environment(
    program: &OsStr,
    effective_cwd: &Path,
    effective_path: Option<&OsStr>,
    effective_pathext: Option<&OsStr>,
    effective_environment: &[(OsString, OsString)],
) -> Result<ResolvedExecutable, ExecutableReadinessError> {
    resolve_executable_for(
        program,
        effective_cwd,
        effective_path,
        effective_pathext,
        ExecutablePlatform::Windows,
        mcp_stdio_launch_strategy(program, ExecutablePlatform::Windows),
        WindowsResolutionContext {
            direct_roots: None,
            cwd_search_suppressed: windows_cwd_search_suppressed(effective_environment),
        },
    )
}

fn resolve_for_windows_direct(
    program: &OsStr,
    effective_cwd: &Path,
    effective_path: Option<&OsStr>,
    roots: &WindowsDirectSearchRoots,
) -> Result<ResolvedExecutable, ExecutableReadinessError> {
    resolve_executable_for(
        program,
        effective_cwd,
        effective_path,
        None,
        ExecutablePlatform::Windows,
        McpStdioLaunchStrategy::Direct,
        WindowsResolutionContext {
            direct_roots: Some(roots),
            cwd_search_suppressed: true,
        },
    )
}

#[cfg(unix)]
fn make_executable(path: &Path, body: &str) {
    use std::os::unix::fs::PermissionsExt;

    std::fs::write(path, body).expect("write executable fixture");
    let mut permissions = std::fs::metadata(path).unwrap().permissions();
    permissions.set_mode(0o700);
    std::fs::set_permissions(path, permissions).unwrap();
}

#[tokio::test]
async fn executable_readiness_resolves_absolute_path_without_spawn() {
    let temp = tempfile::tempdir().unwrap();
    let executable = temp.path().join(if cfg!(windows) {
        "readiness-probe.cmd"
    } else {
        "readiness-probe"
    });
    let marker = temp.path().join("spawned");
    #[cfg(unix)]
    make_executable(
        &executable,
        &format!("#!/bin/sh\ntouch '{}'\n", marker.display()),
    );
    #[cfg(windows)]
    std::fs::write(
        &executable,
        format!("@echo off\r\necho spawned>\"{}\"\r\n", marker.display()),
    )
    .unwrap();

    let resolved = resolve_mcp_stdio_executable(
        executable.as_os_str(),
        temp.path(),
        None,
        if cfg!(windows) {
            Some(OsStr::new(".COM;.EXE;.BAT;.CMD"))
        } else {
            None
        },
        &[],
    )
    .await
    .unwrap();

    assert_eq!(resolved.as_path(), executable);
    assert!(!marker.exists(), "readiness must never execute the target");
    let debug = format!("{resolved:?}");
    assert!(!debug.contains(&temp.path().to_string_lossy().to_string()));
}

#[cfg(unix)]
#[test]
fn executable_readiness_uses_only_supplied_effective_path() {
    let temp = tempfile::tempdir().unwrap();
    let bin = temp.path().join("gui-bin");
    std::fs::create_dir(&bin).unwrap();
    let executable = bin.join("mcp-test-server");
    make_executable(&executable, "#!/bin/sh\nexit 0\n");

    let resolved = resolve_for_unix(
        OsStr::new("mcp-test-server"),
        temp.path(),
        Some(bin.as_os_str()),
    )
    .unwrap();
    assert_eq!(resolved.as_path(), executable);

    let empty_gui_path = temp.path().join("empty-gui-path");
    std::fs::create_dir(&empty_gui_path).unwrap();
    let error = resolve_for_unix(
        OsStr::new("mcp-test-server"),
        temp.path(),
        Some(empty_gui_path.as_os_str()),
    )
    .unwrap_err();
    assert!(matches!(error, ExecutableReadinessError::NotFound { .. }));
}

#[cfg(unix)]
#[test]
fn executable_readiness_distinguishes_missing_path_from_not_found() {
    let temp = tempfile::tempdir().unwrap();
    let missing_path = resolve_for_unix(OsStr::new("npx"), temp.path(), None).unwrap_err();
    assert!(matches!(
        missing_path,
        ExecutableReadinessError::MissingEffectivePath { .. }
    ));

    let not_found = resolve_for_unix(
        OsStr::new("npx"),
        temp.path(),
        Some(temp.path().as_os_str()),
    )
    .unwrap_err();
    assert!(matches!(
        not_found,
        ExecutableReadinessError::NotFound { .. }
    ));
}

#[test]
fn windows_readiness_reports_missing_pathext_as_environment_failure() {
    let temp = tempfile::tempdir().unwrap();
    let error = resolve_for_windows(
        OsStr::new("npx"),
        temp.path(),
        Some(temp.path().as_os_str()),
        None,
    )
    .unwrap_err();

    assert!(matches!(
        error,
        ExecutableReadinessError::InvalidEffectiveEnvironment {
            variable: ExecutableEnvironmentVariable::PathExt,
            ..
        }
    ));
    assert_eq!(
        error.to_string(),
        "cannot resolve executable npx: effective PATHEXT is invalid"
    );
}

#[test]
fn windows_readiness_resolves_cmd_and_bat_shims_without_spawn() {
    let temp = tempfile::tempdir().unwrap();
    let npx = temp.path().join("npx.cmd");
    let uvx = temp.path().join("uvx.bat");
    std::fs::write(&npx, "@echo off\r\necho spawned>marker\r\n").unwrap();
    std::fs::write(&uvx, "@echo off\r\necho spawned>marker\r\n").unwrap();
    let path = OsStr::new(temp.path().to_str().unwrap());
    let pathext = OsStr::new(".exe;.cmd;.bat");

    let npx_resolved =
        resolve_for_windows(OsStr::new("npx"), temp.path(), Some(path), Some(pathext)).unwrap();
    let uvx_resolved =
        resolve_for_windows(OsStr::new("uvx"), temp.path(), Some(path), Some(pathext)).unwrap();

    assert_eq!(npx_resolved.as_path(), npx);
    assert_eq!(uvx_resolved.as_path(), uvx);
    assert!(!temp.path().join("marker").exists());
}

#[test]
fn gui_missing_npx_is_actionable_redacted_and_does_not_widen_path() {
    let temp = tempfile::tempdir().unwrap();
    let secret = "super-secret-path-token";
    let gui_path = temp.path().join(secret);
    std::fs::create_dir(&gui_path).unwrap();

    let error = resolve_for_windows(
        OsStr::new("npx"),
        temp.path(),
        Some(gui_path.as_os_str()),
        Some(OsStr::new(".exe;.cmd;.bat")),
    )
    .unwrap_err();

    assert!(matches!(error, ExecutableReadinessError::NotFound { .. }));
    let user_diagnostic = error.to_string();
    assert!(user_diagnostic.contains("npx"));
    assert!(user_diagnostic.contains("effective launch environment"));
    for diagnostic in [user_diagnostic, format!("{error:?}")] {
        assert!(!diagnostic.contains(secret));
        assert!(!diagnostic.contains(".exe;.cmd;.bat"));
        assert!(!diagnostic.contains(&temp.path().to_string_lossy().to_string()));
    }
}

#[test]
fn executable_readiness_redacts_unsafe_program_labels() {
    let temp = tempfile::tempdir().unwrap();
    let error = resolve_for_unix(
        OsStr::new("token=do-not-log"),
        temp.path(),
        Some(OsStr::new("/also/secret")),
    )
    .unwrap_err();
    let diagnostic = format!("{error:?} {error}");
    assert!(diagnostic.contains("<configured executable>"));
    assert!(!diagnostic.contains("do-not-log"));
    assert!(!diagnostic.contains("/also/secret"));
}

#[cfg(unix)]
#[test]
fn unix_mode_check_uses_the_effective_identity_not_any_execute_bit() {
    assert_eq!(
        unix_mode_execute_permission(0o001, 1000, 1000, 1000, 1000, &[]),
        Err(CandidateFailure::PermissionDenied)
    );
    assert_eq!(
        unix_mode_execute_permission(0o010, 2000, 3000, 1000, 3000, &[]),
        Ok(())
    );
    assert_eq!(
        unix_mode_execute_permission(0o010, 2000, 3000, 1000, 4000, &[3000]),
        Ok(())
    );
    assert_eq!(
        unix_mode_execute_permission(0o000, 1000, 1000, 1000, 1000, &[]),
        Err(CandidateFailure::NotExecutable)
    );
    assert!(matches!(
        candidate_error(CandidateFailure::PermissionDenied, "server"),
        ExecutableReadinessError::PermissionDenied { .. }
    ));
    assert!(matches!(
        candidate_error(CandidateFailure::Io(io::ErrorKind::Other), "server"),
        ExecutableReadinessError::Io {
            kind: io::ErrorKind::Other,
            ..
        }
    ));
}

#[cfg(unix)]
#[test]
fn unix_non_executable_file_and_metadata_io_are_distinct() {
    use std::os::unix::fs::{PermissionsExt, symlink};

    let temp = tempfile::tempdir().unwrap();
    let non_executable = temp.path().join("not-executable");
    std::fs::write(&non_executable, "data").unwrap();
    let mut permissions = std::fs::metadata(&non_executable).unwrap().permissions();
    permissions.set_mode(0o600);
    std::fs::set_permissions(&non_executable, permissions).unwrap();
    let error = resolve_for_unix(non_executable.as_os_str(), temp.path(), None).unwrap_err();
    assert!(matches!(
        error,
        ExecutableReadinessError::NotExecutable { .. }
    ));

    let looped = temp.path().join("metadata-loop");
    symlink(&looped, &looped).unwrap();
    let error = resolve_for_unix(looped.as_os_str(), temp.path(), None).unwrap_err();
    assert!(matches!(error, ExecutableReadinessError::Io { .. }));
}

#[cfg(unix)]
#[test]
fn relative_program_and_path_entries_are_anchored_to_effective_cwd() {
    let temp = tempfile::tempdir().unwrap();
    let bin = temp.path().join("bin");
    std::fs::create_dir(&bin).unwrap();
    let executable = bin.join("relative-mcp");
    make_executable(&executable, "#!/bin/sh\nexit 0\n");

    let explicit = resolve_for_unix(OsStr::new("bin/relative-mcp"), temp.path(), None).unwrap();
    assert_eq!(explicit.as_path(), executable);

    let searched = resolve_for_unix(
        OsStr::new("relative-mcp"),
        temp.path(),
        Some(OsStr::new("bin")),
    )
    .unwrap();
    assert_eq!(searched.as_path(), executable);
}

#[test]
fn windows_rejects_explicit_extensions_outside_effective_pathext() {
    let temp = tempfile::tempdir().unwrap();
    let executable = temp.path().join("not-a-server.txt");
    std::fs::write(&executable, "not executable").unwrap();

    let error = resolve_for_windows(
        OsStr::new("./not-a-server.txt"),
        temp.path(),
        None,
        Some(OsStr::new(".EXE;.CMD;.BAT")),
    )
    .unwrap_err();

    assert!(matches!(
        error,
        ExecutableReadinessError::NotExecutable { .. }
    ));
}

#[test]
fn windows_searches_effective_child_cwd_before_effective_path_without_spawn() {
    let temp = tempfile::tempdir().unwrap();
    let marker = temp.path().join("cwd-spawned");
    let executable = temp.path().join("cwd-server.cmd");
    std::fs::write(
        &executable,
        format!("@echo off\r\necho spawned>\"{}\"\r\n", marker.display()),
    )
    .unwrap();

    let resolved = resolve_for_windows(
        OsStr::new("cwd-server"),
        temp.path(),
        None,
        Some(OsStr::new(".exe;.cmd;.bat")),
    )
    .unwrap();

    assert_eq!(resolved.as_path(), executable);
    assert!(!marker.exists());
}

#[test]
fn readiness_bounds_path_pathext_and_candidate_probes_before_io() {
    let temp = tempfile::tempdir().unwrap();
    let too_long_path = "p".repeat(MAX_EFFECTIVE_PATH_LENGTH + 1);
    let path_length_error = resolve_for_windows(
        OsStr::new("npx"),
        temp.path(),
        Some(OsStr::new(&too_long_path)),
        Some(OsStr::new(".CMD")),
    )
    .unwrap_err();
    assert!(matches!(
        path_length_error,
        ExecutableReadinessError::EnvironmentLimitExceeded {
            limit: ExecutableReadinessLimit::PathLength,
            ..
        }
    ));

    let too_many_path_entries = (0..=MAX_EFFECTIVE_PATH_ENTRIES)
        .map(|index| format!("p{index}"))
        .collect::<Vec<_>>()
        .join(";");
    let path_error = resolve_for_windows(
        OsStr::new("npx"),
        temp.path(),
        Some(OsStr::new(&too_many_path_entries)),
        Some(OsStr::new(".CMD")),
    )
    .unwrap_err();
    assert!(matches!(
        path_error,
        ExecutableReadinessError::EnvironmentLimitExceeded {
            limit: ExecutableReadinessLimit::PathEntries,
            ..
        }
    ));

    let too_long_pathext = format!(".{}", "E".repeat(MAX_EFFECTIVE_PATHEXT_LENGTH));
    let pathext_length_error = resolve_for_windows(
        OsStr::new("npx"),
        temp.path(),
        Some(temp.path().as_os_str()),
        Some(OsStr::new(&too_long_pathext)),
    )
    .unwrap_err();
    assert!(matches!(
        pathext_length_error,
        ExecutableReadinessError::EnvironmentLimitExceeded {
            limit: ExecutableReadinessLimit::PathExtLength,
            ..
        }
    ));

    let too_many_extensions = (0..=MAX_EFFECTIVE_PATHEXT_ENTRIES)
        .map(|index| format!(".E{index}"))
        .collect::<Vec<_>>()
        .join(";");
    let pathext_error = resolve_for_windows(
        OsStr::new("npx"),
        temp.path(),
        Some(temp.path().as_os_str()),
        Some(OsStr::new(&too_many_extensions)),
    )
    .unwrap_err();
    assert!(matches!(
        pathext_error,
        ExecutableReadinessError::EnvironmentLimitExceeded {
            limit: ExecutableReadinessLimit::PathExtEntries,
            ..
        }
    ));

    let probe_path = (0..16)
        .map(|index| format!("p{index}"))
        .collect::<Vec<_>>()
        .join(";");
    let probe_extensions = (0..MAX_EFFECTIVE_PATHEXT_ENTRIES)
        .map(|index| format!(".E{index}"))
        .collect::<Vec<_>>()
        .join(";");
    let probe_error = resolve_for_windows(
        OsStr::new("npx"),
        temp.path(),
        Some(OsStr::new(&probe_path)),
        Some(OsStr::new(&probe_extensions)),
    )
    .unwrap_err();
    assert!(matches!(
        probe_error,
        ExecutableReadinessError::EnvironmentLimitExceeded {
            limit: ExecutableReadinessLimit::CandidateProbes,
            ..
        }
    ));
}

#[test]
fn windows_direct_search_rejects_child_cwd_and_script_shims() {
    let temp = tempfile::tempdir().unwrap();
    let path_dir = temp.path().join("path");
    let current_exe_dir = temp.path().join("current-exe");
    let system_dir = temp.path().join("system");
    let windows_dir = temp.path().join("windows");
    let parent_path_dir = temp.path().join("parent-path-secret");
    for directory in [
        &path_dir,
        &current_exe_dir,
        &system_dir,
        &windows_dir,
        &parent_path_dir,
    ] {
        std::fs::create_dir(directory).unwrap();
    }
    let roots = WindowsDirectSearchRoots {
        current_executable_directory: current_exe_dir,
        system_directory: system_dir,
        windows_directory: windows_dir,
        parent_path: Some(parent_path_dir.into_os_string()),
    };

    std::fs::write(temp.path().join("cmd.exe"), "image fixture").unwrap();
    std::fs::write(temp.path().join("cmd.cmd"), "@echo off\r\n").unwrap();
    std::fs::write(temp.path().join("cmd.bat"), "@echo off\r\n").unwrap();
    let direct = resolve_for_windows_direct(
        OsStr::new("cmd"),
        temp.path(),
        Some(path_dir.as_os_str()),
        &roots,
    )
    .unwrap_err();
    assert!(matches!(direct, ExecutableReadinessError::NotFound { .. }));
    assert!(!direct.to_string().contains("parent-path-secret"));
    assert!(!format!("{direct:?}").contains(&temp.path().to_string_lossy().to_string()));

    for shim in ["cmd.cmd", "cmd.bat"] {
        let error = resolve_executable_for(
            temp.path().join(shim).as_os_str(),
            temp.path(),
            None,
            None,
            ExecutablePlatform::Windows,
            McpStdioLaunchStrategy::Direct,
            WindowsResolutionContext {
                direct_roots: Some(&roots),
                cwd_search_suppressed: true,
            },
        )
        .unwrap_err();
        assert!(matches!(
            error,
            ExecutableReadinessError::NotExecutable { .. }
        ));
    }
}

#[test]
fn windows_direct_search_matches_rust_directory_order_without_child_cwd() {
    let temp = tempfile::tempdir().unwrap();
    let effective_path_dir = temp.path().join("effective-path");
    let current_exe_dir = temp.path().join("current-exe");
    let system_dir = temp.path().join("system");
    let windows_dir = temp.path().join("windows");
    let parent_path_dir = temp.path().join("parent-path");
    for directory in [
        &effective_path_dir,
        &current_exe_dir,
        &system_dir,
        &windows_dir,
        &parent_path_dir,
    ] {
        std::fs::create_dir(directory).unwrap();
        std::fs::write(directory.join("cmd.exe"), "image fixture").unwrap();
    }
    let roots = WindowsDirectSearchRoots {
        current_executable_directory: current_exe_dir.clone(),
        system_directory: system_dir.clone(),
        windows_directory: windows_dir.clone(),
        parent_path: Some(parent_path_dir.clone().into_os_string()),
    };

    let resolved = resolve_for_windows_direct(
        OsStr::new("cmd"),
        temp.path(),
        Some(effective_path_dir.as_os_str()),
        &roots,
    )
    .unwrap();
    assert_eq!(resolved.as_path(), effective_path_dir.join("cmd.exe"));

    std::fs::remove_file(effective_path_dir.join("cmd.exe")).unwrap();
    let resolved = resolve_for_windows_direct(
        OsStr::new("cmd"),
        temp.path(),
        Some(effective_path_dir.as_os_str()),
        &roots,
    )
    .unwrap();
    assert_eq!(resolved.as_path(), current_exe_dir.join("cmd.exe"));

    std::fs::remove_file(current_exe_dir.join("cmd.exe")).unwrap();
    let resolved =
        resolve_for_windows_direct(OsStr::new("cmd"), temp.path(), None, &roots).unwrap();
    assert_eq!(resolved.as_path(), system_dir.join("cmd.exe"));

    std::fs::remove_file(system_dir.join("cmd.exe")).unwrap();
    let resolved =
        resolve_for_windows_direct(OsStr::new("cmd"), temp.path(), None, &roots).unwrap();
    assert_eq!(resolved.as_path(), windows_dir.join("cmd.exe"));

    std::fs::remove_file(windows_dir.join("cmd.exe")).unwrap();
    let resolved =
        resolve_for_windows_direct(OsStr::new("cmd"), temp.path(), None, &roots).unwrap();
    assert_eq!(resolved.as_path(), parent_path_dir.join("cmd.exe"));
}

#[test]
fn windows_no_default_current_directory_suppresses_cwd_and_preserves_path_order() {
    let temp = tempfile::tempdir().unwrap();
    let path_dir = temp.path().join("path");
    std::fs::create_dir(&path_dir).unwrap();
    let cwd_candidate = temp.path().join("server.cmd");
    let path_candidate = path_dir.join("server.cmd");
    std::fs::write(&cwd_candidate, "@echo cwd\r\n").unwrap();
    std::fs::write(&path_candidate, "@echo path\r\n").unwrap();
    let pathext = Some(OsStr::new(".cmd"));

    let enabled = resolve_for_windows_with_environment(
        OsStr::new("server"),
        temp.path(),
        Some(path_dir.as_os_str()),
        pathext,
        &[],
    )
    .unwrap();
    assert_eq!(enabled.as_path(), cwd_candidate);

    let effective_environment = [(
        OsString::from("nOdEfAuLtCuRrEnTdIrEcToRyInExEpAtH"),
        OsString::new(),
    )];
    let suppressed = resolve_for_windows_with_environment(
        OsStr::new("server"),
        temp.path(),
        Some(path_dir.as_os_str()),
        pathext,
        &effective_environment,
    )
    .unwrap();
    assert_eq!(suppressed.as_path(), path_candidate);
}

#[test]
fn invalid_cwd_drive_relative_and_windows_network_paths_fail_closed() {
    let temp = tempfile::tempdir().unwrap();
    let missing_cwd = temp.path().join("missing");
    let invalid_cwd = resolve_for_windows(
        OsStr::new("npx"),
        &missing_cwd,
        Some(temp.path().as_os_str()),
        Some(OsStr::new(".CMD")),
    )
    .unwrap_err();
    assert!(matches!(
        invalid_cwd,
        ExecutableReadinessError::InvalidEffectiveEnvironment {
            variable: ExecutableEnvironmentVariable::Cwd,
            ..
        }
    ));

    let drive_relative = resolve_for_windows(
        OsStr::new(r"C:npx.cmd"),
        temp.path(),
        Some(temp.path().as_os_str()),
        Some(OsStr::new(".CMD")),
    )
    .unwrap_err();
    assert!(matches!(
        drive_relative,
        ExecutableReadinessError::InvalidExecutable { .. }
    ));

    let network = resolve_for_windows(
        OsStr::new(r"\\server\share\npx.cmd"),
        temp.path(),
        Some(temp.path().as_os_str()),
        Some(OsStr::new(".CMD")),
    )
    .unwrap_err();
    assert!(matches!(
        network,
        ExecutableReadinessError::NetworkPathUnsupported { .. }
    ));
}

#[tokio::test]
async fn bounded_worker_returns_without_waiting_for_a_stalled_metadata_probe() {
    let started = std::time::Instant::now();
    let error = run_bounded_resolution(
        Duration::from_millis(5),
        "<configured executable>".to_string(),
        || {
            std::thread::sleep(Duration::from_millis(100));
            Err(ExecutableReadinessError::NotFound {
                executable: "<configured executable>".to_string(),
            })
        },
    )
    .await
    .unwrap_err();
    assert!(matches!(
        error,
        ExecutableReadinessError::ProbeTimedOut { .. }
    ));
    assert!(started.elapsed() < Duration::from_millis(80));
}

#[cfg(windows)]
#[tokio::test]
async fn native_windows_command_shell_resolves_cmd_and_bat_from_effective_cwd() {
    let temp = tempfile::tempdir().unwrap();
    let empty_path = temp.path().join("empty-path");
    std::fs::create_dir(&empty_path).unwrap();
    for program in ["native-cmd.cmd", "native-bat.bat"] {
        std::fs::write(temp.path().join(program), "@echo off\r\nexit /b 0\r\n").unwrap();
        let stem = program.rsplit_once('.').unwrap().0;
        let resolved = resolve_mcp_stdio_executable(
            OsStr::new(stem),
            temp.path(),
            Some(empty_path.as_os_str()),
            Some(OsStr::new(".COM;.EXE;.BAT;.CMD")),
            &[],
        )
        .await
        .unwrap();
        assert_eq!(resolved.as_path(), temp.path().join(program));
    }
}
