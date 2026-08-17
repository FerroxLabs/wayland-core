//! The adapter's live access token, and the one place it is renewed (#936).
//!
//! # The defect this closes
//!
//! The adapter held a `String` access token read once in `start()`. Matrix
//! access tokens issued through the Matrix Authentication Service (the OIDC
//! path `matrix.org` is moving accounts onto) are **short-lived by design**,
//! and a client is expected to hold a refresh token alongside them. With no
//! refresh support the adapter's entire response to expiry was to publish
//! `AuthExpired` and stop — it could *report* a dead token and never *recover*
//! from one.
//!
//! # Cross-process safety is the bar, not in-process single-flight
//!
//! A Matrix refresh token ROTATES: `POST /_matrix/client/v3/refresh` may return
//! a new one, and the presented one is then spent. Two Wayland processes
//! sharing one credentials store that both POST the same refresh token get one
//! winner and one `M_UNKNOWN_TOKEN`, and under RFC 6819 §5.2.2.3 a server is
//! sanctioned to revoke the whole grant — i.e. BOTH processes logged out, not
//! one poll failed. An in-process mutex cannot see a sibling process, so this
//! module takes [`ExclusiveFileLock`] — the same cross-process primitive
//! `wcore_agent::oauth::refresh_lock` takes, from the same `wcore-config`
//! layer — around the whole load → decide → POST → store critical section.
//!
//! The lock releases in `Drop` (never on a timer, never on an explicit
//! unlock this code could skip on an early return), so every `?` and every
//! error arm below frees it.
//!
//! # What "fail closed" means here
//!
//! A refresh path that papers over a real revocation is worse than none, so
//! the outcomes are three, not two:
//!
//! * [`Renewal::Renewed`] — a usable access token is installed.
//! * [`Renewal::Deferred`] — transient (network, lock contention, store I/O).
//!   The caller backs off. This is explicitly NOT a credential verdict: a
//!   blip on the refresh endpoint must not read as "your token was revoked".
//! * [`Renewal::Fatal`] — the credential is dead. `AuthExpired` is published
//!   exactly once, so the health surface says `Unauthenticated` no matter
//!   which call site discovered it.

use std::collections::VecDeque;
use std::hash::{Hash, Hasher};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{Arc, RwLock};
use std::time::{Duration, Instant};

use tokio::sync::Mutex;

use wcore_channels::event::ChannelEvent;
use wcore_config::credentials::{CredentialsStore, ExclusiveFileLock, LockPolicy};

use crate::error::MatrixError;
use crate::rest;

/// Outer wall-clock cap on one refresh round-trip. Every lock timing below is
/// derived from it, so the two cannot drift apart silently.
pub(crate) const REFRESH_POST_TIMEOUT: Duration = Duration::from_secs(POST_TIMEOUT_SECS);
const POST_TIMEOUT_SECS: u64 = 20;

/// Budget for the credential-store reads and writes the critical section does
/// around the POST.
const STORE_BUDGET_SECS: u64 = 5;

/// The most time a healthy holder can occupy the lock.
const MAX_HOLD_SECS: u64 = POST_TIMEOUT_SECS + STORE_BUDGET_SECS;

/// How often a holder re-stamps the lockfile. With a heartbeat the lockfile's
/// mtime tracks LIVENESS rather than acquisition, so staleness is sized against
/// the beat and a crashed holder is detected in seconds regardless of how long
/// the held work takes.
const HEARTBEAT_SECS: u64 = 2;

/// How long an un-refreshed lockfile may sit before a waiter treats its holder
/// as crashed. Five missed beats of slack, so a LIVE holder is never stolen
/// from — a steal from a live holder is the exact double-POST this lock exists
/// to prevent.
const STALE_AFTER_SECS: u64 = HEARTBEAT_SECS * 5;

/// How long a waiter keeps trying. Above `MAX_HOLD_SECS` so healthy contention
/// never times out, and above `STALE_AFTER_SECS` so a crashed holder is always
/// reached by the steal within the wait.
const WAIT_CEILING_SECS: u64 = MAX_HOLD_SECS + STALE_AFTER_SECS + 5;

const _: () = assert!(
    WAIT_CEILING_SECS > MAX_HOLD_SECS,
    "a waiter that gives up before a healthy holder can finish fires the \
     contention path on the happy path"
);
const _: () = assert!(
    WAIT_CEILING_SECS > STALE_AFTER_SECS,
    "a ceiling below staleness means a crashed holder is never stolen from"
);
const _: () = assert!(
    STALE_AFTER_SECS >= HEARTBEAT_SECS * 3,
    "staleness must leave a live holder several missed heartbeats of slack"
);

