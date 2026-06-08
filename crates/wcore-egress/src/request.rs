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
use std::time::Duration;

use crate::error::EgressError;
use crate::policy::{EgressDecision, SharedPolicy};

/// Builds and sends a single outbound request through the egress chokepoint.
///
/// Obtained from [`crate::EgressClient::get`] / `post` / `request` / etc. The
/// chainable configuration methods mirror [`reqwest::RequestBuilder`]; `send`
/// is the policy-gated terminal.
pub struct EgressRequestBuilder {
    client: reqwest::Client,
    policy: SharedPolicy,
    inner: reqwest::RequestBuilder,
}

impl EgressRequestBuilder {
    pub(crate) fn new(
        client: reqwest::Client,
        policy: SharedPolicy,
        inner: reqwest::RequestBuilder,
    ) -> Self {
        Self {
            client,
            policy,
            inner,
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

    /// Try to clone this builder. Returns `None` when the body is a non-cloneable
    /// stream — same semantics as [`reqwest::RequestBuilder::try_clone`]. Used by
    /// the retry layer, which re-sends a request on transient failure.
    pub fn try_clone(&self) -> Option<Self> {
        self.inner.try_clone().map(|inner| Self {
            client: self.client.clone(),
            policy: self.policy.clone(),
            inner,
        })
    }

    /// Build the request, consult the egress policy, and — if allowed — send it.
    ///
    /// This is the single egress gate: the policy sees the fully-built
    /// [`reqwest::Request`] (method, URL, headers, body) and a `Deny` short-
    /// circuits the network call entirely.
    pub async fn send(self) -> Result<reqwest::Response, EgressError> {
        let request = self.inner.build()?;
        match self.policy.check(&request).await {
            EgressDecision::Allow => Ok(self.client.execute(request).await?),
            EgressDecision::Deny { reason } => Err(EgressError::Denied(reason)),
        }
    }
}
