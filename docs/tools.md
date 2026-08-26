# Built-in Tools

The agent registers roughly seventy built-in tools. The eight below are the core
file, shell and delegation set — the ones you will see in almost every run, and the
ones the rest of this page documents. The remainder (memory, media, browser, voice,
status and channel tools, plus anything contributed by a plugin or an MCP server)
are registered by the same mechanism, and which of them a given session actually
holds depends on its posture, its config and its plugins.

| Tool | Function | Concurrent |
|------|----------|------------|
| **Read** | Read file contents (with line numbers) | Yes |
| **Write** | Write files (auto-creates directories) | No |
| **Edit** | Precise string replacement | No |
| **Bash** | Execute shell commands | No |
| **Grep** | Regex search file contents (via ripgrep) | Yes |
| **Glob** | Find files by pattern matching | Yes |
| **Spawn** | Spawn sub-agents for parallel tasks | No |
| **ToolSearch** | Load schemas for deferred tools | Yes |

---

## Read

Read file contents with line numbers, similar to `cat -n`.

- Supports `offset` and `limit` parameters for reading file slices
- Auto-detects binary files
- Output format: line-numbered text

## Write

Write content to a file atomically.

- Atomic write: writes to a temp file first, then renames
- Auto-creates parent directories
- Subject to the [unsaved-work guarantee](#unsaved-work-guarantee) below

## Edit

Find and replace exact strings in a file.

- Matches `old_string` exactly and replaces with `new_string`
- Requires a unique match by default; errors on multiple matches
- Use `replace_all` to replace all occurrences
- Subject to the [unsaved-work guarantee](#unsaved-work-guarantee) below

### Unsaved-work guarantee

An agent that rewrites a file from its own picture of the contents can silently
drop a line the user had on disk and had not committed. Write and Edit are
therefore checked at the tool layer, not by asking the model to be careful.

**What holds.** Through **Write**, a line that is on disk and that the tool
cannot prove is recorded in the commit the session started at will not leave
the disk unless the previous contents have first been written to the
repository's own object store *and read back byte-for-byte*. Where no such copy
can be made, the write is refused and nothing changes.

- The baseline is the commit **the session started at**, not current `HEAD`, so
  a commit made mid-session cannot launder unsaved work into it.
- The recovery copy is a loose, unreferenced object in the object store git
  itself names for that repository (`git rev-parse --git-path objects`), made
  with `git hash-object -w`, and only where that store passes the proof below. The tool result prints the
  exact command to get the bytes back:
  `git -C <repo> cat-file blob <oid>`. Nothing is staged, committed or
  referenced. **`git gc` does not remove it.** Measured on git 2.43.0 and
  2.54.0: with `gc.cruftPacks` (default on since git 2.42) gc moves the object
  into a cruft pack and `git cat-file blob` still returns it — still readable
  after six consecutive `git gc` runs. Disposal takes
  `git -C <repo> gc --prune=now`, or an ordinary gc once `gc.pruneExpire` (two
  weeks by default) has passed; `git gc --auto` needs thousands of loose
  objects and never fires for one. Until then the bytes are inside
  `.git/objects` and, being in no commit, that is the only place they exist —
  whether the file was gitignored, merely untracked, or tracked and wholly
  rewritten. They travel with `cp -a`, `tar`, `rsync` and `git clone` of the
  local path, and `git fsck --lost-found` writes them out as a plaintext file;
  `git push` and `git bundle` do not carry them. The tool result says all of
  this.
- **Edit is never refused** — its `old_string` must match the bytes on disk, so
  anything it removes was quoted from disk rather than silently omitted. It
  copies where it can and states plainly when it could not.
- If git cannot be consulted for a repository that plainly exists — dubious
  ownership under `safe.directory`, an unreadable config, no `git` binary — the
  baseline is unknown, so a Write that would drop lines is **refused** rather
  than allowed, and the refusal quotes git's own reason. Writes that do not
  remove existing lines are unaffected.
- Outside any repository there is no object store, so a Write that would drop
  lines is refused for want of anywhere to put a recoverable copy.
- **The copy is only made where it is provably no more exposed than the file
  itself.** One rule, checked per write; where the proof does not hold the
  Write is refused, **nothing is copied**, and an Edit says no copy was made.
  It holds only when all of these do:
  - the repository does not ignore the file. Asked of the ignore *rules* with
    `git check-ignore --no-index`, never of the index: the index is mutable
    mid-session and one `git add -f` on a gitignored `.env` used to file it
    into `.git/objects`, where `git clone <path>` carries it and
    `git fsck --lost-found` writes it out as plaintext. A *tracked* file
    matched by an ignore rule now reads as ignored, which costs a refusal
    rather than a copy;
  - the session's commit records something under the file's own directory, or
    the file is at the repository root — measured live with `$HOME` as a
    dotfiles repository and the private file at `~/work/env.local`;
  - the object store is inside the work tree the file is in. In a **linked
    worktree** the objects belong to the main repository, and in a
    **submodule** to `<super>/.git/modules/<name>/objects`; both would put the
    bytes outside the repository the user is working in, so both are refused;
  - the copy is no wider-permissioned than the file. git writes a loose object
    `0444`, so a `0600` file is refused rather than copied into a
    world-readable object, as is any file whose directory is reachable by
    fewer people than `.git/objects` is.
- **On Windows the proof cannot be made, so the copy is not made.** Measured on
  git 2.54.0.windows.1: under `%USERPROFILE%`, where most Windows repositories
  live, `.git\objects` inherits `(I)(OI)(CI)(RX)` for both AppContainer package
  SIDs — the principals that confine agent subprocesses. The file may carry the
  same inherited ACEs, which would make the copy no more exposed; this code
  cannot demonstrate that, and a copy that cannot be bounded is not made. A
  Windows Write that would drop unrecorded lines is refused.
- **The recovery copy is never scrubbed, and nothing claims it is.** A copy
  with the secret redacted is not a recovery copy — it is lost work wearing a
  disguise. Placement is the only lever, and the two rules above are it. The
  lines a *refusal* quotes back are a different surface and are scrubbed with
  the engine's own `PIIScrubber`.
- **This guard creates no directory anywhere.** Earlier rounds kept snapshots
  under the profile home behind `restrict_dir`/`restrict_file` helpers that
  were no-ops off Unix, so on Windows `%USERPROFILE%\.wayland` inherited
  `CodexSandboxUsers:(OI)(CI)(RX)` and two AppContainer SIDs. That store is
  gone.
- **A pre-image that cannot be read is refused, not treated as an empty file.**
  A permission denied, a directory in the way, or bytes that are not UTF-8 all
  used to produce an empty pre-image, which skipped the check entirely: as an
  unprivileged uid against a root-owned `0600` file in a writable directory,
  the write proceeded and the file went `root:root 0600` to
  `nobody:nogroup 0644`. The one case that still proceeds is bytes that are
  byte-for-byte what the pinned commit records.
- **The file is re-read immediately before the write lands.** The assessment
  runs several `git` processes — measured at 13.5 ms — and a save that arrived
  inside that window was destroyed 12 times out of 12 while the note claimed
  the previous contents were preserved. If the file is not byte-for-byte what
  was judged, the write is refused. That narrows the window to a single
  syscall rather than closing it.
- git is run with `GIT_DIR`, `GIT_COMMON_DIR`, `GIT_WORK_TREE`,
  `GIT_INDEX_FILE`, `GIT_OBJECT_DIRECTORY`, `GIT_ALTERNATE_OBJECT_DIRECTORIES`
  and `GIT_QUARANTINE_PATH` cleared, so an inherited git environment cannot
  redirect either the recovery write or the read-back that verifies it.

**What does not hold.** Three limits, stated because a broader claim would be
untrue:

- **Bash is not covered.** `sed -i`, `>` and `rm` do not route through the tool
  layer and can still destroy unsaved work. The guarantee covers two of the
  three write surfaces.
- **A modified line is not distinguished from a dropped one.** A whole-file
  transformation that renames a symbol occurring on an unsaved line reads as a
  drop and is refused.
- **Non-UTF-8 files** have no line model, so no line of them can be judged.
  They are refused rather than left unprotected, unless their bytes are
  exactly what the pinned commit records.
- **The recovery note hands the model the object id and the `git cat-file`
  command that retrieves the prior contents unscrubbed** — the same contents
  whose *quoted* lines are scrubbed when they appear in a refusal. There is one
  channel to both the user and the model here, and the user needs the command,
  so the exposure is accepted and stated rather than closed.

## Bash

Execute a shell command and return the result.

- **Not covered by the [unsaved-work guarantee](#unsaved-work-guarantee).** A
  command like `sed -i '2d' file` deletes a line the tool layer never sees, so
  uncommitted work can be lost this way even though Write and Edit protect it.
- Default timeout: 120 seconds, max 600 seconds
- Returns exit code, stdout, and stderr
- Interpreter: `sh -c` on Unix, `cmd /C` on Windows. **Windows override** — run
  commands through Windows PowerShell (`powershell -NoProfile -Command`) or
  PowerShell 7+ (`pwsh`) instead of `cmd`:

  ```toml
  [tools]
  windows_shell = "powershell"   # or "pwsh"
  ```

  Or set `WAYLAND_BASH_SHELL=powershell` / `=pwsh` at runtime, which overrides
  the config key. Either way it affects the Bash tool only — hook, MCP, and
  skill shells keep `cmd /C`. No-op on Unix.

## Grep

Search file contents with regular expressions.

- Uses `rg` (ripgrep) when available, falls back to `grep -rn` (`findstr` on Windows)
- Supports glob filtering and case-insensitive search
- Results limited to 250 lines

### Ignore and secret policy

The search backend is an implementation detail; the policy is not. Grep applies
the SAME ignore and secret rules to `rg`, `grep` and `findstr` output, so what
comes back does not depend on which binary the host happens to have installed.
(It used to: with ripgrep present a `.env` was skipped, and without it the file's
contents were returned and forwarded to the model.)

**Ignore policy.** For a directory search the reportable file set is enumerated
with ripgrep's own `ignore` crate under its standard filters — `.gitignore`,
`.ignore`, `.git/`, and hidden dotfiles are excluded. Two deliberate points:

- A path named **explicitly** is searched even if hidden. The ignore policy
  governs traversal, not an explicit request. (This is ripgrep's rule too.)
- `.gitignore` is honoured **even outside a git repository**, which ripgrep does
  not do. The file states an intent that should not have to wait on `git init`.

**Secret policy.** Files matching the credential-file list that `Read` and the
OS sandbox already use — `.env` and `.env.*`, `*.pem`, `*.key`, `id_rsa`,
`.npmrc`, `.netrc`, `~/.aws/credentials`, `service-account*.json`, and the rest
— are never reported, and naming one directly is refused with an error rather
than served. Grep returns matched line *content*, which makes it the shortest
exfiltration path of any read tool.

Match content that survives is scrubbed with the same `PIIScrubber` the engine
applies to all tool output, so `API_KEY=…` in an ordinary file comes back as
`[REDACTED:SECRET_ASSIGNMENT]`. Grep has to do this itself: its `path:lineno:`
prefix pushes the assignment off the start of the line, and the scrubber's
credential-assignment rules are line-anchored.

**Withholding is never silent.** Anything the policy removes is counted in a
trailing line, e.g.

```
src/app.rs:12:let key = load();
[Grep policy: 1 secret-shaped file(s) withheld (.env); 3 match(es) in ignored paths]
```

A search whose every hit was withheld reports that line, not "No matches found" —
"could not show you" and "there was nothing" are different answers.

## Glob

Find files matching a glob pattern.

- Standard glob patterns (e.g., `**/*.rs`)
- Results sorted by modification time (newest first)
- Returns up to 100 files

## Spawn

See [Sub-Agent Spawning](advanced.md#sub-agent-spawning) in the Advanced Features guide.

## ToolSearch

Load full schemas for deferred tools so the LLM can invoke them. Deferred tools (from MCP servers with `deferred = true`) are registered by name only — their parameter schemas are not loaded until the LLM calls ToolSearch.

The tool takes exactly one parameter, `query` (a string). There is no `select:`
prefix, no multi-name list form, and no result limit — an earlier version of this
page documented all three and none of them exist.

- `query` is lowercased and matched as a **plain substring** against each deferred
  tool's name *and* its description. `"slack"` matches a tool named `slack_send`
  and also any tool whose description mentions Slack.
- **Every** match is returned, with its full parameter schema. There is no cap and
  no `max_results` parameter, so a broad query against a large MCP registry returns
  a correspondingly large result — prefer a specific substring.
- A query that matches nothing returns no tools; an empty query is an error.

---

## How It Works

```
User input → Build request (system prompt + history + tool definitions)
           → Stream LLM API response
           → Output text to stdout in real-time
           → If LLM returns tool_use → confirm → execute → send result back
           → Loop until LLM stops calling tools
           → Output final reply → save session
```

- Concurrent-safe tools (Read, Grep, Glob) execute in parallel
- Non-concurrent tools (Write, Edit, Bash) execute sequentially
- Tool output is auto-truncated to prevent context window overflow
- Tool output can be compacted (see [Output Compaction](advanced.md#output-compaction))

## Tool Descriptions

Each built-in tool includes a detailed description and usage guidance that is injected into the system prompt. These descriptions help the LLM select the right tool and use it effectively — for example, preferring Grep over Bash for content search, or using Edit instead of Write for modifications.

## Script tool (W4)

The `Script` tool composes N built-in tool calls into one. It is
gated by `capabilities.rpc_tool_script` (W0 slot at events.rs:139);
the engine only registers it when `builtin_tools.script.enabled = true`
in wcore-config (default off).

### DSL

```jsonc
{
  "name": "Script",
  "input": {
    "steps": [
      { "id": "s1", "tool": "Grep", "input": { "pattern": "fn run(" } },
      { "id": "s2", "tool": "Read", "input": { "file_path": "${s1.matches.0.file}" } },
      { "id": "s3", "tool": "Edit",
        "input": { "file_path": "${s2.path}", "old_string": "...", "new_string": "..." },
        "approval_required": true }
    ],
    "max_output_lines": 200
  }
}
```

### Safety rails

- **Allow-list**: Read, Write, Edit, Grep, Glob, Bash, RepoMap. No
  SpawnTool, no recursive Script, no MCP tools, no plugin tools (W4
  scope).
- **Refs are json-path only**: `${stepId.field.subfield}`. No arithmetic,
  no shell, no expression language. Path syntax is `name(.name)*` where
  name is `[A-Za-z0-9_]+`.
- **approval_required: true** returns `is_error: true` with a clear
  message pre-W7 — the destructive step does NOT execute. W7 wires
  the formal `Suspend` event + resume-token round-trip.
- **max_output_lines** truncates the aggregated transcript; default 200.
- **Step failure short-circuits** — no half-applied state.

## RepoMap tool (W4, W3→W4 hand-off)

The `RepoMap` tool wraps `wcore_repomap::RepoMap::build` (shipped
standalone in W3) and `render::render_compact` behind the `Tool`
trait. Default-on per `[builtin_tools.repomap] enabled = true`; opt
out via `wcore.toml`. The tool is read-only by construction — it
walks the directory tree, never writes.

### Schema

```jsonc
{
  "name": "RepoMap",
  "input": {
    "query": "LlmProvider",          // optional substring filter
    "file_limit": 100,               // optional cap on rendered files
    "symbol_limit": 50               // optional cap on symbols per file
  }
}
```

### Behaviour

- `RepoMap::build` is offloaded via `tokio::task::spawn_blocking` so a
  5K-file index doesn't stall the runtime.
- `query` substring-filters `render_compact` output line-by-line
  (case-insensitive). Empty/missing query returns the full compact
  view.
- Output is truncated when it exceeds `file_limit × (symbol_limit + 1)`
  lines as a coarse upper bound; raise the limits for more detail.
- Read-only ⇒ `is_concurrency_safe(...)` returns `true` ⇒ Script may
  invoke `RepoMap` (in the allow-list above) without serialisation
  surprises.

## Browser tool family (W8c.1)

`Browser::*` tools are registered by the `wayland-browser` plugin
(via `wcore-browser`). Every op shares the ARIA-tree surface so
prompt budgets stay bounded.

Available ops (variants of `BrowserOp`):

| Op | Description |
|---|---|
| `Browser::navigate { url }` | Drive the active tab to the given URL. Gated by `BrowserPolicy`. |
| `Browser::snapshot` | Capture the current ARIA tree (default surface for LLM reasoning). |
| `Browser::click { selector }` | Click an element addressed by the ARIA-tree selector. |
| `Browser::type { selector, text }` | Type text into a focused field. |
| `Browser::new_tab { url? }` | Open a fresh tab (optionally pre-navigated). |
| `Browser::download { url }` | Download the resource at `url` to the workspace. |

Available capability flag: `capabilities.browser_suite` (W8c.1). The
engine emits `browser_event` and `browser_policy_denied` while ops
run; see `docs/json-stream-protocol.md` §§1.N+6 and 1.N+7.

### Two things a fresh install needs before a browser op works

Neither is done for you, and both are deliberate. A first browser op on
an untouched install is refused, and the tool card names whichever step
is outstanding.

**1. Install the sidecar.** Core does *not* bundle a browser. It drives
the Camoufox sidecar, a separate npm package:

```bash
npm install -g @askjo/camofox-browser
```

Already have one somewhere else? Point `WAYLAND_CAMOUFOX_BIN` at the
`camofox-browser` executable instead. Auto-download
(`[browser.camoufox_download]`) is off by default and has no built-in
artifact: Core never fetches executable code from the network without an
operator-supplied URL *and* a pinned SHA-256 per platform.

**2. Open the policy.** `[browser.policy]` is fail-closed since v0.2.1 —
`default_action = "deny"` with no `allowed_origins`, so every URL that
reaches `Browser::navigate` / `download` / `new_tab { url }` is refused.
This is the SSRF posture, not an oversight; the fix is to name the
origins you intend to visit, including any search engine you expect the
agent to use:

```toml
[browser.policy]
# Glob patterns supported. This is an allow-list: an origin that is not
# named here stays refused.
allowed_origins = ["example.com", "*.mysite.com"]
```

Or, not recommended (it re-opens the SSRF surface):

```toml
[browser.policy]
default_action = "allow"
```

Put it in a file the loader actually reads — the global
`config.toml` under the app config dir (`wayland-core --config-path`) or
`<project dir>/.wayland-core.toml`. A bare `config.toml` in the working
directory is **not** a config source in any layer.

Loopback (`http://localhost:…`) is refused even by an allow-list; it
needs the separate port-scoped grant at `[browser.policy.loopback]`.

`wayland-core --doctor` reports both steps: a `browser backend` row for
the sidecar and a `browser policy` row directly under it.

## Computer use (W8c.2)

`Cua::*` tools are registered by the `wayland-cua` plugin (via
`wcore-cua`). Every op honours the background-mode invariant: no
foreground-app focus stealing.

Available ops (variants of `CuaOp`):

| Op | Description |
|---|---|
| `Cua::left_click { x, y }` / `right_click` / `middle_click` / `double_click` | Mouse button at screen coords. |
| `Cua::move_to { x, y }` | Move the cursor without clicking. |
| `Cua::drag { from, to }` | Press, move, release between two points. |
| `Cua::type { text }` | Type Unicode text into the focused app. |
| `Cua::key { combo }` | Send a key combo (e.g. `cmd+shift+4`). Blocks against `forbidden_key_combos`. |
| `Cua::screenshot` | Capture the screen; optionally redacted per `CuaPolicy`. |
| `Cua::ax_tree` | Capture the accessibility tree for the foreground app. |
| `Cua::wait { ms }` | Sleep without holding the runtime busy. |
| `Cua::frontmost_app` | Identifier of the current foreground app. |

Available capability flag: `capabilities.computer_use` (W8c.2). The
engine emits `cua_event` and `cua_policy_denied` while ops run; see
`docs/json-stream-protocol.md` §§1.N+8 and 1.N+9.

## IJFW tools (W8c.3)

`ijfw::*` tools are registered by the `wayland-ijfw` anchor plugin.
The tool bodies delegate to the registered IJFW MCP server
(`ijfw-memory`); both names below are addressable by the LLM through
the standard `tool_request` flow.

| Tool | Description |
|---|---|
| `ijfw::ijfw_run` | Run a query through the configured IJFW mode pipeline (smart / fast / deep / manual / brutal). |
| `ijfw::ijfw_update_apply` | Apply an IJFW update diff returned by `ijfw_update_check`. |

## Rollback (W8b F5)

The `Rollback` tool tier produces shadow snapshots of every file an
agent edits during a session (see `FileHistory` in `wcore-tools`).
Operators / hosts can request a `tool_result.metadata.rollback_token`
to checkpoint a state, then re-issue the token later via
`Rollback::restore { token }` to revert. Tokens are scoped to the
session and do NOT persist across restarts.

## Token-cost accounting (W12 B.4-tokens)

`tool_token_bench` (in `crates/wcore-agent/src/bin/`) is the
measurement harness for per-tool token-cost accounting. It dispatches
representative `ToolUse` calls through the production
`execute_tool_calls` path, captures the resulting `ToolResult.content`
strings, and emits a markdown table of
`(chars, heuristic_tokens, scripted_input_tokens, delta)` per tool.

Regenerate the scripted baseline:

```bash
vx cargo run --release -p wcore-agent \
    --bin tool_token_bench \
    --features test-utils
```

Output lands at `docs/tool-token-empirical-<UTC-date>.md`. Live-API
verification (real provider tokenization across Anthropic / OpenAI /
Bedrock / Vertex) is documented in §2 of the same doc and still
requires real credentials to fill in — that path is gated behind the
`live-api` Cargo feature on `wcore-agent` and currently exits with a
runbook pointer.

## Web search backends

The `web` tool (search / extract / crawl) dispatches through a pluggable
`WebBackend`. The active backend is chosen at startup by
`build_web_search_backend()`. **Every selected backend falls back to
DuckDuckGo on failure** (transport error, non-2xx, or no valid results),
so search never hard-fails — except when explicitly disabled.

**Selection order (first match wins):**

| Priority | Trigger | Backend |
|----------|---------|---------|
| override | `WAYLAND_WEB_BACKEND=off` | disabled (no fallback) |
| override | `WAYLAND_WEB_BACKEND=duckduckgo` | DuckDuckGo only |
| override | `WAYLAND_WEB_BACKEND=parallel` | Parallel free → DDG |
| 1 | `FIRECRAWL_API_KEY` (+ optional `FIRECRAWL_API_URL`) | Firecrawl → DDG |
| 2 | `PARALLEL_API_KEY` | Parallel REST → DDG |
| 3 | `TAVILY_API_KEY` | Tavily → DDG |
| 4 | `EXA_API_KEY` | Exa → DDG |
| 5 | `SEARXNG_URL` | SearXNG → DDG |
| 6 | `BRAVE_SEARCH_API_KEY` | Brave → DDG |
| default | *(no keys)* | **Parallel free → DDG** |

`WAYLAND_WEB_BACKEND` is an explicit override that wins over key presence;
`auto` (or unset / unrecognized) runs the ladder in the table order
(firecrawl → parallel → tavily → exa → searxng → brave → ddg), so a configured
key always wins over the keyless default and DuckDuckGo is the final fallback.

**Default (no config):** the engine uses Parallel.ai's free, anonymous Search
MCP (`https://search.parallel.ai/mcp`) — ranked URLs with query-relevant
excerpts, no API key. **Privacy:** your search queries are sent to parallel.ai.
A one-time log notes this on first use; set `WAYLAND_WEB_BACKEND=duckduckgo` to
keep queries on DuckDuckGo, or `=off` to disable web search entirely.

**SearXNG** is gated by `SEARXNG_URL` (your own or a public instance — the
engine ships the connector, not the instance). The instance must be **publicly
resolvable**: requests go through the SSRF-safe client, so a `SEARXNG_URL`
pointing at `localhost` / a private IP is rejected. (A scoped opt-in for
private SearXNG instances is a planned follow-up.)

API keys are redacted from logs / model context by `wcore-safety` PII scrubbing.
