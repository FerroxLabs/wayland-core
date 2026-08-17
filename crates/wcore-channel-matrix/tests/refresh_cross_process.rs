//! #936: the cross-process half of the Matrix refresh proof.
//!
//! Everything else about #936 is provable inside one process. This is not.
//! A Matrix refresh token ROTATES — `POST /_matrix/client/v3/refresh` hands
//! back a replacement and the presented one is spent — so the failure being
//! closed here is TWO OS PROCESSES both POSTing the same one. A spec-compliant
//! server treats the replay as theft and may revoke the whole authorization
//! grant (RFC 6819 §5.2.2.3): the cost is both processes logged out, not one
//! message failed. An in-process mutex cannot see a sibling process, so only a
//! test with two real processes can tell "coalesced" from "got lucky on
//! timing".
//!
//! **Shape.** The parent seeds ONE profile directory with an expired access
//! token and a live refresh token, stands up a local homeserver that refuses
//! the old token with `soft_logout: true`, and spawns two copies of this test
//! binary. Each child drives the PRODUCTION path — `MatrixChannel::start()`
//! then `send_message()` — over a real on-disk credentials store rooted at
//! that shared directory. The parent then counts how many refresh POSTs
//! actually arrived.
//!
//! **The homeserver is local.** Never a real one: a burned grant is not a
//! recoverable test failure.
//!
//! **On the red half.** This test is only worth anything if it can fail. It is
//! deliberately NOT paired with a "skip the lock" switch in the shipping
//! binary — a bypass of the control is a worse defect than the one it proves.
//! The red control is a SOURCE MUTANT applied out of tree: make
//! `TokenSource::renew` proceed without `ExclusiveFileLock::acquire` and this
//! test observes two POSTs.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use wcore_channel_matrix::{MatrixChannel, MatrixConfig};
use wcore_channels::Channel;
use wcore_channels::outgoing::OutgoingMessage;
use wcore_config::credentials::{CredentialsStore, PlaintextCredentialsStore};

const ROLE_ENV: &str = "WL936_XPROC_ROLE";
const ROOT_ENV: &str = "WL936_XPROC_ROOT";
const BASE_ENV: &str = "WL936_XPROC_BASE";
const START_ENV: &str = "WL936_XPROC_START_MS";

const ACCESS_HANDLE: &str = "matrix.xproc.access";
const REFRESH_HANDLE: &str = "matrix.xproc.refresh";
const EXPIRED_ACCESS: &str = "syt_expired_xproc";
const SEEDED_REFRESH: &str = "rot_seeded_xproc";
const RENEWED_ACCESS: &str = "syt_renewed_xproc";
const ROTATED_REFRESH: &str = "rot_rotated_xproc";
const ROOM: &str = "!xproc:matrix.example.org";

fn now_ms() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock")
        .as_millis()
}

fn cfg(base: &str) -> MatrixConfig {
    MatrixConfig {
        homeserver_url: base.to_string(),
        credential_handle_access_token: ACCESS_HANDLE.to_string(),
        credential_handle_refresh_token: Some(REFRESH_HANDLE.to_string()),
        user_id: "@xproc:matrix.example.org".to_string(),
    }
}

fn store_at(root: &std::path::Path) -> Arc<dyn CredentialsStore> {
    Arc::new(PlaintextCredentialsStore::new(
        root.join("credentials.toml"),
    ))
}

/// The child half. Runs in a SEPARATE OS PROCESS; the parent below spawns two.
///
/// Named as a test so the harness will run it, but inert unless the parent set
/// `ROLE_ENV` — otherwise every ordinary `cargo nextest` run would try to reach
/// a homeserver that is not there.
#[tokio::test]
async fn xproc_child_entrypoint() {
    let Ok(role) = std::env::var(ROLE_ENV) else {
        return;
    };
    assert_eq!(role, "child", "unexpected role");

    let root = PathBuf::from(std::env::var(ROOT_ENV).expect("root"));
    let base = std::env::var(BASE_ENV).expect("homeserver base");
    let start_at: u128 = std::env::var(START_ENV)
        .expect("start barrier")
        .parse()
        .expect("start barrier is millis");

    let mut channel = MatrixChannel::with_base("xproc", cfg(&base), store_at(&root), base);
    channel.start().await.expect("start reads the seeded token");

    // Barrier. Both children have read the SAME expired access token by now,
    // so releasing them together is what puts two live refreshes in flight at
    // once — which is the only condition under which the cross-process lock is
    // the thing doing the work rather than luck.
    while now_ms() < start_at {
        tokio::time::sleep(Duration::from_millis(2)).await;
    }

    // The production send path: 401 `soft_logout` -> renew -> retry.
    let receipt = channel
        .send_message(OutgoingMessage::text(ROOM, "cross-process refresh"))
        .await;
    let _ = channel.stop().await;

    // A LOSER that could not take the lock is still required to end up
    // authenticated by adopting the winner's token. Failing here is a real
    // failure, not an acceptable outcome.
    let receipt = receipt.expect("the send must succeed after the renewal");
    println!("XPROC_CHILD_OK {}", receipt.id);
}