/// Renew this long before the homeserver's stated expiry.
///
/// Derived from the `/sync` long-poll, not picked: a renewal decision is taken
/// once per loop iteration, and an iteration can be parked in a long poll for
/// [`crate::sync::SYNC_TIMEOUT_MS`] plus its read-timeout buffer. A margin
/// below that would let a token die *inside* a poll that had already started,
/// which converts the proactive path back into the reactive one it exists to
/// avoid.
pub(crate) const RENEW_MARGIN: Duration = Duration::from_secs(120);

const _: () = assert!(
    RENEW_MARGIN.as_millis() as u64 > crate::sync::SYNC_TIMEOUT_MS + 10_000,
    "the renewal margin must exceed one full long-poll plus its read-timeout \
     buffer, or the token can expire inside a poll already in flight"
);

/// Consecutive renewals with no successful `/sync` in between before the
/// credential is declared dead.
///
/// Without this a homeserver that keeps answering 401 `soft_logout` *after* a
/// successful refresh turns this module into an unbounded refresh loop: every
/// iteration spends a rotating refresh token and POSTs again. Three attempts
/// is enough to ride out a genuine race with a peer process and far short of
/// a self-inflicted denial of service.
const MAX_RENEWALS_WITHOUT_PROGRESS: u32 = 3;

/// What a renewal attempt produced. See the module docs for why there are
/// three of these and not two.
#[derive(Debug)]
pub(crate) enum Renewal {
    /// A usable access token is installed; retry the call that failed.
    Renewed,
    /// Transient. Back off and try again later; the credential is NOT accused.
    Deferred(String),
    /// The credential is dead. `AuthExpired` has been published exactly once.
    Fatal,
}

/// The access token currently in play, and what we know about its lifetime.
struct Snapshot {
    access: String,
    /// When the homeserver said this token expires. `None` means "not stated"
    /// — a token read from the credentials store carries no expiry, so no
    /// proactive renewal is scheduled until this adapter has done one refresh
    /// and been told `expires_in_ms`.
    expires_at: Option<Instant>,
}

/// Everything [`TokenSource::new`] needs. A struct rather than nine positional
/// arguments.
pub(crate) struct TokenSourceParams {
    pub creds: Arc<dyn CredentialsStore>,
    pub access_handle: String,
    pub refresh_handle: Option<String>,
    pub access_token: String,
    pub http: wcore_egress::EgressClient,
    pub api_base: String,
    pub inbox: Arc<Mutex<VecDeque<ChannelEvent>>>,
}

/// The adapter's live access token and the only path that replaces it.
pub(crate) struct TokenSource {
    creds: Arc<dyn CredentialsStore>,
    access_handle: String,
    refresh_handle: Option<String>,
    http: wcore_egress::EgressClient,
    api_base: String,
    lock_path: PathBuf,
    inbox: Arc<Mutex<VecDeque<ChannelEvent>>>,
    current: RwLock<Snapshot>,
    /// `AuthExpired` is published at most once per channel start, so a send
    /// path and the `/sync` loop discovering the same dead credential do not
    /// produce two health events.
    reported: AtomicBool,
    renewals_without_progress: AtomicU32,
}

impl TokenSource {
    pub(crate) fn new(params: TokenSourceParams) -> Self {
        let lock_path = refresh_lock_path(&params.api_base, &params.access_handle);
        Self {
            creds: params.creds,
            access_handle: params.access_handle,
            refresh_handle: params.refresh_handle,
            http: params.http,
            api_base: params.api_base,
            lock_path,
            inbox: params.inbox,
            current: RwLock::new(Snapshot {
                access: params.access_token,
                expires_at: None,
            }),
            reported: AtomicBool::new(false),
            renewals_without_progress: AtomicU32::new(0),
        }
    }

    /// The access token to authenticate the next call with.
    pub(crate) fn access(&self) -> String {
        self.read().access.clone()
    }

    /// Whether this adapter can renew at all. Reported on the health reason
    /// when it cannot, because "no refresh token is configured" and "the
    /// homeserver revoked you" are different operator actions.
    pub(crate) fn can_renew(&self) -> bool {
        self.refresh_handle.is_some()
    }

    /// A `/sync` succeeded: the token in play is live, so the renewal-loop
    /// guard resets.
    pub(crate) fn mark_progress(&self) {
        self.renewals_without_progress.store(0, Ordering::Relaxed);
    }

    /// True when the homeserver's stated expiry is inside [`RENEW_MARGIN`].
    pub(crate) fn renewal_due(&self) -> bool {
        match self.read().expires_at {
            Some(at) => at <= Instant::now() + RENEW_MARGIN,
            None => false,
        }
    }

    /// Proactive renewal, ahead of a stated `expires_in_ms`.
    pub(crate) async fn renew_before_expiry(&self) -> Renewal {
        let presented = self.access();
        self.renew(&presented, "access token is about to expire")
            .await
    }

