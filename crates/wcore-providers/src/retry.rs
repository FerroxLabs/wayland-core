use std::cell::RefCell;
use std::future::Future;
use std::sync::Arc;
use std::time::Duration;

use wcore_egress::{EgressError, EgressRequestBuilder};

use super::ProviderError;
use crate::attempt_lifecycle::{
    ProviderAttemptHeaderOutcome, ProviderAttemptNotStartedReason, begin_physical_attempt,
    finish_physical_attempt, start_physical_attempt,
};

/// Default retry policy for provider HTTP calls: 3 attempts, 250 ms → 1 s → 4 s.
pub const DEFAULT_MAX_RETRIES: u32 = 2; // 1 initial + 2 retries = 3 total attempts
pub const INITIAL_BACKOFF: Duration = Duration::from_millis(250);

/// Wall-clock window over which a connection that was established and then
/// destroyed before any response head arrived (peer reset / abort / broken
/// pipe) keeps being re-sent.
///
/// A request COUNT is the wrong unit. What is being ridden out is an interval
/// during which the peer will not complete a request, and an interval has a
/// duration; how many sends fit inside it is an artefact of how fast each one
/// fails, not a property of the failure. Any count is therefore a guess about
/// the shape of the next outage.
///
/// This is the INNER of two bounds and owns only the smallest failure: a
/// single physical send losing its socket — a proxy recycling a worker, a load
/// balancer dropping connections through a rollover, a keep-alive raced to
/// close. The window has to contain at least one full re-establishment of the
/// connection, and the product already states how long that may take:
/// [`crate::http_client::CONNECT_TIMEOUT`] is 30 s. Anything longer than one
/// re-establishment is not a blip and belongs to the OUTER bound — the
/// engine's per-turn unserved-request budget, which is an order of magnitude
/// longer and rebuilds the whole request.
///
/// Holding the window open is cheap: nothing was served on these attempts and
/// nothing was billed, so a re-send costs a socket rather than tokens.
pub const BROKEN_CONNECTION_RETRY_WINDOW: Duration = Duration::from_secs(30);

/// One physical provider HTTP attempt observed by the retry ring.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProviderAttemptEvidence {
    /// True for a physical HTTP send; false for a retry decision made by a
    /// provider wrapper after that send completed.
    pub physical: bool,
    /// Stable machine-readable failure class, absent on a successful response.
    pub failure: Option<String>,
    /// Whether Core immediately scheduled another physical attempt.
    pub retrying: bool,
}

tokio::task_local! {
    static ATTEMPT_EVIDENCE: RefCell<Vec<ProviderAttemptEvidence>>;
    static ATTEMPT_OBSERVER: Option<Arc<dyn Fn(ProviderAttemptEvidence) + Send + Sync>>;
    static MAX_RETRIES_OVERRIDE: u32;
    static CONFIGURED_FALLBACK_ADMITTER: Option<ConfiguredFallbackAdmitter>;
}

pub type ProviderAttemptObserver = Arc<dyn Fn(ProviderAttemptEvidence) + Send + Sync>;
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ConfiguredFallbackAdmission {
    pub estimated_microcents: Option<u64>,
}

pub type ConfiguredFallbackAdmitter = Arc<
    dyn Fn(&str, &str, &str, &str, bool) -> Result<ConfiguredFallbackAdmission, ProviderError>
        + Send
        + Sync,
>;

/// Run `future` with a task-local ceiling on provider retry counts.
///
/// The ceiling applies to both generic provider retries and physical HTTP
/// retries. Setting it to zero permits exactly one attempt. The scope is
/// task-local, so concurrent provider calls retain their own retry policy.
pub async fn scope_max_retries<F>(max_retries: u32, future: F) -> F::Output
where
    F: Future,
{
    let max_retries = effective_max_retries(max_retries);
    MAX_RETRIES_OVERRIDE.scope(max_retries, future).await
}

/// Whether the current task-local scope forbids provider-local retry sends.
///
/// Provider implementations with manual HTTP/auth/capability retry sends must
/// consult this in addition to using [`with_retry`] or
/// [`builder_send_with_retry`]. Configured provider-chain fallback is admitted
/// separately through [`admit_configured_fallback`].
pub fn retries_disabled() -> bool {
    MAX_RETRIES_OVERRIDE
        .try_with(|max_retries| *max_retries == 0)
        .unwrap_or(false)
}

/// Run `future` with a task-local admission hook for configured provider
/// fallback. The hook runs synchronously immediately before every fallback
/// provider is dispatched.
pub async fn scope_configured_fallback_admitter<F>(
    admitter: ConfiguredFallbackAdmitter,
    future: F,
) -> F::Output
where
    F: Future,
{
    CONFIGURED_FALLBACK_ADMITTER
        .scope(Some(admitter), future)
        .await
}

/// Admit one configured fallback provider before it is dispatched.
///
/// `previous_attempted` is false only when the previous provider was skipped
/// without a physical send (for example because its circuit was already
/// open). Without an installed hook, configured fallback remains enabled.
pub fn admit_configured_fallback(
    previous_provider: &str,
    next_label: &str,
    next_provider: &str,
    next_model: &str,
    previous_attempted: bool,
) -> Result<ConfiguredFallbackAdmission, ProviderError> {
    CONFIGURED_FALLBACK_ADMITTER
        .try_with(|admitter| {
            admitter
                .as_ref()
                .map_or(Ok(ConfiguredFallbackAdmission::default()), |admitter| {
                    admitter(
                        previous_provider,
                        next_label,
                        next_provider,
                        next_model,
                        previous_attempted,
                    )
                })
        })
        .unwrap_or(Ok(ConfiguredFallbackAdmission::default()))
}

/// Whether a provider error proves no paid request could have been sent.
/// Keep this deliberately narrow: transport failures and HTTP responses are
/// ambiguous and therefore remain conservatively chargeable.
pub(crate) fn configured_fallback_previous_attempted(error: &ProviderError) -> bool {
    !error.was_not_attempted()
}

fn effective_max_retries(configured: u32) -> u32 {
    MAX_RETRIES_OVERRIDE
        .try_with(|max_retries| configured.min(*max_retries))
        .unwrap_or(configured)
}

/// Capture physical HTTP attempts made while `future` is running.
///
/// The scope is per task and per provider call, so concurrent agents cannot
/// leak evidence into one another. Providers that do not use this retry ring
/// simply return an empty evidence vector.
pub async fn capture_provider_attempts<F>(future: F) -> (F::Output, Vec<ProviderAttemptEvidence>)
where
    F: Future,
{
    ATTEMPT_OBSERVER
        .scope(
            None,
            ATTEMPT_EVIDENCE.scope(RefCell::new(Vec::new()), async move {
                let output = future.await;
                let evidence = ATTEMPT_EVIDENCE.with(|slot| slot.take());
                (output, evidence)
            }),
        )
        .await
}

/// Observe attempts and retry decisions synchronously while `future` runs.
/// Evidence already emitted by the callback survives cancellation of the
/// provider future; the scope and collector are still isolated per task.
pub async fn observe_provider_attempts<F>(observer: ProviderAttemptObserver, future: F) -> F::Output
where
    F: Future,
{
    ATTEMPT_OBSERVER
        .scope(
            Some(observer),
            ATTEMPT_EVIDENCE.scope(RefCell::new(Vec::new()), future),
        )
        .await
}

/// Clone the observer currently attached to this provider call so a spawned
/// response-body task can preserve the same evidence scope.
pub fn current_attempt_observer() -> Option<ProviderAttemptObserver> {
    ATTEMPT_OBSERVER
        .try_with(|observer| observer.clone())
        .ok()
        .flatten()
}

/// Run a spawned response-body future under an observer cloned from its
/// originating provider call.
pub async fn scope_attempt_observer<F>(observer: ProviderAttemptObserver, future: F) -> F::Output
where
    F: Future,
{
    ATTEMPT_OBSERVER.scope(Some(observer), future).await
}

/// Report a typed provider failure discovered after the physical response
/// started (for example an SSE stream that closed before its terminal frame).
pub fn record_provider_failure(failure: impl Into<String>) {
    let evidence = ProviderAttemptEvidence {
        physical: false,
        failure: Some(failure.into()),
        retrying: false,
    };
    let _ = ATTEMPT_OBSERVER.try_with(|observer| {
        if let Some(observer) = observer {
            observer(evidence);
        }
    });
}

fn record_not_attempted(failure: impl Into<String>) {
    let evidence = ProviderAttemptEvidence {
        physical: false,
        failure: Some(failure.into()),
        retrying: false,
    };
    let _ = ATTEMPT_OBSERVER.try_with(|observer| {
        if let Some(observer) = observer {
            observer(evidence);
        }
    });
}

fn record_attempt(failure: Option<String>, retrying: bool) {
    let evidence = ProviderAttemptEvidence {
        physical: true,
        failure,
        retrying,
    };
    let _ = ATTEMPT_EVIDENCE.try_with(|slot| slot.borrow_mut().push(evidence.clone()));
    let _ = ATTEMPT_OBSERVER.try_with(|observer| {
        if let Some(observer) = observer {
            observer(evidence);
        }
    });
}

/// Mark the most recently recorded physical attempt as followed by a
/// provider-level retry outside `builder_send_with_retry` (for example a
/// capability fallback or alternate authentication host).
pub fn mark_last_attempt_retrying() {
    let _ = ATTEMPT_EVIDENCE.try_with(|slot| {
        if let Some(last) = slot.borrow_mut().last_mut() {
            last.retrying = true;
            let decision = ProviderAttemptEvidence {
                physical: false,
                failure: last.failure.clone(),
                retrying: true,
            };
            let _ = ATTEMPT_OBSERVER.try_with(|observer| {
                if let Some(observer) = observer {
                    observer(decision);
                }
            });
        }
    });
}

/// Failure code for a connect-phase failure whose cause proves the configured
/// endpoint cannot be reached, however long the caller keeps trying: the host
/// name does not exist.
pub const FAILURE_DNS: &str = "dns_failure";

/// Failure code for a connect-phase failure where the host resolved and the
/// port actively rejected the connection.
pub const FAILURE_CONNECTION_REFUSED: &str = "connection_refused";

/// Failure code for every other connect-phase failure — the ambiguous residue
/// (network unreachable, connect reset, a resolver that answered "try again").
pub const FAILURE_CONNECTION: &str = "connection";

