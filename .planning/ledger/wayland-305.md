---
issue: 305
repo: FerroxLabs/wayland
kind: feature
title: "[Feature]: improve Win/WSL interop"
status: open
last_verified_commit: 93ede3424
criteria:
  - id: c1
    text: "acp serve boots in a headless WSL with no OS secret service, persisting its server key rather than hard-exiting"
    state: met
    evidence: "test:crates/wcore-cli/src/acp.rs::headless_first_run_persists_to_the_profile_file_at_0600"
    owner: core
    note: "the key is minted once at 0600 and read back on the next boot, so a Win/WSL client does not re-pair after every Core restart"
  - id: c2
    text: "A project-scoped approval allowlist is reachable over REST so a working directory under an enabled entry auto-resolves instead of gating"
    state: not-met
    owner: core
    note: "no permissions/allowlist route exists anywhere in the tree; the issue sizes this at roughly 1600 lines and it has not been started"
  - id: c3
    text: "An approval or exec timeout surfaces an Error frame instead of hanging silently"
    state: not-met
    owner: core
    note: "this is the #287 symptom the reporter actually hit — per-command confirmations taking one to two hours for a directory listing"
  - id: c4
    text: "Desktop autodetects a local WSL Core and offers detected-versus-manual endpoint and key settings"
    state: blocked
    owner: desktop
    note: "the probe needs no Core change since /openapi.json is already unauthenticated, but the settings UI and the allowlist popup are Desktop's surface"
---

The reporter runs their web projects inside WSL and Wayland on Windows, and
reading a WSL codebase from the Windows engine hangs. The cause is not
WSL-specific: the sandbox is chosen by host OS rather than by where the files
live, so the Windows AppContainer probe re-runs on every command. Running Core
inside WSL sidesteps it entirely, because Linux uses a different sandbox.

The reporter's design was built on the actual server source, and its key finding
holds: `wayland-core acp serve` already provides REST, SSE, WebSocket, sessions,
approvals and auth, and `/openapi.json` is unauthenticated, so autodetection over
`127.0.0.1` needs no Core change at all.

One of the four criteria is met. The server key is now persisted rather than
minted per restart, which is what makes pairing survive a Core restart — an
earlier lane that minted a fresh key each start was dropped rather than merged,
recorded on the issue so nobody re-lands it. The REST allowlist half is a
feature that has not been started, and the Desktop half is not core's to build.
