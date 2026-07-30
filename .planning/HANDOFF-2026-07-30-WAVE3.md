# HANDOFF — Wayland Core, 2026-07-30 (wave 3 complete, 2 lanes in flight)

Integration `plan/f20-unified-audit-repair` @ **`a3e68a31`**.
Supersedes `HANDOFF-2026-07-29-M1-WAVE2.md`.

**`.planning/CRITERIA-STATUS.md` is the at-a-glance grade file — read it before the ledger.**
**`.planning/LANE-BRIEF.md` outranks any orchestrator instruction, including this file.**

---

## 0. Do this first

1. **Two lanes were running when the session ended** (§1). Both were told to commit incrementally.
   **Measure their branches before relaunching — do not assume they died empty.** A death notice is
   an absence claim; see LANE-BRIEF §1.
2. **`lane/egress-merge-polarity` found that the fix the orchestrator recommended to Sean is
   WRONG.** Read §2 before touching egress. Sean has been told.
3. `lane/cli-danger-tiers` implements a **CLI change Sean explicitly approved** (§3).
4. Then §4, the ranked open work.

**Merge procedure, unchanged and load-bearing:** one lane at a time; re-merge independently on
hetzner at a **programmatically fetched** expected SHA (never a hand-typed one); `cargo check
--workspace --all-targets`; only then push.

---

## 1. In flight when the session ended

| lane | branch @ | state |
|---|---|---|
| `egress-merge-polarity` | `gh/lane/egress-merge-polarity` @ `da8e46b5` | 3 commits, pass 1 + pass 2 done, RED measurement in progress. One untracked evidence log in its worktree. |
| `cli-danger-tiers` | not yet pushed | worktree created at `a3e68a31`, no commits yet |

Both worktrees are under `waylandcore-frontier-worktrees/lane-<name>`.

---

## 2. Egress: the orchestrator's recommended fix is defective — DO NOT SHIP `||`

**Background.** `--i-accept-exfil-risk` was documented in four places and **does not exist**; the
claims are corrected. `[security] enabled = false` disables the boundary on its own.

**The finding (orchestrator, unreviewed at the time):** `config.rs:4431` is
`enabled: global.security.enabled && project.security.enabled`. Since `enabled = true` means the gate
is ON, `&&` lets **either** layer switch it off — including a project config, which the same function
twice calls *"untrusted (checked into a cloned repo)"* under the existing **GHSA-8r7g**. Five
neighbouring fields get tighten-only clamps; this one appears not to.

**The correction — `lane/egress-merge-polarity`, pass 2.** The orchestrator prescribed matching
`read_only`'s `global || project`. **That is defective:**

