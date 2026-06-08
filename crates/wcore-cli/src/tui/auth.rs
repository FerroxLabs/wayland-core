//! `/auth <provider>` slash-command handler.
//!
//! v0.9.0 Wave-4 E1, Part D. Today only `google-meet` is wired
//! end-to-end; `/auth spotify` resolves to the "Deferred — v0.9.1"
//! notice the Config tab also shows.
//!
//! ## The OAuth round-trip (D026)
//!
//! `google-meet` drives the full loopback authorization-code flow on a
//! background task so the chat surface stays responsive:
//!
//! 1. Confirm `GOOGLE_CLIENT_ID` is in the env (the gate the
//!    `build_google_meet_backend()` resolver also checks).
//! 2. Bind the loopback callback listener (`127.0.0.1:0`, ephemeral
//!    port) and derive the *real* `redirect_uri` from the bound port.
//! 3. Build the authorize URL via `OAuthFlow::build_authorize_url`
//!    (PKCE-S256 by default — B0 contract), capturing `state` + the
//!    PKCE verifier for the callback validation and the token exchange.
//! 4. Open the URL in the user's browser via `open::that_detached`.
//! 5. Wait for the browser redirect on the bound listener, validate the
//!    CSRF `state`, and extract the authorization `code`.
//! 6. Exchange the `code` (plus the PKCE verifier) for tokens at
//!    Google's token endpoint via [`wcore_egress::EgressClient::tool`].
//! 7. Persist the bundle via `OAuthStorage::from_home()?.store(..)` and
//!    post a System/Info turn reporting success (or an honest error at
//!    any step).
//!
//! The blocking steps (waiting on the browser redirect, the token POST)
//! run inside a `tokio::spawn`d task. Completion is reported back to the
//! transcript as a `ProtocolEvent::Info` on the same channel the engine
//! bridge uses — mirroring how `/model` and `/mcp add` report async
//! results. The synchronous dispatch returns immediately with a
//! "started" line.

#[cfg(feature = "remote-registry")]
use tokio::sync::mpsc::UnboundedSender;
#[cfg(any(feature = "remote-registry", test))]
use wcore_agent::oauth::{OAuthFlow, RedirectStrategy};
#[cfg(feature = "remote-registry")]
use wcore_agent::oauth::{OAuthStorage, PkceChallenge};
#[cfg(feature = "remote-registry")]
use wcore_protocol::events::ProtocolEvent;

/// Dispatch a `/auth …` line.
///
/// `events` is the bridge channel the engine uses to post async results
/// back into the transcript. When present, the google-meet flow runs the
/// full loopback round-trip on a background task and reports completion as
/// a later `Info` turn; the returned `String` is the immediate "started"
/// confirmation. When `None` (UI-only boot / tests with no engine), the
/// google-meet arm returns an honest "no engine attached" line instead of
/// silently doing nothing.
#[cfg(feature = "remote-registry")]
pub fn handle_auth_command(line: &str, events: Option<&UnboundedSender<ProtocolEvent>>) -> String {
    let provider = parse_provider(line);
    match provider.as_deref() {
        Some("google-meet") | Some("google_meet") | Some("gmeet") => start_google_meet_flow(events),
        Some("spotify") => spotify_deferred_message(),
        Some(other) => unknown_provider_message(other),
        None => usage_message(),
    }
}

/// Stripped-build variant: with `remote-registry` (and therefore
/// `wcore-egress`) compiled out, the token exchange cannot run. The
/// google-meet arm explains that the network-backed build is required.
#[cfg(not(feature = "remote-registry"))]
pub fn handle_auth_command(line: &str) -> String {
    let provider = parse_provider(line);
    match provider.as_deref() {
        Some("google-meet") | Some("google_meet") | Some("gmeet") => {
            "Google Meet auth needs the network-backed build (the `remote-registry` feature). \
             This binary was built without it."
                .to_string()
        }
        Some("spotify") => spotify_deferred_message(),
        Some(other) => unknown_provider_message(other),
        None => usage_message(),
    }
}

/// Usage hint for a bare `/auth`.
fn usage_message() -> String {
    "Usage: `/auth google-meet`  (Spotify is coming in v0.9.1.)".to_string()
}

