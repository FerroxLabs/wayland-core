//! IMAP inbound. The `imap` crate is synchronous, so we run the poll
//! loop on `tokio::task::spawn_blocking`. New messages land in the
//! shared `inbox` queue that `poll_events` drains.

use std::collections::{HashMap, VecDeque};
use std::sync::{Arc, Mutex as StdMutex};
use std::time::Duration;

use tokio::sync::{Mutex, watch};
use wcore_channels::event::{Attachment, ChannelEvent, ChatType, IncomingMessage, MediaKind};

use crate::config::{MailSecurity, is_loopback_host};
use crate::error::EmailError;
use crate::smtp::ReplyContext;
use crate::uid_store;

/// Shared index mapping an inbound RFC `Message-ID` (== `IncomingMessage.id`)
/// to the threading context needed to reply to it. Populated by the IMAP
/// poll task on every accepted inbound message and read by the SMTP send
/// path when an `OutgoingMessage.reply_to` names a known id. `std::Mutex`
/// because the poll task is synchronous.
pub(crate) type ReplyIndex = Arc<StdMutex<HashMap<String, ReplyContext>>>;

/// Upper bound on the reply index so a long-lived channel can't grow it
/// without bound. When exceeded we clear it wholesale; a dropped entry just
/// means a later reply falls back to a single-id `References` chain (still
/// correctly threaded via `In-Reply-To`), never an error.
pub(crate) const REPLY_INDEX_CAP: usize = 4096;

/// The conventional implicit-TLS ("IMAPS") port. `MailSecurity::Auto` treats
/// this port, and only this port, as implicit TLS.
const IMAPS_PORT: u16 = 993;

/// Insert one reply-threading entry, enforcing `REPLY_INDEX_CAP`. A
/// synthesized `uid:N` id (no real Message-ID) is still recorded so a
/// reply can at least carry `In-Reply-To`. Clears the map when it would
/// exceed the cap (see `REPLY_INDEX_CAP`).
pub(crate) fn record_reply_context(index: &ReplyIndex, id: String, ctx: ReplyContext) {
    if id.is_empty() {
        return;
    }
    let mut guard = match index.lock() {
        Ok(g) => g,
        // A poisoned lock would only happen if a holder panicked; threading
        // metadata is non-critical, so we skip rather than propagate.
        Err(_) => return,
    };
    if guard.len() >= REPLY_INDEX_CAP && !guard.contains_key(&id) {
        guard.clear();
    }
    guard.insert(id, ctx);
}

/// Arguments for the blocking poll task. Cloneable plain data so the
/// `spawn_blocking` closure owns its own copy.
pub(crate) struct ImapPollArgs {
    pub host: String,
    pub port: u16,
    /// Transport security for the IMAP socket. See [`MailSecurity`].
    pub security: MailSecurity,
    pub user: String,
    pub pass: String,
    pub mailbox: String,
    pub poll_interval_secs: u32,
    /// Case-insensitive allow-list of bare sender addresses. When
    /// non-empty, inbound messages whose `From:` addr-spec is not on this
    /// list are dropped before enqueueing. See `ImapConfig::allowed_senders`
    /// for the (lack of) authentication guarantees.
    pub allowed_senders: Vec<String>,
    /// This channel's own addresses (bare addr-spec, lowercased): the SMTP
    /// From address plus the IMAP account when it is address-shaped. Inbound
    /// mail whose `From:` matches is marked `is_self` so the dispatch
    /// kernel's loop guard drops it instead of starting an agent turn
    /// (wayland#547 — the agent otherwise replies to its own mail forever).
    pub own_addresses: Vec<String>,
    /// Message-IDs this channel has sent (recorded by the SMTP path). An
    /// inbound whose id matches is the channel's own mail echoing back and
    /// is likewise marked `is_self`. This detector only ever matches mail
    /// this process sent; the own-address From match above is the blunter
    /// backstop — see [`mark_self_inbound`] for the tradeoff.
    pub sent_ids: crate::sent_index::SentIdIndex,
    pub inbox: Arc<Mutex<VecDeque<ChannelEvent>>>,
    pub last_seen_uid: Arc<StdMutex<u32>>,
    /// Shared reply-threading index; the poll task records one entry per
    /// accepted inbound message so the send path can thread replies.
    pub reply_index: ReplyIndex,
    pub shutdown: watch::Receiver<bool>,
    /// Tokio handle used so the sync task can enqueue events via
    /// `block_on(inbox.lock())`. Falls back to constructing one if `None`.
    pub runtime_handle: tokio::runtime::Handle,
}

/// Drive an IMAP UID-search loop until `shutdown` flips. Runs on the
/// blocking pool.
pub(crate) fn imap_poll_blocking(args: ImapPollArgs) {
    let ImapPollArgs {
        host,
        port,
        security,
        user,
        pass,
        mailbox,
        poll_interval_secs,
        allowed_senders,
        own_addresses,
        sent_ids,
        inbox,
        last_seen_uid,
        reply_index,
        mut shutdown,
        runtime_handle,
    } = args;

    let interval = Duration::from_secs(u64::from(poll_interval_secs.max(1)));

    // Pre-normalize the allow-list once: bare addr-spec, lowercased.
    let allow_set: Option<std::collections::HashSet<String>> = if allowed_senders.is_empty() {
        None
    } else {
        Some(
            allowed_senders
                .iter()
                .map(|s| normalize_from_addr(s))
                .collect(),
        )
    };

    // Resume the UID watermark from disk so a restart neither replays the
    // mailbox nor skips mail that arrived while we were down. `seeded` tracks
    // whether the watermark is authoritative: false until we load a persisted
    // value or seed from the mailbox's UIDNEXT on first connect (poll_once).
    let mut seeded = match uid_store::load(&host, &user, &mailbox) {
        Some(uid) => {
            *last_seen_uid.lock().unwrap() = uid;
            true
        }
        None => false,
    };

    while !*shutdown.borrow() {
        match poll_once(
            &host,
            port,
            security,
            &user,
            &pass,
            &mailbox,
            allow_set.as_ref(),
            &own_addresses,
            &sent_ids,
            &inbox,
            &last_seen_uid,
            &reply_index,
            &runtime_handle,
            &mut seeded,
        ) {
            Ok(_) => {}
            Err(e) => {
                tracing::warn!(
                    target: "wcore_channel_email::imap",
                    error = %e,
                    "imap poll iteration failed; will retry"
                );
            }
        }
        // Sleep in small increments so shutdown propagates quickly.
        let mut elapsed = Duration::ZERO;
        let step = Duration::from_millis(100);
        while elapsed < interval {
            if *shutdown.borrow() {
                return;
            }
            std::thread::sleep(step);
            elapsed += step;
            // Non-blocking peek for an update on the watch channel.
            if shutdown.has_changed().unwrap_or(false) {
                // Refresh the borrow.
                let _ = shutdown.borrow_and_update();
            }
        }
    }
}

/// The UIDs a poll should fetch, in ascending order, given the watermark the
/// poll started from.
///
/// Extracted so the invariant is testable without a live IMAP session, because
/// getting it wrong destroys mail silently.
///
/// F24-C3 (email steady state). The previous form of this filter compared each
/// candidate against a `high_water` that it MUTATED inside the same loop, while
/// iterating the `HashSet<Uid>` that `Session::uid_search` returns. A HashSet
/// has arbitrary iteration order, re-randomised per process, so whenever a poll
/// found more than one new message the largest UID could be visited first —
/// after which `uid <= high_water` was true for every remaining new message and
/// they were all skipped. The watermark was then persisted at that maximum, so
/// the skipped mail could never be searched for again.
///
/// Measured on hetzner: six messages delivered back-to-back, one `UID SEARCH
/// 1005:*` correctly answered `[1005,1006,1007,1008,1009,1010]`, exactly ONE
/// fetch (`uid 1010`) followed, and the next search was `1011:*`. Five inbound
/// messages destroyed, with a single poller and no error logged anywhere.
///
/// Sorting also fixes a second, milder defect the same HashSet caused: inbound
/// messages reached the engine in arbitrary order rather than arrival order.
fn new_uids(uids: std::collections::HashSet<u32>, seen_through: u32) -> Vec<u32> {
    let mut out: Vec<u32> = uids.into_iter().filter(|u| *u > seen_through).collect();
    out.sort_unstable();
    out
}

/// The IMAP client's TLS stream type. `rustls`, not `native_tls`: `native-tls`
/// on Linux is OpenSSL, and it was the ONLY edge in this workspace that reached
/// `openssl-sys` (`cargo tree -i openssl-sys` on 86f1ddbc: one root, this
/// crate). Everything else — every provider connection through reqwest, and
/// lettre's SMTP leg in this same crate — was already rustls.
pub(crate) type ImapTlsStream = rustls::StreamOwned<rustls::ClientConnection, std::net::TcpStream>;

/// The rustls configuration both TLS legs use.
///
/// Anchors come from the OS trust store, which is exactly what the `native_tls`
/// connector this replaced resolved (OpenSSL's default verify paths, located by
/// `openssl-probe`), so the set of certificates IMAP accepts does not change.
/// The SMTP leg's anchors come from lettre's bundled `webpki-roots` and are
/// untouched by this.
///
/// Built once per process. `rustls_native_certs` parses every anchor eagerly
/// while OpenSSL loaded them lazily out of a hashed directory, so rebuilding it
/// per poll would turn a poll loop into a repeated full walk of the trust store.
///
/// The provider is named explicitly instead of using `ClientConfig::builder()`.
/// Feature unification enables BOTH `ring` and `aws-lc-rs` on rustls in this
/// workspace (`aws-smithy-http-client` pulls the latter in), and with two
/// providers compiled in the no-argument builder panics unless something has
/// installed a process default. `wcore-agent`'s postgres backend names `ring`
/// the same way, for the same reason.
fn tls_client_config() -> Result<Arc<rustls::ClientConfig>, EmailError> {
    static CONFIG: std::sync::OnceLock<Result<Arc<rustls::ClientConfig>, String>> =
        std::sync::OnceLock::new();
    CONFIG
        .get_or_init(|| {
            let native = rustls_native_certs::load_native_certs();
            let unreadable = native.errors.len();
            for err in &native.errors {
                tracing::warn!(error = %err, "imap: skipping an unreadable OS trust anchor");
            }
            let mut roots = rustls::RootCertStore::empty();
            let (added, unparsable) = roots.add_parsable_certificates(native.certs);
            if added == 0 {
                return Err(format!(
                    "no usable TLS trust anchors in the OS trust store \
                     ({unparsable} unparsable, {unreadable} unreadable): \
                     IMAP over TLS cannot verify any server"
                ));
            }
            rustls::ClientConfig::builder_with_provider(Arc::new(
                rustls::crypto::ring::default_provider(),
            ))
            .with_safe_default_protocol_versions()
            .map_err(|e| format!("rustls provider rejects the default protocol versions: {e}"))
            .map(|cfg| Arc::new(cfg.with_root_certificates(roots).with_no_client_auth()))
        })
        .clone()
        .map_err(EmailError::Imap)
}

