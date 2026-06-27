# Changelog

## [0.12.13](https://github.com/FerroxLabs/wayland-core/compare/v0.12.12...v0.12.13) (2026-06-27)

A reliability-focused release: a new **capability-first tools gate** so models that can't do function calling degrade gracefully instead of failing the turn, a major Windows sandbox fix, and a round of audited provider- and config-layer hardening.

### Highlights

- **Tool-incapable models just work now — across local and cloud backends.** Point Wayland Core at a model that doesn't support function calling and the turn no longer dies on a raw provider error. Ollama models are detected up front via `/api/show` and have their `tools` array dropped before the request is even sent. Any backend that rejects tools with a `400` — llama.cpp started without `--jinja` (`tools param requires --jinja flag`), or an Ollama model that 400s with `does not support tools` — is caught, retried without tools, and **remembered**, so every later turn for that model skips tools pre-emptively. Tool-incapable Bedrock models (DeepSeek-R1 reasoning, Stability image, Titan/Cohere embedding) are name-gated the same way. Tool-*capable* models are unaffected — they keep their tools and call them exactly as before. ([#389](https://github.com/FerroxLabs/wayland-core/issues/389))
- **The Windows sandbox runs real subprocesses again.** The AppContainer backend no longer caps active processes too aggressively (`ActiveProcessLimit` raised to 512), resolves the launch shell correctly, and emits clearer diagnostics when a shell can't be found — so multi-step tool use works under the sandbox on Windows. ([#321](https://github.com/FerroxLabs/wayland-core/issues/321), [#322](https://github.com/FerroxLabs/wayland-core/issues/322), [#323](https://github.com/FerroxLabs/wayland-core/issues/323), [#324](https://github.com/FerroxLabs/wayland-core/issues/324))

### Provider reliability

