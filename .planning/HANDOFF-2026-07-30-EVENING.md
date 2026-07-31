# HANDOFF — Wayland Core — rev 3, 2026-07-31 ~05:30

Integration `plan/f20-unified-audit-repair` @ **`58aa0267`** (local == remote, verified with
`/usr/bin/git`). Supersedes rev 2 **in place** — one current handoff, deliberately. Three
competing ones is how the ledger drift started.

**`.planning/LANE-BRIEF.md` outranks any orchestrator instruction, including this file.**

Written to survive an account switch: a fresh session with zero memory should resume from
this file alone. §1 is the first five minutes.

---

## 1. FIRST FIVE MINUTES

1. `cd /Users/seandonahoe/dev/waylandcore-ferrox && /usr/bin/git fetch gh`
   Integration = **`58aa0267`**. **87 commits** since rev 2's `674b72c8`; **14 lane merges**.
2. All five gates green at that SHA on `hetzner-dsm`. Baseline is **4 entries**.
3. **Use `/usr/bin/git`, never rtk, for any hash or count.** rtk *fabricated* a commit SHA in
   this repo (`041ae82c` reported for `c9ab048b` — not even a prefix), served a stale
   `git log`, and returned `grep -c` = 0 for a file that matched. Three incidents, one day.
4. Nothing is running — no workflows, no background jobs.

---

## 2. MERGE CADENCE (five gates now)