/// Split a connect-phase failure by whether another send can plausibly change
/// the outcome.
///
/// `reqwest::Error::is_connect()` collapses three very different events into
/// one code. A name that does not resolve is a PERMANENT property of the
/// configuration — one typo in `base_url` and no number of re-sends will ever
/// reach a different answer. A port that refuses is a property of the peer
/// right now. A network that is momentarily unreachable is a property of the
/// link. Only the last two can heal inside one turn.
///
/// That distinction was not available above this function, and the engine's
/// unserved-outage window (`wcore_agent`'s `UNSERVED_OUTAGE_BUDGET`) admitted
/// all three: a `base_url` typo cost a MEASURED 902 s and 36 sends before the
/// run gave up, which a user cannot tell apart from a hang.
///
/// Classified from the error's own source chain rather than its `Display`,
/// because the top-level text is the same `error sending request` for a DNS
/// failure and for a peer that reset an established socket — measured on this
/// tree, not assumed. The chain is only READ here; nothing from it is stored
/// or surfaced, so the H-2 URL-stripping guarantee above is untouched.
///
/// Conservative in the same direction as [`is_http_4xx_error`]: an unrecognised
/// chain returns [`FAILURE_CONNECTION`] and keeps the existing generous budget.
/// The cost of a missed permanent failure is the old behaviour; the cost of a
/// false positive is a transient outage that no longer heals.
fn connect_failure_code(error: &reqwest::Error) -> &'static str {
    let mut refused = false;
    let mut chain = Vec::new();
    let mut cursor: Option<&(dyn std::error::Error + 'static)> = Some(error);
    // Bounded so a self-referential chain (a `source()` cycle through a shared
    // error) cannot spin here.
    for _ in 0..16 {
        let Some(current) = cursor else { break };
        if let Some(io) = current.downcast_ref::<std::io::Error>()
            && io.kind() == std::io::ErrorKind::ConnectionRefused
        {
            refused = true;
        }
        chain.push(current.to_string());
        cursor = current.source();
    }
    classify_connect_chain(refused, &chain)
}

/// The decision half of [`connect_failure_code`], split out so it can be
/// tested against the chains reqwest really produces.
///
/// The two shapes below were MEASURED on this tree (see
/// `tests/connect_failure_classification.rs`), not imagined:
///
/// ```text
/// dns:     error sending request … / client error (Connect) / dns error /
///          failed to lookup address information: Name or service not known
/// refused: error sending request … / client error (Connect) / tcp connect error /
///          Connection refused (os error 111)        [io kind ConnectionRefused]
/// ```
///
/// `chain_text[0]` is the top-level `Display`, which still carries the request
/// URL. It is matched against and then dropped — nothing here is stored or
/// surfaced, so the H-2 URL-stripping guarantee is untouched.
fn classify_connect_chain(refused_io_kind: bool, chain_text: &[String]) -> &'static str {
    let mut refused = refused_io_kind;
    let mut name_lookup_failed = false;
    let mut name_lookup_may_heal = false;
    for text in chain_text {
        let text = text.to_ascii_lowercase();
        // hyper-util's connector labels the resolver leg `dns error`; std's
        // `getaddrinfo` wrapper contributes the `failed to lookup address
        // information: <gai_strerror>` detail underneath it.
        if text.contains("dns error") || text.contains("failed to lookup address information") {
            name_lookup_failed = true;
        }
        // Windows. The markers above are Unix text: `with_transport_cause`
        // keeps only the INNERMOST link, and on Windows that is the bare OS
        // error — MEASURED on this tree as `No such host is known. (os error
        // 11001)`, with the `dns error` label sitting one link further out
        // where nothing reads it. Matched on the numeric suffix, which Rust
        // appends itself and is therefore locale-invariant, unlike the
        // FormatMessage prose in front of it.
        //   11001 WSAHOST_NOT_FOUND, 11004 WSANO_DATA — the name is absent.
        if text.contains("(os error 11001)") || text.contains("(os error 11004)") {
            name_lookup_failed = true;
        }
        // EAI_AGAIN — the RESOLVER was unavailable, not the name absent. That
        // is the transient case (a laptop between networks, a container whose
        // DNS is not up yet) and it must keep the full outage budget.
        // 11002 WSATRY_AGAIN is the Windows spelling of the same event.
        if text.contains("temporary failure in name resolution")
            || text.contains("try again")
            || text.contains("(os error 11002)")
        {
            name_lookup_may_heal = true;
        }
        // Text fallback for the refusal. The io ERROR KIND above is the
        // primary signal and is locale-proof; this catches the path where all
        // that survives is a rendered message (see `provider_failure_code`'s
        // `Connection` arm, which has only a `String`).
        // 10061 WSAECONNREFUSED is the Windows spelling; its prose ("the
        // target machine actively refused it") shares no substring with the
        // Unix message, and the io ERROR KIND is unavailable on the
        // `ProviderError::Connection` path, which has only a rendered string.
        if text.contains("connection refused") || text.contains("(os error 10061)") {
            refused = true;
        }
    }
    if name_lookup_failed && !name_lookup_may_heal {
        return FAILURE_DNS;
    }
    if refused {
        return FAILURE_CONNECTION_REFUSED;
    }
    FAILURE_CONNECTION
}

/// Whether a RENDERED transport failure describes an expired deadline.
///
/// Only [`ProviderError::Connection`] needs this. Everywhere else the live
/// `reqwest::Error` is still in hand and `is_timeout()` answers directly
/// (see [`egress_failure_code`] and the `Http` arms of
/// [`provider_failure_code`]); on the `Connection(String)` arm the error is
/// gone and its rendering is all that survives.
///
/// The `timed out` literal alone was not enough, because ONE failure renders
/// TWO ways. MEASURED on this tree (300 blackholed connects through a 50 ms
/// `connect_timeout`, hetzner-dsm, 2026-08-22), both with `is_timeout()=true`
/// and `is_connect()=true`:
///
/// ```text
/// 281/300 (93.7%)  "error sending request: deadline has elapsed"
///  19/300 ( 6.3%)  "error sending request: operation timed out"
/// ```
///
/// The first rendering carries no `timed out` anywhere, fell through to
/// [`classify_connect_chain`], and came back [`FAILURE_CONNECTION`] — one of
/// the four codes `wcore_agent`'s `is_unserved_request_failure` admits into
/// its 900 s unserved-outage budget. A `base_url` typo therefore bought the
/// full outage window about fifteen times in sixteen, which is the measured
/// 902 s hang; the other one in sixteen failed fast. Same request, same
/// failure, different budget — the intermittency is why a single-sample probe
/// can conclude the classifier is healthy.
///
/// Deliberately NOT a general "does this look slow" match: only spellings
/// that name an expired deadline are admitted, so a refusal, an absent name
/// or an unreachable network keeps its own class (pinned by
/// `widening_the_timeout_class_must_not_swallow_other_connect_failures`).
fn is_timeout_rendering(message: &str) -> bool {
    let text = message.to_ascii_lowercase();
    // `operation timed out` (reqwest), `Connection timed out (os error 110)`
    // (ETIMEDOUT), and `timed out reading response` all share this substring.
    text.contains("timed out")
        // tokio's `Elapsed`, surfaced through hyper-util's connect leg. The
        // dominant rendering above, and the one the old guard missed.
        || text.contains("deadline has elapsed")
        // Windows. WSAETIMEDOUT's prose ("the connection attempt failed
        // because the connected party did not properly respond") shares no
        // substring with the Unix text, so match the numeric suffix Rust
        // appends itself, which is locale-invariant -- the same technique
        // `classify_connect_chain` already uses for 10061 / 11001 / 11002.
        || text.contains("(os error 10060)")
}

fn egress_failure_code(error: &EgressError) -> &'static str {
    match error {
        EgressError::Transport(error) if error.is_timeout() => "timeout",
        EgressError::Transport(error) if error.is_connect() => connect_failure_code(error),
        EgressError::Transport(error) if error.is_body() || error.is_decode() => "stream_body",
        EgressError::Transport(_) => "transport",
        EgressError::Denied(_) => "egress_denied",
        EgressError::BeforeDispatch(_) => "provider_before_dispatch_failed",
        EgressError::BodyTooLarge { .. } => "response_body_too_large",
    }
}

fn provider_not_started_reason(error: &EgressError) -> ProviderAttemptNotStartedReason {
    match error {
        EgressError::Denied(reason) => ProviderAttemptNotStartedReason::EgressDenied {
            reason: reason.clone(),
        },
        EgressError::BeforeDispatch(error) => {
            ProviderAttemptNotStartedReason::BeforeDispatchFailed {
                error: error.to_string(),
            }
        }
        other => ProviderAttemptNotStartedReason::BeforeDispatchFailed {
            error: format!(
                "unexpected pre-dispatch outcome: {}",
                egress_failure_code(other)
            ),
        },
    }
}

/// Stable machine-readable class for a provider error observed above the HTTP
/// retry ring. Unlike `Display`, this is safe to aggregate in receipts.
pub fn provider_failure_code(error: &ProviderError) -> String {
    match error {
        ProviderError::Http(error) if error.is_timeout() => "timeout".to_string(),
        ProviderError::Http(error) if error.is_connect() => connect_failure_code(error).to_string(),
        ProviderError::Http(_) => "http_transport".to_string(),
        ProviderError::Egress(error) => egress_failure_code(error).to_string(),
        ProviderError::Api { status, .. } => format!("http_{status}"),
        ProviderError::Parse(_) => "provider_parse".to_string(),
        ProviderError::RateLimited { .. } => "http_429".to_string(),
        ProviderError::PromptTooLong(_) => "prompt_too_long".to_string(),
        ProviderError::Connection(message) if is_timeout_rendering(message) => {
            "timeout".to_string()
        }
        // Only a rendered message survives here; classify what it says. See
        // `with_transport_cause` for why the message carries enough to do so.
        ProviderError::Connection(message) => {
            classify_connect_chain(false, std::slice::from_ref(message)).to_string()
        }
        ProviderError::MissingApiKey => "missing_api_key".to_string(),
        ProviderError::NotAttempted { .. } => "provider_not_attempted".to_string(),
        ProviderError::PremiumLocked { .. } => "premium_locked".to_string(),
        ProviderError::UpgradeRequired { .. } => "upgrade_required".to_string(),
        ProviderError::SpendCeilingUnresolved { .. } => "spend_ceiling_unresolved".to_string(),
        ProviderError::ContextOverflow { .. } => "context_overflow".to_string(),
    }
}

