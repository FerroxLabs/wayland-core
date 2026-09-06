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
//! SOURCE MUTANT applied out-of-tree: remove the held lock (drop it before
//! `gated_refresh(entry, true)`) while preserving permission to POST. W07's
//! body barrier then observes logout completing before refresh persistence.
//! An unconditional `Busy` mutant forbids POST and cannot prove this race.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use base64::Engine as _;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use wcore_agent::oauth::{ChatGptTokenManager, OAuthFlow, OAuthStorage, OAuthTokens};
use wcore_config::credentials::{CredentialsStore, PlaintextCredentialsStore};
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

// W07 barriers use files only to coordinate processes. All credential reads,
// POSTs, provider locks and writes still run through the production paths.
const W07_ROLE: &str = "W07_OAUTH_ROLE";

struct PausedFirstRead {
    inner: PlaintextCredentialsStore,
    root: PathBuf,
    first: std::sync::atomic::AtomicBool,
}
impl CredentialsStore for PausedFirstRead {
    fn get(
        &self,
        key: &str,
    ) -> Result<Option<String>, wcore_config::credentials::CredentialsError> {
        let result = self.inner.get(key);
        if self.first.swap(false, std::sync::atomic::Ordering::SeqCst) {
            std::fs::write(self.root.join("read-ready"), "ready").unwrap();
            let deadline = std::time::Instant::now() + std::time::Duration::from_secs(20);
            while !self.root.join("read-resume").exists() {
                assert!(
                    std::time::Instant::now() < deadline,
                    "read barrier timed out"
                );
                std::thread::sleep(std::time::Duration::from_millis(10));
            }
        }
        result
    }
    fn put(
        &self,
        key: &str,
        value: &str,
    ) -> Result<(), wcore_config::credentials::CredentialsError> {
        self.inner.put(key, value)
    }
    fn delete(&self, key: &str) -> Result<(), wcore_config::credentials::CredentialsError> {
        self.inner.delete(key)
    }
}

#[tokio::test]
async fn w07_child_entrypoint() {
    let Ok(role) = std::env::var(W07_ROLE) else {
        return;
    };
    let root = PathBuf::from(std::env::var(ROOT_ENV).unwrap());
    let provider = std::env::var("W07_PROVIDER").unwrap();
    if role == "holder" {
        let _held = wcore_agent::oauth::refresh_lock::hold_for_writer(
            storage_at(root.clone()).refresh_lock_path(&provider),
        )
        .await
        .unwrap();
        std::fs::write(root.join("holder-ready"), "ready").unwrap();
        std::future::pending::<()>().await;
        return;
    }
    let storage = if role == "paused" {
        OAuthStorage::at_root(
            root.clone(),
            Box::new(PausedFirstRead {
                inner: PlaintextCredentialsStore::new(root.join("credentials.toml")),
                root,
                first: std::sync::atomic::AtomicBool::new(true),
            }),
        )
        .unwrap()
    } else {
        storage_at(root)
    };
    let flow = flow_to(&std::env::var(URL_ENV).unwrap());
    let result = if provider == "xai" {
        wcore_agent::oauth::xai::XaiTokenManager::new_with_flow(storage, flow)
            .get()
            .await
    } else {
        ChatGptTokenManager::new_with_flow(storage, flow)
            .get()
            .await
            .map(|pair| pair.0)
    };
    match std::env::var("W07_EXPECT").unwrap().as_str() {
        "absent" => assert!(result.unwrap_err().contains("not signed in")),
        "read-error" => assert!(result.unwrap_err().contains("re-read")),
        "timeout" => assert!(result.unwrap_err().contains("timed out")),
        _ => {
            result.expect("refresh or changed-authority adoption succeeds");
        }
    }
}

fn w07_child(
    root: &std::path::Path,
    provider: &str,
    url: &str,
    role: &str,
    expected: &str,
) -> tokio::process::Child {
    let exe = std::env::current_exe().unwrap();
    let mut command = wcore_config::shell::shell_command_argv(
        exe.to_str().unwrap(),
        &["w07_child_entrypoint", "--exact", "--nocapture"],
    );
    command
        .env(W07_ROLE, role)
        .env(ROOT_ENV, root)
        .env(URL_ENV, url)
        .env("W07_PROVIDER", provider)
        .env("W07_EXPECT", expected)
        .env("CODEX_HOME", root.join("empty-codex"))
        .env("GROK_HOME", root.join("empty-grok"))
        .kill_on_drop(true);
    command.spawn().unwrap()
}