    /// Reactive renewal, after the homeserver rejected a call.
    ///
    /// **`soft_logout` is the decision.** Matrix distinguishes a token that
    /// merely expired (401 `M_UNKNOWN_TOKEN` with `soft_logout: true` — the
    /// device survives and the refresh token is still good) from a genuine
    /// revocation (a hard logout, a destroyed device, a 403). Before this,
    /// both looked identical and both were fatal. Only the first is renewed
    /// here; the second still fails closed, which is the half of #936 that a
    /// refresh path could otherwise paper over.
    pub(crate) async fn renew_after_rejection(
        &self,
        presented: &str,
        cause: &MatrixError,
    ) -> Renewal {
        let label = auth_rejection_label(cause);
        if !is_soft_logout(cause) {
            self.publish_auth_expired(format!(
                "{label} — a hard revocation (no soft_logout), which a refresh token cannot \
                 recover; re-authenticate the bot and run `channel reload`"
            ))
            .await;
            return Renewal::Fatal;
        }
        if !self.can_renew() {
            self.publish_auth_expired(format!(
                "{label} — the homeserver reports soft_logout, so this token is refreshable, \
                 but no refresh token is configured; set `credential_handle_refresh_token` on \
                 this channel and run `channel reload`"
            ))
            .await;
            return Renewal::Fatal;
        }
        self.renew(presented, &label).await
    }

    /// Load → decide → POST → store, all under the cross-process lock.
    async fn renew(&self, presented: &str, why: &str) -> Renewal {
        let Some(refresh_handle) = self.refresh_handle.clone() else {
            return Renewal::Deferred("no refresh token is configured".to_string());
        };

        if self
            .renewals_without_progress
            .fetch_add(1, Ordering::Relaxed)
            >= MAX_RENEWALS_WITHOUT_PROGRESS
        {
            self.publish_auth_expired(format!(
                "{why} — refreshed {MAX_RENEWALS_WITHOUT_PROGRESS} times with no successful \
                 call in between, so the homeserver is rejecting freshly issued tokens; \
                 re-authenticate the bot and run `channel reload`"
            ))
            .await;
            return Renewal::Fatal;
        }

        // The spin inside `acquire` sleeps a real thread and the wait ceiling
        // sits above the POST timeout, so awaiting it inline would park a task
        // worker for tens of seconds — on a small runtime, possibly the very
        // worker that would have driven the POST that releases the lock.
        let path = self.lock_path.clone();
        let acquired = tokio::task::spawn_blocking(move || {
            ExclusiveFileLock::acquire(path, lock_policy(), "matrix token refresh")
        })
        .await;
        // The lock is held for the rest of this function and released by its
        // `Drop` — on every arm below, including the early returns.
        let _lock = match acquired {
            Ok(Ok(lock)) => lock,
            Ok(Err(error)) => {
                return Renewal::Deferred(format!(
                    "another process is refreshing this Matrix token and did not finish in \
                     time; nothing was POSTed ({error})"
                ));
            }
            // A panicked blocking task tells us nothing about the lock, so it
            // is handled exactly like contention: no POST.
            Err(error) => {
                return Renewal::Deferred(format!("refresh lock task failed: {error}"));
            }
        };

        // Double-check under the lock. A peer that refreshed while we queued
        // has already written its new access token, so adopting it costs zero
        // POSTs — and POSTing here would spend a refresh token the peer has
        // already rotated away, which is the grant-revoking replay.
        match self.creds.get(&self.access_handle) {
            Ok(Some(stored)) if stored != presented => {
                self.install(stored, None);
                tracing::info!(
                    target: "wcore_channel_matrix::token",
                    "another process had already refreshed this Matrix token; adopted it without a refresh POST",
                );
                return Renewal::Renewed;
            }
            Ok(_) => {}
            Err(error) => {
                return Renewal::Deferred(format!("credentials lookup: {error}"));
            }
        }

        let refresh_token = match self.creds.get(&refresh_handle) {
            Ok(Some(t)) => t,
            Ok(None) => {
                self.publish_auth_expired(format!(
                    "{why} — no refresh token is stored at {refresh_handle:?}; \
                     re-authenticate the bot and run `channel reload`"
                ))
                .await;
                return Renewal::Fatal;
            }
            Err(error) => {
                return Renewal::Deferred(format!("credentials lookup: {error}"));
            }
        };

        let renewed =
            match rest::refresh_access_token(&self.http, &self.api_base, &refresh_token).await {
                Ok(r) => r,
                // The refresh token ITSELF was refused. That is the genuine
                // revocation: there is no further credential to fall back on.
                Err(
                    e @ MatrixError::Http {
                        status: 401 | 403, ..
                    },
                ) => {
                    let refused = auth_rejection_label(&e);
                    self.publish_auth_expired(format!(
                        "{why}; the refresh token was refused too ({refused}) — \
                     re-authenticate the bot and run `channel reload`"
                    ))
                    .await;
                    return Renewal::Fatal;
                }
                Err(error) => {
                    return Renewal::Deferred(format!("token refresh failed: {error}"));
                }
            };

        // Persist the ROTATED REFRESH TOKEN FIRST, and the access token
        // second. The order is the crash-recovery order, not a style choice:
        //
        // * refresh-then-access, interrupted → the store holds the new refresh
        //   token and a stale access token. The next start 401s, refreshes
        //   with the good refresh token, and recovers.
        // * access-then-refresh, interrupted → the store holds a SPENT refresh
        //   token. The next start's refresh POST replays it, and a spec-
        //   compliant server may revoke the whole grant. Unrecoverable.
        if let Some(rotated) = renewed.refresh_token.as_deref()
            && let Err(error) = self.creds.put(&refresh_handle, rotated)
        {
            // Loud, because the store now holds a SPENT refresh token: this
            // process keeps working on the access token below, but a restart
            // will replay a dead token and need a re-authentication.
            tracing::error!(
                target: "wcore_channel_matrix::token",
                error = %error,
                handle = %refresh_handle,
                "could not persist the rotated Matrix refresh token; the stored one is now spent and a restart will require re-authentication",
            );
        }
        if let Err(error) = self.creds.put(&self.access_handle, &renewed.access_token) {
            tracing::warn!(
                target: "wcore_channel_matrix::token",
                error = %error,
                "could not persist the refreshed Matrix access token; this process holds it in memory only",
            );
        }

        self.install(renewed.access_token, renewed.expires_in_ms);
        Renewal::Renewed
    }

