# wcore-tools port notes

Decisions taken while porting Hermes tool surfaces into `wcore-tools`. Each
entry records what was *not* lifted and why, so future contributors don't
re-do the analysis.

## T3-3.1.6 approval — NO-LIFT

**Source:** `wayland-hermes/agent/tools/approval.py` (1158 LOC)

**Decision:** Do not port to `wcore-tools`.

**Overlap with existing Wayland infrastructure:**

- `wcore-agent::approval` — `ApprovalBridge` already implements the
  request/resolve transport layer with opaque correlation IDs, a TTL
  reaper, and active-token redaction (Wave SC SECURITY MAJOR
  remediations). This subsumes Hermes's `submit_pending` /
  `resolve_gateway_approval` / `register_gateway_notify` flow.
- `wcore-protocol::events::{ApprovalRequired, ApprovalResume}` plus
  `wcore-protocol::commands::{ApprovalScope, ApprovalResume}` carry the
  on-wire shape (host integration). Hermes's session-keyed callbacks are
  the same idea, just expressed inline in Python.
- `wcore-protocol/tests/approval_manager_test.rs` covers the bridge
  behavior end-to-end.

**What approval.py contains that is genuinely separate:**

Hermes's `approval.py` also bundles a **dangerous-command classification
policy** — `HARDLINE_PATTERNS`, `DANGEROUS_PATTERNS`,
`detect_dangerous_command`, `is_hardline_blocked`, `_smart_approve`, and
the permanent-allowlist persistence (`approve_permanent` /
`load_permanent_allowlist`). This is shell-policy logic, not a tool
surface — it belongs at the `BashTool` pre-execution gate, not as a
standalone module in `wcore-tools`. The Tier-1 wave for BashTool
hardening is the right home for that policy, and it should be lifted
there as part of `BashTool`'s dangerous-command gate (separate plan,
separate review surface). Lifting it now would either duplicate the
bridge or create an orphan policy module without a caller.

**Conclusion:** No source change in this slice. `BashTool` dangerous-
command policy is a follow-up tracked outside the T3 tool-port wave.

## T3-3.2.4 file_operations — NO-LIFT

**Source:** `wayland-hermes/agent/tools/file_operations.py` (1384 LOC)

**Decision:** Do not port to `wcore-tools`.

**Overlap with existing Wayland infrastructure:**

- `wcore-tools/src/read.rs` — `ReadTool` provides file reads with the
  `FileStateCache` dedup-stub optimization and `validate_user_path`
  gating. Hermes `read_file` + `ReadResult` is the same surface,
  expressed through a shell-execute backend abstraction.
- `wcore-tools/src/write.rs` — `WriteTool` covers atomic file creation /
  rewrite with post-write cache refresh.
- `wcore-tools/src/edit.rs` — `EditTool` covers exact-match patch /
  replace_all with the "must Read first" + stale-mtime guards. Hermes's
  `PatchResult` diff-application surface is subsumed.
- `wcore-tools/src/grep.rs` + `glob.rs` — cover `search` / `SearchResult`.
- `wcore-tools/src/path_validation.rs` and the broader
  `wcore-config::shell` argv discipline already enforce the same write
  deny-list intent that Hermes encodes via `WRITE_DENIED_PATHS` /
  `WRITE_DENIED_PREFIXES` / `WAYLAND_WRITE_SAFE_ROOT`. If a future ticket
  needs the explicit sensitive-path deny list, the natural home is
  `path_validation`, not a new tool.

**What file_operations.py contains that is genuinely separate:**

The module's reason-to-exist in Hermes is the
`FileOperations` ABC + `ShellFileOperations` implementation that
multiplexes read/write/patch/search/lint/execute across **remote
terminal backends** (local, docker, singularity, ssh, modal, daytona).
Wayland's tool layer operates directly against the local filesystem
(or via the host-supplied `ToolContext`), and has no remote-terminal
backend abstraction — there is nothing to multiplex over. The
in-process linters (`_lint_python_inproc`, `_lint_json_inproc`,
`_lint_yaml_inproc`, `_lint_toml_inproc`) are Python-runtime helpers
with no analogue in a Rust agent.

**Conclusion:** No source change in this slice. The read/write/edit/
grep/glob surface already exists; the deny-list policy belongs in
`path_validation` if/when a ticket calls for it; the remote-backend
multiplexer has no home in Wayland's architecture.

## T3-3.2.5 file_tools — NO-LIFT

**Source:** `wayland-hermes/agent/tools/file_tools.py` (810 LOC)

**Decision:** Do not port to `wcore-tools`. Complete overlap with the
existing Wayland built-in tool surface — every public tool in
`file_tools.py` already has a Rust equivalent.

**Tool-by-tool overlap mapping:**

| Hermes (`file_tools.py`)            | Wayland built-in                       |
|-------------------------------------|----------------------------------------|
| `read_file_tool` (L293)             | `crates/wcore-tools/src/read.rs` (`ReadTool`) — offset/limit, FileState mtime tracking via `file_cache`, `path_validation::validate_user_path` |
| `write_file_tool` (L552)            | `crates/wcore-tools/src/write.rs` (`WriteTool`) — `update_cache_after_write`, path validation |
| `patch_tool` (L576, replace/insert) | `crates/wcore-tools/src/edit.rs` (`EditTool`) — old_string/new_string replace with mtime staleness check |
| `search_tool` (L633, content+name)  | `crates/wcore-tools/src/grep.rs` (`GrepTool`) for content; `crates/wcore-tools/src/glob.rs` (`GlobTool`) for filename matching |

**Hermes-side helpers that are policy, not tool surface:**

- `_is_blocked_device` (L85) — `/dev/zero`, `/dev/random`, etc. blocklist.
  Wayland's `read.rs` + `path_validation` handle device-path safety via
  the validator + file size guards; the explicit blocklist is a Linux-
  centric heuristic that doesn't generalize to the Windows CI target.
- `_check_sensitive_path` (L113) — secret-path warning. Lives in
  `wcore-config/credentials.rs` and the redaction layer (Tier-1
  vault/redact slice), not the tool itself.
- `_check_file_staleness` (L521) — mtime drift between read and write.
  Implemented in `crates/wcore-tools/src/file_cache.rs` (the
  `FileStateCache` consumed by `read.rs` / `write.rs` / `edit.rs`).
- `_get_max_read_chars` / `_DEFAULT_MAX_READ_CHARS` (L28-63) — read-size
  guard via `file_read_max_chars` config. Wayland's `read.rs` already
  caps output through its own limit constant; cross-referenced with the
  config crate rather than a tool-local cache.

**Hermes-side machinery deliberately not replicated:**

- `ShellFileOperations` per-task cache (L163, L284) — Hermes shells out
  to `/bin/cat`, `/bin/grep`, etc. through a task-keyed wrapper. Wayland
  reads/writes directly via `std::fs` (and `tokio::process::Command` for
  grep), which is portable to Windows and avoids the spawn-per-op cost.
- `notify_other_tool_call` / `reset_file_dedup` (L465-501) — Hermes
  dedup state for repeated reads. Wayland's `FileStateCache` is the
  Rust-side equivalent, scoped per `ToolContext`.

**Conclusion:** No source files added. file_tools.py is a Python
re-expression of the same five Rust tools (Read/Write/Edit/Glob/Grep)
plus task-cached shell wrappers that don't translate to Wayland's
direct-fs model. Porting would duplicate every tool in `wcore-tools`.

## T3-3.2.6 path_security — NO-LIFT

**Source:** `wayland-hermes/agent/tools/path_security.py` (43 LOC)

**Decision:** Do not port to `wcore-tools`. Full overlap with existing
`wcore-tools::path_validation`.

**Surface comparison:**

Hermes's `path_security.py` exposes two helpers:

- `validate_within_dir(path, root) -> Optional[str]` — `Path.resolve()`
  both inputs and verifies the target is `relative_to(root)`. Returns
  an error string on escape, `None` on success.
- `has_traversal_component(path_str) -> bool` — quick `..`-component
  check before doing a full resolution.

`crates/wcore-tools/src/path_validation.rs` already provides
`validate_user_path()`, which is **strictly stronger** than the Hermes
surface:

1. **Traversal rejection** — matches `has_traversal_component`
   (`path.components().any(|c| matches!(c, Component::ParentDir))`).
2. **Null-byte rejection** — Hermes lacks this entirely.
3. **Absolute-path enforcement** — Hermes lacks this; relies on the
   caller passing a sensible `root`.
4. **Lex-normalization** — `lex_normalize()` collapses `.` / `..`
   components without touching the filesystem.
5. **System deny-list** — `/etc/shadow`, `/etc/sudoers`, `~/.ssh/id_*`,
   `~/.aws/credentials`, `~/.gnupg/private-keys-v1.d`, `~/.kube/config`
   etc. Hermes has no equivalent.

**Containment check (`validate_within_dir`):**

The `relative_to(root)` containment semantics are already covered by
`SandboxedFs` / `VirtualFs` on the `_with_ctx` execution path (see
`path_validation.rs` module docs, lines 30-33). Tools that need a
root-clamp use the sandboxed FS rather than a free-function helper, so
there's no caller for a standalone `validate_within_dir` in Rust.

**Conclusion:** No source change. Wayland's path-validation surface is
already broader than Hermes's; lifting `path_security.py` would either
duplicate `validate_user_path` or add an unused free function.

## T3-3.3.1 ansi_strip — NO-LIFT

**Source:** `wayland-hermes/agent/tools/ansi_strip.py` (44 LOC)

**Decision:** Do not port to `wcore-tools`. Full overlap with existing
`wcore-compact::sanitize::strip_ansi`.

**Existing implementation:**

`crates/wcore-compact/src/sanitize.rs` (lines 5-12) already exposes a
public `strip_ansi(text: &str) -> String` helper backed by a
`LazyLock<Regex>` over `\x1b\[[0-9;]*[a-zA-Z]`. The helper is the
canonical sanitizer for cargo/subprocess output entering the model's
context window and has five unit tests covering:

- bare SGR color codes (`strip_ansi_color_codes`)
- nested bold + color (`strip_ansi_bold_and_nested`)
- no-codes passthrough (`strip_ansi_no_codes_unchanged`)
- CSI cursor movement (`strip_ansi_cursor_movement`)
- empty input (`strip_ansi_empty_input`)

The dependency direction is correct: `wcore-compact` is a bottom-layer
crate, so any future caller in `wcore-tools`, `wcore-providers`, etc.
can depend on it without inverting the graph.

**What hermes ansi_strip.py adds that wcore-compact does not:**

The hermes regex covers the full ECMA-48 surface — OSC (`\x1b]…BEL` /
`ST`), DCS/SOS/PM/APC strings, nF multi-byte escapes, single-byte
Fp/Fe/Fs, 8-bit C1 controls (`\x9b`, `\x9d`, `\x80-\x9f`), plus a
fast-path `_HAS_ESCAPE` pre-check. The current Wayland regex handles
only the SGR/CSI form (`\x1b[…<letter>`), which is sufficient for the
present caller (compaction of cargo output) but would not strip OSC
title sequences or 8-bit C1 controls.

**Why this is still NO-LIFT, not a widening:**

No current caller in `wcore-tools` (or any other crate) needs the
broader ECMA-48 coverage — `BashTool`, `read.rs`, and the provider
output pipelines do not flag ANSI residue as a problem today. Widening
`wcore-compact::strip_ansi` to the ECMA-48 superset is a behavior
change in the compaction sanitizer and belongs in its own slice with a
dedicated regression test against the existing five sanitize tests, not
inside a T3 tool-port sub-wave. Adding a second `strip_ansi` in
`wcore-tools` would violate the no-duplicate-code rule from
`AGENTS.md`.

**Conclusion:** No source change. Future ECMA-48 expansion (OSC/DCS/8-bit
C1) is tracked as a `wcore-compact` enhancement, separate from the
T3 tool-port wave.

## T3-3.3 permission_classifier — NO-LIFT (cross-tier follow-up)

**Source:** `wayland-hermes/agent/tools/permission_classifier.py`
(749 LOC; key symbols at lines 43, 58, 76, 83, 200, 215, 388, 414, 463,
489, 595, 610, 666, 680).

**Decision:** Do not port to `wcore-tools`. Filed as a cross-tier
follow-up against the BashTool pre-exec gate / future approval-policy
wave — NOT a Tier 3 tool surface.

**Overlap with existing Wayland infrastructure:**

