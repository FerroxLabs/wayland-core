//! Live proof for the SMTP reply leg over a PRIVATE certificate chain.
//!
//! A real STARTTLS SMTP relay is stood up on a loopback socket presenting a
//! certificate signed by a throwaway CA that no public trust store knows. The
//! same `LettreSender` is then driven against it twice, changing exactly one
//! variable — whether `tls_root_cert_path` is wired:
//!
//! | run | `tls_root_cert_path` | expected |
//! |-----|----------------------|----------|
//! | 1 | `Some(ca.pem)` | message delivered, relay count goes 0 -> 1 |
//! | 2 | `None`         | send fails at TLS, relay count stays 1 |
//!
//! **Run 2 is the point.** It is the negative control that makes run 1 mean
//! something, and it is what a self-passing gate would not have. Two distinct
//! failure modes are caught by it:
//!
//! * If the wiring were dead — the shape this test exists to close, where the
//!   knob is passed but never reaches the transport — run 1 would fail.
//! * If the leg had instead been "fixed" with `accept_invalid_certs`, run 2
//!   would **succeed**, because verification would be off for everyone. So the
//!   assertion that run 2 FAILS is a positive, executable proof that
//!   certificate verification is still enabled. That is the whole difference
//!   between adding a trust anchor and disabling trust, and it is asserted here
//!   rather than asserted in a comment.
//!
//! Nothing here is `#[ignore]`d, env-gated, or dependent on an external host:
//! the CA, the relay and the socket are all created by the test, so it runs in
//! a normal `cargo test` and its count is visible in the summary line.
//!
//! The relay speaks only the subset of ESMTP lettre uses (EHLO / STARTTLS /
//! AUTH PLAIN / MAIL / RCPT / DATA / QUIT). It is a test fixture, not an MTA.

use std::io::Write as _;
use std::sync::{Arc, Mutex};

use lettre::Message;
use rcgen::{
    BasicConstraints, CertificateParams, CertifiedIssuer, DnType, IsCa, KeyPair, KeyUsagePurpose,
};
use rustls::ServerConfig;
use rustls::pki_types::{CertificateDer, PrivateKeyDer};
use tokio::io::{AsyncBufReadExt, AsyncRead, AsyncWrite, AsyncWriteExt, BufReader};
use tokio::net::{TcpListener, TcpStream};
use tokio_rustls::TlsAcceptor;

use wcore_channel_email::{LettreSender, MailSecurity, MailSender};

/// Body marker asserted in the delivered message, so a "delivered" claim is
/// tied to *this* send rather than to any traffic reaching the relay.
const MARKER: &str = "wayland-core-smtp-tls-root-proof-8f31c2";

// ---------------------------------------------------------------------------
// throwaway PKI
// ---------------------------------------------------------------------------

/// Build a CA and a relay certificate signed by it for `127.0.0.1`.
///
/// The SAN is the IP literal rather than `localhost` deliberately: `localhost`
/// can resolve to `::1` before `127.0.0.1`, which would make this test's
/// failure mode "connection refused" instead of the TLS outcome under test —
/// an unrelated red that would look like a real one.
fn make_pki() -> (String, Vec<CertificateDer<'static>>, PrivateKeyDer<'static>) {
    let ca_key = KeyPair::generate().expect("generate CA key");
    let mut ca_params = CertificateParams::new(Vec::<String>::new()).expect("CA params");
    ca_params.is_ca = IsCa::Ca(BasicConstraints::Unconstrained);
    ca_params.key_usages = vec![KeyUsagePurpose::KeyCertSign, KeyUsagePurpose::CrlSign];
    ca_params
        .distinguished_name
        .push(DnType::CommonName, "wayland-core email test CA");
    let ca = CertifiedIssuer::self_signed(ca_params, ca_key).expect("self-sign CA");

    let relay_key = KeyPair::generate().expect("generate relay key");
    let mut relay_params =
        CertificateParams::new(vec!["127.0.0.1".to_string()]).expect("relay params");
    relay_params
        .distinguished_name
        .push(DnType::CommonName, "wayland-core email test relay");
    let relay_cert = relay_params
        .signed_by(&relay_key, &ca)
        .expect("sign relay cert");

    let chain = vec![relay_cert.der().clone()];
    let key = PrivateKeyDer::Pkcs8(relay_key.serialize_der().into());
    (ca.pem(), chain, key)
}

