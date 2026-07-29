---
lane: 24-c4-support
branch: lane/24-c4-support
base: 5140d640fb5df75f196d0875cb5bcd4a48ddf097
worked: 2026-07-29
closes: F24-C4-H1
opens: F24-C2-M1
verdict: "24-C4 half two goes NOT MET -> MET on Linux. The criterion goes PARTIAL -> MET-WITH-STATED-EXCEPTIONS. The phase's single release blocker is closed."
fence-exposure: "ZERO. git diff --stat 5140d640 HEAD -- crates/wcore-cli/src/{lib,main}.rs is empty, with a live control in the same invocation (gateway.rs 384/1, cron.rs 25/0, in_flight_bound.rs 200/0)."
instrument-defects-mine: 2
instrument-defects-repaired: 2
---

# 24-C4-SUPPORT — the support bundle gets an operator verb

Lane `24-c4-support`, based on `lane/grade-24` at `5140d640` (the verdict commit).

---

## 0. Bottom line

**`F24-C4-H1` is closed.** `wayland-core gateway support-bundle --out <DIR>` exists in the
shipped binary, drives `wcore_gateway::support_bundle::collect`, and was driven end to end
against a **running gateway** on `hetzner-dsm`. The `#[ignore]`d `live_bundle_canary` gate —
which §10(c) of the verdict suspected had never been executed — has now been executed, and was
**proved able to fail two ways** before its pass was believed.

**Criterion 4's second half moves from NOT MET to MET on Linux.** With half one already met on
HTTP/SSE, I grade **`24-C4` as MET-WITH-STATED-EXCEPTIONS** (the exceptions being the transport
envelope and the platform envelope, both unchanged and both already named as non-blocking by the
verdict).

**Per §8 of the verdict, that was the phase's only remaining release blocker.** I did not
re-derive the other four criteria, so I make no claim about the phase goal beyond this one item.

I also took one `24-C2` residual and **found the record's description of it wrong**, in a way
that matters — see §3. That produced one new MEDIUM, `F24-C2-M1`.

I did **not** touch `24-C2` webhook/poll (cut by Sean, 2026-07-29) and did **not** take any
`24-C3` residual. §5 says what I left and why.

---

## 1. What landed

| # | commit | what |
|---|---|---|
| 1 | `2e870a7a` | NOTES file, inside the first 15 minutes (LANE-BRIEF §6b-i) |
| 2 | `80d1bdf8` | **`gateway support-bundle`** — the verb, 5 new tests |
| 3 | `2bbbc8e1` | live evidence |
| 4 | `7a190f4d` | instrument repair: pin the exact `snake_case` status token |
| 5 | `55401b91`+`033261db`+`5a1418eb` | `24-C2` in-flight measurement + the `cron status` annotation |

### The verb

`GatewayCmd::SupportBundle` in `crates/wcore-cli/src/gateway.rs`. **Zero edits to either
fenced file** — `main.rs:1437` already routed `TopCmd::Gateway`, so the verb family was already
reachable and only the variant was missing.

Collects, via the existing collector and adding **no redaction rule of its own**:

- `config.toml` and `credentials.toml` **KEY NAMES ONLY** (structural elision; values never read);
- the process environment as **names only**, secret-marked names flagged;
- `gateway.log`, tail-bounded and scrubbed;
- four projections — a staged **liveness** record, `gateway-status.json`, `channel-health.json`,
  and the delivery journal `deliveries.jsonl`.

Three design points that are not incidental:

**Liveness is a projection, not an afterthought.** `gateway-status.json` is rewritten every tick
and outlives the process that wrote it, so a bundle that inherited `running` from it would tell
support a dead gateway was up. Liveness is derived from the pid record plus a liveness check and
staged as a projection so it is scrubbed, **declared in `manifest.members`**, and covered by the
canary scan. A file in the bundle the manifest does not declare is the collector's
absent-vs-never-collected ambiguity in reverse.

**A production post-condition that can refuse.** After `collect` returns, every known secret is
re-scanned across the finished bundle **over raw bytes** (`read_to_string` would silently skip a
non-UTF-8 member, and a secret that survived into one is still a leak). On a hit the verb
**refuses with a non-zero exit** and names the members. It does not delete the directory —
deleting destroys the only evidence of a redaction defect, and the operator has been told not to
send it.

