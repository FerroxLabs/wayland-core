//! Wave B2 — `PolicyEngine`: explicit ACL over `(Actor, Resource, Action)`.
//!
//! v0.3 scope: explicit grants only. No role hierarchy, no inheritance, no
//! OAuth/OIDC. `Actor::System` is the single hard-coded bypass — it represents
//! the engine itself making internal calls, not a user-facing identity.

use std::fmt;
use std::sync::Arc;

use serde::{Deserialize, Serialize};

use crate::error::{DenyReason, PolicyResult};

/// Who is making the call.
///
/// `User` = human identity (alice, bob). `Agent` = sub-agent / spawned worker
/// (worker-1, planner). `System` = the engine itself — internal book-keeping
/// like reading config files, never an externally-presentable identity.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Actor {
    User(String),
    Agent(String),
    System,
}

/// What is being acted on.
///
/// `File` resources support a minimal glob (see [`glob_match`]). `Memory` uses
/// the tier name as a string so the transport stays decoupled from
/// `wcore-memory`'s enum.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Resource {
    Tool(String),
    File(String),
    McpServer(String),
    Memory(String),
}

/// What kind of operation is being attempted.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Action {
    Invoke,
    Read,
    Write,
    Delete,
}

/// A single grant: actor X may perform action Y on resource Z.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Permission {
    pub actor: Actor,
    pub resource: Resource,
    pub action: Action,
}

/// T7 audit event emitted whenever `PolicyEngine::grant` is called and a
/// sink is configured. Wall clock is `Utc::now()` taken inside `grant`; if a
/// caller needs a stable clock for tests, mock at the sink boundary.
#[derive(Debug, Clone)]
pub struct GrantAuditEvent {
    pub permission: Permission,
    pub at_ms: i64,
}

/// T7 audit sink — invoked on every `PolicyEngine::grant` when a sink is
/// configured via `set_audit_sink`. Intentionally minimal: the sink decides
/// whether to hash-chain, ship to a remote logger, or just buffer in memory.
/// Implementors must be `Send + Sync` because `PolicyEngine` is cloneable
/// and may be shared across tasks.
pub trait GrantAuditSink: Send + Sync + fmt::Debug {
    fn record(&self, event: GrantAuditEvent);
}

/// In-memory policy store. Linear-scan on `check`; fine for v0.3's expected
/// grant count (single-digit to low-double-digit per session). If we ever
/// exceed that, swap the `Vec` for a `HashMap<(Actor, Action), Vec<Resource>>`.
#[derive(Debug, Default, Clone)]
pub struct PolicyEngine {
    grants: Vec<Permission>,
    /// T7 closure: optional audit sink. `None` by default — backwards
    /// compatible with the v0.3 surface that had no audit hook.
    audit_sink: Option<Arc<dyn GrantAuditSink>>,
}

impl PolicyEngine {
    pub fn new() -> Self {
        Self::default()
    }

    /// T7 closure: install an audit sink. Idempotent — call again to replace.
    /// Pass an `Arc::clone(&sink)` if you also need to read from it from the
    /// test side.
    pub fn set_audit_sink(&mut self, sink: Arc<dyn GrantAuditSink>) {
        self.audit_sink = Some(sink);
    }

    /// Add a grant. Duplicates are tolerated — `check` returns `Ok` on the
    /// first match, so duplicates are inert.
    ///
    /// T7: if an audit sink is configured, a `GrantAuditEvent` is emitted
    /// **after** the grant is recorded. Sink failures are intentionally
    /// silent at this layer — the sink contract is observability, not
    /// gating; a panicking sink would brick policy mutation.
    pub fn grant(&mut self, p: Permission) {
        self.grants.push(p.clone());
        if let Some(sink) = &self.audit_sink {
            let at_ms = chrono::Utc::now().timestamp_millis();
            sink.record(GrantAuditEvent {
                permission: p,
                at_ms,
            });
        }
    }