/// Retry a fallible async operation with exponential backoff.
///
/// Retries errors where [`ProviderError::is_retryable`] is true
/// (`RateLimited`, `Connection`, and transient HTTP 5xx / 408 `Api`
/// errors). Non-retryable errors (API 4xx auth/validation, parse
/// failures, prompt-too-long) are returned immediately.
pub async fn with_retry<F, Fut, T>(max_retries: u32, f: F) -> Result<T, ProviderError>
where
    F: Fn() -> Fut,
    Fut: Future<Output = Result<T, ProviderError>>,
{
    let max_retries = effective_max_retries(max_retries);
    let mut backoff = INITIAL_BACKOFF;
    for attempt in 0..=max_retries {
        match f().await {
            Ok(val) => return Ok(val),
            Err(e) if e.is_retryable() && attempt < max_retries => {
                // M3 fix: print 1-based attempt over total-attempt count
                // (`max_retries + 1`), not over the retry count. The loop
                // runs `0..=max_retries`, so 3 attempts when max_retries=2.
                tracing::warn!(
                    attempt = attempt + 1,
                    total = max_retries + 1,
                    error = %e,
                    "provider call failed; retrying"
                );
                // AF3 Risk 2: honour the server's retry-after hint on 429s instead
                // of the exponential backoff schedule.  Cap at 60 s to guard against
                // unreasonably large server hints.
                //
                // NOTE on the 60s cap vs `RETRY_AFTER_CAP_MS` (5 min) in the extractor:
                // the extractor's larger ceiling is for logging/scheduling — recording
                // what the server asked for. This loop caps the *actual* sleep at 60s
                // because a retry that would block more than a minute should fail-fast
                // instead, surfacing the rate-limit upstream where the caller can pick
                // a fallback provider or back off itself.
                let sleep_ms = if let ProviderError::RateLimited { retry_after_ms } = &e {
                    (*retry_after_ms).min(60_000)
                } else {
                    backoff.as_millis() as u64
                };
                tokio::time::sleep(Duration::from_millis(sleep_ms)).await;
                backoff = (backoff * 4).min(Duration::from_secs(4));
            }
            Err(e) => return Err(e),
        }
    }
    // Wave RB STABILITY MINOR #12: replaced `unreachable!()` with an
    // explicit typed error. The match above provably covers every loop
    // iteration (Ok returns; retryable Err continues iff
    // attempt < max_retries; any other Err returns; the final iteration
    // sets attempt == max_retries which fails the guard and falls into
    // the third arm). A future refactor that breaks this invariant will
    // now surface as a normal error path instead of a process panic
    // with "internal error: entered unreachable code".
    Err(ProviderError::Connection(
        "retry policy reached the post-loop arm — this should be impossible; \
         the loop is provably bounded by max_retries"
            .into(),
    ))
}

/// Send a `reqwest::RequestBuilder` with the standard provider retry policy.
///
/// `reqwest::RequestBuilder` is not `Clone`, so callers pass a factory `F`
/// that builds and sends the request each time. Transient connection-level
/// reqwest errors (`is_timeout()`, `is_connect()`) are mapped to
/// [`ProviderError::Connection`] so they satisfy `is_retryable()` and the
/// loop retries them. Body/decode errors (`is_body()`/`is_decode()`, i.e.
/// "error decoding response body" from a stale pooled socket) are also treated
/// as transient. `is_request()` is intentionally excluded: it covers
/// non-transient client-side errors (invalid URL, invalid header value) that
/// will always fail and must not be retried. Remaining reqwest errors
/// (redirect loops, status) fall through as `ProviderError::Http` and are
/// returned immediately.
pub async fn send_with_retry<F, Fut>(f: F) -> Result<reqwest::Response, ProviderError>
where
    F: Fn() -> Fut,
    Fut: Future<Output = Result<reqwest::Response, reqwest::Error>>,
{
    with_retry(DEFAULT_MAX_RETRIES, || async {
        f().await.map_err(provider_error_from_reqwest)
    })
    .await
}

/// Map a `reqwest::Error` to a `ProviderError`, stripping the URL first.
///
/// H-2 / secrets-26: reqwest's `Display` appends ` for url (<URL>)` and a
/// provider that puts a credential in the URL (e.g. Gemini's old `?key=`
/// query form) would leak it into `ProviderError::Connection(e.to_string())`,
/// into `ProviderError::Http`'s `Display`, into the `[retry]` tracing warning, and
/// into the propagated `LlmEvent::Error`. `without_url()` removes the URL from
/// the error before it is ever formatted or stored. Timeout/connect errors map
/// to the retryable `Connection` variant; everything else is `Http`.
fn provider_error_from_reqwest(e: reqwest::Error) -> ProviderError {
    // `is_body()`/`is_decode()` cover "error decoding response body" — almost
    // always a half-closed pooled connection dropped mid-body under bursty
    // load, which is transient and succeeds on a fresh connection. Treat them
    // as retryable alongside timeout/connect. A bare `is_request()` stays
    // excluded (invalid URL/header — permanent, must not retry); only the
    // request-phase errors that carry a transport I/O cause are admitted, via
    // `is_broken_established_connection`.
    let is_transient = e.is_timeout()
        || e.is_connect()
        || e.is_body()
        || e.is_decode()
        || is_broken_established_connection(&e);
    // Read the endpoint BEFORE `without_url()` takes it away. Only the
    // transport classes below can be attributed to an endpoint at all; a
    // decode failure names a body, not a socket.
    let endpoint = (e.is_connect() || e.is_timeout())
        .then(|| e.url().and_then(dialled_endpoint))
        .flatten();
    let e = e.without_url();
    if is_transient {
        ProviderError::Connection(with_endpoint(with_transport_cause(&e), endpoint))
    } else {
        ProviderError::Http(e)
    }
}

/// The `host:port` a request was actually dialled at, or `None` when the URL
/// carries no host (a `data:`/`file:` URL cannot be a provider endpoint).
///
/// #1077: every connect-phase failure rendered the same sentence — `error
/// sending request: Connection refused (os error 111)` for a wrong port, and
/// `error sending request: failed to lookup address information` for a wrong
/// host — and neither named the endpoint. For a `base_url` typo, which is the
/// reported cause, the endpoint IS the diagnosis.
///
/// H-2 / secrets-26: `provider_error_from_reqwest` strips the URL because a
/// provider may put a credential in it (Gemini's old `?key=` query form).
/// This puts back the two components of the authority that provably cannot
/// hold one — `Url::host_str` excludes userinfo, and a port is a number — and
/// nothing else. Pinned by
/// `naming_the_endpoint_must_not_reintroduce_the_url`, which drives a URL
/// carrying userinfo, a path and a `?key=` secret and requires every one of
/// them to stay out.
///
/// The port is resolved rather than echoed, so an implicit `https://host/v1`
/// still reports the 443 it dialled instead of leaving the operator to assume
/// it.
fn dialled_endpoint(url: &reqwest::Url) -> Option<String> {
    let host = url.host_str()?;
    match url.port_or_known_default() {
        Some(port) => Some(format!("{host}:{port}")),
        None => Some(host.to_string()),
    }
}

/// Append the dialled endpoint to a rendered transport failure.
///
/// APPENDED, not substituted: the OS cause says WHAT happened and is what
/// `provider_failure_code` classifies on, so replacing the message with a
/// friendlier sentence would both lose the diagnosis and re-merge the failure
/// classes that `classify_connect_chain` exists to keep apart.
///
/// The appended text is user-supplied — a `base_url` is configuration — and
/// it becomes input to that same substring classifier, so it must not be able
/// to move a failure between classes. It cannot: every marker
/// `classify_connect_chain` and [`is_timeout_rendering`] match on either
/// contains a space (`dns error`, `connection refused`, `timed out`, `try
/// again`, `deadline has elapsed`) or the literal `(os error `, and a host and
/// a port number can contain neither. Pinned by
/// `the_endpoint_text_cannot_change_the_failure_class`.
fn with_endpoint(message: String, endpoint: Option<String>) -> String {
    match endpoint {
        Some(endpoint) => format!("{message} (endpoint {endpoint})"),
        None => message,
    }
}

/// Render a transport error together with its innermost cause.
///
/// Two reasons, and the first is the user-facing one. `reqwest`'s own
/// `Display` for every connect-phase failure is the bare `error sending
/// request` — measured identical for a host that does not exist and for a peer
/// that reset an established socket — so the message the product printed
/// during a 902 s retry storm told the operator nothing at all about why.
///
/// Second, it keeps the two failure-code paths in agreement.
/// [`egress_failure_code`] classifies from the live source chain, but
/// [`ProviderError::Connection`] carries only a `String`, so
/// [`provider_failure_code`] can only classify what the string says. Without
/// the cause, the same failure gets `dns_failure` down one path and
/// `connection` down the other, and only one of them fails fast.
///
/// H-2: the cause is the innermost link, which for this class is an OS error
/// (`getaddrinfo` / `connect(2)`) and cannot carry a URL. A link that
/// nonetheless looks like one is dropped rather than trusted.
fn with_transport_cause(error: &reqwest::Error) -> String {
    let base = error.to_string();
    let mut innermost: Option<String> = None;
    let mut cursor = std::error::Error::source(error);
    for _ in 0..16 {
        let Some(current) = cursor else { break };
        innermost = Some(current.to_string());
        cursor = current.source();
    }
    match innermost {
        Some(cause) if !cause.contains("://") && !base.contains(&cause) => {
            format!("{base}: {cause}")
        }
        _ => base,
    }
}

