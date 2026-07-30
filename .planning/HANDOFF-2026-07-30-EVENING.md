# HANDOFF — Wayland Core — 2026-07-30 late evening (rev 2)

Integration `plan/f20-unified-audit-repair` @ **`58e0f533`** (local == remote, verified).
Supersedes `HANDOFF-2026-07-30-UAT.md` and rev 1 of this file.

**`.planning/LANE-BRIEF.md` outranks any orchestrator instruction, including this file.**

Written to survive an **account switch**: a fresh session with zero memory of the conversation
should resume from this file alone. §1 is the first five minutes.

---

## 1. DO THIS FIRST

1. **Three lanes are reported, verified by their own gates, and NOT merged** — §3a. Merge them on
   the cadence in §2 before anything else.
2. **`cargo clippy --workspace --all-targets -- -D warnings` is now 0 at integration** and is part
   of the gate. Keep it that way; it was 9 errors across 4 crates this morning.
3. **CI itself is not running** — §5. Do not read a green badge as a verdict.
4. **The durable-session decision has been reframed by measurement** — §6. Read it before touching
   anything under `wcore-config/src/config.rs`, `wcore-agent/src/recovery*.rs`, or
   `crates/wcore-cli/tests/f14_sigkill_recovery.rs`.

---

## 2. MERGE CADENCE

One lane at a time:

1. `git fetch gh 'refs/heads/lane/<x>:refs/remotes/gh/lane/<x>'`
2. **Check for commits after the lane's own gated SHA.** Lanes gate, then commit evidence. If the
   post-gate delta contains source, re-gate.
