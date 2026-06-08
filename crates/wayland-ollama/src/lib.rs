//! Wayland-Ollama — reference plugin for the W2.5 `register_providers`
//! surface. Validates the plugin-api path end-to-end with the smallest
//! plausible provider (no tools, no hooks, no agents, no MCP).
//!
//! The plugin registers an `OllamaProvider` against
//! `ScopedProviderRegistry`. End-to-end use of the provider (i.e. an
//! agent session routing through `--model ollama:<name>`) requires
//! engine-side adapter wiring not present today; see the W8a B.4
//! commit body for the scope-out + follow-up needed.

pub mod plugin;
pub mod provider;

pub use plugin::{MANIFEST_TOML, WaylandOllama, WaylandOllamaFactory};
pub use provider::{OllamaError, OllamaProvider};