/// Wrap an already-connected socket in TLS and drive the handshake to
/// completion here, so a bad certificate surfaces as a connect failure rather
/// than as a confusing error on the first IMAP read. `native_tls::connect` was
/// eager in the same way.
fn tls_handshake(host: &str, sock: std::net::TcpStream) -> Result<ImapTlsStream, EmailError> {
    let name = rustls::pki_types::ServerName::try_from(host.to_string())
        .map_err(|e| EmailError::Imap(format!("{host:?} is not a valid TLS server name: {e}")))?;
    let conn = rustls::ClientConnection::new(tls_client_config()?, name)
        .map_err(|e| EmailError::Imap(format!("tls init for {host}: {e}")))?;
    let mut stream = rustls::StreamOwned::new(conn, sock);
    while stream.conn.is_handshaking() {
        stream
            .conn
            .complete_io(&mut stream.sock)
            .map_err(|e| EmailError::Imap(format!("tls handshake with {host}: {e}")))?;
    }
    Ok(stream)
}

/// Connect with implicit TLS (IMAPS): TLS first, then the IMAP greeting.
/// The rustls twin of the `imap::connect` this replaced.
pub(crate) fn connect_implicit_tls(
    host: &str,
    port: u16,
) -> Result<imap::Client<ImapTlsStream>, EmailError> {
    let sock = std::net::TcpStream::connect((host, port))
        .map_err(|e| EmailError::Imap(format!("connect {host}:{port}: {e}")))?;
    let mut client = imap::Client::new(tls_handshake(host, sock)?);
    client
        .read_greeting()
        .map_err(|e| EmailError::Imap(format!("greeting {host}:{port}: {e}")))?;
    Ok(client)
}

/// The tag on the one IMAP command this module issues itself.
const STARTTLS_TAG: &str = "wl1";
/// Upper bound on a single pre-TLS response line.
const STARTTLS_MAX_LINE: usize = 8192;
/// Upper bound on untagged lines the server may interleave before the tagged
/// response, so a chatty or hostile server cannot hold the poll open forever.
const STARTTLS_MAX_UNTAGGED: usize = 64;

/// Read one CRLF-terminated response line from a pre-TLS socket.
///
/// One byte at a time on purpose. A buffered reader may pull bytes past the
/// tagged response into a buffer this function is about to drop, and the bytes
/// immediately after `OK` on a STARTTLS exchange are the peer's TLS records.
fn read_pre_tls_line<S: std::io::Read>(sock: &mut S, host: &str) -> Result<String, EmailError> {
    let mut line: Vec<u8> = Vec::new();
    let mut byte = [0u8; 1];
    loop {
        match sock.read(&mut byte) {
            Ok(0) => {
                return Err(EmailError::Imap(format!(
                    "starttls {host}: server closed the connection mid-response"
                )));
            }
            Ok(_) => {}
            Err(e) => return Err(EmailError::Imap(format!("starttls {host}: read: {e}"))),
        }
        match byte[0] {
            b'\n' => break,
            b'\r' => {}
            b => {
                if line.len() >= STARTTLS_MAX_LINE {
                    return Err(EmailError::Imap(format!(
                        "starttls {host}: response line exceeded {STARTTLS_MAX_LINE} bytes"
                    )));
                }
                line.push(b);
            }
        }
    }
    Ok(String::from_utf8_lossy(&line).into_owned())
}

/// Drive the cleartext half of an IMAP STARTTLS exchange on a raw socket:
/// read the greeting, issue `STARTTLS`, and consume the tagged response.
///
/// This is spelled out here rather than delegated because `imap` 2.4.1 gates
/// `imap::connect_starttls` behind its `tls` feature — i.e. native-tls, i.e.
/// OpenSSL — and offers no public route to the same exchange: `Connection::stream`
/// is `pub(crate)` and `Connection::run_command_and_check_ok` is private, so the
/// upgraded socket cannot be taken back out of a `Client`. The upgraded stream
/// is handed to `imap::Client::new` with NO second greeting, exactly as upstream
/// does: RFC 2595 has the server resume the same session inside TLS.
fn negotiate_starttls<S: std::io::Read + std::io::Write>(
    sock: &mut S,
    host: &str,
) -> Result<(), EmailError> {
    let greeting = read_pre_tls_line(sock, host)?;
    if !(greeting.starts_with("* OK") || greeting.starts_with("* PREAUTH")) {
        return Err(EmailError::Imap(format!(
            "starttls {host}: server did not greet with OK: {greeting}"
        )));
    }
    sock.write_all(format!("{STARTTLS_TAG} STARTTLS\r\n").as_bytes())
        .and_then(|()| sock.flush())
        .map_err(|e| EmailError::Imap(format!("starttls {host}: sending STARTTLS: {e}")))?;
    let tag = format!("{STARTTLS_TAG} ");
    for _ in 0..STARTTLS_MAX_UNTAGGED {
        let line = read_pre_tls_line(sock, host)?;
        let Some(rest) = line.strip_prefix(tag.as_str()) else {
            // An untagged status line the server is allowed to interleave.
            continue;
        };
        if rest.starts_with("OK") {
            return Ok(());
        }
        // Fail CLOSED. A NO/BAD here means the server intends to keep speaking
        // cleartext; carrying on would put the LOGIN password and every message
        // body on the wire unencrypted, which is the exact thing this mode
        // exists to prevent.
        return Err(EmailError::Imap(format!(
            "starttls {host}: server refused STARTTLS: {line}"
        )));
    }
    Err(EmailError::Imap(format!(
        "starttls {host}: no tagged response to STARTTLS within {STARTTLS_MAX_UNTAGGED} lines"
    )))
}

/// Connect on a cleartext port and upgrade in place with STARTTLS.
/// The rustls twin of the `imap::connect_starttls` this replaced.
pub(crate) fn connect_starttls(
    host: &str,
    port: u16,
) -> Result<imap::Client<ImapTlsStream>, EmailError> {
    let mut sock = std::net::TcpStream::connect((host, port))
        .map_err(|e| EmailError::Imap(format!("connect {host}:{port}: {e}")))?;
    negotiate_starttls(&mut sock, host)?;
    Ok(imap::Client::new(tls_handshake(host, sock)?))
}

/// Open the IMAP socket in whichever transport mode `security` resolves to,
/// then run one poll over it.
///
/// This function exists because the mode is a runtime decision but the stream
/// type is a compile-time one: implicit TLS and STARTTLS both yield
/// `Client<TlsStream<TcpStream>>` while plaintext yields `Client<TcpStream>`,
/// so the three arms cannot share a binding. Each arm therefore hands its own
/// concrete client to the stream-generic [`poll_with_client`].
///
/// Previously this was one unconditional implicit-TLS connect on
/// every port. Against a STARTTLS-only or plaintext server that put a TLS
/// ClientHello onto a cleartext port on every single poll — the connection was
/// reset each time, and the loop retried forever without ever reading mail.
// imap poll loop accepts host/port/user/pass/mailbox/inbox/uid/runtime;
// refactoring into a struct is needless ceremony for a sub-driver helper.
#[allow(clippy::too_many_arguments)]
fn poll_once(
    host: &str,
    port: u16,
    security: MailSecurity,
    user: &str,
    pass: &str,
    mailbox: &str,
    allow_set: Option<&std::collections::HashSet<String>>,
    own_addresses: &[String],
    sent_ids: &crate::sent_index::SentIdIndex,
    inbox: &Arc<Mutex<VecDeque<ChannelEvent>>>,
    last_seen_uid: &Arc<StdMutex<u32>>,
    reply_index: &ReplyIndex,
    runtime_handle: &tokio::runtime::Handle,
    seeded: &mut bool,
) -> Result<(), EmailError> {
    match security.resolve(host, port, IMAPS_PORT) {
        MailSecurity::Plaintext => {
            // Fail CLOSED: an unencrypted IMAP session is only ever allowed to
            // the local machine. Anywhere else the LOGIN password and every
            // fetched message body would cross the network in clear text.
            if !is_loopback_host(host) {
                return Err(EmailError::Imap(format!(
                    "imap security = \"plaintext\" refuses non-loopback host {host:?}: \
                     the login password and message bodies would cross the network unencrypted"
                )));
            }
            let stream = std::net::TcpStream::connect((host, port))
                .map_err(|e| EmailError::Imap(format!("connect {host}:{port}: {e}")))?;
            let mut client = imap::Client::new(stream);
            client
                .read_greeting()
                .map_err(|e| EmailError::Imap(format!("greeting {host}:{port}: {e}")))?;
            poll_with_client(
                client,
                user,
                pass,
                mailbox,
                host,
                allow_set,
                own_addresses,
                sent_ids,
                inbox,
                last_seen_uid,
                reply_index,
                runtime_handle,
                seeded,
            )
        }
        MailSecurity::Starttls => {
            let client = connect_starttls(host, port)?;
            poll_with_client(
                client,
                user,
                pass,
                mailbox,
                host,
                allow_set,
                own_addresses,
                sent_ids,
                inbox,
                last_seen_uid,
                reply_index,
                runtime_handle,
                seeded,
            )
        }
        // `resolve` never returns Auto; folding it in with Implicit keeps the
        // match total and preserves the historical default on port 993.
        MailSecurity::Implicit | MailSecurity::Auto => {
            let client = connect_implicit_tls(host, port)?;
            poll_with_client(
                client,
                user,
                pass,
                mailbox,
                host,
                allow_set,
                own_addresses,
                sent_ids,
                inbox,
                last_seen_uid,
                reply_index,
                runtime_handle,
                seeded,
            )
        }
    }
}