    fn install(&self, access: String, expires_in_ms: Option<u64>) {
        let expires_at = expires_in_ms.map(|ms| Instant::now() + Duration::from_millis(ms));
        let mut guard = self.current.write().unwrap_or_else(|e| e.into_inner());
        *guard = Snapshot { access, expires_at };
    }

    fn read(&self) -> std::sync::RwLockReadGuard<'_, Snapshot> {
        self.current.read().unwrap_or_else(|e| e.into_inner())
    }

    /// Publish the health event, at most once. The manager projects
    /// `AuthExpired` onto `HealthState::Unauthenticated`, so this is what
    /// stops the channel reporting `Healthy` against a dead credential.
    async fn publish_auth_expired(&self, reason: String) {
        if self.reported.swap(true, Ordering::SeqCst) {
            return;
        }
        tracing::error!(
            target: "wcore_channel_matrix::token",
            reason = %reason,
            "the Matrix credential is not recoverable; the channel is unauthenticated",
        );
        // Pushed inline rather than from a spawned task: a caller that breaks
        // its loop the instant this returns must find the event already in the
        // inbox, or the health surface learns about the dead credential only
        // if a detached task happens to win a race.
        self.inbox
            .lock()
            .await
            .push_back(ChannelEvent::AuthExpired { reason });
    }
}

/// The timing contract for the refresh lock.
fn lock_policy() -> LockPolicy {
    LockPolicy::new(
        Duration::from_secs(STALE_AFTER_SECS),
        Duration::from_secs(WAIT_CEILING_SECS),
    )
    .with_heartbeat(Duration::from_secs(HEARTBEAT_SECS))
}

/// Where the cross-process refresh lock for this credential lives.
///
/// Keyed on (homeserver × access-token handle) and NOT on the channel name:
/// two channels configured against the same stored token share one rotating
/// refresh token, so they must serialize against each other. A digest, so no
/// credential handle appears in a filename.
fn refresh_lock_path(api_base: &str, access_handle: &str) -> PathBuf {
    let mut h = std::collections::hash_map::DefaultHasher::new();
    api_base.hash(&mut h);
    access_handle.hash(&mut h);
    let key = h.finish();
    wcore_config::config::wayland_config_dir()
        .join("channel-state")
        .join(format!("matrix-{key:016x}.refresh.lock"))
}

/// Whether this rejection is about the CREDENTIAL, as opposed to the operation.
///
/// The single predicate both the `/sync` loop and the send path gate on, so
/// there is exactly one definition of "the homeserver refused who we are".
///
/// Matrix spends 401 and 403 on two different questions, and the status alone
/// cannot tell them apart:
///
/// * **401** — authentication. The token is not accepted. Always a credential
///   rejection, errcode or not: a bare 401 from a gateway that stripped the
///   Matrix body is still something no retry can fix.
/// * **403 with a token errcode** — a credential rejection too. The spec puts
///   `M_UNKNOWN_TOKEN` on 401, but deployments (and reverse proxies in front of
///   them) do return it on 403, and refusing to renew there would strand a
///   channel that a refresh would have healed.
/// * **403 with anything else** — authorization, NOT authentication.
///   `M_FORBIDDEN` on a redaction means the bot's power level is too low; a
///   bare 403 usually means something in front of the homeserver blocked the
///   request. The token is fine.
///
/// The last row is why this exists. Folding it in publishes `AuthExpired`,
/// which the manager projects onto `HealthState::Unauthenticated` and which
/// `TokenSource` latches so it can never be walked back — so ONE refused
/// redaction would mark the channel permanently unauthenticated while every
/// later send kept succeeding. That is the health surface lying in the
/// direction the operator acts on, and `channel reload` cannot raise a power
/// level.
pub(crate) fn is_credential_rejection(e: &MatrixError) -> bool {
    let MatrixError::Http { status, body } = e else {
        return false;
    };
    match status {
        401 => true,
        403 => matches!(
            errcode_of(body),
            Some("M_UNKNOWN_TOKEN" | "M_MISSING_TOKEN")
        ),
        _ => false,
    }
}