/// True when a reqwest error means the request was dispatched onto an
/// established connection and the transport then failed before a response
/// head arrived.
///
/// `reqwest::Error::is_connect()` covers only a failure to ESTABLISH the
/// connection. A peer that accepts the request and then destroys the socket —
/// with a TCP RST, or with an orderly close after hanging — reports neither
/// `is_connect()` nor `is_timeout()` nor `is_body()` nor `is_decode()`: it
/// reports `kind: Request`. Job corpus row B-2 measured the consequence twice.
/// `fault-reset` broke one request mid-task and `fault-timeout` hung one and
/// then closed it; both were classified as the terminal `ProviderError::Http`,
/// neither cost a single retry, and both runs exited 1 with the month-end
/// report unwritten.
///
/// `is_request()` is the base signal, and the exclusion this code used to
/// carry ("`is_request()` covers invalid URL / invalid header value") is not
/// true of this reqwest: both of those are BUILDER errors (`is_builder()`,
/// `is_request() == false`), verified against the linked version in
/// `tests/provider_transport_reset_test.rs`.
///
/// Two request-phase shapes are subtracted so the classification matches what
/// this doc claims:
///
/// - `is_connect()` — the host was never reached. Keeping it on
///   [`DEFAULT_MAX_RETRIES`] lets a provider chain with a fallback fail over
///   promptly instead of waiting out the window.
/// - `is_timeout()` — in the pinned reqwest a client-side timeout is ALSO
///   reported as `Kind::Request` (the total-timeout path constructs
///   `error::request(error::TimedOut)`). A request that ran out of clock is
///   not a destroyed socket: the peer may well have been serving it, so it
///   keeps [`DEFAULT_MAX_RETRIES`] like every other served-or-maybe-served
///   outcome. It is still retryable — `is_timeout()` is admitted by
///   `is_transient` above — only not on the longer window.
fn is_broken_established_connection(e: &reqwest::Error) -> bool {
    e.is_request() && !e.is_connect() && !e.is_timeout()
}

/// True when an [`EgressError`] is a destroyed-mid-request transport failure.
/// Read before the error is consumed by [`provider_error_from_egress`].
fn egress_is_broken_established_connection(e: &EgressError) -> bool {
    matches!(e, EgressError::Transport(inner) if is_broken_established_connection(inner))
}

/// Map an [`EgressError`] from the chokepoint to a `ProviderError`.
///
/// A transport failure is classified exactly like a bare reqwest error
/// (timeout/connect → retryable `Connection`, URL-stripped per H-2); a policy
/// `Denied` is surfaced as `ProviderError::Egress` — terminal, never retried.
pub fn provider_error_from_egress(e: EgressError) -> ProviderError {
    match e {
        EgressError::Transport(inner) => provider_error_from_reqwest(inner),
        EgressError::Denied(reason) => ProviderError::Egress(EgressError::Denied(reason)),
        EgressError::BeforeDispatch(error) => ProviderError::NotAttempted {
            reason: error.to_string(),
        },
        // Terminal — surfaced like Denied, never retried.
        EgressError::BodyTooLarge { limit } => {
            ProviderError::Egress(EgressError::BodyTooLarge { limit })
        }
    }
}

/// Send one provider request through the durable physical-attempt boundary.
///
/// Bedrock rebuilds a SigV4 request inside [`with_retry`] and therefore cannot
/// use [`builder_send_with_retry`]. Keeping its physical send in this helper
/// gives it the same fail-closed lifecycle ordering without adding another
/// retry ring.
pub(crate) async fn send_physical_once(
    builder: EgressRequestBuilder,
) -> Result<reqwest::Response, ProviderError> {
    let lifecycle_attempt = begin_physical_attempt().await?;
    let dispatch_attempt = lifecycle_attempt.clone();
    let builder = builder.before_dispatch(move || {
        let dispatch_attempt = dispatch_attempt.clone();
        async move {
            start_physical_attempt(dispatch_attempt.as_ref())
                .await
                .map_err(|error| error.to_string())
        }
    });
    match builder.send().await {
        Ok(response) => {
            finish_physical_attempt(
                lifecycle_attempt.as_ref(),
                ProviderAttemptHeaderOutcome::HeadersReceived {
                    status: response.status().as_u16(),
                },
            )
            .await?;
            Ok(response)
        }
        Err(error) if error.is_denied() || error.is_before_dispatch() => {
            finish_physical_attempt(
                lifecycle_attempt.as_ref(),
                ProviderAttemptHeaderOutcome::NotStarted {
                    reason: provider_not_started_reason(&error),
                },
            )
            .await?;
            Err(provider_error_from_egress(error))
        }
        Err(error) => {
            finish_physical_attempt(
                lifecycle_attempt.as_ref(),
                ProviderAttemptHeaderOutcome::FailedBeforeHeaders {
                    failure_code: egress_failure_code(&error).to_string(),
                },
            )
            .await?;
            Err(provider_error_from_egress(error))
        }
    }
}

/// Convenience: build the request once (moves `builder`) and send with retry.
///
/// Unlike `send_with_retry`, this takes ownership of a single
/// `RequestBuilder` and clones it on each attempt. Use this when the
/// builder captures data that is cheap to clone (all LLM inference calls).
///
/// Retries cover two transient failure classes:
///   - reqwest connect/timeout errors (no HTTP round-trip completed);
///   - HTTP 5xx / 408 / 429 responses (E-H4 — a completed round-trip
///     with a transient server-side status). The successful `Response`
///     is returned for the *caller* to inspect; only transient statuses
///     are retried here.
///
/// M2: if the builder body is not cloneable (`try_clone()` → `None`), the
/// request is sent **once** without retry rather than failing outright —
/// a non-cloneable streaming body is still a valid single-shot request.
pub async fn builder_send_with_retry(
    builder: EgressRequestBuilder,
) -> Result<reqwest::Response, ProviderError> {
    let max_retries = effective_max_retries(DEFAULT_MAX_RETRIES);
    // A destroyed-mid-request connection is bounded by wall clock, not by a
    // request count — see `BROKEN_CONNECTION_RETRY_WINDOW`. A caller that
    // pinned the ceiling for this scope (`scope_max_retries`, which the engine
    // uses to make one engine attempt exactly one physical send) still wins:
    // asking for `u32::MAX` yields the scoped value when one is set, and "no
    // count bound" when none is.
    let broken_connection_attempt_cap = effective_max_retries(u32::MAX);
    let broken_connection_deadline = tokio::time::Instant::now() + BROKEN_CONNECTION_RETRY_WINDOW;
    let mut backoff = INITIAL_BACKOFF;
    let mut last_err: Option<ProviderError> = None;
    for attempt in 0u32.. {
        // M2: a non-cloneable body cannot be retried — send the original
        // builder exactly once instead of failing with a misleading
        // "Connection" error. `try_clone()` is deterministic, so it fails
        // on attempt 0 and `builder` is still owned here to move into send.
        let try_builder = match builder.try_clone() {
            Some(b) => b,
            None => {
                let lifecycle_attempt = begin_physical_attempt().await?;
                let dispatch_attempt = lifecycle_attempt.clone();
                let builder = builder.before_dispatch(move || {
                    let dispatch_attempt = dispatch_attempt.clone();
                    async move {
                        start_physical_attempt(dispatch_attempt.as_ref())
                            .await
                            .map_err(|error| error.to_string())
                    }
                });
                return match builder.send().await {
                    Ok(response) => {
                        finish_physical_attempt(
                            lifecycle_attempt.as_ref(),
                            ProviderAttemptHeaderOutcome::HeadersReceived {
                                status: response.status().as_u16(),
                            },
                        )
                        .await?;
                        let failure = (!response.status().is_success())
                            .then(|| format!("http_{}", response.status().as_u16()));
                        record_attempt(failure, false);
                        Ok(response)
                    }
                    Err(error) if error.is_denied() || error.is_before_dispatch() => {
                        let failure_code = egress_failure_code(&error).to_string();
                        finish_physical_attempt(
                            lifecycle_attempt.as_ref(),
                            ProviderAttemptHeaderOutcome::NotStarted {
                                reason: provider_not_started_reason(&error),
                            },
                        )
                        .await?;
                        record_not_attempted(failure_code);
                        Err(provider_error_from_egress(error))
                    }
                    Err(error) => {
                        let failure_code = egress_failure_code(&error).to_string();
                        finish_physical_attempt(
                            lifecycle_attempt.as_ref(),
                            ProviderAttemptHeaderOutcome::FailedBeforeHeaders {
                                failure_code: failure_code.clone(),
                            },
                        )
                        .await?;
                        record_attempt(Some(failure_code), false);
                        Err(provider_error_from_egress(error))
                    }
                };
            }
        };
        let lifecycle_attempt = begin_physical_attempt().await?;
        let dispatch_attempt = lifecycle_attempt.clone();
        let try_builder = try_builder.before_dispatch(move || {
            let dispatch_attempt = dispatch_attempt.clone();
            async move {
                start_physical_attempt(dispatch_attempt.as_ref())
                    .await
                    .map_err(|error| error.to_string())
            }
        });
        match try_builder.send().await {
            Ok(resp) => {
                finish_physical_attempt(
                    lifecycle_attempt.as_ref(),
                    ProviderAttemptHeaderOutcome::HeadersReceived {
                        status: resp.status().as_u16(),
                    },
                )
                .await?;
                // E-H4: a 5xx / 408 is a completed HTTP round-trip with a
                // transient *server-side* status. Retry it here instead of
                // handing a doomed response back to the caller.
                //
                // 429 is deliberately NOT retried here: a `Retry-After` of
                // tens of seconds would block `stream()` for a minute-plus.
                // Instead the provider surfaces `RateLimited` (with the
                // header-honoured delay, E-H1) so the caller / resilience
                // layer decides — `with_retry` caps a `RateLimited` sleep at
                // 60 s, the engine can fail over. The final attempt here
                // returns the response so the provider reads the real body.
                let status = resp.status().as_u16();
                let transient_5xx = status >= 500 || status == 408;
                if transient_5xx && attempt < max_retries {
                    record_attempt(Some(format!("http_{status}")), true);
                    tracing::warn!(
                        attempt = attempt + 1,
                        total = max_retries + 1,
                        status,
                        "transient HTTP status; retrying"
                    );
                    tokio::time::sleep(backoff).await;
                    backoff = (backoff * 4).min(Duration::from_secs(4));
                    last_err = Some(ProviderError::Api {
                        status,
                        message: format!("transient HTTP {status}"),
                    });
                    continue;
                }
                let failure = (!resp.status().is_success()).then(|| format!("http_{status}"));
                record_attempt(failure, false);
                return Ok(resp);
            }
            Err(e) if e.is_denied() || e.is_before_dispatch() => {
                let failure_code = egress_failure_code(&e).to_string();
                finish_physical_attempt(
                    lifecycle_attempt.as_ref(),
                    ProviderAttemptHeaderOutcome::NotStarted {
                        reason: provider_not_started_reason(&e),
                    },
                )
                .await?;
                record_not_attempted(failure_code);
                return Err(provider_error_from_egress(e));
            }
            Err(e) => {
                // Classify before `provider_error_from_egress` consumes it: a
                // socket destroyed mid-request earns the longer ceiling.
                let broken_connection = egress_is_broken_established_connection(&e);
                let failure_code = egress_failure_code(&e).to_string();
                finish_physical_attempt(
                    lifecycle_attempt.as_ref(),
                    ProviderAttemptHeaderOutcome::FailedBeforeHeaders {
                        failure_code: failure_code.clone(),
                    },
                )
                .await?;
                // H-2 / secrets-26: strip the URL before formatting so a
                // credential-in-URL provider can't leak the key into the
                // returned error or the `[retry]` tracing warning below.
                let provider_err = match provider_error_from_egress(e) {
                    ProviderError::Connection(msg) => ProviderError::Connection(msg),
                    // A non-transient reqwest error is returned immediately,
                    // exactly as before — only now URL-stripped.
                    other => return Err(other),
                };
                let retrying = if broken_connection {
                    attempt < broken_connection_attempt_cap
                        && tokio::time::Instant::now() < broken_connection_deadline
                } else {
                    attempt < max_retries
                };
                if retrying {
                    record_attempt(Some(failure_code.clone()), true);
                    // M3 fix: 1-based attempt. A broken connection has no
                    // total to report — its bound is the window, not a count.
                    tracing::warn!(
                        attempt = attempt + 1,
                        bound = if broken_connection {
                            "outage_window"
                        } else {
                            "max_retries"
                        },
                        error = %provider_err,
                        "connection error; retrying"
                    );
                    tokio::time::sleep(backoff).await;
                    backoff = (backoff * 4).min(Duration::from_secs(4));
                    last_err = Some(provider_err);
                    continue;
                }
                record_attempt(Some(failure_code), false);
                return Err(provider_err);
            }
        }
    }
    Err(last_err.unwrap_or_else(|| ProviderError::Connection("all retries exhausted".into())))
}

