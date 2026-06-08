//! v0.6.4 Task 2.1 — `UserModelSpec`, the plugin-api-native data contract
//! for plugin-registered user-model backends (e.g. Honcho).
//!
//! Plain data only. No `Arc<dyn Trait>`, no closures. The host adapter in
//! `wcore-agent` (Task 2.2) reads these fields and constructs the real
//! client (e.g. `wayland_honcho::HonchoClient`). Mirrors the pattern used by
//! `McpServerSpec` and `RuleSpec`: spec-then-reify, isolation boundary
//! preserved.
//!
//! Fields are deliberately minimal — the smallest surface that lets a host
//! adapter pick a backend, point it at the right endpoint, and read credentials
//! from the environment. Anything backend-specific rides in `config`
//! (`serde_json::Value`).
//!
//! ## What's a "user model" here?
//!
//! A long-lived per-user identity store the engine threads through completions.
//! Honcho is the canonical example: `learn_preference(user_id, k, v)` /
//! `recall_user(user_id) -> UserProfile`. Other backends (in-RAM, sqlite,
//! third-party services) plug in through this same spec.
//!
//! Backend selection lives in `backend` as an opaque tag — the host adapter
//! maps it to the actual client. v0.6.4 ships only `"honcho"`; future tags are
//! a forward-compatible extension and require no surface change here.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema, PartialEq, Eq)]
pub struct UserModelSpec {
    /// Human-readable name. Plugin-supplied, e.g. `"honcho"`, `"local-fs"`.
    /// Used for diagnostics and duplicate detection.
    pub name: String,
    /// One-line description for surfacing in `wcore status` / settings.
    pub description: String,
    /// Backend tag the host adapter uses to pick the concrete client.
    /// Today `"honcho"` is the only recognised value; unknown tags are
    /// captured but produce a typed error at reification time (Task 2.2).
    pub backend: String,
    /// Optional base URL for the backend service. `None` lets the backend
    /// pick its default (e.g. Honcho's public endpoint).
    #[serde(default)]
    pub base_url: Option<String>,
    /// Environment variable from which the host adapter reads the API key.
    /// `None` means the backend doesn't need a key (mock / on-disk variants).
    #[serde(default)]
    pub api_key_env: Option<String>,
    /// Opaque backend-specific config. Anything not modeled by the fields
    /// above. Defaults to `Value::Null`.
    #[serde(default)]
    pub config: serde_json::Value,
}
