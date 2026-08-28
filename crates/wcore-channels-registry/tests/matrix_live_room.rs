//! LIVE Matrix room drive — the production adapter, a real homeserver.
//!
//! # Why this file exists, and what it is NOT
//!
//! `24-C3` has been declined by six lanes with the same sentence: *no message
//! was ever sent or received against a live platform.* The prior Matrix
//! measurement (`24-C1-abandon-surface/synapse-measure.sh`) drove **curl**
//! against a container Synapse — it established what the *protocol* does and
//! nothing at all about what *this adapter* does.
//!
//! This file drives the adapter the gateway builds: constructed by the
//! production loader [`auto_register_from_dir`] from real on-disk channel TOML,
//! reached through the production [`ChannelManager`], pointed at a real
//! homeserver.
//!
//! It is NOT the `wayland-core` binary, and that is a **reported defect, not a
//! shortcut**. Measured with `/usr/bin/grep` over `crates/` against a
//! same-shape known-positive:
//!
//! | manager method | production (non-test) callers |
//! |---|---|
//! | `react_on` | 2 — `wcore-agent/src/channel_inbound.rs:520,553` |
//! | `send_to`  | 2 — `channel_send_transport.rs:90`, `channel_inbound.rs:588` |
//! | **`edit_on`**   | **0** |
//! | **`delete_on`** | **0** |
//!
//! So no CLI verb, agent tool, gateway path or protocol command can invoke a
//! message edit or delete anywhere in the shipped product, while
//! `wayland-core channel actions --require edit` exists and gates on it. The
//! send leg IS driven through the binary — see `scripts/matrix-live-drive.sh`;
//! only edit and delete have no binary-level path to drive.
//!
//! # Anti-vacuity
//!
//! LANE-BRIEF §3.2 names the env-gated early `return` as a measured
//! self-passing flavour (`live_integrity.rs` printed `5 passed` for zero work).
//! This test is `#[ignore]`d so it does not run unasked, and when it IS asked
//! for it **panics on missing configuration** rather than returning green.
//! There is no path through this file that reports success without having
//! talked to a homeserver.
//!
//! Run:
//! ```text
//! MATRIX_LIVE=1 MATRIX_ACCESS_TOKEN=… MATRIX_ROOM_ID=… MATRIX_USER_ID=… \
//! MATRIX_HOMESERVER=https://matrix.org \
//!   cargo test -p wcore-channels-registry --test matrix_live_room -- --ignored --nocapture
//! ```
//!
//! Every event id it creates is printed with a stable `MLR_` prefix so the
//! independent observer (`scripts/matrix-live-observer.mjs`, a different OS
//! process talking directly to the homeserver) can grade the result. **Nothing
//! in this file grades itself** beyond the assertions that a call did not error.

use std::sync::Arc;

use wcore_channels::ChannelManager;
use wcore_channels::outgoing::OutgoingMessage;
use wcore_channels_registry::auto_register_from_dir;
use wcore_config::credentials::{CredentialsError, CredentialsStore};

const HANDLE: &str = "matrix.live.access_token";

/// Credentials store backed by the process environment.
///
/// Deliberately not a file. LANE-BRIEF §0 requires a real credential to reach a
/// build host on stdin and never to be written to disk; the product's other
/// stores are file- or keyring-backed, so this leg supplies the same
/// `Arc<dyn CredentialsStore>` the production loader takes while keeping the
/// token in memory only. The ADAPTER is production; only the store is not.
struct EnvCreds;

impl CredentialsStore for EnvCreds {
    fn get(&self, key: &str) -> Result<Option<String>, CredentialsError> {
        if key == HANDLE {
            return Ok(std::env::var("MATRIX_ACCESS_TOKEN").ok());
        }
        Ok(None)
    }
    fn put(&self, _key: &str, _value: &str) -> Result<(), CredentialsError> {
        Ok(())
    }
    fn delete(&self, _key: &str) -> Result<(), CredentialsError> {
        Ok(())
    }
}