/// Two processes, one credentials store, one expired access token: exactly ONE
/// refresh POST reaches the homeserver, and BOTH processes end up able to send.
///
/// The assertion that carries the obligation is the POST COUNT. Asserting only
/// that both children "succeeded" would pass just as well if the loser quietly
/// POSTed the spent refresh token a second time and got a fresh pair back —
/// which is precisely the grant-burning replay the lock exists to prevent.
#[tokio::test]
async fn two_processes_issue_exactly_one_refresh_post() {
    if std::env::var(ROLE_ENV).is_ok() {
        return; // this process IS a child; the entrypoint above does the work
    }

    let profile = tempfile::tempdir().expect("profile dir");
    let root = profile.path().to_path_buf();

    // Seed the shared, on-disk pair both processes will read.
    let seed = store_at(&root);
    seed.put(ACCESS_HANDLE, EXPIRED_ACCESS)
        .expect("seed access");
    seed.put(REFRESH_HANDLE, SEEDED_REFRESH)
        .expect("seed refresh");

    let mut server = mockito::Server::new_async().await;

    // The aged-out token is refused with the marker that says "refreshable".
    let refused = server
        .mock("PUT", mockito::Matcher::Any)
        .match_header("authorization", format!("Bearer {EXPIRED_ACCESS}").as_str())
        .with_status(401)
        .with_header("content-type", "application/json")
        .with_body(r#"{"errcode":"M_UNKNOWN_TOKEN","soft_logout":true}"#)
        .expect_at_least(1)
        .create_async()
        .await;

    // The counting endpoint. `expect(1)` is the whole test: a second arrival
    // fails `assert_async` below and names the count it saw.
    let refresh = server
        .mock("POST", "/_matrix/client/v3/refresh")
        .with_status(200)
        .with_header("content-type", "application/json")
        .with_body(format!(
            r#"{{"access_token":"{RENEWED_ACCESS}","refresh_token":"{ROTATED_REFRESH}","expires_in_ms":3600000}}"#
        ))
        .expect(1)
        .create_async()
        .await;

    // Only the renewed token is served.
    let served = server
        .mock("PUT", mockito::Matcher::Any)
        .match_header("authorization", format!("Bearer {RENEWED_ACCESS}").as_str())
        .with_status(200)
        .with_header("content-type", "application/json")
        .with_body(r#"{"event_id":"$xproc_ok"}"#)
        .expect_at_least(2)
        .create_async()
        .await;

    // Enough slack for two cold test binaries to reach the barrier.
    let start_at = now_ms() + 4_000;

    let exe = std::env::current_exe().expect("test binary path");
    let children: Vec<std::process::Child> = (0..2)
        .map(|_| {
            std::process::Command::new(&exe)
                .arg("xproc_child_entrypoint")
                .arg("--exact")
                .arg("--nocapture")
                .env(ROLE_ENV, "child")
                .env(ROOT_ENV, &root)
                .env(BASE_ENV, server.url())
                .env(START_ENV, start_at.to_string())
                // Same profile home => same refresh lock path. Passed in
                // rather than set in-process, so nothing here mutates the
                // environment of a test running beside it.
                .env("WAYLAND_HOME", &root)
                .spawn()
                .expect("spawn a second OS process")
        })
        .collect();

    for (index, mut child) in children.into_iter().enumerate() {
        let status = child.wait().expect("child exited");
        assert!(
            status.success(),
            "child {index} failed; a loser that cannot take the lock must still \
             end up authenticated by adopting the winner's token"
        );
    }

    refused.assert_async().await;
    served.assert_async().await;
    // The symptom, named: "Expected 1 request(s) ... but received 2" means the
    // single-use refresh token was replayed across processes.
    refresh.assert_async().await;

    // The winner's rotated pair must be what is on disk, so the loser adopted
    // it rather than leaving a spent token behind for the next start.
    let stored = store_at(&root);
    assert_eq!(
        stored.get(REFRESH_HANDLE).expect("read").as_deref(),
        Some(ROTATED_REFRESH),
        "the rotated refresh token must be persisted; the seeded one surviving \
         here means the next start replays a token the homeserver already spent"
    );
    assert_eq!(
        stored.get(ACCESS_HANDLE).expect("read").as_deref(),
        Some(RENEWED_ACCESS),
        "the renewed access token must be persisted for the next start",
    );
}
