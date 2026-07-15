//! Read-only reducer for the correlated workflow wire contract.
//!
//! This module never creates runtime authority. It validates serialized
//! producer evidence so hosts can replay workflow runs deterministically.

use std::collections::HashMap;
use std::error::Error;
use std::fmt;

use serde_json::{Map, Value};

use crate::events::{WorkflowChildTerminalState, WorkflowNodeState, WorkflowTerminalState};

pub const WORKFLOW_CONTRACT_VERSION: &str = "1.0";
pub const WORKFLOW_CONTRACT_MAJOR: u64 = 1;

/// States the current runner can actually produce.
pub const SUPPORTED_WORKFLOW_NODE_STATES: &[WorkflowNodeState] = &[
    WorkflowNodeState::Queued,
    WorkflowNodeState::Running,
    WorkflowNodeState::Succeeded,
    WorkflowNodeState::Failed,
    WorkflowNodeState::Blocked,
];

/// Run terminal states the current runner can actually produce.
pub const SUPPORTED_WORKFLOW_TERMINAL_STATES: &[WorkflowTerminalState] = &[
    WorkflowTerminalState::Succeeded,
    WorkflowTerminalState::Failed,
];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WorkflowReplayAcceptance {
    Advanced,
    Duplicate,
    IgnoredAfterChildTerminal,
    IgnoredAfterNodeTerminal,
    IgnoredAfterRunTerminal,
    Unrelated,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WorkflowReplayError {
    UnsupportedContractVersion {
        actual: String,
    },
    Malformed {
        field: &'static str,
    },
    ConflictingDuplicate {
        event_id: String,
    },
    DuplicateRun {
        run_id: String,
    },
    UnknownRun {
        run_id: String,
    },
    InvalidStartSequence {
        actual: u64,
    },
    OutOfOrder {
        run_id: String,
        expected: u64,
        actual: u64,
    },
    ChildOutOfOrder {
        child_run_id: String,
        expected: u64,
        actual: u64,
    },
    ChildCorrelationChanged {
        child_run_id: String,
    },
    InvalidChildParent {
        child_run_id: String,
    },
    ConflictingChildTerminal {
        child_run_id: String,
    },
    ChildTerminalTypeMismatch {
        child_run_id: String,
    },
    ChildAfterNodeTerminal {
        run_id: String,
        child_run_id: String,
        node_id: String,
    },
    NodeChildCorrelationChanged {
        run_id: String,
        node_id: String,
    },
    ConflictingNodeTerminal {
        run_id: String,
        node_id: String,
    },
    ConflictingRunTerminal {
        run_id: String,
    },
    NodesStillActive {
        run_id: String,
        node_ids: Vec<String>,
    },
    InconsistentSuccessFlag {
        run_id: String,
    },
    WorkflowIdentityChanged {
        run_id: String,
    },
    NodeCountMismatch {
        run_id: String,
        expected: u64,
        actual: u64,
    },
    SuccessfulRunHasFailedNodes {
        run_id: String,
        node_ids: Vec<String>,
    },
    ChildrenStillActive {
        run_id: String,
        child_run_ids: Vec<String>,
    },
    ChildNotLinked {
        run_id: String,
        child_run_id: String,
        node_id: String,
    },
    SucceededNodeHasFailedChild {
        run_id: String,
        child_run_id: String,
        node_id: String,
    },
}

impl fmt::Display for WorkflowReplayError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UnsupportedContractVersion { actual } => {
                write!(formatter, "unsupported workflow contract version: {actual}")
            }
            Self::Malformed { field } => write!(formatter, "malformed workflow field: {field}"),
            Self::ConflictingDuplicate { event_id } => {
                write!(
                    formatter,
                    "workflow event {event_id} conflicts with accepted evidence"
                )
            }
            Self::DuplicateRun { run_id } => {
                write!(formatter, "workflow run {run_id} already exists")
            }
            Self::UnknownRun { run_id } => {
                write!(formatter, "workflow run {run_id} has not started")
            }
            Self::InvalidStartSequence { actual } => {
                write!(formatter, "workflow start sequence must be 0, got {actual}")
            }
            Self::OutOfOrder {
                run_id,
                expected,
                actual,
            } => write!(
                formatter,
                "workflow run {run_id} expected sequence {expected}, got {actual}"
            ),
            Self::ChildOutOfOrder {
                child_run_id,
                expected,
                actual,
            } => write!(
                formatter,
                "workflow child {child_run_id} expected sequence {expected}, got {actual}"
            ),
            Self::ChildCorrelationChanged { child_run_id } => {
                write!(
                    formatter,
                    "workflow child {child_run_id} changed correlation identity"
                )
            }
            Self::InvalidChildParent { child_run_id } => write!(
                formatter,
                "workflow child {child_run_id} has an invalid parent call identity"
            ),
            Self::ConflictingChildTerminal { child_run_id } => write!(
                formatter,
                "workflow child {child_run_id} emitted conflicting terminals"
            ),
            Self::ChildTerminalTypeMismatch { child_run_id } => write!(
                formatter,
                "workflow child {child_run_id} terminal disagrees with its inner event"
            ),
            Self::ChildAfterNodeTerminal {
                run_id,
                child_run_id,
                node_id,
            } => write!(
                formatter,
                "workflow run {run_id} child {child_run_id} emitted after node {node_id} terminated"
            ),
            Self::NodeChildCorrelationChanged { run_id, node_id } => write!(
                formatter,
                "workflow run {run_id} node {node_id} changed child correlation"
            ),
            Self::ConflictingNodeTerminal { run_id, node_id } => write!(
                formatter,
                "workflow run {run_id} node {node_id} emitted conflicting terminals"
            ),
            Self::ConflictingRunTerminal { run_id } => {
                write!(
                    formatter,
                    "workflow run {run_id} emitted conflicting terminals"
                )
            }
            Self::NodesStillActive { run_id, node_ids } => write!(
                formatter,
                "workflow run {run_id} finished with active nodes: {}",
                node_ids.join(", ")
            ),
            Self::InconsistentSuccessFlag { run_id } => write!(
                formatter,
                "workflow run {run_id} success flag disagrees with terminal state"
            ),
            Self::WorkflowIdentityChanged { run_id } => {
                write!(formatter, "workflow run {run_id} changed workflow identity")
            }
            Self::NodeCountMismatch {
                run_id,
                expected,
                actual,
            } => write!(
                formatter,
                "workflow run {run_id} expected {expected} nodes, got {actual}"
            ),
            Self::SuccessfulRunHasFailedNodes { run_id, node_ids } => write!(
                formatter,
                "workflow run {run_id} succeeded with failed nodes: {}",
                node_ids.join(", ")
            ),
            Self::ChildrenStillActive {
                run_id,
                child_run_ids,
            } => write!(
                formatter,
                "workflow run {run_id} finished with active children: {}",
                child_run_ids.join(", ")
            ),
            Self::ChildNotLinked {
                run_id,
                child_run_id,
                node_id,
            } => write!(
                formatter,
                "workflow run {run_id} child {child_run_id} is not linked to node {node_id}"
            ),
            Self::SucceededNodeHasFailedChild {
                run_id,
                child_run_id,
                node_id,
            } => write!(
                formatter,
                "workflow run {run_id} node {node_id} succeeded after child {child_run_id} failed"
            ),
        }
    }
}