fn required(var: &str) -> String {
    std::env::var(var).unwrap_or_else(|_| {
        panic!(
            "{var} is not set. This test is #[ignore]d and only runs when asked \
             for explicitly; running it without live configuration is a FAILURE, \
             never a skip (LANE-BRIEF §3.2)."
        )
    })
}

/// Build the production adapter set from real on-disk channel TOML.
async fn production_manager(dir: &std::path::Path, user_id: &str, base: &str) -> ChannelManager {
    std::fs::create_dir_all(dir).unwrap();
    std::fs::write(
        dir.join("mxlive.toml"),
        format!(
            r#"name = "mxlive"
platform = "matrix"
enabled = true

[options]
homeserver_url = "{base}"
credential_handle_access_token = "{HANDLE}"
user_id = "{user_id}"
"#
        ),
    )
    .unwrap();

    let mut mgr = ChannelManager::new();
    let creds: Arc<dyn CredentialsStore> = Arc::new(EnvCreds);
    let n = auto_register_from_dir(&mut mgr, dir, creds)
        .await
        .expect("the production loader must build the matrix adapter");
    assert_eq!(n, 1, "expected exactly one adapter from {}", dir.display());
    mgr.start_all().await.expect("start_all");
    mgr
}

/// Watch the manager's production event fan-out for one nonce, bounded.
///
/// Returns the arriving message's text, or `None` on timeout. `subscribe()` is
/// the same broadcast the gateway's inbound stack consumes, so an arrival here
/// is an arrival at the product — not at a test-only hook.
async fn await_nonce(
    mut rx: tokio::sync::broadcast::Receiver<wcore_channels::TaggedEvent>,
    nonce: &str,
    budget: std::time::Duration,
) -> Option<(String, String)> {
    let deadline = tokio::time::Instant::now() + budget;
    loop {
        let left = deadline.saturating_duration_since(tokio::time::Instant::now());
        if left.is_zero() {
            return None;
        }
        match tokio::time::timeout(left, rx.recv()).await {
            Err(_) => return None,
            Ok(Err(e)) => {
                println!("MLR_INBOUND_RECV_ERR={e}");
                return None;
            }
            Ok(Ok(tagged)) => {
                if let wcore_channels::event::ChannelEvent::MessageReceived { msg } = tagged.event {
                    println!(
                        "MLR_INBOUND_SAW id={} sender={} text_len={}",
                        msg.id,
                        msg.sender_id,
                        msg.text.len()
                    );
                    if msg.text.contains(nonce) {
                        return Some((msg.text, msg.sender_id));
                    }
                }
            }
        }
    }
}