- **Anthropic errors are classified correctly.** Non-credit Anthropic API errors are no longer misread as out-of-credit / billing failures, so genuine transient errors surface instead of a misleading "purchase credits" signal. ([#329](https://github.com/FerroxLabs/wayland-core/issues/329))
- **Flux reasoning summaries render as thinking.** A FluxRouter `reasoning_summary` is now decoded into a per-turn thinking subject, so reasoning summaries appear as proper thinking content. ([#318](https://github.com/FerroxLabs/wayland-core/issues/318))

### Configuration & hygiene

- **Config surface tightened.** `env_passthrough` is now wired through, unknown configuration keys produce a warning (via `serde_ignored`) instead of being silently dropped, and the sandbox configuration surface is exposed as a toggle. ([#325](https://github.com/FerroxLabs/wayland-core/issues/325), [#326](https://github.com/FerroxLabs/wayland-core/issues/326), [#327](https://github.com/FerroxLabs/wayland-core/issues/327))

## [0.12.12](https://github.com/FerroxLabs/wayland-core/compare/v0.12.11...v0.12.12) (2026-06-27)

### Crucible reliability & cost accuracy

This release hardens the Crucible (Mixture-of-Providers) council and the pricing engine behind it — every fix here was found by putting Crucible through a live, cross-vendor proof run and watching where it strained.

- **Bring-your-own pricing catalogs now load.** A custom `WAYLAND_PRICING_PATH` catalog parses reliably, so you can price any model the bundled catalog doesn't yet cover — and Crucible can certify a real spend ceiling against it.
- **Accurate Gemini pricing.** Gemini's live API slugs (e.g. `gemini-2.5-flash`) now resolve to the catalog correctly, so Gemini members are priced — and counted — when Crucible assembles a cost-diverse council.
- **Broader Opus support in councils.** Anthropic's Opus 4.x models, which decline an explicit sampling temperature, are now handled cleanly both as proposers and as the fusing judge.

Backed by new regression tests across `wcore-pricing` and `wcore-providers`.

## [0.12.11](https://github.com/FerroxLabs/wayland-core/compare/v0.12.10...v0.12.11) (2026-06-27)

This release is headlined by **Crucible**, our cross-provider Mixture-of-Providers council — wayland-core's answer to single-model ceilings — folded together with two audited reliability and security fixes.

### ✨ Headliner — Crucible (Mixture-of-Providers)

* **crucible:** a cross-provider council you run with `wayland-core crucible "<task>"`. N proposers, **each pinned to a different LLM provider**, work the task in parallel; a fenced, read-only **aggregator** fuses their answers into one. Three ways to run it: `--auto` gates convening behind a cheap difficulty classifier (trivial tasks get a single direct call, high-stakes tasks convene the full council); `--advisor` injects the fused synthesis into the normal trusted agent loop as private guidance (the agent then reasons, acts, and uses tools on it); `--terminal` prints the fused answer and stops. Includes per-tier proposer/aggregator temperatures, provenance-fenced injection containment, per-proposer **and** global soft deadlines with quorum, and `[crucible]` budget/daily-cap guards. Tri-model cross-audited; 151 dedicated tests. ([#91](https://github.com/FerroxLabs/wayland-core/pull/91))

### Enhancements

* **tools:** `image_generate` and `text_to_speech` now follow your active provider instead of assuming a single hardcoded host. FluxRouter and native OpenAI sessions route to the correct endpoint with the correct key (with proper `/v1` API-root resolution), gracefully fall back to FAL / Gemini Imagen / Hugging Face FLUX via their env keys, and **fail closed** on a base URL carrying embedded credentials. ([#310](https://github.com/FerroxLabs/wayland/issues/310))

### Security & Hardening

* **mcp:** MCP tool curation is now driven purely by **BM25 relevance + recency**. Removed a name-based "rescue" boost that a third-party MCP server could exploit by naming a tool like a built-in to jump the curation budget — closing a budget-hijack vector with no impact on built-in tools (which are never curated). ([#89](https://github.com/FerroxLabs/wayland/issues/89))

### Validation

* Full cross-platform gate green — **9,411 tests** across Linux, macOS, and Windows.

## [0.12.10](https://github.com/FerroxLabs/wayland-core/compare/v0.12.9...v0.12.10) (2026-06-27)


### Features

* **mcp:** provider-aware hard cap on total tool count + real MCP server provenance + BM25 relevance curation — caps the outbound tool array to the model's limit (OpenAI 128), fixing API-400 overflow with large MCP servers (Google Workspace, etc.); fixes uniquely-named MCP tools being misclassified as built-ins ([#86](https://github.com/FerroxLabs/wayland-core/issues/86), [#344](https://github.com/FerroxLabs/wayland-core/issues/344)/[#359](https://github.com/FerroxLabs/wayland-core/issues/359)) ([#87](https://github.com/FerroxLabs/wayland-core/issues/87))


### Bug Fixes

* **deps:** bump pdf-extract 0.12 → lopdf 0.42 ([RUSTSEC-2026-0187](https://rustsec.org/advisories/RUSTSEC-2026-0187)) ([#87](https://github.com/FerroxLabs/wayland-core/issues/87))
* **web-fetch:** wall-clock timeout message now contains "timed out" (de-flake) ([#87](https://github.com/FerroxLabs/wayland-core/issues/87))

## [0.12.9](https://github.com/FerroxLabs/wayland-core/compare/v0.12.8...v0.12.9) (2026-06-25)


### Bug Fixes

* OpenAI tool-name sanitization ([#297](https://github.com/FerroxLabs/wayland-core/issues/297)) + WSL canonicalize off-reactor ([#287](https://github.com/FerroxLabs/wayland-core/issues/287)) ([#84](https://github.com/FerroxLabs/wayland-core/issues/84)) ([af69bdc](https://github.com/FerroxLabs/wayland-core/commit/af69bdc046bef94671426a20a8a1fb7327c91d30))

## [0.12.8](https://github.com/FerroxLabs/wayland-core/compare/v0.12.7...v0.12.8) (2026-06-24)


### Features

* **providers:** add Sakana AI (Fugu) — OpenAI-compatible endpoint ([#82](https://github.com/FerroxLabs/wayland-core/issues/82)) ([a531f22](https://github.com/FerroxLabs/wayland-core/commit/a531f220d9ffbc089815b9dfb78478ff6affa4bd))

## [0.12.7](https://github.com/FerroxLabs/wayland-core/compare/v0.12.6...v0.12.7) (2026-06-23)


### Features

* **#255:** active-window kernel — context % vs the post-swap active model ([#74](https://github.com/FerroxLabs/wayland-core/issues/74)) ([7d22c84](https://github.com/FerroxLabs/wayland-core/commit/7d22c847718e48871bde90d666c906de350aecb8))
* **#279:** JSON-stream observability — active-window %, agent-run correlation, structured traces ([#76](https://github.com/FerroxLabs/wayland-core/issues/76)) ([3b9b070](https://github.com/FerroxLabs/wayland-core/commit/3b9b07006f399af3ccd9689166d028d94f2de003))
* **#280:** smart auto-compaction at active-window threshold (default-off, Flux-aware, memory handoff) ([#78](https://github.com/FerroxLabs/wayland-core/issues/78)) ([508d9e8](https://github.com/FerroxLabs/wayland-core/commit/508d9e8e790771f23f82b8577edecfd511624096))
* **#282:** Flux context-routing contract — client side V1 ([#77](https://github.com/FerroxLabs/wayland-core/issues/77)) ([508af81](https://github.com/FerroxLabs/wayland-core/commit/508af81c533b36e0cdedc0e48f55e6f695c70e1d))
* isolated profiles — CLI-isolation slice (Phase 0 + 1 + 3A + 2) ([#70](https://github.com/FerroxLabs/wayland-core/issues/70)) ([3177b17](https://github.com/FerroxLabs/wayland-core/commit/3177b1763d0334ba03057992d689904b9f810554))


### Bug Fixes

* **#282:** tolerate live Flux context-overflow shapes (found by live E2E) ([#79](https://github.com/FerroxLabs/wayland-core/issues/79)) ([c5aadd6](https://github.com/FerroxLabs/wayland-core/commit/c5aadd636505fb008f5dfa735ff9b09d2b0fe18c))
* **#285:** never emit orphaned tool_result during compaction (DeepSeek 400) ([#75](https://github.com/FerroxLabs/wayland-core/issues/75)) ([5f3aaf7](https://github.com/FerroxLabs/wayland-core/commit/5f3aaf78d01d9bab3fbf80766e97761f024eb4df))
* **#293:** authenticate openai-chatgpt from ~/.codex/auth.json ([#80](https://github.com/FerroxLabs/wayland-core/issues/80)) ([7f0c7cc](https://github.com/FerroxLabs/wayland-core/commit/7f0c7cc1559526f5a5814fd72a8a099500218699))
* OpenAI image default (gpt-image-1) + DeepSeek v4-flash 1M context ([#265](https://github.com/FerroxLabs/wayland-core/issues/265), [#255](https://github.com/FerroxLabs/wayland-core/issues/255)) ([#69](https://github.com/FerroxLabs/wayland-core/issues/69)) ([30dad57](https://github.com/FerroxLabs/wayland-core/commit/30dad572cb15b2ff3cdb0d7f2b936525d7e5ac06))
* **windows:** 4 Windows-only failures ([#257](https://github.com/FerroxLabs/wayland-core/issues/257) CRLF edit, [#262](https://github.com/FerroxLabs/wayland-core/issues/262)/[#263](https://github.com/FerroxLabs/wayland-core/issues/263) MCP stdio quoting, [#267](https://github.com/FerroxLabs/wayland-core/issues/267) sandbox \\?\ path) ([#72](https://github.com/FerroxLabs/wayland-core/issues/72)) ([d7ccbef](https://github.com/FerroxLabs/wayland-core/commit/d7ccbef78194fbbb7ad5ed7e87c7f0afb5370f0f))

## [0.12.6](https://github.com/FerroxLabs/wayland-core/compare/v0.12.5...v0.12.6) (2026-06-22)


### Features

* ChatGPT-sub model filtering ([#158](https://github.com/FerroxLabs/wayland-core/issues/158)) + MiniMax cost catalog ([#240](https://github.com/FerroxLabs/wayland-core/issues/240)) ([#68](https://github.com/FerroxLabs/wayland-core/issues/68)) ([f807397](https://github.com/FerroxLabs/wayland-core/commit/f807397dab29b9eea1fe18a9ef0f80e9ead3edfd))
* FluxRouter capabilities (image/fetch/web_search) + per-model max_tokens + reliability fixes ([#66](https://github.com/FerroxLabs/wayland-core/issues/66)) ([aefdd39](https://github.com/FerroxLabs/wayland-core/commit/aefdd3993c47c0a0ba6e6c7f16fbaf917cc325cd))


### Performance Improvements

* **token-spend:** wire routing tier, cheap+accurate compaction, bound retries, cache hygiene ([#65](https://github.com/FerroxLabs/wayland-core/issues/65)) ([2c70b7b](https://github.com/FerroxLabs/wayland-core/commit/2c70b7b828eb5f4defb4f60f29492d9c3fedf129))

## [0.12.5](https://github.com/FerroxLabs/wayland-core/compare/v0.12.4...v0.12.5) (2026-06-21)


### Features

* **sandbox:** WorkspacePolicy + OS secret-read-deny + Landlock Option A ([#59](https://github.com/FerroxLabs/wayland-core/issues/59)) ([dfa5aa2](https://github.com/FerroxLabs/wayland-core/commit/dfa5aa29c9d4f2a7cdf363f701339ed5147e37ad))


### Bug Fixes

* **#200:** unblock native Gemini egress + stop silent finish_reason=error turns ([#60](https://github.com/FerroxLabs/wayland-core/issues/60)) ([8d95578](https://github.com/FerroxLabs/wayland-core/commit/8d955782faf43d8c473606537337db0384ad0e9e))
* **agent,tools:** close two real Windows bugs (unbounded project-context walk + glob sandbox bypass) ([#64](https://github.com/FerroxLabs/wayland-core/issues/64)) ([fea2c52](https://github.com/FerroxLabs/wayland-core/commit/fea2c52f6069f1e32f1bfbcb7640818a7820b397))
* **cli:** surface a clear, Ollama-aware reason on init failure instead of bare exit 1 ([#186](https://github.com/FerroxLabs/wayland-core/issues/186)) ([#61](https://github.com/FerroxLabs/wayland-core/issues/61)) ([b37b3d1](https://github.com/FerroxLabs/wayland-core/commit/b37b3d12663fdf45b472933bf5eb12f0164fc8db))
* **shell:** accept .exe and absolute-path Windows shell selectors ([#197](https://github.com/FerroxLabs/wayland-core/issues/197)) ([#62](https://github.com/FerroxLabs/wayland-core/issues/62)) ([9b332e7](https://github.com/FerroxLabs/wayland-core/commit/9b332e7eedc9bf4ec9141dbbdceaff6b01a3873b))

## [0.12.4](https://github.com/FerroxLabs/wayland-core/compare/v0.12.3...v0.12.4) (2026-06-20)


### Bug Fixes

* **skills:** hide unreviewed auto-drafted skills from the model catalog ([#56](https://github.com/FerroxLabs/wayland-core/issues/56)) ([a2c0de4](https://github.com/FerroxLabs/wayland-core/commit/a2c0de415e8ce51ee8f0232b8590119276d6e152))
* **skills:** keep the hello test fixture out of the shipped catalog ([#55](https://github.com/FerroxLabs/wayland-core/issues/55)) ([35d334f](https://github.com/FerroxLabs/wayland-core/commit/35d334f7f10b7ca215fb1c674fbb7c64e654f507))

## [0.12.3](https://github.com/FerroxLabs/wayland-core/compare/v0.12.2...v0.12.3) (2026-06-19)


### Features

* **tools:** PowerShell shell for the Bash tool on Windows — selectable via the `WAYLAND_BASH_SHELL` env var or the `[tools] windows_shell` config key (`powershell`/`pwsh`); precedence env > config > default `cmd`, scoped to the Bash tool ([#45](https://github.com/FerroxLabs/wayland-core/issues/45)) ([130dc3d](https://github.com/FerroxLabs/wayland-core/commit/130dc3da1d4720ac407423125f058aacb6c2390d))


### Bug Fixes

* **egress:** allowlist NVIDIA NIM, Cerebras, MiniMax-failover & Qwen hosts ([#48](https://github.com/FerroxLabs/wayland-core/issues/48)) ([a68f2d9](https://github.com/FerroxLabs/wayland-core/commit/a68f2d917f8c950004a9d92ba57cce9d759cbe4d))
* **oauth:** stop advertising a non-existent `wayland auth login grok` command ([#47](https://github.com/FerroxLabs/wayland-core/issues/47)) ([42e16ec](https://github.com/FerroxLabs/wayland-core/commit/42e16ec5009883a1cff42478f2d347ac4fee7a13))
* **providers:** strip empty/missing tool_call_id before sending (DeepSeek 400 guard) ([#50](https://github.com/FerroxLabs/wayland-core/issues/50)) ([c97424d](https://github.com/FerroxLabs/wayland-core/commit/c97424d463f5e976c1e2863db65cebaf74b0a6a7))


### Documentation

* refresh across the board for 0.12.x ([#46](https://github.com/FerroxLabs/wayland-core/issues/46)) ([273c764](https://github.com/FerroxLabs/wayland-core/commit/273c764af7a936b2dc8c73beaf82a310df55b7a2))


### Miscellaneous Chores

* release 0.12.3 ([cd03533](https://github.com/FerroxLabs/wayland-core/commit/cd03533fb210d9cf7cb5727407bfbd211ff5a4b4))

## [0.12.2](https://github.com/FerroxLabs/wayland-core/compare/v0.12.1...v0.12.2) (2026-06-18)


### Bug Fixes

* **providers:** provider auth robustness — Grok OAuth, region failover, auth errors ([#42](https://github.com/FerroxLabs/wayland-core/issues/42)) ([4dfc566](https://github.com/FerroxLabs/wayland-core/commit/4dfc566af50b6a233f4543e837f84efa5ee8490a))


### Miscellaneous Chores

* release 0.12.2 ([0323931](https://github.com/FerroxLabs/wayland-core/commit/03239313f4c02ec36f615cf5bcae7bf3b0590435))

## [0.12.1](https://github.com/FerroxLabs/wayland-core/compare/v0.12.0...v0.12.1) (2026-06-18)

Stable release rolling up everything from the `0.12.1-rc.1` and `0.12.1-rc.2`
prereleases (full per-commit detail in the sections below).

### Highlights

* **Sign in with ChatGPT** — OpenAI Codex OAuth provider with rotating-refresh token manager, device-code login for headless/remote, and token import from the Codex CLI.
* **MiniMax provider** — via the Anthropic-compatible endpoint, visible in the provider/model pickers.
* **Forge zero-config MCP discovery** — one-command `/mcp connect` to a trusted loopback MCP server, scoped-token grant with `${cred:KEY}` headers (token never lands in `config.toml`), opt-in `allow_local`, and a selectable DISCOVERED row in `/doctor`.
* **Config cockpit** — paste-to-connect with live key fingerprinting + a validation ladder, an Essentials/Advanced settings surface, collection editors (tools/egress/failover), config-posture health and self-configure discovery in `/doctor`, a redacted `/effective` config preview, and channel-integration visibility.
* **Live model discovery** — Bedrock (`ListFoundationModels`), Gemini, and a connected-provider catalog refresh, backed by a per-provider 24h disk cache.
* **TUI** — arrow-key cross-provider `/model` and `/provider` pickers, the command palette on `/` from any surface, connection-aware provider listing.
* **Security & stability** — a 42-defect deep-sweep remediation: closed a Forge-MCP token-exfil SSRF, a Glob sandbox bypass, unbounded reads across MCP/Matrix/ACP, a provider key-pool poison DoS, skill-arg shell injection, and MCP header secret leaks; credentials now default to keyring with plaintext fallback (F16).
* **Core fixes** — Windows MCP stdio launch (#164) and the Anthropic unrecoverable-conversation `thinking.signature` 400 (#161); Flux Router reachable out of the box under the egress guard.

### Build System

* **release:** promote 0.12.1 stable ([d50bfbb](https://github.com/FerroxLabs/wayland-core/commit/d50bfbb1f19d173d4fb56350d8ae633d583e7686))

## [0.12.1-rc.2](https://github.com/FerroxLabs/wayland-core/compare/v0.12.1-rc.1...v0.12.1-rc.2) (2026-06-18)


### Features

* **providers:** add MiniMax provider via Anthropic-compatible endpoint ([703ba14](https://github.com/FerroxLabs/wayland-core/commit/703ba14ce25f5b23a19a06cea00aebdb16631bc4))


### Bug Fixes

* **audit:** 19 low/medium defects — browser, sandbox, channels, tools, TUI ([8c589ad](https://github.com/FerroxLabs/wayland-core/commit/8c589ad36be0e4e8605ca1e49c770a52ce6f3385))
* **audit:** 7 high-severity defects — sandbox, provider protocol, unbounded reads ([8273b2a](https://github.com/FerroxLabs/wayland-core/commit/8273b2ac1e56937e816101c45415954a6d4ea6b6))
* **audit:** provider resilience + egress/secret hygiene (8 fixes) ([0e893d9](https://github.com/FerroxLabs/wayland-core/commit/0e893d99f38b623a4deaa65ea27d3c51c424c8eb))
* **config:** default credentials to keyring with plaintext fallback (F16) ([6c57160](https://github.com/FerroxLabs/wayland-core/commit/6c5716080da4429f32a0ccfc9acd0399cfe6bd3f))
* **core:** Windows MCP stdio launch ([#164](https://github.com/FerroxLabs/wayland-core/issues/164)) + Anthropic unrecoverable-conversation ([#161](https://github.com/FerroxLabs/wayland-core/issues/161)) ([38b85e6](https://github.com/FerroxLabs/wayland-core/commit/38b85e6fb6895100e24218366586b08da6dd62d4))
* **egress:** allowlist Flux Router out of the box + accept full-host entries ([1fa6407](https://github.com/FerroxLabs/wayland-core/commit/1fa6407e907227e7c09b7431e968dbd3920e95d0))
* **forge-mcp:** close token-exfil SSRF + 4 reliability defects in discovery flow ([bd2f40d](https://github.com/FerroxLabs/wayland-core/commit/bd2f40d23aa98d64aff2406f5e7d6b8b45a304ba))
* **mcp:** don't caret-escape the program name in Windows stdio launch ([371f619](https://github.com/FerroxLabs/wayland-core/commit/371f619ee47f1c9beb8d4b984c6f8acc979ce132))
* **providers:** drop unsigned thinking blocks when building Anthropic messages ([cdd0968](https://github.com/FerroxLabs/wayland-core/commit/cdd0968dc66acf53471748ebdd40c460b2630b3c))
* **providers:** make MiniMax visible in pickers + bound tool-input accumulator ([e8ac0f2](https://github.com/FerroxLabs/wayland-core/commit/e8ac0f29642e75a97143ec73d9172cb185f5eb1a))


### Build System

* **release:** prepare 0.12.1-rc.2 prerelease ([93975b7](https://github.com/FerroxLabs/wayland-core/commit/93975b72dfa485896e336181dabb85d858d052a6))

## [0.12.1-rc.1](https://github.com/FerroxLabs/wayland-core/compare/v0.12.0...v0.12.1-rc.1) (2026-06-17)


### Features

* **agent:** allow chatgpt.com egress when the chatgpt provider is active ([b3372ac](https://github.com/FerroxLabs/wayland-core/commit/b3372ac8af6b639934b293e0915e21d0c604aebb))
* **agent:** wire openai-chatgpt provider with oauth bearer source ([18a50d6](https://github.com/FerroxLabs/wayland-core/commit/18a50d626b45f8bc78ef729f6836732193f9a971))
* **channels,tui:** surface channel integrations in /doctor + fix F-019 (S10 v1) ([6958c1c](https://github.com/FerroxLabs/wayland-core/commit/6958c1cfbb11e648166af0571c3b42772339584f))
* **cli:** wayland auth login/logout/status for chatgpt ([060dc45](https://github.com/FerroxLabs/wayland-core/commit/060dc4533e6df3781a0fefb8021c31500fa5ecd8))
* **config,tui:** redacted effective-config preview (S9 v1) ([ff30d20](https://github.com/FerroxLabs/wayland-core/commit/ff30d2051303c85cf1019951b59cfccc7cc8287b))
* **config:** chatgpt_defaults compat preset ([8fac871](https://github.com/FerroxLabs/wayland-core/commit/8fac87162af5dd40c9f26c0a7b2196d1590aca55))
* **config:** config cockpit — paste-to-connect, editors, /doctor health, /effective, channels, discovery ([8fe5559](https://github.com/FerroxLabs/wayland-core/commit/8fe5559f04131ea02a0ffba23402f5a36a76f6df))
* **config:** connected_providers credential helper ([4cffba9](https://github.com/FerroxLabs/wayland-core/commit/4cffba9030a56ad6d7c4fdedf08bf80a5060414c))
* **config:** openai-chatgpt provider type + parsing ([5709f87](https://github.com/FerroxLabs/wayland-core/commit/5709f87ae5de3e1633b4f6cf6141e9213a70627d))
* **config:** read the Forge local-MCP discovery file (Slice 3) ([1014e21](https://github.com/FerroxLabs/wayland-core/commit/1014e212eab7bf472f4ac38c02fe9939c2116cc4))
* **mcp:** /mcp connect — one-command zero-config Forge MCP connect (Slice 3, Piece 3) ([17973e6](https://github.com/FerroxLabs/wayland-core/commit/17973e6bbae98189aeefacd4bdc798e55bbf8b3a))
* **mcp:** DISCOVERED row-to-connect + boot-hero Forge line (Slice 3b polish) ([509fd69](https://github.com/FerroxLabs/wayland-core/commit/509fd69a9d3e14ca5211cfbe04b4d559f7c92db8))
* **mcp:** Forge connect flow — ${cred:KEY} headers + live token grant (Slice 3) ([3f66b9f](https://github.com/FerroxLabs/wayland-core/commit/3f66b9f0457bf11c5f66fd9519c016639c6a8952))
* **mcp:** Forge connect polish — selectable DISCOVERED row + boot-hero line (Slice 3b) ([d19af5b](https://github.com/FerroxLabs/wayland-core/commit/d19af5bf85dc1271dd736a53f7e5f8b3701c1289))
* **mcp:** Forge loopback grant client — liveness probe + scoped token (Slice 3) ([df9d1c9](https://github.com/FerroxLabs/wayland-core/commit/df9d1c9ba8bc4e8f08fb1028cbc0dcd7a246e84a))
* **mcp:** Forge zero-config local-MCP discovery — keystone + reader + grant client + connect flow (Slice 3, headless) ([106b869](https://github.com/FerroxLabs/wayland-core/commit/106b8696412d04ca6f53ded3baab453b5de21f66))
* **mcp:** opt-in allow_local to connect trusted loopback MCP servers ([68b0a6b](https://github.com/FerroxLabs/wayland-core/commit/68b0a6ba4902aea9fcfc578e655fa92ebda38939))
* **oauth:** add ChatGPT device-code login (headless/remote path) ([2a6a4e6](https://github.com/FerroxLabs/wayland-core/commit/2a6a4e69118b1af2d3f06dc98d5613f6608f4fee))
* **oauth:** chatgpt token manager with rotating refresh, JWT account-id decode, and flow descriptor ([9a1b5c1](https://github.com/FerroxLabs/wayland-core/commit/9a1b5c156061515b12bab85da2cba5ecedb4b6e1))
* **oauth:** extra authorize params, configurable redirect host/path with dual-stack loopback bind, id_token capture ([765c11a](https://github.com/FerroxLabs/wayland-core/commit/765c11adb9137c28541dda88529a13fdd596dc28))
* **oauth:** import chatgpt tokens from codex cli ([630688d](https://github.com/FerroxLabs/wayland-core/commit/630688d051a0e6302829efa5edb2821847efefd8))
* **providers:** add key fingerprinting for paste-to-detect config ([e71d8ca](https://github.com/FerroxLabs/wayland-core/commit/e71d8ca1d63a98c0c5890481eae9f7a00053686b))
* **providers:** add live key-validation ladder for paste-to-detect ([c576df9](https://github.com/FerroxLabs/wayland-core/commit/c576df9d6104ec3fc53fb57bfe8fb035d16fa82d))
* **providers:** live Bedrock model discovery via ListFoundationModels ([27a25dc](https://github.com/FerroxLabs/wayland-core/commit/27a25dcb0e533eaab1a67ca6bc79224a626b7ff6))
* **providers:** live Gemini model discovery ([ed2126e](https://github.com/FerroxLabs/wayland-core/commit/ed2126e6410fa39f26c575e86308dca5c1119f98))
* **providers:** make runtime provider construction OAuth-aware for openai-chatgpt ([3e067c1](https://github.com/FerroxLabs/wayland-core/commit/3e067c1a414a37a9d4df70c3d44ecb7ca176e257))
* **providers:** ModelCatalog.refresh_connected live discovery service ([0bc02bc](https://github.com/FerroxLabs/wayland-core/commit/0bc02bce82c4c1529f36fcd50138050226b9c237))
* **providers:** openai-chatgpt provider over async oauth bearer source ([c19a795](https://github.com/FerroxLabs/wayland-core/commit/c19a795fde0dfa833e6463f7df66d3816fd465d6))
* **providers:** orchestrate paste-to-detect (fingerprint + validate) ([804373e](https://github.com/FerroxLabs/wayland-core/commit/804373ef44a94af336bc1f3ebca8174cc871f14e))
* **providers:** per-provider model-list disk cache (24h TTL) ([785704e](https://github.com/FerroxLabs/wayland-core/commit/785704ec5d8dbf3d854712187ca7d3ec7975ec5e))
* Sign in with ChatGPT (OpenAI Codex OAuth) ([5ccc0fc](https://github.com/FerroxLabs/wayland-core/commit/5ccc0fcc48ecf1ccc7203277375c853069cf08c8))
* **tui:** /model picker reads live cached models + refreshes on open ([f94e2c0](https://github.com/FerroxLabs/wayland-core/commit/f94e2c02561b6b9812b56ff3faede7547394d9f6))
* **tui:** Advanced config tier — observability/storage/security editors (S6) ([94dc918](https://github.com/FerroxLabs/wayland-core/commit/94dc9182c22de94cf9bfe589f9ccce5dec2cc447))
* **tui:** arrow-key /model and /provider pickers (cross-provider) ([4b46606](https://github.com/FerroxLabs/wayland-core/commit/4b466061e4073a5a8443948cb512086998ff844a))
* **tui:** boot-screen provider discovery + Tab always switches tabs (FIX-5, FIX-7) ([b7f03d9](https://github.com/FerroxLabs/wayland-core/commit/b7f03d906b011f0cc12cf2118a6abe109c18fac8))
* **tui:** collection list editors — tools/egress/failover (S7) ([299cdb7](https://github.com/FerroxLabs/wayland-core/commit/299cdb7432eddcf4162115bcd859f60473a8f0e1))
* **tui:** config-posture health section in /doctor (S8) ([4f1cb34](https://github.com/FerroxLabs/wayland-core/commit/4f1cb345fb4ab0b74710d823ab09a24620caf07d))
* **tui:** Essentials config home — Tools + Wallet rows, posture + health/cost (S5) ([fbaa431](https://github.com/FerroxLabs/wayland-core/commit/fbaa431d31beed947aad16869b511480323bf127))
* **tui:** make /provider picker connection-aware ([130bc72](https://github.com/FerroxLabs/wayland-core/commit/130bc7288d8c9522bae46b34a16a1ed98a18ca9e))
* **tui:** open the command palette with / from any surface ([2f21d06](https://github.com/FerroxLabs/wayland-core/commit/2f21d0688a71e0e956bc3d108a9bf6a9ef4f6fad))
* **tui:** paste-to-connect door in the Config Providers tier (FIX-3) ([e16f293](https://github.com/FerroxLabs/wayland-core/commit/e16f293abb407d7dac1d8a21a62159c9dd14d22f))
* **tui:** paste-to-detect modal state machine + view-model (S4a) ([6cb6e25](https://github.com/FerroxLabs/wayland-core/commit/6cb6e250425ee521177f88aeb3ad695bed628187))
* **tui:** self-configure discovery section in /doctor (S11 v1) ([f01c9f9](https://github.com/FerroxLabs/wayland-core/commit/f01c9f940b1f8448bc054f10475df98e3feeda94))
* **tui:** wire the paste-to-detect /connect overlay (S4b) ([7b75549](https://github.com/FerroxLabs/wayland-core/commit/7b75549b8c2120c247dc6940cd5a840af5a01dd1))
* **types:** codex model aliases for openai-chatgpt ([daa6210](https://github.com/FerroxLabs/wayland-core/commit/daa6210a5ded3e1d95015ab1a0c195cbc9d18cca))


### Bug Fixes

* **model-catalog:** tag a floored model fetch BuiltIn, not a live "synced" ([0bca1a7](https://github.com/FerroxLabs/wayland-core/commit/0bca1a7545c8a5e4d8e7fa155e63f1e694d3014c))
* **model-picker:** load UI-saved provider keys + connection-aware live /model picker ([3a8929f](https://github.com/FerroxLabs/wayland-core/commit/3a8929fd45e9c5ef26ddabe79cf1904d570fd931))
* **providers:** accept codex response.done/incomplete as terminal frames ([0bc0ed6](https://github.com/FerroxLabs/wayland-core/commit/0bc0ed62a96ef8048c67e8a56e962a1ed8f93cff))
* **providers:** Bedrock/Vertex "connected" only with real ambient credentials ([7245065](https://github.com/FerroxLabs/wayland-core/commit/72450658c87fb78c642a91b54ce041f5dcf7cc1d))
* **providers:** don't request encrypted reasoning until we round-trip it ([52eeceb](https://github.com/FerroxLabs/wayland-core/commit/52eecebb3ae3ea70caa4d074a1b4cc68b9890ef4))
* **providers:** drop unused json import; lock socket2/base64 direct edges ([fd9100e](https://github.com/FerroxLabs/wayland-core/commit/fd9100ec250b2cc674887ed47d2cb48e437f5ff6))
* **providers:** forward list_models on OpenAI-compat newtypes (paste-connect) ([efbddba](https://github.com/FerroxLabs/wayland-core/commit/efbddba218df0f854f914a7ee77ff9e4b2fd324d))
* **providers:** ResilientProvider delegates alias_key/list_models to primary ([4c409c1](https://github.com/FerroxLabs/wayland-core/commit/4c409c1da6e5506c615a9279cbd092f41bcb56fe))
* **tui:** Config Esc saves pending toggles instead of reverting ([854f065](https://github.com/FerroxLabs/wayland-core/commit/854f0657843aee2ce2b4af0e0029adfedec45d62))
* **tui:** show em-dash for unrecorded spend in the status bar ([f8e5d65](https://github.com/FerroxLabs/wayland-core/commit/f8e5d6540a370d3a3398161c2e15437da3127f85))
* **tui:** stop /doctor from freezing the whole TUI on live probes ([4121652](https://github.com/FerroxLabs/wayland-core/commit/4121652ebd66cae28084d67d3d64ea6107da020c))
* **tui:** widen Advanced label pad so the value isn't glued to it ([1cb6578](https://github.com/FerroxLabs/wayland-core/commit/1cb65780e38e374606454eea865d520b20798087))


### Documentation

* **providers:** document Sign in with ChatGPT ([90e0c62](https://github.com/FerroxLabs/wayland-core/commit/90e0c6216347e4da8ae068729e7dd1b7104d093c))


### Build System

* **release:** prepare 0.12.1-rc.1 prerelease ([9c5922b](https://github.com/FerroxLabs/wayland-core/commit/9c5922b12b9fe35ba5636421619b756043a596ab))

## [0.11.0-rc.1] - 2026-06-11

Release candidate for 0.11.0. The headline is **inbound channels** — Wayland Core now receives, not just sends — plus native per-command Bash output compaction, a JWT crypto-backend security fix, and a batch of provider and platform fixes. Still a public beta; cut as an RC to soak the new network-facing channel surface before the final 0.11.0.

### Highlights

* **Inbound channels.** Two-way messaging across Telegram, Discord, Slack, WhatsApp, Matrix, Microsoft Teams, and SMS: inbound receive (long-poll / `/sync` / webhook host), an engine-backed turn dispatcher with a tool-posture scope for channel-originated agents, reconnect supervision so channels survive disconnects, Microsoft Teams Bot Framework JWT validation, outbound chunking with per-platform size caps, an idempotency nonce to dedupe retried sends, and react/typing with ack reactions + a typing keepalive state machine.
* **Auth-aware inbound media.** Images and audio attachments are fetched and described/transcribed before the turn, with credentials kept inside each connector boundary.
* **Native Bash output compaction.** Verbose `cargo` / `git` / test-runner / `grep` output is compacted into the model's transcript (the human still sees full output) — block-aware, fail-open, size-gated, default-on via `ProviderCompat::compact_bash`, with per-call savings telemetry.
* **Security.** Migrated the JWT crypto backend to `aws_lc_rs`, dropping `rsa` and eliminating RUSTSEC-2023-0071 (Marvin Attack) at the source. Closed a Grep RCE, skill/rules prompt-injection, and hook shell-execution hardening; capped stdin line length (newline-less OOM DoS); fail-closed on UTF-8 split-codepoint corruption.

### Providers

* gpt-5 family now routes to the OpenAI Responses API (`/v1/responses`).
* Gemini 2.5-class: split SSE frames on CRLF (stops false truncation); inject default items for array schemas (stops tool-registration 400s).
* Default moonshot/qwen to their international endpoints; pin `api_path` so 8 native providers stop 404ing.

### Fixes

* ALSA is no longer a hard dependency — `cpal` is gated behind an off-by-default `voice` feature, so the default binary runs on minimal Linux without `libasound` (#14).
* The `/config` providers pane now scrolls to keep the focused row visible on short terminals (#16).
* PATHEXT-aware `npx` detection on Windows so the IJFW MCP server registers (#6).
* Legacy-YAML migration no longer clobbers an existing `config.toml`.

### Extensibility

* Declarative on-disk plugins under the profile home, wiring hooks + MCP into the engine.

## [0.10.0] - 2026-06-08

First public release. Wayland Core is a domain-agnostic autonomous-agent engine written in Rust: terminal-first, multi-provider, MCP-native, and embeddable. It ships as a **public beta**, capable and open, and still hardening under a continuous endurance soak (see "Built to endure" in the README).

### Highlights

* **Multi-provider.** 7 native provider integrations (Anthropic, OpenAI, Google Gemini, Google Vertex AI, AWS Bedrock with SigV4, Cohere, Azure OpenAI) plus a 104-entry models.dev catalog, all behind one provider-neutral engine and a declarative ProviderCompat layer. Circuit-breaker resilience, mid-stream reconnect, and multi-key rotation across every API-key provider.
* **Orchestration.** Sub-agents, a git-worktree-isolated parallel swarm with a dirty-tree guard, declarative ForgeFlows workflows that lower onto the engine's own execution graph, and selectable reducers via `wayland swarm --reduce mesh|fleet|consensus|debate`.
* **Security by default.** A fail-closed OS-native sandbox (bubblewrap, sandbox-exec, AppContainer), a CI-enforced egress chokepoint with an exfil-shape classifier, an always-on SSRF and metadata floor, and argv-safe shell execution.
* **Extensibility.** MCP in both directions (a client, and a server that advertises and executes its own built-in tools, with runtime injection), roughly 70 built-in tools, skills, blocking lifecycle hooks, and a plugin API.
* **Embeddable.** A typed JSON-Lines protocol drives the engine headlessly behind a host app.
* **Self-evolution (GEPA).** A scored optimizer that evolves prompts and skills against your own reference cases.

### Surfaces

One binary, three ways to run it: a one-shot command, an interactive TUI, or a headless JSON stream.

### Notes

This is a public beta. APIs and behavior may change before 1.0. A continuous, fault-injected endurance trial is ongoing; the method, measurements, and honesty bounds are documented in [docs/resilience.md](docs/resilience.md).
