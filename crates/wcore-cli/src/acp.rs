//! `wayland-core acp` — production caller for the ACP crate. Two
//! modes: `serve` binds the HTTP/SSE transport to a TCP port; `request`
//! is a one-shot client that exercises the same endpoints from the
//! command line (handy for smoke-testing a running server).
//!
//! v0.7.0 1.A.10: this is the methodology #27 "production caller"
//! required for every new public symbol that landed in 1.A.1 → 1.A.8.
//! Without this binding, `AcpServer` + `HttpSseTransport` + `AcpClient`
//! would all be orphans.

use std::net::SocketAddr;
use std::sync::Arc;

use clap::{Args, Subcommand};
use futures::StreamExt;
use wcore_acp::auth::store_api_key;
use wcore_acp::client::AcpClient;
use wcore_acp::protocol::{MessageEvent, MessageSendRequest, SessionCreateRequest};
use wcore_acp::server::AcpServer;
use wcore_acp::transport::HttpSseTransport;

/// Top-level args for `wayland-core acp ...`.
#[derive(Args, Debug)]
pub struct AcpArgs {
    #[command(subcommand)]
    pub command: AcpCmd,
}

/// ACP sub-subcommands.
#[derive(Subcommand, Debug)]
pub enum AcpCmd {
    /// Bind the HTTP/SSE transport to a TCP port and serve sessions.
    Serve(AcpServeArgs),
    /// One-shot ACP request against a running server. Mostly a
    /// smoke-test handle for `serve`.
    Request(AcpRequestArgs),
}

#[derive(Args, Debug)]
pub struct AcpServeArgs {
    /// Bind address (default `127.0.0.1:8080`).
    #[arg(long, default_value = "127.0.0.1:8080")]
    pub bind: String,

    /// Provider slug to resolve the engine with (e.g. `anthropic`,
    /// `openai`). Omit to use the config-file / env default.
    #[arg(long)]
    pub provider: Option<String>,

    /// Model override for the engine. Omit to use the provider default.
    #[arg(long)]
    pub model: Option<String>,

    /// API key override. Omit to use the keychain / env / config-file key.
    #[arg(long)]
    pub api_key: Option<String>,

    /// Base URL override for the LLM provider (e.g.
    /// `https://integrate.api.nvidia.com/v1`). Omit to use the
    /// `[providers.<name>] base_url` from the config file, then the catalog
    /// default. Required for OpenAI-compatible providers (NVIDIA, local
    /// servers, etc.) whose key must NOT be presented to `api.openai.com`.
    #[arg(long)]
    pub base_url: Option<String>,

    /// Auto-approve EVERY tool call (shell, file writes, sub-agents) for API
    /// sessions. Off by default. Turning this on makes the API key
    /// root-equivalent: anyone who can reach the server and present the key
    /// can run arbitrary tools. Only enable on a trusted, access-controlled
    /// bind. Without it, approval-required tools do not auto-execute.
    #[arg(long)]
    pub allow_all_tools: bool,

    /// persona-profiles Phase A — expose the TRUSTED persona-agent roster over
    /// ACP (`agents/list` + the `session/create.agent` selector). OFF by
    /// default: without this flag no roster is installed, so `agents/list`
    /// returns an empty catalog and any `agent` selector resolves to
    /// `AgentNotFound` — byte-identical to the pre-persona server.
    ///
    /// Enumerates ONLY compiled-in `AgentPack` personas and the operator's own
    /// global agent YAML (`wayland_config_dir()/agents`). It never enumerates
    /// project-supplied manifests (untrusted repo content) and never isolated
    /// profiles (a profile is a credential boundary — per-profile isolation is
    /// the supervisor/router topology, one process per profile, not this
    /// in-process roster). Personas share this process's single identity: they
    /// overlay prompt/model/tools only, never credentials.
    #[arg(long)]
    pub enable_agent_selection: bool,
}

#[derive(Args, Debug)]
pub struct AcpRequestArgs {
    /// Base URL of the server (no trailing slash, e.g.
    /// `http://127.0.0.1:8080`).
    #[arg(long, default_value = "http://127.0.0.1:8080")]
    pub base_url: String,

    #[command(subcommand)]
    pub op: AcpRequestOp,
}

#[derive(Subcommand, Debug)]
pub enum AcpRequestOp {
    /// `session/create`. Prints the new session_id.
    CreateSession {
        #[arg(long)]
        model: Option<String>,
    },
    /// `session/list`. Prints one session_id per line.
    ListSessions,
    /// `session/get :id`. Prints session metadata.
    GetSession { session_id: String },
    /// `session/delete :id`.
    DeleteSession { session_id: String },
    /// `message/send :id <text>`. Streams events to stdout, one per
    /// line, JSON-encoded. Exits when the stream ends.
    Send { session_id: String, text: String },
}