impl Error for WorkflowReplayError {}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ChildState {
    parent_call_id: String,
    agent_name: String,
    parent_child_run_id: Option<String>,
    next_sequence: u64,
    terminal: Option<WorkflowChildTerminalState>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct NodeState {
    state: WorkflowNodeState,
    child_run_id: Option<String>,
}

#[derive(Debug, Clone)]
struct RunState {
    workflow_id: String,
    expected_node_count: u64,
    next_sequence: u64,
    nodes: HashMap<String, NodeState>,
    children: HashMap<String, ChildState>,
    terminal: Option<WorkflowTerminalState>,
}

/// Multi-run reference reducer for serialized workflow evidence.
#[derive(Debug, Clone)]
pub struct WorkflowReplayReducer {
    accepted_events: HashMap<String, String>,
    runs: HashMap<String, RunState>,
}

impl WorkflowReplayReducer {
    pub fn new(contract_version: &str) -> Result<Self, WorkflowReplayError> {
        validate_workflow_contract_version(contract_version)?;
        Ok(Self {
            accepted_events: HashMap::new(),
            runs: HashMap::new(),
        })
    }

    /// Apply one JSON event. Non-workflow events are returned as `Unrelated`;
    /// malformed or inconsistent workflow evidence fails closed.
    pub fn accept_json(
        &mut self,
        serialized: &str,
    ) -> Result<WorkflowReplayAcceptance, WorkflowReplayError> {
        let value: Value = serde_json::from_str(serialized)
            .map_err(|_| WorkflowReplayError::Malformed { field: "json" })?;
        let object = value
            .as_object()
            .ok_or(WorkflowReplayError::Malformed { field: "top_level" })?;
        let event_type = required_str(object, "type")?;
        if !matches!(
            event_type,
            "workflow_started" | "workflow_node_event" | "sub_agent_event" | "workflow_finished"
        ) {
            return Ok(WorkflowReplayAcceptance::Unrelated);
        }

        let run_id = required_nonempty_str(object, "run_id")?.to_owned();
        let event_id = required_nonempty_str(object, "event_id")?.to_owned();
        let canonical = canonical_json(&value);
        if let Some(previous) = self.accepted_events.get(&event_id) {
            return if previous == &canonical {
                Ok(WorkflowReplayAcceptance::Duplicate)
            } else {
                Err(WorkflowReplayError::ConflictingDuplicate { event_id })
            };
        }

        let result = match event_type {
            "workflow_started" => self.accept_start(object, &run_id),
            "workflow_node_event" => self.accept_node(object, &run_id),
            "sub_agent_event" => self.accept_child(object, &run_id),
            "workflow_finished" => self.accept_finish(object, &run_id),
            _ => unreachable!("workflow event type was filtered above"),
        }?;
        self.accepted_events.insert(event_id, canonical);
        Ok(result)
    }

