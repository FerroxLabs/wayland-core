// Shared types for v2 memory (5-partition × 3-tier cognitive memory).
//
// Lives in a separate module from v1 `types.rs` so both surfaces can
// coexist during the W5 rollout. The v1 module is removed in Group G.

use std::fmt;
use std::str::FromStr;

use serde::{Deserialize, Serialize};
use uuid::Uuid;

// ---------------------------------------------------------------------------
// Partition (P1..P5)
// ---------------------------------------------------------------------------

/// Five-partition cognitive memory taxonomy.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Partition {
    /// P1: live conversation/tool context (session-scoped).
    Working,
    /// P2: timestamped events with summaries (session/project/global).
    Episodic,
    /// P3: distilled facts as subject/predicate/object triples (project/global).
    Semantic,
    /// P4: reusable skill artifacts + Thompson stats (project/global).
    Procedural,
    /// P5: user model (global only; system-only write).
    Core,
}

impl Partition {
    pub const ALL: [Partition; 5] = [
        Partition::Working,
        Partition::Episodic,
        Partition::Semantic,
        Partition::Procedural,
        Partition::Core,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            Partition::Working => "working",
            Partition::Episodic => "episodic",
            Partition::Semantic => "semantic",
            Partition::Procedural => "procedural",
            Partition::Core => "core",
        }
    }

    /// Design-doc default tier for this partition (used by the dispatcher
    /// when the caller doesn't pin one).
    pub fn default_tier(self) -> Tier {
        match self {
            Partition::Working => Tier::Session,
            Partition::Episodic => Tier::Project,
            Partition::Semantic => Tier::Project,
            Partition::Procedural => Tier::Project,
            Partition::Core => Tier::Global,
        }
    }
}

impl fmt::Display for Partition {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl FromStr for Partition {
    type Err = String;
    fn from_str(s: &str) -> std::result::Result<Self, Self::Err> {
        match s {
            "working" => Ok(Partition::Working),
            "episodic" => Ok(Partition::Episodic),
            "semantic" => Ok(Partition::Semantic),
            "procedural" => Ok(Partition::Procedural),
            "core" => Ok(Partition::Core),
            _ => Err(format!("unknown partition: {s}")),
        }
    }
}

// ---------------------------------------------------------------------------
// Tier (Session / Project / Global)
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Tier {
    Session,
    Project,
    Global,
}

impl Tier {
    pub const ALL: [Tier; 3] = [Tier::Session, Tier::Project, Tier::Global];

    pub fn as_str(self) -> &'static str {
        match self {
            Tier::Session => "session",
            Tier::Project => "project",
            Tier::Global => "global",
        }
    }
}

impl fmt::Display for Tier {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl FromStr for Tier {
    type Err = String;
    fn from_str(s: &str) -> std::result::Result<Self, Self::Err> {
        match s {
            "session" => Ok(Tier::Session),
            "project" => Ok(Tier::Project),
            "global" => Ok(Tier::Global),
            _ => Err(format!("unknown tier: {s}")),
        }
    }
}

// ---------------------------------------------------------------------------
// Valid (Partition, Tier) combinations (design §4.3.1)
// ---------------------------------------------------------------------------

pub fn valid_combinations() -> &'static [(Partition, Tier)] {
    &[
        (Partition::Working, Tier::Session),
        (Partition::Episodic, Tier::Session),
        (Partition::Episodic, Tier::Project),
        (Partition::Episodic, Tier::Global),
        (Partition::Semantic, Tier::Project),
        (Partition::Semantic, Tier::Global),
        (Partition::Procedural, Tier::Project),
        (Partition::Procedural, Tier::Global),
        (Partition::Core, Tier::Global),
    ]
}

pub fn is_valid(p: Partition, t: Tier) -> bool {
    valid_combinations().contains(&(p, t))
}

// ---------------------------------------------------------------------------
// Newtype IDs (UUIDv7 — time-ordered for monotonic seq friendliness)
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct EpisodeId(pub Uuid);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct FactId(pub Uuid);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct ProcedureId(pub Uuid);

impl EpisodeId {
    pub fn new() -> Self {
        Self(Uuid::now_v7())
    }
}

impl Default for EpisodeId {
    fn default() -> Self {
        Self::new()
    }
}

impl FactId {
    pub fn new() -> Self {
        Self(Uuid::now_v7())
    }
}

impl Default for FactId {
    fn default() -> Self {
        Self::new()
    }
}

impl ProcedureId {
    pub fn new() -> Self {
        Self(Uuid::now_v7())
    }
}

