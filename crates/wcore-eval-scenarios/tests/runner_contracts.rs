use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use wcore_eval_scenarios::assertions::Assertion;
use wcore_eval_scenarios::providers::{ProviderConfig, ProviderId};
use wcore_eval_scenarios::runner::{Failure, run_with_binary, run_with_binary_in_environment};
use wcore_eval_scenarios::scenario::{Category, Scenario, Turn};
use wcore_eval_scenarios::tempenv;

#[cfg(target_os = "linux")]
static MIGRATION_CGROUP_COUNTER: AtomicUsize = AtomicUsize::new(0);

fn fixture() -> &'static std::path::Path {
    std::path::Path::new(env!("CARGO_BIN_EXE_wcore-eval-fixture"))
}

fn provider(model: &str) -> ProviderConfig {
    ProviderConfig::new(ProviderId::DeepSeek, model).with_api_key("fixture-key")
}

fn external_control_dir() -> tempfile::TempDir {
    let directory = tempfile::tempdir().expect("create external orphan control dir");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(directory.path(), std::fs::Permissions::from_mode(0o733))
            .expect("make fixture control directory writable by the candidate identity");
    }
    directory
}

#[derive(Debug, Clone, Copy)]
struct OrphanState {
    pid: u32,
    port: u16,
    heartbeat: u64,
}

fn read_orphan_state(path: &std::path::Path) -> Option<OrphanState> {
    let contents = std::fs::read_to_string(path).ok()?;
    let value = |name: &str| contents.lines().find_map(|line| line.strip_prefix(name));
    Some(OrphanState {
        pid: value("pid=")?.parse().ok()?,
        port: value("port=")?.parse().ok()?,
        heartbeat: value("heartbeat=")?.parse().ok()?,
    })
}

#[cfg(target_os = "linux")]
fn read_migration_state(path: &std::path::Path) -> Option<(OrphanState, String)> {
    let contents = std::fs::read_to_string(path).ok()?;
    let migration = contents
        .lines()
        .find_map(|line| line.strip_prefix("migration="))?
        .to_string();
    Some((read_orphan_state(path)?, migration))
}

#[cfg(target_os = "linux")]
fn current_cgroup_directory() -> std::io::Result<std::path::PathBuf> {
    let current = std::fs::read_to_string("/proc/self/cgroup")?
        .lines()
        .find_map(|line| line.strip_prefix("0::"))
        .map(std::path::PathBuf::from)
        .ok_or_else(|| std::io::Error::other("no unified cgroup v2 entry"))?;
    let mountinfo = std::fs::read_to_string("/proc/self/mountinfo")?;
    for line in mountinfo.lines() {
        let Some((before, after)) = line.split_once(" - ") else {
            continue;
        };
        if after.split_whitespace().next() != Some("cgroup2") {
            continue;
        }
        let fields = before.split_whitespace().collect::<Vec<_>>();
        if fields.len() < 5 {
            continue;
        }
        let mount_root = std::path::Path::new(fields[3]);
        let mount_point = std::path::Path::new(fields[4]);
        let relative = current.strip_prefix(mount_root).map_err(|_| {
            std::io::Error::other("current cgroup is outside the cgroup v2 mount root")
        })?;
        return Ok(mount_point.join(relative));
    }
    Err(std::io::Error::other("no cgroup v2 mount found"))
}

#[cfg(target_os = "linux")]
fn remove_empty_cgroup(path: &std::path::Path) -> std::io::Result<()> {
    let deadline = std::time::Instant::now() + Duration::from_secs(1);
    loop {
        match std::fs::remove_dir(path) {
            Ok(()) => return Ok(()),
            Err(error)
                if error.raw_os_error() == Some(libc::EBUSY)
                    && std::time::Instant::now() < deadline =>
            {
                std::thread::sleep(Duration::from_millis(20));
            }
            Err(error) => return Err(error),
        }
    }
}