    fn accept_start(
        &mut self,
        object: &Map<String, Value>,
        run_id: &str,
    ) -> Result<WorkflowReplayAcceptance, WorkflowReplayError> {
        let workflow_id = required_nonempty_str(object, "workflow_id")?.to_owned();
        required_nonempty_str(object, "name")?;
        let expected_node_count = required_u64(object, "node_count")?;
        let sequence = required_u64(object, "sequence")?;
        if sequence != 0 {
            return Err(WorkflowReplayError::InvalidStartSequence { actual: sequence });
        }
        if self.runs.contains_key(run_id) {
            return Err(WorkflowReplayError::DuplicateRun {
                run_id: run_id.to_owned(),
            });
        }
        self.runs.insert(
            run_id.to_owned(),
            RunState {
                workflow_id,
                expected_node_count,
                next_sequence: 1,
                nodes: HashMap::new(),
                children: HashMap::new(),
                terminal: None,
            },
        );
        Ok(WorkflowReplayAcceptance::Advanced)
    }

    fn accept_node(
        &mut self,
        object: &Map<String, Value>,
        run_id: &str,
    ) -> Result<WorkflowReplayAcceptance, WorkflowReplayError> {
        let node_id = required_nonempty_str(object, "node_id")?.to_owned();
        let sequence = required_u64(object, "sequence")?;
        let state: WorkflowNodeState = parse_field(object, "state")?;
        let child_run_id = optional_nonempty_str(object, "child_run_id")?.map(str::to_owned);
        let run = self.run_mut(run_id)?;
        if run.terminal.is_some() {
            return Ok(WorkflowReplayAcceptance::IgnoredAfterRunTerminal);
        }
        require_run_sequence(run_id, run, sequence)?;

        let previous = run.nodes.get(&node_id).cloned();
        if let Some(previous) = previous
            .as_ref()
            .filter(|node| is_node_terminal(node.state))
        {
            if is_node_terminal(state) && state != previous.state {
                return Err(WorkflowReplayError::ConflictingNodeTerminal {
                    run_id: run_id.to_owned(),
                    node_id,
                });
            }
            if previous.child_run_id.is_some()
                && child_run_id.is_some()
                && previous.child_run_id != child_run_id
            {
                return Err(WorkflowReplayError::NodeChildCorrelationChanged {
                    run_id: run_id.to_owned(),
                    node_id,
                });
            }
            run.next_sequence = run.next_sequence.saturating_add(1);
            return Ok(WorkflowReplayAcceptance::IgnoredAfterNodeTerminal);
        }

        if let (Some(previous), Some(child_run_id)) = (&previous, &child_run_id)
            && previous
                .child_run_id
                .as_deref()
                .is_some_and(|id| id != child_run_id)
        {
            return Err(WorkflowReplayError::NodeChildCorrelationChanged {
                run_id: run_id.to_owned(),
                node_id,
            });
        }
        if let Some(child_run_id) = &child_run_id {
            validate_node_child_claim(run_id, run, &node_id, child_run_id)?;
        }
        let child_run_id = child_run_id.or_else(|| previous.and_then(|node| node.child_run_id));
        run.nodes.insert(
            node_id,
            NodeState {
                state,
                child_run_id,
            },
        );
        run.next_sequence = run.next_sequence.saturating_add(1);
        Ok(WorkflowReplayAcceptance::Advanced)
    }

