# Constraints

Extracted from the SPEC `native-uat-repair-BRIEF.md` (precedence 0) plus
program-level invariants restated in the two DOC handoffs. These constrain the
Phase-20 native repair; they do not replace existing accepted-source invariants.

## Sandbox is a security boundary — never weaken to pass a test
- source: .planning/inbox/native-uat-repair-BRIEF.md §8
- type: nfr
- content: Every change to token construction must preserve isolation and be adversarially reviewed. Isolation of the AppContainer is intrinsic (normal SIDs like Everyone/Users are ignored for granting); dropping the redundant deny-only SIDs restores reads without weakening the boundary — but this must be adversarially re-confirmed (open question §7.5).

## Cross-platform discipline (AGENTS.md)
- source: .planning/inbox/native-uat-repair-BRIEF.md §8
- type: protocol
- content: All platform-specific behavior stays behind `wcore_config::shell` / `cfg(...)`; no scattered platform detection.

## No drift outside plan
- source: .planning/inbox/native-uat-repair-BRIEF.md §8; MASTER-HANDOFF-phase20-native-parity.md §5
- type: nfr
- content: All fixes land as reviewed, gated commits on the candidate branch. The proven 4.A/4.B fixes are throwaway spikes in a Windows worktree — re-derive them cleanly under the plan; do NOT port hacks. Reset the tainted local `waylandcore-ferrox` working tree (diagnostic edits in `windows_impl/process.rs` and `storage.rs`) to pristine `be84bd2` before building the real candidate.

## Non-goals
- source: .planning/inbox/native-uat-repair-BRIEF.md §8
- type: nfr
- content: No refactor of the AppContainer implementation beyond what R1–R11 require; no new sandbox features; no changes to already-accepted 20-01…20-16 source outside the native surface without re-review.

## Verified defect inventory (native path, evidence-backed on real Windows 11)
- source: .planning/inbox/native-uat-repair-BRIEF.md §4
- type: nfr
- content: 4.A storage.rs OpenOptions missing `.write(true)` → is_available() false everywhere (fix proven). 4.B CreateRestrictedToken deny-only SIDs break AppContainer grant path → no file readable (fix proven, isolation intrinsic). 4.C wcore-agent E0432 wrong windows-sys import module. 4.D type_and_hold asserts on choice.exe exit index (never 0). 4.E dispatch_smoke non-portable fs::rename of open-handle dir. 4.F two "Windows" proof targets wired to Linux-only Bubblewrap tests (systemic). 4.G native machinery is "aspirational" — macOS harness must be re-validated, not assumed green.

## Native acceptance criteria (Phase 20 native path "done")
- source: .planning/inbox/native-uat-repair-BRIEF.md §9
- type: nfr
- content: All Windows native proof targets green on an AppContainer-capable self-hosted runner (real reads + real containment); all macOS native targets green; Linux aggregate unchanged (11509/0); repaired candidate passes fresh 20-16 (no deferred native), 20-17 re-prep, Sean-authorized 20-18; Cargo.lock consistent; no uncommitted spikes; anti-drift guard in place.

## Infra facts (reference)
- source: .planning/inbox/native-uat-repair-BRIEF.md §8; MASTER-HANDOFF-phase20-native-parity.md §6
- type: protocol
- content: AppContainer-capable Windows 11 client with online self-hosted `wcore-core` msvc runners (`ferrox-win-msvc`, `SEANDESKTOP`; labels `self-hosted, Windows, X64, msvc`; run as NT AUTHORITY\NetworkService). macOS native leg runs on an ephemeral Scaleway Apple-silicon runner and REQUIRES `DOCKER_HOST=unix://$HOME/.colima/default/docker.sock` exported into the runner process env. Linux aggregate via Hetzner remote-cargo. `nightly-windows-soak.yml` candidate mode dispatches native jobs; workflow_dispatch requires the workflow on the default branch. NOTE: this runner-online claim conflicts with STATE.md (see INGEST-CONFLICTS.md WARNING).

## Sean-only authorization gates (program-level, restated)
- source: .planning/inbox/MASTER-HANDOFF-frontier-v2-PROGRAM.md §10
- type: protocol
- content: Never infer approval for source push, main merge, issue closure, release, deployment, canary promotion, native-proof dispatch, deletion of a retained UAT evidence ref, or the exact-tuple 20-18 authorization. Prior 20-18 authorization `cb6f06bd…` is spent/void — a RED result or any new candidate SHA both require FRESH authorization. Never run Cargo/clippy/nextest on the Mac. Never claim native proof from cross-compilation or source inspection. Never edit the dirty primary checkout `/Users/seandonahoe/dev/waylandcore`; this program's clone is `/Users/seandonahoe/dev/waylandcore-ferrox`.
