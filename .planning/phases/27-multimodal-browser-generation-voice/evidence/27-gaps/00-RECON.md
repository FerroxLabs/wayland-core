# 27-gaps — reconnaissance, recorded before any long-running step

Lane `lane/27-gaps`. Merge-base captured once, quoted everywhere:
`BASE=0b16f86791a707c614c14a1e1ee9f1a0c17d27d9`.

This file exists because a transport stall destroyed an hour of the same
investigation with nothing committed. Everything below is written down
before the next remote call, not after it.

## Hosts, probed not assumed (2026-07-27)

| Host | Probe | Result |
|---|---|---|
| `SeanD@seandesktop` | `ssh -o BatchMode=yes ... hostname` | `SeanDesktop`, rc=0 |
| `hetzner-dsm` | `hostname; df -h /root; uptime` | `Ubuntu-2404-noble-amd64-base`, 744G free of 1.8T, load 1.60 |

`seandesktop` interpreter inventory (probed by scp'ing a `.ps1` rather than
quoting PowerShell through ssh, which does not survive the transport):

```
python  -> C:\Users\seand\AppData\Local\Programs\Python\Python312\python.exe
python3 -> C:\Users\seand\AppData\Local\Microsoft\WindowsApps\python3.exe
node    -> C:\Program Files\nodejs\node.exe
bash    -> C:\Program Files\Git\bin\bash.exe
tar     -> C:\Program Files\Git\usr\bin\tar.exe
PSVER=5.1.26100.8875   ARCH=AMD64
```

Consequence for C5: **one Python 3 smoke harness runs natively on all three
platforms.** That is why `scripts/f27-packaged-smoke.py` is Python and not a
shell script — it also sidesteps the pipeline-steals-exit-status trap by
reading `subprocess.run(...).returncode` directly.

## The C5 raw material exists

`FerroxLabs/wayland-core` publishes real packaged archives per release.
`gh release list` → latest `v0.12.25` (2026-07-13). Downloaded and executed
natively on the Mac:

```
$ tar -xzf wayland-core-v0.12.25-aarch64-apple-darwin.tar.gz
$ ./wayland-core --version
wayland-core 0.12.25
$ ./wayland-core --build-info
wayland-core 0.12.25 (source 61b79c4)
```

**This is the fact that makes C5 tractable without a cargo toolchain on the
Mac.** Smoking a *packaged artifact* needs no compiler. The phase verdict
recorded "every Linux measurement came from a `cargo build --release` binary
inside a build tree"; a published archive is the thing that was missing.

The existing `post-tag-smoke` job in `.github/workflows/release.yml` asserts
`--version` matches a SemVer and nothing else — for the two aarch64 targets it
does not even execute the binary, it reads the ELF/PE machine field. That is
why C5 reads NOT MET even though a smoke job exists: it exercises no phase-27
surface.

## The C3 surfaces are reachable with zero credentials

The packaged binary exposes two built-in media/web generation subcommands:

- `image` — FluxRouter image generation, `POST /v1/images/generations`
- `fetch` — FluxRouter web_fetch, `POST /v1/fetch`

Measured on the macOS artifact with every credential variable stripped and
`WAYLAND_HOME` pointed at a throwaway directory. Exit codes are the real
process status (captured without a pipeline):

```
$ ./wayland-core image --prompt "a red square" --out out.png
wayland-core image: no Flux API key (set --api-key, $FLUX_API_KEY, or
[providers.flux-router] in config): no Flux API key found
rc=1        out.png: not created

$ ./wayland-core image --prompt "" --out o.png
wayland-core image: image: --prompt must be non-empty
rc=1

$ ./wayland-core fetch https://example.com
wayland-core fetch: no Flux API key (...): no Flux API key found
rc=1
```

All three refuse loudly, name the missing credential, and write nothing.
That is the honest-refusal half of C3 and it is **PASSING** on the shipped
artifact — the phase verdict's "none of the four generation shapes was ever
exercised" is now false for the built-in shape's credential and failure
clauses.

`mcp-serve --transport stdio` answers `tools/list` with no credential at all:

```
initialize + tools/list  ->  3 tools: ['Read', 'Grep', 'Glob']
```

so the discovery clause has a credential-free observation surface too.

### Caution recorded about the exit codes above

`./wayland-core image ... 2>&1 | head -3; echo "rc=$?"` printed `rc=0` for all
three of these, because `$?` is `head`'s status. The rc=1 values above come
from re-running each command with output redirected and `$?` read directly.
This is trap #1 in the lane brief and it fired here on the first attempt.

## HIGH finding, found by following the product's own instructions

When the engine cannot find a provider credential it prints, verbatim:

```
Provider 'anthropic' requires an API key. To use a LOCAL model with Ollama,
select a model id prefixed with `ollama:` (e.g. `ollama:qwen3-coder:30b`)
-- no API key is needed.
```

Followed verbatim on the packaged artifact:

```
$ ./wayland-core --json-stream -m ollama:qwen3-coder:30b
{"type":"error","error":{"code":"init_failed","message":"Engine failed to
start during init: No API key found. ... Provider 'anthropic' requires an
API key. To use a LOCAL model with Ollama, select a model id prefixed with
`ollama:` ... -- no API key is needed. ...","retryable":false}}
rc=1
```

**The instruction is false, and the error that prints it is the error it
claims to resolve.** This is the same defect shape as the
`[browser]` / `[browser.policy]` HIGH that Criterion 2 already carries: a
surface whose stated remediation sends the user in a circle.

Confirmed in source at lane HEAD, so it is not a stale-release artifact:

- `crates/wcore-config/src/config.rs` resolves the API key at step 6 of
  `resolve_inner_from_files` and returns `MissingApiKey` there. Nothing in
  `wcore-config` inspects the model string; `grep -rn "ollama" crates/wcore-config/src/`
  finds only compat presets, a catalog row and a commented-out profile
  example.
- The route the hint describes really does exist, one layer further in:
  `make_plugin_provider_router` in `crates/wcore-cli/src/main.rs:156` claims
  any model matching `starts_with("ollama:")` and hands back a concrete
  `OllamaProvider`. The plugin defaults to enabled
  (`PluginsConfig::is_enabled` → `unwrap_or(true)`).
- So config resolution bails **before** the router it advertises can ever be
  consulted. The capability is built, wired, enabled by default, and
  unreachable by the one instruction that names it.

Why this matters to Phase 27 specifically, beyond being a defect: a working
credential-free local-model path is the difference between exercising the
MCP-only, late-MCP and combined generation shapes for real and not exercising
them at all. Those three shapes need the agent engine to boot, and the engine
will not boot without a provider. It is the keystone for C3.

## Already fixed at base, so not re-litigated

The Criterion 2 HIGH that the phase verdict lists as "open and unfixed" —
`wcore-browser/src/tool.rs` naming `[browser]` where the loader reads
`[browser.policy]` — **is fixed on the integration branch already**, by
another lane. `crates/wcore-browser/src/tool.rs:499` now routes the text
through `config_hint::disabled_by_default_hint()`, with a round-trip test
through the real loader at `wcore-agent/tests/browser_config_hint_roundtrip.rs`.
Successor item 2 from the phase verdict is therefore closed; I did not redo it.

## What is NOT started, stated plainly

Nothing on hetzner: `ls -d /root/wayland-27*` → `NO_27_WORKTREE`. No build,
no test run, no voice probe, no MCP fixture existed at the time of writing.
