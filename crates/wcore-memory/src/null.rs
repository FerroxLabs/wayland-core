// W7 Pre-flight 0: `NullMemory` — no-op `MemoryApi` implementation for
// engine constructors that don't have memory configured (test fixtures,
// CLIs running with memory disabled). All mutators succeed silently and
// return synthesized identifiers; all readers return empty collections.
//
// This lets `AgentEngine` carry a non-Optional `Arc<dyn MemoryApi>` field
// without forcing every consumer to wire a real `Memory` backend. The W9
// skills-lifecycle Curator/PUM hooks can land on top of this in a later
// wave once they have a real handle to use.

use async_trait::async_trait;
use serde_json::Value;

use crate::api::MemoryApi;
use crate::error::{MemoryError, Result};
use crate::v2_types::{
    AccessToken, CompactReport, DreamReport, Episode, EpisodeId, Fact, FactId, Hit, Procedure,
    ProcedureId, Query, Tier, UserModel,
};

/// No-op `MemoryApi` implementation. Suitable for fixtures and for
/// `AgentBootstrap` paths where the user has not opted into memory.
#[derive(Debug, Default, Clone, Copy)]
pub struct NullMemory;

#[async_trait]
impl MemoryApi for NullMemory {
    async fn record_episode(&self, _ep: Episode, _tok: AccessToken) -> Result<EpisodeId> {
        Ok(EpisodeId::default())
    }
    async fn assert_fact(&self, _f: Fact, _tok: AccessToken) -> Result<FactId> {
        Ok(FactId::default())
    }
    async fn upsert_procedure(&self, _p: Procedure, _tok: AccessToken) -> Result<ProcedureId> {
        Ok(ProcedureId::default())
    }
    async fn list_procedures(&self, _tier: Tier, _tok: AccessToken) -> Result<Vec<Procedure>> {
        Ok(Vec::new())
    }
    async fn update_user_model(&self, _key: &str, _val: Value, _tok: AccessToken) -> Result<()> {
        Ok(())
    }
    async fn search(&self, _q: Query, _tok: AccessToken) -> Result<Vec<Hit>> {
        Ok(Vec::new())
    }
    async fn get_episode(&self, _id: &EpisodeId, _tok: AccessToken) -> Result<Episode> {
        // No store; surface a structured error using the existing
        // `AccessDenied` variant rather than inventing a new one, so this
        // file remains additive-only.
        Err(MemoryError::AccessDenied {
            partition: "null".into(),
            tier: "null".into(),
            reason: "NullMemory has no episodes".into(),
        })
    }
    async fn user_model(&self, _tok: AccessToken) -> Result<UserModel> {
        Ok(UserModel::default())
    }
    async fn dream_now(&self) -> Result<DreamReport> {
        Ok(DreamReport::default())
    }
    async fn compact(&self, _target_tokens: u64) -> Result<CompactReport> {
        Ok(CompactReport::default())
    }
    async fn record_skill_use(&self, _: &str, _: bool, _: u64) -> Result<()> {
        Ok(())
    }
    async fn top_procedures(
        &self,
        _: Tier,
        _: usize,
        _: u64,
        _: AccessToken,
    ) -> Result<Vec<Procedure>> {
        Ok(Vec::new())
    }
    async fn kg_ingest_facts(&self, _transcript: &str) -> Result<usize> {
        // No KG without a backing store; nothing to ingest.
        Ok(0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    #[tokio::test]
    async fn null_memory_implements_memory_api() {
        let mem: Arc<dyn MemoryApi> = Arc::new(NullMemory);
        // Mutator: silently succeeds.
        assert!(
            mem.update_user_model("k", Value::String("v".into()), AccessToken::System)
                .await
                .is_ok()
        );
        // Reader: empty results.
        let hits = mem
            .search(Query::default(), AccessToken::MainAgent)
            .await
            .unwrap();
        assert!(hits.is_empty());
        // User model: default (empty entries vec).
        let user = mem.user_model(AccessToken::System).await.unwrap();
        assert!(user.entries.is_empty());
    }
}