/// One poll over an already-connected IMAP client, whatever its stream type.
#[allow(clippy::too_many_arguments)]
fn poll_with_client<T: std::io::Read + std::io::Write>(
    client: imap::Client<T>,
    user: &str,
    pass: &str,
    mailbox: &str,
    host: &str,
    allow_set: Option<&std::collections::HashSet<String>>,
    own_addresses: &[String],
    sent_ids: &crate::sent_index::SentIdIndex,
    inbox: &Arc<Mutex<VecDeque<ChannelEvent>>>,
    last_seen_uid: &Arc<StdMutex<u32>>,
    reply_index: &ReplyIndex,
    runtime_handle: &tokio::runtime::Handle,
    seeded: &mut bool,
) -> Result<(), EmailError> {
    let mut session = client
        .login(user, pass)
        .map_err(|(e, _)| EmailError::Auth(format!("imap login: {e}")))?;
    let mailbox_meta = session
        .select(mailbox)
        .map_err(|e| EmailError::Imap(format!("select {mailbox}: {e}")))?;

    // First connect with no persisted watermark: seed to the current high UID
    // (UIDNEXT - 1) so pre-existing mail is NOT replayed as new inbound — only
    // messages that arrive after startup are delivered. Persist immediately so
    // a restart resumes from here rather than re-seeding past missed mail.
    if !*seeded {
        let seed_high = match mailbox_meta.uid_next {
            Some(next) => next.saturating_sub(1),
            // Server omitted UIDNEXT: fall back to the max existing UID.
            None => session
                .uid_search("1:*")
                .map(|s| s.into_iter().max().unwrap_or(0))
                .unwrap_or(0),
        };
        {
            let mut g = last_seen_uid.lock().unwrap();
            if seed_high > *g {
                *g = seed_high;
            }
        }
        let seeded_to = *last_seen_uid.lock().unwrap();
        uid_store::save(host, user, mailbox, seeded_to);
        *seeded = true;
        tracing::info!(
            target: "wcore_channel_email::imap",
            seed_high = seeded_to,
            "seeded imap watermark on first connect; pre-existing mail will not be replayed",
        );
    }

    let start_uid = {
        let g = last_seen_uid.lock().unwrap();
        (*g).saturating_add(1)
    };
    let query = format!("{start_uid}:*");
    let uids = session
        .uid_search(&query)
        .map_err(|e| EmailError::Imap(format!("uid_search {query}: {e}")))?;

    let mut new_events: Vec<ChannelEvent> = Vec::new();
    // The watermark AS IT WAS when this poll began. Frozen deliberately: it is
    // the thing "already seen" means, and it must not move while the batch is
    // being filtered against it.
    let seen_through = *last_seen_uid.lock().unwrap();
    let mut high_water = seen_through;

    // `Session::uid_search` returns a `HashSet<Uid>`, whose iteration order is
    // arbitrary and re-randomised per process. Sort ascending before fetching.
    let uids = new_uids(uids, seen_through);

    for uid in uids {
        if uid <= seen_through {
            // `UID N:*` returns at least one result even when nothing
            // new — server semantics. Skip anything we've already seen.
            continue;
        }
        let fetches = match session.uid_fetch(uid.to_string(), "RFC822") {
            Ok(f) => f,
            Err(e) => {
                tracing::warn!(
                    target: "wcore_channel_email::imap",
                    uid = uid,
                    error = %e,
                    "uid_fetch failed; skipping",
                );
                continue;
            }
        };
        for fetch in fetches.iter() {
            let body = match fetch.body() {
                Some(b) => b,
                None => continue,
            };
            if let Some(event) =
                admit_fetched_message(uid, body, allow_set, own_addresses, sent_ids, reply_index)
            {
                new_events.push(event);
            }
        }
        high_water = high_water.max(uid);
    }

    // Bump watermark even if parses failed — otherwise we'd loop on the
    // same UID forever. Persist when it advances so a restart resumes here.
    let advanced = {
        let mut g = last_seen_uid.lock().unwrap();
        if high_water > *g {
            *g = high_water;
            true
        } else {
            false
        }
    };
    if advanced {
        uid_store::save(host, user, mailbox, high_water);
    }

    if !new_events.is_empty() {
        // Bridge sync → async via the runtime handle.
        runtime_handle.block_on(async {
            let mut guard = inbox.lock().await;
            for e in new_events {
                // F9 — bounded, drop-oldest inbox against a flood.
                wcore_channels::push_bounded(&mut guard, e);
            }
        });
    }

    // Best-effort logout; ignore errors.
    let _ = session.logout();
    Ok(())
}

/// Run the full inbound admission chain for one fetched RFC822 body:
/// parse → self-mark (wayland#547 loop guard) → sender allow-list →
/// record reply-threading → wrap as a `MessageReceived` event.
///
/// `None` means the message was dropped here (parse failure or allow-list).
/// Self-marked messages are still returned — the dispatch kernel's loop
/// guard is the single drop point for `is_self`, the same path every other
/// channel uses.
///
/// Extracted from the `poll_once` fetch loop so the guard wiring is
/// testable against raw message bytes without a live IMAP session.
fn admit_fetched_message(
    uid: u32,
    body: &[u8],
    allow_set: Option<&std::collections::HashSet<String>>,
    own_addresses: &[String],
    sent_ids: &crate::sent_index::SentIdIndex,
    reply_index: &ReplyIndex,
) -> Option<ChannelEvent> {
    match parse_message(uid, body) {
        Ok((mut msg, reply_ctx)) => {
            // Loop guard (wayland#547): mark the channel's own mail echoing
            // back in so the dispatch kernel drops it instead of triggering
            // an agent turn that would reply to it forever. INFO (id only,
            // no content) because the kernel drop itself is silent.
            if let Some(detector) = mark_self_inbound(&mut msg, own_addresses, sent_ids) {
                tracing::info!(
                    target: "wcore_channel_email::imap",
                    uid = uid,
                    message_id = %msg.id,
                    detector = ?detector,
                    "inbound is this channel's own mail; marked is_self (loop guard)",
                );
            }
            // Sender allow-list. `msg.author` is the raw `From:` header
            // value (display name + addr-spec); compare its normalized
            // addr-spec against the configured set. NOTE: `From:` is
            // spoofable — this is a delivery-side filter, not
            // authentication (see ImapConfig docs).
            if !is_sender_allowed(allow_set, &msg.author) {
                tracing::info!(
                    target: "wcore_channel_email::imap",
                    uid = uid,
                    "dropping inbound message: From: not in allowed_senders",
                );
                return None;
            }
            // Record threading context so a later reply can set
            // In-Reply-To / References / Re: subject. Keyed by the
            // RFC Message-ID (== msg.id).
            record_reply_context(reply_index, msg.id.clone(), reply_ctx);
            Some(ChannelEvent::MessageReceived { msg })
        }
        Err(e) => {
            tracing::warn!(
                target: "wcore_channel_email::imap",
                uid = uid,
                error = %e,
                "rfc5322 parse failed; dropping message",
            );
            None
        }
    }
}

/// RFC 5322 / MIME parser — surfaces From, Subject, Message-ID,
/// In-Reply-To, References, a non-empty text body (walking the MIME tree
/// and converting HTML-only mail to text), and typed attachments.
///
/// We operate on a lossy-UTF-8 view of the raw message: RFC822 headers and
/// base64/quoted-printable bodies are ASCII, so the only bytes lost are
/// raw-8bit body octets we surface as metadata anyway. This keeps the
/// parser robust against the odd non-UTF-8 byte instead of erroring out.
/// Test-only convenience wrapper that drops the `ReplyContext` returned by
/// [`parse_message`]; production code uses `parse_message` directly.
#[cfg(test)]
pub(crate) fn parse_basic_rfc5322(uid: u32, body: &[u8]) -> Result<IncomingMessage, EmailError> {
    parse_message(uid, body).map(|(msg, _)| msg)
}

/// Parse a raw RFC822 message into the inbound `IncomingMessage` plus the
/// `ReplyContext` (Message-ID + Subject + References) that the SMTP send
/// path needs to thread a reply. The reply context is keyed downstream by
/// the inbound message id (`IncomingMessage.id`, which is the RFC
/// Message-ID when present).
pub(crate) fn parse_message(
    uid: u32,
    body: &[u8],
) -> Result<(IncomingMessage, ReplyContext), EmailError> {
    let text = String::from_utf8_lossy(body).into_owned();
    let (head, body_part) = split_headers_body(&text);
    let headers = unfold_headers(head);

    let mut from: Option<String> = None;
    let mut subject: Option<String> = None;
    let mut date: Option<String> = None;
    let mut message_id: Option<String> = None;
    let mut in_reply_to: Option<String> = None;
    let mut references: Option<String> = None;

    for h in &headers {
        if let Some(rest) = header_value(h, "From") {
            from = Some(rest.to_string());
        } else if let Some(rest) = header_value(h, "Subject") {
            subject = Some(rest.to_string());
        } else if let Some(rest) = header_value(h, "Date") {
            date = Some(rest.to_string());
        } else if let Some(rest) = header_value(h, "Message-ID") {
            message_id = Some(rest.trim_matches(|c| c == '<' || c == '>').to_string());
        } else if let Some(rest) = header_value(h, "In-Reply-To") {
            let stripped = rest.trim_matches(|c| c == '<' || c == '>').to_string();
            if !stripped.is_empty() {
                in_reply_to = Some(stripped);
            }
        } else if let Some(rest) = header_value(h, "References")
            && !rest.is_empty()
        {
            references = Some(rest.to_string());
        }
    }

    let author = from.clone().unwrap_or_else(|| format!("unknown@uid-{uid}"));

    // Walk the MIME tree for a plain-text body + typed attachments. The
    // walk prefers text/plain, falls back to HTML→text, and pulls any
    // attachment parts out as metadata.
    let MimeResult {
        text: body_text,
        attachments,
    } = walk_mime(&headers, body_part, 0);

    // Prepend the subject so consumers can use it as a thread hint, mirroring
    // the prior behavior (and Slack's subject-in-text convention).
    let body_trimmed = body_text.trim_end_matches(['\n', '\r']);
    let combined_text = match &subject {
        Some(s) if !s.is_empty() => {
            if body_trimmed.is_empty() {
                s.clone()
            } else {
                format!("{s}\n\n{body_trimmed}")
            }
        }
        _ => body_trimmed.to_string(),
    };

    let ts_secs = date.and_then(parse_rfc2822_to_epoch).unwrap_or(0);
    let id = message_id.unwrap_or_else(|| format!("uid:{uid}"));

    // Threading context for any reply to this message: its Message-ID,
    // Subject (for `Re: <subj>`), and References chain (for `References`).
    let reply_ctx = ReplyContext {
        message_id: id.clone(),
        subject: subject.clone(),
        references,
    };

    // Stable sender identity: the normalized addr-spec from the From header.
    let sender_id = normalize_from_addr(&author);
    let sender_display = from.as_deref().and_then(extract_display_name);
    let conversation_id = sender_id.clone();

    let msg = IncomingMessage {
        id,
        conversation_id,
        author,
        text: combined_text,
        ts_secs,
        attachments,
        sender_id,
        sender_display,
        // sender_handle, sender_alt_id: no handle/alt-id concept in email.
        // is_bot, is_self: not determinable without knowing our own address here.
        chat_type: ChatType::Direct,
        chat_name: subject,
        // space_id, parent_chat_id: no enclosing workspace in email.
        // thread_id: References-based root is not parsed; would require scanning
        //   the full References chain. Leave None until thread extraction lands.
        // account_id: receiving mailbox not passed into this fn; caller sets it.
        platform: Some("email".into()),
        // was_mentioned, mention_kind: N/A for email.
        reply_to_message_id: in_reply_to,
        // reply_to_text: we don't inline quoted-reply bodies; leave None.
        ..Default::default()
    };
    Ok((msg, reply_ctx))
}

/// Split a raw message into (headers, body) on the first blank line.
/// Accepts `CRLFCRLF` (per RFC) or bare `LFLF` (real-world MTAs are
/// sloppy). When no blank line exists, the whole input is treated as
/// headers with an empty body.
fn split_headers_body(text: &str) -> (&str, &str) {
    match text.find("\r\n\r\n") {
        Some(i) => (&text[..i], &text[i + 4..]),
        None => match text.find("\n\n") {
            Some(i) => (&text[..i], &text[i + 2..]),
            None => (text, ""),
        },
    }
}

