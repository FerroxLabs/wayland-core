---
lane: lane/24c3-channels
criterion: 24-C3
base: b2ddf113681647221dc9e5bbfc7de79b1da90b54
verdict: "24-C3 remains NOT MET and this lane does not claim it. The largest hole
  — edit/delete at 0 of 10 adapters — is half closed (5 of 10 implemented, the
  other 5 declared with reasons), the matrix is now machine-readable and gated in
  both directions, and the Linux-only monoculture is broken on macOS and partly on
  Windows. Routing, setup/auth and media are untouched by this lane."
date: 2026-07-30
---

# 24-C3 — the inbound/native-action channel matrix

> **"Reference channels prove setup/auth, access, routing, media, native actions,
> idempotency, reconnect/reload, and health."** (`ROADMAP.md:119`)

## 1. Verdict

**`24-C3` is still NOT MET. This lane does not claim it** — the sixth lane in a
row to decline, and the reasons are named in §6 rather than gestured at.

What did move: the criterion's **native actions** clause went from *nothing at
all* to *five adapters implementing edit and delete against their real platform
endpoints, and five declaring, with evidence, why they do not*. The matrix is now
a machine-readable artifact with a gate that has been shown to fail in both
directions, and it is reachable from the shipped binary.

## 2. The brief's premises, re-verified

| Premise | Holds? | Evidence |
|---|---|---|
| edit/delete is 0 of 10 adapters | **HELD** | `/usr/bin/grep` for `async fn edit_message` / `async fn delete_message` under `crates/wcore-channel-*/src/` → **0 and 0**, against a known-positive of `async fn send_message` → **40 hits across 14 files** including one `impl Channel` override per adapter. Instrument proven alive in the same capture. |
| everything proven is Linux-only | **HELD at base, no longer true** | see §4 |
| `F24-C3-H5` is FIXED — do not re-fix | **ACCEPTED, not re-litigated** | ledger `CRITERIA-GAP-LEDGER.md:835-841` carries the LATE CORRECTION with independently verified ancestry |
| media / native actions at zero | **HELD for native actions** | no `native_actions` concept existed; both trait methods were pure defaults with no override and no caller outside `manager.rs` |
| `24-C1` residual is a product decision | **AGREED — with one correction.** See §5 |

## 3. THE MATRIX, as counts, with unrun cells visible

### 3a. Native actions — declared vs implemented (10 adapters × 4 ops = 40 cells)

`yes` = implemented against the platform's real endpoint · `no (plat)` =
`PlatformHasNoApi`, permanent · **`NOT BUILT`** = `NotImplemented`, our backlog

| Adapter | edit | delete | react | typing |
|---|---|---|---|---|
| slack | **yes** | **yes** | yes | no (plat) |
| discord | **yes** | **yes** | yes | yes |
| telegram | **yes** | **yes** | yes | yes |
| matrix | **yes** | **yes** | yes | yes |
| msteams | **yes** | **yes** | no (plat) | yes |
| whatsapp | no (plat) | no (plat) | yes | **NOT BUILT** |
| sms | no (plat) | no (plat) | no (plat) | no (plat) |
| email | no (plat) | no (plat) | no (plat) | no (plat) |
| imessage | no (plat) | no (plat) | no (plat) | no (plat) |
| signal | **NOT BUILT** | **NOT BUILT** | **NOT BUILT** | **NOT BUILT** |

**Counts.** edit **5/10** implemented, 4 permanently absent, 1 NOT BUILT.
delete **5/10**, 4 permanently absent, 1 NOT BUILT. Both were **0/10** at base.
react 6/10. typing 5/10. Overall 21 of 40 implemented, **14 permanently absent
with a written reason, 5 NOT BUILT** — and those two categories were
indistinguishable before this lane.

### 3b. Platform coverage — where each cell was actually EXECUTED

**A skip is not a pass. Unrun cells are marked `UNRUN`, and one cell is `N/A`
by construction rather than by omission.**