    fn accept_child(
        &mut self,
        object: &Map<String, Value>,
        run_id: &str,
    ) -> Result<WorkflowReplayAcceptance, WorkflowReplayError> {
        let child_run_id = required_nonempty_str(object, "child_run_id")?.to_owned();
        let parent_call_id = required_nonempty_str(object, "parent_call_id")?.to_owned();
        let agent_name = required_nonempty_str(object, "agent_name")?.to_owned();
        let parent_child_run_id =
            optional_nonempty_str(object, "parent_child_run_id")?.map(str::to_owned);
        let child_sequence = required_u64(object, "child_sequence")?;
        let inner = object
            .get("inner")
            .ok_or(WorkflowReplayError::Malformed { field: "inner" })?;
        let terminal: Option<WorkflowChildTerminalState> =
            optional_field(object, "terminal_state")?;
        let node_id = workflow_parent_node_id(&parent_call_id).ok_or_else(|| {
            WorkflowReplayError::InvalidChildParent {
                child_run_id: child_run_id.clone(),
            }
        })?;
        validate_child_terminal_inner(&child_run_id, terminal, inner)?;
        let run = self.run_mut(run_id)?;
        if run.terminal.is_some() {
            return Ok(WorkflowReplayAcceptance::IgnoredAfterRunTerminal);
        }
        validate_child_node_claim(run_id, run, node_id, &child_run_id)?;
        if run
            .nodes
            .get(node_id)
            .is_some_and(|node| is_node_terminal(node.state))
        {
            return Err(WorkflowReplayError::ChildAfterNodeTerminal {
                run_id: run_id.to_owned(),
                child_run_id,
                node_id: node_id.to_owned(),
            });
        }
        if let Some(child) = run.children.get_mut(&child_run_id) {
            if child.parent_call_id != parent_call_id
                || child.agent_name != agent_name
                || child.parent_child_run_id != parent_child_run_id
            {
                return Err(WorkflowReplayError::ChildCorrelationChanged { child_run_id });
            }
            if child_sequence != child.next_sequence {
                return Err(WorkflowReplayError::ChildOutOfOrder {
                    child_run_id,
                    expected: child.next_sequence,
                    actual: child_sequence,
                });
            }
            if child.terminal.is_some() && terminal.is_some() && child.terminal != terminal {
                return Err(WorkflowReplayError::ConflictingChildTerminal { child_run_id });
            }
            let was_terminal = child.terminal.is_some();
            child.next_sequence = child.next_sequence.saturating_add(1);
            if was_terminal {
                return Ok(WorkflowReplayAcceptance::IgnoredAfterChildTerminal);
            }
            child.terminal = terminal;
        } else {
            if child_sequence != 0 {
                return Err(WorkflowReplayError::ChildOutOfOrder {
                    child_run_id,
                    expected: 0,
                    actual: child_sequence,
                });
            }
            run.children.insert(
                child_run_id,
                ChildState {
                    parent_call_id,
                    agent_name,
                    parent_child_run_id,
                    next_sequence: 1,
                    terminal,
                },
            );
        }
        Ok(WorkflowReplayAcceptance::Advanced)
    }