**`--out` is required, with no default.** This artifact is built to cross a trust boundary. The
product choosing its location is how a bundle ends up somewhere nobody meant to send it from.

### The `24-C2` change

`crates/wcore-cli/src/cron.rs`: `cron status` now annotates a `max_in_flight > 1` line instead of
silently echoing it. The value is **not** rewritten — narrowing a persisted field on a render
path would make the verb disagree with the store. Plus
`crates/wcore-cron/tests/in_flight_bound.rs`, 5 tests.

---

## 2. Gates — every number read back, unproxied

| gate | result |
|---|---|
| `cargo test -p wcore-cli --lib gateway::` | **14 passed, 0 failed, 0 ignored, 1836 filtered out** (5 new) |
| `cargo test -p wcore-cli --lib cron::` | **6 passed, 0 failed, 0 ignored, 1844 filtered out** |
| `cargo test -p wcore-gateway --test support_bundle_redaction` | **4 passed, 0 failed, 1 ignored** |
| `cargo test -p wcore-cron` (all targets) | **73 + 11 + 3 + 5 + 8 + 13 + 0 passed, 0 failed, 0 ignored** |
| `cargo clippy -p wcore-cli -p wcore-cron --all-targets` | clean (only the pre-existing `imap-proto` future-incompat note) |
| `cargo fmt --all` | clean (run on the Mac, which is permitted) |

Every count above includes its `ignored` and `filtered out` fields, read off `/usr/bin/env cargo`
per LANE-BRIEF §3b. **No full-workspace run was taken or is cited** — the verdict's §10(a)
records why one would not be a measurement on this host, and I did not close that.

### Mutation proofs — all six, one of which failed to fire and was repaired

Every new test was given a mutation and required to redden. `M6` mutates **production code in
`wcore-gateway`, rebuilds the shipped binary, and drives the real verb.**

| # | mutation | expected | result |
|---|---|---|---|
| M1 | rename the verb to `support-bundlx` | reachability test RED | **RED** |
| M2 | verb parses, returns `Ok(())` before `collect` | calls-the-collector test RED | **RED** |
| M3 | liveness read from the stale status file | dead-gateway test RED | **survived → see below** |
| M3′ | same, with the correct on-disk token | dead-gateway test RED | **RED** |
| M4 | `--home` ignored for config sources | redirect test RED | **RED** |
| M5 | leak detector blinded (`> 999_999`) | detector test RED | **RED** |
| M6 | **break the log scrub inside `collect`, rebuild, drive the verb live** | verb REFUSES | **`M6_RC=1`**, message: `REDACTION FAILURE — do NOT send this bundle. 1 member(s) … still contain a known secret value: …/recent-log.txt` |

**M3 is the one worth reading.** It survived because *my mutation* searched for `"Running"` while
`GatewayState` is `#[serde(rename_all = "snake_case")]` and the byte sequence on disk is
`"running"`. The mutation never fired, and a mutation that never fires reports the test as strong
for free — the same defect class the brief warns about, produced by me, inside the lane whose job
was that class.

Per LANE-BRIEF §6b-ii I repaired the instrument rather than writing it up: `7a190f4d` pins the
exact token in the test's positive control **and** asserts the CamelCase form is absent, so
whoever writes the next mutation reads the correct string off the test. M3′ then reddened.

---

## 3. The `24-C2` residual — and a correction to the record

The verdict §3 records `max_in_flight` as *"stored and clamped but not enforced at dispatch"*.
I set out to enforce it. **Two measurements say that framing is wrong, and the second one caught
me writing a bad test.**

**(a) The value is unreachable above 2, and above 1 on six of seven variants.**
`effective_bound()` clamps any persisted bound to the variant's `default_bound()`, one-way. Every
variant default is `max_in_flight = 1` **except `Event`, which is 2**. So `CEILING_IN_FLIGHT = 16`
is **decorative** — it bounds a value every variant default already bounds at 2 or below, and no
input reaches it.

**(b) The runner never reads the field.** `runner.rs` enforces `is_spent` (deadline) and
`min_interval_secs`; `max_in_flight` appears **zero** times. Asserted as a source census with a
**known-positive control on both sibling fields**, so a census matching nothing cannot pass.

