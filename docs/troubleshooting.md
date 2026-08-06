# Troubleshooting

## `UNABLE_TO_GET_ISSUER_CERT_LOCALLY` when installing

```
npm error code UNABLE_TO_GET_ISSUER_CERT_LOCALLY
npm error request to https://registry.npmjs.org/@ferroxlabs%2fwayland-core failed,
npm error reason: unable to get local issuer certificate
```

This is Node failing to verify **npm's** TLS certificate, before a single byte of
this package is downloaded. It is not specific to `wayland-core`. Confirm that in
one command:

```bash
npm i -g express
```

If that fails identically, nothing about this package is involved, and no version
of it will install until the trust problem is fixed.

Usual causes:

- **TLS interception.** A corporate proxy or VPN (Zscaler, Netskope and similar),
  or antivirus doing HTTPS scanning, presents its own root certificate. Node ships
  its own CA bundle and does not consult the macOS keychain or the Windows store,
  so a root your OS trusts is still unknown to Node.
- **An old Node** with a stale bundled CA list.
- **A leftover `cafile` or proxy** in `.npmrc`.

Diagnose:

```bash
npm config get cafile proxy https-proxy registry
echo "$NODE_EXTRA_CA_CERTS"
node -v
# who actually signs the registry from this machine:
openssl s_client -connect registry.npmjs.org:443 -servername registry.npmjs.org </dev/null 2>/dev/null \
  | openssl x509 -noout -issuer
```

If the issuer is your employer or an antivirus vendor rather than a public CA,
the connection is being intercepted. Export that root certificate and point Node
at it:

```bash
export NODE_EXTRA_CA_CERTS=/path/to/corporate-root.pem
```

**Do not set `strict-ssl=false` or `NODE_TLS_REJECT_UNAUTHORIZED=0`.** Those do
not fix the trust problem, they switch verification off, which means you can no
longer tell the real registry from whatever is intercepting it.