/// The homeserver's `errcode`, when the body is a readable Matrix error.
fn errcode_of(body: &str) -> Option<&str> {
    // Borrowed out of `body` rather than through a `Value`, so the caller can
    // match on `&str` without an allocation per rejection.
    let rest = body.split_once("\"errcode\"")?.1;
    let rest = rest.trim_start().strip_prefix(':')?.trim_start();
    let rest = rest.strip_prefix('"')?;
    let end = rest.find('"')?;
    Some(&rest[..end])
}

/// Whether the homeserver said this rejection is a SOFT logout — the access
/// token is invalid but the device survives, so a refresh token can recover it.
///
/// Absent or `false` means a hard revocation. Defaulting to "refreshable" on a
/// body we cannot read would convert a genuine revocation into an endless
/// refresh loop against a dead grant, so absence fails closed.
fn is_soft_logout(e: &MatrixError) -> bool {
    let MatrixError::Http { status: 401, body } = e else {
        return false;
    };
    serde_json::from_str::<serde_json::Value>(body)
        .ok()
        .as_ref()
        .and_then(|v| v.get("soft_logout"))
        .and_then(|v| v.as_bool())
        .unwrap_or(false)
}

/// A short, SECRET-FREE label for a credential rejection, suitable for the
/// health surface's operator-facing `reason`.
///
/// Only the homeserver's `errcode` — a fixed spec vocabulary such as
/// `M_UNKNOWN_TOKEN` — is surfaced, never the raw response body. The body is an
/// echo of a request we authenticated, so treating it as printable would make
/// the health surface a place a token could appear; `ProbeReport` holds the same
/// line ("the NAME of a rejected item, never its value") and this must not be the
/// weaker of the two. A body we cannot parse yields the status alone.
pub(crate) fn auth_rejection_label(e: &MatrixError) -> String {
    let MatrixError::Http { status, body } = e else {
        return "platform rejected the credential".to_string();
    };
    match serde_json::from_str::<serde_json::Value>(body)
        .ok()
        .as_ref()
        .and_then(|v| v.get("errcode"))
        .and_then(|c| c.as_str())
    {
        Some(errcode) => format!("homeserver rejected the access token: HTTP {status} {errcode}"),
        None => format!("homeserver rejected the access token: HTTP {status}"),
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use std::collections::HashMap;
    use std::sync::Mutex as StdMutex;
    use wcore_config::credentials::CredentialsError;

    /// A token value that must never reach the health surface.
    const CANARY: &str = "syt_CANARY_2f9c41ab7de6_MUSTNOTLEAK";

    /// The crate's one in-memory credentials store for tests. It lives here
    /// because this module is what reads and rotates the pair it holds.
    pub(crate) struct MemCreds {
        inner: StdMutex<HashMap<String, String>>,
    }

    impl MemCreds {
        pub(crate) fn new(pairs: &[(&str, &str)]) -> Arc<Self> {
            let map = pairs
                .iter()
                .map(|(k, v)| (k.to_string(), v.to_string()))
                .collect();
            Arc::new(Self {
                inner: StdMutex::new(map),
            })
        }
        pub(crate) fn with_token(handle: &str, token: &str) -> Arc<dyn CredentialsStore> {
            Self::new(&[(handle, token)])
        }
        pub(crate) fn empty() -> Arc<dyn CredentialsStore> {
            Self::new(&[])
        }
        /// Read a stored value back — how a test proves a ROTATED token was
        /// actually persisted rather than merely received.
        pub(crate) fn peek(&self, key: &str) -> Option<String> {
            self.inner.lock().unwrap().get(key).cloned()
        }
    }

    impl CredentialsStore for MemCreds {
        fn get(&self, key: &str) -> Result<Option<String>, CredentialsError> {
            Ok(self.inner.lock().unwrap().get(key).cloned())
        }
        fn put(&self, key: &str, value: &str) -> Result<(), CredentialsError> {
            self.inner
                .lock()
                .unwrap()
                .insert(key.to_string(), value.to_string());
            Ok(())
        }
        fn delete(&self, key: &str) -> Result<(), CredentialsError> {
            self.inner.lock().unwrap().remove(key);
            Ok(())
        }
    }

    /// A `TokenSource` whose lock lives in a per-test directory, so parallel
    /// tests never serialize against each other or against a real profile.
    /// A source with NO refresh handle: the shape of a channel configured with
    /// a bare access token, which is every pre-#936 config. It can never take
    /// the refresh lock, so its lock path is never created.
    pub(crate) fn plain_source(
        api_base: &str,
        access_token: &str,
        inbox: &Arc<Mutex<VecDeque<ChannelEvent>>>,
    ) -> Arc<TokenSource> {
        Arc::new(source_with_lock_dir(
            MemCreds::empty(),
            None,
            access_token,
            api_base,
            std::path::Path::new("unreachable-no-refresh-handle-is-configured"),
            Arc::clone(inbox),
        ))
    }

    /// A source that CAN refresh, with its lock confined to `lock_dir` so
    /// parallel tests never serialize against each other or a real profile.
    pub(crate) fn refreshing_source(
        api_base: &str,
        access_token: &str,
        creds: Arc<MemCreds>,
        lock_dir: &std::path::Path,
        inbox: &Arc<Mutex<VecDeque<ChannelEvent>>>,
    ) -> Arc<TokenSource> {
        Arc::new(source_with_lock_dir(
            creds,
            Some("matrix.test.refresh"),
            access_token,
            api_base,
            lock_dir,
            Arc::clone(inbox),
        ))
    }

    fn source_with_lock_dir(
        creds: Arc<dyn CredentialsStore>,
        refresh_handle: Option<&str>,
        access_token: &str,
        api_base: &str,
        dir: &std::path::Path,
        inbox: Arc<Mutex<VecDeque<ChannelEvent>>>,
    ) -> TokenSource {
        let mut src = TokenSource::new(TokenSourceParams {
            creds,
            access_handle: "matrix.test.access".to_string(),
            refresh_handle: refresh_handle.map(str::to_string),
            access_token: access_token.to_string(),
            http: wcore_egress::EgressClient::builder()
                .user_agent("wcore-matrix-token-test")
                .build()
                .unwrap_or_default(),
            api_base: api_base.to_string(),
            inbox,
        });
        src.lock_path = dir.join("refresh.lock");
        src
    }

    fn inbox() -> Arc<Mutex<VecDeque<ChannelEvent>>> {
        Arc::new(Mutex::new(VecDeque::new()))
    }

    async fn auth_reasons(inbox: &Arc<Mutex<VecDeque<ChannelEvent>>>) -> Vec<String> {
        inbox
            .lock()
            .await
            .iter()
            .filter_map(|e| match e {
                ChannelEvent::AuthExpired { reason } => Some(reason.clone()),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn soft_logout_is_read_from_the_body_and_absence_fails_closed() {
        let soft = MatrixError::Http {
            status: 401,
            body: r#"{"errcode":"M_UNKNOWN_TOKEN","soft_logout":true}"#.to_string(),
        };
        assert!(is_soft_logout(&soft), "an explicit soft_logout is soft");

        for hard in [
            // A hard logout: the device is gone, no refresh can recover it.
            r#"{"errcode":"M_UNKNOWN_TOKEN"}"#,
            r#"{"errcode":"M_UNKNOWN_TOKEN","soft_logout":false}"#,
            // Unparseable — must NOT be optimistically treated as refreshable.
            "not json at all",
            r#"{"soft_logout":"true"}"#,
        ] {
            assert!(
                !is_soft_logout(&MatrixError::Http {
                    status: 401,
                    body: hard.to_string(),
                }),
                "{hard} must not be classified refreshable",
            );
        }

        // A 403 is never soft, whatever the body claims.
        assert!(
            !is_soft_logout(&MatrixError::Http {
                status: 403,
                body: r#"{"soft_logout":true}"#.to_string(),
            }),
            "a 403 is a hard refusal regardless of the body",
        );
    }

    /// The half of #936 a refresh path could paper over: a genuinely revoked
    /// token must still fail closed, with no refresh POST attempted at all.
    #[tokio::test]
    async fn a_hard_revocation_fails_closed_without_posting() {
        let mut server = mockito::Server::new_async().await;
        let refresh = server
            .mock("POST", "/_matrix/client/v3/refresh")
            .expect(0)
            .create_async()
            .await;
        let dir = tempfile::tempdir().unwrap();
        let inbox = inbox();
        let creds = MemCreds::new(&[
            ("matrix.test.access", CANARY),
            ("matrix.test.refresh", "the-refresh-token"),
        ]);
        let src = source_with_lock_dir(
            creds,
            Some("matrix.test.refresh"),
            CANARY,
            &server.url(),
            dir.path(),
            Arc::clone(&inbox),
        );

        let outcome = src
            .renew_after_rejection(
                CANARY,
                &MatrixError::Http {
                    status: 401,
                    body: r#"{"errcode":"M_UNKNOWN_TOKEN","error":"Token is not active"}"#
                        .to_string(),
                },
            )
            .await;

        assert!(
            matches!(outcome, Renewal::Fatal),
            "a hard revocation must be fatal, got {outcome:?}",
        );
        refresh.assert_async().await;
        let reasons = auth_reasons(&inbox).await;
        assert_eq!(reasons.len(), 1, "expected one AuthExpired: {reasons:?}");
        assert!(
            reasons[0].contains("M_UNKNOWN_TOKEN"),
            "the reason must name the errcode: {:?}",
            reasons[0]
        );
        assert!(
            !reasons[0].contains(CANARY),
            "the health reason leaked the token: {:?}",
            reasons[0]
        );
    }

    /// The positive direction: a soft logout is refreshed, the rotated pair is
    /// persisted, and no `AuthExpired` is published.
    #[tokio::test]
    async fn a_soft_logout_is_refreshed_and_the_rotated_pair_is_persisted() {
        let mut server = mockito::Server::new_async().await;
        let refresh = server
            .mock("POST", "/_matrix/client/v3/refresh")
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(
                r#"{"access_token":"syt_new_access","refresh_token":"rot_new_refresh","expires_in_ms":300000}"#,
            )
            .expect(1)
            .create_async()
            .await;
        let dir = tempfile::tempdir().unwrap();
        let inbox = inbox();
        let creds = MemCreds::new(&[
            ("matrix.test.access", CANARY),
            ("matrix.test.refresh", "rot_old_refresh"),
        ]);
        let src = source_with_lock_dir(
            Arc::clone(&creds) as Arc<dyn CredentialsStore>,
            Some("matrix.test.refresh"),
            CANARY,
            &server.url(),
            dir.path(),
            Arc::clone(&inbox),
        );

        let outcome = src
            .renew_after_rejection(
                CANARY,
                &MatrixError::Http {
                    status: 401,
                    body: r#"{"errcode":"M_UNKNOWN_TOKEN","soft_logout":true}"#.to_string(),
                },
            )
            .await;

        assert!(
            matches!(outcome, Renewal::Renewed),
            "a soft logout with a refresh token must renew, got {outcome:?}",
        );
        refresh.assert_async().await;
        assert_eq!(src.access(), "syt_new_access", "the new token must be live");
        assert_eq!(
            creds.peek("matrix.test.refresh").as_deref(),
            Some("rot_new_refresh"),
            "the ROTATED refresh token must be persisted, or the next process replays a spent one",
        );
        assert_eq!(
            creds.peek("matrix.test.access").as_deref(),
            Some("syt_new_access"),
        );
        assert!(
            auth_reasons(&inbox).await.is_empty(),
            "a recovered credential must not be reported unauthenticated",
        );
        assert!(
            !src.renewal_due(),
            "a 300s expiry is outside the renewal margin",
        );
    }

    /// A refresh token the homeserver refuses is the end of the line: fatal,
    /// and said so on the health surface.
    #[tokio::test]
    async fn a_refused_refresh_token_is_fatal() {
        let mut server = mockito::Server::new_async().await;
        server
            .mock("POST", "/_matrix/client/v3/refresh")
            .with_status(401)
            .with_body(r#"{"errcode":"M_UNKNOWN_TOKEN","error":"Unrecognised refresh token"}"#)
            .create_async()
            .await;
        let dir = tempfile::tempdir().unwrap();
        let inbox = inbox();
        let creds = MemCreds::new(&[
            ("matrix.test.access", CANARY),
            ("matrix.test.refresh", "rot_dead"),
        ]);
        let src = source_with_lock_dir(
            creds,
            Some("matrix.test.refresh"),
            CANARY,
            &server.url(),
            dir.path(),
            Arc::clone(&inbox),
        );

        let outcome = src
            .renew_after_rejection(
                CANARY,
                &MatrixError::Http {
                    status: 401,
                    body: r#"{"errcode":"M_UNKNOWN_TOKEN","soft_logout":true}"#.to_string(),
                },
            )
            .await;
        assert!(matches!(outcome, Renewal::Fatal), "got {outcome:?}");
        let reasons = auth_reasons(&inbox).await;
        assert_eq!(reasons.len(), 1, "{reasons:?}");
        assert!(
            reasons[0].contains("refresh token was refused"),
            "the reason must distinguish a dead refresh token: {:?}",
            reasons[0]
        );
    }

    /// A network failure on the refresh endpoint is NOT a credential verdict.
    /// Without this split a blip would report `Unauthenticated` and send an
    /// operator to re-authenticate a perfectly good bot.
    #[tokio::test]
    async fn a_transient_refresh_failure_is_deferred_not_fatal() {
        let dir = tempfile::tempdir().unwrap();
        let inbox = inbox();
        let creds = MemCreds::new(&[
            ("matrix.test.access", CANARY),
            ("matrix.test.refresh", "rot_ok"),
        ]);
        let src = source_with_lock_dir(
            creds,
            Some("matrix.test.refresh"),
            CANARY,
            // A port nothing is listening on: a connection error, not an HTTP
            // status.
            "http://127.0.0.1:1",
            dir.path(),
            Arc::clone(&inbox),
        );

        let outcome = src
            .renew_after_rejection(
                CANARY,
                &MatrixError::Http {
                    status: 401,
                    body: r#"{"errcode":"M_UNKNOWN_TOKEN","soft_logout":true}"#.to_string(),
                },
            )
            .await;
        assert!(
            matches!(outcome, Renewal::Deferred(_)),
            "a network fault must defer, not accuse the credential: {outcome:?}",
        );
        assert!(
            auth_reasons(&inbox).await.is_empty(),
            "a transient fault must not report the channel unauthenticated",
        );
    }

    /// The lock path is scoped to the credential, not to the channel, and
    /// carries no cleartext handle.
    #[test]
    fn the_lock_is_credential_scoped_and_names_no_handle() {
        let a = refresh_lock_path("https://matrix.org", "matrix.prod.token");
        assert_eq!(
            a,
            refresh_lock_path("https://matrix.org", "matrix.prod.token")
        );
        assert_ne!(
            a,
            refresh_lock_path("https://other.org", "matrix.prod.token"),
            "a different homeserver is a different grant",
        );
        assert_ne!(
            a,
            refresh_lock_path("https://matrix.org", "matrix.staging.token"),
            "a different stored token must not serialize against this one",
        );
        let name = a.file_name().unwrap().to_string_lossy().to_string();
        assert!(
            !name.contains("matrix.prod.token"),
            "the lock name must be a digest, not the credential handle: {name}"
        );
    }

    /// An unbounded refresh loop is a self-inflicted denial of service AND
    /// burns a rotating token on every pass. After the cap the credential is
    /// declared dead instead.
    #[tokio::test]
    async fn repeated_renewals_without_progress_stop_instead_of_looping() {
        let mut server = mockito::Server::new_async().await;
        // Always succeeds — so nothing but the cap can stop the loop.
        server
            .mock("POST", "/_matrix/client/v3/refresh")
            .with_status(200)
            .with_body(r#"{"access_token":"syt_again","refresh_token":"rot_again"}"#)
            .create_async()
            .await;
        let dir = tempfile::tempdir().unwrap();
        let inbox = inbox();
        let creds = MemCreds::new(&[
            ("matrix.test.access", CANARY),
            ("matrix.test.refresh", "rot_ok"),
        ]);
        let src = source_with_lock_dir(
            creds,
            Some("matrix.test.refresh"),
            CANARY,
            &server.url(),
            dir.path(),
            Arc::clone(&inbox),
        );

        let soft = MatrixError::Http {
            status: 401,
            body: r#"{"errcode":"M_UNKNOWN_TOKEN","soft_logout":true}"#.to_string(),
        };
        let mut outcomes = Vec::new();
        for _ in 0..6 {
            let presented = src.access();
            outcomes.push(src.renew_after_rejection(&presented, &soft).await);
        }
        assert!(
            outcomes.iter().any(|o| matches!(o, Renewal::Fatal)),
            "the renewal loop must terminate: {outcomes:?}",
        );
        let fatal_at = outcomes
            .iter()
            .position(|o| matches!(o, Renewal::Fatal))
            .unwrap();
        assert!(
            fatal_at <= MAX_RENEWALS_WITHOUT_PROGRESS as usize,
            "gave up after {fatal_at} renewals, cap is {MAX_RENEWALS_WITHOUT_PROGRESS}",
        );

        // And a successful call in between resets it, so a channel that is
        // genuinely working is never capped out.
        src.mark_progress();
        assert_eq!(
            src.renewals_without_progress.load(Ordering::Relaxed),
            0,
            "progress must clear the guard",
        );
    }

    /// The label is the string an operator reads on the health surface. It must
    /// carry the errcode and never the response body, which is an echo of a
    /// request we authenticated.
    #[test]
    fn the_auth_label_names_the_errcode_and_never_the_body() {
        let label = auth_rejection_label(&MatrixError::Http {
            status: 401,
            body: format!(r#"{{"errcode":"M_UNKNOWN_TOKEN","error":"{CANARY}"}}"#),
        });
        assert!(label.contains("M_UNKNOWN_TOKEN"), "got {label:?}");
        assert!(label.contains("401"), "got {label:?}");
        assert!(
            !label.contains(CANARY),
            "the label echoed the response body verbatim: {label:?}"
        );

        // An unparseable body must degrade to the status alone, never to the
        // raw bytes.
        let label = auth_rejection_label(&MatrixError::Http {
            status: 403,
            body: CANARY.to_string(),
        });
        assert!(label.contains("403"), "got {label:?}");
        assert!(
            !label.contains(CANARY),
            "an unparseable body must not be echoed: {label:?}"
        );
    }
}
