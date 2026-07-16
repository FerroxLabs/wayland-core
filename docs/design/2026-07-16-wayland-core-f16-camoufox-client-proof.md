# F16 Camoufox client contract proof

## Scope

This increment fixes the Wayland Core client boundary for the maintained
`@askjo/camofox-browser` sidecar. It does not claim that Wayland currently
installs or starts the Node sidecar; lifecycle remains a separate F16 gap.

The contract was checked against `jo-inc/camofox-browser` version `1.11.2` at
commit `ce3a3b085aacba73eb8de6c51733c19fb13bfae4` on 2026-07-16.

## Root cause

The prior backend called an API that the sidecar does not implement:
`POST /sessions`, followed by operations under `/sessions/<id>/...`. It also
expected `session_id`, `final_url`, and a raw JSON accessibility tree that are
not part of the current sidecar response schema. Consequently a healthy
sidecar could not satisfy even the first Wayland browser operation.

The real contract creates a tab with `POST /tabs`, retains the caller-minted
`userId` and `sessionKey`, and operates on `/tabs/<tabId>/...`. Navigation
returns the post-navigation address in `url`; snapshots are returned as
sidecar text containing `[eN]` references.

## Implemented boundary

- Create and close one real sidecar tab per Wayland browser session.
- Retain and send the owning `userId` on every tab operation.
- Map navigate, snapshot, click, fill/type, press, state, screenshot, back,
  and forward to their real routes.
- Convert sidecar `[eN]` snapshot references to Wayland-visible `[@eN]`
  references while retaining a provider-neutral node index.
- Re-check the sidecar's returned `url` under `BrowserPolicy` and fail closed
  when a policy-bearing navigation omits it.
- Return typed `Unsupported` errors for operations without a truthful upstream
  mapping instead of posting to invented endpoints and returning false success.

## Authoritative Linux evidence

All commands ran through the Hetzner `remote-cargo.sh` harness against the
staged tree in slot `wcore-f16`.

1. Targeted Camoufox regressions:
   `cargo test -p wcore-browser backends::camoufox::tests -- --nocapture`
   passed 9/9.
2. Full browser crate:
   `cargo test -p wcore-browser` passed 111 tests across unit and integration
   suites (76 library, 3 operation lock, 27 policy, 1 provider trait, and 4
   timeout/cancellation tests).
3. Strict lint:
   `cargo clippy -p wcore-browser --all-targets --all-features -- -D warnings`
   passed.
4. Local macOS preflight only:
   `cargo fmt --all` and `git diff --check` passed. No Cargo build or test ran
   on the Mac.

## Remaining boundary

This commit makes a running sidecar usable; it does not make one appear.
Wayland's current binary manager downloads a Firefox-derived Camoufox browser
binary, while its supervisor treats that binary as though it were the Node
HTTP server. The host adapter also constructs the HTTP client without invoking
the manager or supervisor. F16 remains open until sidecar discovery, pinned
installation, startup, health readiness, and teardown are implemented and
proved on packaged Windows, Linux, and macOS paths.