If npm stays blocked, skip it. Every release ships prebuilt signed binaries for
macOS, Linux and Windows on the
[Releases](https://github.com/FerroxLabs/wayland-core/releases) page, each
verifiable against `wayland-core-checksums.txt`.

## Installed successfully but still on the old version

`npm install -g @ferroxlabs/wayland-core` installs the **`latest`** dist-tag.
Release candidates are published to **`next`** and are deliberately not `latest`,
so a plain install will reinstall the version you already have and report success.

```bash
npm view @ferroxlabs/wayland-core dist-tags   # what each tag points at
npm i -g @ferroxlabs/wayland-core@next        # the release candidate
npm i -g @ferroxlabs/wayland-core@latest      # the current stable
```

If a release candidate has been announced but your installed version has not
moved, check the dist-tags first: an rc that has not been promoted to `latest` is
invisible to anything tracking the stable channel, and the install will still
report success.

## Stale Engine via npx (already-fixed bugs reappear)

```
API error 400: ... tools[0].function.name ...   # or any bug fixed releases ago
```

If you launch the engine with `npx @ferroxlabs/wayland-core` (or `@latest`),
npx caches the resolved package **by spec string** and never re-queries the
registry (npm/cli#2329) — the box freezes on whatever `latest` was the *first*
time you ran it. Check with:

```bash
npx @ferroxlabs/wayland-core --version   # what you're actually running
npm view @ferroxlabs/wayland-core version  # what's actually latest
```

Only an **exact-version** spec is a guaranteed cache miss:

```bash
npx @ferroxlabs/wayland-core@<latest-version> ...
# or install globally and stop depending on the npx cache:
npm i -g @ferroxlabs/wayland-core@latest
```

The launcher also self-heals: it checks the registry in the background (at
most once a day, never blocking a launch) and prints a warning with the exact
pinned command when the cached engine is behind. Opt out with
`WAYLAND_CORE_SKIP_UPDATE_CHECK=1`.

## API Key Not Configured

```
No API key found. Provide via --api-key, config file, or environment variable
```

Provide an API key via any of: config file, `--api-key` flag, or environment variable.

## Invalid API Key

```
[error] API error: API error 401: ...
```

Verify your API key is correct and active.

## MiniMax / Moonshot 401

```
[error] API error: API error 401: ...
```

Usually a region-locked key. MiniMax and Moonshot each run two platforms with
separate key namespaces (`api.minimax.io` ↔ `api.minimaxi.com`, `api.moonshot.ai`
↔ `api.moonshot.cn`). The engine auto-retries the same key against the alternate
host and pins the winner, so a 401-then-success is normal. A **persistent** 401
means the key is invalid on both regions — issue a key on the other region's
console.

## Perplexity 401 referencing platform.openai.com

```
[error] API error: API error 401: ... platform.openai.com ...
```

The session was started as `--provider openai`, so requests went to
`api.openai.com` instead of `api.perplexity.ai`. Use `--provider perplexity`
(env `PERPLEXITY_API_KEY`).

## Grok signed in but 401, or "grok-4.3 does not support parameter stop"

```
[error] API error: API error 401: ...
[error] API error: API error 400: Model grok-4.3 does not support parameter stop
```

Grok must run as `--provider xai`. Spawned as `--provider openai` it ignores the
stored OAuth login (`~/.grok/auth.json`, and the engine's own token in the
credential ladder) and sends an unsupported `stop` parameter. Under
`--provider xai` the stop suppression is automatic.

## "signed in ... but its OAuth token could not be read out of the secure credential store"

The login is still there; the credential store is what cannot be opened. This
happens when a profile signed in with `WAYLAND_VAULT_PASSPHRASE` set (or with a
keyring running) and is then re-run without it — the migration that moved the
token into the vault deleted the cleartext copy once the write verified, so
there is nothing left to fall back to. The engine refuses rather than telling
you that you are signed out.

The error names the three ways forward, and the third one is exact:

- set `WAYLAND_VAULT_PASSPHRASE` (or `WAYLAND_VAULT_PASSPHRASE_FD`) to the
  passphrase this profile's vault was created under, then re-run; or
- start the OS keyring / Secret Service this profile signed in under; or
- if you removed the credential deliberately, delete the login record the error
  names (`~/.wayland/oauth/{provider}.stored`, or its `$WAYLAND_HOME`
  equivalent) and sign in again. `wayland-core auth logout chatgpt` does the
  same and works even when the store cannot be opened.

## OpenRouter model "vanishes" after one turn

This is an **app-side** issue (the desktop app's model curator), not a core
engine fault — there is no core fix. The engine keeps the selected model bound.

## Profile Not Found

```
Profile 'xxx' not found in config
```

Check that the profile is defined in your config file.

## Model Not Available

```
[error] API error: API error 404: ...
```

Check that `--model` is spelled correctly and your API key has access to that model.

## Request Too Large

```
[error] API error: API error 413: ...
```

Conversation history is too long. Restart the agent or reduce `--max-turns`.

## Rate Limited

```
[error] Provider error: Rate limited, retry after 5000ms
```

API call frequency is too high. The agent will auto-retry after the indicated delay.

## Command Timeout

```
Command timed out after 120000ms
```

A Bash tool command exceeded the timeout. Increase the timeout via the tool's `timeout` parameter.

## ripgrep Not Installed

The Grep tool automatically falls back to system `grep`. For better search performance:

```bash
brew install ripgrep  # macOS
sudo apt install ripgrep  # Debian/Ubuntu
```

## Chromium Live Browser Tests (`browser-live-tests`)

The `wcore-browser` crate ships an opt-in live-browser test suite that spawns a
real Chromium via chromiumoxide and exercises the CDP fallback backend
end-to-end. It's gated behind the `browser-live-tests` Cargo feature so a
default `cargo nextest run` on a dev box does NOT try to launch Chromium.

**Run locally** (requires a Chromium installation):

```bash
# macOS — Google Chrome works as a Chromium substitute.
export WCORE_CHROMIUM_PATH="/Applications/Google Chrome.app/Contents/MacOS/Google Chrome"

# Debian/Ubuntu — install chromium-browser via apt (matches CI).
sudo apt-get install -y chromium-browser
export WCORE_CHROMIUM_PATH=/usr/bin/chromium-browser

# Then run only the live test file:
vx cargo nextest run -p wcore-browser \
  --features browser-live-tests \
  --test chromium_live_test
```

If `WCORE_CHROMIUM_PATH` is unset, the test probes a list of common Chromium
binary paths (`/usr/bin/chromium-browser`, `/usr/bin/google-chrome`,
`/Applications/Google Chrome.app/Contents/MacOS/Google Chrome`, etc.) before
falling back to chromiumoxide's PATH auto-detection.

**CI**: a dedicated `browser-live` job in `.github/workflows/ci.yml`
installs `chromium-browser` on `ubuntu-latest` and runs the suite. The job
is marked `continue-on-error: true` — failures there do **not** block the
main CI lane while we stabilize live-browser runs in CI. See debt-register
A.1 for context.
