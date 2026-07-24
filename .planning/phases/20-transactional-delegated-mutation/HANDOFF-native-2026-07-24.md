# HANDOFF — Phase 20 native-path closure — 2026-07-24 (comprehensive Windows-hardening)

> Fresh session: read top-to-bottom, then resume at §8. You are driving Phase-20's native-path
> closure. macOS is PROVEN green on hardware; Windows has 2 production-code root causes now
> diagnosed + cross-audited, with a comprehensive fix plan (20-43…20-49) authored and waiting to
> execute. Cross-auditor at every gate = **Kimi K3** (verified working — §4). Sean is engaged and
> wants the native path GREEN with no more one-bug-at-a-time circling.

---

## 0. TL;DR — immediate next action
Plans **20-43…20-49** are authored ON DISK (planner just finished; likely uncommitted). Next:
1. Confirm the planner (agent) finished; commit 20-43..20-49 (`/usr/bin/git add … && commit`).
2. Run **`ferrox-plan-checker`** on 20-43..20-49 (adversarial focus: the two production fixes + isolation guardrails). Fold any WARNINGs.
3. Execute **20-43** (the fix) via `ferrox-executor`. Then 20-44 re-seal → 20-45 re-audit (**Kimi K3**) → **20-46 re-dispatch (Sean fresh auth)** → 20-47 review → 20-48 prep → 20-49 terminal.

## 1. Where we are — the journey (so you don't repeat it)
Phase 20 = the transactional delegated-mutation lifecycle. 20-01…20-16 built + Linux-green + reviewed. The **native path** (Windows AppContainer + macOS) was authored WITHOUT ever running on the target OS, so every real-hardware run surfaces a new real bug. Iteration so far:
- **20-19…20-24** ✅ — repaired candidate `95c81ec6`, Linux 11509/0/48, pre-native cross-audit CLEAN.
- **20-25** native dispatch → **RED**: macOS `DirectoryAuthority::rename_into` was `#[cfg(windows)]`-only → E0599 on macOS.
- **20-29…20-31** ✅ — fixed rename_into (unix path) + Windows `safe.directory` → sealed `17412cf2` → re-audit CLEAN.
- **20-32** re-dispatch → **RED** (past setup/compile this time): Windows `choice.exe` exit-code + macOS `alpine:3.19` image absent.
- **20-36…20-37** ✅ — exit-code `exit /b 0` + macOS harness pulls alpine → sealed `daf27337`.
- **Sean: "you're going in circles — deep-dive, find everything at once, fix once."** → ran a **comprehensive `--no-fail-fast` diagnostic** (macOS local + Windows msvc) + 3 static-audit agents + a **Kimi K3 cross-audit**:
  - **macOS: 8/8 targets PASS on hardware — DONE/clean.**
  - **Windows: 5/6 targets RED + lib-test compile-fail.** Kimi collapsed to **2 production root causes** (§2), and **refuted my earlier `\\?\`-only hypothesis** — my 20-36 exit-code fix was chasing a symptom.
- **NOW:** comprehensive Windows-hardening plan set **20-43…20-49** authored (supersedes the daf27337-bound 20-37…20-42).

## 2. THE 2 WINDOWS ROOT CAUSES (from the Kimi K3 cross-audit — this is the spec)
Full artifact: **`.planning/intel/inbox-2026-07-24/KIMI-K3-CROSS-AUDIT-windows-native.md`** (committed `c5fb4260`). Read it.

**#A — `\\?\` verbatim paths from `std::fs::canonicalize`.** `crates/wcore-swarm/src/worktree_manager.rs:9,13` canonicalize → `\\?\C:\…` on Windows. The PowerShell capacity probe (`worktree_manager.rs:417-451`) passes it as a trailing `-Command` arg → PowerShell re-parses it as script → `UnexpectedToken`. **Fails windows-f20-lifecycle + windows-public-dispatch.** Fix: `dunce::simplified()` at the canonicalize site (also defuses latent git-with-verbatim-paths). Guardrail: do NOT default capacity to "unlimited" on parse error (that defeats DispatchAdmission budget). `dunce::simplified` is a no-op on unix → Linux aggregate unaffected (Linux-testable).

**#B — `cmd.exe` quote-mangling.** `quote_arg` (`crates/wcore-sandbox/src/backends/appcontainer/windows_impl/command.rs:76-111`) CRT-escapes embedded quotes to `\"` (`windows_impl/process.rs:613-618`), but `cmd /d /s /c` strips only the OUTER quotes of the RAW command line and runs the rest verbatim → `type "path"` runs as `type \"path\"` → `ERROR_INVALID_NAME`; `type` fails → `&& (… exit /b 0)` never runs → exit stays `choice`'s index. **Fails windows-appcontainer-acl + windows-retained-handle + windows-job-object.** Fix in the BACKEND (`process.rs`): when program is `cmd.exe` and the arg follows `/c`/`/k`, do NOT CRT-escape the payload — wrap outer, inner quotes verbatim. Guardrail: quoting layer only — keep argv discipline; do NOT relax `is_unc_or_device_path`, Low-IL token, Job-Object limits, or the ACL lease. `#![cfg(windows)]` → uncompilable on Mac/Hetzner → real proof is the 20-46 re-dispatch.