/// INBOUND — an event already in the real room reaches the product, with
/// non-empty content, through the production `/sync` loop.
///
/// # The single-account constraint, stated rather than worked around
///
/// `sync.rs:414-416` drops every event whose sender equals the adapter's
/// configured `user_id`, to prevent self-loops. This programme holds exactly
/// one Matrix account, so the probe's sender IS that account and the filter
/// would discard it. `MATRIX_PROBE_USER_ID` therefore names a different mxid.
/// The token and the `user_id` field are independent config inputs, so this is
/// a real configuration and not a patched binary.
///
/// It also buys the control this leg would otherwise lack. The SAME event is
/// then offered to a second production adapter whose `user_id` IS the real
/// sender, and must NOT arrive. Without that, "the message arrived" would be
/// indistinguishable from "the filter is broken", and a green would be
/// available to an adapter that admits everything.
#[tokio::test]
#[ignore = "live: drives a real Matrix homeserver; requires MATRIX_* configuration"]
// SERIAL, and it is not decoration. This test repoints the process-global
// `WAYLAND_HOME` twice, and the matrix adapter persists its /sync cursor under
// `$WAYLAND_HOME/channel-state/`. Both live tests in this binary build a
// production adapter, and `cargo test` runs them as threads of ONE process, so
// unserialized they steer each other's cursor into a `TempDir` the other is
// about to delete. Serializing BOTH is what makes the write safe; serializing
// only the writer would still race the reader.
#[serial_test::serial(wayland_home)]
async fn matrix_inbound_reaches_the_product_from_a_real_room() {
    assert_eq!(required("MATRIX_LIVE"), "1");
    let base = required("MATRIX_HOMESERVER");
    let sender = required("MATRIX_USER_ID");
    let probe_user = required("MATRIX_PROBE_USER_ID");
    let nonce = required("MATRIX_INBOUND_NONCE");
    let _ = required("MATRIX_ACCESS_TOKEN");
    assert_ne!(
        probe_user, sender,
        "MATRIX_PROBE_USER_ID must differ from the sender or the self-echo \
         filter makes this leg unobservable"
    );

    let budget = std::time::Duration::from_secs(90);

    // ---- positive: a distinct configured user_id, so the event is admitted
    let tmp_a = tempfile::tempdir().unwrap();
    // A fresh WAYLAND_HOME per adapter: the /sync cursor persists under
    // `$WAYLAND_HOME/channel-state/`, and a resumed cursor would skip the very
    // event this leg is waiting for.
    unsafe { std::env::set_var("WAYLAND_HOME", tmp_a.path()) };
    let mgr_a = production_manager(&tmp_a.path().join("channels"), &probe_user, &base).await;
    let rx_a = mgr_a.subscribe();
    let got = await_nonce(rx_a, &nonce, budget).await;
    match &got {
        Some((text, from)) => println!(
            "MLR_INBOUND_ARRIVED=true text_len={} sender={} text={:?}",
            text.len(),
            from,
            text
        ),
        None => println!("MLR_INBOUND_ARRIVED=false"),
    }

    // ---- negative control: ONE variable changed — user_id is now the sender
    let tmp_b = tempfile::tempdir().unwrap();
    unsafe { std::env::set_var("WAYLAND_HOME", tmp_b.path()) };
    let mgr_b = production_manager(&tmp_b.path().join("channels"), &sender, &base).await;
    let rx_b = mgr_b.subscribe();
    // A shorter budget is fine and is NOT the reason this must be empty: the
    // positive above proves the event is reachable within `budget`, and the
    // initial /sync that serves it is the first request either adapter makes.
    let echoed = await_nonce(rx_b, &nonce, std::time::Duration::from_secs(45)).await;
    println!("MLR_INBOUND_SELF_ECHO_ADMITTED={}", echoed.is_some());

    let (text, from) = got.expect(
        "the probe event did not reach the product within 90s. This is the leg \
         six lanes said had never been driven; a timeout here is a FAILURE, not \
         a skip.",
    );
    assert!(
        !text.trim().is_empty(),
        "arrived with EMPTY content — an arrival with no body is not an arrival"
    );
    assert_eq!(from, sender, "the sender identity must survive the parse");
    assert!(
        echoed.is_none(),
        "the self-echo filter admitted the adapter's own user — the positive \
         result above would then prove nothing about admission"
    );
    println!("MLR_INBOUND_DONE");
}