/// Hint for an unrecognized provider argument.
fn unknown_provider_message(other: &str) -> String {
    format!(
        "Unknown auth provider: `{other}`. Try: `/auth google-meet` (Spotify is coming in v0.9.1)."
    )
}

/// Extract the provider argument from a `/auth …` line. Trims the
/// leading `/auth`, splits on whitespace, normalises to lowercase.
fn parse_provider(line: &str) -> Option<String> {
    let trimmed = line.trim().strip_prefix("/auth")?.trim();
    if trimmed.is_empty() {
        return None;
    }
    Some(trimmed.split_whitespace().next()?.to_ascii_lowercase())
}

/// The deferred-notice body for Spotify (v0.9.1).
fn spotify_deferred_message() -> String {
    "Spotify OAuth is queued for v0.9.1 — the backend is built but the connect flow ships next \
     release. In the meantime, the spotify_* tools are hidden by the registry."
        .to_string()
}

/// Build the configured Google Meet [`OAuthFlow`], or an `Err` carrying a
/// setup-needed message when `GOOGLE_CLIENT_ID` is missing. Shared by the
/// live round-trip and the unit-test URL builder so they agree on scopes,
/// endpoints, and the PKCE-S256 default.
#[cfg(any(feature = "remote-registry", test))]
fn build_google_meet_flow() -> Result<OAuthFlow, String> {
    let client_id = match read_env("GOOGLE_CLIENT_ID") {
        Some(v) => v,
        None => {
            return Err(
                "Google Meet auth needs `GOOGLE_CLIENT_ID` (and `GOOGLE_CLIENT_SECRET`) in \
                    `~/.wayland/.env`. Open `/config` → Tools & Providers → Google Meet to set \
                    them, or add them directly to the env file."
                    .to_string(),
            );
        }
    };
    let client_secret = read_env("GOOGLE_CLIENT_SECRET");

    Ok(OAuthFlow::new(
        client_id,
        client_secret,
        "https://accounts.google.com/o/oauth2/v2/auth",
        "https://oauth2.googleapis.com/token",
        vec![
            "https://www.googleapis.com/auth/meetings.space.created".into(),
            "https://www.googleapis.com/auth/meetings.space.readonly".into(),
        ],
    )
    // DynamicPort: the redirect_uri is derived from the *actually bound*
    // loopback port (closes R-H7's fixed-port collision), so we never
    // hardcode a placeholder like `127.0.0.1:8765`.
    .with_redirect_strategy(RedirectStrategy::DynamicPort))
    // PKCE-S256 is the default — do NOT opt out.
}

/// Start the Google Meet OAuth round-trip. Spawns the blocking flow on a
/// background task and returns the immediate "started" line. Completion
/// (success or an honest error) arrives later as an `Info` turn on
/// `events`.
///
/// With no `events` channel (UI-only boot / tests), the round-trip cannot
/// report back, so we return an honest line rather than launching a
/// browser into a flow whose result would be dropped.
#[cfg(feature = "remote-registry")]
fn start_google_meet_flow(events: Option<&UnboundedSender<ProtocolEvent>>) -> String {
    // Validate setup synchronously so a missing client id is reported
    // inline (not as a deferred Info turn the user has to wait for).
    let flow = match build_google_meet_flow() {
        Ok(flow) => flow,
        Err(msg) => return msg,
    };

    let tx = match events {
        Some(tx) => tx.clone(),
        None => {
            return "Google Meet auth needs a live engine to complete the browser round-trip. \
                    Start a session and run `/auth google-meet` again."
                .to_string();
        }
    };

    tokio::spawn(async move {
        let message = run_google_meet_connect(flow).await;
        let _ = tx.send(ProtocolEvent::Info {
            msg_id: String::new(),
            message,
        });
    });

    "Connecting Google Meet… opening your browser to authorize. Complete the consent there; \
     this terminal will confirm when the tokens are stored."
        .to_string()
}

