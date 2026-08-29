# THE PLAN — wayland-core 0.13.12

> **GENERATED FILE. Do not edit.** Regenerate with `just plan`. Every fact here is
> joined from `.planning/ledger/` (state), `plan-verification.json` (independent
> verification) and `PLAN-ROUTING.json` (assignment). If this disagrees with anyone's
> recollection, this is right and the recollection is wrong — that is the entire point.

Rendered 2026-08-29 17:17 UTC

## VERDICT: BLOCKED

**16 criteria block the 0.13.12 release.** Full list in §3.

| state | count | means |
|---|---:|---|
| DONE | 32 | met, evidence resolves, independently verified |
| CLAIMED | 223 | met but NOT yet independently verified — never report as done |
| OPEN | 24 | outstanding work |
| HANDOFF | 10 | another team's half, with a filed ticket carrying it |

### ⚠ 14 UNROUTED — nobody is doing these

An unrouted criterion is how work goes missing. The render fails until each has a lane.

- `wl#1228 c1` — A chat id the bot may post to is RECORDED in the live-cap credentials home, not merely used once
- `wl#1228 c2` — The astral run is executed and both arms of its LIVE_CAP_UNIT block are pasted onto the issue
- `wl#1228 c3` — The verdict is APPLIED: a refusal at 4,096 astral scalars drops the declared cap to 2,048, an accept leaves 4,
- `wl#1228 c4` — docs/delivery-semantics.md records the unit, the date, and which arm settled it
- `core#366 d1` — The product can enumerate wayland-created containers WITHOUT being given a nonce -- a key-presence scan reacha
- `core#366 d2` — The nonce-scoped scan_orphans(nonce) contract is left intact for the caller that genuinely wants one run's orp
- `core#366 d3` — An operator surface reports a leftover it did not create in this process -- one whose nonce is not in the live
- `core#366 d4` — The conformance check at conformance.rs:340 is re-examined: it asserts enumerated && found.is_empty() for a no
- `core#366 d5` — A regression test plants a labelled leftover under a nonce the running process has never used, and asserts the
- `core#366 d6` — Whether reclamation is in scope is DECIDED and recorded: state whether an unscoped scan only reports, or also 
- `core#373 c1` — The mechanism is named in code: what makes the ERROR not reach the scoped subscriber. Not inferred -- two infe
- `core#373 c3` — The fix arm scores 0 failures at n>=100 on that same instrument, and the baseline is re-measured at the same n
- `core#373 c4` — Both osv_check log-visibility tests keep their exact-equality assertion on [Level(Error)]
- `core#373 c5` — cargo test --workspace --lib --no-fail-fast passes N>=10 consecutive times on hetzner-dsm, run count recorded

## §3 BLOCKING — the definition of done for 0.13.12

### `UNROUTED`

| criterion | issue | what must become true |
|---|---|---|
| `d1` | core#366 | The product can enumerate wayland-created containers WITHOUT being given a nonce -- a key-presence scan reachable from an operator-facing surface, not |
| `d2` | core#366 | The nonce-scoped scan_orphans(nonce) contract is left intact for the caller that genuinely wants one run's orphans (cancel()); the unscoped scan is an |
| `d3` | core#366 | An operator surface reports a leftover it did not create in this process -- one whose nonce is not in the live registry |
| `d4` | core#366 | The conformance check at conformance.rs:340 is re-examined: it asserts enumerated && found.is_empty() for a nonce chosen so nothing can ever be found |
| `d5` | core#366 | A regression test plants a labelled leftover under a nonce the running process has never used, and asserts the unscoped scan reports it -- creating th |
| `d6` | core#366 | Whether reclamation is in scope is DECIDED and recorded: state whether an unscoped scan only reports, or also reclaims, and justify it against #365's  |
| `c1` | core#373 | The mechanism is named in code: what makes the ERROR not reach the scoped subscriber. Not inferred -- two inferences have already failed |
| `c3` | core#373 | The fix arm scores 0 failures at n>=100 on that same instrument, and the baseline is re-measured at the same n in the same session |
| `c4` | core#373 | Both osv_check log-visibility tests keep their exact-equality assertion on [Level(Error)] |
| `c5` | core#373 | cargo test --workspace --lib --no-fail-fast passes N>=10 consecutive times on hetzner-dsm, run count recorded |

### `decompose` — File each cross-team remainder as its OWN ticket with a contract

Runs on: **gh**  ·  2-decompose

| criterion | issue | what must become true |
|---|---|---|
| `c5` | core#314 | A grant refusal is machine-readable rather than untyped English prose in an Info frame |
| `c4` | wl#388 | The remaining four bullets of this ticket's own Expected Behavior list are met |

### `desktop-run` — Live Desktop session measurement

Runs on: **Desktop app**  ·  2-platform

| criterion | issue | what must become true |
|---|---|---|
| `c4` | wl#559 | This ticket's own close condition: ONE real 26-turn Desktop team run showing non-zero cache_read |

### `macos-ci` — macOS arms via the lane/** CI wildcard

Runs on: **macOS CI**  ·  2-platform

| criterion | issue | what must become true |
|---|---|---|
| `c4` | core#352 | macOS: the pgrep arm is EXECUTED in CI at least once with the run cited, or deleted as unreachable |

### `prompt-cache` — Prompt-cache collapse and re-billed context

Runs on: **hetzner**  ·  1-build

| criterion | issue | what must become true |
|---|---|---|
| `c6` | wl#559 | The skill-router hint and PrePrompt hook contributions no longer land at messages[1] on turn 1 |

