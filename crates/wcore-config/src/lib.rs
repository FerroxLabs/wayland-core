// Configuration layer: runtime Config, ProviderCompat, auth, hooks, provider-specific configs.

// v0.6.1 H2-R5: reusable circuit-breaker primitive shared by wcore-providers
// and wcore-tools. Lives here so neither crate needs to depend on the other.
pub mod circuit_breaker;

// v0.6.1 hardening: durable atomic_write helper. Used by credentials,
// memory store, memory index — anywhere a partial write would
// corrupt user-visible state.
pub mod atomic_io;
pub use atomic_io::atomic_write;
// W8a A.5: BudgetConfig TOML schema (consumed by wcore-agent::budget).
pub mod budget;
// W8c.1 E.11: BrowserConfig TOML schema (consumed by wcore-browser::select_provider).
pub mod browser;
// Data-driven OpenAI-compatible provider catalog (bundled `data/providers.toml`).
// Lets `--provider <id>` resolve any catalog entry through the OpenAI-compat
// path with no per-provider `ProviderType` arm.
pub mod catalog;
// #158: conservative ChatGPT-subscription (OAuth) model-catalog filtering.
// Tier→unavailable-models gating DATA + JWT plan-claim decode; consumed by
// `wcore_providers::OpenAIChatGptProvider::list_models`.
pub mod chatgpt_catalog;
// W8c.2 F.1: CuaConfig TOML schema (consumed by wcore-cua::adapter::from_spec).
pub mod compact;
pub mod compat;
pub mod config;
// THE KERNEL (#255): single per-turn context-window computation. See the
// module header. Co-located with `limits` (the per-model window table).
pub mod context_window;
// Wave SD: CredentialsStore trait + plaintext/keyring backends.
pub mod credentials;
pub mod cua;
pub mod debug;
// v0.9.0 W4 E1 / S-H3: atomic .env writer with strict key/value validation.
pub mod env_file;
pub mod file_cache;
pub mod forge_discovery;
pub mod hooks;
// v0.7.0 Task 1.B.1: convenience facade over `keyring` for `wayland init` + channels.
pub mod keychain;
pub mod limits;
pub mod mcp_cred_refs;
pub mod plan;
pub mod plugins_config;
pub mod profile;
pub mod shell;
pub mod tools;