/// True for HTTP statuses that represent a *transient* server-side failure
/// worth retrying: 5xx server errors, 408 request timeout, 429 rate limit.
///
/// 4xx statuses other than 408/429 are client errors — a retry of the same
/// request will fail identically, so they are not retried.
pub fn is_retryable_http_status(status: u16) -> bool {
    status >= 500 || status == 408 || status == 429
}

/// Extract a `Retry-After` hint (in milliseconds) from response headers.
///
/// Reads the standard `retry-after` header via [`parse_retry_after_header`]
/// (RFC 9110 delta-seconds or HTTP-date). Returns `None` when the header is
/// absent or unparseable — callers fall back to their own default.
pub fn retry_after_ms_from_headers(headers: &reqwest::header::HeaderMap) -> Option<u64> {
    headers
        .get(reqwest::header::RETRY_AFTER)
        .and_then(|v| v.to_str().ok())
        .and_then(parse_retry_after_header)
}

/// Default retry-after used when a 429 response carries no usable hint.
pub const DEFAULT_RETRY_AFTER_MS: u64 = 5_000;

/// Resolve the retry-after delay (ms) for a 429 response.
///
/// E-H1: precedence is (1) the HTTP `Retry-After` response header, then
/// (2) a nested `retry_after` / `retry_after_ms` field in the JSON body
/// (Anthropic and OpenAI populate structured rate-limit detail there),
/// then (3) [`DEFAULT_RETRY_AFTER_MS`]. The body is parsed best-effort —
/// a non-JSON or empty body simply skips step 2.
pub fn resolve_retry_after_ms(headers: &reqwest::header::HeaderMap, body_text: &str) -> u64 {
    if let Some(ms) = retry_after_ms_from_headers(headers) {
        return ms;
    }
    if !body_text.trim().is_empty()
        && let Ok(json) = serde_json::from_str::<serde_json::Value>(body_text)
        && let Some(ms) = extract_retry_after_ms_from_nested(&json)
    {
        return ms;
    }
    DEFAULT_RETRY_AFTER_MS
}

/// Maximum retry-after value we will honour, in milliseconds.
///
/// Providers occasionally return absurd `retry_after` values (hours or days).
/// The retry loop already caps `RateLimited` sleeps at 60 s, but the nested
/// extractor caps at 5 minutes so callers that read the value directly
/// (e.g. for logging or scheduling) still see a sane number.
const RETRY_AFTER_CAP_MS: u64 = 300_000;

/// Extract a retry-after hint (in milliseconds) from a structured error JSON value.
///
/// Walks the value in this precedence order:
///   1. Top-level `retry_after_ms` (already milliseconds)
///   2. Top-level `retry_after` (seconds, multiplied by 1000)
///   3. `parameters.retry_after_ms` / `parameters.retry_after`
///   4. `body.retry_after_ms` / `body.retry_after`
///   5. `headers["retry-after"]` (HTTP-header form, seconds)
///
/// Returns `None` if no field is found, or if the value is non-numeric or
/// not strictly positive. The result is capped at 5 minutes
/// ([`RETRY_AFTER_CAP_MS`]) — providers sometimes return absurd values.
///
/// Field precedence and the cap follow RFC 9110 §10.2.3 / RFC 7231 §7.1.3
/// semantics for `Retry-After`, extended to the JSON body shapes LLM
/// provider APIs return in place of the header.
pub fn extract_retry_after_ms_from_nested(error_json: &serde_json::Value) -> Option<u64> {
    fn as_positive_ms(v: &serde_json::Value) -> Option<u64> {
        // Accept integer or float. Reject zero, negatives, NaN, infinity.
        if let Some(n) = v.as_u64() {
            if n > 0 { Some(n) } else { None }
        } else if let Some(n) = v.as_f64() {
            if n.is_finite() && n > 0.0 {
                Some(n as u64)
            } else {
                None
            }
        } else {
            None
        }
    }
    fn as_positive_seconds_ms(v: &serde_json::Value) -> Option<u64> {
        as_positive_ms(v).map(|s| s.saturating_mul(1000))
    }

    let obj = error_json.as_object()?;

    let candidate = obj
        .get("retry_after_ms")
        .and_then(as_positive_ms)
        .or_else(|| obj.get("retry_after").and_then(as_positive_seconds_ms))
        .or_else(|| {
            obj.get("parameters")
                .and_then(|p| p.as_object())
                .and_then(|p| {
                    p.get("retry_after_ms")
                        .and_then(as_positive_ms)
                        .or_else(|| p.get("retry_after").and_then(as_positive_seconds_ms))
                })
        })
        .or_else(|| {
            obj.get("body").and_then(|b| b.as_object()).and_then(|b| {
                b.get("retry_after_ms")
                    .and_then(as_positive_ms)
                    .or_else(|| b.get("retry_after").and_then(as_positive_seconds_ms))
            })
        })
        .or_else(|| {
            obj.get("headers")
                .and_then(|h| h.as_object())
                .and_then(|h| h.get("retry-after"))
                .and_then(|v| v.as_str())
                .and_then(parse_retry_after_header)
        })?;

    Some(candidate.min(RETRY_AFTER_CAP_MS))
}