/// Dispatch the parsed `acp` subcommand.
pub async fn run(args: AcpArgs) -> anyhow::Result<()> {
    match args.command {
        AcpCmd::Serve(a) => serve(a).await,
        AcpCmd::Request(a) => request(a).await,
    }
}

/// Keychain account name for the ACP server API key. (F-017)
const ACP_SERVER_KEY_ACCOUNT: &str = "acp-server-key";

async fn serve(args: AcpServeArgs) -> anyhow::Result<()> {
    let addr: SocketAddr = args
        .bind
        .parse()
        .map_err(|e| anyhow::anyhow!("invalid bind address {:?}: {e}", args.bind))?;

    // F-017: generate or load a one-time API key for this server instance.
    // We try to load an existing key from the keychain first; if absent
    // (first run), we generate a new one, persist it, and print it to stderr
    // exactly once. The key is 32 random bytes, hex-encoded → 64 chars.
    let api_key = match wcore_config::keychain::get_secret(
        wcore_acp::auth::KEYCHAIN_SERVICE,
        ACP_SERVER_KEY_ACCOUNT,
    ) {
        Ok(k) if !k.is_empty() => k,
        _ => {
            // First run: generate a fresh key.
            let random_bytes: [u8; 32] = {
                let mut buf = [0u8; 32];
                // Use uuid's rng (already in the workspace) for portability.
                let id = uuid::Uuid::new_v4();
                let id2 = uuid::Uuid::new_v4();
                buf[..16].copy_from_slice(id.as_bytes());
                buf[16..].copy_from_slice(id2.as_bytes());
                buf
            };
            let key: String = random_bytes.iter().map(|b| format!("{:02x}", b)).collect();
            store_api_key(ACP_SERVER_KEY_ACCOUNT, &key)
                .map_err(|e| anyhow::anyhow!("keychain store failed: {e}"))?;
            eprintln!(
                "wayland-core acp: generated API key (first run) — \
                 pass as X-API-Key header:\n  {key}"
            );
            key
        }
    };

    // Resolve a runtime Config for the engine. Provider/model/api-key flags
    // override; everything else falls back to the config-file + env cascade.
    // `project_dir` defaults to the cwd the server is launched in.
    let cwd = std::env::current_dir()
        .map_err(|e| anyhow::anyhow!("resolve cwd: {e}"))?
        .to_string_lossy()
        .to_string();
    let config = wcore_config::config::Config::resolve(&wcore_config::config::CliArgs {
        provider: args.provider.clone(),
        api_key: args.api_key.clone(),
        base_url: args.base_url.clone(),
        model: args.model.clone(),
        max_tokens: None,
        max_turns: None,
        system_prompt: None,
        profile: None,
        auto_approve: false,
        project_dir: None,
    })
    .map_err(|e| anyhow::anyhow!("config resolve: {e}"))?;

    // SECURITY (Wave 6 #11): install the process-global egress policy now that
    // `config` is fully resolved, BEFORE the engine is built or serves any turn.
    // `acp serve` early-returns from main's dispatch long before main.rs's own
    // `install_egress_policy` chokepoint, so without this call the ACP-hosted
    // engine runs with NO egress enforcement (the global policy falls back to
    // AllowAll, letting providers/tools/sub-agents reach private/internal hosts
    // and exfiltrate to non-allowlisted destinations even under the default
    // `[security] enabled = true` enforcing posture). The install is one-shot
    // and idempotent (first call wins), matching the pattern in
    // `workflow.rs` — re-installs from sub-agent boots are no-ops. We use the
    // SAME resolved config the engine binds below; no separate re-resolve.
    wcore_agent::egress::install_egress_policy(&config);

    // Engine-backed wiring: the ACP `message/send` path streams real engine
    // output via `EngineTurnEngine`, and the A2A federation surface
    // (handshake / message/send / capabilities) routes through the SAME
    // engine bridge via `EngineA2aHandler` (replacing the echo stub). Both
    // share one resolved Config so they bind the same provider/model.
    let agent_id = hostname_or("wayland-core");
    if args.allow_all_tools {
        eprintln!(
            "WARNING: --allow-all-tools is set. The API key is now root-equivalent: \
             any caller presenting it can run shell, file-write, and sub-agent tools \
             without approval. Ensure the bind ({}) is trusted and access-controlled.",
            args.bind
        );
    }
    // persona-profiles Phase A: build the trusted persona-agent roster ONLY when
    // the operator opts in. Feature default-OFF — with no roster the server keeps
    // its pre-persona behaviour exactly (empty `agents/list`, `AgentNotFound` for
    // any selector, and no persona is resolvable by the engine).
    //
    // ONE roster instance is shared by the server (which AUTHORIZES a selector at
    // session/create) and the turn engine (which RESOLVES that id to the persona
    // overlay). Sharing the instance is the invariant that makes "what you may
    // select" and "what may be applied to your engine" the same set — two rosters
    // could drift and become an authz bypass.
    let roster = if args.enable_agent_selection {
        let r = crate::acp_roster::CliAgentRoster::from_trusted_sources();
        eprintln!(
            "wayland-core acp: persona-agent selection ENABLED — {} trusted agent(s) \
             selectable (AgentPack + global operator YAML; project manifests and \
             isolated profiles are never enumerated).",
            r.len()
        );
        Some(Arc::new(r))
    } else {
        None
    };

    let mut engine = crate::acp_engine::EngineTurnEngine::new(config.clone(), cwd.clone())
        .force_tools(args.allow_all_tools);
    if let Some(r) = &roster {
        engine = engine.with_roster(Arc::clone(r));
    }
    let turn_engine = Arc::new(engine);

    let a2a = Arc::new(crate::acp_engine::EngineA2aHandler::new(
        agent_id, config, cwd,
    ));
    let mut acp_server = AcpServer::new()
        .with_a2a_handler(a2a)
        .with_turn_engine(turn_engine);
    if let Some(r) = roster {
        acp_server = acp_server.with_roster(r);
    }
    let server = Arc::new(acp_server);

    // F-017: wire the ApiKeyVerifier so every request must carry
    // X-API-Key: <api_key>. CorsLayer::permissive() is gone — no
    // wildcard Access-Control-Allow-Origin on this server.
    //
    // Note: store_api_key already persisted the key above; ApiKeyVerifier
    // reads from the keychain at verify-time, so if the keychain read
    // fails at startup it falls back to in-memory comparison via a
    // SimpleKeyVerifier below.
    let verifier = Arc::new(SimpleKeyVerifier { key: api_key });

    // Mount BOTH transports on one listener over the SAME `Arc<AcpServer>`:
    // the ACP HTTP/SSE surface keeps its unversioned `/sessions`, and the
    // REST/OpenAPI surface owns `/v1/*` + `/openapi.json` + `/doc`. There is
    // no second engine binding — REST inherits the engine via the shared
    // `HttpHandler`. The verifier is `Arc::clone`d into both so every
    // authenticated route on either surface is gated by the same key, with
    // `/openapi.json` + `/doc` left as the documented public carve-out.
    let acp_router = HttpSseTransport::new(Arc::clone(&server))
        .with_verifier(Arc::clone(&verifier) as Arc<dyn wcore_acp::auth::Verifier>)
        .router();
    let rest_router = wcore_acp::transport::RestTransport::new(Arc::clone(&server))
        .with_verifier(Arc::clone(&verifier) as Arc<dyn wcore_acp::auth::Verifier>)
        .router();
    let app = acp_router.merge(rest_router);

    let listener = tokio::net::TcpListener::bind(addr).await?;
    let local = listener.local_addr()?;
    eprintln!(
        "wayland-core acp: serving on http://{local} \
         (ACP on /sessions, REST on /v1, docs at /doc)"
    );
    axum::serve(listener, app).await?;
    Ok(())
}

