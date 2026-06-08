//! Typed errors for RON workflow parsing + lowering.

use thiserror::Error;

/// Errors produced while parsing a RON workflow and lowering it onto a
/// [`super::super::graph::GraphConfig`].
///
/// Every variant carries a field/location pointer so a CLI `validate`
/// surface (task B2) can point the author at the offending construct.
#[derive(Debug, Error)]
pub enum WorkflowParseError {
    /// The RON text was syntactically invalid (serde/ron rejected it).
    #[error("invalid RON syntax: {0}")]
    Ron(String),

    /// A phase declared no steps. An empty phase lowers to nothing and
    /// is almost always an authoring mistake.
    #[error("phase `{phase}` is empty: a phase must contain at least one step")]
    EmptyPhase { phase: String },

    /// The workflow declared no phases at all.
    #[error("workflow `{name}` has no phases")]
    EmptyWorkflow { name: String },

    /// An `Agent`/`Pipeline`/`Parallel` step referenced an agent with an
    /// empty name, which cannot become a graph node id.
    #[error("step in phase `{phase}` has an empty agent name")]
    EmptyAgentName { phase: String },

    /// A `Parallel` step declared fewer than two branches; a parallel
    /// fan-out with one branch is degenerate.
    #[error("parallel step in phase `{phase}` needs >= 2 branches, found {found}")]
    DegenerateParallel { phase: String, found: usize },

    /// A `Pipeline` step declared no stages.
    #[error("pipeline step in phase `{phase}` has no stages")]
    EmptyPipeline { phase: String },

    /// A step referenced a named schema that the workflow's `schemas`
    /// table does not define.
    #[error("step `{step}` references unknown schema `{schema}`")]
    MissingSchema { step: String, schema: String },

    /// A cross-stage data ref (`$ref`) pointed at a stage/key that does
    /// not exist earlier in the workflow.
    #[error("step `{step}` has a dangling reference `{reference}`")]
    DanglingRef { step: String, reference: String },

    /// Two steps lowered to the same graph node id. Node ids must be
    /// unique across the whole workflow.
    #[error("duplicate node id `{id}` (every agent/step id must be unique)")]
    DuplicateNodeId { id: String },

    /// A named schema body in the `schemas` table failed to compile as the
    /// JSON-Schema subset (task A4 [`super::schema::WorkflowSchema::parse`]).
    #[error("schema `{name}` is invalid: {message}")]
    InvalidSchema { name: String, message: String },

    /// The RON document exceeded [`super::limits::MAX_RON_BYTES`]. Rejected
    /// before `ron::from_str` so an oversized payload never reaches the parser.
    #[error("workflow RON is too large: {size} bytes exceeds the {limit}-byte limit")]
    TooLarge { size: usize, limit: usize },

    /// The RON document's bracket/paren/brace nesting exceeded
    /// [`super::limits::MAX_NESTING_DEPTH`]. Rejected before `ron::from_str` so
    /// a deeply-nested payload cannot overflow the stack during parse (an
    /// uncatchable abort).
    #[error("workflow RON is nested too deeply: depth {depth} exceeds the {limit} limit")]
    TooDeep { depth: usize, limit: usize },

    /// The workflow lowered to more than [`super::limits::MAX_WORKFLOW_NODES`]
    /// graph nodes.
    #[error("workflow has too many nodes: {count} exceeds the {limit}-node limit")]
    TooManyNodes { count: usize, limit: usize },

    /// A user-authored id used a reserved synthetic prefix (`__`). The lowering
    /// mints synthetic node ids (e.g. `__fan_root__<id>` for a `Parallel` root),
    /// so an author id colliding with one would otherwise surface a confusing
    /// `DuplicateNodeId` for an id the author never wrote. Reserving the prefix
    /// turns that into a clear, actionable error at the offending id.
    #[error("id `{id}` uses the reserved `{prefix}` prefix (synthetic ids only)")]
    ReservedId { id: String, prefix: String },
}
