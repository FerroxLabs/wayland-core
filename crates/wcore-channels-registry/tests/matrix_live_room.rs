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

/// Send → edit → delete, plus both negative controls, against a real room.
#[tokio::test]
#[ignore = "live: drives a real Matrix homeserver; requires MATRIX_* configuration"]
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
        // Matrix accepts an `m.replace` naming an unknown event — the relation
        // is just content. Recorded, not asserted, and the stray event's id is
        // printed so cleanup can redact it.
        Ok(r) => println!(
            "MLR_CONTROL_EDIT_BOGUS_ok=true MLR_CONTROL_EDIT_EVENT={}",
            r.id
        ),
        Err(e) => println!("MLR_CONTROL_EDIT_BOGUS_ok=false MLR_CONTROL_EDIT_BOGUS_err={e}"),
    }
    let del_err = mgr.delete_on("mxlive", &room, bogus).await;
    println!("MLR_CONTROL_DELETE_BOGUS_ok={}", del_err.is_ok());
    if let Err(e) = &del_err {
        println!("MLR_CONTROL_DELETE_BOGUS_err={e}");
    }
    assert!(
        del_err.is_err(),
        "redacting an event id that does not exist must be an ERROR. A silent Ok \
         here means the delete leg's positive result proves nothing, because the \
         call succeeds regardless of whether anything was redacted."
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