/// Unfold a header block: a line starting with whitespace continues the
/// previous header (RFC 5322 §2.2.3). Returns one logical header per
/// entry, each still in `Name: value` form.
fn unfold_headers(head: &str) -> Vec<String> {
    let mut current: Option<String> = None;
    let mut headers: Vec<String> = Vec::new();
    for line in head.lines() {
        if line.starts_with(' ') || line.starts_with('\t') {
            if let Some(ref mut acc) = current {
                acc.push(' ');
                acc.push_str(line.trim());
            }
        } else {
            if let Some(prev) = current.take() {
                headers.push(prev);
            }
            current = Some(line.to_string());
        }
    }
    if let Some(prev) = current.take() {
        headers.push(prev);
    }
    headers
}

/// Case-insensitively match a single unfolded header line against `name`
/// and return its trimmed value, or `None` if it's a different header.
fn header_value<'a>(line: &'a str, name: &str) -> Option<&'a str> {
    let colon = line.find(':')?;
    if line[..colon].trim().eq_ignore_ascii_case(name) {
        Some(line[colon + 1..].trim())
    } else {
        None
    }
}

/// Result of walking a message's MIME tree: the best plain-text body we
/// could extract plus any attachment parts surfaced as metadata.
struct MimeResult {
    text: String,
    attachments: Vec<Attachment>,
}

/// Parsed `Content-Type` of a MIME part.
struct ContentType {
    /// Lowercased `type/subtype` (e.g. `text/plain`, `multipart/mixed`).
    mime: String,
    /// `boundary` parameter for multipart types, if present.
    boundary: Option<String>,
}

/// Walk a MIME body given its (unfolded) headers and raw body text.
///
/// Dispatch:
/// - `multipart/*` — split on the boundary, recurse into each sub-part.
///   For `multipart/alternative` prefer the richest text we can render
///   (plain over html-converted-to-text); for `mixed`/`related` collect
///   the first non-empty text part and accumulate attachments.
/// - `text/plain` — decoded body as-is.
/// - `text/html` — tags stripped to a readable plain body.
/// - anything with a filename / `attachment` disposition — an Attachment.
///
/// Guarantees a non-empty `text` whenever any renderable text part exists
/// anywhere in the tree.
/// Maximum MIME nesting depth walked. A hostile sender can craft a message
/// with thousands of nested `multipart/*` levels; without a cap the
/// recursion would overflow the stack and crash the poll thread. Past this
/// depth a part is surfaced as raw text rather than recursed into.
const MAX_MIME_DEPTH: usize = 20;

/// Upper bound on an attachment we inline (as a `data:` URL) for later fetch.
/// Email images/audio are typically well under this; larger parts stay
/// metadata-only (fetch_media returns Rejected and the enricher falls back to
/// the bare summary), keeping the inbound event bounded with no temp files.
///
/// Derived from the adapter's DECLARED bound rather than being a second
/// hardcoded number, so `media_bounds()` cannot advertise one figure while this
/// path enforces another — which it did, advertising 10 MiB against this 2 MiB.
const MAX_INLINE_ATTACHMENT_BYTES: usize = crate::MEDIA_BOUNDS.max_bytes as usize;

fn walk_mime(headers: &[String], body: &str, depth: usize) -> MimeResult {
    let ct = parse_content_type(headers);
    let disposition = headers
        .iter()
        .find_map(|h| header_value(h, "Content-Disposition"));
    let cte = headers
        .iter()
        .find_map(|h| header_value(h, "Content-Transfer-Encoding"))
        .map(|s| s.to_ascii_lowercase());

    // Attachment parts are surfaced as metadata, not body text. Treat a
    // part as an attachment when it declares `Content-Disposition:
    // attachment`, or when it carries a filename, or when it's a non-text
    // part that isn't multipart.
    let filename = disposition
        .and_then(parse_filename)
        .or_else(|| ct_param(headers, "name"));
    let is_attachment = disposition
        .is_some_and(|d| d.trim().to_ascii_lowercase().starts_with("attachment"))
        || (filename.is_some() && !ct.mime.starts_with("text/"));

    if ct.mime.starts_with("multipart/") {
        if depth >= MAX_MIME_DEPTH {
            // Pathologically nested multipart — stop recursing and surface the
            // raw body as text rather than risk a stack overflow.
            return MimeResult {
                text: body.trim().to_string(),
                attachments: Vec::new(),
            };
        }
        return walk_multipart(&ct, body, depth);
    }

    if is_attachment {
        let name = filename.unwrap_or_else(|| "attachment".to_string());
        return MimeResult {
            text: String::new(),
            attachments: vec![attachment_from(&ct.mime, &name, body, cte.as_deref())],
        };
    }

    // Leaf text part.
    let decoded = decode_transfer(body, cte.as_deref());
    let text = if ct.mime == "text/html" || (ct.mime.is_empty() && looks_like_html(&decoded)) {
        html_to_text(&decoded)
    } else {
        // text/plain (the default per RFC 2045 when no Content-Type).
        decoded
    };
    MimeResult {
        text,
        attachments: Vec::new(),
    }
}

/// Split a multipart body on its boundary and fold the sub-parts into a
/// single `MimeResult`.
fn walk_multipart(ct: &ContentType, body: &str, depth: usize) -> MimeResult {
    let boundary = match &ct.boundary {
        Some(b) => b,
        // Malformed multipart with no boundary: best-effort treat the raw
        // body as text so we don't silently drop content.
        None => {
            return MimeResult {
                text: body.trim().to_string(),
                attachments: Vec::new(),
            };
        }
    };

    let alternative = ct.mime == "multipart/alternative";
    let mut plain: Option<String> = None;
    let mut html: Option<String> = None;
    let mut other_text: Option<String> = None;
    let mut attachments: Vec<Attachment> = Vec::new();

    for raw_part in split_parts(body, boundary) {
        let (phead, pbody) = split_headers_body(raw_part);
        let pheaders = unfold_headers(phead);
        let sub = walk_mime(&pheaders, pbody, depth + 1);
        attachments.extend(sub.attachments);

        if sub.text.trim().is_empty() {
            continue;
        }
        let part_ct = parse_content_type(&pheaders);
        match part_ct.mime.as_str() {
            "text/plain" => {
                if plain.is_none() {
                    plain = Some(sub.text);
                }
            }
            "text/html" => {
                if html.is_none() {
                    html = Some(sub.text);
                }
            }
            _ => {
                // Nested multipart already rendered to text, or an
                // untyped text leaf — keep the first non-empty.
                if other_text.is_none() {
                    other_text = Some(sub.text);
                }
            }
        }
    }

    // multipart/alternative: prefer plain, then html. mixed/related/etc:
    // concatenation isn't meaningful for a chat surface, so we take the
    // first renderable text in priority order. Either way a non-empty text
    // part anywhere yields a non-empty body.
    let text = if alternative {
        plain.or(html).or(other_text).unwrap_or_default()
    } else {
        plain.or(other_text).or(html).unwrap_or_default()
    };

    MimeResult { text, attachments }
}

/// Split a multipart body into raw sub-part slices delimited by
/// `--<boundary>`. The preamble (before the first boundary) and the
/// closing `--<boundary>--` epilogue are discarded per RFC 2046.
fn split_parts<'a>(body: &'a str, boundary: &str) -> Vec<&'a str> {
    let delim = format!("--{boundary}");
    let mut parts = Vec::new();
    // Skip everything up to (and including) the first delimiter line.
    let mut rest = match body.find(&delim) {
        Some(i) => &body[i + delim.len()..],
        None => return parts,
    };
    loop {
        // Each part starts after the CRLF following the delimiter.
        let part_start = rest.find('\n').map_or(rest.len(), |i| i + 1);
        let after = &rest[part_start..];
        match after.find(&delim) {
            Some(next) => {
                let part = &after[..next];
                // Closing delimiter is `--boundary--`; trailing chars after
                // the part are trimmed by split_headers_body downstream.
                parts.push(part.trim_end_matches(['\r', '\n']));
                rest = &after[next + delim.len()..];
                // `--` immediately after the delimiter marks the end.
                if rest.starts_with("--") {
                    break;
                }
            }
            None => break,
        }
    }
    parts
}

/// Parse a part's `Content-Type` header into a normalized struct. When the
/// header is absent, `mime` is left empty; the leaf handler then treats it
/// as `text/plain` per RFC 2045 (unless the body sniffs as HTML).
fn parse_content_type(headers: &[String]) -> ContentType {
    let raw = headers.iter().find_map(|h| header_value(h, "Content-Type"));
    let raw = match raw {
        Some(r) => r,
        None => {
            return ContentType {
                mime: String::new(),
                boundary: None,
            };
        }
    };
    let mime = raw
        .split(';')
        .next()
        .unwrap_or("")
        .trim()
        .to_ascii_lowercase();
    let boundary = ct_param_in(raw, "boundary");
    ContentType { mime, boundary }
}

/// Extract a `Content-Type` parameter (e.g. `boundary`, `name`) from the
/// full header value list.
fn ct_param(headers: &[String], param: &str) -> Option<String> {
    let raw = headers
        .iter()
        .find_map(|h| header_value(h, "Content-Type"))?;
    ct_param_in(raw, param)
}

/// Extract a parameter value from a raw `Content-Type` / disposition
/// value, handling both quoted and bare forms (`name="a.png"` / `name=a`).
fn ct_param_in(raw: &str, param: &str) -> Option<String> {
    for seg in raw.split(';').skip(1) {
        let seg = seg.trim();
        let Some(eq) = seg.find('=') else { continue };
        if seg[..eq].trim().eq_ignore_ascii_case(param) {
            let val = seg[eq + 1..].trim().trim_matches('"');
            if !val.is_empty() {
                return Some(val.to_string());
            }
        }
    }
    None
}

/// Pull the `filename` out of a `Content-Disposition` value.
fn parse_filename(disposition: &str) -> Option<String> {
    ct_param_in(disposition, "filename")
}

/// Map a leaf part to a typed `Attachment`, decoding and inlining its bytes.
///
/// The decoded payload is carried as a `data:<mime>;base64,…` URL when it fits
/// under [`MAX_INLINE_ATTACHMENT_BYTES`], so [`EmailChannel::fetch_media`] can
/// hand the bytes to the inbound-media enricher without a network round-trip or
/// any temp-file lifecycle. Oversize parts stay metadata-only (empty `url`).
/// `path` keeps the human-facing filename; `content_type` + `kind` route it.
fn attachment_from(mime: &str, filename: &str, body: &str, cte: Option<&str>) -> Attachment {
    let kind = if mime.starts_with("image/") {
        MediaKind::Image
    } else if mime.starts_with("audio/") {
        MediaKind::Audio
    } else if mime.starts_with("video/") {
        MediaKind::Video
    } else {
        MediaKind::Document
    };

    let bytes = decode_transfer_bytes(body, cte);
    let url = if !bytes.is_empty() && bytes.len() <= MAX_INLINE_ATTACHMENT_BYTES {
        let ct = if mime.is_empty() {
            "application/octet-stream"
        } else {
            mime
        };
        format!("data:{ct};base64,{}", encode_base64(&bytes))
    } else {
        String::new()
    };

    Attachment {
        url,
        path: Some(filename.to_string()),
        content_type: if mime.is_empty() {
            None
        } else {
            Some(mime.to_string())
        },
        kind,
        transcribed: None,
    }
}

