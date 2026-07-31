# NOTES — lane/connected-before-handshake

Base: `c9ab048b952c5bc74c75ea8f76df06788408de59` (asserted with `/usr/bin/git rev-parse HEAD`
in `/Users/seandonahoe/dev/waylandcore-frontier-worktrees/lane-connected-before-handshake`).

## Premise verification at base (all reads via the Read tool, unproxied grep)

| Premise (from the scoping brief) | Verdict at c9ab048b | Evidence |
|---|---|---|
| `gateway.rs` pushes `Connected` before the handshake is accepted | **TRUE** | `gateway.rs:988-993` pushes `ConnectionStateChanged{Connected}` immediately after the IDENTIFY/RESUME send at `:955-983`, before any frame is read back. Comment at `:985-987` admits it. |
| `Connected` maps to `Healthy` | **TRUE** | `wcore-channels/src/health.rs:54` — `ConnectionState::Connected => HealthState::Healthy`; consumed at `manager.rs:402-414`. |
| 4013/4014 are unclassified | **TRUE** | `gateway.rs:577-586` matches only `Some(4004)`; everything else `_ => None` → resumable reconnect path. |
| Line number moved 926 → 988 | **TRUE** | brief said 926; actual 988. |

## Defect shape

On a rejected token the observable sequence is:

1. connect, HELLO
2. send IDENTIFY
3. **push `Connected` → `Healthy`** (`:988`)
4. peer closes 4004
5. `SessionExit::AuthRejected` (`:1044-1046`)
6. `AuthExpired` (`:760-763`) → `Unauthenticated`

Step 3 is a pure false-positive window. Same window exists for EVERY failed handshake:
4013 (invalid intents), 4014 (disallowed intents), op 9 Invalid Session after IDENTIFY,
and a socket that drops between IDENTIFY and READY.

## Plan

1. Move the `Connected` push from post-send to the point READY (fresh IDENTIFY) or
   RESUMED (resume) is actually observed. `parse_ready` (`:282`) and `is_resumed` (`:292`)
   both already exist and are already called in the dispatch arm (`:1098`, `:1108`).
2. Classify 4013/4014 as terminal credential/config rejections with their own reason text.
3. Prove BOTH the fresh-IDENTIFY and the RESUME arm with the existing fake-gateway harness
   in `crates/wcore-channel-discord/tests/gateway_auth_close.rs` (which already spins a real
   local WebSocket server, HELLOs, and reads IDENTIFY).

## Risk the previous lane named, and how it is addressed

Previous lane declined to move the push because RESUME was untestable. It is testable: the
fake gateway can be extended to accept the RESUME frame and reply `{"op":0,"t":"RESUMED"}`.
The RESUME arm must be exercised or this trades a known defect for an unknown one.

## Status log

- [t0] worktree created, premises verified, notes committed.