/// Parse an HTTP `Retry-After` header value into milliseconds.
///
/// Accepts both forms defined by RFC 9110 §10.2.3:
///   - Delta-seconds: `"30"` → `Some(30_000)`
///   - HTTP-date: `"Wed, 21 Oct 2026 07:28:00 GMT"` → delta-from-now in ms
///
/// Returns `None` for unparseable values, non-positive deltas, or HTTP-dates
/// in the past.
pub fn parse_retry_after_header(value: &str) -> Option<u64> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return None;
    }
    // Delta-seconds form (integer or float).
    if let Ok(n) = trimmed.parse::<u64>() {
        if n > 0 {
            return Some(n.saturating_mul(1000));
        }
        return None;
    }
    if let Ok(n) = trimmed.parse::<f64>() {
        if n.is_finite() && n > 0.0 {
            return Some((n * 1000.0) as u64);
        }
        return None;
    }
    // HTTP-date form (RFC 7231 / IMF-fixdate).
    if let Ok(dt) = chrono::DateTime::parse_from_rfc2822(trimmed) {
        let now = chrono::Utc::now();
        let delta = dt.with_timezone(&chrono::Utc) - now;
        let ms = delta.num_milliseconds();
        if ms > 0 {
            return Some(ms as u64);
        }
        return None;
    }
    None
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::sync::atomic::{AtomicU32, Ordering};

    use serde_json::json;
    use wiremock::matchers::method;

    /// The chains below are transcribed from a live run against a real
    /// NXDOMAIN host and a real closed port — see the module doc on
    /// `classify_connect_chain`. If reqwest/hyper-util reword them, the live
    /// test in `tests/connect_failure_classification.rs` fails and these
    /// fixtures must be re-measured, not adjusted to match the new guess.
    #[test]
    fn a_name_that_does_not_exist_is_permanent_and_a_resolver_outage_is_not() {
        let nxdomain = [
            "error sending request for url (https://unreachable.invalid.localdomain:9999/v1/chat/completions)".to_owned(),
            "client error (Connect)".to_owned(),
            "dns error".to_owned(),
            "failed to lookup address information: Name or service not known".to_owned(),
        ];
        assert_eq!(classify_connect_chain(false, &nxdomain), FAILURE_DNS);

        // EAI_AGAIN keeps the generous budget: the name may well exist, the
        // resolver just could not say so yet.
        let resolver_down = [
            "error sending request for url (https://api.example.com/v1)".to_owned(),
            "client error (Connect)".to_owned(),
            "dns error".to_owned(),
            "failed to lookup address information: Temporary failure in name resolution".to_owned(),
        ];
        assert_eq!(
            classify_connect_chain(false, &resolver_down),
            FAILURE_CONNECTION,
            "a resolver outage is transient and must not be classified permanent"
        );

        let refused = [
            "error sending request for url (http://127.0.0.1:1/v1/chat/completions)".to_owned(),
            "client error (Connect)".to_owned(),
            "tcp connect error".to_owned(),
            "Connection refused (os error 111)".to_owned(),
        ];
        assert_eq!(
            classify_connect_chain(true, &refused),
            FAILURE_CONNECTION_REFUSED
        );

        // An unrecognised connect failure keeps the OLD behaviour. The cost of
        // a miss is the status quo; the cost of a false positive is a
        // transient outage that stops healing.
        let unknown = [
            "error sending request".to_owned(),
            "client error (Connect)".to_owned(),
            "Network is unreachable (os error 101)".to_owned(),
        ];
        assert_eq!(classify_connect_chain(false, &unknown), FAILURE_CONNECTION);
    }

    use wiremock::{Mock, MockServer, ResponseTemplate};

    use super::*;
    use crate::ProviderError;

    #[tokio::test]
    async fn test_retry_succeeds_first_try() {
        let result = with_retry(2, || async { Ok::<_, ProviderError>(42) }).await;
        assert_eq!(result.unwrap(), 42);
    }

    #[tokio::test]
    async fn scoped_zero_disables_generic_retries() {
        let attempts = Arc::new(AtomicU32::new(0));
        let result = scope_max_retries(
            0,
            with_retry(DEFAULT_MAX_RETRIES, || {
                let attempts = Arc::clone(&attempts);
                async move {
                    attempts.fetch_add(1, Ordering::SeqCst);
                    Err::<(), _>(ProviderError::Connection("retryable".into()))
                }
            }),
        )
        .await;

        assert!(result.is_err());
        assert_eq!(attempts.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn retries_disabled_only_inside_zero_scope() {
        assert!(!retries_disabled());
        assert!(scope_max_retries(0, async { retries_disabled() }).await);
        assert!(!scope_max_retries(1, async { retries_disabled() }).await);
        assert!(
            scope_max_retries(0, scope_max_retries(2, async { retries_disabled() })).await,
            "a nested scope cannot weaken its parent's retry ceiling"
        );
    }

    #[tokio::test]
    async fn scoped_zero_limits_builder_to_one_physical_attempt() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .respond_with(ResponseTemplate::new(503))
            .mount(&server)
            .await;
        let client =
            wcore_egress::EgressClient::new().with_policy(Arc::new(wcore_egress::AllowAllPolicy));

        let response = scope_max_retries(0, builder_send_with_retry(client.post(server.uri())))
            .await
            .expect("the final HTTP response is returned for provider parsing");

        assert_eq!(response.status(), reqwest::StatusCode::SERVICE_UNAVAILABLE);
        let requests = server.received_requests().await.expect("recorded requests");
        assert_eq!(requests.len(), 1);
    }

    #[tokio::test]
    async fn builder_default_remains_three_physical_attempts() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .respond_with(ResponseTemplate::new(503))
            .mount(&server)
            .await;
        let client =
            wcore_egress::EgressClient::new().with_policy(Arc::new(wcore_egress::AllowAllPolicy));

        let response = builder_send_with_retry(client.post(server.uri()))
            .await
            .expect("the final HTTP response is returned for provider parsing");

        assert_eq!(response.status(), reqwest::StatusCode::SERVICE_UNAVAILABLE);
        let requests = server.received_requests().await.expect("recorded requests");
        assert_eq!(requests.len(), 3);
    }

    #[tokio::test]
    async fn provider_attempt_capture_is_scoped_and_ordered() {
        let (output, evidence) = capture_provider_attempts(async {
            record_attempt(Some("http_503".to_string()), true);
            record_attempt(None, false);
            42
        })
        .await;

        assert_eq!(output, 42);
        assert_eq!(
            evidence,
            vec![
                ProviderAttemptEvidence {
                    physical: true,
                    failure: Some("http_503".to_string()),
                    retrying: true,
                },
                ProviderAttemptEvidence {
                    physical: true,
                    failure: None,
                    retrying: false,
                },
            ]
        );
        assert!(ATTEMPT_EVIDENCE.try_with(|_| ()).is_err());
    }

    #[tokio::test]
    async fn provider_attempt_capture_does_not_cross_concurrent_tasks() {
        let first = tokio::spawn(capture_provider_attempts(async {
            record_attempt(Some("timeout".to_string()), false);
        }));
        let second = tokio::spawn(capture_provider_attempts(async {
            record_attempt(None, false);
        }));

        let (_, first) = first.await.expect("first capture task");
        let (_, second) = second.await.expect("second capture task");
        assert_eq!(first[0].failure.as_deref(), Some("timeout"));
        assert_eq!(second[0].failure, None);
    }

    #[tokio::test]
    async fn live_attempt_observer_survives_future_cancellation() {
        let observed = Arc::new(std::sync::Mutex::new(Vec::new()));
        let sink = Arc::clone(&observed);
        let observer: Arc<dyn Fn(ProviderAttemptEvidence) + Send + Sync> =
            Arc::new(move |evidence| sink.lock().expect("observer lock").push(evidence));

        let future = observe_provider_attempts(observer, async {
            record_attempt(Some("timeout".to_string()), false);
            std::future::pending::<()>().await;
        });
        assert!(
            tokio::time::timeout(Duration::from_millis(1), future)
                .await
                .is_err()
        );

        let observed = observed.lock().expect("observed lock");
        assert_eq!(observed.len(), 1);
        assert_eq!(observed[0].failure.as_deref(), Some("timeout"));
    }

    #[tokio::test]
    async fn test_retry_succeeds_after_failures() {
        // Pause tokio time so sleep calls return immediately
        tokio::time::pause();

        let counter = Arc::new(AtomicU32::new(0));
        let result = with_retry(2, || {
            let counter = Arc::clone(&counter);
            async move {
                let attempt = counter.fetch_add(1, Ordering::SeqCst);
                if attempt < 2 {
                    Err(ProviderError::Connection("timeout".into()))
                } else {
                    Ok(attempt)
                }
            }
        })
        .await;

        assert!(result.is_ok());
        assert_eq!(counter.load(Ordering::SeqCst), 3);
    }

    #[tokio::test]
    async fn test_retry_exhausted() {
        tokio::time::pause();

        let result = with_retry(2, || async {
            Err::<(), _>(ProviderError::Connection("always fails".into()))
        })
        .await;

        assert!(result.is_err());
        assert!(matches!(result.unwrap_err(), ProviderError::Connection(_)));
    }

    #[tokio::test]
    async fn test_retry_non_retryable_error_fails_immediately() {
        let counter = Arc::new(AtomicU32::new(0));
        let result = with_retry(2, || {
            let counter = Arc::clone(&counter);
            async move {
                counter.fetch_add(1, Ordering::SeqCst);
                Err::<(), _>(ProviderError::Api {
                    status: 401,
                    message: "unauthorized".into(),
                })
            }
        })
        .await;

        // Non-retryable errors should fail immediately without retrying
        assert!(result.is_err());
        assert_eq!(counter.load(Ordering::SeqCst), 1);
    }

    /// AF3 Risk 2: a 429 with retry_after_ms=500 must use the server hint,
    /// not the exponential backoff schedule (which would fire at ~250 ms or
    /// ~1 000 ms).  We use `tokio::time::pause` + `tokio::time::advance` to
    /// control virtual time and assert exact sleep durations without
    /// wall-clock delays.
    #[tokio::test]
    async fn test_rate_limited_uses_retry_after_ms_not_exponential_backoff() {
        tokio::time::pause();

        let counter = Arc::new(AtomicU32::new(0));
        let counter_clone = Arc::clone(&counter);

        // Run the retry loop in a background task so we can advance time.
        let task = tokio::spawn(async move {
            with_retry(1, || {
                let c = Arc::clone(&counter_clone);
                async move {
                    let attempt = c.fetch_add(1, Ordering::SeqCst);
                    if attempt == 0 {
                        // First call: return 429 with a 500 ms hint.
                        Err(ProviderError::RateLimited {
                            retry_after_ms: 500,
                        })
                    } else {
                        Ok(attempt)
                    }
                }
            })
            .await
        });

        // The retry loop is now sleeping for retry_after_ms = 500 ms.
        // Advancing by 499 ms must NOT unblock it (exponential would be 250 ms).
        tokio::time::advance(Duration::from_millis(499)).await;
        // Task should still be pending.
        assert_eq!(
            counter.load(Ordering::SeqCst),
            1,
            "second attempt fired too early"
        );

        // Advance past the 500 ms hint — the retry must fire now.
        tokio::time::advance(Duration::from_millis(2)).await;
        let result = task.await.expect("task panicked");
        assert!(result.is_ok(), "expected Ok after retry, got {result:?}");
        assert_eq!(
            counter.load(Ordering::SeqCst),
            2,
            "expected exactly 2 attempts"
        );
    }

    // ----- T1-A4 nested retry-after extraction -----

    #[test]
    fn test_nested_top_level_retry_after_ms() {
        let v = json!({ "retry_after_ms": 5000 });
        assert_eq!(extract_retry_after_ms_from_nested(&v), Some(5000));
    }

    #[test]
    fn test_nested_top_level_retry_after_seconds() {
        let v = json!({ "retry_after": 5 });
        assert_eq!(extract_retry_after_ms_from_nested(&v), Some(5000));
    }

    #[test]
    fn test_nested_parameters_path() {
        let v = json!({ "parameters": { "retry_after_ms": 2000 } });
        assert_eq!(extract_retry_after_ms_from_nested(&v), Some(2000));
    }

    #[test]
    fn test_nested_body_path() {
        let v = json!({ "body": { "retry_after": 3 } });
        assert_eq!(extract_retry_after_ms_from_nested(&v), Some(3000));
    }

    #[test]
    fn test_nested_headers_path() {
        let v = json!({ "headers": { "retry-after": "60" } });
        assert_eq!(extract_retry_after_ms_from_nested(&v), Some(60_000));
    }

    #[test]
    fn test_nested_precedence_top_over_param() {
        // Top-level `retry_after_ms` must beat `parameters.retry_after_ms`.
        let v = json!({
            "retry_after_ms": 1000,
            "parameters": { "retry_after_ms": 9000 },
        });
        assert_eq!(extract_retry_after_ms_from_nested(&v), Some(1000));
    }

    #[test]
    fn test_nested_cap_at_5_minutes() {
        let v = json!({ "retry_after_ms": 999_999_999u64 });
        assert_eq!(extract_retry_after_ms_from_nested(&v), Some(300_000));
    }

    #[test]
    fn test_nested_no_field_returns_none() {
        let v = json!({ "foo": "bar" });
        assert_eq!(extract_retry_after_ms_from_nested(&v), None);
    }

    // ----- H-2 / secrets-26: URL (and thus any `?key=`) must be stripped
    // from formatted provider errors -----

    /// A reqwest error from a request whose URL carries `?key=<SECRET>` must
    /// NOT leak that secret once mapped through `provider_error_from_reqwest`
    /// and formatted. We provoke a real connect failure against an
    /// unroutable address so reqwest produces a URL-bearing error, then
    /// assert the formatted `ProviderError` contains neither `key=` nor the
    /// secret value.
    #[tokio::test]
    async fn provider_error_strips_key_from_url() {
        // 240.0.0.1 is in the reserved 240/4 block — never routable, so the
        // connect fails fast and deterministically.
        let url =
            "http://240.0.0.1:9/v1beta/models/m:streamGenerateContent?alt=sse&key=SUPER_SECRET_KEY";
        // This test exercises `provider_error_from_reqwest` directly, so it
        // needs a genuine `reqwest::Error` — the one sanctioned raw-reqwest use
        // outside wcore-egress (the egress wrapper would yield an EgressError).
        #[allow(clippy::disallowed_methods)]
        let client = reqwest::Client::builder()
            .connect_timeout(Duration::from_millis(200))
            .build()
            .expect("client builds");

        let reqwest_err = client.get(url).send().await.expect_err("connect must fail");
        // Sanity: the raw reqwest error DOES carry the URL (and the secret).
        // If reqwest ever stops doing this the test still passes the real
        // assertion below; this only documents the threat we are closing.
        let raw = reqwest_err.to_string();

        let mapped = super::provider_error_from_reqwest(reqwest_err);
        let formatted = mapped.to_string();
        assert!(
            !formatted.contains("key="),
            "formatted ProviderError must not contain `key=`: {formatted}"
        );
        assert!(
            !formatted.contains("SUPER_SECRET_KEY"),
            "formatted ProviderError must not contain the secret value: {formatted} (raw was: {raw})"
        );
    }

    /// Finding 5, round 2. In the pinned reqwest a client-side TOTAL timeout
    /// is ALSO reported as `Kind::Request`, so `is_request()` alone matches
    /// it. It has to be subtracted: a request that ran out of clock is not a
    /// destroyed socket — the peer may have been serving it — and the doc on
    /// `BROKEN_CONNECTION_RETRY_WINDOW` promises it keeps the short ceiling.
    /// It stays retryable either way; only the bound differs.
    ///
    /// A bare reqwest client is the point of the test: it pins how reqwest
    /// itself shapes the error, then checks our classifier against that.
    #[allow(clippy::disallowed_methods)]
    #[tokio::test]
    async fn a_client_side_request_timeout_is_not_a_destroyed_socket() {
        use std::io::Read;
        use std::net::TcpListener;

        // Accept, read the request, then hold the socket open answering
        // nothing — so the client's own total timeout is what fires, not a
        // close and not a connect failure.
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind loopback");
        let addr = listener.local_addr().expect("local addr");
        std::thread::spawn(move || {
            let mut held = Vec::new();
            for stream in listener.incoming() {
                let Ok(stream) = stream else { break };
                let _ = stream.set_read_timeout(Some(Duration::from_millis(100)));
                let mut sink = [0u8; 8192];
                let mut handle = &stream;
                let _ = handle.read(&mut sink);
                held.push(stream);
            }
        });

        let client = reqwest::Client::builder()
            .timeout(Duration::from_millis(300))
            .build()
            .expect("client builds");
        let err = client
            .post(format!("http://{addr}/v1/chat/completions"))
            .body("{}")
            .send()
            .await
            .expect_err("a server that never answers cannot complete the request");

        // The premise — this is exactly what makes it a trap.
        assert!(
            err.is_request(),
            "a total timeout must be `Kind::Request` in the pinned reqwest; \
             got {err:?}"
        );
        assert!(!err.is_connect(), "the connection was established");
        assert!(err.is_timeout(), "it is a timeout; got {err:?}");

        // The classification that actually matters.
        assert!(
            !super::is_broken_established_connection(&err),
            "a client-side timeout must NOT earn the broken-connection window"
        );
        // ...and it is still retryable, on the default ceiling.
        assert!(matches!(
            super::provider_error_from_reqwest(err),
            ProviderError::Connection(_)
        ));
    }

    #[test]
    fn test_parse_header_seconds_form() {
        assert_eq!(parse_retry_after_header("30"), Some(30_000));
    }

    #[test]
    fn test_parse_header_invalid_returns_none() {
        assert_eq!(parse_retry_after_header("garbage"), None);
    }

    // ----- R3-B1 parse_retry_after_header edge cases -----
    //
    // The fn's docstring promises non-positive deltas, empty input, NaN, and
    // infinity all map to None. Only "30" → Some and "garbage" → None had
    // coverage before this commit.

    #[test]
    fn test_parse_header_zero_returns_none() {
        assert_eq!(parse_retry_after_header("0"), None);
    }

    #[test]
    fn test_parse_header_negative_integer_returns_none() {
        assert_eq!(parse_retry_after_header("-1"), None);
        assert_eq!(parse_retry_after_header("-5"), None);
    }

    #[test]
    fn test_parse_header_negative_float_returns_none() {
        assert_eq!(parse_retry_after_header("-0.5"), None);
    }

    #[test]
    fn test_parse_header_empty_returns_none() {
        assert_eq!(parse_retry_after_header(""), None);
    }

    #[test]
    fn test_parse_header_whitespace_only_returns_none() {
        assert_eq!(parse_retry_after_header("   "), None);
    }

    #[test]
    fn test_parse_header_nan_returns_none() {
        assert_eq!(parse_retry_after_header("NaN"), None);
    }

    #[test]
    fn test_parse_header_infinity_returns_none() {
        assert_eq!(parse_retry_after_header("inf"), None);
        assert_eq!(parse_retry_after_header("-inf"), None);
    }

    #[test]
    fn test_parse_header_http_date_past_returns_none() {
        // A clearly past HTTP-date must yield None (delta <= 0).
        assert_eq!(
            parse_retry_after_header("Wed, 21 Oct 2015 07:28:00 GMT"),
            None
        );
    }

    #[test]
    fn test_parse_header_http_date_future_returns_some() {
        // A clearly future HTTP-date must yield Some(ms > 0). We don't
        // assert the exact value (depends on wall clock at run time);
        // we only assert structure.
        let parsed = parse_retry_after_header("Wed, 21 Oct 2099 07:28:00 GMT");
        assert!(matches!(parsed, Some(ms) if ms > 0));
    }

    // ----- E-H4: HTTP-status retry classification -----

    #[test]
    fn is_retryable_http_status_covers_5xx_408_429() {
        for s in [500, 502, 503, 504, 529, 408, 429] {
            assert!(is_retryable_http_status(s), "{s} must be retryable");
        }
    }

    #[test]
    fn is_retryable_http_status_excludes_4xx_and_2xx() {
        for s in [200, 400, 401, 403, 404, 422] {
            assert!(!is_retryable_http_status(s), "{s} must NOT be retryable");
        }
    }

    /// E-H4: an `Api{status:503}` MUST be retryable so `with_retry` retries a
    /// transient 5xx instead of aborting the turn. A 401 must NOT be.
    #[test]
    fn provider_error_api_5xx_is_retryable_4xx_is_not() {
        assert!(
            ProviderError::Api {
                status: 503,
                message: "overloaded".into(),
            }
            .is_retryable(),
            "503 must retry"
        );
        assert!(
            ProviderError::Api {
                status: 502,
                message: "bad gateway".into(),
            }
            .is_retryable()
        );
        assert!(
            !ProviderError::Api {
                status: 401,
                message: "unauthorized".into(),
            }
            .is_retryable(),
            "401 must not retry"
        );
        assert!(
            !ProviderError::Api {
                status: 400,
                message: "bad request".into(),
            }
            .is_retryable()
        );
    }

    /// E-H4: `with_retry` must now retry a transient 503 and succeed.
    #[tokio::test]
    async fn with_retry_retries_transient_5xx_then_succeeds() {
        tokio::time::pause();
        let counter = Arc::new(AtomicU32::new(0));
        let result = with_retry(2, || {
            let counter = Arc::clone(&counter);
            async move {
                let attempt = counter.fetch_add(1, Ordering::SeqCst);
                if attempt < 2 {
                    Err(ProviderError::Api {
                        status: 503,
                        message: "overloaded".into(),
                    })
                } else {
                    Ok(attempt)
                }
            }
        })
        .await;
        assert!(result.is_ok(), "503 must be retried to success: {result:?}");
        assert_eq!(counter.load(Ordering::SeqCst), 3);
    }

    /// E-H4: a 400 must still fail-fast with exactly one attempt.
    #[tokio::test]
    async fn with_retry_does_not_retry_4xx() {
        let counter = Arc::new(AtomicU32::new(0));
        let result = with_retry(2, || {
            let counter = Arc::clone(&counter);
            async move {
                counter.fetch_add(1, Ordering::SeqCst);
                Err::<(), _>(ProviderError::Api {
                    status: 400,
                    message: "bad request".into(),
                })
            }
        })
        .await;
        assert!(result.is_err());
        assert_eq!(counter.load(Ordering::SeqCst), 1, "400 must not be retried");
    }

    // ----- E-H1: Retry-After resolution from response -----

    fn header_map(pairs: &[(&str, &str)]) -> reqwest::header::HeaderMap {
        let mut h = reqwest::header::HeaderMap::new();
        for (k, v) in pairs {
            h.insert(
                reqwest::header::HeaderName::from_bytes(k.as_bytes()).unwrap(),
                reqwest::header::HeaderValue::from_str(v).unwrap(),
            );
        }
        h
    }

    #[test]
    fn resolve_retry_after_prefers_header() {
        let h = header_map(&[("retry-after", "30")]);
        // Header (30s) wins over both the body hint and the default.
        assert_eq!(
            resolve_retry_after_ms(&h, r#"{"retry_after_ms": 999}"#),
            30_000
        );
    }

    #[test]
    fn resolve_retry_after_falls_back_to_body() {
        let h = reqwest::header::HeaderMap::new();
        assert_eq!(
            resolve_retry_after_ms(&h, r#"{"error":{"retry_after": 12}}"#),
            // nested under `error` is not a walked path; top-level/body/params are.
            DEFAULT_RETRY_AFTER_MS
        );
        // Top-level body field IS walked.
        assert_eq!(
            resolve_retry_after_ms(&h, r#"{"retry_after_ms": 2500}"#),
            2_500
        );
    }

    #[test]
    fn resolve_retry_after_defaults_when_no_hint() {
        let h = reqwest::header::HeaderMap::new();
        assert_eq!(resolve_retry_after_ms(&h, ""), DEFAULT_RETRY_AFTER_MS);
        assert_eq!(
            resolve_retry_after_ms(&h, "not json at all"),
            DEFAULT_RETRY_AFTER_MS
        );
    }

    #[test]
    fn retry_after_ms_from_headers_parses_and_misses() {
        assert_eq!(
            retry_after_ms_from_headers(&header_map(&[("retry-after", "5")])),
            Some(5_000)
        );
        assert_eq!(
            retry_after_ms_from_headers(&reqwest::header::HeaderMap::new()),
            None
        );
        // Garbage header → None (fall through to body/default).
        assert_eq!(
            retry_after_ms_from_headers(&header_map(&[("retry-after", "soon")])),
            None
        );
    }

    /// THE 900 s BUG — a connect-phase DEADLINE expiry must classify as
    /// `timeout`, not `connection`.
    ///
    /// MEASURED on this tree (reqwest 0.12 / hyper-util 0.1 / tokio 1, probe
    /// run 2026-08-22 on hetzner-dsm), a `ClientBuilder::connect_timeout`
    /// expiring against a blackhole produces:
    ///
    /// ```text
    /// display    = "error sending request for url (http://192.0.2.1:9/v1)"
    /// is_timeout = true          <-- reqwest KNOWS it is a timeout
    /// is_connect = true
    /// source[0]  = "client error (Connect)"
    /// source[1]  = "tcp connect error"
    /// source[2]  = "deadline has elapsed"     <-- no "timed out" anywhere
    /// ```
    ///
    /// The product wraps that as `ProviderError::Connection(with_transport_cause(&e))`.
    /// On the `Connection(String)` arm only the rendered text survives, the
    /// `contains("timed out")` guard misses `deadline has elapsed`, and
    /// `classify_connect_chain` falls through to `FAILURE_CONNECTION`.
    /// `"connection"` is one of the four codes `wcore_agent`'s
    /// `is_unserved_request_failure` admits into `UNSERVED_OUTAGE_BUDGET`
    /// (READ_TIMEOUT 300 s x DEFAULT_FAILURE_THRESHOLD 3 = 900 s), so a
    /// connect timeout buys the full 900 s outage window instead of failing
    /// fast — the measured 902 s / 36-send storm.
    ///
    /// Every rendering below is the SAME failure; the fix must catch all of
    /// them, not one literal.
    #[test]
    fn a_connect_deadline_expiry_is_a_timeout_not_a_connection_failure() {
        // Positive control for the assertion mechanism itself: the rendering
        // that ALREADY works. If this line ever fails, the harness is broken
        // and the failures below prove nothing.
        assert_eq!(
            provider_failure_code(&ProviderError::Connection(
                "error sending request for url (http://192.0.2.1:9/v1): operation timed out"
                    .to_owned()
            )),
            "timeout",
            "control: the 'operation timed out' rendering must already classify as a timeout"
        );

        let connect_timeout_renderings = [
            // As `with_transport_cause` renders it on THIS tree today: reqwest's
            // base Display keeps the URL, and the innermost link is appended.
            "error sending request for url (http://192.0.2.1:9/v1): deadline has elapsed",
            // The URL-less spelling of the identical failure — reqwest omits the
            // `for url (...)` clause when the URL cannot be rendered, and
            // `with_transport_cause` drops any link that looks like a URL (H-2).
            "error sending request: deadline has elapsed",
            // The bare innermost link, which is all that survives when the base
            // Display already contains the cause and no suffix is appended.
            "deadline has elapsed",
        ];

        for rendering in connect_timeout_renderings {
            assert_eq!(
                provider_failure_code(&ProviderError::Connection(rendering.to_owned())),
                "timeout",
                "a connect timeout rendered as {rendering:?} must classify as a \
                 timeout; classifying it as a connection failure hands it the \
                 900 s UNSERVED_OUTAGE_BUDGET and reproduces the 902 s hang"
            );
        }
    }

    /// NEGATIVE CONTROL for the fix above. Widening the timeout class must not
    /// swallow the connect failures that are genuinely NOT timeouts — if it
    /// does, `dns_failure`'s fail-fast and `connection_refused`'s prompt
    /// fail-over both regress into the timeout bucket.
    ///
    /// Every row here PASSES today. They exist to fail if the fix over-matches.
    #[test]
    fn widening_the_timeout_class_must_not_swallow_other_connect_failures() {
        let must_not_become_timeout = [
            // MEASURED: a genuine refusal, same probe run.
            (
                "error sending request for url (http://127.0.0.1:1/v1): Connection refused (os error 111)",
                FAILURE_CONNECTION_REFUSED,
            ),
            // Windows spelling of the same refusal (WSAECONNREFUSED).
            (
                "error sending request: the target machine actively refused it (os error 10061)",
                FAILURE_CONNECTION_REFUSED,
            ),
            // A name that does not exist stays PERMANENT and keeps failing fast.
            (
                "error sending request: failed to lookup address information: Name or service not known",
                FAILURE_DNS,
            ),
            // A resolver outage stays transient and keeps the generous budget.
            (
                "error sending request: failed to lookup address information: Temporary failure in name resolution",
                FAILURE_CONNECTION,
            ),
            // The ambiguous residue stays ambiguous — not a timeout.
            (
                "error sending request: Network is unreachable (os error 101)",
                FAILURE_CONNECTION,
            ),
        ];

        for (rendering, expected) in must_not_become_timeout {
            let code = provider_failure_code(&ProviderError::Connection(rendering.to_owned()));
            assert_eq!(
                code, expected,
                "{rendering:?} must stay {expected:?}; it is not a timeout and \
                 must not be swept into the timeout class by an over-broad fix"
            );
            assert_ne!(
                code, "timeout",
                "{rendering:?} was reclassified as a timeout — the fix over-matches"
            );
        }
    }

    /// The live reproduction, driven end to end through the REAL production
    /// path — `EgressClient` -> `provider_error_from_egress` ->
    /// `provider_error_from_reqwest` -> `provider_failure_code` — with nothing
    /// hand-constructed. The fixture test above pins the strings; this one
    /// proves the strings are real and the defect is reachable, not modelled.
    ///
    /// MEASURED (300 blackholed connects, hetzner-dsm, 2026-08-22): ONE failure
    /// mode renders TWO ways, non-deterministically, both with
    /// `is_timeout()=true` and `is_connect()=true`:
    ///
    /// ```text
    /// 281/300 (93.7%)  "...: deadline has elapsed"   -> classified "connection"  BUG
    ///  19/300 ( 6.3%)  "...: operation timed out"    -> classified "timeout"     ok
    /// ```
    ///
    /// So the 900 s hang is INTERMITTENT: the same misconfigured `base_url`
    /// fails fast about one time in sixteen and burns the full outage budget
    /// the rest of the time. That race is also why a single-sample probe can
    /// conclude the classifier is fine.
    ///
    /// The loop exists for exactly that reason — a one-shot version of this
    /// test passes vacuously 6% of the time (MEASURED: 1 pass in 20 runs). It
    /// asserts that EVERY observed rendering classifies as a timeout, and
    /// separately that the `deadline has elapsed` rendering was actually
    /// exercised, so a run that never reproduces the defect fails loudly
    /// instead of reporting a green it did not earn.
    #[tokio::test]
    async fn a_live_connect_timeout_reaches_the_classifier_as_a_connection_failure() {
        const SAMPLES: usize = 40;
        let mut saw_deadline_rendering = false;
        let mut misclassified: Vec<(String, String)> = Vec::new();

        for _ in 0..SAMPLES {
            // 192.0.2.1 is TEST-NET-1 (RFC 5737): reserved, non-routable, blackholes.
            // B1: constructed through the egress chokepoint, like every other
            // client in this crate.
            let client = wcore_egress::EgressClient::builder()
                .connect_timeout(std::time::Duration::from_millis(50))
                .build()
                .expect("client builds");
            let error = client
                .get("http://192.0.2.1:9/v1")
                .send()
                .await
                .expect_err("a blackholed connect must not succeed");

            // Preconditions: announce loudly if the environment produced some
            // other failure (an RST from a middlebox), rather than passing
            // vacuously on a sample that never exercised the defect.
            let EgressError::Transport(ref transport) = error else {
                panic!(
                    "precondition: a blackholed connect must surface as a transport \
                     failure, got: {error}"
                );
            };
            assert!(
                transport.is_timeout(),
                "precondition: reqwest must report this as a timeout, got: {transport}"
            );
            assert!(
                transport.is_connect(),
                "precondition: it must also be a connect-phase failure, got: {transport}"
            );

            // The real mapping the product runs, not a re-implementation.
            let provider_error = provider_error_from_egress(error);
            let ProviderError::Connection(ref rendered) = provider_error else {
                panic!(
                    "precondition: a transient transport failure must be wrapped as \
                     ProviderError::Connection, got: {provider_error}"
                );
            };
            if rendered
                .to_ascii_lowercase()
                .contains("deadline has elapsed")
            {
                saw_deadline_rendering = true;
            }
            let code = provider_failure_code(&provider_error);
            if code != "timeout" {
                misclassified.push((rendered.clone(), code));
            }
        }

        assert!(
            saw_deadline_rendering,
            "anti-vacuity: {SAMPLES} blackholed connects never produced the \
             'deadline has elapsed' rendering, so this run never exercised the \
             defect and its result means nothing"
        );
        assert!(
            misclassified.is_empty(),
            "reqwest reported is_timeout()=true, but {} of {SAMPLES} connect \
             timeouts classified as something else — the timeout signal is lost \
             crossing into ProviderError::Connection and the 900 s \
             UNSERVED_OUTAGE_BUDGET takes the request. First: {:?}",
            misclassified.len(),
            misclassified.first().expect("non-empty")
        );
    }
}
