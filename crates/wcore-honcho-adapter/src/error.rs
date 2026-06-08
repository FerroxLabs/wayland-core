//! Adapter-local error helpers.
//!
//! Most call sites bubble through `UserModelError` (the trait-level error
//! the engine consumes). This module owns the one conversion that crosses
//! the crate boundary: `HonchoError → UserModelError`.

use wayland_honcho::HonchoError;
use wcore_user_model::UserModelError;

/// Map a `HonchoError` onto the trait-level `UserModelError` taxonomy.
/// Each Honcho variant lands on the closest semantic UserModel variant
/// so callers can match without knowing about Honcho.
pub fn honcho_to_user_model(err: HonchoError) -> UserModelError {
    match err {
        HonchoError::Api(msg) => UserModelError::Rejected(msg),
        HonchoError::Transport(e) => UserModelError::Transport(e.to_string()),
        HonchoError::Egress(e) => UserModelError::Transport(e.to_string()),
        HonchoError::MissingApiKey => UserModelError::Auth("HONCHO_API_KEY missing".to_string()),
    }
}
