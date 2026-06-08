//! Mesh topology dispatcher.
//!
//! In-process coordinator for up to `Topology::Mesh` agents (50) that
//! share a [`Blackboard`] partition. Agents are closures (this crate
//! does NOT spawn subprocesses — that's the orchestrator's concern);
//! the dispatcher's value-add is the cap enforcement, the shared
//! blackboard wiring, the timeout, and the reducer.
//!
//! Lifecycle:
//!   * Caller hands [`MeshDispatcher::dispatch`] a `Vec<MeshAgent>`.
//!   * Each agent runs concurrently with its own `BlackboardCtx`.
//!   * After all complete (or `timeout` elapses), the supplied reducer
//!     collapses the per-agent `AgentReport`s into a single `MeshResult`.
//!
//! All blackboard scoping is `SharedTopicTree` — matches the locked
//! 4.B.2 configuration for `Topology::Mesh`.

use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::time::Duration;

use serde::{Deserialize, Serialize};
use tokio::task::JoinSet;
use tokio::time::error::Elapsed;

use wcore_memory::partition::collaboration::Blackboard;

use crate::topology::{Topology, TopologyError};

/// Per-agent context handed to each closure in a mesh dispatch.
#[derive(Clone)]
pub struct BlackboardCtx {
    pub board: Arc<Blackboard>,
    pub agent_id: String,
    /// Topic prefix the agent is scoped to (e.g. `"mesh/<run-id>/"`).
    pub topic_prefix: String,
}

/// Result of a single agent's run.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentReport {
    pub agent_id: String,
    /// Free-form payload — reducer interprets.
    pub payload: serde_json::Value,
    pub succeeded: bool,
}

/// Boxed-future closure type for mesh agents.
pub type MeshAgent = Box<
    dyn FnOnce(BlackboardCtx) -> Pin<Box<dyn Future<Output = AgentReport> + Send>> + Send + 'static,
>;

/// Reducer: collapse N reports into one result.
pub type Reducer<T> = Box<dyn FnOnce(Vec<AgentReport>) -> T + Send + 'static>;

#[derive(Debug, thiserror::Error)]
pub enum MeshError {
    #[error(transparent)]
    Topology(#[from] TopologyError),
    #[error("mesh dispatch timed out after {0:?}")]
    Timeout(Duration),
}

/// In-process mesh dispatcher.
pub struct MeshDispatcher {
    board: Arc<Blackboard>,
    run_id: String,
    timeout: Duration,
}

impl MeshDispatcher {
    /// Build a dispatcher backed by a fresh in-memory blackboard.
    pub fn new(run_id: impl Into<String>) -> Self {
        Self {
            board: Arc::new(Blackboard::new()),
            run_id: run_id.into(),
            timeout: Duration::from_secs(300),
        }
    }

    /// Build a dispatcher with a caller-supplied blackboard (useful for
    /// integration tests + sharing state across nested dispatches).
    pub fn with_board(run_id: impl Into<String>, board: Arc<Blackboard>) -> Self {
        Self {
            board,
            run_id: run_id.into(),
            timeout: Duration::from_secs(300),
        }
    }

    pub fn with_timeout(mut self, t: Duration) -> Self {
        self.timeout = t;
        self
    }

    /// Borrow the shared blackboard (read-only access for callers).
    pub fn board(&self) -> &Arc<Blackboard> {
        &self.board
    }