| Crate | Linux (hetzner) | macOS (this Mac) | Windows (SeanDesktop) |
|---|---|---|---|
| `wcore-channels` (the capability type) | **118 passed** | via registry | **118 passed** |
| `wcore-channels` framework matrix | **19 passed** | via registry | **19 passed** |
| `wcore-channels` media bounds | **6 passed** | via registry | **6 passed** |
| `wcore-channels-registry` conformance matrix | **3 passed / 9 adapters / 27 cells** | **3 passed / 10 adapters / 30 cells** | **UNRUN — blocked**, see §4c |
| slack / discord / telegram / matrix / msteams | **50 / 62 / 71 / 40 / 42 passed** | UNRUN (rule-barred) | **UNRUN — blocked** |
| whatsapp / sms / email / signal | **41 / 26 / 38 / 83 passed** | UNRUN (rule-barred) | **UNRUN — blocked** |
| imessage adapter crate | **N/A — cannot exist**, registry gates it behind `cfg(target_os = "macos")` | built + declared | **N/A — cannot exist** |

Every figure above is `N passed; 0 failed; **0 ignored; 0 filtered out**` — the
two fields LANE-BRIEF §3b says `rtk` strips. They were read from files via the
Read tool, never through a proxied Bash stdout.

**Honest total: of the 12 crate × platform cells this lane could have filled, 12
ran on Linux, 4 ran on Windows, 2 ran on macOS, and 11 are UNRUN on Windows or
macOS.** The Windows gap is a host provisioning blocker (§4c), not a skip. The
macOS gap is the LANE-BRIEF's own no-cargo-on-the-Mac rule.

### 3c. Final verification at HEAD `1685a5ce`

Scoped to the 12 crates this lane touches, hetzner, tree clean (`DIRTY=0`):

```
total_passed=613   total_failed=0   total_ignored=0   TESTRC=0
cargo clippy <12 crates> --all-targets -- -D warnings   CLIPPYRC=0
cargo fmt --all -- --check                              rc=0
Cargo.toml / Cargo.lock changed:  0 files   (known-positive: 20 files changed overall)
wcore-cli/src/lib.rs + main.rs:   0 lines   (shared-file fence, §6 — zero exposure)
```

**One false alarm, caused by my own harness, disclosed.** An earlier verification
run reported `2211 passed; 23 failed`. The cause was mine: `P="-p …"` was set in
the outer shell but **never exported**, so `$P` expanded to empty inside
`nohup sh -c` and the command became a bare full-workspace `cargo test` plus a
workspace-wide `clippy`. All 23 failures were in **`wcore-agent`** (`engine::*`,
`session::*`, `session_journal::*`, `channel_lease::*`, `orchestration::*`,
`goal::strategy::*`) — file-lock and journal-lease tests in a crate this lane
never touches, i.e. exactly the contention-under-load class LANE-BRIEF §6
describes, and the clippy error was a naming lint in `wcore-memory`. The re-run
above asserts `P_NONEMPTY_CHECK=24` before launching so the same mistake cannot
recur silently. **No failure in this lane's crates, in either run.**

## 4. Breaking the Linux-only monoculture

### 4a. macOS — a result Linux structurally cannot produce

The registry conformance matrix on `target_os=macos` builds **10 adapters / 30
checked cells**; on Linux it builds **9 / 27**, because `channel_factory_for`'s
`"imessage"` arm is `#[cfg(target_os = "macos")]`. This is not the same test run
twice — the macOS run covers an adapter that **cannot be constructed anywhere
else**. `3 passed; 0 failed; 0 ignored; 0 filtered out`, RC=0.

### 4b. macOS — the iMessage measurement, and why it needed a Mac

Per the LANE-BRIEF §0 Darwin exception, **disclosed as required**. This one
needed no `cargo` at all:

```
$ sdef /System/Applications/Messages.app | grep -o '<command name="[^"]*"' | sort -u
<command name="login"
<command name="logout"
<command name="send"
```