impl Default for ProcedureId {
    fn default() -> Self {
        Self::new()
    }
}

// ---------------------------------------------------------------------------
// Access tokens
// ---------------------------------------------------------------------------

/// Capability token presented to MemoryAccessGate.
///
/// Deny-by-default: gate validates token against partition+tier ACL before
/// the dispatcher performs any I/O.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AccessToken {
    /// Internal bootstrap / consolidation / user-model writes.
    System,
    /// The main agent loop.
    MainAgent,
    /// A sub-agent spawned via the Task tool; carries an ACL declared in its
    /// agent YAML (see `gate.rs`).
    SubAgent { agent_name: String },
}

impl AccessToken {
    pub fn kind(&self) -> &'static str {
        match self {
            AccessToken::System => "system",
            AccessToken::MainAgent => "main_agent",
            AccessToken::SubAgent { .. } => "sub_agent",
        }
    }

    pub fn agent_name(&self) -> Option<&str> {
        match self {
            AccessToken::SubAgent { agent_name } => Some(agent_name.as_str()),
            _ => None,
        }
    }
}

// ---------------------------------------------------------------------------
// Source / SourceProduct (for cross-product fusion semantics; column ships,
// fusion does not — design §0)
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum Source {
    MainAgent,
    SubAgent(String),
    Consolidate,
    Compact,
    Legacy,
    User,
    System,
}

impl Source {
    pub fn as_str(&self) -> String {
        match self {
            Source::MainAgent => "main-agent".into(),
            Source::SubAgent(n) => format!("sub-agent:{n}"),
            Source::Consolidate => "consolidate".into(),
            Source::Compact => "compact".into(),
            Source::Legacy => "legacy".into(),
            Source::User => "user".into(),
            Source::System => "system".into(),
        }
    }

    /// Parse the canonical wire form.
    pub fn parse(s: &str) -> Source {
        if let Some(name) = s.strip_prefix("sub-agent:") {
            return Source::SubAgent(name.to_string());
        }
        match s {
            "main-agent" => Source::MainAgent,
            "consolidate" => Source::Consolidate,
            "compact" => Source::Compact,
            "legacy" => Source::Legacy,
            "user" => Source::User,
            "system" => Source::System,
            other => Source::SubAgent(other.to_string()), // permissive
        }
    }
}

// ---------------------------------------------------------------------------
// Records
// ---------------------------------------------------------------------------

/// P2 Episodic entry.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Episode {
    pub id: EpisodeId,
    pub tier: Tier,
    pub ts: i64, // unix epoch seconds
    pub episode_type: String,
    pub summary: String,
    pub atomic_facts: Vec<String>,
    pub source: String,         // Source::as_str()
    pub source_product: String, // wcore-agent | wcore-consolidate | wcore-compact | legacy
    pub session_id: Option<String>,
    pub project_root: Option<String>,
    #[serde(default)]
    pub decay_score: f64,
    #[serde(default)]
    pub status: EpisodeStatus,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum EpisodeStatus {
    #[default]
    Active,
    Archived,
}

impl EpisodeStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            EpisodeStatus::Active => "active",
            EpisodeStatus::Archived => "archived",
        }
    }
}

/// P3 Semantic fact (subject/predicate/object triple).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Fact {
    pub id: FactId,
    pub tier: Tier,
    pub ts: i64,
    pub subject: String,
    pub predicate: String,
    pub object: String,
    pub confidence: f64,
    pub source_episode: Option<EpisodeId>,
    pub superseded_by: Option<FactId>,
}

/// P4 Procedural skill artifact.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Procedure {
    pub id: ProcedureId,
    pub tier: Tier,
    pub ts: i64,
    pub name: String,
    pub description: String,
    pub artifact: String, // YAML / markdown skill body
    pub status: ProcedureStatus,
    pub created_by: String, // "user" | "evolution" | "main-agent" | ...
    pub thompson_alpha: f64,
    pub thompson_beta: f64,
    pub use_count: u64,
    pub success_count: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ProcedureStatus {
    Staged,
    Active,
    Archived,
    Pinned,
}

impl ProcedureStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            ProcedureStatus::Staged => "staged",
            ProcedureStatus::Active => "active",
            ProcedureStatus::Archived => "archived",
            ProcedureStatus::Pinned => "pinned",
        }
    }

    /// Allowed transitions (design §4.3.4, W9 rev-2 amendment §5.3 F11):
    /// Staged → Active | Archived
    /// Active → Archived | Pinned
    /// Pinned → Active | Archived
    pub fn can_transition_to(self, next: ProcedureStatus) -> bool {
        use ProcedureStatus::*;
        matches!(
            (self, next),
            (Staged, Active)
                | (Staged, Archived) // W9: curator may archive a staged-draft loser directly
                | (Active, Archived)
                | (Active, Pinned)
                | (Pinned, Active)
                | (Pinned, Archived)
        )
    }
}