/// Lightweight in-process constant-time API key verifier used by `serve`.
///
/// Differs from [`ApiKeyVerifier`] in that it holds the key in memory
/// (already generated/loaded above) rather than re-hitting the keychain
/// on each request — avoids keychain I/O on every HTTP call. (F-017)
struct SimpleKeyVerifier {
    key: String,
}

impl wcore_acp::auth::Verifier for SimpleKeyVerifier {
    fn verify(
        &self,
        headers: &[(String, String)],
    ) -> Result<wcore_acp::auth::Principal, wcore_acp::AcpError> {
        let presented = headers
            .iter()
            .find_map(|(k, v)| {
                if k.eq_ignore_ascii_case("x-api-key") {
                    Some(v.as_str())
                } else if k.eq_ignore_ascii_case("authorization") {
                    v.strip_prefix("ApiKey ")
                        .or_else(|| v.strip_prefix("apikey "))
                } else {
                    None
                }
            })
            .ok_or_else(|| wcore_acp::AcpError::Auth("missing X-API-Key header".to_string()))?;

        // Constant-time comparison.
        let a = presented.as_bytes();
        let b = self.key.as_bytes();
        if a.len() != b.len() {
            return Err(wcore_acp::AcpError::Auth("api key mismatch".to_string()));
        }
        let mut diff: u8 = 0;
        for (x, y) in a.iter().zip(b.iter()) {
            diff |= x ^ y;
        }
        if diff == 0 {
            Ok(wcore_acp::auth::Principal {
                id: ACP_SERVER_KEY_ACCOUNT.to_string(),
                scheme: wcore_acp::auth::AuthSchemeKind::ApiKey,
            })
        } else {
            Err(wcore_acp::AcpError::Auth("api key mismatch".to_string()))
        }
    }
}

