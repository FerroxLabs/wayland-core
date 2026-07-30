# HANDOFF — Wayland Core — 2026-07-30 evening

Integration `plan/f20-unified-audit-repair` @ **`c9ab048b`** (local == remote, verified).
Supersedes `HANDOFF-2026-07-30-UAT.md`.

**`.planning/LANE-BRIEF.md` outranks any orchestrator instruction, including this file.**

Written to survive an **account switch**: a fresh session with zero memory of the conversation
should be able to resume from this file alone. §1 is what to do in the first five minutes.

---

## 1. DO THIS FIRST

1. **Nothing in flight is lost — every lane pushes its own branch.** A session change kills the
   agents, not the work. Recover with `git ls-remote --heads gh 'refs/heads/lane/*'` and compare
   against §3. Anything listed there with commits is real and mergeable.
2. **Three lanes are merged-but-unlanded** — §3a. They have reported, they are verified, they are
   NOT yet on integration. Merge them first, on the cadence in §2.
3. **One decision is open and it is Sean's** — §6. Do not touch
   `crates/wcore-cli/tests/f14_sigkill_recovery.rs:1106` until it is answered.
4. **A tag is blocked on clippy** — §5. Not cosmetic; it has been blinding Windows CI entirely.

---

## 2. MERGE CADENCE — load-bearing, and it grew today

One lane at a time:

1. `git fetch gh 'refs/heads/lane/<x>:refs/remotes/gh/lane/<x>'`
2. **Check for commits after the lane's own gated SHA.** Lanes gate, then commit evidence. If the
   post-gate delta contains source, re-gate.
3. **Scan the incoming evidence for credential values yourself** — loop over `~/.wayland-secrets/*.env`,
   `git grep -F` each value, and prove the grep alive on a known positive. Do not trust the lane's sweep.
4. Merge locally with `git commit -F <file>` — **never backticks in a shell-quoted message**, they ate
   words from three commits today.
5. Push to a scratch ref `orch-verify-<x>`.
6. On hetzner: `cd /root/wayland && git checkout -- . && git fetch origin <ref> && git checkout FETCH_HEAD`
   then **assert the SHA programmatically**.
7. Gate: `cargo fmt --all --check` + `cargo metadata --locked` + `cargo check --workspace --all-targets`.
8. Only then push to integration; delete the scratch ref.

### THE CADENCE HAS THREE KNOWN HOLES. Two are still open.

- **`--locked`** — closed. Lockfile drift broke `--locked` builds twice this week.
- **clippy — OPEN.** The gate never runs it. `check` passes while clippy fails. 18 merges today went
  through a gate that could not see it. `lane/fix-clippy-gate` is building the fix.
- **tests — OPEN.** The gate never runs a test. That is exactly how a deliberate behaviour change
  invalidated an existing test and reached integration unnoticed (§6). Same lane is designing this;
  it was told to *measure* whether a reverse-dependency rule would actually have caught the real case
  rather than assume it.

---

## 3. IN FLIGHT AT HANDOFF

### 3a. Reported, verified, NOT yet merged — do these first

| branch | HEAD | what |
|---|---|---|
| `lane/fix-tui-tool-results` | `74393cb6` | **11 of 12 formatters mismatched**, not just bash |
| `lane/fix-keyringless-inbound` | `6bb5a604` | keyring-less inbound **proven live** via Twilio |
| `lane/fix-clippy-gate` | `8c1a7524` | clippy + cadence gates (may still be running) |

Note `lane/fix-clippy-gate-negative-control` and `...-negctl2` exist — that lane is doing its
both-directions proofs properly. Do not merge those; they are deliberate red branches.

### 3b. Running when this was written (will be dead after an account switch — re-dispatch)

| branch | doing |
|---|---|
| `lane/durable-posture` (`8a829975`) | the §6 rebuild: local keystore, refuse-when-key-missing, policy-gated degrade, crash-boundary tests |
| `lane/effect-accounting` (`8548e834`) | budget + approval durability under degrade |
| `lane/decision-record` | merged ADR for §6 + the process fix |

**A workflow was also running**, six more items (§7). It is session-scoped and will NOT survive.
The script persists on disk and can be relaunched:

```
Workflow({scriptPath: "/Users/seandonahoe/.claude/projects/-Users-seandonahoe-dev-waylandcore/11929102-d58a-47e9-9644-0e9d530b58c4/workflows/scripts/wayland-tail-closeout-wf_60e19a43-c22.js"})
```

Drop `resumeFromRunId` on a new account — resume is same-session only. Re-running is safe: its first
phase re-checks which items are still open before spending any build time.

