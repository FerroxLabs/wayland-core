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

    /// Where this request will be dispatched, and whether that is somewhere on
    /// this machine or its private network (wayland#372).
    ///
    /// `None` when the request cannot be built (a non-cloneable streaming body)
    /// or carries no host. `reqwest::RequestBuilder` does not expose its URL and
    /// `build()` consumes it, so this goes through `try_clone` — the same call
    /// the provider retry ring already makes once per attempt, and cheap for
    /// the reusable `Bytes` bodies every LLM call uses.
    pub fn endpoint_route(&self) -> Option<EndpointRoute> {
        EndpointRoute::of(self.inner.try_clone()?.build().ok()?.url())
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

/// Where a request is going, in the two terms wayland#372 asks for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EndpointRoute {
    /// `scheme://host[:port]`, and DELIBERATELY nothing else.
    ///
    /// Userinfo, path, query and fragment are dropped. That is not tidiness: a
    /// provider key routinely rides in a query parameter or in userinfo, and
    /// this value is emitted on the protocol wire to a host UI. The origin is
    /// the whole of what "which route did this step use" needs. The port is
    /// carried only when the URL states one, so a default-port cloud endpoint
    /// stays readable while a local `:11434` never loses the digits that
    /// identify it.
    pub origin: String,
    /// Whether `origin` addresses this machine or its private network.
    pub is_local: bool,
}

impl EndpointRoute {
    /// Classify a URL. `None` for a URL with no host (`data:`, `mailto:`).
    pub fn of(url: &reqwest::Url) -> Option<Self> {
        let host = url.host_str()?;
        let origin = match url.port() {
            Some(port) => format!("{}://{host}:{port}", url.scheme()),
            None => format!("{}://{host}", url.scheme()),
        };
        Some(Self {
            is_local: host_is_local(host),
            origin,
        })
    }
}

/// Whether a host literal addresses this machine or its private network.
///
/// PURELY LEXICAL — this never resolves a name. A classifier that did would put
/// a blocking network call on the dispatch path and would fail closed under a
/// DNS outage, which is one of the conditions it exists to report on. A name
/// that is not obviously local is therefore reported as NOT local: the field
/// says "this is definitely on your machine", never "this is definitely not".
fn host_is_local(host: &str) -> bool {
    // `Url::host_str` keeps the brackets on an IPv6 literal.
    let bare = host.trim_start_matches('[').trim_end_matches(']');
    match bare.parse::<std::net::IpAddr>() {
        Ok(std::net::IpAddr::V4(ip)) => ip.is_loopback() || ip.is_private() || ip.is_link_local(),
        Ok(std::net::IpAddr::V6(ip)) => {
            ip.is_loopback()
                // fc00::/7 unique-local and fe80::/10 link-local, written out
                // because `is_unique_local` / `is_unicast_link_local` are not
                // stable.
                || (ip.segments()[0] & 0xfe00) == 0xfc00
                || (ip.segments()[0] & 0xffc0) == 0xfe80
        }
        Err(_) => {
            let name = bare.trim_end_matches('.').to_ascii_lowercase();
            name == "localhost" || name.ends_with(".localhost") || name.ends_with(".local")
        }
    }
}

#[cfg(test)]
mod endpoint_route_tests {
    use super::*;

    fn route(raw: &str) -> Option<EndpointRoute> {
        EndpointRoute::of(&reqwest::Url::parse(raw).unwrap())
    }

    /// The reported configuration in wayland#372: a local Ollama-compatible
    /// endpoint on loopback.
    #[test]
    fn a_loopback_endpoint_is_local() {
        let route = route("http://127.0.0.1:11434/v1/chat/completions").unwrap();
        assert_eq!(route.origin, "http://127.0.0.1:11434");
        assert!(route.is_local);
    }

    /// POLARITY CONTROL. Without this, a classifier stuck at `true` would pass
    /// every local case above and the field would mean nothing.
    #[test]
    fn a_public_endpoint_is_not_local() {
        for raw in [
            "https://api.anthropic.com/v1/messages",
            "https://api.openai.com/v1/chat/completions",
            "https://8.8.8.8/v1",
            // Not obviously local, and never resolved to find out.
            "http://ollama.example.com:11434/v1",
        ] {
            let route = route(raw).unwrap();
            assert!(!route.is_local, "{raw} must not be reported as local");
        }
        assert_eq!(
            route("https://api.anthropic.com/v1/messages")
                .unwrap()
                .origin,
            "https://api.anthropic.com",
            "an implicit default port must not be invented into the origin"
        );
    }

    #[test]
    fn the_private_ranges_and_local_names_are_local() {
        for raw in [
            "http://localhost:11434/v1",
            "http://LOCALHOST:11434/v1",
            "http://box.local:11434/v1",
            "http://192.168.1.5:11434/v1",
            "http://10.0.0.7:11434/v1",
            "http://172.16.4.1:11434/v1",
            "http://169.254.1.1:11434/v1",
            "http://[::1]:11434/v1",
            "http://[fe80::1]:11434/v1",
            "http://[fd00::1]:11434/v1",
        ] {
            assert!(route(raw).unwrap().is_local, "{raw} must be local");
        }
    }

    /// The origin must not carry a credential. Both shapes below put a live key
    /// on the wire if the whole URL were reported.
    #[test]
    fn the_origin_drops_credentials_and_everything_after_the_authority() {
        let route =
            route("https://user:sk-secret@api.example.com/v1/x?key=sk-also-secret#frag").unwrap();
        assert_eq!(route.origin, "https://api.example.com");
        assert!(!route.origin.contains("sk-"), "{}", route.origin);
        assert!(!route.origin.contains('@'), "{}", route.origin);
    }

    #[test]
    fn a_hostless_url_has_no_route() {
        assert!(EndpointRoute::of(&reqwest::Url::parse("data:text/plain,hi").unwrap()).is_none());
    }
}