### The instrument defect I produced getting there

My first version of `in_flight_bound.rs` set a persisted bound of **8** on an `interval` job,
drove a concurrency probe, observed peak 1, and reported that as a measurement of what a bound of
8 buys. **It was a measurement of a bound of 1** — the clamp had narrowed it and the test never
read the effective value back. The suite was green.

It was caught by **driving the real `cron status` verb**, which printed `max_in_flight=1` for the
job the test believed carried 8. A live drive corrected a green test. That is LANE-BRIEF §3.1
doing exactly what it is for, and it is the second instrument defect of mine in this lane.

Repaired: the concurrency probe is **deleted**, because dispatch is awaited inline and a
behavioural probe reports peak 1 on *any* implementation — a tautology wearing a measurement's
clothes. The replacement asserts the clamp directly, with a positive control proving the persisted
value really carried the ceiling before the clamped result is believed.

### `F24-C2-M1` (NEW, MEDIUM, non-blocking)

> `cron status` states `max_in_flight=2` on every event job. The runtime serializes fires and
> never reads the field. `CEILING_IN_FLIGHT = 16` is unreachable by any input.

Same shape as the `poll:` trigger this phase already retired — a surface stating behaviour the
runtime does not implement. MEDIUM, so per LANE-BRIEF §5 it goes to BACKLOG, not to a fix loop.
The **operator-facing half is closed now** by the annotation; making the field real is a dispatch-
model change (Rule 4 architectural) and I did **not** invent one.

Driven live, both arms, with an alive control:

```
--- event job ---
bound:       min_interval=1s max_in_flight=2 deadline=none
             NOTE: fires are serialized; max_in_flight>1 grants no concurrency in this build
ARM_A_NOTE=1   (expect 1)
--- interval job ---
bound:       min_interval=900s max_in_flight=1 deadline=none
ARM_B_NOTE=0   (expect 0)
CONTROL: arm B 'max_in_flight' lines: 1   (the grep is alive)
```

---

## 4. Live evidence — the support bundle, end to end

All on `hetzner-dsm`, against `target/debug/wayland-core` built from this branch.

**The verb is in the shipped binary's `--help`:**
```
  support-bundle  Collect a REDACTED evidence bundle an operator can attach to a ticket
```

**A real gateway, running:** `gateway run --detach` → pid `2999135`; `gateway status` →
`Running`, uptime 1s, binary `…/target/debug/wayland-core`.

**A canary seeded into three real sources** (`config.toml` `api_key`, `credentials.toml`, and a
log line). Positive control before anything else: `/usr/bin/grep -c -F` → `1, 1, 1`.

**The bundle, produced by the shipped binary while the gateway ran:**
```
support bundle: /root/wl-24c4-bundle
  gateway alive:   true
  members:         8
  known secrets:   2
  redactions:      1
  absent sources:  none
```

**The redaction actually happened**, both layers:
```
config-keys.txt      api_key  [value elided]          <- structural elision
recent-log.txt       ... using token [REDACTED] ...   <- exact-secret scrub
```

**Canary sweep across every byte of every member: 0**, with the instrument proved alive in the
same invocation (`grep -rc -F REDACTED` → `recent-log.txt:1`).

**The `#[ignore]`d live gate, driven:**
`F24_LIVE_BUNDLE=… F24_LIVE_CANARY_FILE=… F24_LIVE_SEEDED_DIR=… cargo test … -- --ignored
live_bundle_canary` → **1 passed, 0 failed, 0 ignored, 4 filtered out**. Not a zero-test suite:
the executed count was read back.

**And that gate was proved able to fail, twice:**
- plant the canary in a copy of the bundle → **`A_RC=101`**
- point `F24_LIVE_SEEDED_DIR` at an unseeded directory (the positive-control leg) → **`B_RC=101`**

### §3b-ii — read the credential back from the product's own output

The brief warns that `/root/.wayland/.env` injects `ANTHROPIC_API_KEY` regardless of the shell.
I did not assume either way; I read it off the bundle:

- Under my isolated `WAYLAND_HOME`, `environment-keys.txt` contains `ANTHROPIC_API_KEY` **0**
  times — because `main.rs:951 load_wayland_env_file()` reads `$WAYLAND_HOME/.env` and my home had
  none. **Not a defect.** I record it because for ten minutes it looked like one.
