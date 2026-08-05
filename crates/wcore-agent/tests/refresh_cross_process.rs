//! #172 P1/P3: the cross-process refresh proof.
//!
//! Everything else about #172 is provable in one process. This is not. A
//! rotating refresh token is single-use, and the failure being closed here is
//! TWO OS PROCESSES both POSTing the same one — after which a spec-compliant
//! provider may revoke the entire grant (RFC 6819 §5.2.2.3). An in-process
//! `SingleFlightRefresh` cannot see a sibling process, so only a test with two
//! real processes can distinguish "coalesced" from "got lucky on timing".
//!
//! **Shape.** The parent seeds ONE profile directory with an EXPIRED pair,
//! starts a counting token endpoint on loopback, then spawns two copies of this
//! same test binary. Each child builds a real `ChatGptTokenManager` over a real
//! on-disk store rooted at that shared directory and calls `get()`, which is
//! the production path that notices expiry and refreshes. The parent then
//! counts how many POSTs actually reached the endpoint.
//!
//! **The endpoint is local.** Never the real provider — a burned grant is not
//! a recoverable test failure.
//!
//! **On the red half.** `p1_two_processes_issue_exactly_one_refresh_post` is
//! only meaningful if it can fail. It is not paired with a permanent
//! "bypass the lock" switch, because a bypass of a security control that ships
//! in the binary is a worse defect than the one it proves. The red control is a
//! SOURCE MUTANT applied out-of-tree — make `refresh_cross_process` take the
//! `Busy` arm unconditionally and this test observes two POSTs. That procedure
//! is recorded in the #172 evidence rather than wired in.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use base64::Engine as _;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use wcore_agent::oauth::{ChatGptTokenManager, OAuthFlow, OAuthStorage, OAuthTokens};
use wcore_config::credentials::PlaintextCredentialsStore;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

const ROLE_ENV: &str = "WL172_XPROC_ROLE";
const ROOT_ENV: &str = "WL172_XPROC_ROOT";
const URL_ENV: &str = "WL172_XPROC_TOKEN_URL";

fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock")
        .as_secs()
}

/// An already-expired pair, so `get()` must refresh rather than serve a cached
/// access token. Expiry in the PAST, not "soon": a near-expiry threshold would
/// make the test depend on how long the child took to start.
fn expired_pair() -> OAuthTokens {
    OAuthTokens {
        access_token: jwt_access_token(),
        refresh_token: Some("the-single-use-refresh-token".into()),
        expires_at_unix_secs: Some(now_secs().saturating_sub(3_600)),
        token_type: "Bearer".into(),
        scope: None,
        id_token: None,
    }
}

/// A 3-segment JWT carrying a ChatGPT account id. `get()` decodes the access
/// token to extract the account, so a plain opaque string fails with
/// "not a JWT" — a test-harness defect that reads exactly like a refresh
/// failure. Signatures are not verified; only claims are read.
fn jwt_access_token() -> String {
    let payload = serde_json::json!({
        "https://api.openai.com/auth": { "chatgpt_account_id": "acct-172" }
    });
    let seg = URL_SAFE_NO_PAD.encode(serde_json::to_vec(&payload).expect("payload"));
    format!("hdr.{seg}.sig")
}

fn storage_at(root: PathBuf) -> OAuthStorage {
    let secure = PlaintextCredentialsStore::new(root.join("credentials.toml"));
    OAuthStorage::at_root(root, Box::new(secure)).expect("oauth storage over a real on-disk store")
}

fn flow_to(token_url: &str) -> OAuthFlow {
    OAuthFlow::new(
        "test-client",
        None,
        "http://127.0.0.1/authorize",
        token_url,
        vec!["openid".to_string()],
    )
}