**Also in the fix delta:** job-object flake (system-wide `choice.exe`/`cmd.exe` counting in `hard_process_containment_windows.rs` → pollution when native targets run concurrently; scope to per-tag/PID or serialize); lib-test compile debt (`windows_impl/tests.rs` + `directory_authority_windows_tests.rs` missing `Arc`/`SandboxError`/`SandboxManifest`/`SandboxCommand`/`NetworkPolicy` imports + bad `FILE_ID_BOTH_DIR_INFORMATION` import — not a proof blocker but real). **Watch-items for the re-dispatch** (may be the next layer): git ops with de-verbatimed paths; `tagged_cmd_count` CIM visibility under NetworkService; the `bash` worker under AppContainer + no-Windows-Docker-fallback (`dispatch.rs`); worker test-exe DLL-load under Low-IL.

## 3. THE PLAN SET 20-43…20-49 (authored; commit + plan-check before executing)
| Plan | Objective | Sean gate? |
|------|-----------|-----------|
| 20-43 | Apply #A (dunce::simplified + dep) + #B (backend cmd-payload quoting) + job-object counting scope + lib-test compile-debt imports. Verify: macOS `cargo check --features live-docker --tests`; Hetzner fmt/clippy `-D warnings`/aggregate 11509/0/48. #B Windows proof deferred to 20-46. | no |
| 20-44 | Re-seal ONE new candidate SHA (aggregate 11509/0/48; lock delta from dunce dep if any) | no |
| 20-45 | Non-author pre-native cross-audit — **run Kimi K3** as the independent reviewer + capture the `f20-native-crossaudit` artifact | no |
| **20-46** | **Native RE-DISPATCH** — Windows msvc + fresh ephemeral macOS runner; must exercise ALL 6 Windows targets + the watch-items | **YES — Sean fresh digest auth (all prior void)** |
| 20-47 | Fresh review, native NOT deferred (Kimi K3) | no |
| 20-48 | Re-prep terminal tuple | no |
| **20-49** | Terminal exact-tuple authorize → confirm dual-OS green + aggregate → COMPLETE Phase 20 | **YES — Sean** |

Supersedes (do NOT re-run): retained-RED 20-25/20-32, and the superseded-unexecuted 20-33/34/35/37-42.

## 4. KIMI K3 — the cross-auditor at EVERY gate (VERIFIED WORKING)
See memory `[[cross-auditor-kimi-k3]]`. Invocation (tested, returns answers):
```
/Users/seandonahoe/.kimi-code/bin/kimi -p "<audit prompt>" --output-format text
```
- Binary `/Users/seandonahoe/.kimi-code/bin/kimi` v0.29.0, **subscription-authed, works.** `default_model = kimi-code/k3` (K3 is default; `-m kimi-code/k3` to be explicit). `--output-format text` prefixes bullet lines (`• …`) — parse accordingly. `-w`/cwd sets the repo it reads — run it from the ferrox clone.
- **PATH gotcha:** the Bash-tool shell is spawned BEFORE `.zshrc` adds `~/.kimi-code/bin` → bare `kimi` is "command not found". **Always use the absolute path.**
- Do NOT use `~/.local/bin/kimi-cli` (dead: Moonshot 429 / kimi-code 401). That earlier "K3 blocked on billing" was the WRONG binary.
- Pattern that worked: write the prompt to a file, `kimi -p "$(cat prompt)" --output-format text > out.md`, run in background (it can take minutes on a deep audit).