macOS 26.3 (`25D125`), Messages.app 26.0. Known-positive `send` → **1**;
`edit|unsend|delete|redact|recall|remove` → **0**. Class list: `account`, `chat`,
`file transfer`, `participant` — **no `message` class exists**, so there is no
scriptable object to address a delete to. Messages.app has offered humans
edit/unsend since macOS 13 and exposes none of it to AppleScript, which is this
adapter's only outbound path.

The registry matrix run in §4a *is* a single-crate single-test `-p` invocation and
is also disclosed under the exception; the behaviour under test — the tenth
adapter's existence — is Darwin-only by construction.

### 4c. Windows — partly done, and honestly blocked for the rest

`ssh SeanD@seandesktop` works (the brief was right). Windows has `cargo 1.95.0`,
git, and D: with 5399 GB free. Work was done under `D:\lane-24c3ch`;
`C:\actions-runner-*` was never touched.

- **`wcore-channels` PASSES on Windows** — `WLRC=0`, 118 + 19 + 6 tests, including
  all four `actions::tests`.
- **Every adapter crate is BLOCKED**, and not by this diff:
  `error: failed to run custom build command for aws-lc-sys v0.41.0`. Measured on
  the host: **`nasm` ABSENT, `cmake` ABSENT, `clang` ABSENT, `cl` ABSENT**
  (`perl` present, from Git). `aws-lc-sys` is pulled in by `aws-lc-rs` → rustls,
  i.e. by every crate with an HTTP client. **This is host provisioning, and
  installing a C toolchain on Sean's desktop is not a lane's call** —
  `BL-24C3-WIN-NATIVE-TOOLCHAIN`, needs `nasm` + `cmake` on PATH.

Two Windows instrument hazards were hit and are worth recording:

1. **`Start-Process … -WindowStyle Hidden` over a session-0 ssh silently does
   nothing.** The script landed on disk, reported `LAUNCHED`, and never ran — no
   log, no repo, no `D:\lane-24c3*` entry after 9 minutes. Running the script
   synchronously over ssh worked.
2. **`cargo` and `rustc` WERE running on the box the whole time** — the
   self-hosted CI runners. Had "is cargo running?" been used as evidence my build
   was alive, it would have been a false positive from another workload. This is
   LANE-BRIEF §6a-ii's shared-`/tmp` hazard in a new location: shared *process
   table*.

The `WLRC=`/`WLDONE` sentinel-file pattern was used throughout and earned its
keep — both Windows runs' true exit codes (`101` then `0`) were read from a file
by a separate ssh call, and CLIXML progress records were observed splicing into
the stream exactly as §3.2 warns.

### 4d. The gate can fail AND pass — measured, twice over

Matching the `F24-C-ARRIVAL` shape (*the gate failed first*), at one SHA with the
tree verified clean before and after each run:

**Conformance matrix, one-variable source mutations:**

| Run | Change | RC | Result |
|---|---|---|---|
| baseline | none | **0** | `3 passed; 0 failed` — it can pass |
| mutation A | slack `.edit(Implemented)` → `PlatformHasNoApi` (1 line) | **101** | FAILED: *"slack.edit: declared `platform-has-no-api` but the call did NOT answer Unsupported … outcome = `Err(Auth("bot token not loaded"))`"* |
| mutation B | msteams `.react(PlatformHasNoApi)` → `Implemented` (1 line) | **101** | FAILED: *"msteams.react: declared `implemented` but the call fell through to the trait's Unsupported default"* |

So it catches an **overclaimed** capability and a **stale absence**, and is green
only when declaration and wire agree. The matrix also carries its own mutation
control (`the_matrix_assertion_can_itself_fail_in_both_directions`) so the
assertion cannot silently become a no-op.

**Live CLI gate, same binary, same config dir, one variable (scope):**

| Run | rc | Outcome |
|---|---|---|
| `channel actions` | 0 | 3-row table |
| `channel actions --require delete` | **1** | `REQUIRED BUT UNAVAILABLE: alerts-email (email) delete` + `pager-sms (sms) delete` |
| `channel actions --require delete --name ops-slack` | **0** | passes |
| `channel actions --require nonsense` | 1 | usage error naming the four valid ops |