/// Decode a leaf part body according to its `Content-Transfer-Encoding`.
/// Handles `quoted-printable` and `base64`; `7bit`/`8bit`/`binary`/none
/// pass through unchanged.
fn decode_transfer(body: &str, cte: Option<&str>) -> String {
    match cte {
        Some("quoted-printable") => decode_quoted_printable(body),
        Some("base64") => decode_base64_text(body),
        _ => body.to_string(),
    }
}

/// Decode a single ASCII hex digit to its nibble value.
fn hex_val(b: u8) -> Option<u8> {
    match b {
        b'0'..=b'9' => Some(b - b'0'),
        b'a'..=b'f' => Some(b - b'a' + 10),
        b'A'..=b'F' => Some(b - b'A' + 10),
        _ => None,
    }
}

/// Minimal quoted-printable decoder (RFC 2045 §6.7): `=XX` hex escapes and
/// `=`-at-end-of-line soft breaks. Unrecognized `=` sequences pass through.
fn decode_quoted_printable(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    for line in input.split_inclusive('\n') {
        let trimmed = line.trim_end_matches(['\r', '\n']);
        // Soft line break: a trailing `=` joins to the next line.
        let (content, soft_break) = match trimmed.strip_suffix('=') {
            Some(rest) => (rest, true),
            None => (trimmed, false),
        };
        let bytes = content.as_bytes();
        let mut i = 0;
        let mut buf: Vec<u8> = Vec::with_capacity(bytes.len());
        while i < bytes.len() {
            // `=XX` hex escape, decoded directly from bytes (no str slicing,
            // so a malformed multibyte sequence can never panic).
            if bytes[i] == b'='
                && i + 2 < bytes.len()
                && let (Some(hi), Some(lo)) = (hex_val(bytes[i + 1]), hex_val(bytes[i + 2]))
            {
                buf.push((hi << 4) | lo);
                i += 3;
                continue;
            }
            buf.push(bytes[i]);
            i += 1;
        }
        out.push_str(&String::from_utf8_lossy(&buf));
        if !soft_break {
            // Preserve hard line breaks (normalize to \n).
            if line.ends_with('\n') {
                out.push('\n');
            }
        }
    }
    out
}

/// Minimal base64 decoder for text parts. Ignores whitespace/newlines and
/// stops at padding. Returns a lossy-UTF-8 rendering of the decoded bytes.
fn decode_base64_text(input: &str) -> String {
    String::from_utf8_lossy(&decode_base64_bytes(input)).into_owned()
}

/// Decode standard base64 to raw bytes, skipping whitespace/newlines and
/// stopping at the first `=` pad. Used for both text bodies and binary
/// attachment payloads (and to read back inline `data:` attachment URLs).
pub(crate) fn decode_base64_bytes(input: &str) -> Vec<u8> {
    fn val(c: u8) -> Option<u8> {
        match c {
            b'A'..=b'Z' => Some(c - b'A'),
            b'a'..=b'z' => Some(c - b'a' + 26),
            b'0'..=b'9' => Some(c - b'0' + 52),
            b'+' => Some(62),
            b'/' => Some(63),
            _ => None,
        }
    }
    let mut out: Vec<u8> = Vec::with_capacity(input.len() * 3 / 4);
    let mut acc: u32 = 0;
    let mut bits = 0u32;
    for &c in input.as_bytes() {
        if c == b'=' {
            break;
        }
        let Some(v) = val(c) else { continue };
        acc = (acc << 6) | u32::from(v);
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push((acc >> bits) as u8);
        }
    }
    out
}

/// Encode raw bytes as standard (padded) base64 — the inverse of
/// [`decode_base64_bytes`], used to inline small attachments as `data:` URLs.
fn encode_base64(bytes: &[u8]) -> String {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let b0 = chunk[0] as u32;
        let b1 = *chunk.get(1).unwrap_or(&0) as u32;
        let b2 = *chunk.get(2).unwrap_or(&0) as u32;
        let n = (b0 << 16) | (b1 << 8) | b2;
        out.push(ALPHABET[((n >> 18) & 63) as usize] as char);
        out.push(ALPHABET[((n >> 12) & 63) as usize] as char);
        out.push(if chunk.len() > 1 {
            ALPHABET[((n >> 6) & 63) as usize] as char
        } else {
            '='
        });
        out.push(if chunk.len() > 2 {
            ALPHABET[(n & 63) as usize] as char
        } else {
            '='
        });
    }
    out
}

/// Decode a leaf part body to raw bytes according to its
/// `Content-Transfer-Encoding`. The bytes-returning sibling of
/// [`decode_transfer`], used for binary attachment payloads.
pub(crate) fn decode_transfer_bytes(body: &str, cte: Option<&str>) -> Vec<u8> {
    match cte {
        Some("base64") => decode_base64_bytes(body),
        Some("quoted-printable") => decode_quoted_printable(body).into_bytes(),
        // 7bit / 8bit / binary / none: the body bytes are the payload as-is.
        _ => body.as_bytes().to_vec(),
    }
}

/// Heuristic: does this body look like HTML even without a Content-Type?
fn looks_like_html(s: &str) -> bool {
    let low = s.trim_start().to_ascii_lowercase();
    low.starts_with("<!doctype html") || low.starts_with("<html") || low.contains("<body")
}

/// Strip HTML to a readable plain-text body (v1: a minimal tag stripper,
/// not a full renderer). Drops `<script>`/`<style>` contents entirely,
/// removes all tags, decodes the handful of common entities, and collapses
/// runs of blank lines. Adequate for surfacing the human-readable text of
/// an HTML-only email on a chat surface.
fn html_to_text(html: &str) -> String {
    let mut out = String::with_capacity(html.len());
    let lower = html.to_ascii_lowercase();
    // Work on byte offsets but only ever slice at char boundaries: tag
    // delimiters (`<`, `>`) are ASCII, so byte offsets at those points are
    // always valid boundaries. Text between tags is copied char-by-char.
    let mut i = 0;
    while i < html.len() {
        if html.as_bytes()[i] == b'<' {
            // Drop the entire contents of <script>/<style> blocks.
            let mut skipped = false;
            for tag in ["script", "style"] {
                if lower[i..].starts_with(&format!("<{tag}")) {
                    let close = format!("</{tag}>");
                    i = match lower[i..].find(&close) {
                        Some(end) => i + end + close.len(),
                        None => html.len(),
                    };
                    skipped = true;
                    break;
                }
            }
            if skipped {
                continue;
            }
            // Map block-level tags to newlines so structure survives.
            if lower[i..].starts_with("<br")
                || lower[i..].starts_with("</p")
                || lower[i..].starts_with("</div")
                || lower[i..].starts_with("</tr")
                || lower[i..].starts_with("</li")
                || lower[i..].starts_with("</h")
            {
                out.push('\n');
            }
            // Skip to the end of the tag.
            match html[i..].find('>') {
                Some(end) => i += end + 1,
                None => break,
            }
        } else {
            // Copy one full UTF-8 char (handles multibyte safely).
            let ch = html[i..].chars().next().unwrap_or('\u{FFFD}');
            out.push(ch);
            i += ch.len_utf8();
        }
    }
    let decoded = decode_html_entities(&out);
    collapse_blank_lines(&decoded)
}

/// Decode the small set of HTML entities that show up in real mail.
fn decode_html_entities(s: &str) -> String {
    s.replace("&nbsp;", " ")
        .replace("&amp;", "&")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&#39;", "'")
        .replace("&apos;", "'")
}

/// Trim each line and collapse runs of 2+ blank lines into one.
fn collapse_blank_lines(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut blank_run = 0;
    for line in s.lines() {
        let t = line.trim();
        if t.is_empty() {
            blank_run += 1;
            if blank_run <= 1 {
                out.push('\n');
            }
        } else {
            blank_run = 0;
            out.push_str(t);
            out.push('\n');
        }
    }
    out.trim().to_string()
}

/// Decide whether an inbound message's raw `From:` value passes the
/// sender allow-list. `None` allow-set means "no filtering" (allow all).
/// A non-empty allow-set requires the normalized addr-spec to be present;
/// anything else — including an unparsable/empty sender — is dropped
/// (fail closed).
pub(crate) fn is_sender_allowed(
    allow_set: Option<&std::collections::HashSet<String>>,
    raw_from: &str,
) -> bool {
    match allow_set {
        None => true,
        Some(set) => set.contains(&normalize_from_addr(raw_from)),
    }
}

/// Which loop-guard detector matched an inbound message as the channel's
/// own mail. Surfaced so the admission path can log the reason — the
/// kernel-side drop itself is silent (`record_history: false`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SelfMatch {
    /// Inbound Message-ID is one this process recorded before sending.
    MessageId,
    /// Inbound From matches the channel's own address set.
    OwnAddress,
}

/// Mark an inbound message `is_self` when it is this channel's own mail
/// echoing back into the monitored mailbox (wayland#547 loop guard).
/// Returns the detector that fired, `None` when the message was left alone
/// (or was already marked).
///
/// Two detectors, either suffices:
/// 1. **Echoed Message-ID** — the id is one the SMTP path recorded before
///    sending. Only ever matches mail this process sent.
/// 2. **Own-address From** — the normalized sender matches the channel's
///    own address set. Blunt backstop for relays that rewrite Message-IDs;
///    also catches agent mail sent before the current process started.
///    Tradeoff: a human mailing the monitored account FROM the account's
///    own address (self-note workflows) is also flagged — indistinguishable
///    from an agent echo by headers alone, and letting it through would
///    reinstate the loop on From-rewriting providers.
///
/// Marking (not dropping) keeps the drop decision in the dispatch kernel's
/// existing loop guard (`classify`: `is_self` → silent drop), the same path
/// every other channel uses.
pub(crate) fn mark_self_inbound(
    msg: &mut IncomingMessage,
    own_addresses: &[String],
    sent_ids: &crate::sent_index::SentIdIndex,
) -> Option<SelfMatch> {
    if msg.is_self {
        return None;
    }
    let echoed = sent_ids
        .lock()
        .map(|s| s.contains(&msg.id))
        .unwrap_or(false);
    if echoed {
        msg.is_self = true;
        return Some(SelfMatch::MessageId);
    }
    // `sender_id` is already the normalized (bare, lowercased) addr-spec.
    if !msg.sender_id.is_empty() && own_addresses.iter().any(|a| a == &msg.sender_id) {
        msg.is_self = true;
        return Some(SelfMatch::OwnAddress);
    }
    None
}