/// Drive the full loopback OAuth round-trip and return the System-turn
/// body describing the outcome. Network-bound and browser-launching, so it
/// only runs on the spawned task — never inline.
///
/// Every failure path returns an honest, user-readable line; nothing is
/// swallowed. The authorization `code` and the tokens never appear in the
/// returned message.
#[cfg(feature = "remote-registry")]
async fn run_google_meet_connect(flow: OAuthFlow) -> String {
    // 1. Bind the loopback listener and derive the real redirect_uri.
    let (redirect_uri, listener) = match flow.bind_callback_listener().await {
        Ok(pair) => pair,
        Err(e) => {
            return format!("Google Meet auth could not bind a local callback listener: {e}");
        }
    };

    // 2. Build the authorize URL against the BOUND redirect_uri, capturing
    //    the CSRF state and the PKCE verifier for later validation.
    let (auth_url, state, pkce) = flow.build_authorize_url(&redirect_uri);

    // 3. Open the browser (fire-and-forget). A launch failure (headless
    //    ssh, sandbox) still leaves the user a copyable URL.
    let opened = open::that_detached(&auth_url).is_ok();
    if !opened {
        tracing::debug!(
            target: "wcore_cli::tui::auth",
            "browser launch failed; user must open the authorize URL manually"
        );
    }

    // 4. Wait for the browser redirect, validating state inside.
    let code = match flow.wait_for_code(listener, &state).await {
        Ok(code) => code,
        Err(e) => {
            let url_hint = if opened {
                String::new()
            } else {
                format!("\n\nCould not open a browser. Authorize manually:\n{auth_url}")
            };
            return format!("Google Meet authorization did not complete: {e}{url_hint}");
        }
    };

    // 5. Exchange the code (+ PKCE verifier) for tokens.
    let client = wcore_egress::EgressClient::tool();
    let verifier = pkce.as_ref().map(|p: &PkceChallenge| p.verifier.as_str());
    let tokens = match flow
        .exchange_code(&client, &code, &redirect_uri, verifier)
        .await
    {
        Ok(tokens) => tokens,
        Err(e) => {
            return format!("Google Meet token exchange failed: {e}");
        }
    };

    // 6. Persist the bundle to `~/.wayland/oauth/google_meet.json`.
    let storage = match OAuthStorage::from_home() {
        Ok(s) => s,
        Err(e) => {
            return format!("Google Meet authorized, but the token store could not be opened: {e}");
        }
    };
    if let Err(e) = storage.store("google_meet", &tokens) {
        return format!("Google Meet authorized, but persisting the tokens failed: {e}");
    }

    "Google Meet connected. Tokens stored. The google_meet_* tools are now live.".to_string()
}