    fn accept_finish(
        &mut self,
        object: &Map<String, Value>,
        run_id: &str,
    ) -> Result<WorkflowReplayAcceptance, WorkflowReplayError> {
        let workflow_id = required_nonempty_str(object, "workflow_id")?;
        let sequence = required_u64(object, "sequence")?;
        let terminal_state: WorkflowTerminalState = parse_field(object, "terminal_state")?;
        let succeeded = object
            .get("succeeded")
            .and_then(Value::as_bool)
            .ok_or(WorkflowReplayError::Malformed { field: "succeeded" })?;
        if succeeded != (terminal_state == WorkflowTerminalState::Succeeded) {
            return Err(WorkflowReplayError::InconsistentSuccessFlag {
                run_id: run_id.to_owned(),
            });
        }
        let run = self.run_mut(run_id)?;
        if workflow_id != run.workflow_id.as_str() {
            return Err(WorkflowReplayError::WorkflowIdentityChanged {
                run_id: run_id.to_owned(),
            });
        }
        if let Some(previous) = run.terminal {
            return if previous == terminal_state {
                Ok(WorkflowReplayAcceptance::IgnoredAfterRunTerminal)
            } else {
                Err(WorkflowReplayError::ConflictingRunTerminal {
                    run_id: run_id.to_owned(),
                })
            };
        }
        require_run_sequence(run_id, run, sequence)?;
        let actual_node_count = run.nodes.len() as u64;
        if actual_node_count != run.expected_node_count {
            return Err(WorkflowReplayError::NodeCountMismatch {
                run_id: run_id.to_owned(),
                expected: run.expected_node_count,
                actual: actual_node_count,
            });
        }
        let mut active: Vec<String> = run
            .nodes
            .iter()
            .filter(|(_, node)| !is_node_terminal(node.state))
            .map(|(node_id, _)| node_id.clone())
            .collect();
        if !active.is_empty() {
            active.sort_unstable();
            return Err(WorkflowReplayError::NodesStillActive {
                run_id: run_id.to_owned(),
                node_ids: active,
            });
        }
        if terminal_state == WorkflowTerminalState::Succeeded {
            let mut failed: Vec<String> = run
                .nodes
                .iter()
                .filter(|(_, node)| node.state == WorkflowNodeState::Failed)
                .map(|(node_id, _)| node_id.clone())
                .collect();
            if !failed.is_empty() {
                failed.sort_unstable();
                return Err(WorkflowReplayError::SuccessfulRunHasFailedNodes {
                    run_id: run_id.to_owned(),
                    node_ids: failed,
                });
            }
        }
        let mut active_children: Vec<String> = run
            .children
            .iter()
            .filter(|(_, child)| child.terminal.is_none())
            .map(|(child_run_id, _)| child_run_id.clone())
            .collect();
        if !active_children.is_empty() {
            active_children.sort_unstable();
            return Err(WorkflowReplayError::ChildrenStillActive {
                run_id: run_id.to_owned(),
                child_run_ids: active_children,
            });
        }
        validate_completed_child_links(run_id, run)?;
        run.next_sequence = run.next_sequence.saturating_add(1);
        run.terminal = Some(terminal_state);
        Ok(WorkflowReplayAcceptance::Advanced)
    }

    fn run_mut(&mut self, run_id: &str) -> Result<&mut RunState, WorkflowReplayError> {
        self.runs
            .get_mut(run_id)
            .ok_or_else(|| WorkflowReplayError::UnknownRun {
                run_id: run_id.to_owned(),
            })
    }
}

pub fn validate_workflow_contract_version(version: &str) -> Result<(), WorkflowReplayError> {
    let major = version
        .split_once('.')
        .map_or(version, |(major, _)| major)
        .parse::<u64>()
        .ok();
    if major == Some(WORKFLOW_CONTRACT_MAJOR) {
        Ok(())
    } else {
        Err(WorkflowReplayError::UnsupportedContractVersion {
            actual: version.to_owned(),
        })
    }
}

fn required_str<'a>(
    object: &'a Map<String, Value>,
    field: &'static str,
) -> Result<&'a str, WorkflowReplayError> {
    object
        .get(field)
        .and_then(Value::as_str)
        .ok_or(WorkflowReplayError::Malformed { field })
}

