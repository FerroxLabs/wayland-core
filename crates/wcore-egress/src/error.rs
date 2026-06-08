//! Egress error type.

use thiserror::Error;

/// A failure on the egress path — either the policy refused to let the request
/// leave the process, or the underlying transport (reqwest) errored.
#[derive(Debug, Error)]
pub enum EgressError {
    /// The egress policy denied this request **before** it was sent. The string
    /// is a human-readable reason suitable for surfacing to the operator.
    ///
    /// In B1 the default [`crate::AllowAllPolicy`] never produces this; it is
    /// the seam B2's allowlist/taint/`ask` logic fills in.
    #[error("egress denied: {0}")]
    Denied(String),

    /// The underlying reqwest transport failed (DNS, TLS, timeout, connection
    /// reset, body decode, …). Forwarded verbatim so existing call-site error
    /// handling that matches on reqwest error kinds keeps working through the
    /// `source()` chain.
    #[error(transparent)]
    Transport(#[from] reqwest::Error),
}

impl EgressError {
    /// True if this is a transport-level timeout. Mirrors
    /// [`reqwest::Error::is_timeout`] so callers that previously inspected the
    /// reqwest error can keep their timeout-specific branches.
    pub fn is_timeout(&self) -> bool {
        matches!(self, EgressError::Transport(e) if e.is_timeout())
    }

    /// True if the request was stopped by the egress policy rather than by the
    /// network.
    pub fn is_denied(&self) -> bool {
        matches!(self, EgressError::Denied(_))
    }

    /// True for a transport-level connection failure (DNS/TCP/TLS). Mirrors
    /// [`reqwest::Error::is_connect`] so call sites that branched on it keep
    /// working through the wrapper.
    pub fn is_connect(&self) -> bool {
        matches!(self, EgressError::Transport(e) if e.is_connect())
    }

    /// True for a redirect-policy error (e.g. too many hops, or an SSRF-unsafe
    /// redirect target refused). Mirrors [`reqwest::Error::is_redirect`].
    pub fn is_redirect(&self) -> bool {
        matches!(self, EgressError::Transport(e) if e.is_redirect())
    }

    /// A display string with any URL stripped (H-2 / SECRETS-26: a provider
    /// that puts a credential in the URL must not leak it when the error is
    /// formatted). For a denial, returns the reason verbatim.
    pub fn redacted(&self) -> String {
        match self {
            EgressError::Denied(reason) => reason.clone(),
            // reqwest's Display appends " for url (<URL>)"; strip from the first
            // " for url (" so a query-string secret never reaches a log/UI.
            EgressError::Transport(e) => {
                let full = e.to_string();
                match full.find(" for url (") {
                    Some(idx) => full[..idx].to_string(),
                    None => full,
                }
            }
        }
    }
}
