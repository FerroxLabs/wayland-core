//! gh#1117 — THE ADVERSARIAL NAMESERVER the definition of done names.
//!
//! The issue asks for "an adversarial nameserver answering differently for
//! Core and for the sidecar, with the navigation still refused". Until now
//! that was answered with a structural argument — *Core resolves once and
//! dials that address, so there is no second answer to serve* — and an
//! argument is not a measurement. This file is the measurement.
//!
//! ## What is adversarial about it
//!
//! [`AdversarialZone`] is a real authoritative DNS server: real UDP, real
//! wire format, TTL=0 on every answer. It serves `rebind.wayland-test.invalid`
//! with a **different address every time it is asked**:
//!
//!   * query 1 → `203.0.113.7`, a public address that clears the block list;
//!   * query 2 and after → `127.0.0.1`, the loopback service standing in for
//!     "something on this machine the agent must not reach".
//!
//! That is a TTL=0 intra-navigation rebind, served by the zone's owner, with
//! no cache anywhere to blunt it. Anything that looks the name up twice gets
//! the internal address the second time.
//!
//! ## The verdict it produces
//!
//! Three things are asserted, and each one fails on its own:
//!
//!   1. the zone was asked **exactly once** for the name — a second lookup
//!      anywhere on the path makes this 2;
//!   2. the address the proxy actually opened a connection to is the one the
//!      gate screened, and is not loopback;
//!   3. the loopback service received **nothing**.
//!
//! Both controls are in the test rather than in this comment: a name the zone
//! poisons from the FIRST answer is refused (so the zone is really being
//! consulted and the gate really acts on what it says), and the loopback
//! service's counter is shown to move when something does connect (so "it
//! received nothing" is a measurement and not a broken counter).

use std::io::{Read, Write};
use std::net::{IpAddr, Ipv4Addr, SocketAddr, TcpStream as StdTcpStream, UdpSocket};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, OnceLock};
use std::time::Duration;

use wcore_browser::egress_proxy::PolicyEgressProxy;
use wcore_browser::policy::{BrowserPolicy, PolicyAction};

/// Rebinds on the second answer.
const REBIND_NAME: &str = "rebind.wayland-test.invalid";
/// Poisoned from the first answer — the control.
const POISONED_NAME: &str = "poisoned.wayland-test.invalid";
/// The answer that clears the block list. TEST-NET-3 (RFC 5737): routable
/// nowhere, so an approved dial to it can never accidentally reach anything,
/// while the gate has no reason to refuse it.
const PUBLIC_ANSWER: Ipv4Addr = Ipv4Addr::new(203, 0, 113, 7);

/// Port of the running zone. The policy's resolver seam is a plain `fn`
/// pointer with nowhere to put state, so the zone publishes its port here.
static ZONE_PORT: OnceLock<u16> = OnceLock::new();
/// Per-name query counters, in the same static for the same reason.
static REBIND_QUERIES: AtomicU64 = AtomicU64::new(0);
static POISONED_QUERIES: AtomicU64 = AtomicU64::new(0);

// ── the zone ────────────────────────────────────────────────────────────────

/// Start the authoritative server. Idempotent across tests in this binary:
/// one zone, one port, counters read as deltas.
fn start_zone() -> u16 {
    *ZONE_PORT.get_or_init(|| {
        let socket = UdpSocket::bind((Ipv4Addr::LOCALHOST, 0)).expect("bind zone");
        let port = socket.local_addr().unwrap().port();
        std::thread::spawn(move || {
            let mut buf = [0u8; 512];
            while let Ok((len, peer)) = socket.recv_from(&mut buf) {
                let Some((id, name, question_end)) = parse_query(&buf[..len]) else {
                    continue;
                };
                let answer = answer_for(&name);
                let response = build_response(id, &buf[12..question_end], answer);
                let _ = socket.send_to(&response, peer);
            }
        });
        port
    })
}

/// The zone's policy: what this name resolves to on THIS query.
fn answer_for(name: &str) -> Ipv4Addr {
    if name.eq_ignore_ascii_case(POISONED_NAME) {
        POISONED_QUERIES.fetch_add(1, Ordering::SeqCst);
        return Ipv4Addr::LOCALHOST;
    }
    if name.eq_ignore_ascii_case(REBIND_NAME) {
        let nth = REBIND_QUERIES.fetch_add(1, Ordering::SeqCst);
        // FIRST answer public, every answer after it internal. This is the
        // whole adversary.
        return if nth == 0 {
            PUBLIC_ANSWER
        } else {
            Ipv4Addr::LOCALHOST
        };
    }
    PUBLIC_ANSWER
}