- `wcore-agent::approval::ApprovalBridge` (T1/T2) — owns the
  request/resolve transport with correlation IDs, TTL, and redaction.
  Hermes' `evaluate_command` `GateDecision` plugs into that same
  approval flow at the bridge boundary, not inside `wcore-tools`.
- `wcore-tools::bash::check_denylist` (Wave SA) — already covers the
  *credential-exfiltration* slice of the hard-block list (curl/wget |
  sh and friends). The Hermes hard-block regex set is strictly broader
  (`git push --force`, `dd if=`, `mkfs.*`, `wipefs`, `shred`,
  `sudo`/`doas`, `su -`, `chmod 777`, `chown root`, PowerShell `iex`/
  `irm`) but the integration point is identical: pre-exec gate inside
  `BashTool::call`, alongside (or replacing) `check_denylist`.

**Why not port the classification body into `wcore-tools` now:**

1. The classifier is a *policy* layer, not a *tool*. Its output
   (`Tier::{ReadOnly, WriteClass, ShellClass, HardBlock, Unparseable}`
   + `PermissionMode` from `WAYLAND_PERMISSION_MODE`) drives whether
   `BashTool` auto-approves, prompts via `ApprovalBridge`, or refuses
   outright — that decision belongs at the BashTool pre-exec gate, not
   as a free helper in `wcore-tools`.
2. Faithful port requires a `bashlex`-equivalent AST decomposer in
   Rust (`_walk_for_commands` / `_decompose`, lines 282–386). Without
   it, compound commands like `ls && rm -rf /` mask the dangerous
   sub-command behind a benign first token — the entire reason Hermes
   uses bashlex. A `shlex`-only fallback would silently downgrade
   security guarantees. No suitable crate is currently a dependency;
   adding one is a sub-wave of its own.
3. The `PermissionMode` env-var contract (AGENT_ENV_CONTRACT v1:
   `WAYLAND_PERMISSION_MODE`, `WAYLAND_PRE_APPROVED_HASHES`,
   `WAYLAND_HARDFLOOR_OVERRIDE`, `WAYLAND_WORKSPACE_DIR`) needs a
   matching Wayland host-side contract before the engine can honor
   it. Lifting the agent half without the host half produces dead env
   reads.

**Cross-tier follow-up (filed, not done here):**

- T1/T-future security wave: extend `wcore-tools::bash::check_denylist`
  (or a sibling `hard_block_scan`) with the broader hard-block regex
  set from `permission_classifier.py` lines 99–130 (git push --force,
  dd, mkfs, wipefs, shred, sudo/doas, su -, chmod 777, chown root,
  curl|sh variants, PowerShell iex/irm). Pure regex addition — no AST
  needed for the whole-string scan, and it composes with the existing
  `RegexSet`.
- Future approval-policy wave: introduce `PermissionMode` +
  `Tier`-class classification at the `BashTool` pre-exec gate, wired
  to `ApprovalBridge`. Requires (a) bashlex-equivalent AST, (b) host
  env contract, (c) `ApprovalDisposition` policy table per mode.

**Conclusion:** No source change in T3-3.3. Two follow-ups recorded
above. Lifting the classifier into `wcore-tools` now would either
duplicate the bridge boundary or ship a security-downgraded shlex-only
classifier — neither acceptable.

## T3-3.4 registry — NO-LIFT (overlap with existing `ToolRegistry`)

**Source:** `wayland-hermes/agent/tools/registry.py` (482 LOC).
Hermes' singleton `ToolRegistry` collects tool schemas + handlers via
module-import-time `registry.register(...)` calls, supports toolset
groupings + aliases, AST-based built-in discovery, MCP nuke-and-repave
refresh, per-tool `max_result_size_chars`, emoji metadata, and
`get_definitions()` returning OpenAI-format `{type, function}` schemas.

**Decision:** Do not port to `wcore-tools`. Hermes' `ToolRegistry` is
functionally the same surface as Wayland's existing
`wcore-tools::registry::ToolRegistry` — registering, looking up,
dispatching, and emitting `ToolDef`s for a set of tools. Porting any
helper module would either duplicate that surface under a different
name or introduce a parallel registry that the engine has to
reconcile with at every call site. Both outcomes violate the "no
duplicate code across crates" rule in `AGENTS.md`.

**Overlap with existing Wayland infrastructure (1:1 mapping):**

| Hermes (`registry.py`) | Wayland equivalent | Notes |
|---|---|---|
| `ToolRegistry.register(name, toolset, schema, handler, …)` | `ToolRegistry::register(Box<dyn Tool>)` | Wayland uses trait-object polymorphism; schema lives on `Tool::input_schema()`, handler on `Tool::execute()`. |
| `ToolEntry.toolset` (string grouping) | `Tool::category() -> ToolCategory` (enum) + plugin-api `Toolset` boundary | Wayland uses a typed enum at the tool surface and a plugin-isolation boundary above it — strictly stronger than free-form strings. |
| `register_toolset_alias()` / `get_toolset_alias_target()` | n/a — not needed | Aliases are a Python-side artifact for human-typed CLI flags; Wayland's CLI doesn't expose toolset names as a user-typed dimension. |
| `discover_builtin_tools()` (AST scan + `importlib`) | Static `register(Box::new(...))` calls in `wcore-agent`/plugin init | Wayland resolves discovery at compile time via the plugin system — strictly stronger than runtime AST inspection. |
| MCP refresh via `deregister()` + re-register | `wcore-mcp` client-driven registration into `ToolRegistry` | Wayland's MCP layer already owns the refresh contract; replicating Hermes' mutate-during-read locking would duplicate `wcore-mcp` responsibilities. |
| `get_definitions(tool_names, quiet)` → OpenAI `{type, function}` | `ToolRegistry::to_tool_defs()` / `to_tool_defs_filtered(...)` → `ToolDef` | Wayland emits provider-neutral `ToolDef` and lets each `LlmProvider` format-convert in `build_request_body()` (per AGENTS.md "no hardcoded provider quirks"). Porting Hermes' OpenAI-shape emitter would re-introduce the quirk. |
| `dispatch(name, args, **kwargs)` (catches all exceptions, returns JSON) | `ToolRegistry::dispatch(tool, input)` / `dispatch_with_ctx(...)` via `ToolDispatcher` trait | Wayland adds a per-tool `CircuitBreaker` around dispatch (3 failures / 30s trip) — no counterpart in Hermes. Porting would lose this without merge surgery. |
| `get_max_result_size(name)` | budget config in `wcore-config` / per-tool result truncation in tool impls | Wayland keeps result-size policy out of the registry; lifting it would entangle the registry with budget policy. |
| `get_emoji(name, default)` | n/a — Wayland surface doesn't carry presentation metadata | Emoji is a UI concern routed through `wcore-protocol` events, not the tool registry. |

**Why not port as a helper with a non-colliding name (e.g. `tool_catalog.rs`):**

1. Every load-bearing operation in Hermes' module — register, lookup,
   dispatch, schema enumeration — already exists in
   `wcore-tools::registry::ToolRegistry`. A `tool_catalog` helper
   would either (a) wrap `ToolRegistry` and re-export the same API
   under a new name, or (b) hold its own `HashMap<String, …>` and
   require every dispatch site to know which catalog to consult.
   Both are duplicate-code violations.
2. The Python-idiom features that *don't* map cleanly — `emoji`,
   `max_result_size_chars`, free-form `toolset` strings, AST
   discovery — would each have to be retrofitted onto every existing
   `Tool` impl (Read, Write, Edit, Bash, Grep, Glob, Spawn, MCP-side
   tools, plugin tools). That's a cross-crate refactor disguised as
   a 482-line port, and the resulting fields are not used by the
   engine because Wayland already routes those concerns elsewhere
   (`ToolCategory`, budget config, protocol-side presentation).
3. Hermes' module-import-time auto-registration model is a
   side-effect-driven design that conflicts with Rust's explicit
   construction model and with `wcore-plugin-api`'s scoped
   `register_*` surfaces. There is no semantically-clean Rust
   equivalent of "import a `.py` file to register a tool"; Wayland's
   plugin-anchor crates (`wayland-ijfw` et al. in AGENTS.md crate map)
   already provide the explicit equivalent.

**Conclusion:** No source change in T3-3.4. The Wayland `ToolRegistry`
+ plugin system + `wcore-mcp` refresh path already covers every
behavior in `agent/tools/registry.py`. The handful of Python-side
extras (toolset aliases, emoji, per-tool size, AST discovery) are
either provider-quirk emitters that would violate AGENTS.md or UI
concerns that don't belong at the registry layer.

## T3-3.4 credential_files — NO-LIFT

**Source:** `wayland-hermes/agent/tools/credential_files.py` (422 LOC)

**Decision:** Do not port to `wcore-tools` (nor anywhere else in the
workspace). The module is a **remote-sandbox file-mount registry**, not
a credential store. Wayland has no consumer for it, and the actual
credential-storage surface already exists in Tier-1
`wcore-config/src/credentials.rs`.

**What credential_files.py actually does:**

Per its own module docstring (lines 1-19) and exported surface:

| Hermes export                          | Purpose                                                           |
|----------------------------------------|-------------------------------------------------------------------|
| `register_credential_file` (L70)       | Append a `WAYLAND_HOME`-relative path to a `ContextVar`-scoped registry of files to mount into a remote sandbox at `/root/.wayland/...`. |
| `register_credential_files` (L120)     | Bulk version of the above, fed by skill `required_credential_files` declarations. |
| `_load_config_files` (L145)            | Read `terminal.credential_files` from user config (process-cached). |
| `get_credential_file_mounts` (L190)    | Return the merged session+config list as `{host_path, container_path}` dicts, consumed by Docker/Modal/SSH backends at sandbox-create time. |
| `get_skills_directory_mount` (L216) + `iter_skills_files` (L307) | Mount or sync the skills directory into the remote sandbox. |
| `_safe_skills_path` (L264)             | Path-traversal guard against the skills dir mount target. |
| `get_cache_directory_mounts` (L367) + `iter_cache_files` (L391) | Mount the gateway-cached upload/screenshot/TTS/image cache dirs read-only into the sandbox so tools like `unzip` can see host-side files. |
| `clear_credential_files` (L418)        | Reset the per-context registry. |

The module exists in Hermes because Hermes ships **remote terminal
backends** (Docker, Modal, SSH, Singularity, Daytona) that create
sandboxes with empty filesystems. To execute `gcloud`, `gh auth`, or
skill-supplied tools inside those sandboxes, host-side credential
files and cache dirs must be mounted into the container at known
container paths.

**Why this has no Wayland home:**

1. **No remote-terminal backend abstraction.** Wayland tools
   (`crates/wcore-tools/src/{bash,read,write,edit,grep,glob,spawn}.rs`)
   execute against the local filesystem (or the host-supplied
   `ToolContext` virtual FS via `SandboxedFs`/`VirtualFs` —
   `path_validation.rs` docs, lines 30-33). There is no per-session
   sandbox to mount files *into*. This is the same architectural gap
   the T3-3.2.4 file_operations NO-LIFT recorded (NOTES lines 69-85):
   "Wayland's tool layer operates directly against the local
   filesystem … and has no remote-terminal backend abstraction —
   there is nothing to multiplex over."

2. **Credential *storage* already covered by Tier-1.**
   `wcore-config/src/credentials.rs` (READ-ONLY for this slice) ships
   three vault backends — `Plaintext` (TOML with enforced `0o600`),
   `Keyring` (Keychain / Credential Manager / Secret Service), and
   `EncryptedFile` (Argon2id + XChaCha20-Poly1305 over TOML), per the
   crate's own module docs (lines 8-15, 41-50). API keys / AWS / GCP
   secrets that close SECURITY MAJOR #16 live there. credential_files.py
   does **not** touch any of these — it only enumerates files for
   container mount points.

3. **`required_credential_files` skill front matter has no Wayland
   counterpart in this wave.** Wayland's `wcore-skills` (see
   `crates/wcore-skills/`) consumes its own front-matter schema; the
   Hermes `required_credential_files` key is not part of that contract.
   Adding the registry alone would create a dormant API with no
   producer and no consumer.

4. **Cache-directory mounts (uploads, screenshots, TTS audio, images)
   are gateway-side concerns.** Wayland's browser stack
   (`wcore-browser` — ARIA-tree-first, BrowserPolicy network
   boundary) and the engine's `ToolContext` write artifacts directly
   to the working directory or host-managed paths. No
   gateway-cache → container-mount translation is required, because
   there is no container.

