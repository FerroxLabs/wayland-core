//! Fleet topology dispatcher.
//!
//! Hierarchical reducer for up to `Topology::Fleet` agents (100). The
//! fleet partitions its agent vector into shards of size `shard_size`
//! (default 10), runs each shard as an inner [`MeshDispatcher`] that
//! reduces N agents into 1 shard summary, and finally applies a top-level
//! reducer over the vector of shard summaries to produce the fleet result.
//!
//! Each shard gets its own topic prefix (`fleet/<run-id>/shard-<i>/`),
//! matching the locked `BlackboardScope::ShardedByTier` configuration
//! for `Topology::Fleet` from 4.B.2.

use std::sync::Arc;
use std::time::Duration;

use serde::{Deserialize, Serialize};
use tokio::task::JoinSet;

use wcore_memory::partition::collaboration::Blackboard;

use crate::mesh::{AgentReport, MeshAgent, MeshDispatcher, MeshError, Reducer};
use crate::topology::{Topology, TopologyError};

/// Default shard size — 10 agents per inner Mesh.
pub const DEFAULT_SHARD_SIZE: usize = 10;

/// Summary returned by each shard. Cheap to clone — the heavy lifting
/// happens before the reducer hands this back to the fleet.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ShardSummary {
    pub shard_id: usize,
    pub agent_count: usize,
    pub successes: usize,
    pub failures: usize,
    /// Free-form payload produced by the shard's reducer.
    pub payload: serde_json::Value,
}

/// Boxed reducer collapsing a single shard's reports into a `ShardSummary`.
pub type ShardReducer = Box<dyn FnOnce(usize, Vec<AgentReport>) -> ShardSummary + Send + 'static>;

/// Boxed reducer collapsing all shard summaries into the final fleet result.
pub type FleetReducer<T> = Box<dyn FnOnce(Vec<ShardSummary>) -> T + Send + 'static>;

/// Default shard reducer: count successes/failures, attach an empty payload.
pub fn default_shard_reducer(shard_id: usize, reports: Vec<AgentReport>) -> ShardSummary {
    let successes = reports.iter().filter(|r| r.succeeded).count();
    let failures = reports.iter().filter(|r| !r.succeeded).count();
    ShardSummary {
        shard_id,
        agent_count: reports.len(),
        successes,
        failures,
        payload: serde_json::Value::Null,
    }
}

#[derive(Debug, thiserror::Error)]
pub enum FleetError {
    #[error(transparent)]
    Topology(#[from] TopologyError),
    #[error("fleet shard {shard_id} failed: {source}")]
    Shard {
        shard_id: usize,
        #[source]
        source: MeshError,
    },
    #[error("fleet dispatch timed out after {0:?}")]
    Timeout(Duration),
}

/// Hierarchical fleet dispatcher.
pub struct FleetDispatcher {
    board: Arc<Blackboard>,
    run_id: String,
    shard_size: usize,
    /// Per-shard timeout (passed to the inner MeshDispatcher).
    shard_timeout: Duration,
}

impl FleetDispatcher {
    pub fn new(run_id: impl Into<String>) -> Self {
        Self {
            board: Arc::new(Blackboard::new()),
            run_id: run_id.into(),
            shard_size: DEFAULT_SHARD_SIZE,
            shard_timeout: Duration::from_secs(300),
        }
    }

    pub fn with_board(run_id: impl Into<String>, board: Arc<Blackboard>) -> Self {
        Self {
            board,
            run_id: run_id.into(),
            shard_size: DEFAULT_SHARD_SIZE,
            shard_timeout: Duration::from_secs(300),
        }
    }

    pub fn with_shard_size(mut self, n: usize) -> Self {
        self.shard_size = n.max(1);
        self
    }

    pub fn with_shard_timeout(mut self, t: Duration) -> Self {
        self.shard_timeout = t;
        self
    }

    pub fn board(&self) -> &Arc<Blackboard> {
        &self.board
    }

