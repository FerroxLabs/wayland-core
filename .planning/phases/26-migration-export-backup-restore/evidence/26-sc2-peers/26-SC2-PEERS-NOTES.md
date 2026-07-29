# 26-SC2-PEERS — running notes

Lane `lane/26-sc2-peers`. Base `5be910561f688c75d39492e7b982d6e100772a64`
(`gh/plan/f20-unified-audit-repair`), SHA asserted against `git ls-remote gh` before
any work. Every number here from an unproxied tool (`/usr/bin/grep`, `/usr/bin/find`,
`/usr/bin/git`).

## T0 — the brief's premise re-verified at HEAD

Brief claim: peer coverage is **2 of 4**; no importer for `grok-build` or `gemini-cli`.

```
$ /usr/bin/grep -rniE "grok|gemini" crates/wcore-cli/src/migrate/     → 0      (known-negative)
$ /usr/bin/grep -rniE "hermes|openclaw" crates/wcore-cli/src/migrate/ → 150    (known-positive, same matcher)
$ crates/wcore-config/src/portability/mod.rs:45 enum PeerSource       → Hermes, OpenClaw  (2 variants)
$ /usr/bin/find crates/wcore-cli/src/migrate -type f
    content.rs hermes.rs mod.rs openclaw.rs provenance.rs quarantine.rs rollback.rs select.rs
```

**"2 of 4" HOLDS at HEAD.** The known-positive licenses the zero: the same matcher on the
same tree returns 150 for the two peers that do exist, so the instrument is alive.

One incidental drift vs 26-SC2-SUMMARY §6, which said *"`migrate` still has no rollback
(G3, untouched here)"*: `crates/wcore-cli/src/migrate/rollback.rs` (468 lines) EXISTS at my
base. Lane `26-sc3-rollback` landed since that summary was written. Not my scope; recorded
so no one re-derives it.

## T1 — peer tree layouts, measured read-only

**NOTHING was executed or mutated inside `/Users/seandonahoe/dev/resources/`.** `find`,
`grep`, `head` only.

### grok-build — SpaceXAI's `grok` terminal coding agent (Rust)

Source repo, not an install. No `.grok/` dir in the tree. 6 `SKILL.md`, all under
`crates/codegen/xai-grok-shell/skills/` — those are the agent's **built-in bundled**
skills, not user content. Need to find the product's real user-config path from its source.

### gemini-cli — Google's `gemini` CLI (TypeScript)

Has a real, canonical `.gemini/` config directory checked into the repo root:
```
.gemini/settings.json
.gemini/config.yaml
.gemini/commands/*.toml          (13, incl. nested github/ and oncall/)
.gemini/skills/<name>/SKILL.md   (13 at repo root)
```
24 `SKILL.md` total tree-wide; the other 11 are vendored test-data / builtin package
content, not user skills. `.gemini/skills/` carries helper scripts:
`async-pr-review/scripts/*.sh`, `ci/scripts/ci.mjs`, `pr-address-comments/scripts/*.js`
— i.e. the F26-SC2-M1 helper-carrying class is present here and must be exec-bit stripped.

## T2 — each peer's format, GROUNDED IN ITS OWN SOURCE

`openclaw.rs` grounds its format in the peer's `src/config/paths.ts` constants rather
than in the repo's directory shape. Same discipline applied here. **The `.gemini/` tree
checked into the gemini-cli repo is a PROJECT config, not the user home** — inferring the
importer from it would have been the error the brief warns about.

### grok-build → `grok` (SpaceXAI terminal coding agent, Rust)

| what | value | grounded at |
|---|---|---|
| home | `$GROK_HOME`, else `~/.grok` (dunce-canonicalized) | `crates/codegen/xai-grok-config/src/paths.rs:28-47` |
| config | `<home>/config.toml` (layers: `/etc/grok/managed_config.toml`, `<home>/managed_config.toml`, `<home>/config.toml`) | `xai-grok-config/src/lib.rs:4-6`, `loader.rs:84` |
| model | `[models] default = "<id>"` | `xai-grok-shell/src/agent/config.rs:975-977`; sole writer `util/config/settings_writes.rs:88-104` |
| MCP | `[mcp_servers.<name>]` — `command`/`args`/`url`/`env`/`headers`/`enabled`/timeouts | `util/config/mcp.rs:419,1056,1185,1485` |
| credential | `<home>/auth.json` | `auth/storage.rs` via `auth/flow.rs:1951` |
| skills | `<home>/skills/<name>/SKILL.md` | `builtin.rs:75,134,150` |
| vendor skills | `<home>/bundled/`, `<home>/server-skills/` | `inspect/mod.rs:1820,1828` |
| personas | `<home>/personas/*.toml` | `config/mod.rs:279-315,385-392` |
| memory | `<home>/memory` | `grok_home().join("memory")` |

### gemini-cli → `gemini` (Google, TypeScript)