// ---------------------------------------------------------------------------
// minimal STARTTLS SMTP relay
// ---------------------------------------------------------------------------

/// Messages the relay accepted through to `250 Ok`, in arrival order.
type Received = Arc<Mutex<Vec<String>>>;

async fn serve(listener: TcpListener, acceptor: TlsAcceptor, received: Received) {
    loop {
        let Ok((sock, _peer)) = listener.accept().await else {
            return;
        };
        let acceptor = acceptor.clone();
        let received = Arc::clone(&received);
        tokio::spawn(async move {
            // A connection that dies mid-handshake is an expected outcome here
            // (that is precisely run 2), so errors are dropped rather than
            // panicking a background task.
            let _ = handle_plaintext(sock, acceptor, received).await;
        });
    }
}

/// Pre-STARTTLS phase. Advertises STARTTLS, then hands the socket to TLS.
async fn handle_plaintext(
    sock: TcpStream,
    acceptor: TlsAcceptor,
    received: Received,
) -> std::io::Result<()> {
    let (r, mut w) = tokio::io::split(sock);
    let mut reader = BufReader::new(r);
    w.write_all(b"220 wayland-test ESMTP\r\n").await?;

    let mut line = String::new();
    loop {
        line.clear();
        if reader.read_line(&mut line).await? == 0 {
            return Ok(());
        }
        let verb = line.trim_end().to_ascii_uppercase();
        if verb.starts_with("EHLO") || verb.starts_with("HELO") {
            w.write_all(b"250-wayland-test\r\n250-STARTTLS\r\n250 AUTH PLAIN LOGIN\r\n")
                .await?;
        } else if verb.starts_with("STARTTLS") {
            w.write_all(b"220 2.0.0 Ready to start TLS\r\n").await?;
            let sock = reader.into_inner().unsplit(w);
            let tls = acceptor.accept(sock).await?;
            return handle_tls(tls, received).await;
        } else if verb.starts_with("QUIT") {
            w.write_all(b"221 2.0.0 Bye\r\n").await?;
            return Ok(());
        } else {
            w.write_all(b"502 5.5.1 not implemented\r\n").await?;
        }
    }
}

/// Post-STARTTLS phase. Records each fully-accepted DATA payload.
async fn handle_tls<S>(stream: S, received: Received) -> std::io::Result<()>
where
    S: AsyncRead + AsyncWrite + Unpin + Send,
{
    let (r, mut w) = tokio::io::split(stream);
    let mut reader = BufReader::new(r);

    let mut line = String::new();
    loop {
        line.clear();
        if reader.read_line(&mut line).await? == 0 {
            return Ok(());
        }
        let verb = line.trim_end().to_ascii_uppercase();
        if verb.starts_with("EHLO") || verb.starts_with("HELO") {
            w.write_all(b"250-wayland-test\r\n250 AUTH PLAIN LOGIN\r\n")
                .await?;
        } else if verb.starts_with("AUTH") {
            w.write_all(b"235 2.7.0 Authentication successful\r\n")
                .await?;
        } else if verb.starts_with("MAIL FROM") {
            w.write_all(b"250 2.1.0 Ok\r\n").await?;
        } else if verb.starts_with("RCPT TO") {
            w.write_all(b"250 2.1.5 Ok\r\n").await?;
        } else if verb.starts_with("DATA") {
            w.write_all(b"354 End data with <CR><LF>.<CR><LF>\r\n")
                .await?;
            let mut body = String::new();
            loop {
                let mut chunk = String::new();
                if reader.read_line(&mut chunk).await? == 0 {
                    return Ok(());
                }
                if chunk.trim_end() == "." {
                    break;
                }
                body.push_str(&chunk);
            }
            received.lock().expect("relay mutex").push(body);
            w.write_all(b"250 2.0.0 Ok: queued as TESTQ1\r\n").await?;
        } else if verb.starts_with("QUIT") {
            w.write_all(b"221 2.0.0 Bye\r\n").await?;
            return Ok(());
        } else {
            w.write_all(b"502 5.5.1 not implemented\r\n").await?;
        }
    }
}

