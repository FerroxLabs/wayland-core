//! LIVE Discord native actions — edit and delete against the real platform.
//!
//! # Why this file exists
//!
//! `native_action_matrix.rs` proves Discord *declares* `edit` and `delete` as
//! [`Implemented`] and that the declaration is not its own witness. What it
//! cannot prove is that the two `PATCH` / `DELETE` calls actually work at
//! Discord, because it never contacts Discord. Six lanes closed Phase 24 with
//! the same sentence: *no message was ever sent or received against a live
//! platform.*
//!
//! # Why it is not driven through the binary
//!
//! Because it cannot be. Measured 2026-07-30 with `/usr/bin/grep`:
//! `.edit_message(` / `.delete_message(` have **zero** call sites in
//! `wcore-cli`, `wcore-gateway`, `wcore-agent`, `wcore-tools` and
//! `wcore-protocol` (known-positive in the same search: `.send_message(`, 6
//! hits), and the manager wrappers `edit_on` / `delete_on` are called **only
//! from tests**. `wayland-core channel` offers `list / probe / health / reload
//! / actions` and no verb that edits or deletes. So the capability is real at
//! the adapter and unreachable from the product — tracked as `F24-C3-D1`.
//!
//! This test therefore drives the **production registration path** —
//! [`auto_register_from_dir`], the same function `gateway run` and
//! `gateway resend` call (`wcore-cli/src/gateway.rs:929`) — over a real
//! on-disk channel config with a real credential. It is the closest thing to
//! the product that exists until an operator surface is built.
//!
//! # Running it
//!
//! `#[ignore]` by default; it needs a real bot token and posts real messages.
//!
//! ```text
//! WL_LIVE_DISCORD_HOME=/path/to/home WL_LIVE_DISCORD_CHANNEL=<snowflake> \
//!   cargo test -p wcore-channels-registry --test live_discord_actions -- --ignored --nocapture
//! ```
//!
//! **It never returns early.** An env-gated `return` is the shape that printed
//! `5 passed` for zero work in `live_integrity.rs`; a missing variable
//! `panic!`s here instead, so a misconfigured run is loudly red rather than
//! quietly green.

use std::sync::Arc;

use wcore_channels::{ChannelManager, OutgoingMessage};
use wcore_config::credentials::{CredentialsStore, PlaintextCredentialsStore};

/// Required env var, or a loud failure. Never a silent skip.
fn required(var: &str) -> String {
    match std::env::var(var) {
        Ok(v) if !v.trim().is_empty() => v,
        _ => panic!(
            "{var} is not set. This test contacts real Discord and refuses to \
             pass without it — a skip that reports success is the failure mode \
             this suite exists to avoid."
        ),
    }
}

/// Register the real adapters exactly as `gateway run` does.
async fn production_manager(home: &str) -> ChannelManager {
    let mut mgr = ChannelManager::new();
    let creds: Arc<dyn CredentialsStore> = Arc::new(PlaintextCredentialsStore::new(
        std::path::Path::new(home).join("credentials.toml"),
    ));
    let registered = wcore_channels_registry::auto_register_from_dir(
        &mut mgr,
        &std::path::Path::new(home).join("channels"),
        creds,
    )
    .await
    .expect("auto_register_from_dir must succeed against a real channels dir");
    assert_eq!(
        registered, 1,
        "expected exactly one registered channel; registering zero is how this \
         test would pass while measuring nothing"
    );
    mgr.start_all()
        .await
        .expect("start_all must succeed — registering does NOT connect an adapter");
    mgr
}

/// send → edit → delete, each corroborated by reading the platform back
/// through the adapter's own REST surface is NOT possible (the adapter exposes
/// no read), so corroboration is by receipt identity plus the caller's own
/// independent observer, run outside this process. What this test proves is
/// that the two calls SUCCEED at real Discord rather than returning
/// `Unsupported` or an HTTP error.
#[tokio::test]
#[ignore = "contacts real Discord and posts real messages"]
async fn live_edit_and_delete_against_real_discord() {
    let home = required("WL_LIVE_DISCORD_HOME");
    let chan = required("WL_LIVE_DISCORD_CHANNEL");
    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs();

    let mgr = production_manager(&home).await;

    // ---- capability: the adapter says it can do this at all.
    let actions = mgr
        .native_actions_on(&chan)
        .await
        .expect("the registered channel must report a native-action matrix");
    assert_eq!(
        actions.edit,
        wcore_channels::ActionSupport::Implemented,
        "precondition: Discord must declare edit implemented"
    );
    assert_eq!(
        actions.delete,
        wcore_channels::ActionSupport::Implemented,
        "precondition: Discord must declare delete implemented"
    );

    // ---- 1. send, so there is something real to act on.
    let original = format!("WL-LIVE-ACTIONS-{stamp}-original");
    let receipt = mgr
        .send_to(&chan, OutgoingMessage::text(chan.clone(), original.clone()))
        .await
        .expect("live send must succeed");
    println!("LIVE_SENT id={} conv={}", receipt.id, receipt.conversation_id);
    assert!(
        !receipt.id.is_empty(),
        "a receipt with no message id cannot be edited or deleted"
    );

    // ---- 2. EDIT. A PATCH at Discord returns the SAME message id.
    let edited = format!("WL-LIVE-ACTIONS-{stamp}-EDITED");
    let edit_receipt = mgr
        .edit_on(&chan, &chan, &receipt.id, &edited)
        .await
        .expect("live edit must succeed at real Discord");
    println!("LIVE_EDITED id={}", edit_receipt.id);
    assert_eq!(
        edit_receipt.id, receipt.id,
        "an edit must return the id it edited, not create a new message"
    );

    // ---- 3. KNOWN-NEGATIVE for edit: a message id that does not exist must
    //         FAIL. Without this the edit leg would pass on a dead client.
    let bogus = mgr
        .edit_on(&chan, &chan, "000000000000000001", "should never land")
        .await;
    assert!(
        bogus.is_err(),
        "editing a nonexistent message id must fail; it returned Ok, so the \
         success above proves nothing"
    );
    println!("LIVE_EDIT_KNOWN_NEGATIVE_ERR={:?}", bogus.unwrap_err());

    // ---- 4. DELETE.
    mgr.delete_on(&chan, &chan, &receipt.id)
        .await
        .expect("live delete must succeed at real Discord");
    println!("LIVE_DELETED id={}", receipt.id);

    // ---- 5. KNOWN-NEGATIVE for delete: deleting the SAME id again must now
    //         fail, which is also the strongest available in-process proof
    //         that the delete really removed it rather than returning a
    //         cosmetic 204.
    let again = mgr.delete_on(&chan, &chan, &receipt.id).await;
    assert!(
        again.is_err(),
        "deleting an already-deleted message must fail; if it succeeds, the \
         first delete cannot be shown to have done anything"
    );
    println!("LIVE_DELETE_KNOWN_NEGATIVE_ERR={:?}", again.unwrap_err());

    println!("LIVE_ACTIONS_ALL_PASSED stamp={stamp} deleted_id={}", receipt.id);
}