/// Send → edit → delete, plus both negative controls, against a real room.
#[tokio::test]
#[ignore = "live: drives a real Matrix homeserver; requires MATRIX_* configuration"]
// The other half of the pair above: this test READS `WAYLAND_HOME` through the
// production adapter, so it has to hold the same lock the writer does.
#[serial_test::serial(wayland_home)]
async fn matrix_edit_and_delete_against_a_real_room() {
    assert_eq!(
        required("MATRIX_LIVE"),
        "1",
        "MATRIX_LIVE must be exactly 1 to drive a real room"
    );
    let room = required("MATRIX_ROOM_ID");
    let user_id = required("MATRIX_USER_ID");
    let base = required("MATRIX_HOMESERVER");
    let nonce = required("MATRIX_NONCE");
    let _ = required("MATRIX_ACCESS_TOKEN");

    let tmp = tempfile::tempdir().unwrap();
    // Own `WAYLAND_HOME`, stated rather than inherited: the adapter's /sync
    // cursor lands under it, and inheriting whatever the sibling test left
    // behind means writing into a deleted directory.
    unsafe { std::env::set_var("WAYLAND_HOME", tmp.path()) };
    let mgr = production_manager(tmp.path(), &user_id, &base).await;

    println!("MLR_ROOM={room}");
    println!("MLR_NONCE={nonce}");
    println!(
        "MLR_DECLARED_IDEMPOTENT={}",
        mgr.supports_outbound_idempotency("mxlive").await
    );
    let actions = mgr.native_actions_on("mxlive").await.expect("declaration");
    println!(
        "MLR_DECLARED_ACTIONS edit={} delete={} react={} typing={}",
        actions.edit.as_str(),
        actions.delete.as_str(),
        actions.react.as_str(),
        actions.typing.as_str()
    );

    // ---------------------------------------------------------------- send
    // Two originals: one the edit leg mutates, one the delete leg redacts.
    // Editing and then redacting the same event would leave the edit leg's
    // read-back ungradeable, because a redaction strips what the edit produced.
    let edit_target = mgr
        .send_to(
            "mxlive",
            OutgoingMessage::text(
                &room,
                format!("wayland-core live probe {nonce} edit-target"),
            ),
        )
        .await
        .expect("send edit-target");
    println!("MLR_SENT_EDIT_TARGET={}", edit_target.id);

    let delete_target = mgr
        .send_to(
            "mxlive",
            OutgoingMessage::text(
                &room,
                format!("wayland-core live probe {nonce} delete-target"),
            ),
        )
        .await
        .expect("send delete-target");
    println!("MLR_SENT_DELETE_TARGET={}", delete_target.id);

    // ---------------------------------------------------------------- edit
    let replacement = mgr
        .edit_on(
            "mxlive",
            &room,
            &edit_target.id,
            &format!("wayland-core live probe {nonce} edit-target EDITED"),
        )
        .await
        .expect("edit_on must reach the m.replace route");
    println!("MLR_EDIT_REPLACEMENT={}", replacement.id);
    assert_ne!(
        replacement.id, edit_target.id,
        "an m.replace edit is a NEW event; the same id back would mean nothing was created"
    );

    // -------------------------------------------------------------- delete
    mgr.delete_on("mxlive", &room, &delete_target.id)
        .await
        .expect("delete_on must reach the redact route");
    println!("MLR_DELETED={}", delete_target.id);

    // ------------------------------------------------- negative controls
    // Both are one-variable changes off the calls above. Without them a
    // homeserver that answered 200 to everything would produce the same green.
    let bogus = "$this-event-id-was-never-created-by-anyone";
    let edit_ctl = mgr
        .edit_on(
            "mxlive",
            &room,
            bogus,
            &format!("wayland-core live probe {nonce} control-edit-of-nothing"),
        )
        .await;
    match &edit_ctl {
        Ok(r) => println!(
            "MLR_CONTROL_EDIT_BOGUS_ok=true MLR_CONTROL_EDIT_EVENT={}",
            r.id
        ),
        Err(e) => println!("MLR_CONTROL_EDIT_BOGUS_ok=false MLR_CONTROL_EDIT_BOGUS_err={e}"),
    }
    // The edit control DOES redden: matrix.org rejects a relation to an unknown
    // event with `400 M_UNKNOWN "Can't send relation to unknown event"`
    // (measured 2026-07-30). So `edit_on` returning Ok is informative.
    assert!(
        edit_ctl.is_err(),
        "editing an event that does not exist must be an ERROR; a silent Ok would \
         make the edit leg's positive result meaningless"
    );

    let del_ctl = mgr.delete_on("mxlive", &room, bogus).await;
    println!("MLR_CONTROL_DELETE_BOGUS_ok={}", del_ctl.is_ok());
    if let Err(e) = &del_ctl {
        println!("MLR_CONTROL_DELETE_BOGUS_err={e}");
    }
    // ── F-ML-3, and this assertion is the measurement, not a prediction ──
    //
    // This started out as `assert!(del_ctl.is_err())` — the mirror of the edit
    // control — and it FAILED. matrix.org answers `200 {"event_id": …}` to a
    // redaction of an event id that never existed, corroborated independently
    // by curl outside the product. So the delete path is NOT symmetric with the
    // edit path: an `Ok(())` from `delete_message` is compatible with nothing
    // whatsoever having been redacted.
    //
    // `rest.rs:342-349` says the operation "reports success when the homeserver
    // accepted the redaction, which is the strongest guarantee the protocol
    // offers". Acceptance turns out to guarantee nothing, so that sentence
    // overstates what the caller gets.
    //
    // The assertion is therefore INVERTED to pin the measured platform
    // behaviour, and it is still a live gate in both directions: if matrix.org
    // ever starts rejecting these, this reddens and the note above must be
    // revisited. What it must never become is absent — deleting it would leave
    // the delete leg graded by a status code that carries no information.
    assert!(
        del_ctl.is_ok(),
        "measured 2026-07-30: matrix.org accepts a redaction of a nonexistent \
         event with 200. If this now errors, the platform changed and F-ML-3 \
         needs re-measuring."
    );
    println!(
        "MLR_FINDING_F_ML_3=redaction-of-nonexistent-event-accepted \
         delete_status_carries_no_information=true"
    );

    println!("MLR_DONE");
}