---

## 4. WHAT LANDED TODAY — 13 lanes, each workspace-verified at an asserted SHA

- **THE RELEASE BLOCKER CLEARED.** A keyring-less host runs degraded instead of dying. Decision moved
  to `session.enabled` in `Config::resolve`, upstream of all three journal writers. *(See §6 — this
  is also the thing that needs a decision.)*
- **Keyring-less inbound PROVEN LIVE**, the actual deployment shape: real SMS turn on a keyring-less
  host, `status=delivered`, body `51 FKRI-657418`. `51` is 17×3 so a model really ran; the token came
  from `/dev/urandom` seconds earlier so it cannot be a replay. `JOURNAL_FILES=0`.
- **TUI first-message loss fixed** — 4/20/37 characters destroyed → **0**, at 7.1 chars/sec human speed.
  Three defects, and the third was worse than the two briefed: completing onboarding rebound the engine
  but discarded the fresh view, stranding users on a workspace with **no input line at all**.
- **macOS verified at integration head** — five quadrants, zero loss, on a binary whose `crates/` tree
  hash is byte-identical to `e7bc6d88`.
- **Windows came back cleanest of four platforms**, and its lane found that Windows CI has been dark (§5).
- **`channel health` no longer lies** — Matrix, then Slack + Discord. Base hammered 9 real 401s while
  reporting `Healthy`; fixed stops after 1.
- **Channel onboarding works** — documented config loads first try instead of four round trips;
  `channel credential set|list|remove` exists; the inert `[secrets]` syntax removed (cross-audit 4/4).
- **TUI is quiet** — stderr 41 → 2 lines, `--help` internal ticket IDs 26 → 0, `RUST_LOG=info` restores
  the old behaviour exactly.
- **The mock-evidence class was measured**, not guessed (§7).

---

## 5. TAG BLOCKERS

**1. Clippy — three crates, and it has been blinding Windows CI.**
Every completed Windows self-hosted job in the last 40 runs **failed (5/5), all before running a single
test**; step data shows `Run tests (nextest CI profile)` = **skipped**, because the job dies at
`Clippy (warnings = errors)`. Since `4d5f8ec9`.

- `crates/wcore-cron/tests/single_owner.rs` — **already fixed** on `lane/fix-windows-residuals` @ `f923161b`,
  no `#[allow]`. The `zombie_processes` lint was a real defect: the leaked child may still hold the lease
  the next case contends against.
- `crates/wcore-browser/tests/process_count_reaper_baseline_test.rs:99`
- `crates/wcore-agent/tests/user_model_identity_wire.rs` — 4 × `needless_borrow`

**My first count of "one file, two errors" was wrong** — the log had ABORTED (`build failed, waiting for
other jobs to finish`, zero mentions of `wcore-browser`). Enumerate with `--keep-going`. Treat the list
above as a **lower bound**; three lanes found three crates and none was looking for clippy problems.

Also required: **CI must not report success when the test step never ran.** A job dying before its tests
is currently indistinguishable from a pass.

**2. Contract regeneration** — orchestrator-only, and the **last action before any tag**. Note §7 found
Core's own schema is currently unsatisfiable; fix that first or the regeneration bakes it in.

**3. The three unmerged lanes in §3a.**

Release infrastructure itself is **fine and verified**: `WAYLAND_RELEASE_ACCEPTANCE_SEED` is set
(2026-07-29), `release.yml` signs the manifest and verifies it against the shipped trust root, and a
missing seed is fail-closed (no manifest → `self-update` refuses → npm route). `v0.12.25` is tagged, so
the cut is **0.12.26**.

---

## 6. THE OPEN DECISION — Sean's, and nothing should pre-empt it

**Two deliberate decisions conflict, and the second was made without knowing the first existed.**

- **A — `906287e1`, 2026-07-16**, *"feat(recovery): seal interrupted turn state"*. Verbatim reason:
  *"Make provider, tool, hook, budget, approval, and host recovery durable so restarts fail closed
  instead of replaying ambiguous effects."* Its test
  (`crates/wcore-cli/tests/f14_sigkill_recovery.rs:1106`) asserts that with no secure store the product
  must NOT reach the provider and must NOT start a turn. Fixture prompt: `MUST-NOT-BECOME-DURABLE`.
- **B — `c73ac417`, today.** Cross-audit 2-1 (DEGRADE vs REFUSE): degrade and run. Reason given:
  *"with no journal there is nothing at rest for that encryption to protect."* **The panel was never
  shown A.**

