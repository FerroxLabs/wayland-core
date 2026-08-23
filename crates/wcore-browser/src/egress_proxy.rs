//! `PolicyEgressProxy` — the seam that makes the sidecar's DNS **Core's** DNS.
//!
//! ## The defect this closes (gh#1117)
//!
//! Camoufox is a sidecar. Firefox resolves every name in its own process, so
//! the address it dials was never the address
//! [`BrowserPolicy::evaluate_navigation_target`] checked. An authoritative
//! server answering TTL=0 can hand Core a public address and Firefox an
//! internal one **within one navigation**, and no gate that runs inside Core's
//! process can see it. Sub-resource loads (`<img src=…>`, XHR, fonts) never
//! reached a gate at all.
//!
//! ## How it is closed
//!
//! Core listens on `127.0.0.1:0` and the sidecar is launched with
//! `PROXY_HOST` / `PROXY_PORT` pointing at it (see
//! [`crate::supervisor::BrowserSupervisor::ensure_ready`]). Firefox then hands
//! the proxy a **name**:
//!
//! ```text
//! CONNECT api.example.com:443 HTTP/1.1        (https — verified live)
//! GET http://example.com/ HTTP/1.1            (http  — verified live)
//! ```
//!
//! MEASURED 2026-08-23 against `@askjo/camofox-browser@1.13.1` on real
//! Camoufox: both request forms above were observed verbatim, with the
//! HOSTNAME in the request line and no DNS query issued by Firefox for it.
//! Core resolves the name, screens **every** answer, and dials one of the
//! screened addresses itself. There is no window in which a second answer can
//! be substituted, because there is no second lookup.
//!
//! ## What this proxy enforces, and what it deliberately does not
//!
//! It enforces the policy's **hard address gate**:
//!
//!   * the scheme allowlist,
//!   * the hardcoded block-list (loopback / RFC 1918 / link-local / cloud
//!     metadata / IPv6 ULA / legacy IPv4 encodings), including the gh#911
//!     loopback grant as the one recoverable escape hatch,
//!   * `denied_origins` — an explicit block applies everywhere,
//!   * and the resolution screen: resolve once, refuse unless every answer
//!     clears the block-list, dial one of those answers.
//!
//! It deliberately does **not** apply `allowed_origins` / `default_action`.
//! Those are a NAVIGATION policy — an operator writes `*.example.com` to say
//! where the agent may browse, not to say that example.com may not load its
//! own fonts. They keep applying in full at the three navigation seams
//! ([`crate::tool::BrowserTool::policy_check`] and the two landing-URL checks
//! in [`crate::backends::CamoufoxBackend`]), exactly as before. The split is
//! made explicit by [`BrowserPolicy::address_gate_only`], and it is a
//! narrowing that can only ever refuse more than the old behaviour, never
//! less: before this module NOTHING screened a sub-resource.
//!
//! ## Failure posture
//!
//! Fail-closed at every step. A refused target gets `403` and no tunnel; a
//! head that never terminates, exceeds 16 KiB, or arrives after the read
//! deadline gets `403` and no tunnel; an unparseable or origin-form request
//! line gets `400`. The proxy never falls back to letting the sidecar dial
//! for itself — that is the entire defect.

use std::io;
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio_util::sync::CancellationToken;

use crate::policy::{BrowserPolicy, DialApproval};

/// Cap on the request head Core will buffer before refusing. A proxy request
/// line plus headers is a few hundred bytes; 16 KiB is generous and bounded.
const MAX_HEAD_BYTES: usize = 16 * 1024;
/// How long Core waits for a complete request head.
const HEAD_READ_TIMEOUT: Duration = Duration::from_secs(15);
/// How long Core waits for one approved address to accept a connection.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(15);

/// A running loopback proxy. Dropping this does NOT stop it — the accept loop
/// holds its own `Arc` — so the owner must call [`Self::shutdown`].
#[derive(Debug)]
pub struct PolicyEgressProxy {
    addr: SocketAddr,
    cancel: CancellationToken,
    approved: AtomicU64,
    refused: AtomicU64,
}