1. `git fetch gh` the lane ref. **Check the branch NAME carefully** — see §3.
2. **Orchestrator-run secret scan.** Loop `~/.wayland-secrets/*.env`, `grep -F` each value
   ≥12 chars against the incoming diff, and prove the grep alive per value with an injected
   sentinel. Do not trust the lane's sweep. Expect a benign hit on `MATRIX_HOMESERVER` =
   `https://matrix.org` (public URL) and on `MATRIX_USER_ID` / `MATRIX_ROOM_ID` (#157).
3. `git merge --no-ff <ref> -F <msgfile>` — never backticks in a shell-quoted message.
4. Push scratch `orch-verify-<x>`; on hetzner `git checkout -- . && git fetch origin <ref> &&
   git checkout FETCH_HEAD`, then **assert the SHA programmatically**.
5. Gate: `fmt --all --check` + `metadata --locked` + `check --workspace --all-targets` +
   `clippy --workspace --all-targets --keep-going -- -D warnings` +
   **`bash .planning/scripts/merge-test-gate.sh`**.
6. Push to integration; delete the scratch ref.

**Expect the heartbeat flake.** `wcore-swarm::heartbeat_test
worker_writes_heartbeat_during_long_running_task` falsely reddened **four** merges today.
Protocol: confirm the merge touches zero `wcore-swarm` files, re-run twice, proceed if clean.
Tracked as **#935**. `threads-required = 4` took it from ~2-in-3 to ~1-in-4; it still needs
the root-cause treatment its sibling PID race got.

**hetzner can fetch but NOT push.** To land work generated there:
`git format-patch -1 --stdout <sha>` → scp → `git am` locally. Preserves the message exactly.

---

## 3. TRAPS THAT COST REAL TIME TODAY

- **The `-union` ref.** `lane/connected-before-handshake` (`1e87a7b8`) was a strict
  **ancestor** of the real tip `lane/connected-before-handshake-union` (`2b9b594a`). Merging
  the canonical name would have silently dropped the entire fix **and every gate would still
  have gone green**. Always `git merge-base --is-ancestor` when two refs share a stem.
- **zsh does not word-split unquoted variables.** `for x in $LIST` passes one argument. Bit me
  twice. Use arrays.
- **`nextest` prints "1 test run" (singular)** for a single test; a matcher requiring
  "tests run" extracts nothing and reports a vacuous zero.
- **`ld.so --library-path` ADDS a path, it does not replace.** I used it to "hide" a shared
  library and got a clean pass that meant nothing. Use a container. *I built a vacuous control
  while hunting vacuous controls.*
- **`0 tests run` reads as success.** Always parse the executed count.
- **macOS `awk` ignores `\xNN`; `grep -c X || echo 0` emits `0\n0`; BRE `\?` is GNU-only** and
  matches nothing under BSD sed.

---

## 4. WHAT LANDED (14 lane merges + orchestrator work)

**Product:** keyring residuals closed · ledger store-total · `doc_tool` atomic publish
(16,843/17,015 torn reads → 0) · unsatisfiable array-items schema (`oneOf`→`anyOf`) · watcher
no longer reports edits to its own root · eager secret-deny boot walk removed (`openat`
20,031 → 10,017) · **Goal control reachable by a real user** (`issue_goal_control` had ZERO
call sites) · **Discord `Connected` now means the handshake was accepted** · Matrix
exactly-once precondition stated · log rotation + fallback · **voice shipped by default** ·
**the `wayland-core image` subcommand's default arm was dead on the live router**.

**Measurement, first time on real hardware:** 22-C5 M0–M5 on **real Windows** (SEANDESKTOP) —
the row where nothing had ever moved. 27-C2(c) all three baselines on **real macOS**.

**Orchestrator infrastructure:** merge test gate wired into the cadence and its **deletion
escape hatch closed** (a deleted failing test used to satisfy it) · gate preserves its own
failure log · swarm PID race root-fixed · ADR 0005 · **contract regeneration #6**.

### Contract regeneration #6 — the last named tag blocker, closed
`source_inputs_digest sha256:da3aa114… → sha256:5fd22609…`, corpus current at generator 11.
**Counts identical before and after** — 3 child types, 23 commands, 52 events, 159 fixtures —
so no capability was added, dropped or renamed. That comparison *is* the safety check.
It also fixed a baseline red, so the `desktop_contract_corpus` line was removed **in the same
commit** (the ratchet may only shrink). Baseline 5 → 4.

---

## 5. RELEASE-CANDIDATE STATUS — read before tagging

**Every *named* tag blocker is closed.** But "green" tonight means **my gates on hetzner, not
GitHub's**, and the first row below is the one I would not tag through.

| # | Item | State |
|---|---|---|
| **#158** | **CI is not running** — 91 of 100 integration runs cancelled before starting a job | **OPEN. No GitHub verdict exists for any of tonight's work.** |
| #932 | Refused restore **mutates the target** — data loss, proven by digest | OPEN, in baseline |
| #933 | Two regression detectors blinded by the stderr-noise reduction | OPEN, in baseline |
| #934 | `max_message_len` unverified across 8 adapters — caps asserted against themselves | OPEN |
| #935 | Heartbeat flake — the gate's most frequent false red | OPEN |
| #936 | Matrix adapter cannot survive token expiry — no refresh support | OPEN |
| #937 | **HIGH** — media intake refuses every `$TMPDIR` path on macOS | OPEN, unfixed |
| #938 | FluxRouter STT 402 through the product, 200 by direct curl (same key) | OPEN |

**Criteria ledger:** was 5 MET / 10 PARTIAL / 3 NOT MET. Tonight closed **22-C5**, closed
**27-C2 on macOS**, moved **22-C1**, and shipped **27-C4**. `.planning/CRITERIA-STATUS.md`
has **NOT** been re-graded since — do that before claiming an RC, and **re-verify rather than
assume**: several rows there were found FALSE when actually checked.

---

## 6. THE GAPS SEAN ASKED ABOUT — parity, Grok, "everything meant to be there"

Both are **half-done**, and neither is a code problem. Both are *finished work never landed in
the place that makes it count.*

- **Grok parity — recon merged, ledger row NEVER written.** `lane/recon-grok` is an ancestor of
  HEAD and `.planning/intel/RECON-GROK.md` is real work (grok-build pinned at `c68e39f6`,
  v0.2.102, **the only same-language peer** — 610 crates in common). It carries a pin block
  explicitly formatted "ready to paste into COMPETITIVE-LEDGER.md". **It was never pasted.**
  Measured: the ledger has **1** Grok mention, a line saying grok-build is *not* in the CTRL-01
  contract.
- **Grok pull — built, never driven.** `crates/wcore-cli/src/migrate/grok.rs`, 499 lines,
  registered as `PeerSource::Grok`, 6 inline tests. Measured: **0** grok integration tests
  (hermes / quarantine / typed_dryrun all have them), no evidence directory. Code-complete,
  unit-tested, **never pointed at a real grok-build install** — the same mock-proven-never-live
  shape that produced today's two falsified exactly-once rows.

**Each is roughly one lane.** They are the strongest candidates for "everything that was meant
to be there at the end".

---

## 7. NEXT SESSION — research-verification analysis (Sean's ask)

**Across everything we claim: what is verified, what is asserted, and what is asserted by an
instrument that cannot fail?** Suggested spine:

1. **Unmeasured-cell census.** `docs/delivery-semantics.md` marks **7 of 10** adapters "NOT
   MEASURED at a real destination" — exactly the state Slack and Discord were in the morning
   they were falsified.
2. **Vacuous-instrument census.** Today alone: a macOS process-count baseline reporting exit 0
   because it did not exist; two symlink/over-cap arms refused before reaching their gate;
   `max_message_len` asserted against itself at 9 sites (#934); a criterion (24-C1) with no
   reachable pass state; and my own `ld.so` control. This is a **pattern**, not a list.
3. **Observability gaps.** `22-C1` (a command with zero call sites) and voice (no surface
   enumerates the tool registry) are one failure: **we ship surfaces no product command can
   show you**, so "unreachable" and "working" look identical from outside.
4. **Platform coverage** now that Windows and macOS are both reachable — and note **the Mac
   compiles Rust fine** (§10).

---

## 8. SEAN'S QUEUE (only he can do these)

- **Tag / release / PR / merge-to-main / close issues.** All reserved; I have touched none.
- **Matrix token is dead** (`401 M_UNKNOWN_TOKEN`). Mint a dedicated never-logged-out device.
  The login call also settles **#936**: `expires_in_ms` null → token is permanent and this is a
  credential problem; a value → matrix.org moved to OIDC and the adapter needs refresh support.
- **Rotate Slack + Discord + Matrix tokens** — all were pasted in chat previously.
- **#157** — scrub personal identifiers from committed evidence before the repo goes public.

---

## 9. HOSTS AND HARD RULES

- `ssh hetzner-dsm` → `/root/wayland`. Linux builds. **Fetches, cannot push.**
- `ssh SeanD@seandesktop` → PowerShell, Windows NT 10.0.26200. Work under `D:\`, **never**
  `C:\` root, **NEVER touch `C:\actions-runner-*`** (three live runner services).
- **This Mac**: macOS 26.3 arm64 — real audio device, real display, and it compiles Rust.
- **Never print, echo, or commit a credential value.** `~/.wayland-secrets/*.env` mode 600, to
  hetzner on **ssh stdin ONLY** — never argv, never disk, never a log.
- **Slack is a LIVE COMPANY WORKSPACE** — private channel `C0BLR1UKKU6` only. **Matrix is
  Sean's PERSONAL account** — room `!kntRqkQCkPjhPvMMvf:matrix.org` only, never
  join/leave/invite, redact test events. Discord test channel `1532226655102173318`.
- Peer trees under `~/dev/resources/` are **READ-ONLY**. Wayland Desktop `~/dev/wayland` is
  Sean's working tree — worktree only.
- **Never `git rebase` or `git reset --hard`** — ~230 lane branches share the object store.
- `gh auth switch --user FerroxLabs` before every gh op. Issues on `FerroxLabs/wayland`, code
  on `FerroxLabs/wayland-core`.
- Lanes may not run `wcore-contract generate` — orchestrator only, LAST action before a tag.

---

## 10. STANDING LESSONS

- **A gate must be able to BOTH pass and fail.** One with no reachable pass state is worth as
  little as one that cannot fail. 24-C1 was the latter *as a criterion*.
- **A mock proves what we send and nothing about what the destination does.**
- **A cfg-gated test is only honest if BOTH conditions are tested** — otherwise flipping the
  feature retires the assertion and nothing reports a failure. That is exactly how voice
  shipped a false user-facing string tonight.
- **"COMPILES ONLY ON HETZNER, NEVER THE MAC" IS FALSE.** Measured: `cargo check -p
  wcore-types` in **27s**, full `wcore-cli` in **4m56s**, rustc 1.95.0. The rule traces to a
  2026-06-25 design doc and was copied into every handoff since, costing many needless
  round-trips.
- **Report premises measured FALSE.** Roughly nine of my own were refuted in one day, and the
  lanes that refuted them did the most valuable work. Refuting a brief is a success.
- **Delivering the analysis is not delivering the work** — see the Grok ledger row in §6.