**Causation proven, measured not inferred:** at `b8311575` the test PASSES (1 passed); at integration
head it FAILS, panicking at `f14_sigkill_recovery.rs:204`.

**A second 4-way panel refuted the orchestrator's defence of B.** All three external legs, independently:
the retry does not come from the engine — it comes from the platform redelivering an inbound, an operator
retrying, or a human resending. A missing record does not remove ambiguity, it **exports it to the user**.
Verdicts: SOUND-WITH-CHANGES / SOUND-WITH-CHANGES / **UNSOUND**.

**The false dichotomy, found independently by two legs:** *"no secure store ⇒ no journal"* was never the
only option. **Degrade the ENCRYPTION, not the JOURNALING.** `WAYLAND_VAULT_PASSPHRASE` already exists;
the gap is only that a host with neither keyring nor passphrase falls off a cliff to nothing.

The agreed proposal, now being built by `lane/durable-posture`:
1. Keep degrade as an **availability-over-accountability trade**, stated honestly — not "nothing to protect".
2. Auto-provision a local encrypted keystore. Amnesia becomes last resort.
3. **Refuse when a journal exists but its key does not** — else the engine forks from unknowable prior state.
4. Crash-boundary tests (SIGKILL before/after provider call, approval, tool exec, delivery). "Zero journal"
   must cover WALs, temp files, partially-renamed artifacts. **The evidence to date was taken after a
   COMPLETED turn and proves nothing about a kill mid-turn.**
5. **Two tests**: degrade-allowed AND degrade-forbidden. Rewriting A's test to only the first silently
   widens the degrade path.
6. Per-turn signal — a daemon started three weeks ago has told nobody anything.

**Two findings that outrank the original question:**
- **Downgrade abuse**: making the keyring unavailable converts "stop" into "run with no audit trail".
- **Budget and approvals are unjournaled too**: every restart is a fresh budget, so cost limits are
  unenforceable across restarts; and a pending approval evaporates, so a human may re-approve a
  destructive action believing it is the first ask — human-mediated replay.

**Process finding, and the most durable thing here:** the 2-1 panel taken without sight of A *is* the bug.
The next panel shown A but not B's blocker evidence flips it back. `lane/decision-record` is merging both
into one ADR.

---

## 7. OPEN WORK, RANKED BY WHAT BREAKS IF IT IS WRONG

| # | item | why it matters |
|---|---|---|
| 152 | `is_concurrency_safe` — 45 unconditional `true`s, one consumer batching into **parallel execution**; `doc_tool.rs:215` declares "read-only" then writes to a shared temp dir | silent races. kubectl/aws/gcloud/sql trues ARE backed — do not "fix" those |
| 142 | **Core's own schema is unsatisfiable** for `goal_snapshot` (`core-event.schema.json` `/oneOf[50]/…/tasks/items`, both branches accept anything). `workspace_policy` in neither contract nor DEFERRED | any host validating our contract rejects a VALID frame |
| — | engine reports **its own writes** to the model as user edits (`bootstrap.rs:3139`) — produced an infinite "Re-reading now." loop | real user-facing bug |
| 149 | `gateway.rs:926` publishes `Connected` before the handshake is accepted | root cause of false `Healthy`; affects every failed handshake |
| 153 | Matrix exactly-once holds **only below the message cap** | our one surviving guarantee, precondition unstated |
| 154 | first paint = **two recursive walks of cwd**; walk 2's missing prune is deliberate (stops `Bash cat node_modules/x.pem` bypassing a deny) | latency vs sandbox; needs a decision, not a patch |
| 155 | new log file has no rotation, now written every headless run | unbounded growth on a gateway host |
| 141 | Desktop: D6 (18 handlers) + D7 (12 undeclared host commands) | Sean's other repo |
| 137 | Windows journey cannot honestly pass | needs WhatsApp/SMS delivery identity — product call |
| 130, 131, 116, 103 | BrowserOp::Download; deny.toml expiry 2026-09-02; setenv/getenv; 25-c1 backlog | smaller |

**Closed today by measurement:** #135 (Hermes provenance: 6 KEEP, 2 STRIP, 1 undecided), #136 (mock sweep),
#138 (runner premise REFUTED — served 4 jobs, `busy=true` was true), #146, #147, #151, #93 (was stale).

---

## 8. SEAN'S QUEUE