/// Read an env var, treating empty / whitespace-only as unset (R-H2).
#[cfg(any(feature = "remote-registry", test))]
fn read_env(key: &str) -> Option<String> {
    std::env::var(key).ok().filter(|v| !v.trim().is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_provider_handles_all_aliases() {
        assert_eq!(
            parse_provider("/auth google-meet"),
            Some("google-meet".to_string())
        );
        assert_eq!(
            parse_provider("/auth google_meet"),
            Some("google_meet".to_string())
        );
        assert_eq!(parse_provider("/auth GMeet"), Some("gmeet".to_string()));
        assert_eq!(parse_provider("/auth"), None);
        assert_eq!(parse_provider("/auth   "), None);
    }

    #[test]
    fn usage_and_unknown_messages_are_actionable() {
        assert!(usage_message().contains("google-meet"));
        let unknown = unknown_provider_message("dropbox");
        assert!(unknown.to_lowercase().contains("unknown"));
        assert!(unknown.contains("dropbox"));
    }

    #[test]
    fn spotify_returns_deferred_notice() {
        let out = spotify_deferred_message();
        assert!(out.contains("v0.9.1"), "want deferred notice: {out}");
        assert!(out.to_lowercase().contains("spotify"));
    }

    #[test]
    #[serial_test::serial]
    fn google_meet_without_client_id_explains_setup() {
        // SAFETY: tests in this module run serially; the env mutation
        // is reverted before exiting.
        let saved = std::env::var("GOOGLE_CLIENT_ID").ok();
        unsafe { std::env::remove_var("GOOGLE_CLIENT_ID") };
        let out = build_google_meet_flow()
            .expect_err("missing client id must be an Err with a setup hint");
        assert!(out.contains("GOOGLE_CLIENT_ID"), "want env-var hint: {out}");
        assert!(out.contains(".wayland/.env"), "want setup hint: {out}");
        if let Some(v) = saved {
            unsafe { std::env::set_var("GOOGLE_CLIENT_ID", v) };
        }
    }

    /// D026: the authorize URL must be built against a REAL bound loopback
    /// redirect_uri (ephemeral port), not the old hardcoded `:8765`
    /// placeholder. Bind the listener exactly as the live flow does, then
    /// assert the URL carries that port's `/callback` and the PKCE-S256
    /// challenge.
    ///
    /// This is the regression guard for the placeholder removal: if the
    /// flow ever reverts to a hardcoded redirect, the bound-port assertion
    /// fails.
    #[tokio::test]
    #[serial_test::serial]
    async fn google_meet_authorize_url_uses_bound_loopback_redirect() {
        // SAFETY: serial test; revert env before exiting.
        let saved_id = std::env::var("GOOGLE_CLIENT_ID").ok();
        let saved_secret = std::env::var("GOOGLE_CLIENT_SECRET").ok();
        unsafe { std::env::set_var("GOOGLE_CLIENT_ID", "test-client-id-12345") };
        unsafe { std::env::remove_var("GOOGLE_CLIENT_SECRET") };

        let flow = build_google_meet_flow().expect("with a client id the flow must build");
        // Bind the real loopback listener — this is the placeholder's
        // replacement. The redirect_uri is derived from the bound port.
        let (redirect_uri, listener) = flow
            .bind_callback_listener()
            .await
            .expect("loopback bind must succeed");
        let port = listener.local_addr().expect("bound addr").port();

        // The redirect must be a loopback URL on the actually-bound port,
        // NOT the old hardcoded 127.0.0.1:8765 placeholder.
        assert!(
            redirect_uri.starts_with("http://127.0.0.1:"),
            "redirect must bind loopback: {redirect_uri}"
        );
        assert!(
            redirect_uri.contains(&format!(":{port}/callback")),
            "redirect must carry the bound port: {redirect_uri}"
        );
        assert_ne!(
            redirect_uri, "http://127.0.0.1:8765/callback",
            "the hardcoded placeholder redirect must be gone"
        );

        let (auth_url, _state, pkce) = flow.build_authorize_url(&redirect_uri);
        assert!(pkce.is_some(), "PKCE-S256 pair must be generated");
        assert!(
            auth_url.contains("accounts.google.com/o/oauth2/v2/auth"),
            "want Google authorize endpoint: {auth_url}"
        );
        assert!(
            auth_url.contains("code_challenge_method=S256"),
            "want PKCE-S256 in URL: {auth_url}"
        );
        assert!(
            auth_url.contains("client_id=test-client-id-12345"),
            "want client_id in URL: {auth_url}"
        );
        // The bound loopback redirect (not the old :8765 placeholder) must be
        // carried in the authorize URL. Substring checks are robust without a
        // urlencoding dependency: the loopback host survives percent-encoding
        // (digits and dots are unreserved).
        assert!(redirect_uri.starts_with("http://127.0.0.1:") && !redirect_uri.contains("8765"));
        assert!(
            auth_url.contains("127.0.0.1") && !auth_url.contains("8765"),
            "authorize URL must carry the bound loopback redirect_uri, not the placeholder: {auth_url}"
        );

        // Restore env.
        match saved_id {
            Some(v) => unsafe { std::env::set_var("GOOGLE_CLIENT_ID", v) },
            None => unsafe { std::env::remove_var("GOOGLE_CLIENT_ID") },
        }
        if let Some(v) = saved_secret {
            unsafe { std::env::set_var("GOOGLE_CLIENT_SECRET", v) };
        }
    }

    /// With no engine bridge attached, the google-meet arm must NOT open a
    /// browser into a dropped flow — it returns an honest line instead.
    #[cfg(feature = "remote-registry")]
    #[test]
    #[serial_test::serial]
    fn google_meet_without_events_channel_is_honest() {
        let saved = std::env::var("GOOGLE_CLIENT_ID").ok();
        unsafe { std::env::set_var("GOOGLE_CLIENT_ID", "test-client-id-12345") };
        let out = start_google_meet_flow(None);
        assert!(
            out.to_lowercase().contains("engine"),
            "want a no-engine hint, got: {out}"
        );
        match saved {
            Some(v) => unsafe { std::env::set_var("GOOGLE_CLIENT_ID", v) },
            None => unsafe { std::env::remove_var("GOOGLE_CLIENT_ID") },
        }
    }
}
