# HANDOFF — Wayland Core, post-UAT, 2026-07-30

Integration `plan/f20-unified-audit-repair` @ **`bc90ee1c`**.
Supersedes `HANDOFF-2026-07-30-SHIP.md`. **`.planning/LANE-BRIEF.md` outranks any orchestrator
instruction, including this file.**

---

## 0. Do this first

1. **Four fix lanes are in flight** — §2. Measure their branches before assuming anything; a death
   notice is an absence claim, and mtimes lie (two lanes were declared dead this session and were
   both alive).
2. **The release is blocked on one thing** — §1. Everything else is queued behind it.
3. **Sean is unblocked on credentials** except the Twilio auth token — §4.

**Merge cadence, unchanged and load-bearing:** one lane at a time → merge locally → push to a
disposable `orch-verify-*` ref → on hetzner `git checkout -- .` then checkout that ref and
**assert the SHA** → `cargo check --workspace --all-targets` + `cargo fmt --check` +
**`cargo metadata --locked`** → only then push to integration → delete the scratch ref.

**`cargo metadata --locked` is NEW in that list and it is there because lockfile drift broke
`--locked` builds twice this week**, including `release.yml`, and both times it was found by
accident rather than by a gate. It costs seconds.

---

## 1. THE RELEASE BLOCKER

**No OS keyring → the product is unusable, and every diagnostic says it is fine.**

Found independently by two UAT lanes from opposite directions:

- `uat-channels-live`: every inbound channel turn dies with `Session persistence authority
  unavailable`, while `gateway run` starts, `channel probe` says `Ok`, `channel health` says
  `Healthy`.
- `uat-tui-unix`: the headless CLI cannot complete a single turn. `rc=1`, `no OS keyring was
  usable`.

This is **every Linux server**. Windows is unaffected (it has a credential store); the macOS half
is bounded because a Terminal.app user may have a keychain.

Two *different* workarounds were found — `[session] enabled = false` and
`WAYLAND_VAULT_PASSPHRASE` — which is the clue that the fix belongs upstream of both.
`lane/fix-headless-keyring` is on it and is told: reproduce the bug before claiming a fix, prove
all four quadrants, and **the product must never again report `Healthy` while unable to answer a
message.**

---

## 2. In flight when this was written

| lane | doing |
|---|---|
| `fix-headless-keyring` | §1. The blocker. |
| `fix-tui-first-message` | TUI eats the first 4–20 chars you type. Two defects: modal shows when configured, AND keystrokes are dropped on transition. **Fixing only the first leaves the race for unconfigured users.** |
| `fix-channel-health-truth` | `channel health` says `Healthy` during a live 401. `HealthState::Unauthenticated` is unreachable — no adapter emits `AuthExpired` (0 of 10). |
| `fix-channel-onboarding` | `docs/channels.md`'s own config does not load (4 required fields undocumented); no CLI verb writes a channel credential; the `keychain:` syntax in `config.rs` is inert. |

A **live WhatsApp bridge process** is running on the Mac emitting refreshing pairing QRs to
`scratchpad/wa/live-err.txt` (session material under `scratchpad/wa/home`, NOT in the Desktop
repo — verified). Kill it when pairing is done or abandoned.

**WhatsApp pairing FAILED and it is a product gap, not just bad luck.** Sean scanned and got
*"Couldn't link device. Try again later."* The bridge log shows **12 QRs emitted, zero errors, no
connection-status change** — it never saw a pairing attempt, so the failure was entirely
WhatsApp-side. Cause: **each QR lives ~20 seconds** and cannot survive a chat round-trip.
`requestPairingCode` (an 8-char code typed on the phone, much longer TTL) **exists in the Baileys
library but Desktop's bridge does not expose it** — QR is the only path. So a **remote or headless
operator cannot pair at all**, which is precisely Core's target deployment. Route this to
`whatsapp-bridge` follow-up work: expose pairing-code as the primary mechanism, QR as the
convenience path.

---

## 3. UAT verdict, measured not asserted

| surface | verdict |
|---|---|
| Windows | **works** — cleanest of the four, 5 findings, none blocking |
| macOS / Linux TUI | work, with the first-message input loss |
| Linux headless | **unusable** (§1) |
| Slack / Discord / Matrix | a real agent reply reached real Slack end to end; **inbound half UNRUN** |
| Desktop ↔ Core | **do not agree** — 3 HIGH, all Desktop-side |

**Windows is a genuine reversal** — historically the worst platform. Real ConPTY, zero CRLF
corruption, real FluxRouter turn in 5.06s with incremental streaming, approval proven both
directions with the filesystem as arbiter, zero orphans, and `backup` passes at **305-char paths**
— the `os error 3` past `MAX_PATH` **does not reproduce**.

**Desktop drops 27 of 38 live frames (71%)**, including `execution_policy` and `workspace_policy`,
the two carrying the session's security posture. Worst of the three: **no contract negotiation
exists at all** — a `major:2` `ready` with *forged* digests is byte-indistinguishable from golden
to Desktop's decoder. **Contract regeneration #4 is read by nobody.** All three are Desktop-side;
Core is correct. Routing is Sean's call (§4).