| what | value | grounded at |
|---|---|---|
| home | `$GEMINI_CLI_HOME`, else `os.homedir()`, then `.gemini` | `packages/core/src/utils/paths.ts:13,22-28`; `config/storage.ts:54-60` |
| settings | `<home>/settings.json` | `config/storage.ts:78-80` |
| model | `model.name` | `packages/cli/src/config/settingsSchema.ts:1062-1079` |
| MCP | root `mcpServers: {}`; `command`/`args`/`env`/`cwd`/`url`/`httpUrl`/`headers`/`type` | `settingsSchema.ts:161-174`; `core/src/config/config.ts:478-514` |
| skills | `<home>/skills/` | `config/storage.ts:101-103` |
| commands | `<home>/commands/*.toml` | `config/storage.ts:97-99` |
| memory | `<home>/GEMINI.md` | `core/src/tools/memoryTool.ts:11` |
| agents / policies | `<home>/agents/`, `<home>/policies/` | `config/storage.ts:109-119` |

## T3 — what the EXISTING machinery already covers, checked not assumed

- `peer_skill_roots()` (`quarantine.rs:703`) already includes `home.join("skills")`, which is
  the user skills root for **both** new peers. **No new skill root is required**, and both
  peers' skills therefore inherit `scan_skill_root`'s recursion, symlink refusal and depth bound.
- `write_tree()` (`content.rs:665-676`) calls `strip_execute_bits` on **every** file of **every**
  imported tree. It is peer-agnostic, so the F26-SC2-M1 mitigation carries to a new peer by
  construction rather than by re-implementation. **To be PROVEN on disk, not asserted.**
- `scan_peer_memory()` already scans `home/memory` → grok's memory dir is covered.
  Gemini's memory is a single root file `GEMINI.md` → **not** covered, needs adding.
- `scan_peer_personas()` looks only for `SOUL.md` → grok's `personas/*.toml` **not** covered.

### Deliberate exclusions (same argument as F26-GRADE-M1's `hermes-agent/` carve-out)

`<grok home>/bundled/` is the vendor's shipped catalog and `<grok home>/server-skills/` is
server-pushed; neither is user-authored and both are re-obtained by installing the product.
They are **counted and named** in `deferred_other`, never imported — importing them would
inflate an "imported" count without migrating anything of the user's.

## T4 — both peers are REALLY INSTALLED on this Mac, so the proof uses real homes

The brief pointed at `~/dev/resources/{grok-build,gemini-cli}`, which are the products'
**source repositories**. Sean also has both products installed:

```
~/.grok    config.toml  auth.json  version.json  skills/(5)  bundled/(5)
           marketplace-cache/(78 SKILL.md, 188 exec-bit files)  vendor/ sessions/
~/.gemini  settings.json  GEMINI.md  package.json  agents/(33) commands/(1) extensions/(1)
           google_accounts.json  mcp-oauth-tokens.json  installation_id   NO skills/
```

Both treated **read-only**; nothing inside either was executed or modified, and the staging
script asserts that by digesting every source path before and after
(`SOURCE-INTEGRITY: PASS`).

Real facts the live import must be judged against, measured before running anything:

| | grok | gemini |
|---|---|---|
| model declared | **NO** — `config.toml` has `[cli] [marketplace] [ui]`, no `[models]` | **NO** — no `model` block |
| MCP servers | 0 | **14** (4 `type`+`url`, 10 stdio) |
| credential store | `auth.json` (OIDC session) | `google_accounts.json`, `mcp-oauth-tokens.json`, `installation_id`; **no `oauth_creds.json`** |
| auth type declared | — | `security.auth.selectedType = "gemini-api-key"` |
| version declared | `version.json` → `0.2.103` | `package.json` = `{"type":"commonjs"}`, no version |
| user skills | 5, **all 0644** | none |

Two consequences, both honest rather than convenient:

- **`peer_version` was WRONG for grok.** It probed `VERSION`/`version`/`MANIFEST.json` and
  returned `None` against a home that plainly declares `"version": "0.2.103"`. Found only by
  driving the real tree. Fixed in `facb3c7b`.
- **Credential secrecy.** Every `mcpServers` entry in the real `settings.json` was inspected
  key-by-key: only `command`/`args`/`type`/`url`, and the single `env` holds `PATH`. So
  `settings.json` ships verbatim with no secret. The credential-store FILES are never copied
  — same-named placeholders are written so the by-reference path is exercised, and every
  substitution is printed by the staging script.

## T5 — the hostile case, per peer, from REAL trees

| peer | hostile payload | where it came from | modes at source |
|---|---|---|---|
| gemini | `skills/async-pr-review/scripts/{async-review,check-async-review}.sh`, `skills/ci/scripts/ci.mjs`, `skills/pr-address-comments/scripts/fetch-pr-info.js` | the gemini-cli project's own `.gemini/skills/`, which is the identical layout `Storage.getUserSkillsDir()` returns | **4 × 0755** |
| grok | `skills/brand/scripts/{extract-colors,inject-brand-context,validate-asset}.cjs` | `~/.grok/marketplace-cache/.../skills/brand/`, placed where a marketplace INSTALL puts it | **3 × 0755** |

**grok's `~/.grok/skills/` carries no exec-bit helper today** (measured: 0 of 5). The hostile
payload is therefore a real marketplace skill's real bytes at its real install destination,
not a naturally-occurring one — stated because the difference matters.

7 exec-bit helpers survived transport to hetzner intact (verified there with `find -perm -u+x`).