fn probe_message() -> Message {
    Message::builder()
        .from("bot@acme.test".parse().expect("from"))
        .to("ops@acme.test".parse().expect("to"))
        .subject("tls root proof")
        .body(MARKER.to_string())
        .expect("build message")
}

// ---------------------------------------------------------------------------
// the proof
// ---------------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn smtp_reaches_private_chain_relay_only_when_root_is_wired() {
    let _ = rustls::crypto::ring::default_provider().install_default();

    let (ca_pem, chain, key) = make_pki();
    let mut ca_file = tempfile::NamedTempFile::new().expect("temp CA file");
    ca_file.write_all(ca_pem.as_bytes()).expect("write CA pem");
    ca_file.flush().expect("flush CA pem");
    let ca_path = ca_file.path().to_str().expect("utf8 CA path").to_string();

    let server_cfg = ServerConfig::builder()
        .with_no_client_auth()
        .with_single_cert(chain, key)
        .expect("relay server config");
    let acceptor = TlsAcceptor::from(Arc::new(server_cfg));

    let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind relay");
    let port = listener.local_addr().expect("relay addr").port();
    let received: Received = Arc::new(Mutex::new(Vec::new()));
    tokio::spawn(serve(listener, acceptor, Arc::clone(&received)));

    // Guard against the whole test passing on an inert relay: nothing has been
    // delivered before we send anything.
    assert_eq!(
        received.lock().unwrap().len(),
        0,
        "relay must start empty, else the delivered-count assertion below proves nothing"
    );

    // ---- run 1: root wired -> delivery ----------------------------------
    let wired = LettreSender::new(
        "127.0.0.1",
        port,
        "user".to_string(),
        "pass".to_string(),
        Some(&ca_path),
        // Explicit: this relay is a real STARTTLS host that happens to listen
        // on loopback, so the Auto loopback-plaintext exemption must not apply
        // to it. Naming the mode is exactly what an operator would do here.
        MailSecurity::Starttls,
    )
    .expect("build sender with root");

    let wired_result = wired.send(probe_message()).await;
    assert!(
        wired_result.is_ok(),
        "send with tls_root_cert_path wired must reach the relay, got: {:?}",
        wired_result.err()
    );

    let after_wired = received.lock().unwrap().len();
    assert_eq!(
        after_wired, 1,
        "relay must have accepted exactly one message with the root wired"
    );
    let delivered = received.lock().unwrap()[0].clone();
    assert!(
        delivered.contains(MARKER),
        "delivered payload must be the message this test sent; got:\n{delivered}"
    );

    // ---- run 2: no root -> TLS refusal (the negative control) ------------
    let bare = LettreSender::new(
        "127.0.0.1",
        port,
        "user".to_string(),
        "pass".to_string(),
        None,
        MailSecurity::Starttls,
    )
    .expect("build sender without root");

    let bare_result = bare.send(probe_message()).await;
    assert!(
        bare_result.is_err(),
        "send WITHOUT the root must be refused at TLS. Succeeding here would mean \
         certificate verification is disabled for every caller — the exact regression \
         this crate refuses to ship — or that the relay is not actually verifying."
    );

    assert_eq!(
        received.lock().unwrap().len(),
        1,
        "the unrooted send must not have been delivered; relay count must still be 1"
    );

    // Surfaced so the live transcript records the concrete refusal rather than
    // just a boolean.
    println!(
        "negative control refused as expected: {}",
        bare_result.unwrap_err()
    );
}