**What might look portable but isn't:**

- `_safe_skills_path` (L264) — a relative-path traversal guard.
  Already strictly subsumed by `crates/wcore-tools/src/path_validation.rs`
  `validate_user_path()` (traversal rejection + null-byte + absolute-
  path + lex-normalization + system deny-list) — same surface that
  obsoleted Hermes `path_security.py` in T3-3.2.6 (NOTES lines
  136-178).
- The `ContextVar`-scoped session registry (L33-L43) — a Python
  idiom for preventing cross-session bleed in the gateway request
  pipeline. Wayland's per-session state lives on `ToolContext` /
  `SessionState`, not in module-level context vars; even if a mount
  registry were needed, the Python pattern would not translate.

**Cross-tier follow-up (filed, not done here):**

- If a future Wayland wave introduces a remote-sandbox terminal
  backend (Docker / Modal / SSH analogue), this is where the
  file-mount registry would live — but as a `wayland-remote-backend`
  plugin against `wcore-plugin-api`, **not** as a tool inside
  `wcore-tools`. The natural API shape would mirror
  `BrowserPolicy` / `CuaPolicy` (per the crate map): a `MountPolicy`
  gate plus a per-session registry, consumed by the backend at
  sandbox-create time. No work needed in v0.6.2.

**Conclusion:** No source files added. credential_files.py solves a
problem (host→container file-mount registration for remote sandboxes)
that does not exist in Wayland's architecture. The credential-storage
half is already owned by Tier-1 `wcore-config/credentials.rs`; the
path-safety half is already owned by `wcore-tools::path_validation`.
Porting would create a dormant module with no producer and no consumer.

## T3-3.4 mcp_oauth — DEFER (NO-LIFT)

**Source:** `wayland-hermes/agent/tools/mcp_oauth.py` (482 LOC)

**Decision:** Do not port in sub-wave T3-3.4. This is also NOT a
`wcore-tools` concern — OAuth belongs with the MCP client surface
(`wcore-mcp`), so this note is recorded here only because NOTES.md is
the established T3-3 decision log; the actual integration target is
`wcore-mcp` once Tier 2 server work lands.

**Why DEFER rather than implement now:**

1. **Tier 2 server work has not landed in this trunk.** The
   sub-wave instructions identify `crates/wcore-mcp/src/server.rs` as
   the Tier 2 boundary and forbid modifying it; that file does not
   exist in the current `wcore-mcp/src/` tree (only `config.rs`,
   `lib.rs`, `manager.rs`, `protocol.rs`, `tool_proxy.rs`,
   `transport/`). Per the sub-wave instructions: *"If
   `crates/wcore-mcp/src/server.rs` doesn't exist (Tier 2 incomplete):
   defer with note, do NOT add to wcore-mcp."*

2. **No auth surface exists in `wcore-mcp` to plug into.**
   `crates/wcore-mcp/src/transport/{streamable_http,sse}.rs` carry
   the HTTP transports but no `Authorization`-header hook, no
   `httpx.Auth`-equivalent trait, and `McpServerConfig` (re-exported
   from `wcore-config::config`) has no `auth: oauth` variant or
   `oauth: { ... }` sub-block. Wiring a token provider in isolation
   would have nowhere to call from.

3. **The Hermes module is a thin SDK adapter, not an OAuth
   implementation.** `mcp_oauth.py` is a glue layer over
   `mcp.client.auth.OAuthClientProvider` from the Python MCP SDK —
   the SDK handles discovery, dynamic client registration (RFC 7591),
   PKCE, token exchange, refresh, and step-up auth. The Python module
   only contributes:
   - `WaylandTokenStorage` — on-disk token/client-info persistence
     under `WAYLAND_HOME/mcp-tokens/<server>.json` with 0o600
     permissions and atomic `.tmp` → rename writes.
   - Localhost callback HTTP server (ephemeral port, polls for
     `?code=&state=` redirect).
   - `_redirect_handler` + `_can_open_browser` (SSH / DISPLAY /
     macOS / Windows probe) to either auto-open or print the
     authorization URL.
   - `build_oauth_auth()` — assembles `OAuthClientMetadata`
     (PKCE / `token_endpoint_auth_method=none` by default,
     `client_secret_post` if a secret is configured), optional
     pre-registered client info, and returns the provider.

   The Rust port would need to either (a) wrap a Rust MCP SDK that
   exposes the equivalent provider trait — none is currently a
   `wcore-mcp` dep — or (b) reimplement OAuth 2.1 / PKCE / dynamic
   client registration from scratch against `reqwest`. Option (a)
   is blocked on Tier 2; option (b) is far outside a sub-wave 4
   ≤20-tool-call lift.

4. **Forbidden-modification rule.** Even once Tier 2 lands,
   `server.rs` is off-limits per the sub-wave brief, and OAuth auth
   plausibly needs hooks in the transport request path that Tier 2
   would establish. Implementing now would lock in a shape the Tier 2
   author hasn't designed yet.

**What a future port should preserve:**

- The storage layout (`WAYLAND_HOME/mcp-tokens/<safe-name>.json` and
  `<safe-name>.client.json`, 0o600, atomic writes) — operators
  inheriting existing token caches should not have to re-authorize.
- The `_safe_filename` sanitizer (`[^\w\-]` → `_`, max 128 chars)
  so server names with slashes / spaces / colons stay path-safe.
- Auto-port redirect (`redirect_port: 0` → free ephemeral port)
  alongside an explicit override (`redirect_port: 53682` etc.) for
  air-gapped or firewalled environments where the redirect target
  must be predictable.
