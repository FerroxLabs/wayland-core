# Providers & Authentication

## Supported Providers

| Provider | Auth Method | Notes |
|----------|------------|-------|
| Anthropic | API Key | Prompt caching, streaming, vision |
| OpenAI | API Key | Compatible with DeepSeek, Qwen, Ollama, vLLM |
| AWS Bedrock | SigV4 | Regional endpoints, AWS credential chain |
| Google Vertex AI | GCP OAuth2 / Service Account | Metadata server auto-detection |

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
- The underlying provider must currently be one of `anthropic`, `openai`, `bedrock`, `vertex`
- The alias entry's `model`, `api_key`, `base_url`, and `compat` override the underlying provider's defaults

This fits scenarios like DeepSeek gateways and internal OpenAI-compatible services.

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