**Ours from that lane:** the corpus omits 7 Core events and 1 command, two of which appear in
*every* session; and `docs/json-stream-protocol.md` §4.1 documents a `protocol_error` Core never
emits.

---

## 4. Sean's queue

| item | state |
|---|---|
| **Twilio auth token** | ONLY missing piece. `~/.wayland-secrets/twilio.env` is pre-filled with SID / from / to and an empty `TWILIO_AUTH_TOKEN=`. Console home → Account Info → Show. **Use the primary Live token, not a Test Credential** — test creds accept sends and never deliver, which would read as a pass. |
| **WhatsApp QR** | bridge is live; ask for a fresh QR. Links his PERSONAL WhatsApp; Meta bans numbers for automated use. |
| **Matrix** | RE-MINTED and verified this session. Old token was revoked mid-day. |
| `anthropic.rs:307` | the one provenance site the comparison could not call. Exact identifier match `ANTHROPIC_CACHE_CONTROL_LIMIT = 4`, but both halves externally dictated, and the "moving-breakpoint layout" it credits has zero counterpart in either peer. Strip / notices-file / leave — all defensible. |
| **Desktop's 3 HIGH** | needs routing: issue to the desktop lane, or direct work in `/Users/seandonahoe/dev/wayland`. |
| **Rotate** Slack + Discord + Matrix tokens after UAT | all were pasted in chat. |

---

## 5. What landed this session

Sixteen lanes merged, each workspace-verified on hetzner at a programmatically asserted SHA.

- **Three channels driven live** (Slack, Discord, Matrix) — five message actions each.
- **Exactly-once went 3 of 10 → 1 of 10.** Slack and Discord both declared it on *mockito*
  evidence; both produced **two** messages when finally driven at the real API. Matrix is the only
  survivor and the only one driven live before it was believed. The gateway reads that bit to
  decide whether to re-send, so both were silently duplicating on restart.
- **glibc floor 2.39 → 2.34** — restores Ubuntu 22.04, Debian 12, Rocky 9, AlmaLinux 9, AL2023.
- **Provenance resolved**: 5 attribution headers stripped as false, 3 consolidated into
  `THIRD-PARTY-NOTICES.md`, 8 comments corrected that misdescribed OpenClaw's behaviour.
- **`27-C2` was never a real blocker** — its "blocked on a display-capable host" claim was false in
  both halves; the lane ran it under `xvfb-run`, 2 passed.
- Contract regeneration #4; `Cargo.lock` repaired; README now distinguishes driven-live from
  code-complete, build-enforced.

---

## 6. Corrections that must not be re-litigated

- **Darwin CI needed no work.** A job was already on `sean-mac-arm64`; the runner is at **84.8%
  duty cycle** (~15% headroom) so moving more would queue CI behind Sean's own machine; and
  self-hosted Windows waits a **median 71.8 min** — self-hosted is the *worse* queue here.
- **`Build (x86_64-apple-darwin)` has zero unique detection value** — 0 `cfg(target_arch)`, 0 x86
  intrinsics repo-wide, against a live 64-file `cfg(target_os)` control.
- **`ferrox-win-msvc` served zero jobs across 40 runs while reporting `busy=true`.**
- **Release trust root is DONE** — signing wired and `WAYLAND_RELEASE_ACCEPTANCE_SEED` exists.
- **Desktop does NOT solve delivery identity.** Its Twilio `sendWithRetry` retries 429/5xx with no
  token, so a 5xx-after-accept duplicates — strictly *weaker* than Core's abandon.

---

## 7. Standing lessons — every one earned by a false green

- **Grade off code and executed tests, never a `SUMMARY.md`.** Two criterion rows were stale
  precisely because someone graded off a finding lane's summary while the repair lane merged after.
- **Run every control in both directions.** A permanently-red gate proves as little as a
  permanently-green one.
- **A skip is not a pass.** Count and report unrun cells.
- **A mock proves what we send and nothing about what the destination does.** Two false
  exactly-once claims came from exactly this.
- **`rtk` fabricates machine-readable counts and the absolute path does NOT save you.** Redirect to
  a file, read with the Read tool.
- **`${PIPESTATUS[0]}` fails in `sh`/dash**; `grep -c X || echo 0` yields `"0\n0"`; zsh eats an
  unquoted glob; **a pipe steals exit status** (produced a false HIGH this session).
- **Never use backticks in a shell-quoted commit message** — it ate words from three commits today.
  Write the message to a file and `git commit -F`.
- **Assert the SHA after every checkout.** A checkout that aborts on a dirty file reports the OLD
  SHA and looks successful.
- **A participant that never started reports a clean run.**
- **Tracking documents are systematically stale in the product's favour** — but it cuts both ways.
  `23A-C1`, `24-C5` and `27-C2` all moved UP when re-measured.
- **Every UAT lane caught instrument defects in its own harness** — nine, six, two and two. Roughly
  a dozen false findings never reached Sean. Budget for this; it is the work, not overhead.
