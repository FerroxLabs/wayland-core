# Live four-quadrant proof — Slack and Discord auth-rejection producers

Host `hetzner-dsm`. Every arm on its own `WAYLAND_HOME`, gateway liveness
asserted, owners counted before/during/after via `/proc/<pid>/environ`, torn
down by PID and verified dead. Raw per-arm logs in this directory.

Binaries (debug):

| build | sha256 | commit |
|---|---|---|
| FIXED | `bc82f7f89ba7831d399daff4c415235eb004a3152435187a292c14d80e407ac1` | `41c53619` |
| BASE  | `fd7c2d5bc9f5c5ce63698049628b0f1232a2a9a0ad5c39c1066b5c3822363c4f` | `e7bc6d88` |

## Credential premise, verified BEFORE any arm was built on it

A lane once built quadrant 1 on a token it assumed was revoked and which
worked. Both directions measured in one session:

| probe | result |
|---|---|
| slack `auth.test`, no token | `{"ok":false,"error":"not_authed"}` — auth IS enforced |
| slack `auth.test`, BOGUS token | `{"ok":false,"error":"invalid_auth"}` — **genuine platform rejection** |
| slack `auth.test`, REAL token | `{"ok":true,...,"user":"wayland_core_test"}` — the live credential works |
| discord `/users/@me`, no token | HTTP 401 |
| discord `/users/@me`, BOGUS token | HTTP 401 `{"message": "401: Unauthorized"}` |
| discord `/users/@me`, REAL token | HTTP 200, bot identity present |

The bogus credentials are well-formed constants containing no byte of any real
token. **The refusal is issued by slack.com and by Discord's own gateway** — we
are measuring what the platform does, not what we send. No mock is involved in
any arm.

## Slack

| arm | binary | credential | platform evidence | health | sticky 20s | `--require-healthy` |
|---|---|---|---|---|---|---|
| 1 rejected | FIXED | bogus → real `invalid_auth` | `invalid_auth`=1, mgr terminal=1 | **`unauthenticated`** | yes | **rc=1** |
| 2 absent | FIXED | handle not in store (`handle_in_store=0`) | none | **`disconnected`**, names `slack.authprod.ABSENT_KEY` | yes | rc=1 |
| 3 all fine | FIXED | **real live token** | none | **`healthy`** | yes | **rc=0** |
| 4 ORIGINAL | **BASE** | bogus → real `invalid_auth` | none logged | **`healthy`** ← the bug | yes | **rc=0** |

Arm 1 reason: `adapter reported auth expired: slack auth.test rejected the bot token: invalid_auth`

Arms 1 vs 4 differ **only in the binary**. Arms 1 vs 3 differ **only in the
credential**. Both controls hold.

## Discord

| arm | binary | credential | platform evidence | health | sticky 20s | `--require-healthy` |
|---|---|---|---|---|---|---|
| 1 rejected | FIXED | bogus → real gateway **4004** | `4004`=1, gateway stop=1, mgr terminal=1 | **`unauthenticated`** | yes | **rc=1** |
| 2 absent | FIXED | handle not in store (`handle_in_store=0`) | none | **`disconnected`**, names `discord.authprod.ABSENT_KEY` | yes | rc=1 |
| 3 all fine | FIXED | **real live token** | none | **`healthy`** | yes | **rc=0** |
| 4 ORIGINAL | **BASE** | bogus → real gateway 4004 | **`4004` never seen — base discards the close frame** | **`degraded`** | yes | rc=1 |

Arm 1 reason: `adapter reported auth expired: discord gateway closed with 4004 (authentication failed): the bot token was rejected at IDENTIFY`

## The brief's Discord premise is FALSE as written — corrected by measurement

> Brief: "Slack and Discord still report `Healthy` while holding a rejected credential."

**True for Slack. For Discord the dominant base state is `degraded`, not
`healthy`.** A single sample could not distinguish "always degraded" from
"flaps through healthy", so both binaries were sampled 45 times over ~90s
against a rejected token:

| | BASE `fd7c2d5b` | FIXED `bc82f7f8` |
|---|---|---|
| `healthy` | **2 / 45** | 0 / 45 |
| `degraded` | **43 / 45** | 0 / 45 |
| `unauthenticated` | **0 / 45** | **45 / 45** |
| rejected sessions in 90s (`close frame`) | **8, and climbing** | **0** (stopped after the first) |
| `4004` appears in the log | **0 — the code is discarded** | 1 |

So `Healthy` IS reachable on base Discord — 2 of 45 samples — because
`gateway.rs` pushes `Connected` immediately after sending IDENTIFY, *before*
Discord accepts it. But the honest headline is that base Discord is
**predominantly `degraded` and never `unauthenticated`**, and it re-IDENTIFYs
with the rejected token roughly every 11 seconds forever. Both states are
wrong — `degraded` tells an operator to wait, and the fix is to rotate — but
"reports Healthy" overstates it, and I would rather correct the brief than
inherit its number.

## Credential handling

Real credentials reached hetzner on **ssh stdin only**, never argv, never a
file this lane wrote. They reach disk only inside each arm's private
`WAYLAND_HOME` through the product's own `channel credential set` (stdin-only
by design), which is the mechanism under test.

Sweep, with the sweeper proved alive on a known-positive in the same run:

| target | result |
|---|---|
| committed evidence (this directory) | **0 hits** for all three secrets; known-positive `unauthenticated` = 4 hits |
| hetzner gateway logs + harness scripts | **0 hits**; known-positive `gateway` = 23 files |
| per-arm `WAYLAND_HOME` credential stores | non-zero **as expected** — the product's own store, proving the arms were real. Homes removed at lane cleanup. |

## Harness instrument defect found and REPAIRED mid-lane

The first `handle_in_store` precondition was
`channel credential list | grep -c "$HANDLE"`. That command lists every handle a
**config references**, annotated `stored` or `MISSING` — so it returned **1 for
a deliberately absent credential**. The arm-2 precondition could not fail.

Repaired to key on the status column, with a three-assertion self-test
(`matcher-selftest.sh`), the third assertion being the one that proves the
repair acts:

```
1. known-positive  new_matcher(slack.authprod.signing_secret) = 1   PASS
2. known-negative  new_matcher(slack.authprod.ABSENT_KEY)     = 0   PASS
3. OLD matcher on the known-negative                          = 1   PASS
   (non-zero => the old matcher would have missed it)
SELFTEST=PASS (all three)
```

**All eight arms above were then re-run with the repaired harness.** The arms
first run under the broken precondition are not reported.