| item | state |
|---|---|
| **§6 decision** | the one thing genuinely blocking a coherent posture |
| `cache_tier.rs` provenance | **recommendation: STRIP.** Its attribution's one factual claim about the predecessor is provably wrong; shared literals are Anthropic-dictated; containment 0.1000 sits inside the Rust↔Python negative-control band (0.0000–0.1250) |
| `anthropic.rs:307` | genuinely undecidable. Exact identifier match, both halves externally dictated |
| Desktop contract branch | `lane/core-contract-integration` @ `475bf309` in the **Desktop** repo. Needs Sean's rebase + PR; `origin/main` moved 10 commits past its base. Sean's 16 in-flight files verified never touched |
| WhatsApp/SMS delivery identity | product call. **SMS is structurally unidentifiable** (no metadata channel to a handset). **WhatsApp genuinely can** carry a client-assigned ID |
| Rotate tokens | Slack + Discord + Matrix all went through chat today |

---

## 9. HOSTS, CREDENTIALS, HARD RULES

- **Builds ONLY on `ssh hetzner-dsm`.** NEVER cargo on the Mac; `cargo fmt` on the Mac IS fine.
  **Use debug builds unless release is required** — four concurrent release LTO builds hit load 12.3
  and cost an hour today. Orchestrator owns `/root/wayland`; lanes use their own `/root/` worktrees.
- **Windows: `ssh SeanD@seandesktop`, PowerShell not cmd. Work under `D:\` only.
  NEVER touch `C:\actions-runner-*`** — three live services. (C: has 647 GB free, not the 167 GB
  previously recorded.)
- **Credentials** in `~/.wayland-secrets/*.env` mode 600: discord, slack, matrix, flux, twilio.
  To hetzner on **ssh stdin ONLY** — never argv, never disk, never a log.
- **Slack is a LIVE COMPANY WORKSPACE** — private channel `C0BLR1UKKU6` only.
  **Matrix is Sean's PERSONAL account** — room `!kntRqkQCkPjhPvMMvf:matrix.org` only, never
  join/leave/invite, redact test events. Discord test channel `1532226655102173318`.
- **Peer trees** `/Users/seandonahoe/dev/resources/{hermes-agent,openclaw,grok-build,gemini-cli}` —
  READ-ONLY, never mutate, never execute.
- **Reserved to Sean**: merging to main, PRs, tags, releases, closing GitHub issues.
- **Never `git rebase` or `git reset --hard`** — lanes share the object store. `git checkout -- <path>` is fine.
- Lanes may not run `wcore-contract generate` — orchestrator only, last action before a tag.
- `gh auth switch --user FerroxLabs` before every `gh` op. Issues on `FerroxLabs/wayland`, code on
  `FerroxLabs/wayland-core`.

---

## 10. STANDING LESSONS — every one earned by a false green today

- **A mock proves what we send and nothing about what the destination does.** Three false
  `exactly-once` claims, `channel probe`'s unmeasured negative, and 11 of 12 tool formatters testing
  their own invented payload shape. Same disease, three surfaces, one day.
- **Absence claims need a control.** Five orchestrator briefs were measured FALSE today; every one was
  an unverified absence or an unchecked premise. The lanes that caught them did the most valuable work.
- **A skip is not a pass.** A causation run reported `PRE_RC=0 POST_RC=0` and looked like proof — the
  filter matched nothing and it measured `0 passed; 0 failed; 12 filtered out`. Read the counts, not the rc.
- **Both directions, always.** A gate that cannot pass is as worthless as one that cannot fail. Found
  three times today: the Windows journey gate, a credentials-absent TUI quadrant, and Desktop's
  `test:contract` passing via `--passWithNoTests` against a directory its config did not include.
- **Grade off code and executed tests, never a `SUMMARY.md`.** One lane's notes claimed a quadrant
  passed while its own committed artifact said `MISMATCH` — it patched the harness and never re-ran.
- **Every lane finds instrument defects in its own harness — budget for it, it is the work.** Roughly
  fifty today. The macOS harness would have reported all three TUI fixes BROKEN because macOS `awk`
  does not interpret `\xNN`; a `pkill -f` on an env var matched nothing so five gateways piled onto one
  health file and invalidated four arms.
- **`rtk` FABRICATES machine-readable counts** — it reported 0 lines for a 15-line file. Redirect to a
  file, read with the Read tool. It also rewrites `find`, `sed` and `cat`.
- **Never backticks in a shell-quoted commit message** — write to a file, `git commit -F`.
- **Assert the SHA after every checkout.** A checkout that aborts on a dirty file reports the OLD SHA
  and looks successful.
- **Tracking documents are stale in the product's favour — but it cuts both ways.** `#93` and `#138`
  both moved UP when re-measured.