impl PolicyEgressProxy {
    /// Bind `127.0.0.1:0` and start serving. Returns once the listener is
    /// bound, so the port is usable the moment this resolves.
    ///
    /// `policy` should be [`BrowserPolicy::address_gate_only`] of the
    /// operator's policy — see the module header for why.
    pub async fn start(policy: BrowserPolicy) -> io::Result<Arc<Self>> {
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).await?;
        let addr = listener.local_addr()?;
        let proxy = Arc::new(Self {
            addr,
            cancel: CancellationToken::new(),
            approved: AtomicU64::new(0),
            refused: AtomicU64::new(0),
        });
        let accept_proxy = Arc::clone(&proxy);
        tokio::spawn(async move {
            loop {
                tokio::select! {
                    _ = accept_proxy.cancel.cancelled() => break,
                    accepted = listener.accept() => {
                        let Ok((stream, _peer)) = accepted else {
                            // The listener is broken. Stopping is the
                            // fail-closed direction: the sidecar has no
                            // other route out, so it loses egress rather
                            // than gaining un-screened egress.
                            break;
                        };
                        let policy = policy.clone();
                        let conn_proxy = Arc::clone(&accept_proxy);
                        tokio::spawn(async move {
                            let _ = serve_connection(stream, policy, conn_proxy).await;
                        });
                    }
                }
            }
        });
        Ok(proxy)
    }

    /// Loopback host the sidecar should be pointed at.
    pub fn host(&self) -> String {
        self.addr.ip().to_string()
    }

    /// Ephemeral port the sidecar should be pointed at.
    pub fn port(&self) -> u16 {
        self.addr.port()
    }

    /// Connections that cleared the gate and were tunnelled.
    pub fn approved_count(&self) -> u64 {
        self.approved.load(Ordering::Relaxed)
    }

    /// Connections the gate refused. A test asserting "the sidecar could not
    /// reach it" reads this, not just the client-side error.
    pub fn refused_count(&self) -> u64 {
        self.refused.load(Ordering::Relaxed)
    }

    /// Stop the accept loop. In-flight tunnels finish on their own.
    pub fn shutdown(&self) {
        self.cancel.cancel();
    }
}

async fn serve_connection(
    mut client: TcpStream,
    policy: BrowserPolicy,
    proxy: Arc<PolicyEgressProxy>,
) -> io::Result<()> {
    let Some((head, head_end)) = read_head(&mut client, &proxy).await? else {
        return Ok(());
    };

    let line_end = find(&head, b"\r\n").unwrap_or(head_end);
    let request_line = String::from_utf8_lossy(&head[..line_end]).into_owned();
    let mut fields = request_line.split_whitespace();
    let method = fields.next().unwrap_or_default().to_ascii_uppercase();
    let target = fields.next().unwrap_or_default();

    // The two forms Firefox actually sends to an HTTP proxy. Anything else
    // (origin-form `GET /path`, a bare line) is not a proxy request and is
    // refused rather than guessed at.
    let (gate_url, is_connect) = if method == "CONNECT" {
        match connect_authority_url(target) {
            Some(url) => (url, true),
            None => {
                proxy.refused.fetch_add(1, Ordering::Relaxed);
                return refuse(
                    &mut client,
                    400,
                    "Bad Request",
                    &format!("CONNECT target {target:?} is not host:port"),
                )
                .await;
            }
        }
    } else if target.contains("://") {
        (target.to_string(), false)
    } else {
        proxy.refused.fetch_add(1, Ordering::Relaxed);
        return refuse(
            &mut client,
            400,
            "Bad Request",
            "not a proxy request: expected CONNECT host:port or an absolute-form request target",
        )
        .await;
    };

    let (host, port, addrs) = match policy.approve_dial_target(&gate_url) {
        DialApproval::Approved { host, port, addrs } => (host, port, addrs),
        DialApproval::Denied { reason } => {
            proxy.refused.fetch_add(1, Ordering::Relaxed);
            return refuse(&mut client, 403, "Forbidden", &reason).await;
        }
        DialApproval::Suspend { url } => {
            proxy.refused.fetch_add(1, Ordering::Relaxed);
            // The proxy has no HITL channel; `Ask` cannot be answered here.
            return refuse(
                &mut client,
                403,
                "Forbidden",
                &format!("{url} requires approval (Ask policy) and a proxied sidecar request has no approval channel"),
            )
            .await;
        }
    };

    let mut upstream = match dial(&host, port, &addrs).await {
        Ok(s) => s,
        Err(e) => {
            // Not a policy refusal — do not count it as one — but still no
            // tunnel and no fallback to letting the sidecar dial.
            return refuse(
                &mut client,
                502,
                "Bad Gateway",
                &format!("could not reach {host}:{port} at any screened address: {e}"),
            )
            .await;
        }
    };

    proxy.approved.fetch_add(1, Ordering::Relaxed);

    if is_connect {
        client
            .write_all(b"HTTP/1.1 200 Connection established\r\n\r\n")
            .await?;
        // Anything the client pipelined after the head (typically the TLS
        // ClientHello) belongs to the tunnel.
        if head_end < head.len() {
            upstream.write_all(&head[head_end..]).await?;
        }
    } else {
        // Plain HTTP. The head is forwarded with `Connection: close` forced so
        // the client cannot reuse this connection for a SECOND request to a
        // DIFFERENT host, which would ride a tunnel gated for the first one.
        upstream
            .write_all(&close_framed_head(&head[..head_end]))
            .await?;
        if head_end < head.len() {
            upstream.write_all(&head[head_end..]).await?;
        }
    }

    let _ = tokio::io::copy_bidirectional(&mut client, &mut upstream).await;
    Ok(())
}