    /// Dispatch the full fleet. `shard_reducer` is invoked once per
    /// shard (with its index + the shard's reports); `fleet_reducer`
    /// collapses the per-shard summaries into the final result.
    ///
    /// If `shard_reducer` is `None`, [`default_shard_reducer`] is used.
    pub async fn dispatch<T>(
        &self,
        agents: Vec<MeshAgent>,
        shard_reducer_factory: Option<Box<dyn Fn() -> ShardReducer + Send + Sync>>,
        fleet_reducer: FleetReducer<T>,
    ) -> Result<T, FleetError>
    where
        T: Send + 'static,
    {
        // Cap enforcement against the locked Topology::Fleet config.
        let cfg = Topology::Fleet.default_config();
        cfg.validate_count(agents.len() as u32)?;

        // Partition into shards of `shard_size`. The last shard may be
        // smaller than `shard_size` when the agent count isn't a clean
        // multiple — that's fine, MeshDispatcher handles small N.
        let shard_size = self.shard_size;
        let mut shards: Vec<Vec<MeshAgent>> = Vec::new();
        let mut current: Vec<MeshAgent> = Vec::with_capacity(shard_size);
        for agent in agents {
            current.push(agent);
            if current.len() == shard_size {
                shards.push(std::mem::take(&mut current));
            }
        }
        if !current.is_empty() {
            shards.push(current);
        }

        // Run all shards concurrently. Each gets its own MeshDispatcher
        // sharing the fleet's blackboard but with a per-shard topic
        // prefix derived from the fleet's run_id + shard index.
        let mut set: JoinSet<Result<ShardSummary, FleetError>> = JoinSet::new();
        for (i, shard_agents) in shards.into_iter().enumerate() {
            let mesh_run_id = format!("{}-shard-{}", self.run_id, i);
            let mesh = MeshDispatcher::with_board(mesh_run_id, self.board.clone())
                .with_timeout(self.shard_timeout);
            let shard_reducer: ShardReducer = if let Some(ref f) = shard_reducer_factory {
                f()
            } else {
                Box::new(default_shard_reducer)
            };
            set.spawn(async move {
                // Collect via a passthrough Reducer that stuffs the raw
                // reports into a Vec we then feed to shard_reducer.
                let passthrough: Reducer<Vec<AgentReport>> = Box::new(|reports| reports);
                let reports = mesh
                    .dispatch(shard_agents, passthrough)
                    .await
                    .map_err(|e| FleetError::Shard {
                        shard_id: i,
                        source: e,
                    })?;
                Ok(shard_reducer(i, reports))
            });
        }

        // Collect shard summaries. We propagate the first shard error.
        let mut summaries: Vec<ShardSummary> = Vec::new();
        while let Some(j) = set.join_next().await {
            match j {
                Ok(Ok(s)) => summaries.push(s),
                Ok(Err(e)) => return Err(e),
                Err(_join_err) => {
                    // A panic in a shard task — synthesise a timeout error
                    // since we can't reach the original MeshError. The
                    // shard_id is unknown (JoinError doesn't carry it).
                    return Err(FleetError::Shard {
                        shard_id: usize::MAX,
                        source: MeshError::Timeout(Duration::ZERO),
                    });
                }
            }
        }
        // Stable ordering by shard_id so tests don't flake on JoinSet order.
        summaries.sort_by_key(|s| s.shard_id);

        Ok(fleet_reducer(summaries))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mesh::BlackboardCtx;
    use serde_json::json;

    fn make_agent(_i: usize, succeed: bool) -> MeshAgent {
        Box::new(move |ctx: BlackboardCtx| {
            Box::pin(async move {
                AgentReport {
                    agent_id: ctx.agent_id.clone(),
                    payload: json!({}),
                    succeeded: succeed,
                }
            })
        })
    }

    #[tokio::test]
    async fn happy_path_30_agents_into_3_shards() {
        let fleet = FleetDispatcher::new("fleet-1").with_shard_size(10);
        let agents: Vec<MeshAgent> = (0..30).map(|i| make_agent(i, true)).collect();

        let fleet_reducer: FleetReducer<(usize, usize)> =
            Box::new(|summaries: Vec<ShardSummary>| {
                let shards = summaries.len();
                let total_ok: usize = summaries.iter().map(|s| s.successes).sum();
                (shards, total_ok)
            });

        let (shards, ok) = fleet.dispatch(agents, None, fleet_reducer).await.unwrap();
        assert_eq!(shards, 3);
        assert_eq!(ok, 30);
    }

    #[tokio::test]
    async fn cap_enforced_at_101_agents() {
        let fleet = FleetDispatcher::new("fleet-2").with_shard_size(10);
        let agents: Vec<MeshAgent> = (0..101).map(|i| make_agent(i, true)).collect();
        let fleet_reducer: FleetReducer<()> = Box::new(|_| ());
        let err = fleet
            .dispatch(agents, None, fleet_reducer)
            .await
            .unwrap_err();
        assert!(matches!(
            err,
            FleetError::Topology(TopologyError::ExceedsCap {
                cap: 100,
                requested: 101,
                ..
            })
        ));
    }

    #[tokio::test]
    async fn partial_last_shard_handled() {
        // 25 agents / shard size 10 => shards of [10, 10, 5].
        let fleet = FleetDispatcher::new("fleet-3").with_shard_size(10);
        let agents: Vec<MeshAgent> = (0..25).map(|i| make_agent(i, true)).collect();
        let fleet_reducer: FleetReducer<Vec<usize>> = Box::new(|summaries: Vec<ShardSummary>| {
            summaries.iter().map(|s| s.agent_count).collect()
        });
        let counts = fleet.dispatch(agents, None, fleet_reducer).await.unwrap();
        assert_eq!(counts, vec![10, 10, 5]);
    }

    #[tokio::test]
    async fn mixed_success_failure_aggregated() {
        let fleet = FleetDispatcher::new("fleet-4").with_shard_size(5);
        // 10 agents: alternate succeed/fail. Shards of 5 each see
        // ~2-3 successes apiece, sum to 5 overall.
        let agents: Vec<MeshAgent> = (0..10).map(|i| make_agent(i, i % 2 == 0)).collect();
        let fleet_reducer: FleetReducer<(usize, usize)> =
            Box::new(|summaries: Vec<ShardSummary>| {
                let ok: usize = summaries.iter().map(|s| s.successes).sum();
                let bad: usize = summaries.iter().map(|s| s.failures).sum();
                (ok, bad)
            });
        let (ok, bad) = fleet.dispatch(agents, None, fleet_reducer).await.unwrap();
        assert_eq!(ok, 5);
        assert_eq!(bad, 5);
    }

    #[tokio::test]
    async fn shard_timeout_surfaces_as_shard_error() {
        let fleet = FleetDispatcher::new("fleet-5")
            .with_shard_size(2)
            .with_shard_timeout(Duration::from_millis(50));
        // Two shards; first agent in shard 0 sleeps too long.
        let slow: MeshAgent = Box::new(|ctx: BlackboardCtx| {
            Box::pin(async move {
                tokio::time::sleep(Duration::from_secs(5)).await;
                AgentReport {
                    agent_id: ctx.agent_id,
                    payload: json!({}),
                    succeeded: true,
                }
            })
        });
        let fast: MeshAgent = make_agent(0, true);
        let agents: Vec<MeshAgent> = vec![slow, fast];
        let fleet_reducer: FleetReducer<()> = Box::new(|_| ());
        let err = fleet
            .dispatch(agents, None, fleet_reducer)
            .await
            .unwrap_err();
        assert!(matches!(err, FleetError::Shard { .. }));
    }

    #[tokio::test]
    async fn custom_shard_reducer_runs() {
        let fleet = FleetDispatcher::new("fleet-6").with_shard_size(5);
        let agents: Vec<MeshAgent> = (0..10).map(|i| make_agent(i, true)).collect();

        let factory: Box<dyn Fn() -> ShardReducer + Send + Sync> = Box::new(|| {
            Box::new(|shard_id, reports| ShardSummary {
                shard_id,
                agent_count: reports.len(),
                successes: reports.iter().filter(|r| r.succeeded).count(),
                failures: reports.iter().filter(|r| !r.succeeded).count(),
                payload: json!({ "custom": true, "shard": shard_id }),
            })
        });

        let fleet_reducer: FleetReducer<bool> = Box::new(|summaries: Vec<ShardSummary>| {
            summaries.iter().all(|s| {
                s.payload
                    .get("custom")
                    .and_then(|v| v.as_bool())
                    .unwrap_or(false)
            })
        });

        let all_custom = fleet
            .dispatch(agents, Some(factory), fleet_reducer)
            .await
            .unwrap();
        assert!(all_custom);
    }

    #[tokio::test]
    async fn default_shard_reducer_helper_works() {
        let reports = vec![
            AgentReport {
                agent_id: "a".into(),
                payload: json!({}),
                succeeded: true,
            },
            AgentReport {
                agent_id: "b".into(),
                payload: json!({}),
                succeeded: false,
            },
        ];
        let summary = default_shard_reducer(7, reports);
        assert_eq!(summary.shard_id, 7);
        assert_eq!(summary.agent_count, 2);
        assert_eq!(summary.successes, 1);
        assert_eq!(summary.failures, 1);
    }
}
