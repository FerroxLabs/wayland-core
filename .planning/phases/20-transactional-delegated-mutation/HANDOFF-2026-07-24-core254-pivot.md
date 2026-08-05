# HANDOFF — wayland-core CORE lane — 2026-07-24 — core#254 PIVOT

**Read this top to bottom before doing anything. It changes the plan.**

---

## 0. TL;DR / the one decision that matters

The Phase-20 native-Windows repair I've been grinding for ~10 plan-cycles was **patching symptoms downstream of a Windows sandbox that couldn't run the toolchain at all.** A community PR — **`FerroxLabs/wayland-core#254`** (author @frankforges) — **root-fixes the actual bugs.** 

**ACTION: Phase-20 native repair is PAUSED. Do NOT author or execute another native patch cycle. The next real work is triaging/cross-auditing core#254 for Sean's maintainer decision.** Everything else below is detail supporting that.

Sean is logging out (weekly credit cap) + we are compacting. This is a cold-start doc: assume the next session knows nothing.

---

## 1. Operating constraints (verbatim — these persist, do not relax)

- **Lane = CORE only** (`area:core`). Sean corrected me mid-session: I only handle core-lane issues. Desktop (`area:desktop-ui`, `area:desktop`), flux (`area:flux`, `needs:flux`), providers, mcp are NOT my lane — do not triage/act on them.
- **Two repos:** issues live on **`FerroxLabs/wayland`**; code/PRs live on **`FerroxLabs/wayland-core`**. `gh auth switch --user FerroxLabs` before EVERY gh op. `wl` wrapper is on PATH for coordination.
- **Isolated checkout:** all Phase-20 work is in **`/Users/seandonahoe/dev/waylandcore-ferrox`** (standalone checkout, `.git` is a directory), branch **`plan/f20-unified-audit-repair`**. NEVER edit the sibling dirty primary checkout `/Users/seandonahoe/dev/waylandcore`.
- **Git:** use `/usr/bin/git` for every git op (the RTK shell hook mangles refs otherwise). A pre-existing untouched `AGENTS.md` ijfw-memory churn floats in the worktree — never stage it; `git checkout -- AGENTS.md` non-destructively if a scope gate needs a clean tree.
- **NO cargo/clippy/nextest on the Mac** except the ONE sanctioned compile-only gate: `cargo check -p wcore-sandbox --features live-docker --tests` (D5 carve-out). All heavy build/test runs on Hetzner via `/Users/seandonahoe/.ratchet/harness/remote-cargo.sh`. `cargo fmt`/`vx rustfmt` DO work on the Mac.
- **Cross-audit = the 4-way panel, ALL FOUR MUST PASS, fail-closed** (Sean's standing rule; see memory `cross-audit-panel.md`): Codex 5.6 Sol (`codex exec -m gpt-5.6-sol --sandbox read-only --skip-git-repo-check "<prompt>"`), Gemini 3.1 Pro (`GEMINI_CLI_TRUST_WORKSPACE=true gemini -p "<prompt>" -m gemini-3.1-pro-preview -o text --approval-mode plan --skip-trust`), Kimi K3 (`/Users/seandonahoe/.kimi-code/bin/kimi -p "<prompt>" --output-format text` — ABSOLUTE path; Kimi `-p` does NOT read piped stdin, embed context in the arg), internal Claude adversarial. Any external unreachable → fail-closed BLOCK, Claude leg is never a substitute.
- **Sean gates (his explicit fresh auth required, cannot self-authorize):** native re-dispatch + terminal/merge/release/deploy. A general "keep going" is NOT native-run authorization.
- **All builds compile ONLY on Hetzner (`hetzner-dsm`), never the Mac.** Box is currently healthy (see §5).

---

## 2. THE PIVOT — core#254 (this is the headline)

**PR `FerroxLabs/wayland-core#254`** — `fix(sandbox/windows): stop the DACL cost storm, fix DLL-init/cwd bugs, add Relaxed Sandbox Mode for trusted_local` — @frankforges, **+586/-49, 7 files, OPEN, MERGEABLE, no CI run yet, no review.** Full detail in memory `core-254-windows-sandbox-fix.md`. Fixes issue #922 (3 prior reports over 3 months: #520/#618/#743 + his #921); forwarded intel links it to a ~13-issue cluster (#921/#892/#912/#918/#908/#756/#552/#711/#737/#744/#453/#324/#267).

**The 5 changes:**
1. `wcore-tools/workspace_policy.rs` — DACL cost storm: drop `$HOME` from AppContainer read allowlist on Windows + grant a small `%TEMP%` subdir not the whole thing. **35s timeout → ~20ms.** Non-Windows unchanged.
2. `wcore-sandbox/backends/appcontainer.rs` — `CreateRestrictedToken` disables only `Administrators`, not `Users`/`Authenticated Users` (disabling them → System32 DLLs fail image init → `STATUS_DLL_NOT_FOUND`). **[SECURITY-ADJACENT — claim "boundary unaffected" must be independently verified.]**
3. `wcore-sandbox/backends/appcontainer.rs` — strip `\\?\` verbatim prefix from cwd before `CreateProcessAsUserW` (cmd.exe treats `\\`-prefixed cwd as UNC → C:\Windows fallback → every child spawn broken).
4. **NEW: Relaxed Sandbox Mode** (`sandbox/lib.rs`, `config/config.rs`, `agent/bootstrap.rs`) — `[tools] windows_relaxed_sandbox`/`windows_allow_admin`, opt-in, `trusted_local` ONLY. Medium integrity, NO AppContainer, restricted token; containment = Job Object + no-admin + network policy, **NOT filesystem confinement.** `contained`/remote sessions untouched (full AppContainer stays default). New live test `live_relaxed_windows.rs`. **[THE MAINTAINER DECISION — Sean + core team. His words via the PR: "a maintainer decision rather than something to quietly merge."]**
5. `wcore-tools/bash.rs` — Windows `BashTool::description()` warns the LLM.

**WHY IT'S MY LANE'S HEADLINE — it root-fixes what Phase-20 native was patching downstream:**
- #254 change **#3 (`\\?\` cwd strip)** = my Phase-20 **root cause #A**, but fixed at the *real child-spawn site* (`CreateProcessAsUserW`), far more fundamental than my `dunce::simplified` patch on the swarm capacity probe.
- #254 change **#2 (restricted-token DLL-init)** = the **"worker test-exe DLL-load under Low-IL" watch-item I had DEFERRED to the native run** (STATUS_DLL_NOT_FOUND). frankforges root-caused + fixed it.
- #254 change **#1 (DACL cost storm)** = a broad class of Windows spawn hangs/timeouts.
- **Therefore:** the containment-test observer whack-a-mole (Phase-20 blocks 20-45 → 20-52 → 20-59, all the SAME test's fail-open observer at deeper and deeper layers) was polishing a proof-test sitting on a sandbox that structurally could not run git/npm/powershell. That's the circle Sean kept flagging.

**DISPOSITION (NOT a quick merge — this is task #31):**
1. Do NOT merge. Sean's maintainer decision on the Relaxed Mode posture.
2. Run the **4-way cross-audit** on just the two security-sensitive changes: **#2** (does re-enabling Users/Authenticated Users in the restricted token widen the boundary?) + **#4** (is Job-Object + no-admin + network-policy acceptable containment for `trusted_local`, and is the `contained`/remote default truly untouched?). Changes #1/#3/#5 are lower-risk bug fixes but audit them too for correctness.
3. **CI has not run** — trigger it (the PR shows `CI checks: none`).
4. **Reconcile with the Phase-20 native branch** — #254 supersedes my #A patch and my Low-IL watch-item; figure out what of the Phase-20 native work (if any) is still needed on top of #254 vs. thrown away.
5. Bring Sean an audited recommendation. Do NOT merge autonomously.

---

## 3. Phase-20 native chain — EXACT state (PAUSED, do not resume without a Sean ruling)

Branch `plan/f20-unified-audit-repair`, tip **`fe7dd8ea`**, working tree clean.

**What's the running story (why so many plan numbers — it is NOT ~20 fuck-ups):** each repair cycle = 3 plans (fix → re-seal → re-audit) + a native/review/prep/terminal tail, numbered upward from 20-43. **Only THREE actual audit BLOCKS occurred, all the same defect class** (the ONE containment test `hard_process_containment_windows.rs` could report "processes reaped" without evidence — it has 3 layers, took 3 passes to close each):
- **20-45 BLOCK** — reap assertion vacuous (tagged-tree count went empty once the parent cmd died).
- **20-52 BLOCK** — the fix's *Rust* observer read a failed query as `0` (`unwrap_or(0)` no `status.success()`).
- **20-59 BLOCK** — the same observer's *PowerShell* layer read a failed CIM query as `"0"` (no `-ErrorAction Stop`).
- **My own fault:** twice I called a fix "comprehensive" when it only covered one layer. The production sandbox code (#A/#B) passed EVERY audit — zero blocks there. All three blocks were test-observer integrity.

**Sealed candidate lineage (each is `source_sha`):**
- `3f839309` (20-44 seal) → BLOCKED by 20-45.
- `f0dd5b6d` (20-51 seal) → BLOCKED by 20-52.
- `8a1d2d84` (20-58 seal) → BLOCKED by 20-59.
- **`fe7dd8ea` = 20-60's fix commit (PowerShell-layer fail-closed), LANDED but NOT sealed/summarized/audited.** The executor committed the task work, then I STOPPED it while it waited on the Hetzner gates (Sean redirected to core#254). So: 20-60 fix is on the branch, but **there is no 20-60-SUMMARY, no 20-61 re-seal, no 20-62 re-audit.** The chain is cleanly parked mid-verification.

**Plans on disk (authored, unexecuted where noted):**
- `20-60-PLAN.md` (fix — the commit `fe7dd8ea` corresponds to this), `20-61-PLAN.md` (reseal, unexecuted), `20-62-PLAN.md` (re-audit, unexecuted).
- `20-53..56-PLAN.md` — native re-dispatch (Sean-gated) / native-inclusive 4-way review / prep / terminal — REBOUND onto the 20-61 seal, all pending. 20-53 Sean gate intact (`checkpoint:human-verify` blocking-human, `autonomous:false`, void-list = 95c81ec6/17412cf2/daf27337/3f839309/f0dd5b6d/8a1d2d84).
- Sealed/executed records NOT to mutate: 20-43/44/45/50/51/52/57/58/59 (+ their SUMMARYs/CROSS-AUDITs on disk).

**If Sean says "abandon the native grind, go with #254":** the 20-57/60 observer-hardening (fail-closed reap observers) is still *correct test-quality work* and could be salvaged/rebased onto a #254-based branch — but only the containment-test hardening; the #A `dunce` probe patch and the Low-IL watch-item are superseded by #254 #3/#2. **If Sean says "still need native as belt-and-braces":** finish 20-61 reseal → 20-62 4-way re-audit of `fe7dd8ea`; if 20-62 is a clean all-four PASS, it's staged at the 20-53 Sean native gate. **My hard-stop commitment still stands:** if 20-62 blocks a 4th time, do NOT patch again — escalate to Sean for the native-Win32-enumeration rewrite of the observers (no shell, no string-parse, fail-closed by type).

**The pending 20-60 verification (if resumed):** run on Hetzner against `fe7dd8ea` — `cargo check -p wcore-sandbox --features live-docker --tests` (Mac, sanctioned), Hetzner clippy `-p wcore-sandbox --all-targets --all-features -- -D warnings`, Hetzner aggregate `nextest --workspace --profile ci --no-fail-fast` (MUST be 11509/0/48 — the 20-60 change is `#![cfg(windows)]`, Linux-unchanged). Then write 20-60-SUMMARY, then 20-61.

---

## 4. GitHub inbox — core lane, triage NOT yet done

132 open issues on FerroxLabs/wayland; **59 `area:core` (my lane), 35 bug, 5 security, 5 priority:high.** I invoked `ferrox-inbox` but pivoted to core#254 before producing the triage report. **Core-lane issues worth a look (I only skimmed):**
- **#564** (`area:core`, in-progress) — flaky auto_skill drafter test under parallel nextest, pre-existing on main; pollutes every core gate run (this is the "flaky" I kept seeing in aggregates — make it hermetic/tempdir-scoped/serial_test).
- **#372** (`bug`, `area:core`) — agent restarts its own planning loop instead of continuing (the looping behavior, reported by a user on Windows).
- **#552** (`bug`, `area:core`, in-progress) — Claude Fable 5 hangs indefinitely, bash output capture broken — **linked in the #254 cluster.**
- **#644, #657, #667, #673** (`area:core`, `security`) — path/Bash/egress hardening (P0 WorkspaceTrust-through-Bash on #657).
- **#635, #636, #637, #648, #650** (`area:core`) — context-window caps, graceful compaction, multimodal/image ingestion.
- **#661** (`area:core`) — fail-loud, stop reporting swallowed failures as empty success.
- Note #247 (ACP reap on exit) and #388 (long-task truncate) are `area:desktop-ui`/`needs:flux` — NOT my lane despite being relevant symptoms.

To do a proper triage next session: `ferrox-inbox --issues` (add `--label` to auto-label, `--to-backlog` to route approved+gated issues into `.planning/BACKLOG.md`). The workflow enforces the issue-first approval gate.

---

## 5. Infra state (all healthy, verified this session)

- **Hetzner `hetzner-dsm` disk: 37% used / ~1.1 TB free** (was 98%/42GB at session start — the disk-admission test `spawner::concurrent_near_cap_admits_exactly_one_retained_workspace` was failing environmentally). I reclaimed ~1TB in stages: 322GB `wayland-f20-native/target/debug` + ~79GB stale dated scratch dirs + **245 stale `/root/cargo-slots/*` per-task caches (~610GB).** INFRA GOTCHA saved to memory: the ferrox Hetzner harness creates one cargo-slot per task and NEVER reaps them — reap `/root/cargo-slots/*` (keep the active build slot + `repo-<hash>` shared cache) when the box nears full. DO NOT manually rm `/var/lib/containerd` (219GB — it's dockerd's LIVE backend) or `/root/rambuild` (live buildkit) or `/root/.cache/sccache` (active).
- **Native runners staged & confirmed:** both Windows self-hosted msvc runners `ferrox-win-msvc` + `SEANDESKTOP` ONLINE + idle (labels `[self-hosted,Windows,X64,msvc]`); macOS acceptance runner = THIS Mac LOCALLY (ephemeral, registers at dispatch via `scratchpad/register-f20-macos-runner.sh` with DEFAULT PATH); `docker pull alpine:3.19` DONE (macOS leg needs it). NOTE: macOS was already 8/8 PROVEN on hardware earlier — native macOS is done; only Windows native was open.
- **API instability:** ferrox-planner died twice mid-session on transient errors (ENOTFOUND / "connection closed mid-response"), each left clean state (no partial commits) and I re-dispatched. If a background agent fails, check `git log`/`git status` in the ferrox checkout before re-running — it usually committed nothing.

---

## 6. Cross-audit panel — verified invocations (memory: cross-audit-panel.md)

ALL FOUR MUST PASS, zero findings, fail-closed. Each emits its own schema-validated artifact.
1. **Codex 5.6 Sol** — `codex exec -m gpt-5.6-sol --sandbox read-only --skip-git-repo-check "<prompt>"` (reads stdin: `cat ctx | codex exec ...`). Without `--skip-git-repo-check` outside a repo it errors "not a trusted directory."
2. **Gemini 3.1 Pro** — `GEMINI_CLI_TRUST_WORKSPACE=true gemini -p "<prompt>" -m gemini-3.1-pro-preview -o text --approval-mode plan --skip-trust`. Model string MUST be `gemini-3.1-pro-preview` (bare `gemini-3.1-pro`/`gemini-3-pro` 404). Without the trust env+flag it exits 55. Strip hook-banner/bullet prefixes.
3. **Kimi K3** — `/Users/seandonahoe/.kimi-code/bin/kimi -p "<full prompt+context>" --output-format text`. ABSOLUTE path (the Bash shell spawns before .zshrc adds ~/.kimi-code/bin to PATH — do NOT conclude it's uninstalled). Kimi `-p` does NOT read piped stdin — embed context in the arg. Strip "• " prefixes. Do NOT use `~/.local/bin/kimi-cli` (dead 429/401).
4. **Internal Claude adversarial** — a non-author Claude leg, prompted to REFUTE (default-refuted-if-uncertain). The ferrox-executor context itself served as this leg in the 20-45/52/59 audits.

---

## 7. FIRST ACTIONS for the next session (in order)

1. Read memory: `core-254-windows-sandbox-fix.md`, `f20-native-build-state.md` (now flagged PAUSED), `cross-audit-panel.md`, `cross-auditor-kimi-k3.md`. Read this handoff.
2. `gh auth switch --user FerroxLabs`. Pull core#254 fresh (`gh pr view 254 -R FerroxLabs/wayland-core` + `gh pr diff 254`) — confirm still open/unmerged, check for new review/CI/comments since 2026-07-24.
3. **If Sean is present:** present the core#254 recommendation (§2 disposition) and get his call on (a) Relaxed Mode posture, (b) whether Phase-20 native is abandoned or finished as belt-and-braces. Do NOT merge.
4. **If autonomous & Sean wants the audit prepped:** run the 4-way cross-audit of core#254 changes #2 + #4 (get the diff via `gh pr diff 254 -R FerroxLabs/wayland-core`, feed each auditor per §6), write the panel result as an artifact, trigger CI. Stop before any merge — that's Sean's gate.
5. Do NOT author/execute another Phase-20 native patch cycle unless Sean explicitly says to finish it.
6. If asked to triage the core inbox: `ferrox-inbox --issues` scoped to `area:core` (§4).

Task tracker: #28 (Phase-20 native) = PAUSED; #31 (core#254 audit+decision) = the live one; #29 (open program controls, CTRL-01) still pending after.
