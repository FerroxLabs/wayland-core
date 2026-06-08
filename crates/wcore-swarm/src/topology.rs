//! Multi-agent topology configurations.
//!
//! 4.B.2: defines the four canonical tiers and their parent-visibility +
//! blackboard-scope rules. Pure data — no allocators, no async, no I/O.
//!
//! Used by:
//! * 4.B.3 (`mesh.rs`) and 4.B.4 (`fleet.rs`) to enforce per-tier caps
//!   and decide what the parent sees.
//! * 4.B.5 (`spawn_tool.rs`) to lift the legacy hardcoded MAX_SUB_AGENTS=5
//!   in favour of `Topology::default_config().max_agents`.
//! * the CLI `--topology=<name>` flag (parsed via [`Topology::from_str`]).
//!
//! Canonical tiers (locked for v0.7.0):
//!
//! | Topology | Max agents | Parent visibility           | Blackboard scope    |
//! |----------|-----------:|-----------------------------|---------------------|
//! | Spawn    |          5 | FullTranscript              | None                |
//! | Swarm    |         20 | StatusAndArtifacts          | None                |
//! | Mesh     |         50 | BlackboardTopics            | SharedTopicTree     |
//! | Fleet    |        100 | HierarchicalSummary         | ShardedByTier       |

use std::fmt;
use std::str::FromStr;

use serde::{Deserialize, Serialize};
use thiserror::Error;

/// The four canonical multi-agent topologies.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Topology {
    /// Up to 5 sub-agents with full transcript visibility. The default —
    /// matches the legacy SpawnTool behaviour.
    Spawn,
    /// Up to 20 worktree-isolated workers. Parent sees status + final
    /// artifacts only.
    Swarm,
    /// Up to 50 agents sharing a blackboard. Parent sees blackboard
    /// topics (not individual transcripts).
    Mesh,
    /// 100+ agents in a hierarchical reduction. Parent sees the final
    /// summarized result; intermediate tiers reduce locally.
    Fleet,
}

/// How much each child's work the parent observes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ParentVisibility {
    /// Parent sees every message the child produced.
    FullTranscript,
    /// Parent sees only the final status code + artifact list.
    StatusAndArtifacts,
    /// Parent sees blackboard topic summaries (not children directly).
    BlackboardTopics,
    /// Parent sees the result of the hierarchical reduction.
    HierarchicalSummary,
}

/// How the blackboard partition is laid out for this topology.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BlackboardScope {
    /// No blackboard — children cannot share state out-of-band.
    None,
    /// Single shared topic tree (all children publish/subscribe in the
    /// same namespace). Used by Mesh.
    SharedTopicTree,
    /// Sharded by hierarchical tier (each reducer sees its tier's slice).
    /// Used by Fleet.
    ShardedByTier,
}

/// Per-topology knob bundle.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct TopologyConfig {
    pub topology: Topology,
    pub max_agents: u32,
    pub parent_visibility: ParentVisibility,
    pub blackboard_scope: BlackboardScope,
}

/// Errors surfaced by topology validation + parsing.
#[derive(Debug, Error, PartialEq, Eq)]
pub enum TopologyError {
    #[error("{topology:?} topology supports at most {cap} agents; requested {requested}")]
    ExceedsCap {
        topology: Topology,
        requested: u32,
        cap: u32,
    },
    #[error("unknown topology {0:?} (expected one of: spawn, swarm, mesh, fleet)")]
    UnknownTopology(String),
}

impl Topology {
    /// Canonical config for this tier. Use this everywhere a default is
    /// needed; callers can `clone()` and tweak fields if they need to
    /// (e.g. lowering `max_agents` for a constrained host).
    pub fn default_config(self) -> TopologyConfig {
        match self {
            Self::Spawn => TopologyConfig {
                topology: self,
                max_agents: 5,
                parent_visibility: ParentVisibility::FullTranscript,
                blackboard_scope: BlackboardScope::None,
            },
            Self::Swarm => TopologyConfig {
                topology: self,
                max_agents: 20,
                parent_visibility: ParentVisibility::StatusAndArtifacts,
                blackboard_scope: BlackboardScope::None,
            },
            Self::Mesh => TopologyConfig {
                topology: self,
                max_agents: 50,
                parent_visibility: ParentVisibility::BlackboardTopics,
                blackboard_scope: BlackboardScope::SharedTopicTree,
            },
            Self::Fleet => TopologyConfig {
                topology: self,
                max_agents: 100,
                parent_visibility: ParentVisibility::HierarchicalSummary,
                blackboard_scope: BlackboardScope::ShardedByTier,
            },
        }
    }