fn required_nonempty_str<'a>(
    object: &'a Map<String, Value>,
    field: &'static str,
) -> Result<&'a str, WorkflowReplayError> {
    let value = required_str(object, field)?;
    if value.is_empty() {
        Err(WorkflowReplayError::Malformed { field })
    } else {
        Ok(value)
    }
}

fn required_u64(
    object: &Map<String, Value>,
    field: &'static str,
) -> Result<u64, WorkflowReplayError> {
    object
        .get(field)
        .and_then(Value::as_u64)
        .ok_or(WorkflowReplayError::Malformed { field })
}

fn parse_field<T>(
    object: &Map<String, Value>,
    field: &'static str,
) -> Result<T, WorkflowReplayError>
where
    T: serde::de::DeserializeOwned,
{
    serde_json::from_value(
        object
            .get(field)
            .cloned()
            .ok_or(WorkflowReplayError::Malformed { field })?,
    )
    .map_err(|_| WorkflowReplayError::Malformed { field })
}

fn optional_field<T>(
    object: &Map<String, Value>,
    field: &'static str,
) -> Result<Option<T>, WorkflowReplayError>
where
    T: serde::de::DeserializeOwned,
{
    match object.get(field) {
        None | Some(Value::Null) => Ok(None),
        Some(value) => serde_json::from_value(value.clone())
            .map(Some)
            .map_err(|_| WorkflowReplayError::Malformed { field }),
    }
}

fn optional_nonempty_str<'a>(
    object: &'a Map<String, Value>,
    field: &'static str,
) -> Result<Option<&'a str>, WorkflowReplayError> {
    match object.get(field) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(value)) if !value.is_empty() => Ok(Some(value)),
        _ => Err(WorkflowReplayError::Malformed { field }),
    }
}

fn workflow_parent_node_id(parent_call_id: &str) -> Option<&str> {
    parent_call_id
        .strip_prefix("workflow:")
        .filter(|node_id| !node_id.is_empty())
}

fn validate_child_terminal_inner(
    child_run_id: &str,
    terminal: Option<WorkflowChildTerminalState>,
    inner: &Value,
) -> Result<(), WorkflowReplayError> {
    let Some(terminal) = terminal else {
        return Ok(());
    };
    let actual = inner
        .as_object()
        .and_then(|object| object.get("type"))
        .and_then(Value::as_str);
    let expected = match terminal {
        WorkflowChildTerminalState::Succeeded => "info",
        WorkflowChildTerminalState::Failed => "error",
    };
    if actual == Some(expected) {
        Ok(())
    } else {
        Err(WorkflowReplayError::ChildTerminalTypeMismatch {
            child_run_id: child_run_id.to_owned(),
        })
    }
}

fn validate_node_child_claim(
    run_id: &str,
    run: &RunState,
    node_id: &str,
    child_run_id: &str,
) -> Result<(), WorkflowReplayError> {
    if run.nodes.iter().any(|(other_node_id, node)| {
        other_node_id != node_id && node.child_run_id.as_deref() == Some(child_run_id)
    }) {
        return Err(WorkflowReplayError::NodeChildCorrelationChanged {
            run_id: run_id.to_owned(),
            node_id: node_id.to_owned(),
        });
    }
    if let Some(child) = run.children.get(child_run_id) {
        let expected_parent = format!("workflow:{node_id}");
        if child.parent_call_id != expected_parent {
            return Err(WorkflowReplayError::NodeChildCorrelationChanged {
                run_id: run_id.to_owned(),
                node_id: node_id.to_owned(),
            });
        }
    }
    Ok(())
}

