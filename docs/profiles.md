# Isolated Profiles

An **isolated profile** is a self-contained `WAYLAND_HOME`-rooted home with its
own `config.toml`, credentials, OAuth tokens, long-term memory, and skills.
Switching profiles switches *everything* — different API keys, different memory,
different session history — with no cross-contamination between them.

This is a different feature from the `--profile <name>` **config selector**
(named `[profiles.*]` sections inside one config file). See
[Isolated profiles vs. the config selector](#isolated-profiles-vs-the-config-selector)
below — the two share the `--profile` flag and are easy to confuse.

## What an isolated profile is

Wayland Core normally writes all of its state into a single home directory (the
"default home"). An isolated profile is an **independent home directory** that
Wayland Core uses *instead of* the default home for the duration of a launch.
Because the home root is the only thing the engine consults at runtime, swapping
the home swaps the entire environment:

| Store | On-disk entry | Holds |
|-------|---------------|-------|
| config | `config.toml` | Provider, model, MCP servers, all settings |
| credentials | `credentials.toml` / `credentials.enc` / `credentials.kdf.json` | Provider API keys (plaintext or encrypted vault) |
| oauth | `oauth/` | OAuth tokens (e.g. ChatGPT-subscription auth) |
| memory | `memory/` | Long-term cross-session memory |
| skills | `skills/` | Installed and drafted skills |

Everything else the engine writes (`.env`, `logs/`, sessions, `cron/`,
`evolved/`) also lives under the profile home, so it is isolated too. The
profiles control plane reports the five stores above via `profile show`.

## When to use one

- **Multiple accounts / clients.** Keep each client's keys, memory, and session
  history fully separate — there is no shared credential or memory store to leak
  across.
- **Work vs. personal.** Different providers, different defaults, different
  history, one binary.
- **Throwaway / experimental setups.** Create a profile, try a configuration,
  delete the whole home when done.

If you only want to switch *provider/model settings* and are happy sharing one
credential and memory store, you want the config selector instead — see below.

## How a profile is selected at launch

Wayland Core resolves the active home **once**, at process entry, and
materializes it into the `WAYLAND_HOME` environment variable. After that,
`WAYLAND_HOME` is the single source of truth for the rest of the run.

Resolution order (first match wins):

1. **`WAYLAND_HOME` already set** in the environment — an explicit override
   always wins and is never overridden.
2. **`--profile <name>` on the command line** — if a profile directory of that
   name exists, its home becomes `WAYLAND_HOME`.
3. **The active pointer** — the profile recorded by `profile use` (see below).
4. **Otherwise the default home** — the pre-profiles single home.

If a selected name is invalid or its directory does not exist, Wayland Core
prints a warning to stderr and **falls through to the default home** — it never
aborts the launch (so `--help` and the like always run). The one exception is
host mode; see [Host / JSON-stream mode](#host--json-stream-mode).

There are three ways to activate a profile, in order of how "sticky" they are:

```bash
# 1. Per-launch, environment override (most explicit, never second-guessed)
WAYLAND_HOME=/path/to/profiles/work wayland-core "..."

# 2. Per-launch, by name
wayland-core --profile work "..."     # see the caveat in the disambiguation section

# 3. Persistent — set the active pointer once, then launch bare
wayland-core profile use work
wayland-core "..."                    # uses 'work' until you change the pointer
```

## The `wayland-core profile` command family

```
wayland-core profile <SUBCOMMAND>
```

| Subcommand | Purpose |
|------------|---------|
| `create <name>` | Create a new, empty isolated profile |
| `use <name>` | Set the active profile for future launches |
| `list` | List all profiles (active marked with `*`) |
| `show [name]` | Show a profile's home path and which stores it contains (defaults to the active profile) |
| `rename <old> <new>` | Rename a profile (re-points the active pointer if it named `old`) |
| `delete <name>` | Delete a profile and its entire home directory |
| `export <name>` | Copy a profile's home tree to a directory (secrets excluded by default) |
| `import <path>` | Adopt a directory tree as a new profile |

### `create`

```
wayland-core profile create <name> [--base <name>] [--use]
```

Creates a new, empty profile (its own config / credentials / memory).

| Flag | Behavior |
|------|----------|
| `--base <name>` | Record an inheritance marker naming an existing profile. **No state is copied** — and never any secrets. A `profile.toml` with `base = "<name>"` is written; inheritance is resolved at launch in a later release. A missing base is a hard error and leaves no half-made profile behind. |
| `--use` | Also set the new profile active (writes the active pointer). |

```bash
wayland-core profile create work
wayland-core profile create client.acme --use
wayland-core profile create staging --base work      # marker only, no secrets copied
```

### `use`

```
wayland-core profile use <name>
```

Writes the **active pointer** so future launches that pass neither `--profile`
nor `WAYLAND_HOME` use this profile. The profile must already exist (`use` does
not create). The name is case-folded.

```bash
wayland-core profile use work
```

### `list`

```
wayland-core profile list
```

Lists all profiles, sorted, with the active one marked `*`. Reports an empty
state cleanly, and warns if the active pointer names a profile that no longer
has a directory (a dangling pointer you can repair).

### `show`

```
wayland-core profile show [name]
```

Prints the profile's home path and which stores are present (config,
credentials, oauth, memory, skills). Defaults to the active profile when `name`
is omitted. **Never prints secret values** — only whether each store exists.

```bash
wayland-core profile show          # the active profile
wayland-core profile show work
```

### `rename`

```
wayland-core profile rename <old> <new>
```

Renames the profile directory. `new` must not already exist. If the active
pointer currently names `old`, it is re-pointed to `new` so the active selection
follows the rename. A case-only rename (`work` → `Work`) maps to the same
on-disk directory and is a no-op success.

### `delete`

```
wayland-core profile delete <name> [--yes] [--force]
```

Deletes the profile **and its entire home directory**.

| Flag | Behavior |
|------|----------|
| `--yes` | Skip the interactive confirmation prompt. On a non-interactive stdin (no TTY) deletion is **refused** without this flag rather than blocking. |
| `--force` | Allow deleting the **currently-active** profile. Without it, deleting the active profile is refused. When the active profile is deleted, the active pointer is cleared so the next launch falls through to the default home. |

```bash
wayland-core profile delete staging --yes
wayland-core profile delete work --yes --force      # also allowed when 'work' is active
```

### `export`

```
wayland-core profile export <name> [--out <path>] [--include-secrets]
```

Copies the profile's home tree into a plain directory.

| Flag | Behavior |
|------|----------|
| `--out <path>` | Destination directory (created if absent). Defaults to a directory named after the profile (lowercased) in the current working directory. |
| `--include-secrets` | Include secrets in the export. **By default secrets are excluded** — `credentials*` files and the `oauth/` directory are skipped. Passing this prints a stderr warning, because the export then contains live keys. |

Symlinks in the tree are never followed (path-escape defense).

```bash
wayland-core profile export work --out ./work-backup
wayland-core profile export work --out ./work-full --include-secrets   # WARNS — contains keys
```

### `import`

```
wayland-core profile import <path> [--as <name>]
```

Adopts an existing directory tree (for example, a `profile export` tree) as a
**new** profile. The new profile must not already exist.

| Flag | Behavior |
|------|----------|
| `--as <name>` | Name for the imported profile. Defaults to the source directory's name (any trailing `.ext` removed). |

The source must be an existing directory. Symlinks are never followed
(zip-slip / path-escape defense). Secrets in the source are **not** filtered on
import — you are adopting a tree you supplied, so whether it carries keys depends
on how it was exported.

```bash
wayland-core profile import ./work-backup
wayland-core profile import ./some-tree --as work
```

## Profile names

Names become filesystem directory components, so the grammar is strict and
anything ambiguous across platforms is rejected up front (never sanitized):

- Non-empty, at most **64 bytes**.
- Only ASCII letters, digits, `.`, `_`, `-` (this rejects every path separator,
  `:`, spaces, NUL, and all control characters).
- Not all dots (`.`, `..`, `...`), no leading `.`, no leading `-`, no trailing `.`.
- Not a Windows reserved device name (`CON`, `NUL`, `COM1`…, with or without an
  extension) — rejected on every platform so a profile made on Linux stays
  usable on Windows.
- Not `active` (reserved for the control plane).

Names are **case-folded**: `Work` and `work` refer to the same profile on every
platform.

## Where profiles live

Profiles are stored under a control-plane root:

1. `WAYLAND_PROFILES_ROOT` if set — must be an **absolute**, control-char-free
   path (a relative override is ignored so the root never depends on the current
   directory).
2. Otherwise `<os-native config dir>/wayland-core-profiles/` — a sibling of the
   legacy home, so the existing single home is untouched and still serves as the
   default profile.

Each profile is a subdirectory of that root. The **active pointer** is a small
file named `active` at the root (never inside any profile home), holding the
name of the profile to use when neither `WAYLAND_HOME` nor `--profile` is given.
A corrupt or dangling pointer never aborts a launch — it falls through to the
default home.

> The profiles root is resolved independently of `WAYLAND_HOME` by design: a
> profile home is a *child* of the root, so the root must never be computed from
> a home.

## Security and secret handling

- **`export` excludes secrets by default.** `credentials*` files and `oauth/`
  are skipped unless you pass `--include-secrets`, which prints a stderr warning.
- **`create --base` copies no state at all** — only an inheritance marker is
  written. Credentials and OAuth tokens are never inherited at create time.
- **`show` never prints secret values** — only whether each store is present.
- **Symlinks are never followed** during export or import, and on Windows NTFS
  reparse points (junctions) are skipped too, so a hostile tree cannot redirect
  a copy outside its root.
- **Profiles are fully isolated.** Each has its own credential and memory store;
  there is no shared store to cross-write between profiles.

## Isolated profiles vs. the config selector

These are two different features that happen to share the `--profile` flag, and
they are the most common source of confusion.

| | Isolated profile (this page) | `--profile` config selector |
|--|------------------------------|------------------------------|
| What it switches | The entire home: config **and** credentials, OAuth, memory, skills, sessions | Only settings within one config file (provider, model, keys, MCP servers, …) |
| Where it lives | A separate `WAYLAND_HOME`-rooted directory under the profiles root | A `[profiles.<name>]` section inside a single `config.toml` |
| How it's defined | `wayland-core profile create <name>` | A TOML section, with optional `extends` inheritance — see [providers.md](providers.md) |
| How it's selected | Active pointer (`profile use`), `WAYLAND_HOME`, or `--profile` | `--profile <name>` |
| Isolation | Full — separate credentials and memory | None — same home, same credentials and memory |

The config selector is documented under
[Custom aliases and profiles in providers.md](providers.md). A `[profiles.*]`
section bundles provider/model settings for quick switching, for example:

```toml
[profiles.claude-fast]
provider = "anthropic"
model = "claude-haiku-4-5-20251001"

[profiles.claude-deep]
extends = "claude-fast"
model = "claude-opus-4-8"
```

```bash
wayland-core --profile claude-fast "Quick question"
```

> **Sharp edge — the `--profile` flag drives *both* mechanisms.** When you pass
> `--profile <name>`, Wayland Core (1) tries to activate an isolated profile of
> that name (sets `WAYLAND_HOME` if the directory exists, else warns and uses the
> default home) **and** (2) selects the `[profiles.<name>]` config section from
> the resolved config — which **errors** if no such section exists ("Profile
> '<name>' not found in config"). So `--profile work` for an isolated profile
> named `work` only works cleanly if the resolved `config.toml` also has a
> `[profiles.work]` section. To activate an isolated profile **without** also
> selecting a config section, use the active pointer (`wayland-core profile use
> work`, then launch bare) or set `WAYLAND_HOME` directly — neither passes
> `--profile` to the config layer.

## Host / JSON-stream mode

Interactive CLI/TUI use is tolerant: an unresolved `--profile` warns and falls
through to the default home. **Host mode is stricter.** If a host launches
`wayland-core --json-stream` with `--profile` but no `WAYLAND_HOME` was
materialized (the profile could not be resolved to an existing home), the engine
**refuses to start** rather than silently writing the shared default home and
cross-writing another profile's credentials and memory. A host that drives
profiles must set `WAYLAND_HOME` to the profile's isolated home before spawning
the engine, or create the home first with `wayland-core profile create`.

## See also

- [getting-started.md](getting-started.md) — installation, CLI usage, config cascade
- [providers.md](providers.md) — provider setup and the `[profiles.*]` config selector