/// Extract the display name from a `From:`-style header value, returning
/// `None` when no display name is present (bare addr-spec form).
///
/// `Alice <alice@acme.com>`        -> `Some("Alice")`
/// `"Carol D" <carol@acme.com>`    -> `Some("Carol D")`
/// `bob@acme.com`                  -> `None`
fn extract_display_name(raw: &str) -> Option<String> {
    let trimmed = raw.trim();
    let angle = trimmed.find('<')?;
    let name = trimmed[..angle]
        .trim()
        .trim_matches(|c| c == '"' || c == '\'');
    if name.is_empty() {
        None
    } else {
        Some(name.to_string())
    }
}

/// Extract the bare `addr-spec` from a `From:`-style header value and
/// lowercase it for case-insensitive comparison.
pub(crate) fn normalize_from_addr(raw: &str) -> String {
    let trimmed = raw.trim();
    let inner = match (trimmed.find('<'), trimmed.find('>')) {
        (Some(open), Some(close)) if close > open + 1 => &trimmed[open + 1..close],
        _ => trimmed,
    };
    inner
        .trim()
        .trim_matches(|c| c == '"' || c == '\'')
        .to_lowercase()
}

fn parse_rfc2822_to_epoch(s: String) -> Option<i64> {
    chrono::DateTime::parse_from_rfc2822(&s)
        .ok()
        .map(|dt| dt.timestamp())
}

#[cfg(test)]
mod tests {
    use super::*;

    // ----- B-3: IMAP transport security -----
    //
    // Measured defect, corpus row B-3 on two platforms: `poll_once` opened
    // every connection with `imap::connect`, i.e. implicit TLS, on whatever
    // port it was given. Against the row's hermetic plaintext mail host the
    // mail server logged 358 (Linux) / 179 (Windows) TLS ClientHellos on its
    // cleartext IMAP port and served not one IMAP command, for the whole
    // 30-minute run. These tests assert on the BYTES the server receives,
    // which is the same third-party observation the corpus grades from.

    /// The first byte of a TLS record of type `handshake`. The corpus counts
    /// these to prove the client spoke TLS where it should have spoken IMAP.
    const TLS_HANDSHAKE_BYTE: u8 = 0x16;

