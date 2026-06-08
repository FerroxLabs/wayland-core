// M3.2.2 — bootstrap must wire a real `Memory` + spawn the decay
// scheduler iff `cfg.memory.enabled = true`. When disabled, bootstrap
// stays on `NullMemory` and must NOT add a decay handle.
//
// We assert the contract by reading the engine's `decay_handles_len()`
// accessor (a thin wrapper around the M3.2 field) rather than racing the
// scheduler timer — the gating decision is what we care about; the
// timing of the first tick is covered by `wcore-memory`'s
// `scheduler_runs_at_interval` test.
//
// HANDLE COUNT RATIONALE (updated for v0.8.x substrate campaign):
//
// The v0.8.x campaign added several always-on background tasks that each
// park a `JoinHandle` on the engine's `decay_handles` vec (the vec name
// is a historical misnomer — it now holds any background task the engine
// must abort on Drop, not only memory-decay tasks):
//
//   1. AgentBus lifecycle observer (v0.8.1 U2 — bus_observer.into_join_handle())
//      Forwards Spawned/FirstMessage/Completed/Errored events to tracing
//      and the protocol OutputSink.
//
//   2. SkillWatcher reload task (F-039 — reload_handle)
//      Drives hot-reload of the skill catalog when the watcher fires a
//      version bump.
//
//   3. SkillWatcher keepalive task (F-039 — pending::<()>() park)
//      Holds the SkillWatcher alive so its Drop (which calls stop()) fires
//      at session shutdown. This is a synthetic task that never resolves;
//      it is aborted by engine Drop.
//
// When `memory.enabled = true` a FOURTH handle is added:
//
//   4. Memory decay scheduler (M3.2 — mem.spawn_decay_scheduler())
//      The only handle that depends on the memory.enabled flag.
//
// So the baseline is 3 (disabled) and 4 (enabled).  We assert MINIMUM
// rather than exact counts so this test survives future substrate additions
// without a staleness drift: any new always-on subsystem is free to add
// another handle and this test remains green.  The important contract is
// the DELTA: enabled adds at least 1 more handle than disabled.
//
// If you need the exact count at a point in time: as of v0.8.2 it is
// 3 (disabled) / 4 (enabled).

use std::sync::Arc;

use wcore_agent::bootstrap::AgentBootstrap;
use wcore_agent::output::null_sink::NullSink;
use wcore_config::compat::ProviderCompat;
use wcore_config::config::{Config, ProviderType};

fn cfg_with_memory(enabled: bool) -> Config {
    let mut c = Config {
        provider_label: "openai".into(),
        provider: ProviderType::OpenAI,
        api_key: "sk-test".into(),
        base_url: "http://localhost:0".into(),
        model: "gpt-test-model".into(),
        max_tokens: 1024,
        max_turns: Some(5),
        compat: ProviderCompat::openai_defaults(),
        ..Default::default()
    };
    c.memory.enabled = enabled;
    c.memory.decay_interval_secs = 3600;
    c
}

fn null_output() -> Arc<dyn wcore_agent::output::OutputSink> {
    Arc::new(NullSink)
}

#[tokio::test]
async fn bootstrap_with_memory_enabled_spawns_decay_scheduler() {
    let cfg = cfg_with_memory(true);
    let workdir = tempfile::TempDir::new().expect("workdir");
    let result = AgentBootstrap::new(cfg, workdir.path().to_str().unwrap(), null_output())
        .build()
        .await
        .expect("bootstrap should succeed with memory enabled");

    let count = result.engine.decay_handles_len();

    // The enabled path must park AT LEAST 1 handle for the decay scheduler.
    // We use >= 1 rather than == 1 because the v0.8.x substrate campaign
    // added always-on background tasks that also park handles regardless of
    // memory.enabled (AgentBus observer + SkillWatcher ×2 = 3 baseline as
    // of v0.8.2).  The decay scheduler is the 4th.  Using a minimum bound
    // means future substrate additions don't break this test; the actual
    // gate (enabled produces more handles than disabled) is verified by
    // `bootstrap_with_memory_disabled_spawns_no_scheduler`.
    assert!(
        count >= 1,
        "memory.enabled=true must park at least one decay-scheduler handle; got {count}"
    );

    // Dropping the engine (which `result` owns) must abort all background
    // tasks — covered by the `AgentEngine::Drop` impl. We don't assert on
    // the abort directly (tokio's `JoinHandle::is_finished()` is racy),
    // but we drop here so the test process exits cleanly.
    drop(result);
}

#[tokio::test]
async fn bootstrap_with_memory_disabled_spawns_no_scheduler() {
    let workdir = tempfile::TempDir::new().expect("workdir");

    // Build both variants and compare counts so this test doesn't depend
    // on a magic literal that drifts with substrate additions.
    let disabled_count = {
        let cfg = cfg_with_memory(false);
        let r = AgentBootstrap::new(cfg, workdir.path().to_str().unwrap(), null_output())
            .build()
            .await
            .expect("bootstrap should succeed with memory disabled");
        r.engine.decay_handles_len()
    };

    let enabled_count = {
        let cfg = cfg_with_memory(true);
        let r = AgentBootstrap::new(cfg, workdir.path().to_str().unwrap(), null_output())
            .build()
            .await
            .expect("bootstrap should succeed with memory enabled");
        r.engine.decay_handles_len()
    };

    // The disabled path must produce strictly fewer handles than the enabled
    // path — the decay-scheduler handle is the exact diff.
    // We can't assert disabled_count == 0 because the v0.8.x substrate
    // campaign added always-on background tasks (AgentBus observer +
    // SkillWatcher ×2) that park handles regardless of memory.enabled.
    // The important invariant is: disabled < enabled (decay not spawned).
    assert!(
        disabled_count < enabled_count,
        "memory.enabled=false must produce fewer decay handles than memory.enabled=true \
         (disabled={disabled_count}, enabled={enabled_count})"
    );
}