impl FromStr for ProcedureStatus {
    type Err = String;
    fn from_str(s: &str) -> std::result::Result<Self, Self::Err> {
        match s {
            "staged" => Ok(ProcedureStatus::Staged),
            "active" => Ok(ProcedureStatus::Active),
            "archived" => Ok(ProcedureStatus::Archived),
            "pinned" => Ok(ProcedureStatus::Pinned),
            _ => Err(format!("unknown procedure status: {s}")),
        }
    }
}

impl fmt::Display for ProcedureStatus {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl fmt::Display for EpisodeStatus {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// P5 Core user-model k/v entry.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct UserModelEntry {
    pub key: String,
    pub value: serde_json::Value,
    pub ts: i64,
}

/// Aggregate read of the user model (all keys).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct UserModel {
    pub entries: Vec<UserModelEntry>,
}

// ---------------------------------------------------------------------------
// Query + Hit
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Query {
    pub text: String,
    pub tier: Tier,
    pub partition: Option<Partition>,
    pub entities: Option<Vec<String>>,
    pub limit_per_modality: usize,
    pub kg_depth: u8,
    pub token_budget: Option<u32>,
}

impl Default for Query {
    fn default() -> Self {
        Self {
            text: String::new(),
            tier: Tier::Project,
            partition: None,
            entities: None,
            limit_per_modality: 20,
            kg_depth: 1,
            token_budget: None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Hit {
    pub partition: Partition,
    pub tier: Tier,
    pub id: String, // uuid string (covers all partitions uniformly)
    pub score: f64,
    pub session_id: Option<String>,
    pub preview: String,
}

// ---------------------------------------------------------------------------
// Reports
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct DreamReport {
    pub episodes_compressed: u64,
    pub facts_consolidated: u64,
    pub procedures_crystallized: u64,
    pub episodes_decayed: u64,
    /// v0.6.4 Task 6.6c — transitive edges materialized by
    /// `kg::inference::infer_once` during the dream cycle. Additive field,
    /// defaults to 0 when the KG is empty or `WAYLAND_KG=off`.
    #[serde(default)]
    pub kg_edges_inferred: u64,
    pub elapsed_ms: u128,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct CompactReport {
    pub tokens_before: u64,
    pub tokens_after: u64,
    pub turns_offloaded: u64,
    pub bookmarks_inserted: u64,
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    #[test]
    fn partition_all_has_five_unique() {
        assert_eq!(Partition::ALL.len(), 5);
        let s: HashSet<_> = Partition::ALL.iter().collect();
        assert_eq!(s.len(), 5);
    }

    #[test]
    fn tier_all_has_three_unique() {
        assert_eq!(Tier::ALL.len(), 3);
        let s: HashSet<_> = Tier::ALL.iter().collect();
        assert_eq!(s.len(), 3);
    }

    #[test]
    fn partition_default_tier_matches_design() {
        assert_eq!(Partition::Working.default_tier(), Tier::Session);
        assert_eq!(Partition::Core.default_tier(), Tier::Global);
        assert_eq!(Partition::Episodic.default_tier(), Tier::Project);
        assert_eq!(Partition::Semantic.default_tier(), Tier::Project);
        assert_eq!(Partition::Procedural.default_tier(), Tier::Project);
    }

    #[test]
    fn valid_combinations_excludes_denied_cells() {
        // P1 only Session
        assert!(is_valid(Partition::Working, Tier::Session));
        assert!(!is_valid(Partition::Working, Tier::Project));
        assert!(!is_valid(Partition::Working, Tier::Global));
        // P3/P4 never Session
        assert!(!is_valid(Partition::Semantic, Tier::Session));
        assert!(!is_valid(Partition::Procedural, Tier::Session));
        // P5 only Global
        assert!(is_valid(Partition::Core, Tier::Global));
        assert!(!is_valid(Partition::Core, Tier::Project));
        assert!(!is_valid(Partition::Core, Tier::Session));
        // P2 all three
        assert!(is_valid(Partition::Episodic, Tier::Session));
        assert!(is_valid(Partition::Episodic, Tier::Project));
        assert!(is_valid(Partition::Episodic, Tier::Global));
    }

    #[test]
    fn valid_combinations_count() {
        // 1 (P1) + 3 (P2) + 2 (P3) + 2 (P4) + 1 (P5) = 9
        assert_eq!(valid_combinations().len(), 9);
    }

    #[test]
    fn access_token_variants_exhaustive() {
        let t = AccessToken::System;
        assert_eq!(t.kind(), "system");
        assert_eq!(t.agent_name(), None);

        let t = AccessToken::MainAgent;
        assert_eq!(t.kind(), "main_agent");

        let t = AccessToken::SubAgent {
            agent_name: "reviewer".into(),
        };
        assert_eq!(t.kind(), "sub_agent");
        assert_eq!(t.agent_name(), Some("reviewer"));
    }

    #[test]
    fn episode_serde_roundtrip() {
        let ep = Episode {
            id: EpisodeId::new(),
            tier: Tier::Project,
            ts: 1727900000,
            episode_type: "tool_call".into(),
            summary: "ran cargo test".into(),
            atomic_facts: vec!["x is y".into()],
            source: "main-agent".into(),
            source_product: "wcore-agent".into(),
            session_id: Some("s1".into()),
            project_root: Some("/p".into()),
            decay_score: 1.0,
            status: EpisodeStatus::Active,
        };
        let j = serde_json::to_string(&ep).unwrap();
        let back: Episode = serde_json::from_str(&j).unwrap();
        assert_eq!(ep, back);
    }

    #[test]
    fn fact_serde_roundtrip() {
        let f = Fact {
            id: FactId::new(),
            tier: Tier::Global,
            ts: 1,
            subject: "rust".into(),
            predicate: "has_feature".into(),
            object: "async".into(),
            confidence: 0.9,
            source_episode: None,
            superseded_by: None,
        };
        let j = serde_json::to_string(&f).unwrap();
        let back: Fact = serde_json::from_str(&j).unwrap();
        assert_eq!(f, back);
    }

    #[test]
    fn procedure_serde_roundtrip() {
        let p = Procedure {
            id: ProcedureId::new(),
            tier: Tier::Project,
            ts: 1,
            name: "deploy".into(),
            description: "deploy via vx".into(),
            artifact: "---\n...\n".into(),
            status: ProcedureStatus::Staged,
            created_by: "evolution".into(),
            thompson_alpha: 1.0,
            thompson_beta: 1.0,
            use_count: 0,
            success_count: 0,
        };
        let j = serde_json::to_string(&p).unwrap();
        let back: Procedure = serde_json::from_str(&j).unwrap();
        assert_eq!(p, back);
    }

    #[test]
    fn user_model_serde_roundtrip() {
        let m = UserModel {
            entries: vec![UserModelEntry {
                key: "style.commits".into(),
                value: serde_json::json!({"format": "imperative"}),
                ts: 1,
            }],
        };
        let j = serde_json::to_string(&m).unwrap();
        let back: UserModel = serde_json::from_str(&j).unwrap();
        assert_eq!(m, back);
    }

    #[test]
    fn source_parse_roundtrip() {
        assert_eq!(Source::parse("main-agent"), Source::MainAgent);
        assert_eq!(
            Source::parse("sub-agent:reviewer"),
            Source::SubAgent("reviewer".into())
        );
        assert_eq!(Source::SubAgent("r".into()).as_str(), "sub-agent:r");
    }

    #[test]
    fn procedure_status_transitions() {
        use ProcedureStatus::*;
        assert!(Staged.can_transition_to(Active));
        assert!(Staged.can_transition_to(Archived)); // W9 rev-2 amendment
        assert!(Active.can_transition_to(Archived));
        assert!(Active.can_transition_to(Pinned));
        assert!(Pinned.can_transition_to(Active));
        assert!(Pinned.can_transition_to(Archived));
        // Forbidden:
        assert!(!Archived.can_transition_to(Staged));
        assert!(!Archived.can_transition_to(Active));
    }

    #[test]
    fn partition_display_roundtrip() {
        for p in Partition::ALL {
            let s = p.to_string();
            let back: Partition = s.parse().unwrap();
            assert_eq!(p, back);
        }
    }

    #[test]
    fn tier_display_roundtrip() {
        for t in Tier::ALL {
            let s = t.to_string();
            let back: Tier = s.parse().unwrap();
            assert_eq!(t, back);
        }
    }

    #[test]
    fn episode_id_unique() {
        let a = EpisodeId::new();
        let b = EpisodeId::new();
        assert_ne!(a, b);
    }
}