- The non-interactive guard: when no cached tokens exist and stdin
  is not a TTY, *warn and continue* (don't fail) — first-run
  bootstrap is the only place the warning fires.
- The browser-detection ladder (SSH_CLIENT / SSH_TTY → no;
  Windows / macOS → yes; Linux/X11 / Wayland → only if `DISPLAY`
  or `WAYLAND_DISPLAY`). This determines whether we attempt
  `webbrowser.open` equivalent vs. just printing the URL.
- The configuration shape under `mcp_servers.<name>.oauth`:
  optional `client_id`, `client_secret`, `scope`, `redirect_port`,
  `client_name`, `timeout`.

**Conclusion:** No source change in T3-3.4. Re-evaluate once Tier 2
`wcore-mcp/src/server.rs` lands and exposes an auth-provider hook;
the new module would live at `crates/wcore-mcp/src/oauth.rs` (not in
`wcore-tools`) per the sub-wave decision tree.

## T3-3.4 process_registry — NO-LIFT (orchestrator-applied)

**Source:** `wayland-hermes/agent/tools/process_registry.py` (1211 LOC).

**Decision:** Do not port. No worktree commit (agent deleted its branch
after surveying — orchestrator-applied this note post-merge so the
analysis is preserved.)

**Overlap / dependency analysis:**

Hermes' `ProcessRegistry` owns the entire background-process lifecycle:
20+ field `ProcessSession` dataclass (PID, Popen handle, PTY handle,
watcher metadata, rate-limited pattern matcher state, gateway
session_key), 28 methods (`spawn_local`, `spawn_via_env`, `_reader_loop`,
`_env_poller_loop`, `_pty_reader_loop`, `poll`, `wait`, `kill_process`,
`write_stdin` / `submit_stdin` / `close_stdin`,
`has_active_for_session`, `_write_checkpoint`,
`recover_from_checkpoint`, `_check_watch_patterns`).

Wayland equivalents:
- `crates/wcore-tools/src/bash.rs` uses synchronous `cmd.spawn()` +
  `.wait_with_output()` — run-to-completion only, no background mode.
- `crates/wcore-tools/src/script.rs` has no background process spawn.
- No `ProcessRegistry` / `BackgroundProcess` / `spawn_process` symbol
  exists anywhere in `wcore-tools` or `wcore-agent`.

**Why not port:**

1. **Zero consumers.** `BashTool` is run-to-completion; no
   `terminal_tool`, no `background=true` flag, no gateway watcher
   channel exists. A direct port would produce dead code.
2. **~70% of hermes LOC wires to absent subsystems** —
   `tools.environments.local` Docker/Modal/SSH abstraction,
   gateway-side `pending_watchers` + completion queue + watcher
   protocol, `ptyprocess`-backed PTY reader thread, env-poller for
   sandboxed PIDs, crash-recovery checkpoint at
   `~/.wayland/processes.json`. None of those concerns have a Wayland
   equivalent.
3. **Architectural import, not helper port.** A faithful port would
   require simultaneously introducing the environment trait, watcher
   protocol, BashTool background mode, and notification queue —
   multi-wave architecture work, outside Tier-3 helper-lift scope.

**Recommendation for future waves:**

When BashTool gains a `background=true` mode or a `terminal_tool` is
planned, design a Rust-native registry from scratch using
`tokio::process::Child` + `tokio::sync::Mutex<HashMap<SessionId,
ProcessSession>>` + a bounded `VecDeque<u8>` rolling-output buffer.
Skip translating the Python — its watcher/PTY/env-poller branches are
dead weight for Wayland's architecture.

**Conclusion:** No source change. Branch deliberately not preserved
(no commits to merge). Future background-process work picks up from
this note + the hermes source as a behaviour reference, not a port
target.

## T3-3.5 browser_camofox_state — NO-LIFT

**Source:** `wayland-hermes/agent/tools/browser_camofox_state.py` (47 LOC).
Exposes two helpers:

- `get_camofox_state_dir() -> Path` returns
  `WAYLAND_HOME/browser_auth/camofox` — the host-side root that the
  Camofox sidecar persists profile data under.
- `get_camofox_identity(task_id) -> {user_id, session_key}` derives a
  profile-stable `wayland_<10hex>` user id (uuid5 over the state-dir
  path) and a per-task `task_<16hex>` session key (uuid5 over
  `<state-dir>:<task_id>`). The Wayland gateway sends these to the
  Camofox HTTP service so it can map repeated `open_session` calls to
  the same persistent browser profile directory across restarts.

**Decision:** Do not port to `wcore-tools` (or anywhere else in this
sub-wave). The owning surface is `wcore-browser`, which is READ-ONLY
for this slice, and the engine-side persistence contract is not yet
expressed there.

**Why this is NOT a wcore-tools concern:**

The helpers are pure inputs to the Camofox provider's `open_session`
HTTP body — they do not read or write the local filesystem, do not
spawn processes, and do not interact with `ToolContext`. They belong
with the Camofox backend, not with the agent's tool surface. Placing
them in `wcore-tools` would create a dormant module with no producer
inside the tools crate and no consumer (the only legitimate caller
lives behind the `BrowserProvider` trait in `wcore-browser`).

**Why this is NO-LIFT against `wcore-browser` rather than DEFER:**

The current Camofox backend's persistent-profile contract is already
narrower than the Hermes one and is correct for the present engine
shape:

- `crates/wcore-browser/src/backends/camoufox.rs:117-148` sends only
  `{ "persistent_profile": bool }` on `POST /sessions`. There is no
  `user_id` / `session_key` field on the wire, and the sidecar Wayland
  ships against today does not consume one.
- `crates/wcore-browser/src/backends/{browserbase,chromium}.rs` carry
  the same `persistent_profile: bool` shape (lines 75/79/102 and
  121/135 respectively). The boolean is the cross-backend invariant;
  the Hermes user/session identity is a Camofox-specific extension
  that the other two backends do not honor.
- A separate user-id/session-key identity contract requires (a) a
  matching field on the Camofox HTTP API, (b) a Wayland-side notion of
  "active profile" to derive the digest from, and (c) a task-id
  surface threaded through `BrowserSession` / `SessionCtx`. None of
  those exist in `wcore-browser` today, and adding them would be a
  cross-cutting browser-stack design choice — not a 47-LOC helper port.

**What a future port should preserve (if/when the identity contract is
added to `wcore-browser`):**

- The directory layout under `<wayland-home>/browser_auth/camofox/` so
  operators inheriting existing persistent profiles do not lose them.
  `wayland_home` resolution would route through `wcore-config` (the
  central `WAYLAND_HOME` resolver), not a free helper.
- The uuid5 derivation strategy (`NAMESPACE_URL`,
  `camofox-user:<scope_root>` and
  `camofox-session:<scope_root>:<logical_scope>`, truncated to 10 and
  16 hex chars respectively) so that re-deriving from the same
  profile root yields the same `wayland_<...>` / `task_<...>` strings
  the sidecar already has cached.
- The `task_id or "default"` fallback for session scope so callers
  that have not yet opened a logical "task" still get a stable key
  per profile.
- 0o600-equivalent permissions on any on-disk state if the directory
  ever stores tokens — Hermes' helper does not write anything itself,
  but a future `wcore-browser` extension should match
  `wcore-config::credentials` discipline.

**Cross-tier follow-up (filed, not done here):**

- When `wcore-browser`'s Camofox backend gains a Wayland-managed
  identity field on `open_session`, the natural home is
  `crates/wcore-browser/src/backends/camoufox.rs` (alongside the
  existing `persistent_profile` plumbing) with the `WAYLAND_HOME`
  resolution borrowed from `wcore-config`. The two helpers fit in
  ~30 lines of Rust against `uuid::Uuid::new_v5` + the existing
  `dirs`/config-cascade plumbing — they do not warrant a standalone
  module.

**Conclusion:** No source change in T3-3.5. `wcore-browser` is
READ-ONLY for this slice, and the Hermes module's two helpers have no
caller in the current `BrowserProvider` contract. Re-evaluate when the
Camofox HTTP surface grows a Wayland identity field; until then, the
helpers stay in Hermes.

## T3-3.4 checkpoint_manager — NO-LIFT (orchestrator-applied)

**Source:** `wayland-hermes/agent/tools/checkpoint_manager.py` (635 LOC).

**Decision:** Do not port. Sub-wave branch created (NOOP — no diff vs
trunk); orchestrator preserves the analysis here.

**Why this is NOT a Tier-3 tool surface:**

Hermes' own module docstring states:

> *"This is NOT a tool — the LLM never sees it. It's transparent
> infrastructure controlled by the `checkpoints` config flag or
> `--checkpoints` CLI flag."*

Tier-3 sub-wave 4 is a **tool port** slot. checkpoint_manager is
engine infrastructure (shadow-`git` snapshot system under
`~/.wayland/checkpoints/{sha256(abs_dir)[:16]}/` driven by
`GIT_DIR`+`GIT_WORK_TREE`). It does not belong here.

**Tier-1 has already made the scoping decision:**

`crates/wcore-cli/src/crash_sentinel.rs:10-13` explicitly states:

> *"The Forge version also persists a checkpoint payload + history; we
> lift only the flag mechanic here. The richer checkpoint payload is
> out of scope for T1-E2."*

Re-introducing the richer payload via Tier-3 would contradict that
deliberate Tier-1 decision.

**Different layer from `crash_sentinel`:**

- `crash_sentinel` is a dirty-death RAII flag (write-on-start /
  remove-on-Drop).
- hermes `checkpoint_manager` is a shadow-`git` snapshot system with
  pre-mutation `ensure_checkpoint` hooks, per-turn `new_turn` signals,
  and rich history. Not the same concern — but **the richer concern
  was intentionally deferred at Tier 1, not assigned to Tier 3.**

**Architectural cross-cutting that a faithful port requires:**

- A new crate (`wcore-checkpoint`) or a major addition to
  `wcore-agent` / `wcore-config`.
- Hook points in `wcore-tools` Write / Edit for pre-mutation
  `ensure_checkpoint`.
- A conversation-turn signal from `wcore-agent` for `new_turn`.
- `git` shell-out discipline via `wcore_config::shell` argv mode (paths
  are LLM-adjacent input).
- CLI flag `--checkpoints` + config-cascade wiring.

These cuts span multiple crates and the agent loop — properly a
dedicated Tier-1/2-style engine epic, not a sub-wave-4 helper port.

**No clean helper carve-out:**

All `_validate_commit_hash`, `_shadow_repo_path`, `_git_env`,
`_run_git`, `_init_shadow_repo`, `_dir_file_count`,
`format_checkpoint_list`, `_parse_shortstat`, `_prune` helpers are
tightly coupled to the shadow-repo model. Lifting any in isolation
would produce a stub/orphan, violating the "NO STUBS, NO DUPLICATES"
rule.

**Recommendation for future:**

If the richer checkpoint payload is desired, plan it as a dedicated
engine epic (e.g. `T2-E*` or `T1-E2-followup`), not a Tier-3 tool
sub-wave. Suggested home: a new `wcore-checkpoint` mid-layer crate
(deps: `wcore-config` for shell argv-mode, `wcore-types` for shared
paths), with hook points exposed for `wcore-tools` Write / Edit
pre-mutation and `wcore-agent` per-turn invocation.

**Conclusion:** No source change in T3-3.4. Tier-1 already scoped this
to the flag mechanic; the richer payload is a future engine epic.

## T3-3.5 browser_tool — NO-LIFT

**Source:** `wayland-hermes/agent/tools/browser_tool.py` (2596 LOC,
surgical scan only — symbols + header read, body not inspected).

**Decision:** Do not port to `wcore-tools`. The browser surface is
already owned by the dedicated `wcore-browser` crate, which is the
correct semantic home per the crate map (multi-backend browser tool
family with ARIA-tree-first surface, `BrowserPolicy` network
boundary, and `BrowserSupervisor` lifecycle).

**Surface mapping (Hermes → wcore-browser):**

| Hermes function | wcore-browser owner |
|----------------|---------------------|
| `browser_navigate` | `BrowserOp::Navigate` via `BrowserTool` |
| `browser_snapshot` | `BrowserOp::Snapshot` (ARIA-tree-first) |
| `browser_click` | `BrowserOp::Click` (ref-selector) |
| `browser_type` | `BrowserOp::Type` |
| `browser_scroll` | `BrowserOp::Scroll` |
| `browser_back` | `BrowserOp::Back` |
| `browser_press` | `BrowserOp::Press` |
| `browser_console` | `BrowserOp::Console` |
| `browser_get_images` | `BrowserOp::GetImages` |
| `browser_vision` | `BrowserOp::Vision` |
| `_browser_eval` / `_camofox_eval` | `BrowserOp::Eval` (provider-dispatched) |
| `_create_local_session` / `_create_cdp_session` / `_get_session_info` | `SessionCtx` + `BrowserTool::ensure_session` per-sub-agent isolation |
| `_ensure_cdp_supervisor` / `_stop_cdp_supervisor` / `_cleanup_inactive_browser_sessions` / `_reap_orphaned_browser_sessions` / `_browser_cleanup_thread_worker` / `_emergency_cleanup_all_sessions` | `BrowserSupervisor` (lifecycle owner) |
| `_resolve_cdp_override` / `_get_cdp_override` / `_get_dialog_policy_config` / `_allow_private_urls` / `_is_local_mode` / `_is_local_backend` / `_get_cloud_provider` | `BrowserProvider` impls (Camoufox primary, chromiumoxide fallback, Browserbase cloud) + `BrowserPolicy` network boundary |
| `_find_agent_browser` / `_browser_install_hint` / `_discover_homebrew_node_dirs` / `_merge_browser_path` / `_requires_real_termux_browser_install` | Provider-internal binary discovery (out of scope for tool layer) |
| `_run_browser_command` / `_extract_relevant_content` / `_truncate_snapshot` / `_extract_screenshot_path_from_text` | `BrowserProvider` execution path + `OpResult` formatting |
| `_maybe_start_recording` / `_maybe_stop_recording` / `_cleanup_old_screenshots` | Provider-internal recording / artifact housekeeping |
| `reset()` global session map | `BrowserTool.sessions` per-instance `Mutex<HashMap>` |
| `_update_session_activity` | `BrowserSupervisor` activity tracking |
| `_get_vision_model` / `_get_extraction_model` | Provider-internal model selection (config-cascade in Wayland) |
| `_get_command_timeout` / `_socket_safe_tmpdir` | Provider-internal env knobs |

**Where wcore-browser already lives:**

`crates/wcore-browser/src/tool.rs` (`BrowserTool` impl) plus
`op.rs`, `policy.rs`, `provider.rs`, `supervisor.rs` — see crate map
entry for `wcore-browser` in AGENTS.md ("Multi-backend browser tool
family (Camoufox primary, chromiumoxide fallback, Browserbase cloud);
ARIA-tree-first surface; BrowserPolicy network boundary;
BrowserSupervisor lifecycle"). The plugin-side mirror
(`wayland-browser`) goes through `wcore-plugin-api` per audit F2 with
no direct `wcore-browser` dependency.

**Architectural reason for placement:**

Per AGENTS.md ("place it in the **lowest crate where it semantically
belongs**"), browser automation belongs in its own dedicated crate
because (a) it has a substantially larger surface than the file-level
tools, (b) it carries multi-process supervisor + network-policy
concerns that don't apply to other `wcore-tools` members, and (c)
adding browser code to `wcore-tools` would force every consumer of
the tool layer (including thin CLI configurations) to pull in
provider + supervisor + policy machinery. Keeping `wcore-browser`
separate preserves the "single responsibility per crate" boundary.

**Genuinely-unique features check:**

Surgical scan surfaces no functionality wcore-browser lacks:
- Sub-agent isolation: covered by `BrowserTool.sessions` map +
  `ensure_session` (sub-agent ↔ `SessionCtx` 1:1).
- Cancellation: covered by `execute_with_ctx` racing
  `ctx.cancel.cancelled()` (500ms max per S2).
- Cloud + local backends: covered by `BrowserProvider` trait with
  Camoufox/chromiumoxide/Browserbase impls.
- ARIA-tree snapshot: explicit design point of `wcore-browser`
  ("ARIA-tree-first surface").
- Recording / vision / image extraction: covered by `BrowserOp`
  variants dispatched through the provider.

If a specific operation is later identified that `wcore-browser`
genuinely lacks (e.g. a CDP-only escape hatch with no provider
mapping), it should be filed as a cross-tier follow-up against
`wcore-browser` — never added to `wcore-tools`, which would violate
both the dependency-graph layering and the "no duplicate code across
crates" rule.

**Conclusion:** No source change in `wcore-tools` for T3-3.5.
`wcore-browser` already owns this surface end-to-end.
## T3-3.5 browser_cdp_tool — NO-LIFT

**Source:** `wayland-hermes/agent/tools/browser_cdp_tool.py` (564 LOC)

**Decision:** Do not port to `wcore-tools`. The browser tool surface
sits in `wcore-browser` (READ-ONLY at this sub-wave boundary), and
adding a raw CDP passthrough there would violate a locked design
constraint. See "Forbidden by design lock" below.

**What `browser_cdp_tool.py` is:**

A single `browser_cdp(method, params, target_id, frame_id, timeout,
task_id)` tool that sends arbitrary Chrome DevTools Protocol commands
over a WebSocket — the escape hatch for browser ops not covered by the
typed `browser_navigate` / `browser_click` / `browser_console` surface
(native dialogs, iframe-scoped evaluation, cookie/network control,
low-level tab management, etc.).

Public surface (per `grep -n "^def "`):

- `browser_cdp(...)` — the tool handler (stateless WS connect path).
- `_browser_cdp_via_supervisor(...)` — routes `frame_id`-scoped calls
  through Hermes's CDP supervisor for OOPIF Runtime.evaluate.
- `_resolve_cdp_endpoint()` — delegates to `tools.browser_tool._get_cdp_override`.
- `_cdp_call(...)` — opens a fresh `websockets` connection per call.
- `_browser_cdp_check()` — feature-flag gate.

**Forbidden by design lock (§5.16 / REV-2 audit F6):**

`wcore-browser/src/op.rs` declares:

```rust
//! `BrowserOp` enum — the v1 tool-surface. **No `Evaluate` variant** per
//! design §5.16 (REV-2 audit F6 lock).
//!
//! The locked-variant-count guard + forbidden-name scan live in
//! `tests/op_enum_test.rs`; touching this enum requires bumping
//! [`BROWSER_OP_LOCKED_VARIANT_COUNT`] AND re-auditing §5.16's Evaluate-ban
//! rationale.
pub const BROWSER_OP_LOCKED_VARIANT_COUNT: usize = 18;
```

A raw CDP passthrough is precisely the kind of "Evaluate-class" escape
hatch §5.16 bans. The intent is that the curated `BrowserOp` surface
(ARIA-ref-driven Navigate / Snapshot / Click / Fill / Press / Read /
Screenshot / etc.) stays small, typed, and auditable — exactly so the
LLM cannot bypass it with arbitrary `Runtime.evaluate` or
`Network.setExtraHTTPHeaders`-style calls. Lifting `browser_cdp` would
require either (a) a 19th locked variant explicitly contradicting
§5.16, or (b) a sibling raw-CDP tool registered alongside `BrowserOp`
— which is the same constraint violation in a different shape.

**Overlap with existing wcore-browser CDP infrastructure:**

- `crates/wcore-browser/src/backends/chromium.rs` is THE CDP backend.
  It uses `chromiumoxide` for typed CDP dispatch — e.g.
  `chromiumoxide::cdp::browser_protocol::page::CaptureScreenshotFormat`,
  `Accessibility.getFullAXTree` for Snapshot, `DOM.setFileInputFiles`
  for Upload. CDP is consumed but NEVER exposed raw to the LLM.
- The `BrowserPolicy` network boundary and `BrowserSupervisor`
  lifecycle (per the crate map in `AGENTS.md`) also assume that all
  ops flow through `BrowserOp` so policy can gate them. A raw CDP
  method takes a free-form method name + JSON params — the policy
  layer can't meaningfully gate `Network.continueInterceptedRequest`
  vs `Runtime.evaluate` without re-parsing the whole CDP protocol,
  which defeats the boundary.
- No existing `BrowserOp` variant, no `BrowserAdapter` method, no
  plugin-API mirror corresponds to a raw CDP passthrough. The
  `grep -rn "raw_cdp\|browser_cdp\|cdp_call"` over
  `crates/wcore-browser/` and `crates/wcore-agent/` returns zero hits.

**Hard isolation boundary (sub-wave 5 rule):**

This sub-wave declares `wcore-browser/*` READ-ONLY. Even if §5.16 did
not lock the enum, the right home for a raw CDP escape hatch is inside
`wcore-browser` (it needs the backend's existing WebSocket, the
supervisor's frame-session map, and the policy layer), not
`wcore-tools` — and changes to `wcore-browser` are out of scope for
T3-3.5.

**Hermes-side coupling that does NOT carry:**

- `_browser_cdp_via_supervisor` requires Hermes's `CDPSupervisor` with
  its `attach_frame` / `evaluate_in_frame` / per-task session
  registry. `wcore-browser::supervisor` is a different abstraction
  (process / lifecycle), not a CDP session manager.
- `_resolve_cdp_endpoint` delegates to `tools.browser_tool._get_cdp_override`,
  which reads `BROWSER_CDP_URL` env var and `browser.cdp_url` config
  key set by Hermes's `/browser connect` slash command. The Wayland
  CLI has no equivalent `connect` command surface, and the cdp_url
  config field is not part of the Wayland config schema.
- Camoufox (the primary backend per AGENTS.md crate map) is REST-only
  and explicitly does not expose CDP — per the tool's own docstring:
  *"The Camofox backend is REST-only and does not expose CDP."* So
  even after a port, `browser_cdp` would only work on the chromiumoxide
  fallback path, narrowing its utility further.

**Recommendation for future:**

If a raw CDP escape hatch is ever desired:

1. First re-open §5.16 / REV-2 audit F6 with explicit design
   justification (auditability, policy-gating story, threat model
   delta vs the typed `BrowserOp` surface).
2. If §5.16 is amended, the surface belongs inside `wcore-browser`
   (alongside `BrowserAdapter`), gated by `BrowserPolicy` with a new
   "raw_cdp" capability flag, and surfaced through the
   plugin-API mirror — not as a standalone `wcore-tools` tool.
3. Bump `BROWSER_OP_LOCKED_VARIANT_COUNT` and update
   `tests/op_enum_test.rs` per the existing guard's contract.

That is an engine-design decision, not a Tier-3 tool port.

**Conclusion:** No source change in T3-3.5 sub-wave 5. The §5.16
op-enum lock and the read-only `wcore-browser` boundary both forbid
the natural port target. Documented here so future sub-waves don't
re-do the overlap survey.
## T3-3.5 browser_dialog_tool — NO-LIFT

**Source:** `wayland-hermes/agent/tools/browser_dialog_tool.py` (148 LOC).

**Decision:** Do not port. Sub-wave branch preserves this note only.

**What the source is:**

A thin response-only shim over a CDP-supervisor surface. The tool
itself does essentially nothing — it:

1. Looks up `SUPERVISOR_REGISTRY[task_id]` (the CDP supervisor that
   tracks `pending_dialogs`).
2. Calls `supervisor.respond_to_dialog(action, prompt_text, dialog_id)`.
3. JSON-wraps the result.

It is gated on `_browser_cdp_check()` (delegated to
`browser_cdp_tool._browser_cdp_check`) so it appears/disappears in
lockstep with `browser_cdp`.

**Why the port cannot land here:**

- `wcore-browser` is **READ-ONLY** for this sub-wave. The prerequisite
  backend surface — a `BrowserSupervisor` that captures native JS
  dialogs into a `pending_dialogs` queue and exposes
  `respond_to_dialog(...)` — does not exist. Verified:

  ```
  $ grep -rni "dialog" crates/wcore-browser/
  (no matches in source)
  ```

  The existing `BrowserSupervisor` (`crates/wcore-browser/src/supervisor.rs`,
  used by `adapter.rs` and `tool.rs`) is a lifecycle supervisor —
  session-end notifications, not a `Page.javascriptDialogOpening`
  listener.

- Without the supervisor surface, a ported `browser_dialog` tool would
  unconditionally return the "No CDP supervisor is attached" branch —
  a permanent stub. Violates the NO STUBS rule.

- The gate (`_browser_cdp_check`) has no Rust counterpart either —
  `browser_cdp_tool` is not in this sub-wave's scope. Adding a
  schema-only `browser_dialog` with no activation predicate would
  surface the tool to the LLM in cases where there is no backend at
  all, the opposite of the hermes gating contract.

**What a real port requires (out of scope for T3-3.5):**

A coordinated change set across two crates:

1. `wcore-browser` (currently READ-ONLY):
   - `supervisor::Dialog { id, dialog_type, message, default_prompt }`
   - `BrowserSupervisor::pending_dialogs() -> Vec<Dialog>`
   - `BrowserSupervisor::respond_to_dialog(action, prompt_text,
     dialog_id) -> Result<DialogResolution, _>`
   - CDP `Page.enable` + `Page.javascriptDialogOpening` /
     `Page.handleJavaScriptDialog` wiring in the chromiumoxide /
     Browserbase backend paths.
   - Camoufox-path explicit "not supported" branch.

2. `wcore-tools` (this crate):
   - `browser_dialog` `ToolSpec` with the same schema (action /
     prompt_text / dialog_id).
   - Gate predicate that mirrors the hermes `_browser_cdp_check` —
     blocked until `browser_cdp` itself lands.

3. Plugin-mirror: `wayland-browser` would also need a
   `BrowserDialogToolSpec` mirror through `wcore-plugin-api`, since
   `wcore-browser` is not allowed as a plugin dep (audit F2).

These cuts cross the explicit READ-ONLY boundary of this sub-wave and
also depend on `browser_cdp_tool` landing first. Properly a paired
`wcore-browser` + `wcore-tools` epic once the supervisor's dialog
queue is designed.

**Snapshot coupling:**

The hermes contract requires the response payload to be visible to
the LLM via `browser_snapshot.pending_dialogs[]`. Wayland's
`browser_snapshot` (in `wcore-browser`) currently has no
`pending_dialogs` field — adding it is part of the
`wcore-browser`-side work above, not something a tool-layer-only port
can stub around.

**Conclusion:** No source change in T3-3.5. Real port is a paired
`wcore-browser` + `wcore-tools` epic, blocked on the supervisor
dialog-queue design and on `browser_cdp` landing first. Branch
preserved (note-only commit) so the orchestrator can record the
decision.
## T3-3.5 browser_camofox — NO-LIFT

**Source:** `wayland-hermes/agent/tools/browser_camofox.py` (613 LOC).

**Decision:** Do not port to `wcore-tools` (nor anywhere else). Full
overlap with the existing **Tier-mid `wcore-browser` Camoufox backend**,
which per `AGENTS.md` is the documented PRIMARY browser provider for
Wayland and is READ-ONLY in this sub-wave.

**Existing Wayland implementation:**

`crates/wcore-browser/src/backends/camoufox.rs` (445 LOC) already
implements the Camoufox sidecar HTTP client:

- `CamoufoxBackend::new(base_url)` / `with_policy(base_url, policy)` —
  constructs the reqwest client with `BrowserPolicy::reqwest_redirect_policy`
  installed so 3xx hops are policy-checked.
- `default_url() == "http://localhost:9377"` — same default as Hermes
  `_DEFAULT_TIMEOUT` / `CAMOFOX_URL` config.
- `impl BrowserProvider for CamoufoxBackend` — wires `BrowserOp`
  variants (Navigate/Snapshot/Click/Fill/Screenshot/State/Network/
  Console) to `POST /sessions/<id>/{navigate,snapshot,click,fill,
  screenshot}`, `GET /sessions/<id>/{state,network,console}`, with the
  policy re-check on `final_url` to close SECURITY-v0.2.0 BLOCKER #3
  (one-shot policy bypass via redirects).
- Monotonic `snapshot_counter` keyed into `OpResult`, matching the
  ARIA-tree-first surface required by `AGENTS.md` for `wcore-browser`.
- wiremock-backed test strategy at the sidecar URL — no real Camoufox
  install needed for CI (mirrors Hermes' decoupling of test from binary
  install).

**Hermes → Wayland surface mapping:**

| Hermes (`browser_camofox.py`)             | Wayland equivalent                                  |
|-------------------------------------------|-----------------------------------------------------|
| `get_camofox_url()` / `is_camofox_mode()` | `CamoufoxBackend::default_url()` + `BrowserSupervisor` provider selection in `wcore-browser` |
| `check_camofox_available()`               | reqwest probe inside backend init; supervisor health check |
| `get_vnc_url()`                           | n/a — VNC is a sidecar-side debug surface, not an agent contract |
| `camofox_navigate(url, task_id)`          | `BrowserOp::Navigate { url }` → `POST /sessions/<id>/navigate` with policy re-check on `final_url` |
| `camofox_snapshot(full, task_id, …)`      | `BrowserOp::Snapshot` → `POST /sessions/<id>/snapshot` + `decode_snapshot` to `RawAriaNode` |
| `camofox_click(ref, task_id)`             | `BrowserOp::Click { ref }` → `POST /sessions/<id>/click` |
| `camofox_type(ref, text, task_id)`        | `BrowserOp::Fill { ref, text }` → `POST /sessions/<id>/fill` |
| `camofox_scroll(direction, task_id)`      | `BrowserOp` scroll variant on the same sidecar API |
| `camofox_back(task_id)`                   | `BrowserOp` back variant on the same sidecar API |
| `camofox_press(key, task_id)`             | `BrowserOp` key-press variant on the same sidecar API |
| `camofox_close(task_id)`                  | `BrowserProvider::close_session` → `DELETE /sessions/<id>` |
| `camofox_get_images(task_id)`             | `BrowserOp::Screenshot` → `POST /sessions/<id>/screenshot` |
| `camofox_vision(question, annotate, …)`   | n/a — vision Q&A is a model-side concern, routed through the LLM provider layer not the browser backend |
| `camofox_console(clear, task_id)`         | `BrowserOp` console variant → `GET /sessions/<id>/console` |
| `_get_session` / `_ensure_tab` / `_drop_session` (`ContextVar`-scoped per-task session map) | `BrowserSession` lifecycle on `BrowserSupervisor` — owns session creation, ID issuance, and drop semantics in Rust without Python's `ContextVar` idiom |
| `_post` / `_get` / `_get_raw` / `_delete` (`requests` helpers) | reqwest client on `CamoufoxBackend`, with `BrowserPolicy::reqwest_redirect_policy` installed (strictly stronger than Hermes — Hermes' `requests` follows redirects without a policy gate) |

**Architectural reasons NO-LIFT is the only correct call here:**

1. **`AGENTS.md` declares Camoufox the PRIMARY backend of `wcore-browser`**
   (crate map row: "Multi-backend browser tool family (Camoufox primary,
   chromiumoxide fallback, Browserbase cloud); ARIA-tree-first surface;
   BrowserPolicy network boundary; BrowserSupervisor lifecycle"). Porting
   a parallel implementation into `wcore-tools` would create a second
   Camoufox client outside the `BrowserProvider` trait hierarchy —
   immediate violation of the no-duplicate-code rule in `AGENTS.md`.

2. **`wcore-browser` is READ-ONLY in this sub-wave** per the task brief.
   Any incremental improvements to the Rust Camoufox backend (e.g.
   adopting Hermes' `_managed_persistence_enabled` flag or the
   `_ensure_tab(about:blank)` bootstrap path) must land via a
   `wcore-browser`-owned sub-wave, not a `wcore-tools` lift.

3. **The Hermes module is a tool-surface façade, not a backend.**
   Functions named `camofox_navigate` / `camofox_click` etc. are the
   names of *agent tools* in the Hermes registry — they wrap the same
   sidecar HTTP calls Wayland's `CamoufoxBackend` already wraps. In
   Wayland, the equivalent agent-side surface is `BrowserTool`
   (defined per the `wayland-browser` plugin crate per `AGENTS.md`)
   dispatching to `wcore-browser::BrowserProvider`. No new tool entry
   in `wcore-tools` is appropriate; that crate hosts Read/Write/Edit/
   Bash/Grep/Glob/Spawn — not browser surfaces.

4. **The `ContextVar`-scoped per-task session map** (Hermes lines 139,
   169, 190) is a Python idiom for preventing cross-session bleed in
   the gateway request pipeline. Wayland's `BrowserSession` /
   `SessionCtx` already owns this concern via explicit
   `BrowserSupervisor` lifecycle — the same pattern used by Tier-1
   `wcore-config`'s ApprovalBridge correlation IDs. A Python-style
   context-var registry would not translate.

5. **`camofox_vision` (Hermes line 506) is a model-side concern, not
   a browser-backend concern.** It packages a screenshot + a question
   into an LLM call. The Wayland equivalent flows through
   `wcore-providers` (image content block) + the existing browser
   `Screenshot` op, not through the browser provider trait itself.
   Lifting it into `wcore-tools` would violate the "no hardcoded
   provider quirks" rule from `AGENTS.md` — it presumes the OpenAI
   chat-completions image format.

**What might look portable but isn't:**

- `_safe_skills_path` / `_safe_filename`-style sanitizers — Hermes
  passes refs and task IDs through to the sidecar verbatim; the sidecar
  enforces ref scoping. Wayland's policy gate is `BrowserPolicy` and
  the request signing happens at the reqwest layer.
- `get_vnc_url()` — VNC is an operator-debug surface exposed by the
  sidecar's `/health` endpoint. Wayland's host-integration story is
  the JSON stream protocol (`wcore-protocol`), not a VNC URL leak from
  the browser backend.
- The `_managed_persistence_enabled()` env-driven storage-mode flag —
  Camoufox sidecar persistence is a sidecar-binary configuration
  (CLI flags / env vars on the sidecar process), not an agent-side
  concern. The Wayland operator configures the sidecar directly.

**Cross-tier follow-ups (filed, not done here):**

- If a future Wayland wave decides to surface VNC URLs as part of an
  operator-debug protocol event, that addition belongs in
  `wcore-protocol` events + a `wcore-browser` health-probe helper —
  not in `wcore-tools`.
- The `camofox_vision` Q&A flow, if desired, is a `wcore-agent` +
  `wcore-providers` integration (screenshot → image content block →
  model call), wired through `BrowserTool` rather than a separate
  surface. Tracked outside the T3 tool-port wave.

**Conclusion:** No source files added. `browser_camofox.py` is the
Python expression of the same Camoufox sidecar HTTP client that
`crates/wcore-browser/src/backends/camoufox.rs` already implements,
with stronger security guarantees (redirect-policy gating, SSRF
hardening) than the Hermes module. Lifting any helper would either
duplicate the existing backend or split the Camoufox surface across
two crates in violation of `AGENTS.md`.
---

## T3-3.5 browser_supervisor — NO-LIFT (orthogonal concern; wcore-browser owns surface)

**Source:** `wayland-hermes/agent/tools/browser_supervisor.py` (1362 LOC)
**Target home (if any):** `crates/wcore-browser/src/supervisor.rs` (READ-ONLY this worktree)

**Decision:** NO-LIFT in `wcore-tools`. The Rust supervisor already
exists in `wcore-browser` and owns the supervisor surface for the
engine. Any gap-fill belongs to a wcore-browser follow-up, not a
Tier-3 tool port.

### Surface comparison (names overlap, concerns are orthogonal)

| Concern | hermes `browser_supervisor.py` | wcore-browser `supervisor.rs` |
|---|---|---|
| Layer | **CDP-protocol supervisor** | **OS-process supervisor** |
| Transport | Persistent CDP WebSocket per `task_id` | Local PID tracking + HTTP `/health` |
| Tracks | `PendingDialog`, `DialogRecord`, `FrameInfo`, `ConsoleEvent`, `SupervisorSnapshot`, per-target session attachment | `BackendHandle { session_id, pid, parent_pid }`, child/parent PID liveness |
| Subscribes to | `Page.*`, `Runtime.*`, `Target.*`, `Fetch.requestPaused` (dialog bridge) | n/a — out-of-process watch via `kill(pid, 0)` / `tasklist` |
| Enforces | Dialog policy (`must_respond` / `auto_dismiss` / `auto_accept`) via injected `alert/confirm/prompt` bridge + `Fetch` interception | Orphan reaping (SIGTERM child when host parent dies), `kill_on_drop(true)` on launch |
| Snapshot caps | `FRAME_TREE_MAX_ENTRIES=30`, `FRAME_TREE_MAX_OOPIF_DEPTH=2`, `CONSOLE_HISTORY_MAX=50`, `RECENT_DIALOGS_MAX=20` | n/a — no in-browser state |
| Registry | `_SupervisorRegistry` keyed by `task_id` | `BrowserSupervisor` (single, holds `Vec<BackendHandle>`) |
| Public API | `respond_to_dialog`, snapshot read, dialog-bridge XHR endpoint (`wayland-dialog-bridge.invalid`) | `register`, `on_session_end`, `live_sessions`, `start_reaper`, `healthcheck`, `launch_camoufox` |

The two files **share a name but solve unrelated problems**:
- Hermes solves *"observe and respond to in-browser dialog/frame/console
  events over CDP."*
- wcore-browser solves *"keep the OS child process from leaking when
  the host crashes."*

### Why nothing lifts to `wcore-tools`

1. **Layer mismatch.** Both are mid-crate engine concerns (browser
   backend lifecycle). `wcore-tools` is the Tier-3 *tool surface*
   layer — neither variant belongs here.
2. **wcore-browser is READ-ONLY in this sub-wave** per the standard
   forbidden file list and audit F2 (plugins have no `wcore-browser`
   dep). Even the in-scope wcore-browser variant is off-limits to
   modify here.
3. **No stub-and-orphan port.** The CDP-supervisor concern requires:
   - persistent `tokio-tungstenite` WebSocket per session
   - CDP `Page`/`Runtime`/`Target`/`Fetch` domain subscriptions and
     auto-attach event handling
   - injected `addScriptToEvaluateOnNewDocument` dialog bridge + Fetch
     URL-pattern interception of `wayland-dialog-bridge.invalid`
   - `BrowserPolicy` integration to gate the dialog-bridge host
   - per-target attach/detach reconciliation against the frame tree

   These are wcore-browser-internal cuts. Lifting any helper into
   `wcore-tools` would be a stub against an absent backend.

### Possible follow-up (out of scope for T3-3.5)

If the CDP-event-supervisor concern is genuinely desired, it should
be planned as a wcore-browser internal epic co-located with the
Camoufox CDP client. Suggested cut:

- A new `wcore-browser::cdp_supervisor` module alongside
  `supervisor.rs` (process supervisor) — keep names disambiguated
  (`CdpSupervisor` vs the existing `BrowserSupervisor` for the
  process side).
- Wires through `BrowserPolicy` for the dialog-bridge host
  allowlist + `BrowserSupervisor` for child-process anchoring.
- Surfaces a `Snapshot { dialogs, frame_tree, recent_console }`
  consumed by the existing tool layer (today's `browser_snapshot`
  return path inside the wayland-browser plugin's
  `BrowserToolSpec` mirror).

That is a wcore-browser change, not a wcore-tools change, and is
**explicitly deferred** here.

**Conclusion:** No source change in T3-3.5. wcore-browser already owns
the supervisor surface; the CDP-event-supervisor concern (if pursued)
is a wcore-browser follow-up, not a Tier-3 tool port.

## T3-3.6 openrouter_client — NO-LIFT (orchestrator-applied)

**Source:** `wayland-hermes/agent/tools/openrouter_client.py` (44 LOC)

**Decision:** Do not port. Sub-agent's branch carried no commit (working
tree clean) — orchestrator-applied this note post-merge so the analysis
is preserved.

**What the hermes module does:**

Thin caching wrapper around `agent.auxiliary_client.resolve_provider_client(
"openrouter", async_mode=True)`. Exposes `get_async_client()`,
`check_api_key()` (env var), `reset()` (test-only).

**Why NO-LIFT:**

1. **No OpenRouter provider in `wcore-providers`.** The Rust crate ships
   Anthropic, OpenAI, Bedrock, Gemini, Vertex — no OpenRouter. References
   exist only as a pricing-table entry (`wcore-pricing/pricing.toml`) and
   doc-comments in `vision_tools.rs` / `video_analyze_tool.rs`.
2. **`wcore-providers/*` is READ-ONLY** in this sub-wave per task brief.
   Adding an OpenRouter provider is Tier-1 territory.
3. **No central auxiliary router exists in Rust yet.** Hermes' module
   wraps a Python-side `resolve_provider_client` singleton; there is no
   Rust equivalent to wrap.
4. **Caching wrapper is moot in Rust.** Providers already manage their
   own cheap-clone `reqwest::Client` via `wcore-providers/src/http_client.rs`.
   Call sites (`vision_tools.rs`, `video_analyze_tool.rs`) consume the
   `LlmProvider` trait directly — no singleton helper needed.

**Recommended cross-tier follow-up:**

When a Tier-1 wave adds an OpenRouter provider to `wcore-providers/`,
expose the OpenAI-compatible surface natively through the existing
`LlmProvider` trait. No separate `openrouter_client` helper in
`wcore-tools/` is justified — that would duplicate provider-routing
logic that belongs in `wcore-providers`.

**Conclusion:** No source change. Branch deliberately empty; future
OpenRouter provider work picks up from this note + the hermes source.

## T3-3.6 xai_http — NO-LIFT (orchestrator-applied)

**Source:** `wayland-hermes/agent/tools/xai_http.py` (12 LOC)

**Decision:** Do not port. Sub-agent placed analysis in a non-standard
`.planning/tier3/` location; orchestrator-applied this canonical note
to keep the convention.

**What the hermes module does:**

Single function `wayland_xai_user_agent() -> str` returning
`f"Wayland-Agent/{__version__}"` — a User-Agent string for direct xAI
HTTP calls.

**Why NO-LIFT:**

1. **No xAI/Grok provider exists** in `crates/wcore-providers/src/`
   (providers ship: anthropic, bedrock, gemini, openai, vertex).
2. **No shared User-Agent infrastructure** in wcore-providers — adding
   one for a single absent provider would be premature abstraction.
3. **`wcore-providers/*` is READ-ONLY** this sub-wave; xAI provider is
   Tier-1 scope.
4. **Trivial Rust equivalent**: a one-line `format!("Wayland-Agent/{}",
   env!("CARGO_PKG_VERSION"))` — no helper warranted for one `format!`
   call.

**Recommended cross-tier follow-up:**

Tied to a future `wcore-providers` xAI provider addition. If/when
multiple providers need a Wayland-branded UA, extract to
`wcore-providers/src/http_client.rs` at that point — not speculatively
now.

**Conclusion:** No source change.

## T3-3.6 code_execution_tool — NO-LIFT (orchestrator-applied)

**Source:** `wayland-hermes/agent/tools/code_execution_tool.py` (1416 LOC).
Sub-agent placed NOTES.md at the worktree root rather than in
`crates/wcore-tools/`; orchestrator-applied this canonical note.

**What the hermes module does:**

A **Programmatic Tool Calling (PTC)** multiplexer with two transports:
- **Local (UDS):** parent spawns child Python that calls back via Unix
  socket (POSIX-only).
- **Remote (file-RPC):** ships script + stub module to Docker / SSH /
  Modal / Daytona / Browserbase; polls request/response files.

Hermes' own docstring: *"collapsing multi-step tool chains into a single
inference turn."*

**Why NO-LIFT — Wayland already covers this:**

| Hermes capability | Wayland surface |
|---|---|
| Multi-step tool chain in one turn | `crates/wcore-tools/src/script.rs` (F13 ScriptTool) |
| Sandbox tool allow-list | `script::ALLOW_LIST` (Read/Write/Edit/Grep/Glob/Bash/RepoMap) |
| Output reference between steps | `${stepId.json.pointer}` json-pointer refs |
| Output truncation | `max_output_lines` |
| Approval-gate destructive ops | `step.approval_required` |
| Single shell command | `crates/wcore-tools/src/bash.rs` |
| Credential exfil guard | Wave-SA `denylist()` RegexSet |

Wayland's ScriptTool is **strictly safer**: refuses arbitrary Python in
exchange for a deterministic allow-listed step DSL with json-pointer-
only refs (no arithmetic, no shell, no expression language). Remote-
sandbox shipping has no Wayland analog or roadmap pull. Same pattern as
T3-3.2.4 file_operations NO-LIFT.

**Conclusion:** No source change. ScriptTool already owns the
multi-step concern; remote-sandbox shipping is out of scope.

## T3-3.6 terminal_tool — NO-LIFT (orchestrator-applied)

**Source:** `wayland-hermes/agent/tools/terminal_tool.py` (1717 LOC).
Sub-agent placed NOTES.md at the worktree root; orchestrator-applied
this canonical note.

**What the hermes module does:**

A multi-backend execution multiplexer (local / docker / modal / ssh /
singularity / daytona) with VM lifecycle, idle-reap threads, task-scoped
env overrides, sudo flows, and Modal-gateway routing.

**Why NO-LIFT — same pattern as code_execution_tool, file_operations,
process_registry:**

Wayland has zero of those backend layers. `crates/wcore-tools/src/bash.rs`
is the only surface that maps and it already covers local foreground
execution with **stronger security** than terminal_tool:
- RegexSet credential denylist (Wave-SA).
- Centralized cross-platform shell helper (`wcore_config::shell::
  shell_command_argv`) — argv mode prevents shell-metacharacter
  interpretation of LLM-supplied input.

**What bash.rs lacks intentionally:**
- Background-task mode (would need a `ProcessRegistry` — see T3-3.4.2
  NO-LIFT note for why that's also out of scope).
- Multi-backend (docker/modal/ssh/etc.) — no Wayland equivalent of the
  `tools.environments.local` abstraction.

**Future-hook sketch:**

If a remote execution backend is ever added (e.g. `wayland-remote-backend`
plugin), the right cuts are: (a) a `wcore-types::ExecutionBackend` trait
sibling to `Spawner`, (b) `bash::BashTool` opt-in dispatch to the
trait when configured, (c) policy/credential checks at the
`wcore-config` layer. Translating Python is not the path — write a
Rust-native registry from scratch.

**Conclusion:** No source change. bash.rs (+ ScriptTool) cover local
execution with stronger security; remote-backend multiplexing is out
of scope for Wayland's architecture.

## T3-3.7 team_invoke / team_list — NO-LIFT

**Source:** `wayland-hermes/agent/tools/team_invoke.py` (351 LOC).

**What the hermes module does:**

Two model-facing tools (`team_invoke`, `team_list`) registered into the
`wayland_teams` toolset. Both are thin JSON adapters over a separate
**TEAMS panel** subsystem (PRD-2 §8.1):

- `team_invoke` calls `teams.executor::invoke_team(team_id, prompt, …)`
  against the global team registry. A "team" is a registered multi-agent
  panel with members, a synthesis mode (`synthesize` / `split_decision`
  / `foreman_picks` / `sequential_debate`), a chair, debate rounds,
  tier overrides, and disagreement-axis extraction. The return value is
  a `TeamResult` frozen dataclass containing `MemberResponse`,
  `ChairDecision`, and `DebateRound` sub-dataclasses + cost + warnings.
- `team_list` calls `teams.registry::list_teams(status=…)` and projects
  each team into `{id, name, description, category, members_count,
  synthesis_mode, default_tier, tags, plugin_id}`.

Both tools depend on three subsystems that **do not exist** in wcore:
1. `teams.executor` — the multi-agent panel orchestrator (chair logic,
   debate rounds, synthesis modes, disagreement-axis extractor,
   cost-gate, spend-journal integration).
2. `teams.registry` — global team registry + `suggest_team_ids` fuzzy
   match for "did-you-mean" hints.
3. `wayland_cli.agent_registry` — v2 AgentRegistry for resolving
   `<plugin>:<agent_id>` reference members.

**Why NO-LIFT — overlap survey vs delegate / spawn / dispatcher:**

Hermes already has `delegate_task` (single-task or batch fan-out of
isolated subagent children) and Wayland mirrors that as
`crates/wcore-tools/src/delegate.rs` (sub-wave 3.1.3) + `SpawnTool` in
`wcore-agent` (sub-wave 1). Those cover the *fan-out subagent* primitive
end to end via `wcore_types::spawner::Spawner` trait dispatch.

`team_invoke` is **not** that primitive. It is the model-facing entry
to a *panel registry with chair / debate / synthesis modes* — a
qualitatively different orchestration layer that lives strictly above
`Spawner`. The 351-LOC tool file is purely a JSON adapter; the actual
logic is in `teams/executor.py` + `teams/registry.py` + chair / debate
helpers which are out of scope for this sub-wave's budget (≤18 calls)
and not on the v0.6.2 roadmap.

Porting only the tool shell would require a stub `invoke_team()` that
either returns `executor_unavailable` on every call (a pure stub — bans
explicitly forbid this) or invents a parallel TEAMS subsystem from
scratch (massively out of budget and out of scope).

**What is preserved by existing tools:**

- Single-task delegation with isolated context → `DelegateTool`
  (`delegate.rs`).
- Parallel fan-out of subagents with summary aggregation →
  `DelegateTool` batch mode + `SpawnTool`.
- Subagent depth limits, blocked-tools, credential pool isolation →
  `wcore-agent::spawn_tool` + `AgentSpawner`.

What is **not** preserved (and therefore not regressed by NO-LIFT,
because it never existed in wcore):
- Pre-registered multi-agent panels with named chairs / debate rounds.
- Synthesis-mode dispatch (synthesize / split_decision / foreman_picks
  / sequential_debate).
- Disagreement-axis extraction across member responses.
- `tier_override` / `expose_disagreement_override` panel-level knobs.
- `teams.registry::suggest_team_ids` fuzzy match.

**Future-hook sketch:**

When Wayland eventually grows a TEAMS subsystem, the right cuts are:
1. A `wcore-teams` crate sibling to `wcore-skills` / `wcore-memory`
   that owns `TeamRegistry`, `TeamSpec`, `TeamResult`, and the
   synthesis-mode enum.
2. A `TeamExecutor` trait in `wcore-types` (mirrors `Spawner`) so the
   tool can stay in `wcore-tools` without inverting the dep graph.
3. `TeamInvokeTool` + `TeamListTool` in `wcore-tools` parameterised by
   `Arc<dyn TeamExecutor>` injected at CLI bootstrap (mirrors
   `DelegateTool`'s `Arc<dyn Spawner>` wiring).
4. Plugin authors register panels via `wcore-plugin-api` extension
   points (similar to how `wayland-ijfw` exercises every `register_*`
   surface).

Translating the Python adapter is not the path — write Rust-native
TEAMS support from scratch when the panel-orchestration layer lands.

**Conclusion:** No source change. `DelegateTool` + `SpawnTool` already
own the subagent fan-out primitive; the panel-registry / chair / debate
/ synthesis-mode layer above them is a future Tier-4-scope feature, not
a tool port. Porting `team_invoke` without the TEAMS executor would be
a pure stub.

## T3-3.7 kanban_tools — NO-LIFT (orchestrator-applied)

**Source:** `wayland-hermes/agent/tools/kanban_tools.py` (876 LOC). Sub-
agent committed directly to trunk at `77c14cf` (bypassing the merge gate)
AND placed its analysis at `.blackboard/t3-3/SUBWAVE-7-KANBAN-TOOLS-NO-LIFT.md`
instead of canonical NOTES.md — orchestrator-applied this canonical
entry and removes the stray file in the same commit.

**Decision:** Do not port. `kanban_tools.py` is a thin tool-surface
shim — 100% of its semantics delegate to `wayland_cli.kanban_db`, a
SQLite-backed dispatcher subsystem that does not exist in wayland-core
(`grep -rln "kanban" crates/` → 0 hits).

**Tool-by-tool backend dependency (in `kanban_db.py`):**

| Handler | Backend dependency |
|---|---|
| `_handle_show` | `get_task`, `list_comments`, `list_events`, `list_runs`, `parent_ids`, `child_ids`, `build_worker_context` |
| `_handle_complete` | `complete_task` + `HallucinatedCardsError` audit + `expected_run_id` |
| `_handle_block` | `block_task` run-lifecycle FSM transition |
| `_handle_heartbeat` | `heartbeat_claim` (TTL extend) + `heartbeat_worker` — guards `release_stale_claims` reclamation |
| `_handle_comment` | `add_comment` thread |
| `_handle_create` | `create_task` w/ parents, tenant, triage, workspace_kind enum, idempotency_key, skills, dispatcher-driven `todo→ready` promotion |
| `_handle_link` | `link_tasks` w/ cycle + self-link rejection |

**Surrounding subsystems the tools rely on but core lacks:**

- Dispatcher process model (`WAYLAND_KANBAN_TASK`, `WAYLAND_KANBAN_RUN_ID`,
  `WAYLAND_KANBAN_CLAIM_LOCK` env contract).
- Profile-config integration (`load_config().toolsets`) for orchestrator gating.
- Multi-tenant scoping (`WAYLAND_TENANT`).
- Audit-event pipeline (every mutation lands a row in the event table).
- TTL-cached `check_fn` registration in the tool registry.

**Why not "thin extension of `todo.rs`":**

`crates/wcore-tools/src/todo.rs` is an in-process
`Arc<Mutex<TodoStore>>` scoped to a single agent session, with four
statuses, no IDs that survive a restart, no inter-process semantics,
no dependency graph, no claims, no audit trail. Kanban is
cross-session / cross-process / cross-tenant orchestration. Different
problem domain; not a superset.

**Why not "stub the tool surface against an in-memory map":**

The value of these tools *is* the semantics (dependency-gated
`todo→ready` promotion, hallucinated-card audit on complete, claim-
TTL heartbeat-vs-reclaim race, idempotency-key dedup, cycle detection
on link, multi-tenant isolation). A `HashMap<TaskId, Task>` behind
the schema strings would be a façade that lies to the model.

**Honest port scope (out of T3 budget):**

1. New crate `wcore-kanban` (Cargo workspace add).
2. SQLite schema port (tasks / runs / comments / events / claims / links).
3. Dispatcher process (separate binary or `wcore-cli` subcommand).
4. Profile-config toolset wiring + env-var gating contract.
5. THEN the 7-tool shim against that backend.

Estimated >2000 LOC + new external CLI surface. Tracked separately if
the kanban subsystem is ever scoped for wayland-core.

**Conclusion:** No source change. Kanban subsystem itself is a
future Tier-4 lift.

## T3-3.8 memory_tool — NO-LIFT

**Source:** `wayland-hermes/agent/tools/memory_tool.py` (666 LOC).

**Decision:** NO-LIFT in `wcore-tools` for sub-wave T3-3.8.

**Surface comparison.** The Hermes `memory_tool` is a flat,
file-backed curated-MD store:

- Two targets: `memory` (agent's personal notes -> `MEMORY.md`) and
  `user` (user profile -> `USER.md`).
- Three actions: `add(target, content)`, `replace(target, old_text,
  new_content)`, `remove(target, old_text)` — substring-match on
  short unique tokens, no IDs.
- Hard character budget per target (2200 / 1375), entry delimiter `§`,
  file-locked (`fcntl` on Unix / `msvcrt` on Windows).
- "Frozen snapshot" prefix-cache discipline — system prompt captures
  state at session start; mid-session writes are durable but only
  visible to the *next* session.

The Wayland `wcore-memory` crate is categorically richer:

- 5-partition x 3-tier model (episodic / semantic / procedural /
  user-model / consolidation across `session` / `project` / `global`).
- Typed entries: `Episode`, `Fact`, `Procedure`, `UserModel`.
- `MemoryApi` write surface: `record_episode`, `assert_fact`,
  `upsert_procedure`, `update_user_model(key, val, tok)`,
  `record_skill_use`. Read surface: `search`, `get_episode`,
  `user_model`, `list_procedures`, `top_procedures`. Lifecycle:
  `dream_now`, `compact`.
- Frozen-snapshot / prefix-cache concern is satisfied by
  `wcore-memory/src/prompt.rs` + `v2_prompt.rs`.
- Embeddings, gating, CDC, audit, partitioning, retrieval ranking —
  all already production-grade and bound through
  `PartitionDispatcher`.
- Session-side read tool is already bound: `SessionSearchTool`
  (T3-3.1.7, `crates/wcore-tools/src/session_search.rs`) shims
  `MemoryApi::search`.

**Why NO-LIFT — Wayland already covers this through a categorically
richer architecture:**

1. **Intent overlap, shape mismatch.** Hermes `add/replace/remove`
   over a flat MD file targets the same intent as Wayland
   `assert_fact` / `update_user_model` (durable cross-session
   knowledge), but through structured typed assertions, not
   free-text substring-matched entries. Lifting Hermes shape on top
   of Wayland would regress the data model.

2. **`MEMORY.md` / `USER.md` semantics aren't expressible against
   `MemoryApi`.** `MemoryApi` has no "render the live curated
   markdown block" exit, no per-target char budget, and no
   substring-match edit. Building those would be `wcore-memory`
   work — and `wcore-memory/*` is Tier-2 territory and READ-ONLY in
   T3-3 (forbidden list).

3. **Frozen-snapshot pattern already lives in `wcore-memory`.** The
   prompt-cache discipline (`prompt.rs`, `v2_prompt.rs`) is the
   wayland-core equivalent of Hermes's `format_for_system_prompt` +
   `_system_prompt_snapshot` pair. There is no missing wiring at
   `wcore-tools` layer.

4. **Read path is already bound.** `SessionSearchTool`
   (T3-3.1.7) is the existing tool-surface bridge to
   `MemoryApi::search`. A second tool that also talks to
   `MemoryApi` for writes would need to commit a write-side tool
   design (single `memory` action-dispatched verb? per-partition
   tools? `assert_fact` vs `update_user_model` discrimination by
   action arg?) that belongs with the consumer architecture, not
   ported as a thin shim against an incompatible source shape.

5. **Same pattern as prior NO-LIFTs.** This mirrors the rationale in
   T3-3.4 registry (NOTES line 296), T3-3.4 credential_files (line
   360), T3-3.6 code_execution_tool (line 1342), T3-3.6
   terminal_tool (line 1380), and T3-3.7 team_invoke (line 1422):
   Wayland's richer Tier-1/Tier-2 architecture obsoletes the Hermes
   file-shape surface. A thin tool-shim would either parrot a worse
   data model or invent a new shape that should be designed against
   `MemoryApi`, not transliterated from the legacy MD store.

**Why not "ship a thin `memory` tool against `assert_fact` +
`update_user_model`":**

Doing so would commit (a) a tool-name (`memory`) that overlaps with
the partition concept inside `wcore-memory`, (b) an action-dispatch
grammar (`add` / `replace` / `remove`) that does not map cleanly to
typed write ops (`Fact` upsert by id vs `UserModel` key-value set),
and (c) per-target char budgets / substring-match edits that are not
concepts `MemoryApi` exposes. Each of those demands
`wcore-memory`-side design work that is explicitly out of scope for
T3-3 (forbidden list).

**What is preserved by the existing Wayland stack:**

- Durable cross-session knowledge -> `MemoryApi::assert_fact`,
  `update_user_model`, `record_episode`.
- User profile -> `UserModel` + `update_user_model`.
- Read-side recall in tools -> `SessionSearchTool` (T3-3.1.7).
- Prompt-cache discipline -> `wcore-memory/src/prompt.rs` +
  `v2_prompt.rs`.
- Char-budget-like backpressure -> `compact(target_tokens)` +
  `dream_now()` consolidation.

**What is not preserved (and is not regressed by NO-LIFT, because the
loss is intentional):**

- Flat `MEMORY.md` / `USER.md` text files. Wayland uses the
  structured partition DB; users who want a Markdown export should
  request a separate `wcore-memory` export entry point.
- Substring-match `replace` / `remove` over free-text entries. Wayland
  identifies writes by typed id (`FactId`, `UserModel` key,
  `ProcedureId`), not by short unique substrings.
- 2200 / 1375 character budgets. Wayland uses `compact` against
  token budgets, not per-target char caps.

**Conclusion:** No source change in T3-3.8. `wcore-memory` is the
architectural successor; the read-side tool surface
(`SessionSearchTool`) is already bound; the write-side tool surface
is a Tier-2 design decision against `MemoryApi`, not a Hermes
transliteration. Re-evaluate if a write-side memory tool is ever
scoped — and at that point design it against `MemoryApi`, not
against `memory_tool.py`.
## T3-3.8 skill_manager_tool — NO-LIFT

**Source:** `wayland-hermes/agent/tools/skill_manager_tool.py` (799 LOC).

**Decision:** Do not port. `wcore-skills` already owns the entire
skill lifecycle that this tool wraps. The hermes file is an
agent-facing CRUD shim over `~/.wayland/skills/` (create / edit /
patch / delete + write_file / remove_file for supporting assets);
every primitive it implements is already provided by an existing
wcore-skills module.

**Capability-by-capability coverage:**

| hermes responsibility | wcore-skills coverage |
|---|---|
| Resolve `~/.wayland/skills/` | `paths::user_skills_dir()` |
| Project-level `.wayland-core/skills/` discovery | `paths::project_skills_dirs()`, `discovery::RuntimeDiscovery` |
| Frontmatter validation (name, description, len limits) | `frontmatter.rs` (+ `frontmatter_tests.rs`) |
| Skill name / category validation regex | `types.rs` (canonical naming) |
| Atomic file write w/ temp + rename | `artifacts::write_artifacts` → `wcore_config::atomic_write` |
| Path-traversal guard for supporting files | `artifacts::resolve_under_root` (`PathEscape` error) |
| Security scan on create (matches `skills_guard`) | `audit.rs` |
| Stage / Active / Archived state transitions | `curate.rs` (P4 state machine, F11) |
| Autonomous draft creation from tool-call patterns | `draft.rs` (F10 pattern detector) |
| Skill loading | `loader.rs`, `bundled.rs` |
| Permissions / conditional activation | `permissions.rs`, `conditional.rs` |

`grep -rn "SkillManagerTool\|skill_manager\|skill_manage" crates/`
→ 0 hits (no prior partial port to extend).

**What remains is a thin agent-Tool-trait wrapper:**

The hermes module's `skill_manage(...)` function and its
`SKILL_MANAGE_SCHEMA` are an agent-facing tool-shim that brokers six
subcommands (create/edit/patch/delete/write_file/remove_file) onto
the `user_skills_dir` tree. The shim composes:

- `wcore_skills::paths::user_skills_dir` (location)
- `wcore_skills::frontmatter` (validate input)
- `wcore_skills::artifacts::{render_template,resolve_under_root}` (atomic + escape-safe writes)
- `wcore_skills::audit::scan_skill` (post-write scan)

…behind an `agent::Tool` impl. That impl is a **wcore-agent** concern,
not a wcore-skills concern (wcore-skills is layered below wcore-agent
in the crate graph, and is READ-ONLY for this worktree per the audit
boundary). It does not justify a new module under `wcore-tools/src/`
either — the agent-managed-tool registration surface lives in
wcore-agent's tool host.

**Why not lift now anyway:**

1. `wcore-skills/*` is read-only per the sub-wave 8 contract — even
   re-exporting helpers (e.g. `pub use frontmatter::validate_*`)
   would violate the boundary.
2. The shim has zero unique semantics — every check, every write,
   every scan is already implemented and tested in wcore-skills.
   Reimplementing them in wcore-tools would duplicate code across
   crates (forbidden by AGENTS.md "No Duplicate Code Across Crates").
3. The natural home (wcore-agent tool registry) is out of T3-3.8 scope.

**Cross-tier follow-up (tracked, not implemented here):**

`SkillManagerTool` agent-Tool impl in **wcore-agent** that:

- Imports `wcore_skills::{paths, frontmatter, artifacts, audit}` (no
  new logic — pure composition).
- Registers six subcommands matching the hermes JSON schema
  (`create`, `edit`, `patch`, `delete`, `write_file`, `remove_file`).
- Routes writes under `user_skills_dir()` only; rejects writes to
  bundled / external-discovered skills (mirrors hermes
  `_is_local_skill` guard, which corresponds to `LoadedFrom::User` in
  wcore-skills `types.rs`).
- Estimated effort: ~150 LOC shim, zero wcore-skills changes.

**Conclusion:** No source change. Lifecycle coverage already complete
in wcore-skills; the missing piece is a wcore-agent Tool-trait
wrapper, which is outside the T3-3.8 boundary.

## T3-3.8 skills_tool — NO-LIFT (orchestrator-consolidated)

**Source:** `wayland-hermes/agent/tools/skills_tool.py` (1436 LOC).
Sub-agent placed analysis at `.blackboard/t3-3/t3-3-8-skills-tool-NO-LIFT.md`
(non-canonical location); orchestrator migrates the analysis here and
deletes the stray file in the same commit.

**Decision:** NO-LIFT. `SkillTool` already implements `Tool` in
`crates/wcore-agent/src/skill_tool.rs` (line 97), registered via
`bootstrap.rs:495`. Constructors include fork-mode (`with_spawner`)
that exceeds the hermes scope.

`crates/wcore-skills/` (33 modules — artifacts, audit, bundled,
conditional, context_modifier, curate, discovery, draft, executor,
frontmatter, hooks, loader, mcp, paths, permissions, prioritizer,
prompt, refs, shell, substitution, telemetry, types, watcher) covers
the entire 1436-LOC hermes surface, including `skills_list` /
`skill_view` / `check_skills_requirements` / YAML frontmatter parse /
platforms gating / progressive disclosure tiers / `~/.wayland/skills/`
discovery.

The `Tool` trait surface in `wcore-tools/src/lib.rs:239,248` explicitly
documents methods that "Only SkillTool overrides" — confirming
SkillTool is the engine's authoritative wrapper. `wcore-skills/*` is
READ-ONLY for this sub-wave; the canonical SkillTool surface is also
pre-existing W6 territory and not touched.

**Conclusion:** No source change. Engine surface already exceeds
hermes parity through `wcore-agent::skill_tool::SkillTool` +
`wcore-skills` modules.

## T3-3.8 skill_provenance — NO-LIFT (orchestrator-consolidated)

**Source:** `wayland-hermes/agent/tools/skill_provenance.py` (78 LOC).
Sub-agent placed analysis at `.blackboard/t3-3/t3-3-8-skill-provenance-NO-LIFT.md`;
orchestrator migrates here and deletes the stray.

**What the Python source does:** exposes a `ContextVar[str]` named
`skill_write_origin` (default `"foreground"`) plus
`set/reset/get_current_write_origin` and `is_background_review()`.
Sole producer in hermes is `_spawn_background_review` in `run_agent.py`
(curator autonomous self-improvement fork); sole consumer is the
`skill_manage create` path which tags newly-written skills as
agent-created so the curator can later auto-prune them.

**Why NO-LIFT — no producer, no consumer in engine:**

1. **No background-review fork concept.**
   `grep -rn "background_review\|spawn_background\|review_fork\|self_improve\|memory_write_origin" crates/`
   returns zero hits. `Curator` exists at `wcore-skills/src/curate.rs`
   but does not spawn an agent loop; `draft.rs:164` uses a hardcoded
   `created_by: "main-agent-f10"`.

2. **No skill-write tool path that branches on provenance.**
   `wcore-skills::draft` (staged drafts), `wcore-skills::curate`
   (archival), `wcore-agent::skill_tool::SkillTool` (list/view/run)
   — none branch on per-call write-origin. No call site to consume
   the signal.

3. **`ContextVar` pattern explicitly rejected for this codebase.**
   T3-3.4 `env_passthrough` documented: *"Wayland runs as a single-
   tenant CLI / library, so we use a process-global `RwLock` instead."*
   Skill provenance is even less of a multi-tenant concern.

4. **`wcore-skills/*` is READ-ONLY this wave.** The only place a
   provenance enum could meaningfully wire (next to `SkillSource` in
   `wcore-skills/src/types.rs` or against `draft.rs:created_by`) is
   off-limits. A helper-only module in another crate would have no
   producer, no consumer, and only round-trip tests — meeting the
   working definition of a stub. **NO STUBS.**

**When to revisit:** alongside a background-curator-fork feature, when
producer + consumer land together. Rust equivalent is ~20 LOC via
`tokio::task_local!` + `Drop`-based scope (no token plumbing).

**Conclusion:** No source change. Deferred until the background-
curator-fork feature is scoped.
