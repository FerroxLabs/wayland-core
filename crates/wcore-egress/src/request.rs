//! [`EgressRequestBuilder`] — the per-request wrapper around
//! [`reqwest::RequestBuilder`].
//!
//! This type exists so that **the only way to send a request is through
//! [`EgressRequestBuilder::send`]**, which consults the egress policy. If
//! [`crate::EgressClient::get`] (etc.) returned a raw
//! [`reqwest::RequestBuilder`], its `.send()` would bypass the policy and the
//! workspace lint could not catch it. The chaining methods below forward
//! 1:1 to reqwest so call sites read unchanged.

use std::fmt::Display;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use crate::error::{BeforeDispatchError, EgressError};
use crate::observer::{
    EgressAttemptGuard, EgressOutcome, SharedEgressObserver, classify_transport_error,
};
use crate::policy::{EgressDecision, SharedPolicy};

type BeforeDispatchFuture =
    Pin<Box<dyn Future<Output = Result<(), BeforeDispatchError>> + Send + 'static>>;
type BeforeDispatchHook = Arc<dyn Fn() -> BeforeDispatchFuture + Send + Sync>;

/// Builds and sends a single outbound request through the egress chokepoint.
///
/// Obtained from [`crate::EgressClient::get`] / `post` / `request` / etc. The
/// chainable configuration methods mirror [`reqwest::RequestBuilder`]; `send`
/// is the policy-gated terminal.
pub struct EgressRequestBuilder {
    client: reqwest::Client,
    policy: SharedPolicy,
    observer: SharedEgressObserver,
    next_attempt_id: Arc<AtomicU64>,
    inner: reqwest::RequestBuilder,
    before_dispatch: Option<BeforeDispatchHook>,
}

impl EgressRequestBuilder {
    pub(crate) fn new(
        client: reqwest::Client,
        policy: SharedPolicy,
        observer: SharedEgressObserver,
        next_attempt_id: Arc<AtomicU64>,
        inner: reqwest::RequestBuilder,
    ) -> Self {
        Self {
            client,
            policy,
            observer,
            next_attempt_id,
            inner,
            before_dispatch: None,
        }
    }

    /// Add a single header. Mirrors [`reqwest::RequestBuilder::header`],
    /// including its generic key/value bounds.
    pub fn header<K, V>(mut self, key: K, value: V) -> Self
    where
        reqwest::header::HeaderName: TryFrom<K>,
        <reqwest::header::HeaderName as TryFrom<K>>::Error: Into<http::Error>,
        reqwest::header::HeaderValue: TryFrom<V>,
        <reqwest::header::HeaderValue as TryFrom<V>>::Error: Into<http::Error>,
    {
        self.inner = self.inner.header(key, value);
        self
    }

    /// Add a whole [`reqwest::header::HeaderMap`].
    pub fn headers(mut self, headers: reqwest::header::HeaderMap) -> Self {
        self.inner = self.inner.headers(headers);
        self
    }

    /// Set the request body to a JSON serialization of `json`.
    pub fn json<T: serde::Serialize + ?Sized>(mut self, json: &T) -> Self {
        self.inner = self.inner.json(json);
        self
    }

    /// Set the request body to a URL-encoded form serialization of `form`.
    pub fn form<T: serde::Serialize + ?Sized>(mut self, form: &T) -> Self {
        self.inner = self.inner.form(form);
        self
    }

    /// Append serialized query-string parameters to the URL.
    pub fn query<T: serde::Serialize + ?Sized>(mut self, query: &T) -> Self {
        self.inner = self.inner.query(query);
        self
    }

    /// Set a raw body (string, bytes, or stream).
    pub fn body<T: Into<reqwest::Body>>(mut self, body: T) -> Self {
        self.inner = self.inner.body(body);
        self
    }

    /// Send a `multipart/form-data` body.
    pub fn multipart(mut self, form: reqwest::multipart::Form) -> Self {
        self.inner = self.inner.multipart(form);
        self
    }

    /// Set an `Authorization: Bearer <token>` header.
    pub fn bearer_auth<T: Display>(mut self, token: T) -> Self {
        self.inner = self.inner.bearer_auth(token);
        self
    }

    /// Set an `Authorization: Basic` header.
    pub fn basic_auth<U, P>(mut self, username: U, password: Option<P>) -> Self
    where
        U: Display,
        P: Display,
    {
        self.inner = self.inner.basic_auth(username, password);
        self
    }

    /// Set a per-request wall-clock timeout.
    pub fn timeout(mut self, timeout: Duration) -> Self {
        self.inner = self.inner.timeout(timeout);
        self
    }

    /// Run an async callback after policy admission and immediately before the
    /// physical network dispatch.
    ///
    /// The callback is not invoked when policy denies the request. Returning
    /// an error stops the request before network I/O. Builder clones retain the
    /// callback, so each retry gets its own pre-dispatch invocation.
    pub fn before_dispatch<F, Fut, E>(mut self, hook: F) -> Self
    where
        F: Fn() -> Fut + Send + Sync + 'static,
        Fut: Future<Output = Result<(), E>> + Send + 'static,
        E: Display + Send + 'static,
    {
        self.before_dispatch = Some(Arc::new(move || {
            let future = hook();
            Box::pin(async move {
                future
                    .await
                    .map_err(|error| BeforeDispatchError::new(error.to_string()))
            })
        }));
        self
    }

    /// Try to clone this builder. Returns `None` when the body is a non-cloneable
    /// stream — same semantics as [`reqwest::RequestBuilder::try_clone`]. Used by
    /// the retry layer, which re-sends a request on transient failure.
    pub fn try_clone(&self) -> Option<Self> {
        self.inner.try_clone().map(|inner| Self {
            client: self.client.clone(),
            policy: self.policy.clone(),
            observer: self.observer.clone(),
            next_attempt_id: self.next_attempt_id.clone(),
            inner,
            before_dispatch: self.before_dispatch.clone(),
        })
    }

    /// Build the request, consult the egress policy, and — if allowed — send it.
    ///
    /// This is the single egress gate: the policy sees the fully-built
    /// [`reqwest::Request`] (method, URL, headers, body) and a `Deny` short-
    /// circuits the network call entirely.
    pub async fn send(self) -> Result<reqwest::Response, EgressError> {
        let request = self.inner.build()?;
        let attempt_id = self.next_attempt_id.fetch_add(1, Ordering::Relaxed);
        let mut observation = EgressAttemptGuard::new(self.observer.clone(), attempt_id, &request);
        match self.policy.check(&request).await {
            EgressDecision::Allow => {
                observation.mark_allowed();
                if let Some(before_dispatch) = self.before_dispatch
                    && let Err(error) = before_dispatch().await
                {
                    observation.finish(EgressOutcome::BeforeDispatchFailed);
                    return Err(error.into());
                }
                match self.client.execute(request).await {
                    Ok(response) => {
                        observation.finish(EgressOutcome::HttpResponse {
                            status: response.status().as_u16(),
                        });
                        Ok(response)
                    }
                    Err(error) => {
                        observation.finish(EgressOutcome::TransportError {
                            class: classify_transport_error(&error),
                        });
                        Err(EgressError::Transport(error))
                    }
                }
            }
            EgressDecision::Deny { reason } => {
                observation.finish(EgressOutcome::Denied);
                Err(EgressError::Denied(reason))
            }
        }
    }
}
