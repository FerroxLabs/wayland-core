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
        result
            .final_text
            .contains("arg_secret=false config_secret=false key_env=true poison=false budget=true"),
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