async fn wait_for_orphan_state(path: &std::path::Path, timeout: Duration) -> Option<OrphanState> {
    let deadline = tokio::time::Instant::now() + timeout;
    loop {
        if let Some(state) = read_orphan_state(path) {
            return Some(state);
        }
        if tokio::time::Instant::now() >= deadline {
            return None;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
}

#[cfg(unix)]
fn process_exists(pid: u32) -> bool {
    // SAFETY: signal 0 performs only an existence/permission check.
    let result = unsafe { libc::kill(pid as libc::pid_t, 0) };
    result == 0
        || !matches!(
            std::io::Error::last_os_error().raw_os_error(),
            Some(libc::ESRCH)
        )
}

#[cfg(windows)]
fn process_exists(pid: u32) -> bool {
    use windows_sys::Win32::Foundation::{CloseHandle, WAIT_OBJECT_0};
    use windows_sys::Win32::System::Threading::{OpenProcess, SYNCHRONIZE, WaitForSingleObject};

    // SAFETY: the handle is used only for a zero-time liveness query and is
    // closed on every successful OpenProcess path.
    let process = unsafe { OpenProcess(SYNCHRONIZE, 0, pid) };
    if process.is_null() {
        return false;
    }
    let exited = unsafe { WaitForSingleObject(process, 0) } == WAIT_OBJECT_0;
    unsafe { CloseHandle(process) };
    !exited
}

fn listener_accepts_connections(port: u16) -> bool {
    let address = std::net::SocketAddr::from(([127, 0, 0, 1], port));
    std::net::TcpStream::connect_timeout(&address, Duration::from_millis(50)).is_ok()
}

async fn wait_for_orphan_cleanup(state: OrphanState, timeout: Duration) -> bool {
    let deadline = tokio::time::Instant::now() + timeout;
    loop {
        let process_gone = !process_exists(state.pid);
        if process_gone && !listener_accepts_connections(state.port) {
            return true;
        }
        if tokio::time::Instant::now() >= deadline {
            return false;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

async fn emergency_kill_owned_orphan(state: OrphanState) {
    #[cfg(unix)]
    // SAFETY: the owned fixture inherits a fresh evaluator process group. The
    // negative PID targets that group; the direct PID is a final fallback.
    unsafe {
        libc::kill(-(state.pid as libc::pid_t), libc::SIGKILL);
        libc::kill(state.pid as libc::pid_t, libc::SIGKILL);
    }

    #[cfg(windows)]
    {
        use windows_sys::Win32::Foundation::CloseHandle;
        use windows_sys::Win32::System::Threading::{
            OpenProcess, PROCESS_TERMINATE, SYNCHRONIZE, TerminateProcess, WaitForSingleObject,
        };

        // SAFETY: the fixture PID names a test-owned process. The handle is
        // bounded-waited and closed on the only successful-open path.
        let process = unsafe { OpenProcess(PROCESS_TERMINATE | SYNCHRONIZE, 0, state.pid) };
        if !process.is_null() {
            unsafe {
                TerminateProcess(process, 1);
                WaitForSingleObject(process, 1_000);
                CloseHandle(process);
            }
        }
    }

    let _ = wait_for_orphan_cleanup(state, Duration::from_secs(1)).await;
}

#[cfg(target_os = "linux")]
async fn emergency_kill_orphan(state: OrphanState) {
    if process_exists(state.pid) {
        // SAFETY: the fixture deliberately makes itself the leader of a fresh
        // process group whose id equals its pid. This cleanup targets only that
        // test-owned group, then the pid as a fallback.
        unsafe {
            libc::kill(-(state.pid as libc::pid_t), libc::SIGKILL);
            libc::kill(state.pid as libc::pid_t, libc::SIGKILL);
        }
        let _ = wait_for_orphan_cleanup(state, Duration::from_secs(1)).await;
    }
}

async fn assert_owned_orphan_cleaned(
    control_path: &std::path::Path,
    result: &wcore_eval_scenarios::runner::ScenarioResult,
) {
    let state = wait_for_orphan_state(control_path, Duration::from_secs(1))
        .await
        .expect("owned descendant must publish pid, port, and heartbeat");
    let cleaned = wait_for_orphan_cleanup(state, Duration::from_secs(1)).await;
    if !cleaned {
        emergency_kill_owned_orphan(state).await;
    }
    assert!(
        cleaned,
        "owned descendant pid={} still listens on 127.0.0.1:{}",
        state.pid, state.port
    );
    if result.execution.containment_authoritative {
        assert!(
            result.execution.cleanup_verified,
            "authoritative runner reported unverified cleanup: {:?}",
            result.failures
        );
    } else {
        assert!(
            !result.execution.cleanup_verified,
            "non-authoritative cleanup must not be reported as verified"
        );
    }
    #[cfg(windows)]
    {
        assert!(result.execution.containment_authoritative);
        assert_eq!(result.execution.sandbox_backend, "windows-job-object");
    }
}

#[tokio::test]
async fn turn_deadline_is_enforced_as_over_time() {
    let scenario = Scenario::new("turn_deadline", Category::Hardening)
        .max_total_time(Duration::from_secs(2))
        .turn(Turn::new("wait").max_time(Duration::from_millis(50)));

    let result = run_with_binary(&scenario, &provider("fixture-slow"), fixture())
        .await
        .expect("runner returns a failed scenario result");

    assert!(
        result
            .failures
            .iter()
            .any(|failure| matches!(failure, Failure::OverTime { .. })),
        "expected OverTime, got {:?}",
        result.failures
    );
    assert!(result.wall_time < Duration::from_secs(1));
}

#[tokio::test]
async fn completed_tool_calls_obey_the_per_turn_step_ceiling() {
    let scenario = Scenario::new("turn_steps", Category::Hardening)
        .max_total_time(Duration::from_secs(2))
        .turn(Turn::new("use tools").max_steps(1));

    let result = run_with_binary(&scenario, &provider("fixture-steps"), fixture())
        .await
        .expect("runner returns a failed scenario result");

    assert!(result.failures.iter().any(|failure| matches!(
        failure,
        Failure::StepsExceeded {
            observed: 2,
            budget: 1
        }
    )));
    assert!(
        result
            .trace
            .entries
            .iter()
            .all(|entry| entry.duration.is_some()),
        "every correlated tool request/result must carry a duration: {:?}",
        result.trace.entries
    );
}

#[tokio::test]
async fn cleanup_runs_after_a_successful_scenario() {
    let cleanups = Arc::new(AtomicUsize::new(0));
    let observed = Arc::clone(&cleanups);
    let scenario = Scenario::new("cleanup_success", Category::Hardening)
        .turn(Turn::new("finish"))
        .cleanup(move |_| {
            observed.fetch_add(1, Ordering::SeqCst);
            Ok(())
        });

    let result = run_with_binary(&scenario, &provider("fixture"), fixture())
        .await
        .expect("scenario completes");

    assert!(result.passed, "unexpected failures: {:?}", result.failures);
    assert_eq!(cleanups.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn capability_startup_events_after_ready_satisfy_frozen_gate() {
    let scenario =
        Scenario::new("capability_honesty", Category::Hardening).turn(Turn::new("finish"));

    let result = run_with_binary(&scenario, &provider("fixture"), fixture())
        .await
        .expect("scenario completes");

    assert!(
        !result.failures.iter().any(|failure| matches!(
            failure,
            Failure::AssertionFailed { assertion, .. } if assertion == "CapabilityHonesty"
        )),
        "startup events emitted immediately after Ready were not captured: {:?}",
        result.failures
    );
}

#[tokio::test]
async fn missing_capability_startup_event_fails_frozen_gate() {
    let scenario =
        Scenario::new("capability_honesty_missing", Category::Hardening).turn(Turn::new("finish"));

    let result = run_with_binary(
        &scenario,
        &provider("fixture-missing-capability"),
        fixture(),
    )
    .await
    .expect("runner returns a failed scenario result");

    assert!(
        result.failures.iter().any(|failure| matches!(
            failure,
            Failure::AssertionFailed { assertion, observed }
                if assertion == "CapabilityHonesty"
                    && observed.contains("DelegateIsolation: missing declared")
                    && observed.contains("activation proof rate 0.875")
        )),
        "missing startup proof must fail the frozen threshold: {:?}",
        result.failures
    );
}

#[tokio::test]
async fn ready_without_construction_fails_frozen_capability_gate() {
    let scenario = Scenario::new("capability_honesty_unconstructed", Category::Hardening)
        .turn(Turn::new("finish"));

    let result = run_with_binary(
        &scenario,
        &provider("fixture-unconstructed-capability"),
        fixture(),
    )
    .await
    .expect("runner returns a failed scenario result");

    assert!(
        result.failures.iter().any(|failure| matches!(
            failure,
            Failure::AssertionFailed { assertion, observed }
                if assertion == "CapabilityHonesty"
                    && observed.contains("SmartHandoff: illegal transition Configured -> Ready")
                    && observed.contains("activation proof rate 0.875")
        )),
        "constructed stage must be required before ready: {:?}",
        result.failures
    );
}

#[tokio::test]
async fn cleanup_runs_when_setup_fails() {
    let cleanups = Arc::new(AtomicUsize::new(0));
    let observed = Arc::clone(&cleanups);
    let scenario = Scenario::new("cleanup_setup_error", Category::Hardening)
        .setup(|_| anyhow::bail!("fixture setup failure"))
        .cleanup(move |_| {
            observed.fetch_add(1, Ordering::SeqCst);
            Ok(())
        });

    let error = run_with_binary(&scenario, &provider("fixture"), fixture())
        .await
        .expect_err("setup error must propagate");

    assert!(error.to_string().contains("scenario setup failed"));
    assert_eq!(cleanups.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn latest_multi_turn_cost_aggregate_wins() {
    let scenario = Scenario::new("latest_cost", Category::Hardening)
        .turn(Turn::new("first"))
        .turn(Turn::new("second"));

    let result = run_with_binary(&scenario, &provider("fixture-cost"), fixture())
        .await
        .expect("scenario completes");

    assert_eq!(result.cost_usd, 0.02);
}

#[tokio::test]
async fn missing_cost_evidence_fails_closed() {
    let scenario =
        Scenario::new("missing_cost", Category::Hardening).turn(Turn::new("no accounting"));

    let result = run_with_binary(&scenario, &provider("fixture-no-cost"), fixture())
        .await
        .expect("runner returns a failed result");

    assert!(
        result
            .failures
            .iter()
            .any(|failure| matches!(failure, Failure::CostMissing))
    );
}

#[tokio::test]
async fn oversized_unterminated_stdout_is_a_bounded_runner_error() {
    let scenario = Scenario::new("oversized_stdout", Category::Hardening)
        .max_total_time(Duration::from_secs(2))
        .turn(Turn::new("emit oversized protocol data").max_time(Duration::from_millis(250)));

    let result = run_with_binary(&scenario, &provider("fixture-oversized-stdout"), fixture())
        .await
        .expect("runner returns a typed failure result");

    assert!(
        result.failures.iter().any(|failure| {
            matches!(failure, Failure::RunnerError(message) if message.contains("stdout event exceeded 65536 bytes"))
        }),
        "oversized protocol data must be classified as a bounded runner error, got {:?}",
        result.failures
    );
    assert!(
        result.wall_time < Duration::from_secs(1),
        "output limit should fail before the scenario deadline: {:?}",
        result.wall_time
    );
}

#[cfg(target_os = "linux")]
#[tokio::test]
async fn timeout_reaps_detached_descendant_and_listener() {
    if std::env::var_os("WCORE_EVAL_REQUIRE_CONTAINMENT").is_none() {
        eprintln!("skipping authoritative cgroup contract outside the containment gate");
        return;
    }
    let control_dir = external_control_dir();
    let control_path = control_dir.path().join("orphan-state");
    let model = format!("fixture-orphan:{}", control_path.display());
    let scenario = Scenario::new("detached_orphan", Category::Hardening)
        .max_total_time(Duration::from_secs(2))
        .turn(Turn::new("spawn detached listener").max_time(Duration::from_millis(500)));

    let result = run_with_binary(&scenario, &provider(&model), fixture())
        .await
        .expect("timeout returns a failed scenario result");
    assert!(
        result
            .failures
            .iter()
            .any(|failure| matches!(failure, Failure::OverTime { .. })),
        "fixture must exercise the turn-timeout cleanup path: {:?}",
        result.failures
    );

    let state = wait_for_orphan_state(&control_path, Duration::from_secs(1))
        .await
        .expect("detached fixture must publish pid, port, and heartbeat");
    let cleaned = wait_for_orphan_cleanup(state, Duration::from_millis(500)).await;
    let final_state = read_orphan_state(&control_path).unwrap_or(state);

    // This test is intentionally red against direct-child-only cleanup. Always
    // reap the escaped fixture before asserting so a red run leaves no residue.
    if !cleaned {
        emergency_kill_orphan(final_state).await;
    }

    assert!(
        cleaned,
        "timeout left detached descendant pid={} listening on 127.0.0.1:{}; heartbeat advanced {} -> {}",
        state.pid, state.port, state.heartbeat, final_state.heartbeat
    );
}

#[cfg(target_os = "linux")]
#[tokio::test]
async fn candidate_cannot_migrate_descendants_to_parent_or_sibling_cgroups() {
    if std::env::var_os("WCORE_EVAL_REQUIRE_CONTAINMENT").is_none() {
        eprintln!("skipping cgroup authority attack outside the containment gate");
        return;
    }

    let cgroup_parent = current_cgroup_directory().expect("resolve test cgroup");
    let sequence = MIGRATION_CGROUP_COUNTER.fetch_add(1, Ordering::Relaxed);
    let sibling = cgroup_parent.join(format!(
        "wayland-eval-hostile-sibling-{}-{sequence}",
        std::process::id()
    ));
    std::fs::create_dir(&sibling).expect("create hostile sibling cgroup");

    let seed_provider = provider("fixture");
    let env = tempenv::build(&seed_provider).expect("create hostile fixture environment");
    let control_dir = env.path().join("migration-state");
    std::fs::create_dir(&control_dir).expect("create migration control directory");
    let model = format!(
        "fixture-cgroup-migration:{}|{}",
        sibling.display(),
        control_dir.display()
    );
    let scenario = Scenario::new("cgroup_authority_escape", Category::Hardening)
        .max_total_time(Duration::from_secs(2))
        .turn(Turn::new("attempt parent and sibling cgroup migration"));

    let outcome =
        run_with_binary_in_environment(&scenario, &provider(&model), fixture(), &env).await;

    let mut states = Vec::new();
    for name in ["parent", "sibling"] {
        let path = control_dir.join(format!("{name}-state"));
        let state = wait_for_orphan_state(&path, Duration::from_secs(1))
            .await
            .unwrap_or_else(|| panic!("{name} migration fixture did not publish state"));
        let migration = read_migration_state(&path)
            .map(|(_, migration)| migration)
            .expect("migration result must be recorded");
        states.push((name, state, migration));
    }

    for (_, state, _) in &states {
        if !wait_for_orphan_cleanup(*state, Duration::from_millis(500)).await {
            emergency_kill_owned_orphan(*state).await;
        }
    }
    remove_empty_cgroup(&sibling).expect("remove hostile sibling cgroup");

    let result = outcome.expect("authority attack returns a scenario result");
    for (name, state, migration) in states {
        assert!(
            migration.starts_with("denied:"),
            "{name} migration was not denied: {migration} (pid={})",
            state.pid
        );
        assert!(
            !process_exists(state.pid) && !listener_accepts_connections(state.port),
            "denied {name} migration descendant survived cleanup"
        );
    }
    assert!(result.execution.containment_authoritative);
    assert!(result.execution.cleanup_verified);
    assert!(result.passed, "unexpected failures: {:?}", result.failures);
}

#[tokio::test]
async fn normal_exit_reaps_owned_descendant_listener() {
    let control_dir = external_control_dir();
    let control_path = control_dir.path().join("owned-orphan-state");
    let model = format!("fixture-owned-orphan:{}", control_path.display());
    let scenario = Scenario::new("owned_orphan_normal_exit", Category::Hardening)
        .max_total_time(Duration::from_secs(2))
        .turn(Turn::new("spawn inherited listener"));

    let result = run_with_binary(&scenario, &provider(&model), fixture())
        .await
        .expect("normal run returns a scenario result");

    assert_owned_orphan_cleaned(&control_path, &result).await;
    assert!(result.passed, "unexpected failures: {:?}", result.failures);
}

#[tokio::test]
async fn timeout_reaps_owned_descendant_listener() {
    let control_dir = external_control_dir();
    let control_path = control_dir.path().join("owned-orphan-state");
    let model = format!("fixture-owned-orphan-timeout:{}", control_path.display());
    let scenario = Scenario::new("owned_orphan_timeout", Category::Hardening)
        .max_total_time(Duration::from_secs(2))
        .turn(Turn::new("spawn inherited listener").max_time(Duration::from_millis(250)));

    let result = run_with_binary(&scenario, &provider(&model), fixture())
        .await
        .expect("timeout returns a failed scenario result");

    assert_owned_orphan_cleaned(&control_path, &result).await;
    assert!(
        result
            .failures
            .iter()
            .any(|failure| matches!(failure, Failure::OverTime { .. }))
    );
}

#[tokio::test]
async fn outer_deadline_reaps_owned_descendant_listener() {
    let control_dir = external_control_dir();
    let control_path = control_dir.path().join("owned-orphan-state");
    let model = format!("fixture-owned-orphan-timeout:{}", control_path.display());
    let scenario = Scenario::new("owned_orphan_outer_timeout", Category::Hardening)
        .max_total_time(Duration::from_secs(1))
        .turn(Turn::new("spawn inherited listener").max_time(Duration::from_secs(5)));

    let result = run_with_binary(&scenario, &provider(&model), fixture())
        .await
        .expect("outer timeout returns a failed scenario result");

    assert_owned_orphan_cleaned(&control_path, &result).await;
    assert!(
        result
            .failures
            .iter()
            .any(|failure| matches!(failure, Failure::Hung { .. }))
    );
}

#[tokio::test]
async fn cancellation_reaps_owned_descendant_listener() {
    let control_dir = external_control_dir();
    let control_path = control_dir.path().join("owned-orphan-state");
    let stop_marker = control_path.with_extension("stop-observed");
    let model = format!("fixture-owned-orphan-cancel:{}", control_path.display());
    let scenario = Scenario::new("owned_orphan_cancellation", Category::Hardening)
        .max_total_time(Duration::from_secs(2))
        .turn(Turn::new("spawn inherited listener").stop_mid_turn());

    let result = run_with_binary(&scenario, &provider(&model), fixture())
        .await
        .expect("cancelled run returns a scenario result");

    assert_owned_orphan_cleaned(&control_path, &result).await;
    assert!(result.execution.cancellation_requested);
    assert!(
        stop_marker.exists(),
        "fixture never observed the stop command"
    );
    assert!(
        !result
            .failures
            .iter()
            .any(|failure| matches!(failure, Failure::Hung { .. } | Failure::OverTime { .. }))
    );
}

#[tokio::test]
async fn assertion_failure_still_reaps_owned_descendant_listener() {
    let control_dir = external_control_dir();
    let control_path = control_dir.path().join("owned-orphan-state");
    let model = format!("fixture-owned-orphan:{}", control_path.display());
    let scenario = Scenario::new("owned_orphan_assertion", Category::Hardening)
        .max_total_time(Duration::from_secs(2))
        .turn(
            Turn::new("spawn inherited listener")
                .assert(Assertion::Contains("deliberately absent")),
        );

    let result = run_with_binary(&scenario, &provider(&model), fixture())
        .await
        .expect("assertion failure returns a scenario result");

    assert_owned_orphan_cleaned(&control_path, &result).await;
    assert!(result.failures.iter().any(|failure| matches!(
        failure,
        Failure::AssertionFailed { assertion, .. }
            if assertion.contains("deliberately absent")
    )));
}

#[tokio::test]
async fn direct_child_early_exit_reaps_owned_descendant_listener() {
    let control_dir = external_control_dir();
    let control_path = control_dir.path().join("owned-orphan-state");
    let model = format!("fixture-owned-orphan-exit:{}", control_path.display());
    let scenario = Scenario::new("owned_orphan_early_exit", Category::Hardening)
        .max_total_time(Duration::from_secs(2))
        .turn(Turn::new("spawn inherited listener and exit"));

    let result = run_with_binary(&scenario, &provider(&model), fixture())
        .await
        .expect("early exit returns a failed scenario result");

    assert_owned_orphan_cleaned(&control_path, &result).await;
    assert!(result.failures.iter().any(
        |failure| matches!(failure, Failure::RunnerError(message) if message.contains("stdout"))
    ));
}

#[tokio::test]
async fn dropping_runner_future_reaps_owned_descendant_listener() {
    let control_dir = external_control_dir();
    let control_path = control_dir.path().join("owned-orphan-state");
    let task_control_path = control_path.clone();
    let task = tokio::spawn(async move {
        let model = format!(
            "fixture-owned-orphan-timeout:{}",
            task_control_path.display()
        );
        let scenario = Scenario::new("owned_orphan_future_drop", Category::Hardening)
            .max_total_time(Duration::from_secs(30))
            .turn(Turn::new("spawn inherited listener").max_time(Duration::from_secs(30)));
        run_with_binary(&scenario, &provider(&model), fixture()).await
    });

    // A fixed authoritative candidate identity serializes concurrent evaluator
    // runs. Allow this cancellation probe to reach the front of that queue
    // before requiring the descendant to publish its state.
    let state = wait_for_orphan_state(&control_path, Duration::from_secs(10))
        .await
        .expect("owned descendant must publish before cancellation");
    task.abort();
    let join_error = task.await.expect_err("runner task should be cancelled");

    let cleaned = wait_for_orphan_cleanup(state, Duration::from_secs(2)).await;
    if !cleaned {
        emergency_kill_owned_orphan(state).await;
    }
    assert!(join_error.is_cancelled());
    assert!(
        cleaned,
        "dropping the runner future left descendant pid={} listening on 127.0.0.1:{}",
        state.pid, state.port
    );
}

#[tokio::test]
async fn run_is_hermetic_and_redacts_child_secret_exfiltration() {
    const SECRET: &str = "wcore-canary-secret-7f84c1";
    let _poison = EnvGuard::poison();
    let scenario = Scenario::new("hermetic", Category::Hardening)
        .max_total_cost_usd(0.031)
        .turn(Turn::new("inspect containment"));

    let result = run_with_binary(
        &scenario,
        &ProviderConfig::new(ProviderId::DeepSeek, "fixture-hermetic").with_api_key(SECRET),
        fixture(),
    )
    .await
    .expect("runner completes hermetic fixture");

    assert!(
        result.final_text.contains(
            "arg_secret=false config_secret=false key_env=false poison=false budget=true"
        ),
        "fixture observed an uncontained run: {}",
        result.final_text
    );
    let retained = serde_json::to_string(&result).expect("serialize retained result");
    assert!(
        !retained.contains(SECRET),
        "canary secret survived in retained result: {retained}"
    );
    assert!(
        retained.contains("[REDACTED]"),
        "malicious leak was not visibly redacted"
    );
    assert!(
        result
            .failures
            .iter()
            .any(|failure| matches!(failure, Failure::SecretDetected { sink } if sink == "stdout")),
        "capture-time stdout redaction must fail the run: {:?}",
        result.failures
    );
}

#[test]
fn provider_debug_never_exposes_api_key() {
    const SECRET: &str = "wcore-canary-secret-debug";
    let provider = ProviderConfig::new(ProviderId::Anthropic, "fixture").with_api_key(SECRET);
    let debug = format!("{provider:?}");
    assert!(
        !debug.contains(SECRET),
        "provider Debug leaked its key: {debug}"
    );
    assert!(
        debug.contains("[REDACTED]"),
        "provider Debug hid key presence: {debug}"
    );
}

struct EnvGuard(Vec<(&'static str, Option<std::ffi::OsString>)>);

impl EnvGuard {
    fn poison() -> Self {
        let names = [
            "HOME",
            "XDG_CONFIG_HOME",
            "XDG_CACHE_HOME",
            "GIT_CONFIG_GLOBAL",
            "SSH_AUTH_SOCK",
            "HTTPS_PROXY",
            "PATH",
        ];
        let previous = names
            .into_iter()
            .map(|name| {
                let old = std::env::var_os(name);
                // SAFETY: nextest runs each integration test in its own process;
                // this test restores every value before that process exits.
                unsafe { std::env::set_var(name, "wcore-poison") };
                (name, old)
            })
            .collect();
        Self(previous)
    }
}

impl Drop for EnvGuard {
    fn drop(&mut self) {
        for (name, value) in self.0.drain(..) {
            // SAFETY: paired restoration for the process-local test mutation.
            unsafe {
                match value {
                    Some(value) => std::env::set_var(name, value),
                    None => std::env::remove_var(name),
                }
            }
        }
    }
}