> `read_only` can use `||` because its default is **`false`** — absence is the identity element for
> `||`. `security.enabled` defaults to **`true`**, the identity for `&&`, **not** `||`. Under `||`, a
> global `enabled = false` (the operator's deliberate off switch) plus a project file that says
> *nothing* about `[security]` → serde fills project `enabled = true` → `false || true = true` →
> **the operator's off switch silently stops working as soon as any project config exists.**

A naive polarity flip trades an exfil hole for a different correctness bug. **The fix must be
presence-aware (`Option<bool>`) or operator-owned, and pinned by an executable test.**

**Strong hint at the right answer, same lane:** `tui/surfaces/config.rs:662,713,719` — the TUI's
egress toggle and its `egress_allow` editor both persist through **`patch_global_config`**, i.e. to
the GLOBAL file. **No TUI surface writes `[security]` into a project file.** The product already
treats egress as an operator control; the merge is the only thing that says otherwise.

**Still open in that lane:** the real-merge RED measurement (global ON + project `enabled = false`)
in both trust states; whether `egress_allow`'s **concatenation** lets an untrusted project *widen*
the allowlist; a sweep of other security fields for polarity.

---

## 3. CLI danger tiers — approved by Sean, in flight

Two tiers, named so the superset relationship is visible in the flag itself:

| Tier | Canonical | Aliases | Effect |
|---|---|---|---|
| 1 | `--dangerously-skip-permissions` | `--force`, `--yolo` | approvals bypassed, **OS sandbox stays ON** |
| 2 | `--dangerously-skip-permissions-and-sandbox` | `--dangerous` (deprecated) | approvals **and** sandbox, time-bounded lease |

**The hazard this lane exists to avoid:** `main.rs:1091` is
`let approval_bypass = cli.force || cli.dangerously_skip_permissions;` — so `--force` and
`--dangerously-skip-permissions` are already identical, and `--dangerous` is separately what feeds
the sandbox lease. **Aliasing `--force` into tier 2 would silently strip the OS sandbox from every
existing script and CI job** — a privilege escalation delivered by a rename, invisible in the
caller's diff. **The key deliverable is a test that fails if any spelling ever changes tier.**

Lease (15 min default / 1 h max) and the argv-only provenance rule stay on **tier 2 only** —
time-bounding approvals would break long scripted runs.

**`--auto-approve` is unresolved.** The orchestrator told Sean it was tier 1; it actually feeds a
*different field* (`main.rs:1765` `auto_approve`, not `approval_bypass`). The lane must prove it
identical before aliasing it, and **leave it alone otherwise**.

**Egress folds into tier 2 only after §2 lands.** Do not pre-empt it.

---

## 4. Still open, ranked

**Needs a host we don't have:**
- `27-C2(c)` — three policy baselines. All three have mechanism in source; the **live measurement** is
  missing, and **two of three legs need a display-capable host**. hetzner cannot host them — which
  the liveness probe now correctly refuses. Sean's Mac or SeanDesktop.
- Windows run for the reload lease fix — Windows uses a **mandatory** rather than advisory lock, so
  the `flock`-based single-owner lease deserves a real run. The lane explicitly declines to claim it.

**Real product gaps:**
- `24-C3` — media and native actions still at zero, reconnect half untouched. **The repairing lane
  declines to claim the criterion.**
- `21-C3` — tool *live* cells open; **Windows unmeasured by anyone**.
- `setenv`/`getenv` memory unsafety — **improved, explicitly NOT closed**. `#[serial]` serializes
  writers against writers, never against arbitrary readers in third-party crates. Real closure means
  removing process-global env mutation from the test path, not more serialization.

**Before any tag:**
- **Contract regeneration #4 must be the LAST action.** Any producer-type change drifts
  `fixture_digest`/`schema_digest`/`source_inputs_digest` and reddens the corpus — as it silently had
  before regeneration #3. Generate on hetzner, `rsync` `crates/wcore-protocol/contracts/` back,
  commit and push from the Mac (hetzner has no GitHub credentials).
- **Re-rank the release blockers.** `23A-C1` and `24-C5` are cleared, and the sentence that made
  `24-C2` the number-one blocker is no longer true. §3 of the ledger is stale.

**Sean's, and only his:** merge to main · tag/publish · core#254 reply · close #142.

---

## 5. What landed this session

**28 lanes merged**, each workspace-checked on hetzner at a verified SHA. Criteria now **4 MET,
11 PARTIAL, 3 NOT MET**, with both previously release-blocking criteria cleared.

Defects closed, mostly **not** proof debt:

- **A cross-user prompt leak (HIGH).** The brief rendered from a hardcoded `"default"` bucket while
  every write keyed `$WAYLAND_USER_ID`, so one user's inferred traits reached another user's system
  prompt and shipped to a third-party provider. Extracted verbatim from a captured request body.
- **Journals the product could not read back (HIGH).** The checksum hashed a *re-serialization of the
  decoded event*, not the bytes on disk, so integrity depended on serde round-tripping being a
  bijection — and the reader blesses fifteen encodings the decode drops. "Sequence 16" and the
  load-sensitivity were artefacts; reproduction is now 1/1 → 0/1 in 0.00 s. The fix repairs journals
  already on disk.
- **Torn SQLite on both the archive and rollback paths.** 4/4 and 7/8 failures at base with real
  concurrent writers; rows lost that were committed *before* the operation launched; both verbs
  exiting 0 throughout. Also found: TRUNCATE journal mode — what a network filesystem forces — was
  broken too.
- **Reload started pollers for a gateway holding no lease.** Polling is a destructive read, so the
  rightful owner saw nothing. Plus a health surface that cleared its own only failure condition on
  reload while the path stayed dead.
- **Local models billed at Anthropic's rate.** `ProviderType` had no Ollama variant, so
  `ollama_defaults()` had **zero production construction sites**. `$0.018840` → `$0.000000` live.
- **The image tool broken by default on FluxRouter**, closed through `ProviderCompat`.
- **The flake family closed at its sources** — 16/25 → 0/25, 18/25 → 0/25 at 4× load, 13/25 → 0/25.
- **A user-visible config-rewrite bug**: `HashMap` ordering rewrote the whole of the user's
  `config.toml` on every save.

---

## 6. Standing lessons earned this session — all now in LANE-BRIEF

- **§2a — hetzner's `git fetch origin` updated nothing but `main`** (single-branch refspec, 37 of 40
  trees). A checkout could be silently 21 hours stale. Refspecs widened; **the rule outlives the fix:
  assert the SHA you expected after every checkout.**
- **§3b-iii — a permanently-RED gate proves as little as a permanently-green one.** `22-C3` was
  graded off a grep whose needle pointed at the wrong directory: FAILED forever, no reachable pass
  state. **Run the control in both directions: can it fail, and can it pass.**
- **Your brief's measurements are probably stale.** Five lanes falsified orchestrator premises this
  session. Three rows were graded off *the wrong artifact* — a finding lane's summary rather than the
  repair that followed.
- **`EXIT=$?` after a pipe reports the pipe's last stage.** Use `set -o pipefail` or `${PIPESTATUS[0]}`.
- **`rtk` rewrites `git`, `grep`, `cargo`, `ls` and `git status --porcelain`** — and the `porcelain`
  rewrite fires **even when written `command git`**. Only the absolute path escapes it.
- **Never share `CARGO_TARGET_DIR` across worktrees** when a crate bakes `CARGO_MANIFEST_DIR` into a
  digest — it reddens files you never touched.

**The single most important one:** the ledger re-grade, whose whole job was catching permanently-red
instruments, **was itself defeated by one** — it graded `27-C2(b)` off a line that reads `true`
forever while the real fix sat in its own ancestry. **No audit is immune to the failure mode it is
auditing for.**

---

## 7. Housekeeping

- **Anthropic key: ROTATED by Sean.** No longer an open item.
- All hetzner lane worktrees and their `target/` dirs were removed by their lanes; ~476–534 GB free.
- The orchestrator's own scratch branches on hetzner (`orch-*`) are disposable.
- **`f23-journey-day3.timer` / `f23-journey-verify.timer` fire 2026-07-30 14:31Z and 14:45Z.** They
  are **transient** (`systemd-run`) and will **not** survive a reboot. Definitions and a restore
  procedure are captured at `/root/f23-journey-recovery/` on hetzner.