/// Returns `(id, qname, offset just past the question section)`.
fn parse_query(packet: &[u8]) -> Option<(u16, String, usize)> {
    if packet.len() < 12 {
        return None;
    }
    let id = u16::from_be_bytes([packet[0], packet[1]]);
    let mut i = 12;
    let mut labels: Vec<String> = Vec::new();
    loop {
        let len = *packet.get(i)? as usize;
        i += 1;
        if len == 0 {
            break;
        }
        let label = packet.get(i..i + len)?;
        labels.push(String::from_utf8_lossy(label).into_owned());
        i += len;
    }
    // QTYPE + QCLASS
    i += 4;
    if i > packet.len() {
        return None;
    }
    Some((id, labels.join("."), i))
}

fn build_response(id: u16, question: &[u8], answer: Ipv4Addr) -> Vec<u8> {
    let mut out = Vec::with_capacity(64);
    out.extend_from_slice(&id.to_be_bytes());
    out.extend_from_slice(&0x8480u16.to_be_bytes()); // response, authoritative
    out.extend_from_slice(&1u16.to_be_bytes()); // QDCOUNT
    out.extend_from_slice(&1u16.to_be_bytes()); // ANCOUNT
    out.extend_from_slice(&0u16.to_be_bytes()); // NSCOUNT
    out.extend_from_slice(&0u16.to_be_bytes()); // ARCOUNT
    out.extend_from_slice(question);
    out.extend_from_slice(&0xC00Cu16.to_be_bytes()); // pointer to the QNAME
    out.extend_from_slice(&1u16.to_be_bytes()); // A
    out.extend_from_slice(&1u16.to_be_bytes()); // IN
    out.extend_from_slice(&0u32.to_be_bytes()); // TTL=0 — rebind at will
    out.extend_from_slice(&4u16.to_be_bytes()); // RDLENGTH
    out.extend_from_slice(&answer.octets());
    out
}

// ── the resolver the policy uses ────────────────────────────────────────────

/// Ask the adversarial zone, the way a stub resolver would ask the
/// authoritative server for that name.
fn zone_resolver(host: &str) -> Vec<IpAddr> {
    let Some(port) = ZONE_PORT.get() else {
        return Vec::new();
    };
    let socket = match UdpSocket::bind((Ipv4Addr::LOCALHOST, 0)) {
        Ok(s) => s,
        Err(_) => return Vec::new(),
    };
    let _ = socket.set_read_timeout(Some(Duration::from_secs(2)));
    let mut query = Vec::with_capacity(64);
    query.extend_from_slice(&0x4242u16.to_be_bytes());
    query.extend_from_slice(&0x0100u16.to_be_bytes());
    query.extend_from_slice(&1u16.to_be_bytes());
    query.extend_from_slice(&[0, 0, 0, 0, 0, 0]);
    for label in host.split('.') {
        query.push(label.len() as u8);
        query.extend_from_slice(label.as_bytes());
    }
    query.push(0);
    query.extend_from_slice(&1u16.to_be_bytes());
    query.extend_from_slice(&1u16.to_be_bytes());
    if socket
        .send_to(&query, (Ipv4Addr::LOCALHOST, *port))
        .is_err()
    {
        return Vec::new();
    }
    let mut buf = [0u8; 512];
    let Ok((len, _)) = socket.recv_from(&mut buf) else {
        return Vec::new();
    };
    parse_first_a(&buf[..len]).into_iter().collect()
}

fn parse_first_a(packet: &[u8]) -> Option<IpAddr> {
    let (_, _, question_end) = parse_query(packet)?;
    let rdata = packet.get(question_end + 12..question_end + 16)?;
    Some(IpAddr::V4(Ipv4Addr::new(
        rdata[0], rdata[1], rdata[2], rdata[3],
    )))
}

// ── the loopback service the second answer points at ────────────────────────

struct Victim {
    port: u16,
    hits: Arc<AtomicU64>,
}

async fn spawn_victim() -> Victim {
    let listener = tokio::net::TcpListener::bind((Ipv4Addr::LOCALHOST, 0))
        .await
        .unwrap();
    let port = listener.local_addr().unwrap().port();
    let hits = Arc::new(AtomicU64::new(0));
    let server_hits = Arc::clone(&hits);
    tokio::spawn(async move {
        while let Ok((mut stream, _)) = listener.accept().await {
            server_hits.fetch_add(1, Ordering::SeqCst);
            tokio::spawn(async move {
                use tokio::io::AsyncWriteExt;
                let _ = stream
                    .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 5\r\n\r\nPWNED")
                    .await;
            });
        }
    });
    Victim { port, hits }
}

// ── driving the proxy ───────────────────────────────────────────────────────