- Under the **real** home, the same file contains
  `ANTHROPIC_API_KEY  [value elided: name marks a secret]` — **1** hit. So the scrubber does learn
  the host's injected credential on a real deployment.
- **Sweep:** the real key VALUE appears in **0** files of that bundle, while
  `/usr/bin/grep -r -F -l` with identical flags finds it in `/root/.wayland/.env` (**1**). A
  differential negative, not a bare zero. That bundle was then deleted.
- No credential was read into, printed by, or committed to anything in this lane. The value was
  never echoed. Hits in my evidence directory: **0**.

---

## 5. What I did NOT do

- **`24-C2` webhook + poll** — cut by Sean 2026-07-29 as scorecard work. Not built. The verdict
  lists them at §9 items 3-4; they are **cut**, not open.
- **`24-C2` continuation gate** (verdict §9 item 5 — hard-kill a gateway mid-fire, count at an
  out-of-process sink). Not attempted. Still open, still non-blocking.
- **`24-C2` macOS automation evidence** (item 13). Not attempted — no permitted host runs macOS
  and the Mac cannot build. A rule-imposed gap, not a refusal.
- **Every `24-C3` residual** (items 6-11). Not attempted. I chose one C2 item and took it
  properly rather than three half-way; given the two instrument defects that item produced, I
  think that was the right call, but it does mean C3 is exactly where the verdict left it.
- No merge, no PR, no tag, no release, no issue closed, no `wcore-contract generate`, nothing
  under `.github/`. Nothing compiled on the Mac (`cargo fmt` only).

### One thing I found and deliberately left

`main.rs`'s doc string for the `Gateway` subcommand lists seven verbs and omits **both**
`abandoned` (pre-existing) and `support-bundle`. It renders as the `gateway --help` about-line.
`main.rs` is a **fenced file** and a doc-comment amendment is a modification, not an additive
block, so I did not take fence exposure for a stale help string. Cosmetic; flagged for whoever
next has a legitimate reason to edit that file.

---

## 6. Per-criterion status after this lane

| criterion | verdict (2026-07-29 am) | after this lane |
|---|---|---|
| 24-C1 | MET-WITH-STATED-EXCEPTIONS | unchanged — not re-derived |
| **24-C2** | PARTIAL | **PARTIAL.** Webhook/poll are CUT, not gaps. The `max_in_flight` residual is now measured and the operator surface is honest; the dispatch model is unchanged. Continuation still NOT MET. |
| 24-C3 | PARTIAL | unchanged — not touched |
| **24-C4** | **PARTIAL** | **MET-WITH-STATED-EXCEPTIONS.** Half one was already met on HTTP/SSE; half two is now reachable, driven from the shipped binary, and canary-proved live. Exceptions unchanged: REST `/v1` has no resume or idempotency, stdio and WebSocket have none of the three, and everything is Linux. |
| 24-C5 | MET-WITH-STATED-EXCEPTIONS | unchanged — not re-derived |

**On the release question:** the verdict's §8 named `F24-C4-H1` as the phase's one remaining
blocker and costed it at ~0.5 lane-sessions. It is closed. Every other item that verdict listed is
recorded there as can-ship-open, and nothing I measured today contradicts that. I am **not**
declaring the phase goal achieved — I re-derived one criterion, not five, and `support` was only
one of seven verbs in that sentence.

---

## 7. Findings

| id | sev | state |
|---|---|---|
| `F24-C4-H1` | HIGH | **CLOSED.** Verb built, wired, driven live; the live canary gate executed and proved able to fail. |
| `F24-C2-M1` | MEDIUM | **NEW, open, non-blocking → BACKLOG.** `max_in_flight` is unreachable above 2, unread by the runner, and `CEILING_IN_FLIGHT` is decorative. Operator-facing half closed by the `cron status` annotation. |

Two instrument defects of my own, both repaired in-lane rather than only noted (§2 M3, §3).

---

_Lane `24-c4-support`, 2026-07-29. Evidence: `24-C4-SUPPORT-evidence/`. Working trail:
`24-C4-SUPPORT-NOTES.md`._
