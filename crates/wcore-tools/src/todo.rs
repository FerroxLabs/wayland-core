//! Todo Tool — Planning & Task Management
//!
//! Ported from an upstream MIT-licensed library (see THIRD-PARTY-NOTICES.md) into the wcore-tools dispatch surface.
//!
//! Provides an in-memory task list the agent uses to decompose complex tasks,
//! track progress, and maintain focus across long conversations.
//!
//! Design notes
//! ------------
//! * Single `todo` tool. Provide `todos` to write; omit to read.
//! * Every call returns the full current list as a JSON string with summary
//!   counts (mirrors the Python contract — see `todo_tool.py:156` upstream).
//! * State management diverges from the Python original: the Python tool
//!   stores the list on the per-session `AIAgent` instance and re-injects it
//!   after compression. The Rust port holds an `Arc<Mutex<TodoStore>>`
//!   shared by all clones of the `TodoTool` so concurrent dispatcher calls
//!   observe the same list. Post-compression re-injection is out of scope
//!   for this port — `TodoStore::format_for_injection` is exposed for a
//!   future engine integration.

use std::sync::Arc;

use async_trait::async_trait;
use serde_json::{Value, json};
use tokio::sync::Mutex;

use wcore_protocol::events::ToolCategory;
use wcore_types::tool::{JsonSchema, ToolResult};

use crate::Tool;

/// Valid status values for todo items. Mirrors `VALID_STATUSES` in the Python
/// upstream (`todo_tool.py:22`).
const VALID_STATUSES: &[&str] = &["pending", "in_progress", "completed", "cancelled"];

/// A single todo item. Order in `TodoStore::items` is priority.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TodoItem {
    pub id: String,
    pub content: String,
    pub status: String,
}

impl TodoItem {
    fn to_json(&self) -> Value {
        json!({
            "id": self.id,
            "content": self.content,
            "status": self.status,
        })
    }
}

/// In-memory todo list. One instance per `TodoTool`.
#[derive(Default, Debug)]
pub struct TodoStore {
    items: Vec<TodoItem>,
}

impl TodoStore {
    pub fn new() -> Self {
        Self { items: Vec::new() }
    }

    /// Replace the list entirely (or merge by id if `merge` is true) and
    /// return a clone of the current items.
    pub fn write(&mut self, todos: Vec<Value>, merge: bool) -> Vec<TodoItem> {
        let deduped = Self::dedupe_by_id(todos);
        if !merge {
            // Replace mode: new list entirely.
            self.items = deduped.into_iter().map(Self::validate).collect();
            return self.read();
        }

        // Merge mode: update existing items by id, append new ones.
        for t in deduped {
            let item_id = t
                .get("id")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .trim()
                .to_string();
            if item_id.is_empty() {
                // Can't merge without an id.
                continue;
            }

            if let Some(existing) = self.items.iter_mut().find(|i| i.id == item_id) {
                // Update only the fields the LLM actually provided.
                if let Some(content) = t.get("content").and_then(|v| v.as_str()) {
                    let content = content.trim();
                    if !content.is_empty() {
                        existing.content = content.to_string();
                    }
                }
                if let Some(status) = t.get("status").and_then(|v| v.as_str()) {
                    let status = status.trim().to_ascii_lowercase();
                    if VALID_STATUSES.contains(&status.as_str()) {
                        existing.status = status;
                    }
                }
            } else {
                // New item — validate fully and append.
                self.items.push(Self::validate(t));
            }
        }
        self.read()
    }

    /// Return a clone of the current list.
    pub fn read(&self) -> Vec<TodoItem> {
        self.items.clone()
    }

    /// True if any items are present.
    pub fn has_items(&self) -> bool {
        !self.items.is_empty()
    }

    /// Render the active (pending/in_progress) tasks for post-compression
    /// injection. Mirrors `TodoStore.format_for_injection` upstream
    /// (`todo_tool.py:90`). Returns `None` if there are no active items.
    pub fn format_for_injection(&self) -> Option<String> {
        let active: Vec<&TodoItem> = self
            .items
            .iter()
            .filter(|i| i.status == "pending" || i.status == "in_progress")
            .collect();
        if active.is_empty() {
            return None;
        }

        let mut lines =
            vec!["[Your active task list was preserved across context compression]".to_string()];
        for item in active {
            let marker = match item.status.as_str() {
                "completed" => "[x]",
                "in_progress" => "[>]",
                "pending" => "[ ]",
                "cancelled" => "[~]",
                _ => "[?]",
            };
            lines.push(format!(
                "- {} {}. {} ({})",
                marker, item.id, item.content, item.status
            ));
        }
        Some(lines.join("\n"))
    }