    /// Check whether `actor` may perform `action` on `resource`.
    ///
    /// Returns `Ok(())` on allow, `Err(DenyReason)` on deny. The deny reason
    /// distinguishes "no grant at all" from "grant existed but path didn't
    /// match" so callers can surface useful errors.
    pub fn check(&self, actor: &Actor, resource: &Resource, action: Action) -> PolicyResult<()> {
        // System actor bypasses ACL. This is the engine's own internal
        // callers (config reads, hook execution, etc.) — never an
        // externally-presentable identity.
        if matches!(actor, Actor::System) {
            return Ok(());
        }

        let mut matched_actor_resource_kind = false;
        for g in &self.grants {
            if &g.actor != actor {
                continue;
            }
            if g.action != action {
                continue;
            }
            match (&g.resource, resource) {
                (Resource::Tool(a), Resource::Tool(b)) if a == b => return Ok(()),
                (Resource::McpServer(a), Resource::McpServer(b)) if a == b => {
                    return Ok(());
                }
                (Resource::Memory(a), Resource::Memory(b)) if a == b => return Ok(()),
                (Resource::File(pat), Resource::File(path)) => {
                    matched_actor_resource_kind = true;
                    if glob_match(pat, path) {
                        return Ok(());
                    }
                }
                _ => {}
            }
        }

        if matched_actor_resource_kind {
            Err(DenyReason::PathNotInAllowlist)
        } else {
            Err(DenyReason::NoMatchingGrant)
        }
    }

    /// Number of grants currently held. Useful for tests and trace events.
    pub fn len(&self) -> usize {
        self.grants.len()
    }

    pub fn is_empty(&self) -> bool {
        self.grants.is_empty()
    }
}

/// Minimal glob matcher used for `Resource::File` patterns.
///
/// Supported forms:
/// - `**`               — matches any path
/// - `<prefix>/**`      — matches `<prefix>` itself or anything beneath it
/// - `**/<suffix>`      — matches any path that ends in `<suffix>`
/// - otherwise          — exact string match
///
/// T5 closure: any request path containing a `..` path-component is
/// rejected up-front — string-prefix matching against a normalized prefix
/// is otherwise trivially defeated by `/<prefix>/../<elsewhere>`. Patterns
/// containing `..` are also rejected so no one writes a grant that depends
/// on traversal semantics.
///
/// Dependency-free on purpose. If grants demand richer patterns (`*.rs`,
/// brace expansion, multi-segment `**` in the middle) swap in the `globset`
/// crate and re-export this function's signature.
fn glob_match(pattern: &str, path: &str) -> bool {
    // T5 closure: reject `..` segments before any prefix/suffix logic.
    // `has_dotdot_segment` is segment-aware so paths like `..rc` and
    // `..../file` (no `..` *segment*) still match cleanly.
    if has_dotdot_segment(pattern) || has_dotdot_segment(path) {
        return false;
    }
    if pattern == "**" {
        return true;
    }
    if let Some(prefix) = pattern.strip_suffix("/**") {
        return path == prefix || path.starts_with(&format!("{prefix}/"));
    }
    if let Some(suffix) = pattern.strip_prefix("**/") {
        return path.ends_with(suffix);
    }
    pattern == path
}

/// T5 helper: true iff `s` contains a `..` as a `/`-delimited path segment.
/// Accepts both forward and back slashes so Windows-style paths handed in
/// by tests or non-Unix callers don't slip through.
fn has_dotdot_segment(s: &str) -> bool {
    s.split(['/', '\\']).any(|seg| seg == "..")
}

#[cfg(test)]
mod tests {
    //! Tests for the private `glob_match` helper. Public ACL behavior is
    //! covered in `tests/acl_test.rs`.

    use super::glob_match;

    #[test]
    fn double_star_matches_anything() {
        assert!(glob_match("**", "/etc/passwd"));
        assert!(glob_match("**", ""));
    }

    #[test]
    fn prefix_double_star_matches_subtree_and_root() {
        assert!(glob_match("/tmp/workspace/**", "/tmp/workspace"));
        assert!(glob_match("/tmp/workspace/**", "/tmp/workspace/file.txt"));
        assert!(glob_match("/tmp/workspace/**", "/tmp/workspace/a/b.rs"));
        assert!(!glob_match("/tmp/workspace/**", "/tmp/elsewhere"));
        assert!(!glob_match("/tmp/workspace/**", "/tmp/workspaceX"));
    }

    #[test]
    fn suffix_double_star_matches_endings() {
        assert!(glob_match("**/secret.key", "/etc/secret.key"));
        assert!(glob_match("**/secret.key", "secret.key"));
        assert!(!glob_match("**/secret.key", "/etc/secret.keys"));
    }

    #[test]
    fn exact_match_when_no_glob() {
        assert!(glob_match("/etc/passwd", "/etc/passwd"));
        assert!(!glob_match("/etc/passwd", "/etc/passwd.bak"));
    }
}
