# 24-RECONNECT — running notes

Lane `24-reconnect`. Branch `lane/24-reconnect`. Merge-base captured ONCE, as a SHA:

```
BASE=15cda12d6a189d7cad3daf0998eded4710f809af
```

(`/usr/bin/git merge-base HEAD plan/f20-unified-audit-repair` at T+0; `/usr/bin/git`, unproxied.)

Committed at T+13 min before any measurement, per LANE-BRIEF §6b-i. Appended and re-committed
after every measurement.

---

## T+0 — assignment as I read it

`24-C3`'s `reconnect/reload` clause has two halves.

- **reload** — a new adapter added to config and picked up by `channel reload`. Driven by
  `24-C3-FINISH` (found `F24-C3-H5`: reload registered the adapter, reported healthy, denied
  every message because the inbound access policy was never reloaded), fixed and live-proven by
  `24-H5`.
- **reconnect** — the *remote* side goes away under a running process. **Never driven.** Every
  prior lane says so explicitly:
  - `24-C3-FINISH` §7: "Did not measure the `reconnect` half of reconnect/reload, only `reload`."
  - `24-H5-SUMMARY` §8: "Did not measure the `reconnect` half ... Only `reload`."
  - `24-H6-SUMMARY` §6: "Did not measure the upstream-drop half of `reconnect`. My probe
    restarts the *process*; dropping the *connection* under a running process is untouched."

`24-H6` restarted the **process**. That is a different event from the **connection** dropping
under a process that stays up: the process-restart path is now cursor-persisted on disk, whereas
an in-process reconnect can recover from in-memory state and never touch the file. So H6's fix
does NOT imply this half.

**I am NOT grading `24-C3`.** Seven lanes have declined it correctly and nothing I can do in one
lane moves `media` or `native actions`, which have zero evidence on every adapter. I grade the
`reconnect` half only, per adapter, and name the adapters I did not reach.

## T+0 — the defect shape I am hunting

Five silent inbound message-loss defects in two days, all invisible from source review, all only
visible when the real product was driven. The most recent (`F24-C3-H6`, matrix) was exactly this
shape: **everything delivered while the process was down was lost**, because the sync cursor was
process-local. Assume a sixth exists until measured otherwise.

For an upstream disconnect the loss window is the same but the mechanism differs. The question is
not "does it reconnect" — a health probe answers that and answers it green for a broken adapter.
The question is **"is every message delivered around the disconnect window accounted for, exactly
once."** So the measurement is a census: N delivered before + M during + K after, and I derive
losses and duplicates myself from the sink journal. **I do not trust any figure the product
reports about itself.**

## T+0 — adapter taxonomy (to be verified against source, not assumed)

Upstream-drop is only a meaningful event for adapters that *hold* or *initiate* an upstream
connection. Adapters we merely receive webhooks on have no upstream to drop from our side.

| adapter | expected upstream shape | drop is expressible? |
|---|---|---|
| discord | WebSocket gateway, `op6 RESUME` w/ replay already in fixture | **yes — the sharpest target** |
| matrix | long-poll `/sync`, cursor now persisted (H6) | **yes** |
| telegram | long-poll `getUpdates`, `offset_store` on disk | **yes** |
| email/imap | IMAP connection + `uid_store` on disk | yes, but email has a pre-existing SMTP/webpki blocker on every prior run |
| slack / whatsapp / sms | inbound webhook — *they* connect to *us* | probably not; verify |
| signal | verify | ? |
| msteams / imessage | undriven / platform-blocked | no |

TO VERIFY IN SOURCE before building anything.

## T+0 — rules of evidence I am binding myself to

1. **Every absence claim needs a known-positive in the same invocation and a decoy proving the
   detector fires.** "No messages were lost" is self-passing on a dead instrument. Broken grep,
   typo'd path, unquoted glob, wrong tree — every one of those returns a zero for free.
2. Every claim gets a **one-variable negative control proven to redden**.
3. Load-bearing numbers come from `/usr/bin/grep`, `/usr/bin/git`, `/usr/bin/env cargo`.
   `rtk` rewrites `git log` (drops merges), `grep` (reported 9 matches in 7 files for a one-file
   search whose true answer was 0), `cargo` (strips `0 ignored` / `0 filtered out`), and `wc -c`
   (returned 0 for a 72-byte file — cross-check byte counts two ways).
4. Read back `N passed` counts; never trust exit status. A suite exits 0 having run zero tests.
5. Compile only on `hetzner-dsm`. Never on the Mac.

## T+0 — concurrency hazard, inherited and NOT yet verified fixed

`24-H6` §8.0 reported that `f24-inbound.mjs` binds a **fixed** `127.0.0.1:18787` and its launcher
`pkill`s three **global** patterns, so two lanes destroy each other's runs. My dispatch says a
prior lane repaired this — `F24_WEBHOOK_PORT` is now honoured and the pkill patterns are scoped.
**I will verify that in the source before running anything**, because taking it on trust is how
I'd both lose my own run and damage four other lanes'.

**I will pick a port no other lane would pick.** Four other lanes are live.

## T+0 — plan

1. Verify the harness concurrency repair actually landed (read the source, do not trust).
2. Read each candidate adapter's connection loop in source; establish where an upstream drop is
   even expressible and what state (if any) survives it.
3. Build a probe that induces a genuine upstream disconnect against the hermetic fixture, with a
   before/during/after message census and self-derived loss+duplicate counts.
4. Prove the probe can fail: decoy message it must miss, known-positive it must find, and a
   one-variable product mutation that reddens it.
5. Drive `gateway run` (the real installed surface), not `--json-stream` only.
6. Grade per adapter. Name what I did not reach.

---

## Log

- **T+0** worktree verified `/Users/seandonahoe/dev/waylandcore-frontier-worktrees/lane-24-reconnect`,
  branch `lane/24-reconnect`, HEAD == BASE == `15cda12d`. Fence exposure at this point: zero by
  construction (no commits yet).
- **T+13** this file committed before any measurement.