fn validate_child_node_claim(
    run_id: &str,
    run: &RunState,
    node_id: &str,
    child_run_id: &str,
) -> Result<(), WorkflowReplayError> {
    if let Some(node) = run.nodes.get(node_id)
        && node
            .child_run_id
            .as_deref()
            .is_some_and(|known| known != child_run_id)
    {
        return Err(WorkflowReplayError::NodeChildCorrelationChanged {
            run_id: run_id.to_owned(),
            node_id: node_id.to_owned(),
        });
    }
    if let Some((other_node_id, _)) = run.nodes.iter().find(|(other_node_id, node)| {
        other_node_id.as_str() != node_id && node.child_run_id.as_deref() == Some(child_run_id)
    }) {
        return Err(WorkflowReplayError::NodeChildCorrelationChanged {
            run_id: run_id.to_owned(),
            node_id: other_node_id.clone(),
        });
    }
    Ok(())
}

fn validate_completed_child_links(run_id: &str, run: &RunState) -> Result<(), WorkflowReplayError> {
    for (node_id, node) in &run.nodes {
        if let Some(child_run_id) = &node.child_run_id {
            let Some(child) = run.children.get(child_run_id) else {
                return Err(WorkflowReplayError::ChildNotLinked {
                    run_id: run_id.to_owned(),
                    child_run_id: child_run_id.clone(),
                    node_id: node_id.clone(),
                });
            };
            if workflow_parent_node_id(&child.parent_call_id) != Some(node_id) {
                return Err(WorkflowReplayError::ChildNotLinked {
                    run_id: run_id.to_owned(),
                    child_run_id: child_run_id.clone(),
                    node_id: node_id.clone(),
                });
            }
        }
    }

    for (child_run_id, child) in &run.children {
        let node_id = workflow_parent_node_id(&child.parent_call_id).ok_or_else(|| {
            WorkflowReplayError::InvalidChildParent {
                child_run_id: child_run_id.clone(),
            }
        })?;
        let Some(node) = run.nodes.get(node_id) else {
            return Err(WorkflowReplayError::ChildNotLinked {
                run_id: run_id.to_owned(),
                child_run_id: child_run_id.clone(),
                node_id: node_id.to_owned(),
            });
        };
        if node.child_run_id.as_deref() != Some(child_run_id) {
            return Err(WorkflowReplayError::ChildNotLinked {
                run_id: run_id.to_owned(),
                child_run_id: child_run_id.clone(),
                node_id: node_id.to_owned(),
            });
        }
        if node.state == WorkflowNodeState::Succeeded
            && child.terminal == Some(WorkflowChildTerminalState::Failed)
        {
            return Err(WorkflowReplayError::SucceededNodeHasFailedChild {
                run_id: run_id.to_owned(),
                child_run_id: child_run_id.clone(),
                node_id: node_id.to_owned(),
            });
        }
    }
    Ok(())
}

fn require_run_sequence(
    run_id: &str,
    run: &RunState,
    actual: u64,
) -> Result<(), WorkflowReplayError> {
    if actual == run.next_sequence {
        Ok(())
    } else {
        Err(WorkflowReplayError::OutOfOrder {
            run_id: run_id.to_owned(),
            expected: run.next_sequence,
            actual,
        })
    }
}

fn is_node_terminal(state: WorkflowNodeState) -> bool {
    matches!(
        state,
        WorkflowNodeState::Succeeded | WorkflowNodeState::Failed | WorkflowNodeState::Blocked
    )
}

fn canonical_json(value: &Value) -> String {
    match value {
        Value::Object(object) => {
            let mut keys: Vec<&String> = object.keys().collect();
            keys.sort_unstable();
            let body = keys
                .into_iter()
                .map(|key| {
                    format!(
                        "{}:{}",
                        serde_json::to_string(key).expect("JSON object keys serialize"),
                        canonical_json(&object[key])
                    )
                })
                .collect::<Vec<_>>()
                .join(",");
            format!("{{{body}}}")
        }
        Value::Array(array) => format!(
            "[{}]",
            array
                .iter()
                .map(canonical_json)
                .collect::<Vec<_>>()
                .join(",")
        ),
        _ => serde_json::to_string(value).expect("parsed JSON values serialize"),
    }
}
