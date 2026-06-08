//! Bot Framework OAuth2 client-credentials token cache.
//!
//! The Bot Framework token endpoint issues ~1-hour tokens. We cache
//! and refresh 5 minutes before expiry to avoid per-send latency.

use serde::Deserialize;
use tokio::sync::Mutex;

use crate::error::MsTeamsError;

/// Token endpoint for the Bot Framework multi-tenant common path.
pub const BF_TOKEN_URL: &str =
    "https://login.microsoftonline.com/botframework.com/oauth2/v2.0/token";
const BF_TOKEN_SCOPE: &str = "https://api.botframework.com/.default";
const REFRESH_BUFFER_SECS: u64 = 300; // 5 min

#[derive(Deserialize)]
struct TokenResponse {
    access_token: String,
    expires_in: u64,
}

#[derive(Debug)]
struct CachedToken {
    token: String,
    expires_at_secs: u64,
}

/// Shared, mutex-protected token cache. Clone to share across the channel.
#[derive(Debug, Default, Clone)]
pub struct TokenCache {
    inner: std::sync::Arc<Mutex<Option<CachedToken>>>,
}

impl TokenCache {
    pub fn new() -> Self {
        Self::default()
    }

    /// Return a valid Bearer token, refreshing if expired or near expiry.
    pub async fn get_token(
        &self,
        http: &wcore_egress::EgressClient,
        app_id: &str,
        app_password: &str,
        token_url: &str,
    ) -> Result<String, MsTeamsError> {
        let now = now_secs();
        {
            let guard = self.inner.lock().await;
            if let Some(ref cached) = *guard
                && cached.expires_at_secs > now + REFRESH_BUFFER_SECS
            {
                return Ok(cached.token.clone());
            }
        }
        // Fetch a fresh token.
        let new_token = fetch_token(http, app_id, app_password, token_url).await?;
        let expires_at = now + new_token.expires_in.saturating_sub(REFRESH_BUFFER_SECS);
        let mut guard = self.inner.lock().await;
        *guard = Some(CachedToken {
            token: new_token.access_token.clone(),
            expires_at_secs: expires_at,
        });
        Ok(new_token.access_token)
    }
}

async fn fetch_token(
    http: &wcore_egress::EgressClient,
    app_id: &str,
    app_password: &str,
    token_url: &str,
) -> Result<TokenResponse, MsTeamsError> {
    let params = [
        ("grant_type", "client_credentials"),
        ("client_id", app_id),
        ("client_secret", app_password),
        ("scope", BF_TOKEN_SCOPE),
    ];

    let resp = http
        .post(token_url)
        .form(&params)
        .send()
        .await
        .map_err(|e| MsTeamsError::Network(e.to_string()))?;

    let status = resp.status().as_u16();
    if !resp.status().is_success() {
        let body = resp.text().await.unwrap_or_default();
        return Err(MsTeamsError::TokenFetch { status, body });
    }

    resp.json::<TokenResponse>()
        .await
        .map_err(|e| MsTeamsError::Parse(e.to_string()))
}

fn now_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}
