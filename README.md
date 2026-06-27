<div align="center">

![Wayland Core — Forged to run. Hardened to last. Built to evolve.](docs/img/hero.png)

# Wayland Core

### The self-evolving AI agent. Brilliant today, smarter tomorrow.

**Most AI tools are as good as they'll ever be the day you install them. Wayland Core isn't — it convenes a council of rival models on your hardest problems, fuses their best answer into one, and rewrites its own prompts to get sharper every single run. Terminal-first, on your keys, in Rust.**

Terminal-first · Multi-provider · Self-evolving · MCP-native · Embeddable · Apache-2.0

[![npm](https://img.shields.io/npm/v/@ferroxlabs/wayland-core?style=for-the-badge&logo=npm&logoColor=white&label=npm&color=e85d2a)](https://www.npmjs.com/package/@ferroxlabs/wayland-core)
[![CI](https://img.shields.io/github/actions/workflow/status/FerroxLabs/wayland-core/ci.yml?style=for-the-badge&logo=githubactions&logoColor=white&label=CI&branch=main)](https://github.com/FerroxLabs/wayland-core/actions/workflows/ci.yml)
[![license](https://img.shields.io/badge/license-Apache--2.0-3b3b3b?style=for-the-badge)](LICENSE)
[![Rust](https://img.shields.io/badge/built_with-Rust-dea584?style=for-the-badge&logo=rust&logoColor=black)](https://www.rust-lang.org/)
[![platforms](https://img.shields.io/badge/macOS_·_Linux_·_Windows-2b2b2b?style=for-the-badge)](#install)
[![status](https://img.shields.io/badge/status-public_beta-e85d2a?style=for-the-badge)](#built-to-endure)

[Install](#install) · [Quick start](#quick-start) · [Providers](#provider-neutral-core) · [Orchestration](#orchestration--swarms) · [Crucible](#crucible--a-mixture-of-providers-council) · [Security](#security-by-default-fail-closed) · [Channels](#omni-channel-deployment--scheduled-triggers) · [Browser](#browser--computer-use) · [Memory](#memory-sessions--cost-governance) · [Evolution](#self-evolution-gepa) · [Endurance](#built-to-endure) · [Embedding](#embedding-json-lines-protocol--acp-interop) · [Docs](#documentation)

</div>

---

Most agents are frozen the day you install them, and married to one model. Wayland Core is neither. Hand it a hard problem and it convenes a **council of rival models** that cross-audit into one answer ([Crucible](#crucible--a-mixture-of-providers-council)). It **rewrites and scores its own prompts** between runs ([GEPA](#self-evolution-gepa)). Every tool runs in an OS-native sandbox behind a single egress gate, and it speaks [MCP](https://modelcontextprotocol.io/) in both directions — all from one Rust binary, on your keys. It's the engine inside [Wayland Desktop](https://getwayland.com), but it stands alone: a one-shot command, a full-screen TUI, or a headless stream you embed.

> **Wayland Core** is the engine, on its own, open (this repo, Apache-2.0). **[Wayland Desktop](https://getwayland.com)** is the GUI product built on it. Core is the engine; Desktop is one application that embeds it.

## The 30-second proof

```bash
npx @ferroxlabs/wayland-core "read Cargo.toml, list the workspace crates, and explain the dependency layering"
```

One command. The agent reads the file, runs `grep`/`glob` across the tree, reasons, and answers, with every tool call gated and streamed. Or run `wayland-core` with no arguments and it detects your provider keys and drops you into the TUI:

<div align="center">

![Wayland Core — connect a provider](docs/img/screenshot-onboarding.png)

</div>

**Paste a key, get a provider.** Paste an API key (or run `/connect` in the TUI) and the engine fingerprints the provider from the key's shape, validates it live, and stores it in your OS keyring. From there, `/config` exposes Essentials and Advanced editors, `/doctor` shows provider, key, and MCP health, and `/effective` prints the resolved config with secrets redacted.

## What it is

- **A standalone engine.** The engine is the product, not a feature bolted onto an editor and not a wrapper around one vendor's API.
- **Terminal-first.** A one-shot command, an interactive TUI, or a headless stream. The terminal is the primary home, not an afterthought.
- **Embeddable.** Drive it from your own app over a typed JSON-Lines protocol. It is exactly how Wayland Desktop uses it.
- **Apache-2.0.** Permissive. Build on it commercially without an AGPL obligation.

## Install

**npm** (recommended, pulls the right prebuilt binary for your platform):

```bash
npm install -g @ferroxlabs/wayland-core
wayland-core --version
```

```bash
# or run it once, no install
npx @ferroxlabs/wayland-core "summarize the TODOs in this repo and draft a triage plan"
```

**Prebuilt binaries** for macOS (arm64/x64), Linux (arm64/x64), and Windows (arm64/x64) are on the [Releases](https://github.com/FerroxLabs/wayland-core/releases) page, each verifiable against `wayland-core-checksums.txt`.

**From source** (Rust 1.95+):

```bash
cargo install --git https://github.com/FerroxLabs/wayland-core wcore-cli
```

## Quick start

```bash
# 1. Generate a config, then add an API key for any provider
wayland-core --init-config
wayland-core --config-path        # shows where the config lives

# 2. One-shot: the agent reads files and uses tools to answer
wayland-core "Read Cargo.toml and explain the dependencies"

# 3. Interactive TUI (just run it)
wayland-core

# 4. Everything else
wayland-core --help
```

---

## Provider-neutral core

The engine never knows which vendor it's talking to. It builds one neutral request type, `LlmRequest`, and reads one neutral event stream, `LlmEvent` — `TextDelta`, `ToolUse`, `ThinkingDelta`, `Done`, `Error`. That's the whole contract. Every provider adapter implements a single async trait, `LlmProvider`, whose core method is `stream(&LlmRequest) -> Receiver<LlmEvent>`. Wire-format translation happens inside the adapter, where it belongs. The agent loop above it stays vendor-blind.

Vendor quirks don't get hardcoded. There is no `if base_url.contains("openai.com")` branch anywhere. The differences — field names, message-shape rules, which API surface to hit, reasoning vs. thinking, tool-array caps, temperature support, cache markers — live in one configuration layer, `ProviderCompat`: 31 `Option<T>` fields where `None` means "use the provider's default." 24 preset constructors set those defaults per vendor, and a single map binds each of the 23 built-in providers to its preset. Your config layers on top. Every field resolves as `user.or(default)`, so anything you set wins and anything you leave alone keeps the shipped default. Adapters then read compat instead of sniffing URLs: `api_path()`, `max_tokens_field`, `uses_responses_api()`, `supports_temperature`, `include_usage_in_stream`, and the rest.

- **23 built-in providers, one `--provider <slug>` switch.** The slug picks the wire, the base URL, and the compat preset.
- **Point any OpenAI-compatible backend at a built-in wire** with a custom alias — set `provider`, `model`, `api_key`, `base_url`, and you're done. No code.
- **Override a quirk in config, not in a fork.** A self-hosted server that rejects `stream_options`? `include_usage_in_stream = false`.
- **Data-driven pricing.** A bundled `pricing.toml` — 46 model rows across 25 provider tables — computes per-token cost in integer microcents from per-Mtok USD rates. Swap the whole catalog with `WAYLAND_PRICING_PATH`.
- **Resilience is built in.** Transient failures retry automatically, with multi-key rotation on supported providers; opt into a circuit breaker plus same-provider model fallback with one `[provider_chain]` block.

```toml
# Point a custom backend at the OpenAI wire, then bend one quirk
[providers.my-service]
provider = "openai"
model    = "custom-model-v1"
base_url = "https://my-service.example.com/api/openai"

[providers.my-service.compat]
include_usage_in_stream = false   # self-hosted server rejects stream_options
```

<div align="center">

![Providers and model routing — Wayland Core vs OpenClaw, Hermes, opencode, aider](docs/img/compare-providers.png)

</div>

## Orchestration & swarms

A single agent is the floor, not the ceiling. Wayland Core fans one task out across many workers and brings the results back, with real isolation between them. Three distinct mechanisms ship in the code, and a four-tier topology model governs all of them: **Spawn** (5 agents), **Swarm** (20), **Mesh** (50), **Fleet** (100). Each tier fixes the agent cap, how much the parent sees, and the blackboard scope — and the caps are enforced, not advisory. Ask for 51 agents on a 50-cap tier and you get `TopologyError::ExceedsCap`, not a quietly-truncated run.

- **Sub-agents (`Spawn`)** fan parallel work out from one tool call. Each sub-agent gets its own conversation context and its own tool access; the count is capped by the active topology (default Spawn, 5).
- **Worktree swarm** runs N workers as OS subprocesses, each in a fresh `git worktree` on its own branch. A dirty-checkout guard runs `git status --porcelain` first and **refuses to dispatch on an uncommitted tree** — that guard exists because a contamination incident in v0.2.2 taught us why it has to. Per-worker timeouts, `kill_on_drop` SIGKILL on expiry, and idempotent `git worktree remove --force` cleanup. Process isolation, not threads, so one bad worker can't corrupt another.
- **In-process dispatchers** (`MeshDispatcher`, `FleetDispatcher`) are library primitives: they coordinate caller-supplied agent closures over a shared blackboard, enforce the tier cap, apply a timeout, and reduce the reports. Fleet partitions agents into shards (default 10) under topic prefixes like `fleet/<run-id>/shard-<i>/`. They coordinate and reduce; spawning the agents is the orchestrator's job.

Every worker spawn goes through argv mode — `Command::new(program).args(args)`, no shell interpreter — so worker commands are never re-parsed by a shell. Final stdout/stderr come back through `collect()`; opt-in heartbeats (`.swarm-status.json`, ~5s tick) give you liveness without consuming the result.

Roll the results up however the job needs. The `wayland-core swarm` CLI dispatches the worktree path and routes the collected results through one of four reducers:

```bash
# Run the test suite across 4 isolated worktrees, roll up pass/fail/total
wayland-core swarm --workers 4 --worker-command "cargo test" \
  --base-branch main --branch-prefix swarm/ci --timeout 30m --reduce fleet

# Strict >50% majority over normalized worker stdout
wayland-core swarm --workers 5 --worker-command "pytest" --reduce consensus
```

- `mesh` — verbatim passthrough of every worker result.
- `fleet` — succeeded / failed / total roll-up.
- `consensus` — strict majority: a bucket wins only if its votes are more than half of the *successful* workers, otherwise the top three are returned as disputed.
- `debate` — first round whose workers agree wins; at the CLI the batch is a single round (multi-round replay lives in the orchestrator, not the CLI path).

Topology is pure data with cap enforcement, the guards have tests behind them (58 across the swarm crate), and the live TUI labels the running tier by sub-agent count — 0-5 Spawn, 6-20 Swarm, 21-50 Mesh, 51+ Fleet. One note on reach: the standard monitored relay clamps Spawn fan-out to the Mesh cap of 50, so the 100-agent Fleet ceiling is the unmonitored library path, not the everyday `Spawn` call.

<div align="center">

![Fleet spawn fan-out — one orchestrator, isolated workers, merged result](docs/img/diagram-swarm.png)

</div>

<div align="center">

![Orchestration, compared](docs/img/compare-orchestration.png)

</div>

## Crucible — a Mixture-of-Providers council

Crucible is a council of rival providers. Hand it a hard task and it fans out to N sub-agents, each pinned to its *own* LLM provider — Anthropic, OpenAI, DeepSeek, GLM, Kimi, Gemini, Flux-routed models — that answer in parallel; a separate, read-only judge then fuses them into one. The diversity is the whole point: cross-vendor, not one family arguing with itself. We call it **Mixture-of-Providers**.

<div align="center">

![A cross-vendor Crucible council convening live in the Wayland Core TUI — two proposers (Gemini, OpenAI) and an independent Anthropic judge, with the certified spend ceiling shown before a cent is spent](docs/img/crucible-tui.png)

</div>

*Convening a council live in the TUI: two proposers pinned to different vendors, an independent judge from a third, the certified ceiling ($0.70) beside the single-model cost ($0.49), the daily envelope, and the gate's reasoning — all on the table before you approve a cent.*

<div align="center">

![The fused result — a three-vendor council (Anthropic, OpenAI, Gemini) ranking JWT vulnerabilities by severity, every proposal, provider, and cost on the table](docs/img/crucible-run.png)

</div>

*…and the fused output: a three-vendor council ranking the audit by severity — every proposal, provider, and cost on the table. Head-to-head benchmarks (Crucible vs. router-level mixtures vs. solo frontier models, cost-matched) are in flight.*

It's off by default. List a roster in a `[crucible]` block:

```toml
[crucible]
enabled    = true
proposers  = ["anthropic:claude-opus-4-7", "openai:gpt-5", "deepseek:deepseek-v4-pro"]
aggregator = "anthropic:claude-opus-4-7"   # optional; falls back to the first usable proposal
```

Then run it: `wayland-core crucible "do a security audit of this deployment plan"`.

Each member pulls its *own* credentials from your `[providers]` map, so a council is genuinely keyed across vendors, not one key wearing hats. Routing prefixes don't defeat that — a Flux-pinned GPT-5 and a direct `openai:gpt-5` collapse to the same vendor family, so the judge stays independent and an Auto roster stays diverse.

Then the cost discipline, because N models answering one question costs N times as much:

- **A deterministic preflight gate decides whether to convene at all.** A zero-LLM keyword/length classifier reads the *leading* instruction span and sizes the roster by stakes — low goes Direct (one call), medium pulls 3 members, high pulls 5. A high-stakes word buried in a pasted stack trace won't escalate it.
- **Two roster modes.** Manual: you list the providers. Auto: a deterministic Assembler picks a cost-effective, vendor-diverse roster per task, and you can `--deny` a vendor or force `--deep`.
- **Spend is gated before anything spawns.** A judge-inclusive worst-case ceiling is certified up front. The per-run `max_cost_usd` cap is strict — an unpriceable roster under a cap is refused, not run. A default-on $20/user/day envelope rides on top.
- **It fails closed.** In a non-interactive session it refuses to spend unless you've explicitly opted in. On a TTY it prints a cost card and waits for Y/n.
- **The judge can't touch your machine.** The aggregator is a read-only sub-agent — no `Bash`, no `Write`, no `Edit`, by construction. Every proposal reaches it wrapped in untrusted-data fencing with forged section delimiters neutralized, so one poisoned proposer can't hijack the synthesis.

Fan-out is bounded by a per-route semaphore, and tail latency is capped: each proposer gets a hard deadline, and once quorum is met a global soft-deadline cancels the stragglers — timed-out members are kept as errored proposals so the provenance stays honest. The fused answer is either printed (Terminal mode) or injected as private guidance into the normal tool-using loop (Advisor mode, `--advisor`).

Shipped in v0.12.11. The council pipeline carries 84 unit tests plus 31 integration tests across the gate, resolver, roster validation, budget, fan-out, and injection-fencing paths.

**Where it's honest about its edges.** Some Flux-routed SKUs are unpriced today, so a Flux council can't always certify a hard ceiling — that's exactly why the per-run cap is opt-in and the daily envelope only soft-binds on Flux, accruing from actual usage instead of refusing up front. The daily envelope binds within a process, not yet across separate CLI runs. The convene-or-not gate is a deterministic heuristic, not a learned router. And the shipped invocation surface is the `wayland-core crucible` batch command; the slash command, natural-language tool, and full TUI/desktop approval cards are designed, not all shipped.

## Security by default (fail-closed)

Security here is a posture, not a checkbox. When the safe thing and the convenient thing disagree, the engine picks safe and makes you opt out on purpose. Four mechanisms carry that, and they hold up when someone reads the source.

- **No unsandboxed default.** Model-driven shell and tools run inside an OS-native sandbox — bubblewrap on Linux, `sandbox-exec` on macOS, AppContainer on Windows, Docker if you opt in. When no real sandbox is available, execution is refused, not quietly downgraded to host permissions. Running with no isolation takes an explicit `WAYLAND_ALLOW_NO_SANDBOX=1`. A stray `WAYLAND_SANDBOX=none` does nothing without it.
- **One egress chokepoint, enforced by a lint.** Every outbound HTTP request flows through a single client, and a clippy lint bans constructing a raw `reqwest` client anywhere else — so a missed migration fails the build instead of leaking a hole. On that seam sits a fail-closed host allowlist for untrusted URLs, an exfil-shape classifier that hard-denies suspicious POSTs and high-entropy paths to non-allowlisted hosts, a hard byte-cap body reader, and a resolve-once resolver that re-checks the IP at connect time to close DNS-rebinding races. Deny stops *before* the socket opens. Shared multi-tenant suffixes — `amazonaws.com`, `*.workers.dev`, `*.vercel.app`, around 45 of them — can never be apex-allowlisted.
- **SSRF and metadata floor, always on.** Cloud-metadata endpoints (`169.254.169.254` and the GCP, AWS, Alibaba, and Oracle equivalents) and lookalike hosts are rejected outright, independent of any allowlist you configure.
- **Output validation and secret scrubbing.** Model output runs through a validator (refusal, credential-leak, and format checks) and a scrubber that redacts about 30 credential and PII shapes — AWS, OpenAI, Anthropic, GitHub, Slack, and Stripe keys, JWTs, PEM private keys, DB connection strings — down to `[REDACTED:KIND]`. An optional LLM-judge validator is budget-capped and fails *open*: it skips when the budget is spent and never blocks your turn.
- **Default-deny permissions.** A multi-actor policy engine gates every (actor, resource, action) tuple; no matching grant means denied. File globs reject any `..` segment before matching, and bearer tokens are SHA-256 signed with a TTL, revocation, and rotation that cannot extend an expiry.

The egress gate is on out of the box. Tell it what to trust:

```toml
[security]
enabled = true                          # default; egress gate on
egress_allow = ["example.com",          # registrable domain — covers subdomains
                "myapp.workers.dev"]     # shared host — exact match only, no apex
```

Turning the gate off is deliberately awkward. It takes `enabled = false` in the config file *and* a `--i-accept-exfil-risk` flag on the command line at the same invocation. No single environment variable can silence it.

The source backs this: the sandbox crate alone carries 110+ test cases, with 90+ more across output safety and 50+ across permissions, and the threat model is written down in `docs/security/permissions-threat-model.md`.

<div align="center">

![Security: fail-closed — sandbox plus one CI-enforced egress gate](docs/img/diagram-security.png)

</div>

<div align="center">

![Security and sandboxing, compared](docs/img/compare-security.png)

</div>

## Browser + computer use

The agent doesn't stop at the filesystem. Wayland Core ships two real desktop-automation tool families, both in-tree, both security-gated: a browser the agent drives, and synthesized control of the host desktop itself.

**The `Browser` tool** is an interactive browser with a fixed, locked surface of 18 operations — navigate, snapshot, read, click, fill, press, select, upload, download, screenshot, get_state, wait_for, network_log, console, new_tab, close_tab, back, forward. It is ARIA-tree-first by design: the model reasons over an accessibility snapshot with `@e1`/`@e2` element refs, not raw DOM, so a page costs a bounded chunk of the prompt budget instead of dumping a megabyte of markup at the model. There is no JavaScript-evaluation op. The `Evaluate` variant is deliberately banned, and a test enforces its absence. For a plain read-only fetch the tool tells the model to use `WebFetch` instead.

**The `Cua` tool** is computer use: synthesized mouse, keyboard, screenshot, and accessibility-tree control of the host across macOS, Linux X11, Linux Wayland, and Windows. Eleven locked ops — left_click, right_click, double_click, mouse_move, scroll, type, key, screenshot, ax_tree, wait, frontmost_app. The defining invariant is that it stays out of your way: synthesized input must never move your cursor, raise a window, or steal foreground focus. On macOS that means input is posted at the HID layer (`CGEventTapLocation::HID`) and the code never calls an `activate` API — a `focus_invariance_test` locks it down.

- **Multi-backend, runtime-selected.** Camoufox is the default and talks to a sidecar over HTTP; Chromium (chromiumoxide/CDP) and Browserbase (cloud) are opt-in behind cargo features. The default build is Camoufox-only. CUA picks its backend from the platform: CGEvent on macOS, x11rb XTest on Linux X11 (on by default), `wlrctl` + `grim` on Wayland (opt-in), UI Automation + SendInput on Windows. An unsupported target returns a typed error, never a silent no-op.
- **Fail-closed network policy.** `BrowserPolicy` runs pre-dispatch on every URL-bearing op, `default_action = Deny` since v0.2.1. Scheme allowlist is http/https only. Hardcoded blocks — regardless of your allow/deny lists — cover RFC1918 ranges, loopback, the `169.254.169.254` metadata endpoint, link-local, IPv6 ULA, and legacy IPv4 encodings (octal, hex, decimal). A TOFU cache pins host→first-IP to refuse DNS-rebinding, and the policy is re-evaluated on every redirect hop, capped at 10.
- **Filesystem confinement.** Model-chosen download and upload paths must be absolute, no `..`, no null bytes, no dotfile or OS-secret target, symlink-aware and confined to a downloads root. If you set none, it fails closed onto a temp directory so confinement always runs.
- **App-aware HITL gating.** `CuaPolicy` resolves the frontmost app and gates every op: forbidden apps are rejected, others suspend for human approval. Forbidden key combos are checked on both `Key` ops and `Type` text, with unicode/glyph normalization (`Cmd+Q`, `command-q`, `^Q`, glyphs all canonicalize to `cmd+q`). Unknown frontmost app plus any app-scoped rule routes to Suspend.
- **Screenshot redaction.** Two passes: an always-on heuristic password-band blur, then OCR-backed sensitive-text blur (Apple Vision on macOS, Windows.Media.Ocr on Windows, Tesseract on Linux behind a feature). The matcher catches emails, SSNs, 13–19-digit cards, and key prefixes like `sk-`, `ghp_`, `aws_secret_`. Best-effort, off by default.
- **Isolated, cancellable.** Each sub-agent gets its own cookie jar and tab. Every op races a wall-clock deadline, the cancel token, and completion in a `tokio` select — Navigate gets 60s, Click/Fill 10s, and so on, with the browser running as a 120s-budget MCP-category tool, not a 600s one.

Both families register through a thin plugin shell. Per audit F2 the `wayland-browser` and `wayland-cua` crates carry no dependency on the real `wcore-browser`/`wcore-cua` engine crates — they register a spec mirror through the plugin API and the host reifies the actual tool, so the isolation boundary is structural, not a convention. The host has to advertise `capabilities.browser_suite` / `capabilities.computer_use` or neither family registers at all. On Linux Wayland the adapter goes further and refuses registration outright on a restricted compositor (GNOME mutter default, focus-steal-off Hyprland), probed live and re-checked mid-session.

```toml
[browser]
# Disabled by default (fail-closed). Allow specific domains to turn it on:
allowed_origins = ["example.com", "*.mysite.com"]
# Or, not recommended (SSRF risk):
# default_action = "allow"
```

The depth shows up in the tests: 120+ `#[test]`/`#[tokio::test]` attributes across `wcore-browser` (the policy suite alone has 27), 99 across `wcore-cua`. [→ docs/tools.md](docs/tools.md)

## Omni-channel deployment & scheduled triggers

The same engine runs as a chat bot on ten messaging platforms, and fires itself on a schedule. Drop a TOML file in `~/.wayland/channels/`, boot the engine, and every enabled channel auto-registers. From there the agent answers inbound DMs and group messages with a real agent turn — reading, tool-calling, reasoning — and sends the reply back through the same platform.

Each platform is its own crate, written against the platform-native API, not a generic webhook shim: **Slack** (Web API plus an Events webhook, HMAC-SHA256 signed with a 5-minute replay window), **Discord** (REST plus a Gateway WebSocket with heartbeat), **Telegram** (long-poll), **WhatsApp** (Cloud API plus Meta `X-Hub-Signature-256`), **Signal** (a `signal-cli` subprocess under a respawn supervisor), **SMS** (Twilio, HMAC-SHA1 webhook), **email** (SMTP out, IMAP poll in), **iMessage** (macOS, reads `chat.db` read-only, sends via AppleScript), **Matrix** (raw CS-API, deliberately no `matrix-sdk`), and **MS Teams** (Bot Framework, OAuth2 client-credentials).

- **One file per channel, auto-registered on boot.** The engine scans the channels dir, parses each TOML, and registers the rest. A disabled, unknown, or malformed config is skipped with a warn log — one bad file can never crash boot.
- **Inbound is fail-closed.** An unconfigured channel denies every message. The default DM policy is an *empty* allowlist, groups are disabled, and a mention is required. You name the stable platform sender ids that may drive the agent; everyone else is refused.
- **A tool posture decides what the agent may touch.** `conversational` (default) gives the host no filesystem or shell. Opt up to `workspace` (jailed to a workspace root) or `full` (host-wide). It is enforced at the tool registry, so dropped tools are un-dispatchable, not merely hidden.
- **Secrets are handles, not tokens.** The TOML carries a credential handle resolved from the OS credentials store at connection time. Over-long replies are chunked per the connector's own message-length limit, and reconnects back off and retry.

```toml
# ~/.wayland/channels/tg.toml
name = "tg"
platform = "telegram"
enabled = true

[options]
credential_handle = "telegram.acme.bot_token"

[inbound]
dm = "allowlist"
dm_allowlist = ["123456789"]   # platform sender ids; "*" = anyone
group = "disabled"             # open | allowlist | disabled
require_mention = true
tools = "conversational"       # conversational (default) | workspace | full
ack = "both"                   # off | reactions | typing | both
```

**Scheduled triggers** are the other half. The cron subsystem parses standard 5-field crontab (and 6/7-field) expressions and fires one of three action types on a recurrence: run a slash command, post a message to a channel, or invoke a skill. Jobs persist to `~/.wayland/cron/jobs.json` with an atomic write; outcomes append to a history ring buffer. It ticks every 30 seconds — either inline at engine boot, or as a detached daemon:

```bash
wayland-core cron add "0 9 * * *"   --skill morning-brief
wayland-core cron add "*/15 * * * *" --slash "/status"
wayland-core cron add "0 8 * * 1"   --channel team --text "Good morning"
wayland-core cron list           # also: status · history · enable · disable · logs
wayland-core cron daemon         # detached background runner
```

**What we do not claim yet.** Not every platform is full duplex. MS Teams is a send-only MVP; inbound is parsed but not yet exposed over the host. The inbound webhook host serves Slack, WhatsApp, and Twilio SMS only — the poll-based connectors (Telegram, Matrix, Signal) don't take webhooks. DM pairing is fail-closed: a pairing request is denied until you add the sender to the allowlist by hand. In the headless daemon, `--skill` and `--channel` jobs dispatch, but `--slash` jobs record as *staged* rather than executing. And inbound media enrichment (image-to-description, voice-to-transcript) is inert unless a vision or transcription key is configured. `tools = "full"` on a publicly reachable channel is host-wide access identical to a local CLI session — the code and docs both flag it as dangerous.

Ten platform crates, in-tree, each wired to a registry factory. More than 500 tests across the channels, registry, cron, and per-platform crates. [→ docs/channels.md](docs/channels.md)

## Memory, sessions & cost governance

An agent that forgets every session and spends without a ceiling is a liability. Wayland Core gives it a persistent brain that's on out of the box and hard money guardrails you set, both built to hold up when someone reads the source.

**Cross-session memory.** A SQLite-backed store so the agent remembers what it saw, did, learned, and concluded, instead of starting blank every run. It is organized as five partitions (Working, Episodic, Semantic, Procedural, and a Core user-model) across three durability tiers (Session, Project, Global), which is exactly nine valid cells, not fifteen. The dispatcher enforces that matrix and rejects a write to any invalid cell, with the count locked by unit tests. It is **on by default** — a fresh install gets a real memory backend, so self-evolution, skill routing, and the user-model all work out of the box; opt out with `--no-memory` or `memory.enabled = false`. It never retroactively ingests sessions from before it was active: memory is forward-only by design.

It also keeps itself from growing unbounded:

- **Decay, not deletion.** A relevance score of `exp(-age_days / 7.0)` ages entries down, and episodes past ~30 days flip to Archived. Nothing is ever `DELETE`d, so old context stays queryable while it stops dominating recall.
- **A dream cycle at session end** runs four phases in order: Compress (summarize batches of Working entries into Episodes), Consolidate (Episodic into Semantic facts), Crystallize (a pattern seen three or more times becomes a staged Procedure), then Decay. It is throttled to once every 30 minutes by default.
- **Deny-by-default access.** Every read and write validates an access token against a partition-plus-tier ACL, and every access is logged to an audit DB. Facts are append-only; a correction is a new row that supersedes the old one, and secrets are stripped on the compaction path.

**Sessions.** Every run is saved to disk, the provider, model, working directory, token usage, and full message history, under a versioned schema with a migration ladder and WAL crash recovery, so a `SIGKILL` mid-turn does not corrupt the file. Resume the most recent with `-c`, jump to a specific one with `--resume <id>`, see them all with `--list-sessions`, or print what the agent remembers about one with `--memory-show <session>`.

**Cost governance.** Real spend caps that block the next turn when crossed. Set per-session token and dollar caps; a rejected charge does not stick, verified by tests that assert the running total is unchanged after a blocked overrun. A separate execution budget tracks a whole session tree, wall time, tool runtime, process count, agent depth, tokens, and cost, rolling child counters up to ancestors and checking caps in a fixed, deterministic order. Per-turn cost is computed from a real provider-by-model pricing catalog (USD per million tokens, bundled at compile time, 46 sections today) and returned in integer microcents, so a million input tokens at $15/Mtok is exactly 1,500,000,000 microcents, no float drift. The engine charges against the model that was actually dispatched, not the premium tier you asked for.

Two honest limits, stated up front. Caps are **post-hoc, not pre-flight**: a turn is billed by the provider before it is charged against the budget, so a cap cannot un-spend the turn that crossed it, only block the next one. And cost accuracy depends on the catalog, a miss falls back to a heuristic that for some models is $0.00, an honest absent charge rather than a wrong one. The live-pricing refresh diffs OpenRouter against the bundled catalog on a 24-hour TTL but only emits change events for a human to inspect; it never auto-applies a price.

```toml
# .wayland-core.toml — memory is ON by default; spend caps are opt-in

[memory]
enabled = true                     # the default; set false (or --no-memory) to opt out
dream_cycle_throttle_secs = 1800   # min gap between dream cycles (30 min)
decay_interval_secs       = 3600   # decay sweep cadence (1 hour)

[session_cap]
max_tokens_in  = 200000
max_tokens_out = 16384
max_cost_usd   = 1.50              # blocks the NEXT turn once crossed
max_wall_time_secs    = 600        # execution-tree caps
max_tool_runtime_secs = 120
max_processes         = 8
max_agent_depth       = 4
```

```bash
wayland-core --list-sessions
wayland-core -c                              # resume most-recent
wayland-core --resume 2026-05-16-abc123
wayland-core --memory-show 2026-05-16-abc123
wayland-core --compaction full               # off | safe | full (default: safe)
```

[→ docs/memory.md](docs/memory.md)

## Self-evolution (GEPA)

This one doesn't stay the version you installed. It rewrites its own prompts and skills, scores every rewrite, and keeps only the versions that actually win.

The unit of evolution is a skill: markdown plus YAML front matter. Hand it a seed and the loop does the rest. Each generation it mutates the seed into a fan-out of variant children, scores every child, and keeps a child only when it clears two bars at once — the running best *and* its own parent. Everything that loses is written to a graveyard on disk, one JSON file per child, so you can see what was tried. Winners persist to a cross-run SQLite table (`evolved_prompts`, in the global memory tier), so the next run bootstraps from the last run's best instead of starting cold.

The scoring never touches an LLM. That is the point. `wcore-eval` is a deterministic, LLM-free trust boundary, so a model can't grade its own homework into a false win.

- **Deterministic by construction.** Mutations come from a blake3 hash of `(parent_hash, generation, child_index)` seeding a ChaCha20 RNG, so the same lineage always produces byte-identical output. Three of the four mutators — Reorder, SwapSynonym, Precondition — are pure Rust; only Paraphrase calls a model, and even that is fixture-replayable.
- **Fixed mutator rotation.** One parent, no crossover. Each generation picks one mutator round-robin from `[Paraphrase, Reorder, SwapSynonym, Precondition]` and produces `--fan-out` children (default 4).
- **It knows when to stop.** Termination on a generation ceiling, a plateau (rolling window, default 3, min-delta 0.01), budget exhaustion, or no improvement. A NaN or non-finite score is refused outright as `ScoreInvalid` rather than spinning the loop.
- **Bounded.** A per-child timeout (default 30s) wraps mutate-plus-score so a hung paraphrase can't eat the run budget, and the loop checks the budget between every child.
- **Curator hand-off.** Winners never overwrite the live catalog directly. They pass through a curator boundary (`Promote` / `Archive`) before they land — promotion into the live skill set stays a deliberate step, not an automatic one.

Two ways to run it:

```bash
# Offline: evolve a seed skill, LLM-free scoring, winners persisted, losers graveyarded
wcore-evolve --seed-file skill.md --seed-name my-skill \
  --generations 5 --fan-out 4 --child-timeout-secs 30

# Live: at session end, paraphrase the system prompt of a successful run
wayland-core --online-evolution "refactor the auth module"
#   [observability]
#   online_evolution = true   # config equivalent
```

The offline binaries are developer tools for tuning skills before they ship: `wcore-evolve` scores skill-metadata structure (nine static checks), and `wcore-evolve-bench` scores end-to-end task outcomes against a 30-case mini-bench. The user-facing surface is the opt-in `--online-evolution` flag (off by default): when a session ends with at least half its turns using tools, the engine emits an evolution event, paraphrases that session's system prompt, and saves the variant to `$WAYLAND_HOME/evolved/`.

A sibling crate, `wcore-dispatch`, handles the other half of "learn what works" — a Thompson-sampling router (Beta posterior over success/failure) that learns which templates, agents, and skills to pick. It is not a model router; model routing lives in Flux.

**What it does not do yet.** The live session "success" signal is shallow — it measures tool-usage density, not task correctness. Winners are persisted, not auto-promoted: the wiring to push an evolved variant back into the live catalog is a manual gate by design. Backed by 70+ tests in `wcore-evolve` and 23 in `wcore-dispatch`. [→ docs/wcore-evolve.md](docs/wcore-evolve.md)

## Built to endure

Finishing one task is the easy bar. The hard one is staying up: running unattended for hours, on its own codebase, taking deliberate crashes and injected faults without drifting or corrupting state. Wayland Core is built for that, and we are proving it in the open as an endurance trial.

The mechanics are real code. Your prompt is journaled to a write-ahead log before the model ever sees it (`wcore-agent/src/session.rs`: `append_wal`/`merge_wal`/`recover_from_wal`, with a dedup guard so a restart never replays a message twice). File edits are checkpointed for rollback. Provider calls retry and reconnect mid-stream, with multi-key rotation on supported providers; switch on `[provider_chain]` to wrap them in a circuit breaker with same-provider fallback. And `wcore-replay` lets you reload a session trace and dry-run it through a version-skew guard before you trust it.

<div align="center">

![Resilience under fire — WAL, retry, and checkpoint recovering through deliberate kills](docs/img/diagram-resilience.png)

</div>

**Measured so far** — one continuous 12-hour unattended run:

- 322 maintenance iterations attempted, 229 accepted and committed (~71%), each gated on a real compile + lint pass before it entered history.
- Survived a `SIGKILL` injected mid-build. It restarted and resumed on its own: zero duplicate commits, zero lost commits, clean working tree.
- No degradation across the window. Acceptance held a stable ~68–71% equilibrium; memory, disk, and cache stayed flat.
- Single-digit USD for the full 12 hours, read from the provider's raw usage records — not a self-reported number.
- A separate fault-injection suite kills the process inside every sensitive window of the commit path (before commit, after commit before merge, after merge): **80 of 80 recovered with zero duplicate commits.**

Tune the breaker, or replay a trace, with what ships in the box:

```toml
[provider_chain]
failure_threshold = 3       # consecutive failures before the breaker opens (default 3)
recovery_timeout_secs = 30  # Open -> HalfOpen cooldown (default 30)
# add same-provider fallback models here (cross-provider failover is a follow-up)
```

```text
/replay   # load a session trace, version-skew-guarded dry-run for diagnostics
```

**What we do not claim.** These numbers come from a single 12-hour run; one clean run is not proof of a week, and the continuous week and month are roadmap goals, labeled as goals. The trial harness lives outside this repo — the figures above are reported measurements from that experiment, not something you reproduce from the crates here. `/replay` today means schema load, version-skew guard, and diff; live re-execution against a provider is intentionally out of scope for now. WAL recovery covers the user prompt stream, not arbitrary in-flight tool state. Wayland Core is public beta; behavior may change before 1.0. [→ docs/resilience.md](docs/resilience.md)

## Embedding (JSON-Lines protocol) + ACP interop

Wayland Core is an engine you embed, not just a chat CLI. The same agent core sits behind two host-integration surfaces, so your own app, an external orchestrator, or another agent can drive real engine turns over a wire instead of scraping a prompt.

```bash
wayland-core --json-stream                       # embed over stdin/stdout (NDJSON)
wayland-core acp serve --bind 127.0.0.1:8080     # ACP + REST + A2A on one port
```

**JSON-Lines stream.** Launch with `--json-stream` and the binary drops out of the TUI into a line-delimited loop: one JSON object per line, UTF-8, one process per conversation, multi-turn. The host sends `Message`, `ToolApprove`, `ToolDeny`, `InitHistory`, `SetMode`, `SetConfig`, `AddMcpServer`, `ApprovalResume`. The engine streams back typed events — `ready`, `stream_start`, `text_delta`, `thinking`, `tool_request`, `tool_result`, `stream_end`, `error` — with an honest `retryable` flag on every error. On init it emits one `ready` event advertising its capabilities (tool approval, thinking, effort and effort levels, modes, current mode, MCP). Newer capability flags are serde-skipped when off, so a v0.1.21 host still sees the original seven-field shape it was built against. This is exactly how Wayland Desktop embeds the engine.

**ACP + A2A interop.** `wayland-core acp serve` binds one HTTP listener that fronts three surfaces at once:

- **ACP (Agent Client Protocol)** — JSON-RPC 2.0 at `/sessions`: `session/create`, `session/list`, `session/get`, `session/delete`, and `message/send`, the last an SSE stream of `MessageEvent` frames. Every envelope is `deny_unknown_fields` with `non_exhaustive` enums and eight distinct error codes.
- **REST / OpenAPI** at `/v1/*` — sessions CRUD, `POST /v1/sessions/{id}/prompt` (SSE `text/event-stream`), tools, health, plus an unauthenticated `/openapi.json` and a `/doc` HTML spec viewer for discovery. Every `/v1/*` data route is auth-gated.
- **A2A federation** — three methods (`a2a/handshake`, `a2a/message/send`, `a2a/capabilities`) on the same substrate, so other agents can talk to it directly. Handshake answers `agent_kind = "wayland-core"`.

Approval is enforced on the HTTP path, not just the TUI. A gated tool emits `ApprovalRequired{call, reason}` before it runs, and REST exposes `POST /v1/sessions/{id}/approvals/{call_id}/resolve` to answer it; abandoned approvals auto-resolve as denied after a 300s TTL. `serve` installs the process-global egress policy before it accepts a single connection, auto-generates a 64-char hex API key on first run, stores it in the OS keychain, prints it once to stderr, and verifies it constant-time on every request. Pass `--allow-all-tools` and that key becomes root-equivalent — off by default, and warned in the help text.

The point is one engine, many surfaces: CLI, TUI, JSON-stream, and an embedded HTTP host all driving the same core.

**Honest bounds.** ACP sessions are in-memory and do not survive a restart. A2A is an MVP — three of seven methods are live; the other four (`message/stream`, `task/create`, `task/status`, `task/cancel`) are a tracked follow-up, not shipped. OAuth is scaffolded only; just the API-key and Bearer verifiers are real today. A per-session `system_prompt` is stored but not yet applied to the engine build — the configured prompt is used. Stdio and WebSocket transports exist in `wcore-acp`, but `acp serve` currently binds the HTTP/SSE + REST routers; keep the bind trusted and access-controlled.

[→ docs/json-stream-protocol.md](docs/json-stream-protocol.md)

## Extensibility

- **MCP, both directions.** Connect to many MCP servers concurrently over stdio, SSE, or streamable-HTTP (a wedged server is skipped, not fatal), and inject servers at runtime mid-session. Wayland Core also **runs as an MCP server that advertises and executes its own built-in tools**, so another agent can drive it over MCP.
- **~70 built-in tools.** `Read`, `Write`, `Edit`, `Bash`, `Grep`, `Glob`, `Spawn` are the headline; the catalog also covers git, GitHub, Kubernetes, Postgres, PDF, and more. `Bash` runs network-denied with secrets scrubbed from its environment by default.
- **Skills.** Markdown plus YAML front matter, with path-glob conditional activation, forked context, a per-skill model pin, a per-skill tool allowlist, and shell-expansion directives.
- **Hooks.** Shell or native hooks on `pre_tool_use` / `post_tool_use` / `stop`; a pre-hook can block a tool call.
- **Plugins.** Register tools, hooks, agents, skills, rules, and MCP servers through a stable plugin API.

[→ docs/tools.md](docs/tools.md) · [→ docs/skills.md](docs/skills.md) · [→ docs/mcp.md](docs/mcp.md)

<div align="center">

![Built-in tools, compared — Wayland Core vs OpenClaw, Hermes, opencode, aider](docs/img/compare-tools.png)

</div>

<div align="center">

![Extensibility, compared](docs/img/compare-extensibility.png)

</div>

## Architecture

A workspace of 54 focused crates. Dependencies flow strictly downward; the engine only ever sees provider-neutral types, and format conversion lives inside each provider. The table groups the load-bearing ones by layer.

<div align="center">

![One engine, many surfaces — CLI, TUI, JSON stream, and an embedded host all driving the same core](docs/img/diagram-architecture.png)

</div>

| Layer | Crates | Responsibility |
|-------|--------|----------------|
| Foundation | `wcore-types`, `wcore-compact`, `wcore-pricing` | Provider-neutral data types; context compression; pricing-as-data |
| Core services | `wcore-config`, `wcore-providers`, `wcore-tools`, `wcore-mcp`, `wcore-egress` | Config + ProviderCompat, LLM providers, built-in tools, MCP, the egress chokepoint |
| Safety & limits | `wcore-sandbox`, `wcore-permissions`, `wcore-safety`, `wcore-budget` | OS sandbox, multi-actor ACLs, output validation + PII scrubbing, spend caps |
| Capabilities | `wcore-skills`, `wcore-memory`, `wcore-swarm`, `wcore-browser`, `wcore-cua`, `wcore-evolve`, `wcore-dispatch` | Skills, memory, swarm, browser, computer use, self-evolution, bandit routing |
| Channels & schedule | `wcore-channels` (+ `-registry`, ten `wcore-channel-*`), `wcore-cron` | Omni-channel connectors + cron triggers |
| Interop | `wcore-protocol`, `wcore-acp`, `wcore-replay` | JSON-stream protocol, ACP/A2A server, session replay |
| Engine | `wcore-agent` | Agent loop, sessions, orchestration, the Crucible council, workflows |
| Surface | `wcore-cli` | CLI / TUI / JSON-stream binary |

[→ AGENTS.md](AGENTS.md)

## How it compares

We ran a file-level audit of the open-source agent CLIs and a docs-level orientation against the closed ones. Where we lose, we say so (git auto-commit loops, for instance, belong to opencode and aider).

<div align="center">

![Landscape comparison — Wayland Core vs opencode, aider, Claude Code, Codex CLI](docs/img/compare-capabilities.png)

</div>

Closed-source tools (Claude Code, Codex CLI) are a docs-based orientation, not a code audit.

## Documentation

| Document | Covers |
|----------|--------|
| [Getting Started](docs/getting-started.md) | Install, CLI reference, config and cascading precedence |
| [Providers & Auth](docs/providers.md) | Multi-provider setup, ProviderCompat, profiles |
| [Built-in Tools](docs/tools.md) | The tool catalog and execution flow |
| [Skills](docs/skills.md) | Front matter, shell expansion, conditional activation |
| [MCP Integration](docs/mcp.md) | Transport types, deferred loading, runtime injection |
| [Channels](docs/channels.md) | Messaging-platform connectors, inbound policy, cron triggers |
| [Memory](docs/memory.md) | Partitions, tiers, the dream cycle, cost governance |
| [Self-Evolution](docs/wcore-evolve.md) | The GEPA loop, mutators, scoring, curator hand-off |
| [Advanced](docs/advanced.md) | Sub-agents, hooks, memory, plan mode, compaction |
| [Resilience](docs/resilience.md) | The endurance trial: method, measurements, and honesty bounds |
| [JSON Stream Protocol](docs/json-stream-protocol.md) | Host integration protocol spec |

## Contributing

Issues and PRs welcome. Before a PR: `cargo fmt --all`, `cargo clippy --workspace --all-targets -- -D warnings`, and `cargo nextest run`. Keep changes surgical, and keep provider differences in `ProviderCompat`, never hardcoded. [→ AGENTS.md](AGENTS.md)

## License

[Apache-2.0](LICENSE). Wayland Core is a derivative work; see [NOTICE](NOTICE) for upstream attribution.

<div align="center">
<sub>Part of the Forge Suite · <a href="https://getwayland.com">getwayland.com</a></sub>
</div>