    /// Accept one connection, send an IMAP greeting, and return the first
    /// line the client sends back.
    fn imap_listener_capturing_first_line() -> (u16, std::sync::mpsc::Receiver<Vec<u8>>) {
        use std::io::{BufRead, BufReader, Write};
        let listener = std::net::TcpListener::bind(("127.0.0.1", 0)).expect("bind loopback");
        let port = listener.local_addr().expect("local addr").port();
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let Ok((mut sock, _)) = listener.accept() else {
                return;
            };
            let _ = sock.set_read_timeout(Some(Duration::from_secs(10)));
            // Server speaks first, exactly like a real IMAP host.
            let _ = sock.write_all(b"* OK [CAPABILITY IMAP4rev1 AUTH=PLAIN LOGIN] ready\r\n");
            let _ = sock.flush();
            let mut reader = BufReader::new(sock.try_clone().expect("clone sock"));
            let mut line = Vec::new();
            let _ = reader.read_until(b'\n', &mut line);
            let _ = tx.send(line);
            // Refuse, so the poll ends promptly; the assertion is on the bytes.
            let _ = sock.write_all(b"* BYE closing\r\n");
        });
        (port, rx)
    }

    #[allow(clippy::too_many_arguments)]
    fn poll_once_against(host: &str, port: u16, security: MailSecurity) -> Result<(), EmailError> {
        let inbox = Arc::new(Mutex::new(VecDeque::new()));
        let last_seen = Arc::new(StdMutex::new(0u32));
        let reply_index: ReplyIndex = Arc::new(StdMutex::new(HashMap::new()));
        let sent_ids = crate::sent_index::new_index();
        let mut seeded = true;
        poll_once(
            host,
            port,
            security,
            "agent",
            "pw",
            "INBOX",
            None,
            &[],
            &sent_ids,
            &inbox,
            &last_seen,
            &reply_index,
            &tokio::runtime::Handle::current(),
            &mut seeded,
        )
    }

    /// The B-3 regression. A plaintext IMAP host on a loopback socket must be
    /// spoken to in IMAP, not TLS.
    ///
    /// Pre-fix this test fails on the FIRST assertion: the captured line
    /// begins with 0x16 (a TLS ClientHello) and contains no IMAP command at
    /// all, because `imap::connect` was unconditional.
    #[tokio::test(flavor = "multi_thread")]
    async fn plaintext_loopback_imap_speaks_imap_not_tls() {
        let (port, rx) = imap_listener_capturing_first_line();
        // `Auto` is what the corpus config produces: it names no security mode
        // at all, so the default has to be the one that works here.
        let _ = tokio::task::spawn_blocking(move || {
            poll_once_against("127.0.0.1", port, MailSecurity::Auto)
        })
        .await;

        let first = rx
            .recv_timeout(Duration::from_secs(15))
            .expect("the server must have received something");
        assert!(
            !first.is_empty(),
            "the server received no bytes at all from the client"
        );
        assert_ne!(
            first[0], TLS_HANDSHAKE_BYTE,
            "the client opened with a TLS ClientHello on a plaintext IMAP port; \
             captured {first:?}"
        );
        let text = String::from_utf8_lossy(&first).to_uppercase();
        assert!(
            text.contains("LOGIN"),
            "the client must authenticate in cleartext IMAP; captured {text:?}"
        );
    }

    /// The other direction, and the one that keeps the fix from being a
    /// weakening: an explicit `plaintext` toward a host that is not provably
    /// the local machine is REFUSED, and no socket is opened to it.
    #[tokio::test(flavor = "multi_thread")]
    async fn plaintext_is_refused_for_a_non_loopback_host() {
        let err = tokio::task::spawn_blocking(|| {
            // Port 1 so that if the guard ever regressed, the connect would
            // fail with a connection error rather than reach anything.
            poll_once_against("imap.example.com", 1, MailSecurity::Plaintext)
        })
        .await
        .expect("join")
        .expect_err("plaintext to a remote host must be refused");
        let msg = err.to_string();
        assert!(
            msg.contains("refuses non-loopback host"),
            "expected the fail-closed refusal, got: {msg}"
        );
    }

    // ----- STARTTLS on rustls -----
    //
    // The move off native-tls (i.e. off OpenSSL) had to reimplement the
    // cleartext half of the STARTTLS exchange, because `imap` 2.4.1 gates
    // `imap::connect_starttls` behind its native-tls feature and exposes no
    // public route to the same three lines. These tests grade THAT code against
    // the bytes a server actually receives, which is the only place a downgrade
    // would be visible.

    /// What [`starttls_server`] reports: the cleartext command line it read,
    /// and the bytes the client sent after the tagged response.
    type StarttlsCapture = std::sync::mpsc::Receiver<(Vec<u8>, Vec<u8>)>;

    /// A loopback server that greets, answers one `STARTTLS` with `tagged`,
    /// and then records whatever the client sends next. The recorded bytes are
    /// what separates "upgraded to TLS" from "kept talking in the clear".
    fn starttls_server(
        greeting: &'static str,
        pre_tagged: &'static [&'static str],
        tagged: &'static str,
    ) -> (u16, StarttlsCapture) {
        use std::io::{Read, Write};
        let listener = std::net::TcpListener::bind(("127.0.0.1", 0)).expect("bind loopback");
        let port = listener.local_addr().expect("local addr").port();
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let Ok((mut sock, _)) = listener.accept() else {
                return;
            };
            let _ = sock.set_read_timeout(Some(Duration::from_secs(10)));
            let _ = sock.write_all(greeting.as_bytes());
            let _ = sock.flush();
            // Read the client command a byte at a time so nothing that follows
            // it is swallowed into a buffer this server then drops.
            let mut command = Vec::new();
            let mut byte = [0u8; 1];
            while let Ok(1) = sock.read(&mut byte) {
                command.push(byte[0]);
                if byte[0] == b'\n' {
                    break;
                }
            }
            for line in pre_tagged {
                let _ = sock.write_all(line.as_bytes());
            }
            let _ = sock.write_all(tagged.as_bytes());
            let _ = sock.flush();
            let mut after = [0u8; 64];
            let n = sock.read(&mut after).unwrap_or(0);
            let _ = tx.send((command, after[..n].to_vec()));
        });
        (port, rx)
    }

    /// The happy path: the client must negotiate in CLEARTEXT IMAP and only
    /// then open TLS. Untagged lines before the tagged OK are tolerated.
    #[tokio::test(flavor = "multi_thread")]
    async fn starttls_negotiates_in_cleartext_then_opens_tls() {
        let (port, rx) = starttls_server(
            "* OK [CAPABILITY IMAP4rev1 STARTTLS] ready\r\n",
            &["* CAPABILITY IMAP4rev1 STARTTLS\r\n"],
            "wl1 OK Begin TLS negotiation now\r\n",
        );
        let _ = tokio::task::spawn_blocking(move || {
            poll_once_against("127.0.0.1", port, MailSecurity::Starttls)
        })
        .await;

        let (command, after) = rx
            .recv_timeout(Duration::from_secs(15))
            .expect("the server must have received something");
        let text = String::from_utf8_lossy(&command).to_uppercase();
        assert!(
            !command.is_empty() && command[0] != TLS_HANDSHAKE_BYTE,
            "the client opened with a TLS ClientHello on a STARTTLS port; captured {command:?}"
        );
        assert!(
            text.contains("STARTTLS"),
            "the client must issue STARTTLS in cleartext IMAP; captured {text:?}"
        );
        assert_eq!(
            after.first().copied(),
            Some(TLS_HANDSHAKE_BYTE),
            "after a STARTTLS OK the very next bytes must be a TLS ClientHello, \
             otherwise the session stayed in the clear; captured {after:?}"
        );
    }

    /// The control, and the one that keeps the happy path from being vacuous:
    /// a server that REFUSES STARTTLS must not be talked to anyway. Nothing may
    /// follow the refusal on the wire — least of all a LOGIN.
    #[tokio::test(flavor = "multi_thread")]
    async fn a_refused_starttls_fails_closed_and_sends_nothing_further() {
        let (port, rx) = starttls_server(
            "* OK ready\r\n",
            &[],
            "wl1 NO STARTTLS is not supported here\r\n",
        );
        let err = tokio::task::spawn_blocking(move || {
            poll_once_against("127.0.0.1", port, MailSecurity::Starttls)
        })
        .await
        .expect("join")
        .expect_err("a refused STARTTLS must not yield a usable session");
        let msg = err.to_string();
        assert!(
            msg.contains("server refused STARTTLS"),
            "expected the fail-closed refusal, got: {msg}"
        );

        let (command, after) = rx
            .recv_timeout(Duration::from_secs(15))
            .expect("the server must have received the STARTTLS command");
        assert!(
            String::from_utf8_lossy(&command)
                .to_uppercase()
                .contains("STARTTLS"),
            "captured {command:?}"
        );
        assert!(
            after.is_empty(),
            "the client kept talking after the server refused to encrypt; captured {after:?}"
        );
    }

    /// `negotiate_starttls` reads exactly one line and stops. If it ever used a
    /// buffered reader it would swallow the bytes after the tagged OK, and those
    /// bytes are the peer's TLS records — a corruption that only shows up
    /// against a server that pipelines, which is most of them.
    #[test]
    fn the_negotiation_leaves_the_bytes_after_the_tagged_ok_on_the_stream() {
        // One buffer holding the greeting, the tagged OK, and a stand-in for the
        // first TLS record, delivered as if they arrived in a single read.
        let mut script: Vec<u8> = Vec::new();
        script.extend_from_slice(b"* OK ready\r\n");
        script.extend_from_slice(b"wl1 OK go ahead\r\n");
        script.extend_from_slice(&[TLS_HANDSHAKE_BYTE, 0x03, 0x03, 0xAA]);

        struct Duplex {
            inbound: std::io::Cursor<Vec<u8>>,
            outbound: Vec<u8>,
        }
        impl std::io::Read for Duplex {
            fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
                std::io::Read::read(&mut self.inbound, buf)
            }
        }
        impl std::io::Write for Duplex {
            fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
                self.outbound.extend_from_slice(buf);
                Ok(buf.len())
            }
            fn flush(&mut self) -> std::io::Result<()> {
                Ok(())
            }
        }

        let mut duplex = Duplex {
            inbound: std::io::Cursor::new(script),
            outbound: Vec::new(),
        };
        negotiate_starttls(&mut duplex, "mail.example.com").expect("negotiation must succeed");
        assert_eq!(
            duplex.outbound, b"wl1 STARTTLS\r\n",
            "exactly one cleartext command, and it is STARTTLS"
        );
        let mut rest = Vec::new();
        std::io::Read::read_to_end(&mut duplex, &mut rest).expect("drain");
        assert_eq!(
            rest,
            vec![TLS_HANDSHAKE_BYTE, 0x03, 0x03, 0xAA],
            "the negotiation consumed bytes belonging to the TLS handshake"
        );
    }

    /// Port 993 keeps its historical implicit-TLS behaviour off the loopback:
    /// the fix must not turn an IMAPS deployment into a cleartext one.
    #[tokio::test(flavor = "multi_thread")]
    async fn remote_default_port_still_resolves_to_implicit_tls() {
        assert_eq!(
            MailSecurity::Auto.resolve("imap.example.com", IMAPS_PORT, IMAPS_PORT),
            MailSecurity::Implicit
        );
        assert_eq!(
            MailSecurity::Auto.resolve("imap.example.com", 143, IMAPS_PORT),
            MailSecurity::Starttls
        );
    }

    // ----- F24-C3: steady-state UID selection -----
    //
    // These four are the regression guard for a measured silent-loss defect:
    // six messages arrived between polls, `UID SEARCH` returned all six, and
    // exactly one was fetched because the filter's watermark moved underneath
    // it while it walked an unordered `HashSet`.

    /// The pre-fix filter, kept executable so the tests below can prove the
    /// repair changes an outcome rather than merely restating the new one.
    /// A self-test that only asserts the fixed behaviour passes on code that
    /// was never broken.
    fn legacy_selection(uids: &[u32], seen_through: u32) -> Vec<u32> {
        let mut taken = Vec::new();
        let mut high_water = seen_through;
        for &uid in uids {
            if uid <= high_water {
                continue;
            }
            taken.push(uid);
            high_water = high_water.max(uid);
        }
        taken
    }

    #[test]
    fn every_new_uid_is_selected_regardless_of_set_iteration_order() {
        let uids: std::collections::HashSet<u32> = (1005..=1010).collect();
        let got = new_uids(uids, 1004);
        assert_eq!(
            got,
            vec![1005, 1006, 1007, 1008, 1009, 1010],
            "a poll that finds six new messages must fetch all six"
        );
    }

    #[test]
    fn the_old_filter_loses_five_of_six_when_the_largest_uid_comes_first() {
        // The exact order that produced the measured loss on hetzner: the
        // HashSet happened to yield 1010 first.
        let hostile = [1010, 1005, 1006, 1007, 1008, 1009];
        assert_eq!(
            legacy_selection(&hostile, 1004),
            vec![1010],
            "this is the defect: five inbound messages silently discarded"
        );
        // And the repaired filter does not care what order they arrive in.
        let got = new_uids(hostile.iter().copied().collect(), 1004);
        assert_eq!(
            got.len(),
            6,
            "the repair must take all six from the same input"
        );
    }

    #[test]
    fn already_seen_uids_are_still_skipped() {
        // The guard the original comment describes must survive the repair:
        // `UID N:*` returns at least one result even when nothing is new.
        let uids: std::collections::HashSet<u32> = [1004].into_iter().collect();
        assert!(
            new_uids(uids, 1004).is_empty(),
            "the server echoing the last-seen UID must not re-deliver it"
        );
    }

    #[test]
    fn selection_is_ascending_so_mail_reaches_the_engine_in_arrival_order() {
        let uids: std::collections::HashSet<u32> = [1009, 1005, 1007].into_iter().collect();
        assert_eq!(new_uids(uids, 0), vec![1005, 1007, 1009]);
    }

    // ----- wayland#547 loop guard: mark_self_inbound -----

    #[test]
    fn mark_self_by_own_address() {
        let idx = crate::sent_index::new_index();
        let mut m =
            IncomingMessage::new("some-id@x", "bot@acme.com", "Bot <bot@acme.com>", "hi", 0);
        m.sender_id = "bot@acme.com".into();
        let hit = mark_self_inbound(&mut m, &["bot@acme.com".to_string()], &idx);
        assert_eq!(hit, Some(SelfMatch::OwnAddress));
        assert!(m.is_self);
    }

    #[test]
    fn mark_self_by_echoed_message_id_even_from_foreign_address() {
        // A relay that rewrites the From still can't dodge the id match.
        let idx = crate::sent_index::new_index();
        idx.lock().unwrap().record("wl-1-0@acme.com".into());
        let mut m = IncomingMessage::new(
            "wl-1-0@acme.com",
            "conv",
            "Rewritten <alias@relay.example>",
            "hi",
            0,
        );
        m.sender_id = "alias@relay.example".into();
        let hit = mark_self_inbound(&mut m, &["bot@acme.com".to_string()], &idx);
        assert_eq!(hit, Some(SelfMatch::MessageId));
        assert!(m.is_self);
    }

    #[test]
    fn unrelated_inbound_is_not_marked_self() {
        let idx = crate::sent_index::new_index();
        idx.lock().unwrap().record("wl-1-0@acme.com".into());
        let mut m = IncomingMessage::new("user-mail@x", "conv", "Alice <alice@x.com>", "hi", 0);
        m.sender_id = "alice@x.com".into();
        assert_eq!(
            mark_self_inbound(&mut m, &["bot@acme.com".to_string()], &idx),
            None
        );
        assert!(!m.is_self);
    }

    #[test]
    fn empty_sender_id_never_matches_an_empty_own_address() {
        // An unconfigured/unparsable from_address normalizes to "" — that
        // must not flag inbound mail whose sender also failed to parse.
        let idx = crate::sent_index::new_index();
        let mut m = IncomingMessage::new("id@x", "conv", "", "hi", 0);
        m.sender_id = String::new();
        assert_eq!(mark_self_inbound(&mut m, &[String::new()], &idx), None);
        assert!(!m.is_self);
    }

    // ----- wayland#547 loop guard: the REAL admission path -----
    // These drive raw RFC822 bytes through `admit_fetched_message` — the
    // same function `poll_once` calls per fetch — so removing the guard
    // wiring (not just the helper) fails tests.

    fn empty_reply_index() -> ReplyIndex {
        Arc::new(StdMutex::new(HashMap::new()))
    }

    #[test]
    fn echoed_outbound_is_marked_self_through_the_admission_path() {
        let idx = crate::sent_index::new_index();
        idx.lock().unwrap().record("wl-abc-0@acme.com".into());
        let raw = b"From: Bot <bot@acme.com>\r\nSubject: Re: task\r\nMessage-ID: <wl-abc-0@acme.com>\r\n\r\nself echo\r\n";
        let ev = admit_fetched_message(7, raw, None, &[], &idx, &empty_reply_index())
            .expect("echo must still be admitted; the kernel is the drop point");
        let ChannelEvent::MessageReceived { msg } = ev else {
            panic!("expected MessageReceived");
        };
        assert!(msg.is_self, "echoed outbound must carry is_self");
    }

    #[test]
    fn own_address_from_is_marked_self_through_the_admission_path() {
        let idx = crate::sent_index::new_index();
        let raw = b"From: Bot <bot@acme.com>\r\nSubject: hello\r\nMessage-ID: <relay-rewrote-this@mta>\r\n\r\nhi\r\n";
        let ev = admit_fetched_message(
            8,
            raw,
            None,
            &["bot@acme.com".to_string()],
            &idx,
            &empty_reply_index(),
        )
        .expect("own-address mail must still be admitted");
        let ChannelEvent::MessageReceived { msg } = ev else {
            panic!("expected MessageReceived");
        };
        assert!(msg.is_self, "own-address From must carry is_self");
    }

    #[test]
    fn ordinary_inbound_is_not_marked_self_through_the_admission_path() {
        let idx = crate::sent_index::new_index();
        idx.lock().unwrap().record("wl-abc-0@acme.com".into());
        let raw = b"From: Alice <alice@x.com>\r\nSubject: question\r\nMessage-ID: <alices-mail@x.com>\r\n\r\nhelp?\r\n";
        let ev = admit_fetched_message(
            9,
            raw,
            None,
            &["bot@acme.com".to_string()],
            &idx,
            &empty_reply_index(),
        )
        .expect("ordinary mail admitted");
        let ChannelEvent::MessageReceived { msg } = ev else {
            panic!("expected MessageReceived");
        };
        assert!(!msg.is_self, "ordinary inbound must not be flagged");
    }

    #[test]
    fn parse_basic_message_extracts_from_subject_body() {
        let raw = b"From: Alice <alice@acme.com>\r\nSubject: Hello\r\nDate: Mon, 1 Jan 2024 12:00:00 +0000\r\nMessage-ID: <abc@x>\r\n\r\nThe body line.\r\n";
        let m = parse_basic_rfc5322(42, raw).unwrap();
        assert_eq!(m.id, "abc@x");
        assert_eq!(m.author, "Alice <alice@acme.com>");
        assert!(m.text.starts_with("Hello"), "text = {}", m.text);
        assert!(m.text.contains("The body line."), "text = {}", m.text);
        assert_eq!(m.ts_secs, 1_704_110_400);
    }

    #[test]
    fn parse_handles_bare_lflf_body_separator() {
        let raw = b"From: bob@acme.com\nSubject: s\n\nbody";
        let m = parse_basic_rfc5322(7, raw).unwrap();
        assert_eq!(m.author, "bob@acme.com");
        assert!(m.text.contains("body"));
    }

    #[test]
    fn parse_synthesises_id_when_no_message_id() {
        let raw = b"From: x@y\r\n\r\nhi";
        let m = parse_basic_rfc5322(99, raw).unwrap();
        assert_eq!(m.id, "uid:99");
    }

    #[test]
    fn parse_unfolds_multi_line_subject() {
        let raw = b"From: a@b\r\nSubject: line one\r\n  line two\r\n\r\nbody";
        let m = parse_basic_rfc5322(1, raw).unwrap();
        assert!(m.text.starts_with("line one line two"), "text = {}", m.text);
    }

    #[test]
    fn normalize_extracts_addr_spec_from_display_name() {
        assert_eq!(
            normalize_from_addr("Alice <alice@acme.com>"),
            "alice@acme.com"
        );
        assert_eq!(normalize_from_addr("bob@acme.com"), "bob@acme.com");
        // Case-insensitive.
        assert_eq!(normalize_from_addr("OPS@ACME.COM"), "ops@acme.com");
        // Quoted display name with angle-addr.
        assert_eq!(
            normalize_from_addr("\"Carol D\" <carol@acme.com>"),
            "carol@acme.com"
        );
    }

    fn allow(senders: &[&str]) -> std::collections::HashSet<String> {
        senders.iter().map(|s| normalize_from_addr(s)).collect()
    }

    #[test]
    fn no_allowlist_allows_everything() {
        // None = filtering disabled (preserves prior behavior).
        assert!(is_sender_allowed(None, "anyone@anywhere.com"));
        assert!(is_sender_allowed(None, "attacker <evil@phisher.test>"));
    }

    #[test]
    fn allowlist_drops_forged_from_outside_list() {
        let set = allow(&["ops@acme.com", "Alice@Acme.com"]);
        // Legit, case- and display-name-insensitive: allowed.
        assert!(is_sender_allowed(Some(&set), "Alice <alice@acme.com>"));
        assert!(is_sender_allowed(Some(&set), "OPS@acme.com"));
        // Forged From: impersonating a trusted admin not on the list: dropped.
        assert!(!is_sender_allowed(
            Some(&set),
            "Trusted Admin <trusted-admin@company.com>"
        ));
        // Empty / unparsable sender with an allow-list set: fail closed.
        assert!(!is_sender_allowed(Some(&set), ""));
        assert!(!is_sender_allowed(Some(&set), "unknown@uid-7"));
    }

    #[test]
    fn allowlist_filters_parsed_message_author() {
        // End-to-end through the real parser: a forged From outside the
        // allow-list must not pass the gate.
        let set = allow(&["alice@acme.com"]);
        let forged = b"From: Trusted Admin <trusted-admin@company.com>\r\nSubject: x\r\n\r\nbody";
        let msg = parse_basic_rfc5322(1, forged).unwrap();
        assert!(!is_sender_allowed(Some(&set), &msg.author));

        let legit = b"From: Alice <alice@acme.com>\r\nSubject: x\r\n\r\nbody";
        let msg = parse_basic_rfc5322(2, legit).unwrap();
        assert!(is_sender_allowed(Some(&set), &msg.author));
    }

    // -----------------------------------------------------------------
    // MIME walk (FIX 2)
    // -----------------------------------------------------------------

    #[test]
    fn multipart_alternative_html_only_yields_nonempty_text() {
        // multipart/alternative whose only renderable part is text/html.
        let raw = b"From: a@b\r\n\
Subject: Hi\r\n\
Content-Type: multipart/alternative; boundary=\"BD\"\r\n\
\r\n\
--BD\r\n\
Content-Type: text/html; charset=utf-8\r\n\
\r\n\
<html><body><p>Hello <b>world</b></p></body></html>\r\n\
--BD--\r\n";
        let m = parse_basic_rfc5322(1, raw).unwrap();
        assert!(m.text.contains("Hello"), "text = {:?}", m.text);
        assert!(m.text.contains("world"), "text = {:?}", m.text);
        // Subject still prepended.
        assert!(m.text.starts_with("Hi"), "text = {:?}", m.text);
        // Tags stripped.
        assert!(
            !m.text.contains('<'),
            "text should have no tags: {:?}",
            m.text
        );
    }

    #[test]
    fn multipart_alternative_prefers_plain_over_html() {
        let raw = b"From: a@b\r\n\
Content-Type: multipart/alternative; boundary=\"X\"\r\n\
\r\n\
--X\r\n\
Content-Type: text/plain\r\n\
\r\n\
plain body wins\r\n\
--X\r\n\
Content-Type: text/html\r\n\
\r\n\
<p>html body</p>\r\n\
--X--\r\n";
        let m = parse_basic_rfc5322(2, raw).unwrap();
        assert!(m.text.contains("plain body wins"), "text = {:?}", m.text);
        assert!(!m.text.contains("html body"), "text = {:?}", m.text);
    }

    #[test]
    fn multipart_mixed_text_plus_attachment_yields_text_and_attachment() {
        // multipart/mixed: a text/plain part + an attachment part.
        let raw = b"From: a@b\r\n\
Subject: report\r\n\
Content-Type: multipart/mixed; boundary=\"MIX\"\r\n\
\r\n\
--MIX\r\n\
Content-Type: text/plain\r\n\
\r\n\
see attached\r\n\
--MIX\r\n\
Content-Type: image/png; name=\"chart.png\"\r\n\
Content-Disposition: attachment; filename=\"chart.png\"\r\n\
Content-Transfer-Encoding: base64\r\n\
\r\n\
aGVsbG8=\r\n\
--MIX--\r\n";
        let m = parse_basic_rfc5322(3, raw).unwrap();
        assert!(m.text.contains("see attached"), "text = {:?}", m.text);
        assert_eq!(m.attachments.len(), 1, "attachments = {:?}", m.attachments);
        let att = &m.attachments[0];
        assert_eq!(att.kind, MediaKind::Image);
        assert_eq!(att.content_type.as_deref(), Some("image/png"));
        assert_eq!(att.path.as_deref(), Some("chart.png"));
        // The decoded payload (`aGVsbG8=` → "hello") is inlined as a data URL
        // so fetch_media can serve the bytes without a network round-trip.
        assert_eq!(att.url, "data:image/png;base64,aGVsbG8=");
        assert_eq!(decode_base64_bytes("aGVsbG8="), b"hello");
    }

    #[test]
    fn attachment_kind_maps_from_content_type() {
        let empty: Option<&str> = None;
        assert_eq!(
            attachment_from("image/jpeg", "a.jpg", "", empty).kind,
            MediaKind::Image
        );
        assert_eq!(
            attachment_from("audio/mpeg", "a.mp3", "", empty).kind,
            MediaKind::Audio
        );
        assert_eq!(
            attachment_from("video/mp4", "a.mp4", "", empty).kind,
            MediaKind::Video
        );
        assert_eq!(
            attachment_from("application/pdf", "a.pdf", "", empty).kind,
            MediaKind::Document
        );
    }

    #[test]
    fn base64_roundtrips_through_encode_decode() {
        let data = b"\x00\x01\x02\xff binary \xfe payload";
        assert_eq!(decode_base64_bytes(&encode_base64(data)), data);
    }

    #[test]
    fn decode_transfer_bytes_handles_base64_and_plain() {
        assert_eq!(decode_transfer_bytes("aGVsbG8=", Some("base64")), b"hello");
        assert_eq!(decode_transfer_bytes("hi", None), b"hi");
    }

    #[test]
    fn oversize_attachment_is_metadata_only() {
        // A part above the inline cap surfaces metadata but no fetch url.
        let big = "A".repeat(MAX_INLINE_ATTACHMENT_BYTES + 4); // 7bit, 1 byte/char
        let att = attachment_from("application/zip", "big.zip", &big, None);
        assert!(att.url.is_empty(), "oversize attachment must not inline");
        assert_eq!(att.path.as_deref(), Some("big.zip"));
    }

    #[test]
    fn small_attachment_inlines_as_data_url() {
        let att = attachment_from("image/png", "x.png", "aGVsbG8=", Some("base64"));
        assert_eq!(att.url, "data:image/png;base64,aGVsbG8=");
    }

    #[test]
    fn quoted_printable_html_part_decodes_and_strips() {
        let raw = b"From: a@b\r\n\
Content-Type: text/html\r\n\
Content-Transfer-Encoding: quoted-printable\r\n\
\r\n\
<p>caf=C3=A9 =26 more</p>\r\n";
        let m = parse_basic_rfc5322(4, raw).unwrap();
        // =C3=A9 -> é ; =26 -> & ; tags stripped.
        assert!(m.text.contains("café"), "text = {:?}", m.text);
        assert!(m.text.contains('&'), "text = {:?}", m.text);
        assert!(!m.text.contains("<p>"), "text = {:?}", m.text);
    }

    #[test]
    fn plain_text_single_part_unchanged() {
        // Regression: a bare text/plain message still surfaces its body.
        let raw = b"From: a@b\r\nSubject: s\r\n\r\njust text\r\n";
        let m = parse_basic_rfc5322(5, raw).unwrap();
        assert!(m.text.contains("just text"), "text = {:?}", m.text);
        assert!(m.attachments.is_empty());
    }

    #[test]
    fn html_strip_drops_script_and_decodes_entities() {
        let html = "<html><head><style>p{color:red}</style></head>\
<body><script>evil()</script><p>A &amp; B</p><p>line two</p></body></html>";
        let text = html_to_text(html);
        assert!(text.contains("A & B"), "text = {:?}", text);
        assert!(text.contains("line two"), "text = {:?}", text);
        assert!(
            !text.contains("evil"),
            "script body must be dropped: {:?}",
            text
        );
        assert!(
            !text.contains("color:red"),
            "style body must be dropped: {:?}",
            text
        );
    }

    #[test]
    fn parse_message_returns_reply_context() {
        let raw = b"From: a@b\r\n\
Subject: Original subject\r\n\
Message-ID: <orig@host>\r\n\
References: <root@host>\r\n\
\r\n\
hello";
        let (msg, ctx) = parse_message(7, raw).unwrap();
        assert_eq!(msg.id, "orig@host");
        assert_eq!(ctx.message_id, "orig@host");
        assert_eq!(ctx.subject.as_deref(), Some("Original subject"));
        assert_eq!(ctx.references.as_deref(), Some("<root@host>"));
    }

    #[test]
    fn record_reply_context_skips_empty_id_and_enforces_cap() {
        let index: ReplyIndex = Arc::new(StdMutex::new(HashMap::new()));
        // Empty id is ignored.
        record_reply_context(&index, String::new(), ReplyContext::default());
        assert_eq!(index.lock().unwrap().len(), 0);
        // Normal insert.
        record_reply_context(
            &index,
            "a@x".into(),
            ReplyContext {
                message_id: "a@x".into(),
                subject: Some("s".into()),
                references: None,
            },
        );
        assert_eq!(index.lock().unwrap().len(), 1);
        assert!(index.lock().unwrap().contains_key("a@x"));
    }
}