### `win-owned-tree` — OwnedTree kills the process tree on Windows, not the leaf

Runs on: **hetzner**  ·  1-build

| criterion | issue | what must become true |
|---|---|---|
| `c4` | core#358 | A negative control passes in both arms, so a change that kills too much fails here |

## §4 DECOMPOSED — another team's half, tracked

These are NOT partials. Core's half is closed; the remainder is filed against a named
owner with its own contract. A blocked criterion with no ticket does not appear here —
it appears in §3 as blocking, because that is what it is.

| criterion | issue | owner | carried by |
|---|---|---|---|
| `c5` | core#113 | maintainer | FerroxLabs/wayland#1229 |
| `c2` | wl#1088 | desktop | FerroxLabs/wayland#1223 |
| `c3` | wl#1151 | desktop | FerroxLabs/wayland#1224 |
| `c3` | wl#434 | flux | FerroxLabs/wayland#1226 |
| `c3` | wl#863 | flux | FerroxLabs/wayland#1227 |
| `c4` | wl#863 | flux | FerroxLabs/wayland#1227 |
| `c5` | wl#863 | flux | FerroxLabs/wayland#1227 |
| `c5` | wl#934 | maintainer | FerroxLabs/wayland#1186 |
| `c8` | wl#934 | maintainer | FerroxLabs/wayland#1228 |
| `c5` | wl#998 | desktop | FerroxLabs/wayland#1225 |

## §5 CLAIMED BUT UNVERIFIED — 223

Marked `met` with resolving evidence, but no independent verifier has confirmed the lane.
Historically this is exactly where a partial hides: a criterion written thin reads `met`
while the reported bug is still live. Do not report these as done.

- **core#113** — c1, c2, c3, c4
- **core#238** — c1, c2, c3, c4, c5, c6
- **core#244** — c1, c2
- **core#253** — c1, c2, c3, c5, c6, c7
- **core#314** — c1, c2, c3, c4
- **core#322** — c1, c2, c3, c4
- **core#323** — c1, c2, c3, c4
- **core#324** — c1, c2, c3
- **core#325** — c1, c2, c3, c4
- **core#335** — c1, c2, c3, c4
- **core#336** — c1, c2
- **core#337** — c1, c4
- **core#338** — c1, c2, c3, c4
- **core#339** — c1, c2, c3, c4, c5, c6
- **core#340** — c1, c2, c3, c4, c5
- **core#342** — c1, c2, c3, c4, c5
- **core#350** — c1, c2, c4, c5
- **core#352** — c1, c2, c3, c5
- **core#353** — c1, c2, c3, c4
- **core#354** — c1, c2, c3, c4, c5, c6, c7
- **core#356** — c1, c2, c3, c4
- **core#358** — c1, c2, c3, c5, c6
- **core#360** — c1, c3, c6
- **core#361** — c1, c2, c3, c4, c5, c6
- **core#363** — c1, c2, c3, c4, c5, c6
- **core#365** — c1, c2, c3, c4, c5, c6
- **core#373** — c2
- **wl#174** — c1, c2, c3, c4, c5
- **wl#305** — c1
- **wl#388** — c1, c2, c3
- **wl#434** — c1
- **wl#559** — c1, c2
- **wl#863** — c1, c2
- **wl#908** — c1, c2, c3
- **wl#934** — c1, c2, c3, c4, c6
- **wl#998** — c1, c2, c3, c4
- **wl#1088** — c1
- **wl#1134** — c1, c2, c3, c4, c5
- **wl#1150** — c1, c2, c3
- **wl#1151** — c1
- **wl#1155** — c1, c2, c3
- **wl#1156** — c1, c2
- **wl#1161** — c1, c2
- **wl#1162** — c1, c2
- **wl#1163** — c1, c2, c3
- **wl#1164** — c5
- **wl#1165** — c2
- **wl#1166** — c1, c2, c3, c4, c5
- **wl#1168** — c1, c2, c3
- **wl#1170** — c1, c2, c3, c4
- **wl#1171** — c1, c2, c3
- **wl#1172** — c1, c2, c3
- **wl#1173** — c1, c2, c3
- **wl#1174** — c1, c2, c3
- **wl#1175** — c1, c2, c3
- **wl#1176** — c1, c2, c3, c4, c5
- **wl#1177** — c1, c2, c3
- **wl#1178** — c1, c2, c3, c4, c5
- **wl#1179** — c1, c2, c3, c4, c5
- **wl#1180** — c1, c2, c3, c4
- **wl#1181** — c5
- **wl#1182** — c1, c2, c3
- **wl#1186** — c1, c2, c3, c4, c6

## §6 OUT OF SCOPE for 0.13.12 — feature work

Excluded by explicit instruction: defects ship, feature requests wait. The work still
gets built and its branch pushed; it just does not gate this release.

- **wl#305** — [Feature]: improve Win/WSL interop

## §7 DONE — verified

Every criterion met, evidence resolves in the tree, and an independent adversarial
verifier re-ran the gate and confirmed it.

- **core#244** — c3
- **core#253** — c4
- **core#336** — c3, c4
- **core#337** — c2, c3
- **core#350** — c3
- **core#353** — c5
- **core#355** — c1, c2, c3, c4
- **core#360** — c2, c4, c5
- **wl#434** — c2
- **wl#559** — c3, c5
- **wl#934** — c7
- **wl#998** — c6
- **wl#1150** — c4
- **wl#1151** — c2
- **wl#1155** — c4
- **wl#1164** — c1, c2, c3, c4
- **wl#1165** — c1
- **wl#1181** — c1, c2, c3, c4