async fn wait_file(path: &std::path::Path) {
    tokio::time::timeout(std::time::Duration::from_secs(20), async {
        while !path.exists() {
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("child reached the barrier");
}

async fn successful_child(mut child: tokio::process::Child) {
    assert!(
        tokio::time::timeout(std::time::Duration::from_secs(25), child.wait())
            .await
            .unwrap()
            .unwrap()
            .success()
    );
}

#[tokio::test]
async fn w07_logout_first_prevents_stale_refresh_and_fresh_process_post() {
    for provider in ["chatgpt", "xai"] {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        let storage = storage_at(root.to_path_buf());
        storage.store(provider, &expired_pair()).unwrap();
        let server = MockServer::start().await;
        let child = w07_child(
            root,
            provider,
            &format!("{}/token", server.uri()),
            "paused",
            "absent",
        );
        wait_file(&root.join("read-ready")).await;
        let writer =
            wcore_agent::oauth::refresh_lock::hold_for_writer(storage.refresh_lock_path(provider))
                .await
                .unwrap();
        storage.delete(provider).unwrap();
        std::fs::write(root.join("read-resume"), "resume").unwrap();
        drop(writer);
        successful_child(child).await;
        successful_child(w07_child(
            root,
            provider,
            &format!("{}/token", server.uri()),
            "fresh",
            "absent",
        ))
        .await;
        assert!(storage.load(provider).unwrap().is_none());
        assert!(server.received_requests().await.unwrap().is_empty());
    }
}

#[tokio::test]
async fn w07_authoritative_read_error_never_posts_cached_pair() {
    for provider in ["chatgpt", "xai"] {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        let storage = storage_at(root.to_path_buf());
        storage.store(provider, &expired_pair()).unwrap();
        let server = MockServer::start().await;
        let child = w07_child(
            root,
            provider,
            &format!("{}/token", server.uri()),
            "paused",
            "read-error",
        );
        wait_file(&root.join("read-ready")).await;
        std::fs::write(root.join("credentials.toml"), "[invalid").unwrap();
        std::fs::write(root.join("read-resume"), "resume").unwrap();
        successful_child(child).await;
        assert!(server.received_requests().await.unwrap().is_empty());
    }
}

#[tokio::test]
async fn w07_changed_authoritative_pair_is_adopted_without_post() {
    for provider in ["chatgpt", "xai"] {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        let storage = storage_at(root.to_path_buf());
        storage.store(provider, &expired_pair()).unwrap();
        let server = MockServer::start().await;
        let child = w07_child(
            root,
            provider,
            &format!("{}/token", server.uri()),
            "paused",
            "success",
        );
        wait_file(&root.join("read-ready")).await;
        let writer =
            wcore_agent::oauth::refresh_lock::hold_for_writer(storage.refresh_lock_path(provider))
                .await
                .unwrap();
        let mut changed = expired_pair();
        changed.refresh_token = Some("new-authoritative-pair".into());
        // Still expired intentionally: changed, not freshness, is the gate.
        storage.store(provider, &changed).unwrap();
        std::fs::write(root.join("read-resume"), "resume").unwrap();
        drop(writer);
        successful_child(child).await;
        assert!(server.received_requests().await.unwrap().is_empty());
    }
}

/// HTTP headers arrive before the response-body barrier. This proves the
/// provider lock covers body consumption as well as send() and persistence.
async fn held_endpoint() -> (
    String,
    tokio::sync::oneshot::Receiver<()>,
    tokio::sync::oneshot::Sender<()>,
    tokio::task::JoinHandle<()>,
) {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}/token", listener.local_addr().unwrap());
    let (ready_tx, ready_rx) = tokio::sync::oneshot::channel();
    let (resume_tx, resume_rx) = tokio::sync::oneshot::channel();
    let server = tokio::spawn(async move {
        let (mut socket, _) = listener.accept().await.unwrap();
        let mut request = Vec::new();
        loop {
            let mut byte = [0];
            socket.read_exact(&mut byte).await.unwrap();
            request.push(byte[0]);
            if request.ends_with(b"\r\n\r\n") {
                break;
            }
        }
        let headers = String::from_utf8(request).unwrap();
        let len: usize = headers
            .lines()
            .find_map(|line| {
                line.to_ascii_lowercase()
                    .strip_prefix("content-length:")
                    .map(|v| v.trim().parse().unwrap())
            })
            .unwrap();
        socket.read_exact(&mut vec![0; len]).await.unwrap();
        let body = serde_json::json!({"access_token": jwt_access_token(), "refresh_token": "rotated", "expires_in": 3600}).to_string();
        socket
            .write_all(
                format!(
                    "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                    body.len()
                )
                .as_bytes(),
            )
            .await
            .unwrap();
        ready_tx.send(()).unwrap();
        resume_rx.await.unwrap();
        socket.write_all(body.as_bytes()).await.unwrap();
    });
    (url, ready_rx, resume_tx, server)
}

#[tokio::test]
async fn w07_refresh_first_orders_logout_after_body_persist_and_allows_other_provider() {
    for provider in ["chatgpt", "xai"] {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        let storage = storage_at(root.to_path_buf());
        storage.store(provider, &expired_pair()).unwrap();
        let (url, ready, resume, server) = held_endpoint().await;
        let child = w07_child(root, provider, &url, "fresh", "success");
        tokio::time::timeout(std::time::Duration::from_secs(10), ready)
            .await
            .unwrap()
            .unwrap();
        let other = if provider == "chatgpt" {
            "xai"
        } else {
            "chatgpt"
        };
        let independent = tokio::time::timeout(
            std::time::Duration::from_secs(2),
            wcore_agent::oauth::refresh_lock::hold_for_writer(storage.refresh_lock_path(other)),
        )
        .await
        .unwrap()
        .unwrap();
        drop(independent);
        let logout = async {
            let _writer = wcore_agent::oauth::refresh_lock::hold_for_writer(
                storage.refresh_lock_path(provider),
            )
            .await
            .unwrap();
            storage.delete(provider).unwrap();
        };
        tokio::pin!(logout);
        assert!(
            tokio::time::timeout(std::time::Duration::from_millis(250), &mut logout)
                .await
                .is_err(),
            "logout must wait for the paused refresh"
        );
        resume.send(()).unwrap();
        logout.await;
        successful_child(child).await;
        server.await.unwrap();
        assert!(storage.load(provider).unwrap().is_none());
        successful_child(w07_child(root, provider, &url, "fresh", "absent")).await;
    }
}

#[tokio::test]
async fn w07_killed_writer_is_recovered_without_a_refresh_post() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    let storage = storage_at(root.to_path_buf());
    storage.store("chatgpt", &expired_pair()).unwrap();
    let mut child = w07_child(
        root,
        "chatgpt",
        "http://127.0.0.1:1/token",
        "holder",
        "success",
    );
    wait_file(&root.join("holder-ready")).await;
    child.kill().await.unwrap();
    child.wait().await.unwrap();
    let held = tokio::time::timeout(
        std::time::Duration::from_secs(20),
        wcore_agent::oauth::refresh_lock::hold_for_writer(storage.refresh_lock_path("chatgpt")),
    )
    .await
    .unwrap()
    .unwrap();
    storage.delete("chatgpt").unwrap();
    drop(held);
    assert!(storage.load("chatgpt").unwrap().is_none());
}

#[tokio::test]
async fn w07_response_body_timeout_releases_provider_writer_lock() {
    for provider in ["chatgpt", "xai"] {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        let storage = storage_at(root.to_path_buf());
        storage.store(provider, &expired_pair()).unwrap();
        let (url, ready, _resume, server) = held_endpoint().await;
        let child = w07_child(root, provider, &url, "fresh", "timeout");
        tokio::time::timeout(std::time::Duration::from_secs(10), ready)
            .await
            .unwrap()
            .unwrap();
        successful_child(child).await;
        server.abort();
        let _writer = tokio::time::timeout(
            std::time::Duration::from_secs(2),
            wcore_agent::oauth::refresh_lock::hold_for_writer(storage.refresh_lock_path(provider)),
        )
        .await
        .unwrap()
        .unwrap();
        assert_eq!(
            storage.load(provider).unwrap().unwrap().refresh_token,
            expired_pair().refresh_token
        );
    }
}
