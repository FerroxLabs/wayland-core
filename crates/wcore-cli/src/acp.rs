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
use std::path::PathBuf;
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

    /// Serve as an ISOLATED PROFILE — the profile's own home (credentials,
    /// `.env`, memory, skills, SOUL), and its provider/model from that home's
    /// own `config.toml`.
    ///
    /// This is the supervisor/router spawn primitive: one `acp serve --profile
    /// <name>` child process PER profile, each with its own `WAYLAND_HOME`, is
    /// how per-profile identity stays isolated. N profiles are NEVER served
    /// from one process — that would share this process's global
    /// `WAYLAND_HOME` / credential / egress singletons across identities.
    ///
    /// The home is materialized at process entry by
    /// `profile::activate_for_launch()` (which scans raw argv, so it sees this
    /// flag) and becomes `WAYLAND_HOME`. `Config::resolve` then reads that
    /// home's own `config.toml` for provider/model — so nothing is routed into
    /// the config-file `[profiles.<name>]` OVERLAY, which is a distinct
    /// mechanism whose missing-table case hard-errors.
    ///
    /// FAILS CLOSED: if the profile is unknown, has no home on disk, or
    /// disagrees with an explicitly-set `WAYLAND_HOME`, the server refuses to
    /// start rather than silently falling through to the SHARED DEFAULT home
    /// and cross-writing another profile's credentials and memory.
    ///
    /// `--profile` also works in the GLOBAL position (`wayland-core --profile X
    /// acp serve`); both positions are validated. This field exists so clap
    /// accepts and documents the subcommand form — the guard itself reads the
    /// name from raw argv (see `serve`), which is the single source of truth
    /// `activate_for_launch` also uses, so the two cannot disagree.
    #[arg(long, value_name = "NAME")]
    pub profile: Option<String>,
}

