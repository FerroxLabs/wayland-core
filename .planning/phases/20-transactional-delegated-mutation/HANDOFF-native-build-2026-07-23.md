# HANDOFF — Phase 20 native-repair build (overnight, autonomous) — 2026-07-23

> Fresh session: read this top to bottom, then resume at §8. You are driving the Phase-20
> native-path closure autonomously on Hetzner. Sean is PRE-AUTHORIZED for the two gates
> (20-25 native dispatch, 20-28 terminal) — you do NOT stop for permission, only for a
> genuinely offline runner or a real failure. "Hammer through the night." If unsure on
> anything substantive, spin a cross-audit agent rather than guess (Sean's explicit rule).

---

## 0. TL;DR — immediate next action
- A `ferrox-executor` (opus) was executing **plan 20-19** when this handoff was written. It
  already committed the fixes (`0e8e6c1`) and reset the tainted tree; it was finishing the
  Hetzner Linux proof + `20-19-SUMMARY.md`.
- **First thing:** check whether 20-19 finished — does `20-19-SUMMARY.md` exist and are its
  Hetzner gates green? (`git -C /Users/seandonahoe/dev/waylandcore-ferrox log --oneline -3`;
  `ls .../20-19-SUMMARY.md`). If yes → dispatch **20-20**. If the executor died mid-proof,
  resume 20-19's proof step via `hbuild.sh` (§2), then write the summary.
- Then drive 20-20 → 20-28 in order (§3), each as its own agent, proving on Hetzner (§2).

## 1. Where we are
- **Repo / branch:** `/Users/seandonahoe/dev/waylandcore-ferrox`, branch
  `plan/f20-unified-audit-repair`. NEVER edit the dirty primary checkout
  `/Users/seandonahoe/dev/waylandcore`.
- **Candidate lineage:** accepted 20-08 successor `6937ef6` (tree `6db6fc85`) → working
  commit `be84bd2` → this session's docs commits → 20-19 fix `0e8e6c1` (HEAD at handoff).
- **Phase 20 = 16/18 accepted + native path RED/incomplete.** The transactional
  delegated-mutation lifecycle (20-01…20-16) is built, Linux-green, reviewed CLEAN. What
  remains is the native path, now planned as additive plans 20-19…20-28.
- **Use /usr/bin/git** (the RTK hook mangles refs). No Co-Authored-By in commits (match history).

## 2. THE BUILD HARNESS (critical — Rust NEVER builds on the Mac)
- **Helper:** `/private/tmp/claude-501/-Users-seandonahoe-dev-waylandcore/11929102-d58a-47e9-9644-0e9d530b58c4/scratchpad/hbuild.sh "<remote cmd>"`
  - It `git bundle`s the current branch tip → scp to `hetzner-dsm` → fetch into the reusable
    build clone `/root/wayland-f20-native` (checks out FETCH_HEAD, preserves cargo cache) →
    runs `<remote cmd>` there. **Commit before proving** (it syncs committed state only).
  - Example: `hbuild.sh "vx cargo clippy -p wcore-sandbox --all-targets 2>&1 | tail -40"`,
    `hbuild.sh "vx cargo nextest run --profile ci 2>&1 | tail -30"`.
  - If the scratchpad path is gone in the fresh session, recreate the helper from this spec
    (bundle → scp hetzner-dsm:/root/f20-native.bundle → in /root/wayland-f20-native:
    `git fetch -q /root/f20-native.bundle plan/f20-unified-audit-repair && git checkout -q -f FETCH_HEAD`).
- **Hetzner:** `ssh hetzner-dsm` (host in ~/.ssh/config, 95.216.244.213). Toolchain `vx 0.8.36`
  → cargo/rustc 1.97.0, 96 cores, 300G free. Build clone already seeded with full lineage.
- **Mac is OK for:** `cargo fmt` / `vx rustfmt`, node tooling, git, editing. NOT cargo build/clippy/test.
- **Windows-only code (`#[cfg(windows)]`) cannot compile on Hetzner (Linux).** Do NOT claim
  Windows proof from Hetzner. Windows correctness is proven only at **20-25** on the msvc runner.

## 3. THE PLAN SET (20-19 → 20-28, strictly serial, all committed at `3e48ad6`+`0394963`)
| Plan | Does | Autonomous? | Requirements |
|------|------|-------------|--------------|
| 20-19 | Hard reset to pristine `be84bd2` (Task1) + real fixes fresh: storage `.write(true)`, drop deny-only SIDs (isolation preserved), snapshot.rs msvc import | yes | r15,r1,r2,r3 |
| 20-20 | Windows test-bug fixes (choice.exe, dispatch_smoke rename) + falsifiable AppContainer isolation proofs (genuine DENY blocks; normal-SID-only denied) | yes | r2,r5,r6 |
| 20-21 | Author REAL Windows Job-Object containment tests (none exist) + repoint 2 mis-wired harness targets + wrong-OS anti-drift guard | yes | r7,r8 |
| 20-22 | macOS harness re-validation + self-hosted msvc `runs-on` + UAT-proof writer gap + native-inclusive review profile (`f20-native-16`) | yes | r9,r11,r13 |
| 20-23 | Seal ONE candidate SHA: regenerate `Cargo.lock` + Hetzner build + aggregate `nextest --profile ci` 11509/0 | yes | r10,r4 |
| 20-24 | Independent pre-native cross-audit (schema-validated artifact PER reviewer — non-author) | yes | r12,r13 |
| **20-25** | **Native-proof dispatch**: Windows on SeanDesktop (Tailscale, self-hosted msvc) + macOS on Scaleway ephemeral; re-proves BOTH the 07-23 AppContainer findings AND the actual 07-22 compile/test-mapping failures on hardware | **NO — Sean gate #1 (PRE-AUTHORIZED)** | r14,r4,r7,r9,r11,r3 |
| 20-26 | Fresh independent review, native NOT deferred (schema-validated per reviewer) | yes | r13,r12 |
| 20-27 | Re-prep: durable pending terminal tuple (idempotent, mode-0600, no-follow) | yes | r12 |
| **20-28** | **Terminal: exact-tuple authorize → confirm retained dual-OS native green + aggregate → COMPLETE Phase 20** | **NO — Sean gate #2 (PRE-AUTHORIZED)** | F20-01…GATE-02, r12, r4 |

- Plan-checker verdict on all 10: **PASS** (2 warnings folded into `0394963`).
- Construction (19–23) and independent review (24, 26) must be SEPARATE agents (author≠reviewer).
- 20-25/20-28 are `autonomous:false` in frontmatter but Sean PRE-AUTHORIZED both this session
  → proceed through them; do not pause for approval. Still verify preconditions (candidate
  green through 20-24 before dispatch; 20-26/27 done before terminal; runners actually online).

## 4. Infra + gates + hard constraints
- **Native runners (Sean's infra):** Windows = **SeanDesktop over Tailscale** (self-hosted
  msvc, AppContainer-capable); macOS = **Scaleway** ephemeral Apple-silicon. At 20-25, verify
  they're online/registered before dispatch; if asleep, ping Sean (that's the one thing
  pre-auth can't fix). `gh` is logged in as **FerroxLabs** (active) for the dispatch.
- **20-25 dispatch** likely pushes the candidate branch to GitHub `FerroxLabs/wayland-core`
  so the runners can pull — that push is PART of the pre-authorized native dispatch; state
  clearly what you push. Routine Hetzner builds use bundles (no push).
- **Sean-only (still, unless already pre-authorized):** main merge, release, deploy, canary,
  issue closure, deletion of a retained UAT evidence ref. 20-25 + 20-28 are pre-authorized.
- **Never** run cargo on the Mac; never claim native proof from cross-compilation; never edit
  the primary checkout; `gh auth switch --user FerroxLabs` before any gh op.

## 5. This session's commits (on `plan/f20-unified-audit-repair`)
`3436192` re-map codebase · `e1b207f` ingest 3 master-handoffs + reconcile · `738977e`
whole-program F00–F30 audit + native-provenance correction · `17feb09` native-repair
PRD+CONTEXT · `3e48ad6` plans 20-19..20-28 · `0394963` plan-checker warnings folded ·
`0e8e6c1` 20-19 sandbox fixes. **None pushed to GitHub.**

## 6. Audit findings + CORRECTED native provenance (do NOT re-overstate)
- The only recorded **20-18 ran RED 2026-07-22 vs PRE-repair `5e665ec`** (hosted
  `windows-2022` + ephemeral macOS) — failures were a wcore-sandbox **Windows COMPILE** error
  + a **macOS test-mapping** error, NOT the AppContainer read issue. **No 20-18 has run vs
  `6937ef6`.** The 07-23 AppContainer finding was an OFF-PLAN diagnostic. The repaired
  candidate's Windows+macOS native is UNPROVEN on hardware until 20-25.
- **20-08 attestation gap:** the old summaries prose-claimed 4 independent reviews with no
  schema artifact; only 1 attested review exists. Closed going forward by r13 (every review
  gate emits a schema-validated artifact per reviewer). New reqs r13/r14/r15 in REQUIREMENTS.md
  + `.planning/intel/inbox-2026-07-23/AUDIT-2026-07-23.md`.

## 7. Whole-program state (F20→F30) — audited VALID + fully mapped
- 9-agent audit verdict: 74 requirement IDs (58 F + 4 CTRL + 12 native), 0 orphans, 0 dup
  mappings, dependency graph acyclic + valid topological order, F00–F19 baseline consistent.
- After Phase 20: **admission gates before Phase 21** — CTRL-01 (Sean pins exact Hermes +
  OpenClaw versions) + D1 (Desktop-lane conformance). Then 21→30 each via the Ferrox loop
  (discuss→plan→plan-check→execute→review→verify→ship). Deferred-to-boundary (not defects):
  23A/23B requirement-ID split (`/ferrox-discuss-phase 23`); per-phase seam inventory for
  24–27 (`/ferrox-plan-phase`).

## 8. EXACT next steps for the fresh session
1. Confirm 20-19 finished (summary + green Hetzner Linux gates). If not, resume its proof via `hbuild.sh` and write `20-19-SUMMARY.md`.
2. Dispatch **20-20** (ferrox-executor, opus) — read `20-20-PLAN.md`, same harness/constraints as §2. Prove on Hetzner. Write summary.
3. Continue 20-21 → 20-22 → 20-23 (each executor → Hetzner proof → summary).
4. **20-24**: spawn a NON-author reviewer (e.g. general-purpose or a ferrox reviewer agent) for the independent pre-native cross-audit; it must emit a schema-validated artifact.
5. **20-25** (pre-authorized): verify SeanDesktop + Scaleway runners online (ping Sean if not); dispatch native proof; state the GitHub push explicitly; collect retained dual-OS evidence.
6. **20-26** (non-author review, native NOT deferred) → **20-27** (re-prep tuple) → **20-28** (pre-authorized terminal authorize → confirm green → COMPLETE Phase 20).
7. Update STATE.md/ROADMAP.md progress as plans complete. Then report Phase 20 done + surface the CTRL-01/D1 gate for Phase 21.

## 9. File pointers
- Plans/summaries: `.planning/phases/20-transactional-delegated-mutation/20-19..20-28-{PLAN,SUMMARY}.md`
- Scope/decisions: `20-CONTEXT.md` (D1–D7) · acceptance: `20-NATIVE-REPAIR-PRD.md` (R1–R15)
- Defect inventory: `.planning/inbox/native-uat-repair-BRIEF.md`
- Audit + addenda: `.planning/intel/inbox-2026-07-23/AUDIT-2026-07-23.md`
- Live state: `.planning/STATE.md` · roadmap: `.planning/ROADMAP.md` · reqs: `.planning/REQUIREMENTS.md`
- Native proof scripts: `scripts/f20-native-{windows-proof.ps1,macos-proof.sh,uat-proof.mjs}`
- Build helper: `<scratchpad>/hbuild.sh` (recreate from §2 if absent)
