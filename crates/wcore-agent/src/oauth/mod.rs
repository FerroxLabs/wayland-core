//! v0.9.0 Wave-1 B0 — shared OAuth infrastructure.
//!
//! Provides a generic `OAuthFlow` (authorize-URL + token-exchange) plus
//! single-flight refresh and encrypted at-rest token storage. Designed
//! so Wave-1 sub-agent B9 (Google Meet) can wire its provider directly;
//! future OAuth providers add a `pub fn build_<provider>_flow()` here.
//!
//! Security primitives baked in by default:
//! - PKCE (S256) is required by default — opt-out is explicit via
//!   `OAuthFlow::without_pkce()` for legacy providers that reject the
//!   challenge.
//! - CSRF `state` token is 32 random bytes from `OsRng`, compared with
//!   `subtle::ConstantTimeEq` on the callback so timing leaks cannot
//!   forge a valid replay.
//! - Token storage at `~/.wayland/oauth/{provider}.json` enforces dir
//!   mode `0700` + file mode `0600` on Unix.
//! - Single-flight refresh ensures N concurrent refresh calls coalesce
//!   into one network round-trip.
//! - Callback listener has a 5-minute idle timeout so a user who closes
//!   the browser tab doesn't leak the bound port.

pub mod chatgpt;
pub mod flow;
pub mod pkce;
pub mod storage;
pub mod xai;

pub use chatgpt::{
    ChatGptLoginStatus, ChatGptTokenManager, CodexClaims, build_chatgpt_flow, decode_codex_claims,
    login_status as chatgpt_login_status,
};
pub use flow::{OAuthFlow, OAuthTokens, RedirectStrategy, RefreshError, SingleFlightRefresh};
pub use pkce::{PkceChallenge, PkceMode};
pub use storage::{OAuthStorage, OAuthStorageError};