/// Read until the end of the request head. `Ok(None)` means the peer went away
/// without sending one; a head that is too large or too slow is refused here.
async fn read_head(
    client: &mut TcpStream,
    proxy: &Arc<PolicyEgressProxy>,
) -> io::Result<Option<(Vec<u8>, usize)>> {
    let mut head = Vec::with_capacity(1024);
    let mut buf = [0u8; 4096];
    let deadline = tokio::time::Instant::now() + HEAD_READ_TIMEOUT;
    loop {
        if let Some(i) = find(&head, b"\r\n\r\n") {
            return Ok(Some((head, i + 4)));
        }
        if head.len() >= MAX_HEAD_BYTES {
            proxy.refused.fetch_add(1, Ordering::Relaxed);
            refuse(
                client,
                403,
                "Forbidden",
                "proxy request head exceeded 16 KiB before terminating",
            )
            .await?;
            return Ok(None);
        }
        let read = tokio::time::timeout_at(deadline, client.read(&mut buf)).await;
        match read {
            Ok(Ok(0)) => return Ok(None),
            Ok(Ok(n)) => head.extend_from_slice(&buf[..n]),
            Ok(Err(e)) => return Err(e),
            Err(_) => {
                proxy.refused.fetch_add(1, Ordering::Relaxed);
                refuse(
                    client,
                    403,
                    "Forbidden",
                    "proxy request head did not arrive within the read deadline",
                )
                .await?;
                return Ok(None);
            }
        }
    }
}

/// `example.com:443` → `https://example.com:443/`, the shape
/// [`BrowserPolicy::approve_dial_target`] parses. The scheme is `https`
/// because CONNECT is only ever used for a TLS tunnel; the PORT is preserved
/// verbatim because the gh#911 loopback grant is port-scoped.
fn connect_authority_url(target: &str) -> Option<String> {
    let (host, port) = target.rsplit_once(':')?;
    if host.is_empty() || port.parse::<u16>().is_err() {
        return None;
    }
    Some(format!("https://{host}:{port}/"))
}

/// Rewrite a forwarded request head so the connection cannot be reused.
///
/// Hop-by-hop headers a proxy must not forward (`Proxy-Connection`,
/// `Keep-Alive`) are dropped, any existing `Connection:` is replaced, and
/// `Connection: close` is appended. The result: one gated request per TCP
/// connection, so a second request on the same socket cannot inherit the
/// first one's approval.
fn close_framed_head(head: &[u8]) -> Vec<u8> {
    let text = String::from_utf8_lossy(head);
    let mut out = String::with_capacity(text.len() + 24);
    for (i, line) in text.split("\r\n").enumerate() {
        if line.is_empty() {
            continue;
        }
        if i > 0 {
            let name = line.split(':').next().unwrap_or_default().trim();
            if name.eq_ignore_ascii_case("connection")
                || name.eq_ignore_ascii_case("proxy-connection")
                || name.eq_ignore_ascii_case("keep-alive")
            {
                continue;
            }
        }
        out.push_str(line);
        out.push_str("\r\n");
    }
    out.push_str("Connection: close\r\n\r\n");
    out.into_bytes()
}