/// The child half. Runs in a SEPARATE OS PROCESS; the parent below spawns two.
///
/// Named as a test so the harness will run it, but it is inert unless the
/// parent set `ROLE_ENV` — otherwise every ordinary `cargo nextest` run would
/// try to reach a token endpoint that is not there.
#[tokio::test]
async fn xproc_child_entrypoint() {
    let Ok(role) = std::env::var(ROLE_ENV) else {
        // Not a child. Nothing to assert, and deliberately nothing skipped
        // silently either — the parent test is what carries the obligation.
        return;
    };
    assert_eq!(role, "child", "unexpected role");

    let root = PathBuf::from(std::env::var(ROOT_ENV).expect("root"));
    let token_url = std::env::var(URL_ENV).expect("token url");

    let mgr = Arc::new(ChatGptTokenManager::new_with_flow(
        storage_at(root),
        flow_to(&token_url),
    ));

    // The production path: notices the stored pair is expired and refreshes.
    match mgr.get().await {
        Ok((access, _account)) => {
            assert!(
                !access.is_empty(),
                "a refreshed access token must not be empty"
            );
            println!("XPROC_CHILD_OK {access}");
        }
        Err(error) => {
            // A LOSER that could not take the lock is still required to end up
            // with a working token by reloading the winner's pair. Failing here
            // is a real failure, not an acceptable outcome.
            panic!("child refresh failed: {error}");
        }
    }
}

/// P1 + P3. Two processes, one profile, one expired pair: exactly ONE POST
/// reaches the token endpoint, and BOTH processes end up authenticated.
///
/// P3 is the half that is easy to get wrong. An earlier draft of this proof
/// asserted only that the loser "succeeded", which passes just as well if the
/// loser quietly POSTed a second time and got a fresh pair back — the exact
/// grant-burning behaviour the lock exists to prevent. The assertion that
/// carries the obligation is the POST COUNT, not the success count.
#[tokio::test]
async fn p1_two_processes_issue_exactly_one_refresh_post() {
    if std::env::var(ROLE_ENV).is_ok() {
        return; // this process IS a child; the child test above does the work
    }

    let profile = tempfile::tempdir().expect("profile dir");
    let root = profile.path().to_path_buf();

    // Seed the shared, on-disk, expired pair.
    storage_at(root.clone())
        .store("chatgpt", &expired_pair())
        .expect("seed the expired pair both processes will read");

    // Local counting endpoint. Never the real provider.
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/token"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "access_token": jwt_access_token(),
            "refresh_token": "rotated-refresh",
            "expires_in": 3600,
            "token_type": "Bearer",
        })))
        .mount(&server)
        .await;
    let token_url = format!("{}/token", server.uri());

    let exe = std::env::current_exe().expect("test binary path");
    let mut children = Vec::new();
    for _ in 0..2 {
        let child = std::process::Command::new(&exe)
            .arg("xproc_child_entrypoint")
            .arg("--exact")
            .arg("--nocapture")
            .env(ROLE_ENV, "child")
            .env(ROOT_ENV, &root)
            .env(URL_ENV, &token_url)
            .spawn()
            .expect("spawn a second OS process");
        children.push(child);
    }

    for (index, mut child) in children.into_iter().enumerate() {
        let status = child.wait().expect("child exited");
        assert!(
            status.success(),
            "child {index} failed; a loser that cannot take the lock must still \
             end up authenticated by reloading the winner's pair"
        );
    }

    let posts = server
        .received_requests()
        .await
        .expect("the mock server records requests")
        .len();

    assert_eq!(
        posts, 1,
        "two processes sharing one profile must issue exactly ONE refresh POST; \
         {posts} means the single-use token was replayed, which a compliant \
         provider may answer by revoking the whole grant"
    );

    // The winner's rotated pair must be what is on disk, so the loser adopted
    // it rather than keeping the spent one.
    let stored = storage_at(root)
        .load("chatgpt")
        .expect("load")
        .expect("a pair must remain stored");
    assert_eq!(
        stored.refresh_token.as_deref(),
        Some("rotated-refresh"),
        "the rotated pair must be persisted; the spent token surviving here \
         means the next refresh starts from a token the provider already burned"
    );
}
