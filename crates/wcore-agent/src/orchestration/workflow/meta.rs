//! Workflow metadata — the `meta` block of a RON workflow.

use serde::Deserialize;

/// Author-declared metadata for a workflow. Carried alongside the
/// lowered [`super::super::graph::GraphConfig`] so callers can present a
/// name / description and a self-declared agent-count hint.
///
/// `est_agents` is a *hint only*; the authoritative pre-execution count
/// comes from the IR-walking cost estimator (task B5). Defaults to `0`
/// when omitted.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct WorkflowMeta {
    pub name: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub est_agents: usize,
}