    /// Validate and normalize a todo item.
    fn validate(item: Value) -> TodoItem {
        let item_id = item
            .get("id")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .trim()
            .to_string();
        let item_id = if item_id.is_empty() {
            "?".to_string()
        } else {
            item_id
        };

        let content = item
            .get("content")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .trim()
            .to_string();
        let content = if content.is_empty() {
            "(no description)".to_string()
        } else {
            content
        };

        let status = item
            .get("status")
            .and_then(|v| v.as_str())
            .unwrap_or("pending")
            .trim()
            .to_ascii_lowercase();
        let status = if VALID_STATUSES.contains(&status.as_str()) {
            status
        } else {
            "pending".to_string()
        };

        TodoItem {
            id: item_id,
            content,
            status,
        }
    }

    /// Collapse duplicate ids, keeping the last occurrence in its original
    /// position. Mirrors `_dedupe_by_id` upstream (`todo_tool.py:147`).
    fn dedupe_by_id(todos: Vec<Value>) -> Vec<Value> {
        let mut last_index: std::collections::HashMap<String, usize> =
            std::collections::HashMap::new();
        for (i, item) in todos.iter().enumerate() {
            let item_id = item
                .get("id")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .trim()
                .to_string();
            let key = if item_id.is_empty() {
                "?".to_string()
            } else {
                item_id
            };
            last_index.insert(key, i);
        }
        let mut indices: Vec<usize> = last_index.values().copied().collect();
        indices.sort();
        indices.into_iter().map(|i| todos[i].clone()).collect()
    }
}

/// `TodoTool` — Tool trait implementation. The store is shared via
/// `Arc<Mutex<...>>` so all clones of the tool see the same list (the Python
/// tool is single-threaded; we use `tokio::sync::Mutex` so `execute` stays
/// `async`-friendly).
pub struct TodoTool {
    store: Arc<Mutex<TodoStore>>,
}

impl Default for TodoTool {
    fn default() -> Self {
        Self::new()
    }
}

impl TodoTool {
    pub fn new() -> Self {
        Self {
            store: Arc::new(Mutex::new(TodoStore::new())),
        }
    }

    /// Construct a tool that shares an existing store. Lets the engine
    /// reach into the same list for post-compression re-injection.
    pub fn with_store(store: Arc<Mutex<TodoStore>>) -> Self {
        Self { store }
    }

    /// Cloneable handle to the underlying store.
    pub fn store(&self) -> Arc<Mutex<TodoStore>> {
        self.store.clone()
    }
}

#[async_trait]
impl Tool for TodoTool {
    fn name(&self) -> &str {
        "todo"
    }

    fn description(&self) -> &str {
        "Manage your task list for the current session. Use for complex tasks \
         with 3+ steps or when the user provides multiple tasks. Call with no \
         parameters to read the current list.\n\n\
         Writing:\n\
         - Provide 'todos' array to create/update items\n\
         - merge=false (default): replace the entire list with a fresh plan\n\
         - merge=true: update existing items by id, add any new ones\n\n\
         Each item: {id: string, content: string, status: pending|in_progress|completed|cancelled}\n\
         List order is priority. Only ONE item in_progress at a time.\n\
         Mark items completed immediately when done. If something fails, \
         cancel it and add a revised item.\n\n\
         Always returns the full current list."
    }

    fn input_schema(&self) -> JsonSchema {
        json!({
            "type": "object",
            "properties": {
                "todos": {
                    "type": "array",
                    "description": "Task items to write. Omit to read current list.",
                    "items": {
                        "type": "object",
                        "properties": {
                            "id": {
                                "type": "string",
                                "description": "Unique item identifier"
                            },
                            "content": {
                                "type": "string",
                                "description": "Task description"
                            },
                            "status": {
                                "type": "string",
                                "enum": ["pending", "in_progress", "completed", "cancelled"],
                                "description": "Current status"
                            }
                        },
                        "required": ["id", "content", "status"]
                    }
                },
                "merge": {
                    "type": "boolean",
                    "description": "true: update existing items by id, add new ones. false (default): replace the entire list.",
                    "default": false
                }
            },
            "required": []
        })
    }

    fn is_concurrency_safe(&self, _input: &Value) -> bool {
        // Writes mutate shared state; treat as serialized.
        false
    }

