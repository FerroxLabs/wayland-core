# THE PLAN — wayland-core 0.13.12

> **GENERATED FILE. Do not edit.** Regenerate with `just plan`. Every fact here is
> joined from `.planning/ledger/` (state), `plan-verification.json` (independent
> verification) and `PLAN-ROUTING.json` (assignment). If this disagrees with anyone's
> recollection, this is right and the recollection is wrong — that is the entire point.

Rendered 2026-08-29 14:14 UTC

## VERDICT: BLOCKED

**43 criteria block the 0.13.12 release.** Full list in §3.

| state | count | means |
|---|---:|---|
| DONE | 32 | met, evidence resolves, independently verified |
| CLAIMED | 195 | met but NOT yet independently verified — never report as done |
| OPEN | 46 | outstanding work |
| HANDOFF | 0 | another team's half, with a filed ticket carrying it |

## §3 BLOCKING — the definition of done for 0.13.12

### `atref-residuals` — @-ref secret guard: the residuals #339 shipped past

Runs on: **hetzner**  ·  1-build

| criterion | issue | what must become true |
|---|---|---|
| `c4` | core#322 | The TUI @-ref directory walk gives the same treatment to a store reached under another name |

### `browser-revive` — Browser tool non-functional by default

Runs on: **hetzner**  ·  3-unrouted-pickup

| criterion | issue | what must become true |
|---|---|---|
| `c5` | core#113 | The deny-by-default browsing posture is recorded as a decision on the issue and the issue is dispositioned |

### `channel-caps` — Message caps: matrix/msteams probe shape, Telegram UTF-16, WhatsApp

Runs on: **hetzner**  ·  1-build

| criterion | issue | what must become true |
|---|---|---|
| `c8` | wl#934 | Telegram's unit question is settled: the cap is characters or UTF-16 code units, measured rather than assumed |

### `container-latch` — Container backend latches on a leftover name, and attests a run that never happened

Runs on: **hetzner**  ·  1-build

| criterion | issue | what must become true |
|---|---|---|
| `c1` | core#365 | A container left in Created under the name a new task would take does not fail that task -- either the name carries the nonce, or the submit path clea |
| `c2` | core#365 | A daemon-level refusal is not reported as a task exit: exit-125 from docker run produces a distinct outcome, never a receipt asserting the task ran an |
| `c3` | core#365 | The daemon's stderr reaches the operator on a daemon-level failure rather than being captured into a receipt nobody reads |
| `c4` | core#365 | A red arm is quoted verbatim: the new guard reverted, the test failing, restored and green, with the mutation shown to have landed on code |
| `c5` | core#365 | conformance_matrix passes on a host that has run it before with a leftover container present -- the regression test creates the wedged container itsel |
| `c6` | core#365 | The orphan-scan path is checked for the same latch: state whether docker ps -a --filter label=wayland.task.nonce= would have found these two, and if n |

### `decompose` — File each cross-team remainder as its OWN ticket with a contract

Runs on: **gh**  ·  2-decompose

| criterion | issue | what must become true |
|---|---|---|
| `c5` | core#314 | A grant refusal is machine-readable rather than untyped English prose in an Info frame |
| `c2` | wl#1088 | The user-visible half — the chat interface no longer reports Read/Glob/Write/Edit as restricted |
| `c3` | wl#1151 | The transcript stops assembling out of order |
| `c4` | wl#388 | The remaining four bullets of this ticket's own Expected Behavior list are met |
| `c3` | wl#434 | The alias-resolves-server-side path is closed end to end |
| `c5` | wl#998 | Desktop sends the per-tool field on the ACP path |

### `desktop-run` — Live Desktop session measurement

Runs on: **Desktop app**  ·  2-platform

| criterion | issue | what must become true |
|---|---|---|
| `c4` | wl#559 | This ticket's own close condition: ONE real 26-turn Desktop team run showing non-zero cache_read |

### `flake-584` — Shared-process lib suite: the #584 fixture misses its truncation boundary under load

Runs on: **hetzner**  ·  1-build

| criterion | issue | what must become true |
|---|---|---|
| `c1` | core#361 | The mechanism is named: what makes output lack truncated under load is identified in code, not inferred |
| `c2` | core#361 | The failure is reproduced deliberately at least once, with the command and environment recorded, before any fix is written |
| `c3` | core#361 | The fixture reaches the truncation boundary deterministically, independent of scheduling |
| `c4` | core#361 | Both assertions survive: the anti-vacuity control at mod.rs:5744 and the fragment assertion at :5749 |
| `c5` | core#361 | A red arm is quoted verbatim: the fixture failing before the change, from a real run |
| `c6` | core#361 | After the fix, cargo test --workspace --lib --no-fail-fast passes N>=10 consecutive times on the build host, and the run count is recorded |

### `flux-contract` — Anvil/Elevation loop-ownership contract

Runs on: **hetzner**  ·  3-unrouted-pickup

| criterion | issue | what must become true |
|---|---|---|
| `c3` | wl#863 | F1 confirmed for the current deployment: Elevation is unreachable by default from flux-fast, flux-standard, flux-reasoning and flux-auto |
| `c4` | wl#863 | F3 server half: requests carrying loop_owner or a client nonce bypass or vary the Flux semantic cache |
| `c5` | wl#863 | F4: the bandit routes loop_owner requests to a tool-calling-capable arm, or a flux-agentic alias with that guarantee exists |

