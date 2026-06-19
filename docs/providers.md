# Providers & Authentication

## Supported Providers

Pass any of these to `--provider` (or set `provider` in config). Aliases resolve
to the same built-in. The canonical list lives in `BUILTIN_PROVIDER_NAMES`
(`crates/wcore-config/src/config.rs`).

| Provider | Slug (aliases) | Notes |
|----------|----------------|-------|
| Anthropic | `anthropic` | Native wire — prompt caching, streaming, vision |
| OpenAI | `openai` | Chat-completions wire; base for most OpenAI-compatible providers |
| AWS Bedrock | `bedrock` | Hosts Claude; SigV4 + AWS credential chain |
| Google Vertex AI | `vertex` | Hosts Claude; GCP OAuth2 / service account |
| Google Gemini | `gemini` (`google`) | Native Gemini wire (functionDeclarations, thoughtSignature) |
| Azure OpenAI | `azure-openai` (`azure`) | Azure-hosted OpenAI deployments |
| Together | `together` | OpenAI-compatible |
| Fireworks | `fireworks` | OpenAI-compatible |
| NVIDIA | `nvidia` | OpenAI-compatible (NIM) |
| Perplexity | `perplexity` | OpenAI-compatible; `sonar` online-search models. Env `PERPLEXITY_API_KEY` |
| Cerebras | `cerebras` | OpenAI-compatible, fast inference |
| OpenRouter | `openrouter` | OpenAI-compatible router (100+ models) |
| Flux Router | `flux-router` (`flux`) | OpenAI-compatible router |
| DeepSeek | `deepseek` | OpenAI-compatible |
| xAI / Grok | `xai` (`grok`) | OpenAI-compatible; OAuth or `XAI_API_KEY` — see [Sign in with Grok](#sign-in-with-grok-xai) |
| Groq | `groq` | OpenAI-compatible, LPU inference |
| Moonshot / Kimi | `moonshot` (`kimi`) | OpenAI-compatible, region-locked keys |
| Qwen | `qwen` (`alibaba`, `dashscope`) | DashScope OpenAI-compat mode |
| Mistral | `mistral` | OpenAI-compatible |
| Cohere | `cohere` | OpenAI-compatible |
| OpenAI (ChatGPT) | `openai-chatgpt` (`chatgpt`) | OAuth — routes through the ChatGPT Codex backend on your subscription. See [Sign in with ChatGPT](#sign-in-with-chatgpt) |
| MiniMax | `minimax` (`minimaxi`) | Anthropic-wire provider; region-locked keys |

---

## Host integration: pick the right `--provider`

An embedding app must spawn each provider under its **own** `--provider`, not
under `--provider openai`. The `ProviderType` is what keys OAuth refresh, the
`grok-4.3` stop-param suppression, and the correct `base_url`:

- **Grok** → `--provider xai`. Spawned as `openai` it ignores the xAI OAuth
  token files, sends the unsupported `stop` parameter, and hits
  `api.openai.com` (401).
- **Perplexity** → `--provider perplexity`. Spawned as `openai` it targets
  `api.openai.com` instead of `api.perplexity.ai` and 401s.

The same holds for every entry in the table above: the slug selects the wire,
base URL, and compat preset.

---

## Custom Provider Alias

If your backend is compatible with a built-in provider's protocol, you can define a custom alias for it instead of setting `provider` directly to a built-in name.

```toml
[default]
provider = "my-service"

[providers.my-service]
provider = "openai"
model = "custom-model-v1"
api_key = "sk-xxx"
base_url = "https://my-service.example.com/api/openai"
```

Rules:

- `provider = "my-service"` is a config-layer alias
- `[providers.my-service].provider` must point at an underlying built-in provider
- The underlying provider must be one of the built-in provider slugs listed under [Supported Providers](#supported-providers)
- The alias entry's `model`, `api_key`, `base_url`, and `compat` override the underlying provider's defaults

This fits scenarios like DeepSeek gateways and internal OpenAI-compatible services.

### Generic / self-hosted OpenAI-compatible endpoints (vLLM, llama.cpp, LM Studio)

Point `base_url` at the server's API root **without** a trailing `/v1` — the engine
appends `/v1/chat/completions` itself (so `http://127.0.0.1:8003`, not
`http://127.0.0.1:8003/v1`).

Some self-hosted servers reject the `stream_options: {include_usage: true}` field the
engine sends by default (to collect token-usage accounting) with an HTTP 400, or simply
stream nothing — which can present as a chat that produces **no response and no error**.
If a local endpoint returns nothing, drop that field via compat:

```toml
[providers.my-local.compat]
include_usage_in_stream = false   # omit stream_options for picky OpenAI-compatible servers
```

The trade-off is no in-stream token counts for that provider. An empty stream now also
surfaces a visible error instead of a silent no-op.

A related compat field is `supports_stop_param` (default `true`). The engine
attaches "fluff" stop sequences as a client-side output token-optimization, but
some reasoning models / endpoints reject the OpenAI `stop` parameter outright
with a 400 (xAI's `grok-4.3`: *"Model grok-4.3 does not support parameter
stop"*). Set it `false` to suppress the optimization so those models run — xAI
sets this by default:

```toml
[providers.my-reasoning-endpoint.compat]
supports_stop_param = false
```

---

## Region-locked keys (MiniMax, Moonshot)

MiniMax and Moonshot each run **two** region-locked platforms that share the
wire protocol but **not** the key namespace — a key issued on one host 401s on
the other:

| Provider | Default host | Alternate host |
|----------|--------------|----------------|
| MiniMax | `api.minimax.io` | `api.minimaxi.com` |
| Moonshot | `api.moonshot.ai` | `api.moonshot.cn` |

On a 401/403 the engine retries the **same** key against the alternate host and
pins whichever authenticates for the rest of the session — no user action and no
config required. This is driven by the `auth_fallback_base_url` compat field
(set by `minimax_defaults` / `moonshot_defaults` in
`crates/wcore-config/src/compat.rs`; the retry-and-pin lives in
`wcore-providers` `anthropic.rs` / `openai.rs`). If a key 401s on **both**
regions it is simply invalid — issue one on the other region's console.

---

## Profile Inheritance

Profiles support `extends` to inherit settings from another profile, avoiding duplication.

### Configuration

```toml
# Base profile
[profiles.base-anthropic]
provider = "anthropic"
api_key = "sk-ant-xxx"

# Inherits base-anthropic, overrides model
[profiles.claude-fast]
extends = "base-anthropic"
model = "claude-haiku-4-5-20251001"
max_tokens = 4096

[profiles.claude-deep]
extends = "base-anthropic"
model = "claude-opus-4-8"
max_tokens = 16384

# Profile can specify which MCP servers to use
[profiles.dev]
extends = "base-anthropic"
model = "claude-sonnet-4-6"
mcp_servers = ["filesystem", "github"]
```

### Usage

```bash
wayland-core --profile claude-fast "Quick question"
wayland-core --profile claude-deep "Deep security audit"
wayland-core --profile dev "Create a GitHub issue"
```

- Supports multi-level inheritance chains
- Auto-detects circular inheritance
- Child profile settings override parent

---

## AWS Bedrock

Access Claude models via AWS Bedrock with SigV4 authentication.

### Configuration

```toml
[default]
provider = "bedrock"

[bedrock]
region = "us-east-1"
# Option 1: Explicit credentials
access_key_id = "AKIA..."
secret_access_key = "..."
# session_token = "..."

# Option 2: AWS profile
# profile = "my-profile"

# Option 3: Environment variables (AWS_ACCESS_KEY_ID, AWS_SECRET_ACCESS_KEY)
# Used automatically when no credentials are configured

[profiles.bedrock-claude]
provider = "bedrock"
model = "anthropic.claude-sonnet-4-6-20251015-v1:0"
# or: model = "bedrock:sonnet"   (short-form, see Model short-forms below)
```

### Credential Priority

1. Explicit credentials in config file
2. AWS profile
3. Environment variables (`AWS_ACCESS_KEY_ID`, `AWS_SECRET_ACCESS_KEY`, `AWS_SESSION_TOKEN`)

---

## Google Vertex AI

Access Claude models via Google Vertex AI with GCP OAuth2 authentication.

### Configuration

```toml
[default]
provider = "vertex"

[vertex]
project_id = "my-gcp-project"
region = "us-central1"

# Option 1: Service Account key file
credentials_file = "/path/to/service-account.json"

# Option 2: Application Default Credentials
# Run: gcloud auth application-default login

# Option 3: Metadata Server (auto on GCE/GKE/Cloud Run)
# Used automatically when in GCP environments

[profiles.vertex-claude]
provider = "vertex"
model = "claude-sonnet-4-6@20251015"
# or: model = "vertex:sonnet"    (short-form, see Model short-forms below)
```

### Auth Methods

| Method | Use Case |
|--------|----------|
| Service Account Key | CI/CD, server-side apps |
| Application Default Credentials | Local development (requires gcloud CLI) |
| Metadata Server | GCE/GKE/Cloud Run and other GCP environments |

---

## Ollama (local inference, W8a)

Ollama is shipped as a plugin (`wayland-ollama`) rather than as a
built-in provider. The plugin registers an `LlmProvider`
implementation through `wcore-plugin-api::register_providers`; the
engine downcasts to a real provider via the existing
`HostProviderRegistrar` path.

### Selection

```bash
wayland-core --model ollama:llama-4
wayland-core --model ollama:qwen3-coder
```

The `ollama:` prefix routes through the wayland-ollama plugin. The
suffix is the model name as known to your local Ollama daemon. The
plugin contacts `http://localhost:11434` by default; override via
the standard `OLLAMA_HOST` environment variable.

### Requirements

- The `wayland-ollama` plugin must be enabled in `plugins.toml`
  (default: enabled). Disable via:
  ```toml
  [plugins.wayland-ollama]
  enabled = false
  ```
- A running Ollama daemon and a pulled model. See
  https://ollama.com for installation.

### Capability flag

`capabilities.plugins` flips to `true` whenever any plugin (including
wayland-ollama) is loaded — see W8c.3 H.2 plugin-aware capability
advertising in `crates/wcore-agent/src/output/protocol_sink.rs`.

---

## Sign in with ChatGPT

Authenticate with your **ChatGPT subscription** instead of an OpenAI API key and
route inference through the ChatGPT **Codex** backend
(`chatgpt.com/backend-api/codex`). API-key OpenAI (`--provider openai`) is
untouched and remains the always-works fallback — this path degrades to "logged
out," never to a broken engine.

### Logging in

```bash
wayland-core auth login chatgpt
```

This opens your browser to OpenAI's sign-in page (a loopback PKCE flow on
`http://localhost:1455/auth/callback`). Approve the request and the tokens are
written **encrypted** to:

```
~/.wayland/oauth/chatgpt.json     # dir mode 0700, file mode 0600 on Unix
```

The stored access token is a JWT; your `chatgpt_account_id` is read from it (no
separate API call) and sent as the `chatgpt-account-id` request header. Login
fails if the token carries no account id. Refresh tokens **rotate** (single-use)
and are re-persisted transparently on every turn near expiry, so sign-in
survives across sessions without re-authenticating.

> If port `1455` is already in use the login errors with guidance — it is the
> exact redirect URI registered against OpenAI's Codex client and cannot be
> changed. A device-code flow for headless/SSH hosts is a planned follow-up.

### Using it

Select the provider and a Codex model:

```bash
wayland-core --provider openai-chatgpt --model gpt-5.5 "explain this repo"
```

`chatgpt` is accepted as an alias for `openai-chatgpt`. The default model is
`gpt-5.5`. Available Codex model ids:

| Model id | Short-form |
|----------|-----------|
| `gpt-5.5` (default) | `openai-chatgpt:5.5` |
| `gpt-5.5-pro` | `openai-chatgpt:5.5-pro` |
| `gpt-5.4` | `openai-chatgpt:5.4` |
| `gpt-5.4-codex` | `openai-chatgpt:5.4-codex` (or `openai-chatgpt:codex`) |
| `gpt-5.3-codex` | `openai-chatgpt:5.3-codex` |
| `gpt-5.3-codex-spark` | `openai-chatgpt:5.3-codex-spark` |

These ids are valid **only** for `--provider openai-chatgpt`; they are not
OpenAI API model names.

### Status and logout

```bash
wayland-core auth status          # signed in (plan: pro), expires in N min — or "not signed in"
wayland-core auth logout chatgpt  # clears the in-memory cache + on-disk token + any tmp orphan
```

### Importing a Codex CLI login

If you already signed in with OpenAI's Codex CLI, import its tokens instead of
re-running the browser flow:

```bash
wayland-core auth login chatgpt --import-codex
```

This reads `$CODEX_HOME/auth.json` (default `~/.codex/auth.json`), validates the
file's ownership/permissions, decodes the account id, and stores the tokens
under `~/.wayland/oauth/chatgpt.json`. `wayland-core auth status` also attempts a
one-shot import when no wayland token exists yet.

### Fallback

If anything about subscription auth stops working, switch back to an API key at
any time:

```bash
wayland-core --provider openai --model gpt-4o "..."   # always-works fallback
```

### A note on Terms of Service

This path reuses OpenAI's **published Codex** `client_id`
(`app_EMoamEEZ73f0CkXaXp7hrann`) to authenticate a ChatGPT subscription for a
third-party agent — outside that client's originally intended use. It is what
the open-source Codex/OpenClaw clients do and is **tolerated in practice today**,
but there is no cited explicit permission; "allowed in practice" is an
observation, not a guarantee. If OpenAI tightens client/originator/edge checks,
this path may break — API-key auth is the supported, always-works alternative.

---

## Sign in with Grok (xAI)

Grok runs under `--provider xai` (alias `grok`). There is **no** `auth login`
command for it — connect one of two ways:

**API key.** Set `XAI_API_KEY` (or `api_key` in `[providers.xai]`) and run:

```bash
wayland-core --provider xai --model grok-4.3 "explain this repo"
```

**OAuth refresh.** The engine refreshes xAI OAuth tokens itself, at parity with
[Sign in with ChatGPT](#sign-in-with-chatgpt) (load / refresh / persist over the
~6h access-token lifetime, no host re-spawn). It does **not** start a browser
login flow — it reads tokens that already exist on disk, from whichever source
is **fresher**:

```
~/.grok/auth.json            # the Grok CLI's credential file ($GROK_HOME/auth.json when set)
~/.wayland/oauth/xai.json    # the engine's own store (written by an app or a prior refresh)
```

Preferring the fresher file avoids racing the Grok CLI for the **single-use,
rotating** refresh token (xAI rotates it on every refresh). Access tokens last
~6h. When OAuth credentials are present, the `xai` API-key gate is exempt, so no
`XAI_API_KEY` is needed. The OAuth client id is pinned but overridable at runtime
via `WAYLAND_XAI_OAUTH_CLIENT_ID` (no rebuild). Evidence:
`crates/wcore-agent/src/oauth/xai.rs`, `crates/wcore-config/src/config.rs`
(`xai_oauth_credentials_present`).

> Spawn Grok as `--provider xai`, never `--provider openai` — see
> [Host integration: pick the right `--provider`](#host-integration-pick-the-right---provider).
> Under `openai` the OAuth token files are ignored and the unsupported `stop`
> parameter is sent.

---

## Model short-forms (W8 / B.4)

Bedrock and Vertex IDs are long (`anthropic.claude-sonnet-4-6-20251015-v1:0`,
`claude-sonnet-4-6@20251015`). The CLI accepts shorthand of the form
`<provider>:<role>` and expands it to the canonical literal before the
provider request is built.

```bash
wayland-core --model bedrock:sonnet     # ⇒ anthropic.claude-sonnet-4-6-20251015-v1:0
wayland-core --model bedrock:opus       # ⇒ anthropic.claude-opus-4-6-20251015-v1:0
wayland-core --model bedrock:haiku      # ⇒ anthropic.claude-haiku-4-5-20251001-v1:0
wayland-core --model vertex:sonnet      # ⇒ claude-sonnet-4-6@20251015
wayland-core --model vertex:opus        # ⇒ claude-opus-4-6@20251015
wayland-core --model vertex:haiku       # ⇒ claude-haiku-4-5@20251001
wayland-core --model vertex:gemini-pro  # ⇒ gemini-2.5-pro
wayland-core --model vertex:gemini-flash # ⇒ gemini-2.5-flash
wayland-core --model anthropic:sonnet   # ⇒ claude-sonnet-4-6
wayland-core --model openai:gpt-4o      # ⇒ gpt-4o
```

Strings that don't match a known `<provider>:<role>` pair flow through
verbatim — so fully-qualified literals (e.g. a pinned `…-v2:0` revision)
still work. The canonical pins live in
[`crates/wcore-types/src/model_aliases.rs`](../crates/wcore-types/src/model_aliases.rs);
update there once when a model deprecates, every dependent fixes itself.
