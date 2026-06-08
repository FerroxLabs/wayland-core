# Troubleshooting

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