### `instrument-integrity` — Prove the instruments can fail: mutation + measurement arms

Runs on: **hetzner**  ·  1-build

| criterion | issue | what must become true |
|---|---|---|
| `c5` | core#352 | A red arm is quoted verbatim for each platform arm |

### `macos-ci` — macOS arms via the lane/** CI wildcard

Runs on: **macOS CI**  ·  2-platform

| criterion | issue | what must become true |
|---|---|---|
| `c4` | core#352 | macOS: the pgrep arm is EXECUTED in CI at least once with the run cited, or deleted as unreachable |

### `maintainer` — Sean-only: credentials and platform accounts

Runs on: **Sean**  ·  2-maintainer

| criterion | issue | what must become true |
|---|---|---|
| `c5` | wl#1186 | whatsapp (Meta Cloud API): a credential exists and a boundary probe sends at cap and at +1 |
| `c5` | wl#934 | Every adapter's declared cap is verified against the real platform limit |

### `mcp-gate-mode` — MCP malware gate: explicit permissive/strict operator choice

Runs on: **hetzner**  ·  1-build

| criterion | issue | what must become true |
|---|---|---|
| `c7` | core#354 | The already-shipping non-session MCP launch path reads the operator's chosen mode, not the uninstalled permissive default |

### `telegram-topic` — Telegram forum-topic target sent as reply_to_message_id, never message_thread_id

Runs on: **hetzner**  ·  1-build

| criterion | issue | what must become true |
|---|---|---|
| `c6` | core#363 | Discord does not regress: a thread channel id never reaches message_reference |

### `win-owned-tree` — OwnedTree kills the process tree on Windows, not the leaf

Runs on: **hetzner**  ·  1-build

| criterion | issue | what must become true |
|---|---|---|
| `c6` | core#358 | clippy --target x86_64-pc-windows-msvc -p wcore-cli --all-targets -D warnings is clean |

### `win-runs` — Windows measurement arms - serialize, ONE box

Runs on: **SeanDesktop**  ·  2-platform

| criterion | issue | what must become true |
|---|---|---|
| `c4` | core#238 | A Windows probe records whether bare NUL is a device on the build under test and whether fs::metadata reports is_file() true for it |
| `c1` | core#324 | An instrumented run establishes whether the failure is a product race in AppContainer ACE application or a race in the test fixture |
| `c2` | core#324 | concurrent_allow_and_deny_identities_do_not_interfere passes at retries=0 over N of at least 20 on the AppContainer-capable host |
| `c3` | core#324 | Whichever arm the measurement indicts, the deny half of the test is still non-vacuous afterwards |
| `c3` | core#342 | The same guarantee holds on Windows, where the product ships |
| `c5` | core#342 | The two arms asserting a Unix-only guarantee state the Windows truth, from a measured Windows rate |
| `c5` | core#350 | The issue's own close condition is met: a green nightly-windows-soak run against this tree |
| `c2` | core#358 | A test grades the grandchild case ON WINDOWS: a direct child with a detached grandchild, guard dropped while unwinding, both gone afterwards |
| `c3` | core#358 | The red arm is quoted VERBATIM from a real Windows run, showing the grandchild surviving before the change |
| `c5` | core#358 | The CI run that executed the Windows arm is cited by URL |
| `c5` | wl#1164 | A live confirmation run on SeanDesktop is recorded before the change lands |

## §4 DECOMPOSED — another team's half, tracked

These are NOT partials. Core's half is closed; the remainder is filed against a named
owner with its own contract. A blocked criterion with no ticket does not appear here —
it appears in §3 as blocking, because that is what it is.

_None recorded yet._

## §5 CLAIMED BUT UNVERIFIED — 195

Marked `met` with resolving evidence, but no independent verifier has confirmed the lane.
Historically this is exactly where a partial hides: a criterion written thin reads `met`
while the reported bug is still live. Do not report these as done.

- **core#113** — c1, c2, c3, c4
- **core#238** — c1, c2, c3, c5, c6
- **core#244** — c1, c2
- **core#253** — c1, c2, c3, c5, c6, c7
- **core#314** — c1, c2, c3, c4
- **core#322** — c1, c2, c3
- **core#323** — c1, c2, c3, c4
- **core#325** — c1, c2, c3, c4
- **core#335** — c1, c2, c3, c4
- **core#336** — c1, c2
- **core#337** — c1, c4
- **core#338** — c1, c2, c3, c4
- **core#339** — c1, c2, c3, c4, c5, c6
- **core#340** — c1, c2, c3, c4, c5
- **core#342** — c1, c2, c4
- **core#350** — c1, c2, c4
- **core#352** — c1, c2, c3
- **core#353** — c1, c2, c3, c4
- **core#354** — c1, c2, c3, c4, c5, c6
- **core#356** — c1, c2, c3, c4
- **core#358** — c1, c4
- **core#360** — c1, c3
- **core#363** — c1, c2, c3, c4, c5
- **wl#174** — c1, c2, c3, c4, c5
- **wl#305** — c1
- **wl#388** — c1, c2, c3
- **wl#434** — c1
- **wl#559** — c1, c2, c6
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