    /// Dispatch `agents` concurrently and apply `reducer` to the
    /// collected reports.
    pub async fn dispatch<T>(
        &self,
        agents: Vec<MeshAgent>,
        reducer: Reducer<T>,
    ) -> Result<T, MeshError>
    where
        T: Send + 'static,
    {
        // Cap enforcement against the locked Topology::Mesh config.
        let cfg = Topology::Mesh.default_config();
        cfg.validate_count(agents.len() as u32)?;

        let topic_prefix = format!("mesh/{}/", self.run_id);

        let mut set: JoinSet<AgentReport> = JoinSet::new();
        for (i, agent) in agents.into_iter().enumerate() {
            let ctx = BlackboardCtx {
                board: self.board.clone(),
                agent_id: format!("{}-{}", self.run_id, i),
                topic_prefix: topic_prefix.clone(),
            };
            set.spawn(agent(ctx));
        }

        let collected = match tokio::time::timeout(self.timeout, async move {
            let mut out = Vec::new();
            while let Some(j) = set.join_next().await {
                match j {
                    Ok(r) => out.push(r),
                    Err(e) => out.push(AgentReport {
                        agent_id: "<join_error>".to_string(),
                        payload: serde_json::json!({ "join_error": e.to_string() }),
                        succeeded: false,
                    }),
                }
            }
            out
        })
        .await
        {
            Ok(v) => v,
            Err(_e) => {
                // Elapsed (`_e` is `tokio::time::error::Elapsed`).
                // We can't recover already-running agents — surface the
                // timeout and let the caller decide what to do.
                let _: Elapsed = _e;
                return Err(MeshError::Timeout(self.timeout));
            }
        };

        Ok(reducer(collected))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn agent_report(id: &str, payload: serde_json::Value, ok: bool) -> AgentReport {
        AgentReport {
            agent_id: id.to_string(),
            payload,
            succeeded: ok,
        }
    }

    #[tokio::test]
    async fn happy_path_three_agents_each_post_and_succeed() {
        let mesh = MeshDispatcher::new("run-1");
        let mk_agent = |i: usize| -> MeshAgent {
            Box::new(move |ctx: BlackboardCtx| {
                Box::pin(async move {
                    use wcore_memory::partition::collaboration::BlackboardEntry;
                    let topic = format!("{}step/{}", ctx.topic_prefix, i);
                    ctx.board.write(BlackboardEntry::new(
                        topic,
                        json!({ "i": i }),
                        ctx.agent_id.clone(),
                    ));
                    agent_report(&ctx.agent_id, json!({"i": i}), true)
                })
            })
        };
        let agents: Vec<MeshAgent> = vec![mk_agent(0), mk_agent(1), mk_agent(2)];

        let reducer: Reducer<usize> =
            Box::new(|reports: Vec<AgentReport>| reports.iter().filter(|r| r.succeeded).count());

        let n = mesh.dispatch(agents, reducer).await.unwrap();
        assert_eq!(n, 3);

        // Blackboard saw all three writes.
        let board_view = mesh.board().read_prefix("mesh/run-1/");
        assert_eq!(board_view.len(), 3);
    }

    #[tokio::test]
    async fn over_cap_errors_with_topology_exceeds_cap() {
        let mesh = MeshDispatcher::new("run-2");
        // 51 trivial agents → must exceed Mesh cap of 50.
        let agents: Vec<MeshAgent> = (0..51)
            .map(|i| -> MeshAgent {
                Box::new(move |ctx: BlackboardCtx| {
                    Box::pin(async move { agent_report(&ctx.agent_id, json!({"i": i}), true) })
                })
            })
            .collect();
        let reducer: Reducer<()> = Box::new(|_| ());
        let err = mesh.dispatch(agents, reducer).await.unwrap_err();
        assert!(matches!(
            err,
            MeshError::Topology(TopologyError::ExceedsCap {
                cap: 50,
                requested: 51,
                ..
            })
        ));
    }

    #[tokio::test]
    async fn timeout_surfaces_mesh_error() {
        let mesh = MeshDispatcher::new("run-3").with_timeout(Duration::from_millis(50));
        let agents: Vec<MeshAgent> = vec![Box::new(|ctx: BlackboardCtx| {
            Box::pin(async move {
                tokio::time::sleep(Duration::from_secs(5)).await;
                agent_report(&ctx.agent_id, json!({}), true)
            })
        })];
        let reducer: Reducer<()> = Box::new(|_| ());
        let err = mesh.dispatch(agents, reducer).await.unwrap_err();
        assert!(matches!(err, MeshError::Timeout(_)));
    }

    #[tokio::test]
    async fn reducer_aggregates_mixed_success_failure() {
        let mesh = MeshDispatcher::new("run-4");
        let agents: Vec<MeshAgent> = (0..4)
            .map(|i| -> MeshAgent {
                Box::new(move |ctx: BlackboardCtx| {
                    Box::pin(
                        async move { agent_report(&ctx.agent_id, json!({"i": i}), i % 2 == 0) },
                    )
                })
            })
            .collect();
        let reducer: Reducer<(usize, usize)> = Box::new(|reports: Vec<AgentReport>| {
            let ok = reports.iter().filter(|r| r.succeeded).count();
            let bad = reports.iter().filter(|r| !r.succeeded).count();
            (ok, bad)
        });
        let (ok, bad) = mesh.dispatch(agents, reducer).await.unwrap();
        assert_eq!(ok, 2);
        assert_eq!(bad, 2);
    }

    #[tokio::test]
    async fn shared_board_visible_across_agents() {
        let mesh = MeshDispatcher::new("run-5");

        let agents: Vec<MeshAgent> = vec![
            Box::new(|ctx: BlackboardCtx| {
                Box::pin(async move {
                    use wcore_memory::partition::collaboration::BlackboardEntry;
                    let topic = format!("{}note", ctx.topic_prefix);
                    ctx.board.write(BlackboardEntry::new(
                        topic,
                        json!({ "from": ctx.agent_id.clone() }),
                        ctx.agent_id.clone(),
                    ));
                    agent_report(&ctx.agent_id, json!({}), true)
                })
            }),
            Box::new(|ctx: BlackboardCtx| {
                Box::pin(async move {
                    // Yield once so the first agent's write lands before we read.
                    tokio::task::yield_now().await;
                    tokio::time::sleep(Duration::from_millis(20)).await;
                    let topic = format!("{}note", ctx.topic_prefix);
                    let saw = !ctx.board.read_topic(&topic).is_empty();
                    agent_report(&ctx.agent_id, json!({ "saw_first": saw }), saw)
                })
            }),
        ];

        let reducer: Reducer<bool> = Box::new(|reports: Vec<AgentReport>| {
            reports.iter().any(|r| {
                r.payload
                    .get("saw_first")
                    .and_then(|v| v.as_bool())
                    .unwrap_or(false)
            })
        });
        let saw_through_board = mesh.dispatch(agents, reducer).await.unwrap();
        assert!(saw_through_board);
    }
}