/// Resolve `--profile` to the isolated home the process is ACTUALLY bound to,
/// refusing every case where the two could disagree.
///
/// `profile::activate_for_launch()` already ran at process entry and either set
/// `WAYLAND_HOME` to the profile's home, or — when the name is invalid or the
/// home does not exist — WARNED and fell through to the shared default home.
/// That fall-through is tolerable for interactive CLI use but NOT for a host
/// protocol: an ACP server told to serve profile `work` while actually writing
/// the default home would cross-write another identity's credentials and
/// memory. This is the same contract `json_stream_profile_guard` (main.rs)
/// enforces for `--json-stream`; ACP is the same class of surface.
///
/// Returns the resolved home on success, or a loud error string to refuse with.
fn resolve_profile_home(name: &str) -> Result<PathBuf, String> {
    let dir = wcore_config::profile::profile_dir(name)
        .map_err(|e| format!("refusing to serve ACP with an invalid --profile {name:?}: {e}"))?;

    if !dir.is_dir() {
        return Err(format!(
            "refusing to serve ACP with --profile {name:?}: no profile home at {}. \
             Without it the server would fall through to the SHARED DEFAULT home and \
             cross-write another profile's credentials and memory. Create it with \
             `wayland-core profile create {name}`.",
            dir.display()
        ));
    }

    // `WAYLAND_HOME` is the single source of truth after activation, and an
    // explicit one always WINS over `--profile` (profile.rs resolution order).
    // So if it points somewhere else, the flag is a lie about the identity this
    // process actually serves — refuse rather than serve the wrong home.
    if let Some(home) = std::env::var_os("WAYLAND_HOME") {
        let home = PathBuf::from(home);
        let same = match (home.canonicalize(), dir.canonicalize()) {
            (Ok(a), Ok(b)) => a == b,
            _ => home == dir,
        };
        if !same {
            return Err(format!(
                "refusing to serve ACP: --profile {name:?} resolves to {} but WAYLAND_HOME is {}. \
                 An explicit WAYLAND_HOME wins over --profile, so this server would serve one \
                 profile's identity under another's name. Unset one of them.",
                dir.display(),
                home.display()
            ));
        }
    }

    Ok(dir)
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
    // Bind the isolated profile FIRST — before the bind address is parsed, before
    // the server's API key is loaded-or-GENERATED into the keychain, before the
    // config is resolved, and before the egress policy is installed. A refused
    // profile must have ZERO side effects: if this ran later, `--profile ghost`
    // (a name with no home) would fall through to the SHARED DEFAULT home, mint
    // and persist a fresh server key there, and only then refuse. Fails closed —
    // see `resolve_profile_home`.
    //
    // We read the requested profile from RAW ARGV (`requested_profile_from_argv`),
    // NOT `args.profile`. `--profile` is accepted in two clap positions — global
    // (`wayland-core --profile X acp serve`) and subcommand (`... acp serve
    // --profile X`) — and `activate_for_launch` (which binds WAYLAND_HOME at
    // process entry) honors BOTH via the same argv scan. Reading only the
    // subcommand field would leave the global position unguarded: activation
    // would fall through to the default home while this guard saw `None` and
    // waved it past — serving one identity while writing another's. One argv
    // source keeps the guard and activation in lockstep by construction.
    if let Some(name) = wcore_config::profile::requested_profile_from_argv() {
        let home = resolve_profile_home(&name).map_err(|e| anyhow::anyhow!("{e}"))?;
        eprintln!(
            "wayland-core acp: serving isolated profile {name:?} (home {})",
            home.display()
        );
    } else if let Some(unbound) =
        wcore_config::profile::launch_outcome().and_then(|o| o.unbound_selection)
    {
        // No `--profile` flag, but activation selected a profile via the `active`
        // pointer (`wayland-core profile use <name>`) whose home is absent — so it
        // fell through to the SHARED DEFAULT home. The flag guard above can't see
        // this (there is no flag); reason on activation's RESULT instead. Fail
        // closed: serving now would expose the default identity's credentials and
        // memory under the selected profile's name. Same class as the flag path.
        return Err(anyhow::anyhow!(
            "refusing to serve ACP: the active profile {unbound:?} was selected but has no home \
             on disk, so this process fell through to the SHARED DEFAULT home. Serving would \
             expose the default identity's credentials and memory under {unbound:?}. Recreate it \
             (`wayland-core profile create {unbound}`) or clear the selection \
             (`wayland-core profile use default`)."
        ));
    }

    let addr: SocketAddr = args
        .bind
        .parse()
        .map_err(|e| anyhow::anyhow!("invalid bind address {:?}: {e}", args.bind))?;

    // F-017: generate or load a one-time API key for this server instance.
    // We try to load an existing key from the keychain first; if absent
    // (first run), we generate a new one, persist it, and print it to stderr
    // exactly once. The key is 32 random bytes, hex-encoded → 64 chars.
    // persona-profiles PR-7: when the profile SUPERVISOR spawns this process as
    // a per-profile CHILD, it injects a pre-generated server key via
    // WAYLAND_ACP_SERVER_KEY so the parent's AcpClient can authenticate to us
    // (X-API-Key) WITHOUT scraping our stderr for a keychain-generated key. The
    // env var is readable only by this child process (and root); children bind
    // localhost ephemeral ports. Takes precedence over the keychain path below.
    let api_key = match std::env::var("WAYLAND_ACP_SERVER_KEY") {
        Ok(k) if !k.is_empty() => k,
        _ => match wcore_config::keychain::get_secret(
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
        },
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
        // NOT the profile name. `CliArgs.profile` selects a config-file
        // `[profiles.<name>]` OVERLAY table — a DIFFERENT mechanism from an
        // isolated-home profile, and a missing table is a hard error. An
        // isolated profile carries its provider/model/keys through its OWN
        // `config.toml`, which `Config::resolve` already reads because
        // `wayland_config_dir()` honors the `WAYLAND_HOME` that
        // `activate_for_launch` set from `--profile`. So the overlay is both
        // redundant here and a footgun (it would abort every profile that
        // hasn't had a `[profiles.<name>]` table hand-authored).
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
    use serial_test::serial;
    use std::time::Duration;
    use tokio::net::TcpListener;

    /// Point `profiles_root()` at a scratch dir and clear any inherited
    /// `WAYLAND_HOME`. Mutates process-global env, hence `#[serial]` on every
    /// test that calls it.
    fn with_profiles_root(dir: &std::path::Path) {
        // SAFETY: these tests are `#[serial]`, so no other test thread is
        // observing the environment while we mutate it.
        unsafe {
            std::env::set_var("WAYLAND_PROFILES_ROOT", dir);
            std::env::remove_var("WAYLAND_HOME");
        }
    }

    fn clear_profile_env() {
        // SAFETY: as above — serialized.
        unsafe {
            std::env::remove_var("WAYLAND_PROFILES_ROOT");
            std::env::remove_var("WAYLAND_HOME");
        }
    }

    /// The happy path: a profile whose home exists resolves to that home.
    #[test]
    #[serial]
    fn existing_profile_resolves_to_its_own_home() {
        let tmp = tempfile::tempdir().unwrap();
        with_profiles_root(tmp.path());
        std::fs::create_dir_all(tmp.path().join("work")).unwrap();

        let home = resolve_profile_home("work").expect("an existing profile resolves");
        assert_eq!(home, tmp.path().join("work"));
        clear_profile_env();
    }

    /// THE CORE GUARD: a profile with no home on disk must REFUSE, not fall
    /// through to the shared default home. Falling through is what
    /// cross-writes another identity's credentials and memory.
    #[test]
    #[serial]
    fn missing_profile_home_refuses_instead_of_using_the_default_home() {
        let tmp = tempfile::tempdir().unwrap();
        with_profiles_root(tmp.path());

        let err = resolve_profile_home("ghost").expect_err("a missing home must refuse");
        assert!(
            err.contains("no profile home"),
            "the refusal must name the cause; got: {err}"
        );
        clear_profile_env();
    }

    /// An explicit `WAYLAND_HOME` WINS over `--profile` (profile.rs resolution
    /// order), so a disagreement means the server would serve one identity
    /// under another's name. Refuse.
    #[test]
    #[serial]
    fn wayland_home_disagreeing_with_the_profile_refuses() {
        let tmp = tempfile::tempdir().unwrap();
        with_profiles_root(tmp.path());
        std::fs::create_dir_all(tmp.path().join("work")).unwrap();
        let other = tmp.path().join("somewhere-else");
        std::fs::create_dir_all(&other).unwrap();
        // SAFETY: serialized.
        unsafe { std::env::set_var("WAYLAND_HOME", &other) };

        let err = resolve_profile_home("work").expect_err("a mismatched home must refuse");
        assert!(
            err.contains("WAYLAND_HOME"),
            "the refusal must name the conflict; got: {err}"
        );
        clear_profile_env();
    }

    /// The agreeing case is NOT a conflict: activation sets `WAYLAND_HOME` to
    /// exactly the profile's home, which is the normal supervisor spawn.
    #[test]
    #[serial]
    fn wayland_home_agreeing_with_the_profile_is_accepted() {
        let tmp = tempfile::tempdir().unwrap();
        with_profiles_root(tmp.path());
        let home = tmp.path().join("work");
        std::fs::create_dir_all(&home).unwrap();
        // SAFETY: serialized.
        unsafe { std::env::set_var("WAYLAND_HOME", &home) };

        let got = resolve_profile_home("work").expect("an agreeing home is the normal spawn");
        assert_eq!(got, home);
        clear_profile_env();
    }

    /// A hostile name must never be joined onto the profiles root.
    #[test]
    #[serial]
    fn traversal_name_is_rejected_by_validation() {
        let tmp = tempfile::tempdir().unwrap();
        with_profiles_root(tmp.path());

        let err = resolve_profile_home("../../etc").expect_err("traversal must be rejected");
        assert!(
            err.contains("invalid --profile"),
            "the refusal must name it invalid; got: {err}"
        );
        clear_profile_env();
    }

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