/// Best-effort agent-id source — hostname when available, otherwise the
/// supplied fallback. v0.8.1 U12.
fn hostname_or(fallback: &str) -> String {
    std::env::var("HOSTNAME")
        .ok()
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| fallback.to_string())
}

async fn request(args: AcpRequestArgs) -> anyhow::Result<()> {
    let client =
        AcpClient::new(&args.base_url).map_err(|e| anyhow::anyhow!("build client: {e}"))?;
    match args.op {
        AcpRequestOp::CreateSession { model } => {
            let resp = client
                .create_session(SessionCreateRequest {
                    model,
                    tools: Vec::new(),
                    system_prompt: None,
                    agent: None,
                })
                .await
                .map_err(|e| anyhow::anyhow!("create_session: {e}"))?;
            println!("{}", resp.session_id);
        }
        AcpRequestOp::ListSessions => {
            let resp = client
                .list_sessions()
                .await
                .map_err(|e| anyhow::anyhow!("list_sessions: {e}"))?;
            for s in &resp.sessions {
                println!("{}", s.session_id);
            }
        }
        AcpRequestOp::GetSession { session_id } => {
            let resp = client
                .get_session(&session_id)
                .await
                .map_err(|e| anyhow::anyhow!("get_session: {e}"))?;
            // Pretty-print the metadata as JSON for stable parsing.
            println!("{}", serde_json::to_string_pretty(&resp.session)?);
        }
        AcpRequestOp::DeleteSession { session_id } => {
            client
                .delete_session(&session_id)
                .await
                .map_err(|e| anyhow::anyhow!("delete_session: {e}"))?;
            eprintln!("deleted {session_id}");
        }
        AcpRequestOp::Send { session_id, text } => {
            let mut stream = client
                .send_message(MessageSendRequest {
                    session_id,
                    text,
                    tools: Vec::new(),
                })
                .await
                .map_err(|e| anyhow::anyhow!("send_message: {e}"))?;
            while let Some(ev) = stream.next().await {
                let ev = ev.map_err(|e| anyhow::anyhow!("stream: {e}"))?;
                println!("{}", serde_json::to_string(&ev)?);
                if matches!(ev, MessageEvent::Done { .. } | MessageEvent::Error { .. }) {
                    break;
                }
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;
    use tokio::net::TcpListener;

    /// End-to-end smoke: `serve` on an ephemeral port, then drive
    /// each `Request` op against it, asserting basic shape.
    #[tokio::test]
    async fn e2e_create_list_get_send_delete() {
        // Spin up a server in-process on an ephemeral port. We don't
        // use the `serve` helper directly because it takes a `String`
        // and blocks on `axum::serve`; we replicate its wiring here.
        let server = Arc::new(AcpServer::new());
        let transport = HttpSseTransport::new(Arc::clone(&server));
        let app = transport.router();
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let _serve_handle = tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        // Give the listener a moment to start accepting.
        tokio::time::sleep(Duration::from_millis(50)).await;

        let base = format!("http://{addr}");

        // create_session
        let create_args = AcpRequestArgs {
            base_url: base.clone(),
            op: AcpRequestOp::CreateSession {
                model: Some("opus".to_string()),
            },
        };
        request(create_args).await.unwrap();

        // The session_id is on stdout — we don't capture it here, but
        // list_sessions verifies the create landed.
        let client = AcpClient::new(&base).unwrap();
        let listed = client.list_sessions().await.unwrap();
        assert_eq!(listed.sessions.len(), 1);
        let id = listed.sessions[0].session_id.clone();

        // get_session via the dispatcher
        let get_args = AcpRequestArgs {
            base_url: base.clone(),
            op: AcpRequestOp::GetSession {
                session_id: id.clone(),
            },
        };
        request(get_args).await.unwrap();

        // send (streams a Done event)
        let send_args = AcpRequestArgs {
            base_url: base.clone(),
            op: AcpRequestOp::Send {
                session_id: id.clone(),
                text: "hi".to_string(),
            },
        };
        request(send_args).await.unwrap();

        // delete
        let delete_args = AcpRequestArgs {
            base_url: base,
            op: AcpRequestOp::DeleteSession { session_id: id },
        };
        request(delete_args).await.unwrap();
        let listed_after = client.list_sessions().await.unwrap();
        assert!(listed_after.sessions.is_empty());
    }
}
