# Audit: CONFIG / AUTH / ONBOARDING slice

Mockup: `mockups/wayland-tui.html` (Onboarding section, lines 537–580, 1005–1009)
Engine: `crates/wcore-config/` + `crates/wcore-cli/src/main.rs` @ `feat/v0.6.1-hardening`

## Findings table

| # | Mockup element | Engine reality (file:line) | Verdict | Fix needed |
|---|---|---|---|---|
| 1 | 3-step onboarding wizard "Connect → Trust folder → Ready", "Step 1/3" | No wizard exists. CLI is pure flags: `--init-config`, `--config-path`, `--login`, `--logout` (`main.rs:166–172,241–247`). `init_config()` writes a commented TOML template, prints one line, exits (`config.rs:1321–1338`). REPL has only `/quit`/`/exit` (`main.rs:508`). | ASPIRATIONAL-OK | Net-new UI. Ground it on `init_config()` + `OAuthManager::login()` + the doctor probe. Do not present as existing. |
| 2 | Paste key → provider auto-detected by prefix (`sk-ant-` → Anthropic), live-validated vs `/v1/models` in ~2s | No prefix detection and no live key validation anywhere. `sk-ant-` only appears in redaction/PII regexes (`wcore-cua/src/redact/mod.rs:309`, `wcore-safety/src/pii.rs:17`) and config-template comments (`config.rs:1352`). Provider is chosen explicitly via `--provider`/`PROVIDER` env/config `[default] provider` (`main.rs:107`). | WRONG | Engine cannot auto-detect a provider from a key, and never calls `/v1/models`. Either build both (new code) or change the mockup to "select provider, paste key". |
| 3 | Option "Use Ollama — detected at localhost:11434" | Ollama exists as the `wayland-ollama` plugin; default endpoint `http://localhost:11434/api/chat` (`wayland-ollama/src/plugin.rs:44`), overridable via `OLLAMA_BASE_URL` (`main.rs:90`). `--doctor` *probes* for a running Ollama daemon (`doctor/mod.rs:297`). But there is no auto-detect-and-offer in any setup flow. | ASPIRATIONAL-OK | Real capability (plugin + doctor probe). The "detected at localhost:11434" live-probe UX is net-new but grounded. |
| 4 | Option "Skip for now — read-only, `/setup` any time" | No `/setup` command (REPL only handles `/quit`/`/exit`, `main.rs:508`). No read-only mode (see #5). Skipping = running with no key, which just fails at first API call. | WRONG | `/setup` does not exist; "read-only" does not exist. Both need to be built or the option reworded. |
| 5 | "Trust folder" onboarding step + "read-only mode" | No folder-trust / workspace-trust concept. The only "read-only" in the codebase is Plan Mode (restricts agent to read-only *tools*, `plan.rs:5`) and the host-events read-only snapshot flag (`tools.rs:30`) — neither is a trust gate. `--project-dir` just sets where `.wayland-core.toml` loads from (`main.rs:142`); no prompt, no trust ledger. | WRONG | No trust primitive exists. Plan Mode is the closest *real* thing but it is a runtime tool-restriction, not a per-folder trust decision. Build it or drop the step. |
| 6 | Diff: `self.compat.max_tokens_field.as_deref().unwrap_or("max_tokens")` | Exact match. `max_tokens_field: Option<String>` on `ProviderCompat` (`compat.rs:13`); consumed verbatim in `openai.rs:266–270`: `self.compat.max_tokens_field.as_deref().unwrap_or("max_tokens")`. | ACCURATE | None. This is real code, copied correctly. |
| 7 | Plan treats `crates/wcore-config/src/compat.rs` as a NEW file; references `compat/provider.rs` | `compat.rs` already exists — 28 KB, declared `pub mod compat;` in `lib.rs:18`. It is a single flat file, not a `compat/` directory. No `compat/provider.rs`. | WRONG | `compat.rs` exists today; the redesign cannot "create" it. If splitting into `compat/`, that is a refactor of existing code, not new-file work. |
| 8 | Status bar: cwd `~/dev/wayland/engine`, branch `feat/v0.6.1-hardening` | Real cwd and real current branch of this repo. | ACCURATE | None (cosmetic, but correct). |

## Detail — the real CONFIG/AUTH surface

**CLI flags (`main.rs:105–259`).** Config/auth-relevant: `--provider/-p`, `--api-key/-k`,
`--base-url/-b`, `--model/-m`, `--max-tokens`, `--max-turns`, `--system-prompt`,
`--profile`, `--project-dir`, `--init-config`, `--config-path`, `--login`, `--logout`,
`--doctor`. There is **no `--setup`, no wizard, no interactive prompt**. `--init-config`
short-circuits before the agent runs (`main.rs:358`) and just drops a template file.

**Auth (`auth.rs`).** A *single* auth mechanism: **OAuth 2.0 device-authorization flow**
for Claude.ai subscriber accounts (`OAuthManager::login()`, `auth.rs:107–197`). It posts to
`{auth_url}/device/code`, prints a verification URI + user code to stderr, polls
`{token_url}` until authorized, and saves tokens to `<config_dir>/wayland-core/auth.json`
(atomic write, `auth.rs:269`). Endpoints default to `https://claude.ai/oauth*` (`auth.rs:75–85`).
This is **Anthropic-only** — there is no OpenAI/Google/Bedrock OAuth. Other providers
authenticate purely by **API key** via `--api-key`, env (`API_KEY`, `ANTHROPIC_API_KEY`,
`OPENAI_API_KEY`), or `[providers.<name>] api_key` in TOML. Bedrock uses AWS SigV4,
Vertex uses GCP ADC (`config.rs:1374–1386`). **There is no `/v1/models` round-trip and
no key-prefix sniffing in the auth path.**

**ProviderCompat (`compat.rs:10–95`).** Real, with ~24 `Option<T>` fields. `max_tokens_field`
is field #1 (line 13). Presets: `anthropic_defaults()` (line 99), and an `openai_defaults()`
that sets `max_tokens_field: Some("max_tokens")` (line 196). User overrides merge field-by-field
(`merge`, line ~231: `user.max_tokens_field.or(defaults.max_tokens_field)`). The
`.as_deref().unwrap_or(...)` consumption pattern in the mockup diff is exactly how
`openai.rs` reads it. The OpenAI-official override value is `"max_completion_tokens"`
(`config.rs:1368`, `compat.rs:418`). The mockup diff (#6) is faithful.

**Config files.** Global: `<dirs::config_dir()>/wayland-core/config.toml`
(`app_config_dir()` `config.rs:965`, `global_config_path()` `config.rs:971`). Project:
`./.wayland-core.toml` (`project_config_path()` `config.rs:977`). Project overrides global;
`[profiles.<name>]` blocks support `--profile` with parent-inheritance + cycle detection
(`resolve_profile`, `config.rs:1234–1254`). `init_config()` writes `0o600`-permissioned
template, refuses to overwrite (`config.rs:1321–1338`).

## What the onboarding mockup should actually depict

There is no wizard today, so the honest framing is: **this is a net-new redesign feature**,
not a render of current behavior. It can be built, but only on these real primitives:

1. **First-run trigger.** Detect "no global `config.toml` and no `auth.json`" — both paths
   are already known (`global_config_path()`, `OAuthManager::has_credentials()` `auth.rs:257`).
   That's a legitimate "first run" signal to launch a setup TUI.

2. **Connect step — be honest about the two real paths.** (a) Anthropic via the existing
   **OAuth device flow** (`--login` today) — the wizard would surface the verification URI +
   user code instead of printing to stderr. (b) Any provider via **API key** written to
   `[providers.<name>]` in `config.toml`. The wizard must **ask which provider** — there is no
   prefix auto-detect. If the redesign wants `sk-ant-` → Anthropic detection and a
   `/v1/models` validation ping, that is **new code to specify and build**; flag it as such,
   don't claim the engine does it.

3. **Ollama option.** Grounded: the `wayland-ollama` plugin + the `--doctor` Ollama probe
   (`doctor/mod.rs:297`) already exist. A "detected at localhost:11434" line is a reasonable
   net-new probe reusing that logic.

4. **Drop or redefine "Trust folder" and "read-only".** Neither exists. Options: (a) cut the
   step and make onboarding 2-step ("Connect → Ready"); or (b) scope it as a new feature and
   build a real per-folder trust ledger. The nearest existing concept is **Plan Mode**
   (`plan.rs`), a runtime read-only-tools restriction — usable as the "read-only" substrate,
   but it is not folder-scoped trust.

5. **`/setup` re-entry.** No slash-command system exists (REPL handles only `/quit`/`/exit`).
   Either build a slash-command dispatcher, or point users at the real re-entry: rerun with
   `--init-config` / `--login`.

**Bottom line:** the diff (#6) and status bar (#8) are accurate. Everything else in the
onboarding screen is aspirational. The two genuinely-grounded pieces are OAuth device flow
(Anthropic) and the Ollama probe; prefix auto-detection, `/v1/models` validation, folder
trust, read-only mode, and `/setup` are all WRONG as "engine reality" and must be labeled
as new-build scope. The plan's premise that `compat.rs` is a new file is also WRONG — it
exists and is 28 KB.
