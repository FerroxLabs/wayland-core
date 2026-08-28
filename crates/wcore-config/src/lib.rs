// Configuration layer: runtime Config, ProviderCompat, auth, hooks, provider-specific configs.

// v0.6.1 H2-R5: reusable circuit-breaker primitive shared by wcore-providers
// and wcore-tools. Lives here so neither crate needs to depend on the other.
pub mod circuit_breaker;

// v0.6.1 hardening: durable atomic_write helper. Used by credentials,
// memory store, memory index — anywhere a partial write would
// corrupt user-visible state.
pub mod atomic_io;
pub use atomic_io::{atomic_write, atomic_write_checked};
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
// #693 — the non-bypassable command floor shared by BOTH shell surfaces
// (`wcore_tools::bash::BashTool` and `wcore_skills::shell`). It lives here
// because `wcore-skills` does not depend on `wcore-tools`, and because the
// protected set resolves through this crate own `profile_home()` /
// `wayland_config_dir()`.
pub mod command_floor;
pub mod compact;
pub mod compat;
pub mod confidential_blob;
pub mod config;
// Anvil (native gated-forge engine): `[anvil]` kill-switch config.
pub mod anvil;
// THE KERNEL (#255): single per-turn context-window computation. See the
// module header. Co-located with `limits` (the per-model window table).
pub mod context_window;
// Wave SD: CredentialsStore trait + plaintext/keyring backends.
pub mod credentials;
pub mod crucible;
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
// F25-04: the plugin approval gate + its content-digest primitive. Lives here
// because BOTH `wcore-cli` (which writes approvals) and `wcore-agent` (whose
// loader enforces them) must agree byte-for-byte on the digest and the verdict;
// duplicating either would create two answers to "is this plugin approved?".
pub mod network_path;
pub mod plugin_governance;
pub mod plugins_config;
pub mod portability;
pub mod profile;
// wayland#896: quiesced snapshot lease over profile state. Lives here because
// it is the crate that owns profile_home()/profiles_root() — the roots it must
// enumerate completely — and it must stay reachable from every producer surface.
pub mod quiesce;
pub mod resolution_provenance;
// #1173: shared by `wcore-providers` (which sends the keyless placeholder
// bearer) and this crate's credential resolution (which must stop refusing to
// start before that path is reached). One predicate, two layers, no drift.
pub mod self_hosted;
pub mod shell;
// Filesystem-aware SQLite journal-mode selection. WAL corrupts databases on
// network filesystems (measured); every SQLite call site selects through here.
pub mod sqlite_journal;
// Consistent point-in-time capture of a live SQLite database. A WAL database is
// a trio of files that is only meaningful together; copying them independently
// yields a corrupt restore (measured). Needs a real connection, so it is gated
// on the same `sqlite` feature as `sqlite_journal`'s connection-taking half.
#[cfg(feature = "sqlite")]
pub mod sqlite_snapshot;
pub mod tools;
pub mod workspace_trust;