    /// Lowercase canonical name (matches the FromStr accepted forms).
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Spawn => "spawn",
            Self::Swarm => "swarm",
            Self::Mesh => "mesh",
            Self::Fleet => "fleet",
        }
    }
}

impl fmt::Display for Topology {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl FromStr for Topology {
    type Err = TopologyError;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.trim().to_ascii_lowercase().as_str() {
            "spawn" => Ok(Self::Spawn),
            "swarm" => Ok(Self::Swarm),
            "mesh" => Ok(Self::Mesh),
            "fleet" => Ok(Self::Fleet),
            _ => Err(TopologyError::UnknownTopology(s.to_string())),
        }
    }
}

impl TopologyConfig {
    /// Enforce the per-tier agent cap. `count` is the *requested* number
    /// of children to spawn under this topology.
    pub fn validate_count(&self, count: u32) -> Result<(), TopologyError> {
        if count > self.max_agents {
            Err(TopologyError::ExceedsCap {
                topology: self.topology,
                requested: count,
                cap: self.max_agents,
            })
        } else {
            Ok(())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_configs_match_locked_table() {
        let spawn = Topology::Spawn.default_config();
        assert_eq!(spawn.max_agents, 5);
        assert!(matches!(
            spawn.parent_visibility,
            ParentVisibility::FullTranscript
        ));
        assert!(matches!(spawn.blackboard_scope, BlackboardScope::None));

        let swarm = Topology::Swarm.default_config();
        assert_eq!(swarm.max_agents, 20);
        assert!(matches!(
            swarm.parent_visibility,
            ParentVisibility::StatusAndArtifacts
        ));

        let mesh = Topology::Mesh.default_config();
        assert_eq!(mesh.max_agents, 50);
        assert!(matches!(
            mesh.parent_visibility,
            ParentVisibility::BlackboardTopics
        ));
        assert!(matches!(
            mesh.blackboard_scope,
            BlackboardScope::SharedTopicTree
        ));

        let fleet = Topology::Fleet.default_config();
        assert_eq!(fleet.max_agents, 100);
        assert!(matches!(
            fleet.parent_visibility,
            ParentVisibility::HierarchicalSummary
        ));
        assert!(matches!(
            fleet.blackboard_scope,
            BlackboardScope::ShardedByTier
        ));
    }

    #[test]
    fn validate_count_under_cap_is_ok() {
        for t in [
            Topology::Spawn,
            Topology::Swarm,
            Topology::Mesh,
            Topology::Fleet,
        ] {
            let cfg = t.default_config();
            assert!(cfg.validate_count(cfg.max_agents).is_ok());
            assert!(cfg.validate_count(0).is_ok());
        }
    }

    #[test]
    fn validate_count_over_cap_errors() {
        let cfg = Topology::Spawn.default_config();
        let err = cfg.validate_count(6).unwrap_err();
        assert_eq!(
            err,
            TopologyError::ExceedsCap {
                topology: Topology::Spawn,
                requested: 6,
                cap: 5,
            }
        );
    }

    #[test]
    fn from_str_parses_all_four_case_insensitive() {
        for (s, expected) in [
            ("spawn", Topology::Spawn),
            ("Spawn", Topology::Spawn),
            ("SPAWN", Topology::Spawn),
            ("swarm", Topology::Swarm),
            ("mesh", Topology::Mesh),
            ("fleet", Topology::Fleet),
            ("  spawn  ", Topology::Spawn),
        ] {
            assert_eq!(s.parse::<Topology>().unwrap(), expected);
        }
    }

    #[test]
    fn from_str_rejects_unknown() {
        assert_eq!(
            "raft".parse::<Topology>().unwrap_err(),
            TopologyError::UnknownTopology("raft".to_string())
        );
    }

    #[test]
    fn display_round_trips_through_from_str() {
        for t in [
            Topology::Spawn,
            Topology::Swarm,
            Topology::Mesh,
            Topology::Fleet,
        ] {
            assert_eq!(t.to_string().parse::<Topology>().unwrap(), t);
            assert_eq!(t.as_str(), t.to_string());
        }
    }

    #[test]
    fn topology_config_is_copy_and_serde_round_trip() {
        let cfg = Topology::Mesh.default_config();
        let cfg2 = cfg; // Copy
        assert_eq!(cfg, cfg2);
        let json = serde_json::to_string(&cfg).unwrap();
        let back: TopologyConfig = serde_json::from_str(&json).unwrap();
        assert_eq!(cfg, back);
    }
}