3. **Scan incoming evidence for credential values yourself** — loop `~/.wayland-secrets/*.env`,
   `git grep -F` each value, prove the grep alive on a known positive. Do not trust the lane's sweep.
   (Expect hits on `MATRIX_USER_ID` / `MATRIX_ROOM_ID` — identifiers, not secrets. See #157.)
4. `git merge --no-ff <ref> -F <msgfile>` — **never backticks in a shell-quoted message.**
5. Push to scratch ref `orch-verify-<x>`.
6. On hetzner: `cd /root/wayland && git checkout -- . && git fetch origin <ref> && git checkout FETCH_HEAD`,
   then **assert the SHA programmatically**.
7. Gate: `fmt --all --check` + `metadata --locked` + `check --workspace --all-targets`
   + **`clippy --workspace --all-targets --keep-going -- -D warnings`**.
8. Push to integration; delete the scratch ref.

**A test gate now exists** — `.planning/scripts/merge-test-gate.sh`, with
`.planning/merge-test-baseline.txt`. It is **differential against a committed known-red baseline**,
because integration is red on three tests and an all-green gate would have no reachable pass state.
Full workspace nextest measured 216s loaded / 76s warm. Its can-fail proof uses the **real** keyring
incident, not a synthetic seed. **Wire it into the cadence** — it is not yet a step above.

### Correction to a claim I repeated all day
"The cadence runs no tests" was **false**. Lanes do test. The real gap is finer: each lane tests only
the crates it *changed*, the test that broke lived in `wcore-cli` whose source nobody touched, and the
one workspace-wide gate compiles everything and **runs nothing**. "Make lanes test more" would not
have caught it.

---

## 3. LANE STATE

### 3a. Reported and verified, NOT merged — do these first

| branch | HEAD | what |
|---|---|---|
| `lane/fix-keyringless-inbound` | `6bb5a604` | keyring-less inbound **proven live** via Twilio; guards writer #3; wires `durable_sessions_disabled_by_host()` into `doctor` |
| `lane/decision-record` | `a3df23de` | ADRs `docs/decisions/0003` + `0004`; found #159 |
| `lane/effect-accounting` | `599f6183` | budget/approval measurement; `cache list` aggregate; found #160 |

Note `lane/fix-clippy-gate-negative-control` and `-negctl2` exist and are **deliberately red** — do
not merge them.

### 3b. Merged today (13 + 3 = 16 lanes)

Keyring blocker · TUI first-message · macOS verification · channel health (Matrix, then Slack+Discord)
· channel onboarding · TUI noise · evidence audit · Windows residuals · **tool-result formatters** ·
**clippy + CI gates** · **durable posture**.

### 3c. Running when written — dead after an account switch, re-dispatch if needed

A **workflow** covering six items (concurrency-safe audit, Core contract defects, self-edit loop,
`Connected`-before-handshake, guarantee honesty + log rotation, boot-walk decision). Session-scoped.
Script persists:

```
Workflow({scriptPath: "/Users/seandonahoe/.claude/projects/-Users-seandonahoe-dev-waylandcore/11929102-d58a-47e9-9644-0e9d530b58c4/workflows/scripts/wayland-tail-closeout-wf_60e19a43-c22.js"})
```

Drop `resumeFromRunId` on a new account. Re-running is safe — its first phase re-checks which items
are still open before spending build time.

---

## 4. WHAT LANDED — highlights only

- **Release blocker cleared**: keyring-less hosts run. **Proven live end-to-end** — real inbound SMS
  turn, `status=delivered`, body `51 FKRI-657418` (`51` = 17×3, so a model really ran; token from
  `/dev/urandom` seconds earlier, so not a replay).
- **TUI first-message loss**: 4/20/37 chars destroyed → **0**, at 7.1 chars/sec. macOS verified at a
  tree hash byte-identical to integration.
- **Tool-result rendering**: **11 of 12 formatters** were reading a payload shape nobody produces.
  Every successful web search had been rendering `Found 0 results`.
- **Clippy**: 9 errors / 5 targets / 4 crates → **0**, exhaustiveness proven on a `cargo clean`-ed tree.
- **`channel health` no longer lies**; **channel onboarding works**; **TUI is quiet** (stderr 41 → 2).
- **`[session] require_durability`** with a five-boundary SIGKILL matrix carrying anti-vacuity
  assertions.

---

## 5. TAG BLOCKERS

**1. CI IS NOT RUNNING.** Last 100 runs on integration: **91 cancelled, 5 failure, 2 success**, and
every sampled cancelled run has `jobs.total_count == 0` — they never started. Cause:
`cancel-in-progress: false` keeps ONE pending run per concurrency group and cancels the older. With
merges landing all day, nearly everything was superseded before starting. **Windows being dark was
not a Windows problem.** Any "CI is green" claim about this branch in the last two days is
unsupported. (#158)

**2. The `vx` toolchain pin does not hold.** A clean-tree run resolved **rustc 1.97.1** against a
`vx.toml` AND a `rust-toolchain.toml` both pinning 1.95.0 — vx prepends its own non-shim cargo.
Every `ci.yml` job using that action is on floating stable. `workflow_synth.rs:388` already fails on
1.97, and that is a lower bound (fail-fast). **Named, not fixed.** (#158)

**3. The `report` job goes green on zero tests** — 14 runs, all `run=failure, report=SUCCESS`. A
hard-red assertion for a skipped test step landed with the clippy merge, but the concurrency problem
means it has barely executed.

**4. Contract regeneration** — orchestrator-only, **last action before a tag**. Fix #142 first or the
regeneration bakes in an unsatisfiable schema. #159 may also force a `ready` shape change — resolve
before regenerating, not after.

**5. The three unmerged lanes in §3a.**

Release infrastructure is verified fine: seed set 2026-07-29, `release.yml` signs and verifies
against the shipped trust root, missing seed is fail-closed. `v0.12.25` is tagged; the cut is **0.12.26**.

---

## 6. DURABLE SESSIONS — reframed by measurement, and now Sean's to ratify

**THE DECISIVE FACT: there is no encrypted journal.** The key protects exactly ONE field,
`sealed_prepared_request` (`recovery.rs:157`). `LEGACY_EVENT_TYPES` (`session_journal.rs:2376`)
already carries **keyless** write-ahead pairs for provider, tool, approval and delivery. A missing key
therefore costs only **replay** — and a single validation branch at **`recovery.rs:331`** escalates
that to total amnesia. Verified independently at merge.

So "no secure store ⇒ no journal" was never a security requirement. It is one `if`.

**Three framings were wrong, in order:** Decision A (2026-07-16, `906287e1`) refused outright.
Decision B (`c73ac417`, today) chose amnesia on the reasoning "nothing at rest to protect". My own
proposal said "degrade the encryption, not the journaling" — assuming an encryption layer that does
not exist.

**A 4-way panel went UNANIMOUS 3/3 on a fourth option the lane invented: JOURNAL WITHOUT THE SEAL.**
That is the recommended target posture. It supersedes A, B, and my proposal.

The same panel corrected a hole I had written into the remedy: **refuse the SESSION, not the PROCESS.**
A global brick hands an attacker who suppresses the keyring an availability kill — the mirror of the
downgrade-abuse risk the earlier panel raised.

**Causation is proven twice** (mine + `lane/decision-record`, independent hetzner runs): at
`b8311575` the f14 test passes 11/0; at `e7bc6d88` it is 10/1. Exactly one test flipped. **Do not
re-run it.**

**Landed:** `[session] require_durability` (Keep|Degrade|Refuse, also closing an untrusted-project-config
downgrade nobody had noticed); a per-turn notice riding the already-contracted `ProtocolEvent::Info`
so **no regeneration is needed**; the two-test pair; the five-boundary SIGKILL matrix.

**STILL OPEN — the headline:**
1. **Option D itself is not built.** Design of record in the lane's `.planning/phases/durable-posture/` §10.
2. **Refuse-when-a-journal-exists-but-its-key-does-not is not built.** Behind it, **HIGH-1, measured**:
   a profile seeded with a real recoverable turn, reopened with `--resume` after its key is gone,
   starts `rc=0` and **emits `ready` carrying that session id** while writing nothing. Six prior
   artifacts sit unread. It asserts continuity it cannot back.

**Corrected premises for anyone revisiting:** the July-16 test also required the journal to EXIST
(line 1143), so `c73ac417` reversed two properties not one; the `:204` panic is the READY handshake,
not the journal read; and "creates no `sessions/` directory" is false where the directory already
exists — it stays empty.

---

## 7. OPEN WORK, RANKED BY WHAT BREAKS IF IT IS WRONG

| # | item |
|---|---|
| 158 | **CI not running** + vx pin bypassed + `report` green on zero tests |
| 160 | **No cross-session spend ceiling exists.** `per_user_daily_usd` has "no TOML counterpart today" (`tracker.rs:55`) — verified. Fresh-session-per-process billed **100,000 tokens under a 25,000 cap, 0 refusals**. Unrun and highest-value: `channel_dispatch.rs:223-247` silently creates a fresh session when the store is empty, unlike the CLI |
| 159 | **Degraded `ready` silently drops `session_id`** (`Option` + `skip_serializing_if`) while the corpus still declares it. Collides with Desktop's new validator |
| 156 | Option D + refuse-on-existing-journal + HIGH-1 (§6) |
| 152 | `is_concurrency_safe`: 45 unconditional `true`s, one consumer batching into **parallel execution**; `doc_tool.rs:215` declares "read-only" then writes a shared temp dir. kubectl/aws/gcloud/sql trues ARE backed — do not "fix" those |
| 142 | **Core's own schema is unsatisfiable** for `goal_snapshot`; `workspace_policy` in neither contract nor DEFERRED |
| — | Engine reports **its own writes** as user edits (`bootstrap.rs:3139`) — infinite "Re-reading now." loop |
| 149 | `gateway.rs:926` publishes `Connected` before the handshake is accepted |
| 153 | Matrix exactly-once holds **only below the message cap** |
| 154 | First paint = two recursive walks of cwd; walk 2's missing prune is deliberate (blocks a sandbox bypass) |
| 155, 157 | Log rotation; personal-identifier scrub before public release |
| 141 | Desktop D6 (18 handlers) + D7 (12 undeclared host commands) |
| 137, 130, 131, 116, 103 | Windows journey (needs delivery identity); BrowserOp::Download; deny.toml 2026-09-02; setenv/getenv; 25-c1 backlog |

Six of these are in the running workflow (§3c).

---

## 8. SEAN'S QUEUE

| item | state |
|---|---|
| **Ratify option D** (§6) | panel unanimous; the measurement is in hand |
| `cache_tier.rs` provenance | **recommendation: STRIP.** Its attribution's one factual claim about the predecessor is provably wrong; shared literals are Anthropic-dictated; containment 0.1000 sits inside the Rust↔Python negative-control band (0.0000–0.1250) |
| `anthropic.rs:307` | genuinely undecidable — exact identifier match, both halves externally dictated |
| Desktop contract branch | `lane/core-contract-integration` @ `475bf309` in the **Desktop** repo. Needs Sean's rebase + PR; `origin/main` moved 10 commits past its base. Sean's 16 in-flight files verified never touched. **Coordinate with #159 first** |
| WhatsApp/SMS delivery identity | product call. **SMS is structurally unidentifiable** (no metadata channel to a handset); **WhatsApp genuinely can** carry a client-assigned ID |
| Rotate tokens | Slack + Discord + Matrix all went through chat |

---

## 9. HOSTS, CREDENTIALS, HARD RULES

- **Builds ONLY on `ssh hetzner-dsm`.** NEVER cargo on the Mac; `cargo fmt` on the Mac IS fine.
  **Debug builds unless release is required** — four concurrent release LTO builds hit load 12.3 and
  cost an hour. Orchestrator owns `/root/wayland`; lanes use their own `/root/` worktrees.
- **Windows: `ssh SeanD@seandesktop`, PowerShell not cmd. Work under `D:\` only.
  NEVER touch `C:\actions-runner-*`** — three live services. (C: has 647 GB free.)
- **Credentials** `~/.wayland-secrets/*.env` mode 600. To hetzner on **ssh stdin ONLY**.
- **Slack is a LIVE COMPANY WORKSPACE** — private channel `C0BLR1UKKU6` only. **Matrix is Sean's
  PERSONAL account** — room `!kntRqkQCkPjhPvMMvf:matrix.org` only, redact test events. Discord test
  channel `1532226655102173318`.
- **Peer trees** `/Users/seandonahoe/dev/resources/*` — READ-ONLY, never execute.
- **Reserved to Sean**: merging to main, PRs, tags, releases, closing GitHub issues.
- **Never `git rebase` or `git reset --hard`** — ~200 lane branches share the object store.
- Lanes may not run `wcore-contract generate`.
- `gh auth switch --user FerroxLabs` before every `gh` op. Issues on `FerroxLabs/wayland`, code on
  `FerroxLabs/wayland-core`.

---

## 10. STANDING LESSONS — every one earned by a false green

- **A mock proves what we send and nothing about what the destination does.** Three false
  `exactly-once` claims, `channel probe`'s unmeasured negative, and **11 of 12 tool formatters**
  testing their own invented payload shape. One disease, three surfaces, one day.
- **Absence claims need a control.** Roughly eight orchestrator premises were measured FALSE today;
  every one was an unverified absence or an unchecked assumption. **The lanes that refuted me did the
  most valuable work of the day** — including proving that the thing I proposed protecting
  (an encrypted journal) does not exist.
- **A skip is not a pass.** A causation run reported `PRE_RC=0 POST_RC=0` and looked like proof; it
  had measured `0 passed; 0 failed; 12 filtered out`. **Read the counts, never the exit code.**
- **Both directions, always.** A gate that cannot pass is as worthless as one that cannot fail. Found
  four times today: the Windows journey gate, a credentials-absent TUI quadrant, Desktop's
  `test:contract` passing via `--passWithNoTests`, and a lane's own Slack-only probe control.
- **Anti-vacuity is the difference between a matrix and a decoration.** The SIGKILL matrix asserts its
  own provider-request count AND requires a durable run to leave residue. Copy that pattern.
- **Grade off code and executed tests, never a `SUMMARY.md`.** One lane's notes claimed a quadrant
  passed while its own committed artifact said `MISMATCH`.
- **Every lane finds instrument defects in its own harness — budget for it, it IS the work.** ~60
  today. The macOS harness would have reported all three TUI fixes BROKEN because macOS `awk` does
  not interpret `\xNN`. A `pkill -f` on an env var matched nothing, so five gateways piled onto one
  health file and invalidated four arms. A `shutil.copy` that lost mtime made cargo skip a rebuild, so
  an ablation ran the *previous* binary against a clean `git status`.
- **`rtk` FABRICATES machine-readable counts** — 0 lines for a 15-line file. It also rewrites `find`,
  `sed` and `cat`. Redirect to a file, read with the Read tool.
- **Never backticks in a shell-quoted commit message** — write to a file, `git commit -F`.
- **Assert the SHA after every checkout.** A checkout that aborts on a dirty file reports the OLD SHA
  and looks successful.
- **Adopt, do not re-author.** Two lanes independently wrote the *same* fix for `single_owner.rs`; the
  diff was only the comment. Check sibling branches before writing.