## 5. BUILD HARNESS + RUNNERS
- **Rust Linux/aggregate proof ONLY on Hetzner.** `scratchpad/hbuild.sh "<cmd>"` (git-bundle → `hetzner-dsm:/root/wayland-f20-native` → cargo). Plans also reference `/Users/seandonahoe/.ratchet/harness/remote-cargo.sh` (exists). Commit before proving.
- **The ONE sanctioned Mac cargo (D5 carve-out):** `cargo check -p wcore-sandbox --features live-docker --tests` (compile-only). cargo at `~/.cargo/bin/cargo`.
- **macOS acceptance runner = THIS Mac (local), NOT Scaleway.** Scaleway was a prior-session doc error (Sean never used it; also out of stock). Docker Desktop is live (default socket, no colima). Register the ephemeral runner via `scratchpad/register-f20-macos-runner.sh` (run with DEFAULT PATH so it finds `gh` at `/opt/homebrew/bin`; derives image label `f20-image-2e61537b…`), then `~/actions-runner-f20/run.sh` in bg (PATH must include `~/.cargo/bin:/usr/local/bin:/usr/bin`), it self-deregisters after one job. Pre-pull `docker pull alpine:3.19` before the macOS run.
- **Windows runners:** self-hosted msvc `ferrox-win-msvc` + `SEANDESKTOP` (online, persistent). `gh auth switch --user FerroxLabs` before every gh op.
- **Dispatch mechanic (learned):** `workflow_dispatch --ref refs/f20-native-uat/<sha>` returns 422 — create a BRANCH `f20-native-uat-<fullsha>` at the sealed SHA and dispatch on it (so `github.sha`==sealed SHA). Push the UAT ref too (retained, deletion not authorized). The proof harness `scripts/f20-native-uat-proof.mjs` has ONLY a `request` CLI subcommand (`verify-publication`/`verify-evidence` are plan fiction — compose verification from gh logs + the mjs exported primitives).

## 6. SEAN GATES + constraints
- **Sean-gated (fresh digest auth each time; ALL prior auths — 95c81ec6, 17412cf2, daf27337 — VOID):** 20-46 native re-dispatch, 20-49 terminal. Present the digest + runner status; get explicit go.
- Never cargo on the Mac except the D5 carve-out; never claim native proof from cross-compilation; `/usr/bin/git` (RTK mangles refs); no `Co-Authored-By`; work in `/Users/seandonahoe/dev/waylandcore-ferrox` (NEVER the dirty primary `/Users/seandonahoe/dev/waylandcore`); no branch push except the authorized UAT ref/dispatch branch; honesty gates (RED → stop + report, never fake green).

## 7. Candidate lineage + commits
`95c81ec6`(20-23 seal) → `17412cf2`(20-30 seal, +rename/safe.dir) → `daf27337`(20-37 seal, +exit-code/alpine — now known comprehensively-RED on Windows). Next seal = 20-44 (new SHA on top of the 20-43 fix). Branch HEAD at handoff: `c5fb4260` (Kimi cross-audit). Nothing pushed to GitHub except UAT refs/dispatch/diag branches.

## 8. EXACT next steps
1. Confirm planner finished; `git add` + commit 20-43..20-49 (`docs(phase20): comprehensive Windows-hardening plans 20-43..20-49`).
2. `ferrox-plan-checker` on 20-43..20-49 (verify #A/#B fixes + isolation guardrails + the dunce Linux-no-op claim + serial chain). Fold WARNINGs.
3. `ferrox-executor` 20-43 (production fixes; Hetzner aggregate 11509/0/48 + macOS `cargo check --tests`; #B deferred to 20-46).
4. 20-44 re-seal → 20-45 re-audit (**Kimi K3** + ferrox non-author) → **20-46 re-dispatch (Sean gate: re-register macOS runner, push UAT ref+branch, dispatch, capture ALL 6 Windows targets + watch-items)** → 20-47 review (Kimi K3) → 20-48 prep → 20-49 terminal (Sean gate → Phase 20 COMPLETE).
5. After Phase 20: CTRL-01 (Sean pins Hermes/OpenClaw baselines) + D1 before Phase 21; then 21→30.

## 9. File pointers
- Plans/summaries: `.planning/phases/20-transactional-delegated-mutation/20-{19..49}-{PLAN,SUMMARY}.md`
- **Kimi K3 cross-audit:** `.planning/intel/inbox-2026-07-24/KIMI-K3-CROSS-AUDIT-windows-native.md`
- Diagnostic run: GitHub Actions run `30064412019` (`f20-win-diag-daf27337` branch) — Windows `--no-fail-fast` results
- Scope/decisions: `20-CONTEXT.md` (D1-D7 + D5 carve-out amendment); PRD: `20-NATIVE-REPAIR-PRD.md`
- Harness: `scratchpad/{hbuild.sh,register-f20-macos-runner.sh,bootstrap-f20-macos.sh}`
- Memory: `[[f20-native-build-state]]`, `[[cross-auditor-kimi-k3]]`