/// The transaction id is stable across PROCESS lifetimes — the property the
/// exactly-once row depends on and the one a process-local counter destroyed.
///
/// This test does not touch the network. It runs in a **different process** on
/// every `cargo test` invocation, so a `#[test]` that pins the derived id to a
/// literal is a genuine cross-restart assertion: the value could only drift if
/// the derivation stopped being a pure function of the key.
///
/// The live half — that the same id on the wire really does collapse at
/// matrix.org — is `scripts/matrix-live-drive.sh` leg 5, which kills the
/// gateway mid-send and reads the room back.
#[test]
fn the_derived_transaction_id_is_a_pure_function_of_the_delivery_key() {
    // Recomputed here from the algorithm in `wcore-channel-matrix/src/rest.rs`
    // rather than imported, because `txn_id_for_key` is `pub(crate)`. Any change
    // to the derivation must break this, which is the point.
    fn expected(key: &str) -> String {
        let path_safe = !key.is_empty()
            && key.len() <= 64
            && key
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.' | b':'));
        if path_safe {
            return key.to_string();
        }
        let mut h: u64 = 0xcbf2_9ce4_8422_2325;
        for b in key.as_bytes() {
            h ^= *b as u64;
            h = h.wrapping_mul(0x0000_0100_0000_01b3);
        }
        format!("wl-{h:016x}")
    }

    // A real delivery id, in the shape `wcore-cron/src/runner.rs:324-338` mints.
    let key = "cron:mxlive-job:1785121776528";
    assert_eq!(
        expected(key),
        "cron:mxlive-job:1785121776528",
        "a path-safe delivery key must reach the wire verbatim, in every process"
    );
    // Known-negative: a key that is NOT path-safe must hash — and must hash to
    // this exact value, which was computed in a DIFFERENT process (node, at
    // authoring time) and committed. A clock- or counter-seeded id cannot match
    // a literal pinned before the test process existed, which is precisely the
    // defect the restart-unstable `AtomicU64` counter had.
    let hostile = "cron:job with spaces/and slashes:1785121776528";
    assert_eq!(expected(hostile), "wl-6b522756ee4e5490");
    // …and the two branches really are different code paths.
    assert_ne!(expected(hostile), hostile);
    assert_eq!(expected(""), "wl-cbf29ce484222325");
}