/// Send one absolute-form proxy request and read whatever comes back within
/// `budget`. A dial that hangs (the public answer routes nowhere) is expected
/// and is not what is being measured, so the read is bounded rather than
/// waited out.
fn proxy_request(proxy_port: u16, url: &str, budget: Duration) -> String {
    let mut stream =
        StdTcpStream::connect((Ipv4Addr::LOCALHOST, proxy_port)).expect("connect to proxy");
    let host = url.split('/').nth(2).unwrap_or_default();
    let request = format!("GET {url} HTTP/1.1\r\nHost: {host}\r\n\r\n");
    stream.write_all(request.as_bytes()).expect("write request");
    let _ = stream.set_read_timeout(Some(budget));
    let mut buf = Vec::new();
    let mut chunk = [0u8; 4096];
    loop {
        match stream.read(&mut chunk) {
            Ok(0) => break,
            Ok(n) => buf.extend_from_slice(&chunk[..n]),
            Err(_) => break,
        }
    }
    String::from_utf8_lossy(&buf).into_owned()
}

fn gate_policy() -> BrowserPolicy {
    BrowserPolicy::new(PolicyAction::Allow, vec![], vec![])
        .with_resolver(zone_resolver)
        .address_gate_only()
}

// ── the test ────────────────────────────────────────────────────────────────

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_adversarial_nameserver_gets_exactly_one_answer_into_the_dial() {
    start_zone();
    let victim = spawn_victim().await;
    let proxy = PolicyEgressProxy::start(gate_policy()).await.unwrap();

    // ── CONTROL 1: the zone is real and the gate acts on what it says.
    //    A name poisoned from the FIRST answer must be refused, naming
    //    loopback. If this fails, nothing below is a measurement — it would
    //    mean the resolver seam is not wired to the zone at all.
    let poisoned_before = POISONED_QUERIES.load(Ordering::SeqCst);
    let refused = proxy_request(
        proxy.port(),
        &format!("http://{POISONED_NAME}:{}/", victim.port),
        Duration::from_secs(5),
    );
    assert!(
        refused.starts_with("HTTP/1.1 403"),
        "CONTROL FAILED — a name the zone resolves to 127.0.0.1 was not \
         refused: {refused:?}"
    );
    assert!(
        refused.contains("loopback"),
        "CONTROL FAILED — refused, but not for the reason the zone created: \
         {refused:?}"
    );
    assert!(
        POISONED_QUERIES.load(Ordering::SeqCst) > poisoned_before,
        "CONTROL FAILED — the zone was never asked, so the refusal above came \
         from something other than DNS"
    );
    assert_eq!(
        victim.hits.load(Ordering::SeqCst),
        0,
        "the refused request still reached the loopback service"
    );

    // ── THE MEASUREMENT. One navigation for a name that rebinds on the
    //    second answer. The gate screens answer 1 (public) and the proxy
    //    dials THAT. Answer 2 (loopback) is never asked for, so it can never
    //    be dialled.
    let queries_before = REBIND_QUERIES.load(Ordering::SeqCst);
    let dials_before = proxy.dialled_addrs().len();
    let _ = proxy_request(
        proxy.port(),
        &format!("http://{REBIND_NAME}:{}/", victim.port),
        Duration::from_secs(3),
    );

    let queries = REBIND_QUERIES.load(Ordering::SeqCst) - queries_before;
    assert_eq!(
        queries, 1,
        "the adversarial zone was asked {queries} times for {REBIND_NAME}; \
         every answer after the first is 127.0.0.1, so any number but 1 is a \
         window the zone owner can rebind through"
    );

    let dialled: Vec<SocketAddr> = proxy.dialled_addrs().split_off(dials_before);
    assert_eq!(
        dialled,
        vec![SocketAddr::from((PUBLIC_ANSWER, victim.port))],
        "the proxy did not dial the address the gate screened"
    );
    assert!(
        !dialled.iter().any(|addr| addr.ip().is_loopback()),
        "the proxy dialled loopback for a name the gate approved as public"
    );

    assert_eq!(
        victim.hits.load(Ordering::SeqCst),
        0,
        "the adversarial zone moved the connection onto the loopback service \
         — gh#1117 is open"
    );

    // ── CONTROL 2 for that last assertion: the counter CAN move. Without
    //    this, a victim that never accepts anything would report "0 hits"
    //    forever and the assertion above would be unfalsifiable.
    let _ = StdTcpStream::connect((Ipv4Addr::LOCALHOST, victim.port)).expect("connect to victim");
    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    while victim.hits.load(Ordering::SeqCst) == 0 && std::time::Instant::now() < deadline {
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    assert_eq!(
        victim.hits.load(Ordering::SeqCst),
        1,
        "CONTROL FAILED — the loopback service does not count connections, so \
         'it received nothing' above proved nothing"
    );

    proxy.shutdown();
}