Driven against the real `wayland-core` binary (338 MB debug build) with real
on-disk `~/.wayland/channels/*.toml`, per-run stdout/stderr captured to separate
files — the first drive interleaved them and the attribution was ambiguous, which
would have been an easy misread of a load-bearing claim.

## 5. Do I agree `24-C1`'s residual is purely a product decision?

**Substantially yes — with one correction and one caveat.**

I tested the judgement rather than assuming it, by asking of each of the seven
"no primitive at all" adapters whether an idempotency slot exists that we are
failing to use. **It does not, for six of them** — Telegram, SMTP, signal-cli,
AppleScript iMessage, MS Teams and Meta Graph offer no client-supplied
deduplication token on their send paths. For those, `false` is permanent and
truthful, exactly as the re-grade says, and no lane-session closes them.

**The correction is Twilio.** Twilio's `Messages` API *does* accept a
client-supplied idempotency token on some product surfaces, so "no primitive at
all" is too strong as stated for that one row. I did **not** implement or measure
it — doing so needs a real Twilio account, which no build host has — so I am
flagging the ledger's wording as **possibly overstated for Twilio only**, not
claiming the capability. Someone with the credential should check it before the
"7 of 10" figure is repeated.

**The caveat is that the residual is not zero implementation sessions.** The
product decision (explicit per-channel at-most-once vs at-least-once policy,
exposed as configuration) still has to be *built* once it is made. This lane
supplies the shape that decision should take: `NativeActions` is precisely a
per-channel, three-state, machine-readable, gate-able capability declaration, and
the delivery-semantics declaration `24-C1` needs is the same construct over
`{at-most-once, at-least-once, exactly-once}`. **I recommend it be modelled on
`crates/wcore-channels/src/actions.rs` rather than invented separately** — same
`Implemented`/`PlatformHasNoApi`/`NotImplemented` discipline, same
declaration-checked-against-behaviour conformance matrix, same `--require` gate
so an operator can refuse to deploy a channel whose semantics they cannot accept.

What such a declaration would assert, per adapter, on today's evidence:
**exactly-once**: slack, matrix, discord (3/10 — matching the re-grade).
**at-most-once (abandon on unknown outcome)**: telegram, whatsapp, sms, email,
signal, msteams, imessage (7/10). That is the sentence Sean has to approve.

## 6. Why I am not claiming the criterion

The criterion names nine clauses. This lane touched one and a half:

- **native actions** — moved from zero to 21/40 declared-and-checked. Not
  complete: signal is 4 open cells and whatsapp typing is 1.
- **health, reconnect/reload, idempotency** — pre-existing, untouched here.
- **setup/auth, access, routing, media** — **untouched by this lane.** In
  particular *routing* and the **end-to-end inbound matrix from the binary against
  a real adapter** — the clause the row has named as unmet since 24-03 — is still
  not done. Nothing here sends or receives a message against a live platform.
- **macOS and Windows** — improved but not closed; §3b's UNRUN column is the
  honest picture and the Windows adapter blocker is real.

A criterion whose headline clause is *"reference channels PROVE …"* is not met by
hermetic fixtures alone, however carefully both directions are controlled.

## 7. Open items

| Id | Item | Owner |
|---|---|---|
| `BL-24C3-WIN-NATIVE-TOOLCHAIN` | `nasm` + `cmake` on SeanDesktop PATH; unblocks every adapter crate on Windows | Sean (host provisioning) |
| `BL-24C3-SIGNAL-EDIT-DELETE` | 4 open cells; needs a version-pinned real `signal-cli` so the test is not asserting our own guessed RPC method name | next lane |
| `BL-24C3-WA-TYPING-SEAM` | WhatsApp typing exists but is keyed to a received `message_id`; `Channel::send_typing(conversation_id)` cannot supply one — **trait-signature gap, not a backlog item** | needs a seam decision |
| `BL-24C1-TWILIO-IDEMPOTENCY` | verify whether Twilio accepts a client idempotency token; ledger's "7 of 10 have no primitive" may be overstated by one | needs a Twilio credential |