/// Dial the target. When `addrs` is non-empty these are the ONLY addresses
/// that may be used — they are what Core resolved and screened, and no second
/// lookup happens anywhere on this path.
///
/// An empty `addrs` means the gate approved without a lookup: either the host
/// IS an IP literal (so dialling by name performs no DNS), or it is a
/// canonical loopback name under an authorising gh#911 grant, which the
/// operator asked for by port.
async fn dial(host: &str, port: u16, addrs: &[IpAddr]) -> io::Result<TcpStream> {
    if addrs.is_empty() {
        return match tokio::time::timeout(CONNECT_TIMEOUT, TcpStream::connect((host, port))).await {
            Ok(r) => r,
            Err(_) => Err(io::Error::new(io::ErrorKind::TimedOut, "connect timed out")),
        };
    }
    let mut last: Option<io::Error> = None;
    for ip in addrs {
        let attempt = tokio::time::timeout(
            CONNECT_TIMEOUT,
            TcpStream::connect(SocketAddr::new(*ip, port)),
        )
        .await;
        match attempt {
            Ok(Ok(stream)) => return Ok(stream),
            Ok(Err(e)) => last = Some(e),
            Err(_) => last = Some(io::Error::new(io::ErrorKind::TimedOut, "connect timed out")),
        }
    }
    Err(last
        .unwrap_or_else(|| io::Error::new(io::ErrorKind::AddrNotAvailable, "no screened address")))
}

async fn refuse(client: &mut TcpStream, status: u16, phrase: &str, reason: &str) -> io::Result<()> {
    let body = format!("Wayland browser policy refused this request: {reason}\n");
    let head = format!(
        "HTTP/1.1 {status} {phrase}\r\n\
         Content-Type: text/plain; charset=utf-8\r\n\
         Content-Length: {}\r\n\
         Connection: close\r\n\r\n",
        body.len()
    );
    client.write_all(head.as_bytes()).await?;
    client.write_all(body.as_bytes()).await?;
    client.shutdown().await
}

fn find(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack
        .windows(needle.len())
        .position(|window| window == needle)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn connect_authority_becomes_a_gateable_url() {
        assert_eq!(
            connect_authority_url("example.com:443").as_deref(),
            Some("https://example.com:443/")
        );
        // The port is preserved, not normalised away: the gh#911 grant is
        // port-scoped and 3000 must not arrive at the gate as 443.
        assert_eq!(
            connect_authority_url("localhost:3000").as_deref(),
            Some("https://localhost:3000/")
        );
        assert_eq!(connect_authority_url("example.com").as_deref(), None);
        assert_eq!(connect_authority_url("example.com:http").as_deref(), None);
        assert_eq!(connect_authority_url(":443").as_deref(), None);
    }

    #[test]
    fn forwarded_head_forces_one_request_per_connection() {
        let head = b"GET http://example.com/ HTTP/1.1\r\n\
                     Host: example.com\r\n\
                     Proxy-Connection: keep-alive\r\n\
                     Connection: keep-alive\r\n\
                     Keep-Alive: timeout=5\r\n\r\n";
        let out = String::from_utf8(close_framed_head(head)).unwrap();
        assert!(out.starts_with("GET http://example.com/ HTTP/1.1\r\n"));
        assert!(out.contains("Host: example.com\r\n"));
        assert!(
            !out.to_ascii_lowercase().contains("keep-alive"),
            "a reusable connection lets a SECOND request to a DIFFERENT host \
             ride the first one's approval; got {out:?}"
        );
        assert!(out.ends_with("Connection: close\r\n\r\n"), "got {out:?}");
    }

    #[test]
    fn head_scanner_finds_the_terminator_across_chunk_boundaries() {
        assert_eq!(find(b"abc\r\n\r\ndef", b"\r\n\r\n"), Some(3));
        assert_eq!(find(b"abc\r\n", b"\r\n\r\n"), None);
    }
}