    async fn execute(&self, input: Value) -> ToolResult {
        // Validate the schema we care about: if `todos` is present, it must
        // be an array; if `merge` is present, it must be a boolean. Anything
        // else is a 400-class error (mirrors the registry-level guard the
        // Python harness would have done before dispatch).
        let todos_present = input.get("todos").is_some_and(|v| !v.is_null());
        if todos_present && !input["todos"].is_array() {
            return ToolResult {
                content: "Invalid input: 'todos' must be an array".to_string(),
                is_error: true,
            };
        }
        if let Some(merge) = input.get("merge")
            && !merge.is_null()
            && !merge.is_boolean()
        {
            return ToolResult {
                content: "Invalid input: 'merge' must be a boolean".to_string(),
                is_error: true,
            };
        }

        // Reject items with an explicitly invalid status enum value. The Python
        // upstream silently coerces unknown statuses to "pending" via
        // `_validate` — we keep that lenient behaviour for fields the LLM
        // omitted, but a status that's present-and-clearly-wrong (e.g.
        // "bogus") is a schema violation worth surfacing.
        if let Some(arr) = input.get("todos").and_then(|v| v.as_array()) {
            for item in arr {
                if let Some(s) = item.get("status").and_then(|v| v.as_str()) {
                    let s = s.trim().to_ascii_lowercase();
                    if !VALID_STATUSES.contains(&s.as_str()) {
                        return ToolResult {
                            content: format!(
                                "Invalid status '{}'. Expected one of: {}",
                                s,
                                VALID_STATUSES.join(", ")
                            ),
                            is_error: true,
                        };
                    }
                }
            }
        }

        let merge = input
            .get("merge")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);

        let mut store = self.store.lock().await;
        let items = if todos_present {
            let todos = input["todos"].as_array().cloned().unwrap_or_default();
            store.write(todos, merge)
        } else {
            store.read()
        };

        let total = items.len();
        let mut pending = 0;
        let mut in_progress = 0;
        let mut completed = 0;
        let mut cancelled = 0;
        for i in &items {
            match i.status.as_str() {
                "pending" => pending += 1,
                "in_progress" => in_progress += 1,
                "completed" => completed += 1,
                "cancelled" => cancelled += 1,
                _ => {}
            }
        }

        let items_json: Vec<Value> = items.iter().map(|i| i.to_json()).collect();
        let payload = json!({
            "todos": items_json,
            "summary": {
                "total": total,
                "pending": pending,
                "in_progress": in_progress,
                "completed": completed,
                "cancelled": cancelled,
            }
        });

        ToolResult {
            content: serde_json::to_string(&payload).unwrap_or_else(|_| "{}".to_string()),
            is_error: false,
        }
    }

    fn category(&self) -> ToolCategory {
        ToolCategory::Info
    }

    fn describe(&self, input: &Value) -> String {
        if input.get("todos").map(|v| !v.is_null()).unwrap_or(false) {
            "todo write".to_string()
        } else {
            "todo read".to_string()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Tool;
    use crate::registry::ToolRegistry;
    use serde_json::json;

    #[test]
    fn registers_in_dispatcher() {
        let mut registry = ToolRegistry::new();
        registry.register(Box::new(TodoTool::new()));
        let found = registry.get("todo");
        assert!(found.is_some(), "todo tool should be retrievable by name");
        assert_eq!(found.unwrap().name(), "todo");
    }

    #[tokio::test]
    async fn add_item_then_list_returns_it() {
        let tool = TodoTool::new();

        // Write one item (replace mode).
        let write = tool
            .execute(json!({
                "todos": [
                    { "id": "1", "content": "build the thing", "status": "pending" }
                ]
            }))
            .await;
        assert!(!write.is_error, "write should succeed: {}", write.content);
        let parsed: Value = serde_json::from_str(&write.content).unwrap();
        assert_eq!(parsed["summary"]["total"], 1);
        assert_eq!(parsed["summary"]["pending"], 1);
        assert_eq!(parsed["todos"][0]["id"], "1");
        assert_eq!(parsed["todos"][0]["content"], "build the thing");
        assert_eq!(parsed["todos"][0]["status"], "pending");

        // Read back with no args — should still see the same item.
        let read = tool.execute(json!({})).await;
        assert!(!read.is_error);
        let parsed: Value = serde_json::from_str(&read.content).unwrap();
        assert_eq!(parsed["summary"]["total"], 1);
        assert_eq!(parsed["todos"][0]["id"], "1");

        // Merge mode: update status of existing item.
        let merge = tool
            .execute(json!({
                "todos": [
                    { "id": "1", "content": "build the thing", "status": "completed" }
                ],
                "merge": true
            }))
            .await;
        assert!(!merge.is_error);
        let parsed: Value = serde_json::from_str(&merge.content).unwrap();
        assert_eq!(parsed["summary"]["completed"], 1);
        assert_eq!(parsed["summary"]["pending"], 0);
    }

    #[tokio::test]
    async fn rejects_invalid_status_enum() {
        let tool = TodoTool::new();
        let r = tool
            .execute(json!({
                "todos": [
                    { "id": "1", "content": "x", "status": "bogus" }
                ]
            }))
            .await;
        assert!(r.is_error, "invalid status should be rejected");
        assert!(
            r.content.contains("Invalid status"),
            "expected status error, got: {}",
            r.content
        );

        // Also: 'todos' as a non-array is a hard error.
        let r = tool.execute(json!({ "todos": "not an array" })).await;
        assert!(r.is_error);
        assert!(r.content.contains("must be an array"));
    }
}
