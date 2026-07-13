use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use wcore_eval_scenarios::providers::{ProviderConfig, ProviderId};
use wcore_eval_scenarios::runner::{Failure, run_with_binary};
use wcore_eval_scenarios::scenario::{Category, Scenario, Turn};

fn fixture() -> &'static std::path::Path {
    std::path::Path::new(env!("CARGO_BIN_EXE_wcore-eval-fixture"))
}

fn provider(model: &str) -> ProviderConfig {
    ProviderConfig::new(ProviderId::DeepSeek, model).with_api_key("fixture-key")
}

#[cfg(target_os = "linux")]
#[derive(Debug, Clone, Copy)]
struct OrphanState {
    pid: u32,
    port: u16,
    heartbeat: u64,
}

#[cfg(target_os = "linux")]
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

#[cfg(target_os = "linux")]
fn process_exists(pid: u32) -> bool {
    // SAFETY: signal 0 performs only an existence/permission check.
    let result = unsafe { libc::kill(pid as libc::pid_t, 0) };
    result == 0
        || !matches!(
            std::io::Error::last_os_error().raw_os_error(),
            Some(libc::ESRCH)
        )
}

#[cfg(target_os = "linux")]
fn listener_accepts_connections(port: u16) -> bool {
    let address = std::net::SocketAddr::from(([127, 0, 0, 1], port));
    std::net::TcpStream::connect_timeout(&address, Duration::from_millis(50)).is_ok()
}

#[cfg(target_os = "linux")]
async fn wait_for_orphan_cleanup(state: OrphanState, timeout: Duration) -> bool {
    let deadline = tokio::time::Instant::now() + timeout;
    loop {
        if !process_exists(state.pid) && !listener_accepts_connections(state.port) {
            return true;
        }
        if tokio::time::Instant::now() >= deadline {
            return false;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
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
    let control_dir = tempfile::tempdir().expect("create external orphan control dir");
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
