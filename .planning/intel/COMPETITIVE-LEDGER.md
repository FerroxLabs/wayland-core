# Competitive Capability Ledger v1 — CTRL-01

This control ledger exists before broad execution. It prevents F30 from first discovering product gaps.

## Maturity states

`ABSENT → SOURCE → CONFIGURED → CONSTRUCTED → REACHED → EFFECTIVE → OPERATOR_COMPLETE → PACKAGED_PROVEN`

Every capability row must record: stable coverage ID, owner (`core`, `protocol`, `desktop`, or shared), current maturity, security authority owner, exact evidence IDs, pinned Hermes/OpenClaw comparison baseline, delta, limitation, and last refresh phase. Source presence alone never earns effectiveness or parity.

## Admission rule

- Bootstrap and retroactively map accepted F03/F05 evidence before Phase 21 begins.
- Pin exact Hermes and OpenClaw versions before Phase 21; `UNPINNED` is an explicit open state, not a baseline.
- Refresh changed rows at every admitted phase.
- Contradictory live/customer evidence reopens the row and enters `FIELD-REGRESSIONS.md`.
- F30 independently reviews the accumulated ledger; it does not author the first comparison.
- CTRL-01 remains open until every active row uses the declared maturity enum and has a pinned peer baseline, security owner, exact evidence IDs, delta, limitation, and refresh phase.

---

## Pinned peer baselines

**UNCHANGED BY THIS REFRESH.** Both peer baselines remain **PINNED** exactly as recorded on
2026-07-26. Nothing in this refresh re-pins, re-reads or re-measures a peer tree, and no peer was
added. Every pin below was read directly from a read-only local checkout on 2026-07-26; nothing
here is recalled, inferred, or fetched from the network. Each pin names the exact file and field
or the exact git command that produced it.

**Baseline token `BASE-2026-07-13`** — the frozen comparison baseline. This is the snapshot pair
that the accepted frontier evaluation program and gap audit were measured against
(`docs/design/2026-07-13-wayland-core-frontier-evaluation-program.md:7`,
`docs/design/2026-07-13-wayland-core-frontier-gap-audit-and-execution-plan.md:6`). Every delta
recorded in this ledger is bound to it.

| Peer | Repository | Baseline commit | Exact version | Version pin source | Commit date |
|---|---|---|---|---|---|
| Hermes Agent | `https://github.com/NousResearch/hermes-agent.git` | `dbe734beff0caf5e8ee2acbe4277db7f6cf84a21` | **0.17.0** | `git show dbe734be:pyproject.toml` → line 10 `version = "0.17.0"` | 2026-06-27 |
| OpenClaw | `https://github.com/openclaw/openclaw.git` | `11a0ad10e91a50d5a0e636494eea4d7ad3eaf9fc` | **2026.6.2** | `git show 11a0ad10:package.json` → line 3 `"version": "2026.6.2"` | 2026-06-16 |

Both baseline commits were verified to **resolve and be ancestors of local HEAD**
(`git cat-file -t` → `commit`; `git merge-base --is-ancestor <base> HEAD` → true) in the
checkouts at `/Users/seandonahoe/dev/resources/hermes-agent` and
`/Users/seandonahoe/dev/resources/openclaw`, both with clean working trees
(`git status --short` → empty). The recovered Hermes version `0.17.0` independently corroborates
the string already recorded at
`docs/design/2026-07-13-wayland-core-frontier-evaluation-program.md:379`. The OpenClaw version
`2026.6.2` was **not** recorded in any program document and is newly recovered here; before this
refresh OpenClaw had a commit pin but no version label anywhere in the repo.

**Declared refresh candidate `HEAD-2026-07-26`** — the newer snapshot on disk, recorded so the
next refresh has an exact forward target. It is **not** the baseline for any delta in this ledger.

| Peer | HEAD commit | Version | Version pin source | `git describe --tags` | HEAD date |
|---|---|---|---|---|---|
| Hermes Agent | `d59b79fadd1e9edd7afc5c679cc3b143838e7c01` | 0.18.2 | `pyproject.toml:10` | `v2026.7.7.2-1200-gd59b79fad` | 2026-07-17 |
| OpenClaw | `3659c85e534fdb8b8ce6b7505a83d92cc2e4df8e` | 2026.7.2 | `package.json:3` | `release-publish/ced50f88e928-20260717-311-g3659c85e53` | 2026-07-18 |

Baseline-to-HEAD drift at the time of pinning: Hermes `0.17.0 → 0.18.2`; OpenClaw
`2026.6.2 → 2026.7.2`. Deltas below are **not** re-measured against `HEAD-2026-07-26`.

**Peers not in the CTRL-01 contract.** `gemini-cli` and `grok-build` are also checked out at
`/Users/seandonahoe/dev/resources/`. CTRL-01, `REQUIREMENTS.md` CTRL-01/F30-01/F30-03, and
`ROADMAP.md:83` all declare the peer set as Hermes and OpenClaw only. No family row references
either tool, so neither was added — widening the declared peer set is a change to the control's
contract, not a refresh of it.

## Core-side commit lineage used by this refresh

Every Core SHA cited below was checked to be a linear ancestor chain, so "later than" statements
in the Limitation column are checkable rather than assumed
(`git merge-base --is-ancestor`, all → true, run 2026-07-28):

`9821ef76` (F20A seal) → `2ecdfdf5` (Phase 27 base) → `ac94b1d5` (Phase 21 re-verification) →
`32e2f57d` (Phase 28 certified candidate) → `42e1f2b2` (this refresh's base).

## Evidence ID index

| Evidence ID | Artifact |
|---|---|
| `F03-RECEIPT@1c644ccd` | `docs/design/2026-07-13-wayland-core-f03-evidence-receipt.md`; implementation source `1c644ccdee8180bd2eded312d391f486be99902d` on `frontier/m0` |
| `F05-RECEIPT@0825c92d` | `docs/design/2026-07-13-wayland-core-f05-capability-activation-receipt.md`; implementation source `0825c92d42fe1777822e2c3463f9eb581ba5cd5d` on `frontier/m0` |
| `F05-TRUTH-{n}` | Row `{n}` of the F05 startup truth table, `…-f05-capability-activation-receipt.md` §2 |
| `F20-SEAL@01a5b0ae` | Phase 20 close, SHA `01a5b0ae459c9d5088cfd7e41271a5d4ece1b9bb` (tree `4a5247ca`); `cargo nextest` 11519/11519 passed, 48 skipped. Logs: `phases/20-transactional-delegated-mutation/20-56-evidence/{build,test}-01a5b0ae-GREEN.log.gz` |
| `F20A-SEAL@9821ef76` | Phase 20A close, SHA `9821ef7603ac1e687b600cda591af1657c883484` (tree `0a1267a9`, tag `f20a-candidate-9821ef76`) |
| `RUN-30184651330` | `nightly-windows-soak` `workflow_dispatch` run `30184651330`, 2026-07-26. Windows job `89747993276` 6/6 PASS; macOS job `89747992986` 8/8 PASS; both at nonce `96c91107…`. Detail: `phases/20A-native-windows-macos-uat/20A-04-SUMMARY.md` §13 |
| `GAP-AUDIT-2026-07-13 §3` | Comparative scorecard, `docs/design/2026-07-13-wayland-core-frontier-gap-audit-and-execution-plan.md` §3, table at lines 64-90. Derived from direct inspection of the `BASE-2026-07-13` snapshots. **Static source comparison, not a runtime benchmark.** |
| `PEER-PROBE-2026-07-26` | Structural probes (`git ls-tree`, `git grep`) executed against both peer trees **at the `BASE-2026-07-13` commits** on 2026-07-26. Records presence/absence of a counterpart, never a performance claim. |

### Added by the 2026-07-28 refresh

Each ID below names one artifact a reader can open. None asserts a peer comparison; all are
Core-side execution evidence. Every path was verified present on disk at `42e1f2b2`.

| Evidence ID | Artifact |
|---|---|
| `F21-VERDICT@f2d186f6` | `phases/21-child-authority-and-budget-inheritance/21-04-PHASE-VERDICT.md` — Phase 21's own grading at base `f2d186f6`: Criterion 1 NOT MET, Criterion 2 MET-WITH-STATED-EXCEPTIONS, Criterion 3 NOT MET; six HIGH findings; all four requirements OPEN |
| `F21-CRIT3-REPAIR@359ce2bf` | `phases/21-child-authority-and-budget-inheritance/21-05-CRITERION3-REPAIR.md` — at `359ce2bf`, all fourteen decisive live Linux rows carry one or two delegated child provider turns on BOTH surfaces; tool, filesystem, secret, egress and depth REFUSED live with a real actor |
| `F21-REVERIFY@ac94b1d5` | `phases/21-child-authority-and-budget-inheritance/21-REVERIFICATION.md` — third grading, verified 2026-07-27T01:30:00Z at `ac94b1d57cc95b577f60b8b3da3be0d536a6d7ad`. SC1 upgraded NOT MET → MET-WITH-STATED-EXCEPTIONS (tool-authority intersection at all six production spawner sites + dispatch-time `PolicyGate` + source-derived seventh-site guard, live-proven with a differential control); F21-04-02 DISPROVED; F21-04-03 repaired (6/6 clean live two-sibling runs). SC3 still NOT MET. **Linux only; no Windows run produced.** Phase goal NOT ACHIEVED for the third time |
| `F05-NEG-PERSISTS@2ecdfdf5` | `phases/27-multimodal-browser-generation-voice/evidence/27-01/OBS-RAW.log` — the shipped binary's own capability-activation stream at SHA `2ecdfdf5`, host `Ubuntu-2404-noble-amd64-base`, version `wayland-core 0.12.25`. **18 observations of `{"capability":"delegate_isolation","stage":"unavailable","reason":"isolation_not_enforced"}`.** Corroborated on Windows at `phases/25-remote-reach-nodes-plugin-lifecycle/evidence/25-02-win-pos-approved.txt:107` and `25-02-win-neg-unapproved.txt:98` (2026-07-27; that artifact does not assert its own SHA) |
| `F22-VERDICT-2026-07-27` | `phases/22-supervision-durable-goals-fleet-loops/22-PHASE-VERDICT.md`, the `UPDATE — 2026-07-27` section which supersedes the original grading. C1 FAILED (one surface of three), C2 PASSED both platforms, C3 FAILED unchanged, C4 PARTIAL, C5 PARTIAL. **Phase goal STILL NOT ACHIEVED** |
| `F22-FLEET-WIRE` | `phases/22-supervision-durable-goals-fleet-loops/22-03-SUMMARY-WIRE.md` — `kill -9` on the process group of `wayland-core 0.12.25` release mid-wave, 2026-07-27T12:13:53Z: 7 group members → 0, restart exit 0, 2 claims revoked on lease expiry, 4 completions drained from the outbox the dead parent never observed, effects **12 / 12 / 12**, 14 attempts, 12 dependency releases, 0 unresolved. **The counting gate was falsified in the same run** (a duplicated effect → 13, exit 1). Windows `taskkill /T /F` at 2026-07-27T13:12:06Z reproduced the same numbers |
| `F23A-DISPOSITION` | `phases/23A-governed-skills/23A-04-SUMMARY.md` — Success Criterion 1 **NOT MET**: `promote`, `revoke`, `rollback` unimplemented, `observe` partial, and "cannot execute before governed promotion" satisfied only by the absence of any promotion path. Plans 23A-02/03/04 NOT STARTED. F23A-01-H1 fixed; F23A-01-H2 (any errored tool call kills the session) left open and committed red |
| `F23B-01-SESSION` | `phases/23B-continuous-agency/23B-01-SUMMARY.md` + `23B-01-LIVE-EVIDENCE.md` — `wayland-core session` search/inspect/fork/retry/export/retain/reconcile/cancel driven against the shipped binary on Linux; closes live Windows UAT defect D2. macOS, Windows and TUI legs explicitly open |
| `F23B-02-MEMORY` | `phases/23B-continuous-agency/23B-02-SUMMARY.md` — recall provenance plus correct/forget/privacy/retention through the unmodified `MemoryAccessGate`, reachable from `/memory`. **F23-04 (cache/compaction cost truth) NOT STARTED** |
| `F23B-03-INDEX` | `phases/23B-continuous-agency/23B-03-SUMMARY.md` + `23B-03-LIVE-EVIDENCE.md` — persistent incremental SQLite index reachable as `wayland-core index build\|status\|search\|verify`; **all three mandatory platform legs PASS on real hardware**, each nonce-bound and each against a binary whose `--build-info` source SHA was asserted equal to the commit under test before any measurement. Every mandatory F23-06 clause closed; the OPTIONAL semantic layer deferred |
| `F23B-04-CLOCK` | `phases/23B-continuous-agency/23B-04-SUMMARY.md` + `23B-04-LIVE-EVIDENCE.md` — multi-day journey **day one only**: Linux `2026-07-27T14:21:19Z`, Windows `2026-07-27T23:54:26Z`, **macOS NOT ACHIEVED, nothing run**. Journey SHA `0ed05322`. **The journey cannot close before `2026-07-30T23:54:26Z`** |
| `F23B-H1-RECOVERY` | `phases/23B-continuous-agency/23B-H1-RECOVERY-SUMMARY.md` — journals already on disk carrying an explicit `"effect_receipt":null` failed their checksum on read (silent permanent user data loss); recovered with content intact on Linux, macOS and Windows against real binaries, without loosening the integrity check |
| `F24-01-GATEWAY` | `phases/24-gateway-automation-channels-typed-api/24-01-SUMMARY.md` + `24-01-GATEWAY-CONTRACT.md` — `wcore-gateway` lifecycle state machine, single-instance lock, ordered drain, exactly-once delivery ledger; one HIGH Windows detach defect measured and fixed. **No live operator journey at this plan** |
| `F24-02-AUTOMATION` | `phases/24-gateway-automation-channels-typed-api/24-02-SUMMARY.md` + `24-02-AUTOMATION-CONTRACT.md` — single-owner schedule lease over `flock`/`LockFileEx`, seven trigger types, bounded retry and history, all seven types authored from the shipped binary. This is the persistent scheduling Phase 22 C4 deferred to Phase 24 |
| `F24-B-LIFECYCLE` | `phases/24-gateway-automation-channels-typed-api/24-B-SUMMARY.md` §5 — real `wayland-core 0.12.25` release on `hetzner-dsm` against real `systemctl --user`: `install` wrote a real unit and `is-enabled` → `enabled`; `kill -9` recorded by systemd itself (`code=killed, status=9/KILL`) and **RECOVERED after 5s with a new pid — nothing in the run restarted it**; `drain --budget-ms 5000` went `Draining (pending 12)` → `Drained (pending 0)` with all 12 abandoned BY NAME and recorded durably; `uninstall` left no residual process. Four HIGH defects found by running it. §6 records that the macOS CI binary **provably does not carry this code** (`--help \| grep -cE "^\s+gateway"` → `0`) |
| `F24-C-ARRIVAL` | `phases/24-gateway-automation-channels-typed-api/24-C-SUMMARY.md` §8 + `24-C-arrival-evidence/` — delivery ARRIVAL at an **independent sink**: 10 deliveries, 10 distinct messages at the destination, none twice; the one delivery whose outcome was UNKNOWN across a `kill -9` and a platform restart produced **exactly one** message. **Before the fix that same delivery produced two.** Linux only; a full 12-of-12 clean tally is still open (F24-C-M1) |
| `F24-04-TYPEDCLIENT` | `phases/24-gateway-automation-channels-typed-api/24-04-SUMMARY.md` (lane 24e) — a client that severed its connection **having received ZERO bytes** recovered, twelve seconds later, an event the server produced entirely after it had gone; over HTTP/SSE on Linux through the shipped binary. `wcore-acp` 148 passed; 11 mutations each reddened their named test. **24-04's own four tasks (journey driver, Windows interface decision, three platform journeys, acceptance panel) NOT STARTED; the phase is NOT closed** |
| `F25-01-BACKENDS` | `phases/25-remote-reach-nodes-plugin-lifecycle/25-01-SUMMARY.md` — `wcore-exec-backend` provider-neutral contract; four reference backends behind one conformance harness; **three of them ran the same deterministic task through the shipped binary on `hetzner-dsm` and diffed to EQUIVALENT**. Success Criterion 1 **NOT MET** — the hibernating cloud leg is unexercised for want of a credential only Sean can mint |
| `F25-02-PLUGINS` | `phases/25-remote-reach-nodes-plugin-lifecycle/25-02-SUMMARY.md` + `25-02-LIFECYCLE-TRANSCRIPT.md` — all twelve `wayland-core plugin` verbs plus all four negative cases driven through the shipped **release** binary on Linux **and** Windows; approval is a load-time gate the engine enforces, bound to the plugin directory's SHA-256. **Success Criterion 3 MET on Linux, MET-with-one-recorded-divergence on Windows** — the only MET Success Criterion produced by phases 21-29 |
| `F25-03-NODES` | `phases/25-remote-reach-nodes-plugin-lifecycle/25-03-SUMMARY.md` + `25-03-NODE-EVIDENCE.md` — node identity attested INSIDE the signed receipt body; all six F25-03 properties driven through the shipped binary with attribution re-verified after all five disruptions. **Success Criterion 2 NOT MET** — no second physical host; no SSH trust relationship exists between `hetzner-dsm` and `SeanD@seandesktop` and creating one is reserved to Sean |
| `F25-04-FAILCLOSED` | `phases/25-remote-reach-nodes-plugin-lifecycle/25-04-SUMMARY.md` + `25-04-FAIL-CLOSED-EVIDENCE.md` — all five hostile compromise cases fail closed on **both** hosts, each compromise induced for real. **Success Criterion 4 NOT MET** — SSH and cloud cannot be enumerated on the proof hosts, so their orphan counts are `NOT MEASURED`. Three HIGH found, fixed and re-proved |
| `F26-01-DISCOVERY` | `phases/26-migration-export-backup-restore/26-01-SUMMARY.md` + `26-01-BASELINE.md` — Hermes AND OpenClaw discovery typed, deterministic, structurally secret-redacted and non-mutating; proved on Linux against canary corpora **and on macOS against Sean's REAL peer installs with live credentials** (7 real secret values extracted, **0 hits** in either emitted document, both homes unmutated). `migrate --help` lists both peers on the real binary. F26-01 claimed |
| `F26-03-BACKUP` | `phases/26-migration-export-backup-restore/26-03-SUMMARY.md` (completed in lane 26c) — backup, restore and exact rollback; **F26-03-D fixed at the product**: `wcore_config::atomic_io`'s tempfile round trip reached Win32 without long-path handling and failed with os error 3 at a 320-character non-verbatim absolute path on Windows 11 26200. Windows interruption legs run. F26-03 and F26-04 claimed. **Plans 26-02 (import/apply + quarantine) and 26-04 (hostile corpora) NOT STARTED** |
| `F27-VERDICT` | `phases/27-multimodal-browser-generation-voice/27-PHASE-VERDICT.md` — **GOAL NOT ACHIEVED.** C1 PARTIAL, C2/C3/C4/C5 NOT MET, 0/5 requirements complete. `browser_suite` and `computer_use` still report `true` on a box with no browser binary and no display, measured invariant across five single-variable observations with the next operation failing `spawn camoufox: No such file or directory`. Zero packaged smokes on zero platforms |
| `F28-CANDIDATE@32e2f57d` | `phases/28-native-cross-platform-certification/28-01-CANDIDATE-LEDGER.md` + `28-02-SUMMARY.md` §1 — candidate `32e2f57d09fe4b287e513081862217dc9daa5901`, tree `63ec0e6c…`, **6 of 6 per-target CI release artifacts digest-bound** (linux `e8431ba2…`, macOS `945534d6…`, windows `baf9bd69…`). Every family ran **the CI release artifact itself**, sha256-asserted on the host before the run. **This certification covers `32e2f57d` and NOT the current tip** |
| `F28-MATRIX-651` | `phases/28-native-cross-platform-certification/28-02-MATRIX-RESULTS.md` (rendered from `results.json`) — **651 of 651 cells with an outcome, 0 skipped; 627 pass / 24 red; 147 of 147 unskippable critical cells ran.** linux 216-0-0, macos 192-**24**-0, windows 219-0-0. All 50 sandbox greens carry a differential activeness observation; absence of a violation is not expressible as a green. 32 mutations each rejected with the control re-run green after every one |
| `F28-CONTROL-WEDGE` | `phases/28-native-cross-platform-certification/28-02-OBSERVABILITY-CONTROL.md` — verdict **`wedge-clearable`**: session type made no difference, lease state made all the difference; session-0 non-interactive SSH reported the sandbox **available** and ran a contained worker. `observation-blocked` therefore NOT AUTHORISED for any cell. In both wedged observations the product **REFUSED TO EXECUTE** — `KR-05`'s "continues UNSANDBOXED" half DISPROVED for the delegated-execution surface, its "reads like a platform limitation" half CONFIRMED. **Does not generalise: `seandesktop` is one physical box** |
| `F-28-02-001-FIXED` | `phases/28-native-cross-platform-certification/F-28-02-001-DISPOSITION.md` + `F-28-02-001-MACOS-PROOF.md` — the 24 macOS reds were an **observability** gap, not a containment gap; macOS containment is real (`SandboxExecBackend`, a `(deny default)` SBPL profile) and was simply unobservable through any black-box surface. Disposition FIXED, with `swarm`'s macOS refusal deliberately untouched |
| `F29-CENSUS` | `phases/29-supply-chain-release-integrity/29-01-SUMMARY.md` — 30 census rows, **nine HIGH, no CRITICAL**. Recorded as prominently as the gaps: the release path **already** carries keyless Sigstore SLSA provenance (`actions/attest-build-provenance@v4`), npm `--provenance`, and `self_update.rs` fails **closed** without `gh attestation verify`. `grep -rn 'environment:' .github/workflows/` returns **zero** — no job declares a GitHub Environment. F29-CEN-17: rollback rehearsal does not exist anywhere in CI |
| `F29-SBOM@5028fe28` | `phases/29-supply-chain-release-integrity/29-02-CLEANROOM-RESULTS.md` — byte-deterministic CycloneDX SBOM, **865 components, 447,364 bytes, sha256 `5028fe28…`**, cross-path byte-identical from differing `cargo metadata` inputs (1,538 checkout-path occurrences each). A real defect was found in getting there: the serial was derived from the raw text and moved at byte 83 between checkout paths |
| `F29-DENY-FAIL` | `phases/29-supply-chain-release-integrity/29-02-SUMMARY.md` §The measurements — **first execution of `cargo deny check` in the repository's history**, `cargo-deny 0.20.2`, 1,017 crates: `advisories FAILED, bans ok, licenses FAILED`, `F29-DENY-STATUS::FAIL::exit=5`. `deny.toml` unweakened by one character; deliberately NOT chained into `check-all` because the verdict is red |
| `F29-REPRO-VARIANCE` | `phases/29-supply-chain-release-integrity/29-02-CLEANROOM-RESULTS.md` — `F29-REPRO::DOCUMENTED-VARIANCE::a=ca35c34f…::b=8272fae8…`, class **`path_prefix`**, mechanism located to seven `OUT_DIR` paths from `cranelift-codegen`'s build script reaching the binary through `file!()`. **The shipped release is reproducible only accidentally**, because GitHub runners always check out at the same absolute path |
| `F29-02-H1` | `phases/29-supply-chain-release-integrity/29-02-SUMMARY.md` §4 + `.planning/SEAM-REQUESTS/29.md` SR-29-6 — **OPEN HIGH.** `.cargo/audit.toml` silences `RUSTSEC-2026-0194`/`0195` on a stated "sole path"; the real graph has **three** consumer paths and the two through `wcore-tools` — which reads **user-supplied** docx/pptx/xlsx — are absent from the justification. calamine 0.26.1 has 25 `.attributes()` sites and zero `with_checks(false)`, so **0194 is reachable**; 0195 is not. Panel 2-1 HIGH (codex HIGH, kimi HIGH, gemini MEDIUM) |
| `CTRL01-PANEL-2026-07-28` | `.planning/intel/evidence/ctrl-01-refresh-2026-07-28/` — the four-way maturity panel run for this refresh: `panel-prompt.txt` (the identical prompt sent to all three externals), `panel-codex.txt`, `panel-gemini.txt`, `panel-kimi.txt`. Splits recorded verbatim in "Maturity panel" below |

---

## Initial coverage families

Delta column convention: the verdict is from `GAP-AUDIT-2026-07-13 §3` where one exists, followed
by the `PEER-PROBE-2026-07-26` structural counterpart observed at the pinned baseline. Both are
static-source statements. No cell below asserts a measured runtime, cost, or success-rate number —
F30-03 owns that and has not run.

**The Delta column was NOT re-derived by the 2026-07-28 refresh.** Every verdict and probe finding
in it is reproduced unchanged from the 2026-07-26 pinning. Where a phase since executed has made a
Delta cell's statement about **Core** look stale, that is flagged as an open question for F30
inside the **Limitation** column and is deliberately **not** resolved here — F30 owns the first
comparison, and re-deriving a delta would require re-reading the peer trees, which this refresh
did not do.

| Coverage IDs | Family | Owner | Security authority owner | Maturity | Evidence IDs | Hermes/OpenClaw baseline | Delta | Limitation | Last refresh | Next proof |
|---|---|---|---|---|---|---|---|---|---|---|
| AUTH-* | posture, approval, policy, sandbox, secrets, egress | core | core | CONSTRUCTED *(unchanged)* | `F03-RECEIPT@1c644ccd`; `F05-RECEIPT@0825c92d`; `F05-TRUTH-6`; `F21-VERDICT@f2d186f6`; `F21-CRIT3-REPAIR@359ce2bf`; `F21-REVERIFY@ac94b1d5`; `F05-NEG-PERSISTS@2ecdfdf5`; `F25-04-FAILCLOSED`; `F28-CONTROL-WEDGE`; `CTRL01-PANEL-2026-07-28` | `BASE-2026-07-13` — Hermes 0.17.0 @ `dbe734be`; OpenClaw 2026.6.2 @ `11a0ad10` | Sandbox/egress: **Core architectural lead, operationally unproven**; permission UX: **mixed/behind in product semantics** (`GAP-AUDIT §3`). Probe: OpenClaw ships `packages/net-policy`, `src/security`, `src/secrets`, a documented sandbox surface (`docs/gateway/sandboxing.md`, `docs/gateway/sandbox-vs-tool-policy-vs-elevated.md`) and a sandbox CI smoke (`.github/workflows/sandbox-common-smoke.yml`); Hermes has **no** dedicated sandbox module at baseline — only `tests/agent/test_file_safety_sandbox_mirror.py` and `tests/tools/test_modal_sandbox_fixes.py`, i.e. isolation is delegated to its execution backends | **DOES NOT MOVE, and the reason changed.** Real enforcement landed: the tool-authority guard Phase 21 confirmed **ABSENT** is now computed unconditionally at all six production spawner sites, `PolicyGate` went from **zero callers and fail-open** to installed at dispatch from the same snapshot, and filesystem/egress/secret REFUSED live with a child that demonstrably took its own provider turn (`F21-REVERIFY@ac94b1d5`). F21-01 is COMPLETE. **It still does not promote**, on four checkable grounds: (1) `F21-REVERIFY` is **Linux only** — no Windows evidence exists at the repaired SHA and Windows equivalence is asserted by nobody; (2) Success Criterion 3 is **NOT MET** and the phase goal was graded NOT ACHIEVED a third time; (3) four of six budget dimensions hold **by absence of a request channel, not by enforcement** — `engine.rs:6173` is still `begin_active_turn(turn_id, None)` and `SubAgentConfig` carries no budget field; (4) **the carried `delegate_isolation` limitation is now discharged as a NEGATIVE, not cleared** — the shipped binary's own activation stream emits `unavailable / isolation_not_enforced` **18 times at `2ecdfdf5`, a descendant of the Phase 20 seal** (`F05-NEG-PERSISTS@2ecdfdf5`). Panel 2-1 CONSTRUCTED; kimi-k3's REACHED dissent is recorded and is right about the sub-area — the tool/filesystem/egress/secret dimensions individually carry REACHED-kind evidence. **Open question for F30 — not resolved here:** whether the Delta's "operationally unproven" clause still describes Core after the F21 repair | Phases 21 + 25-04 + 28-02, 2026-07-28 | Phase 30 (F30-01 independent CTRL-01 review); a Windows run at or after `ac94b1d5`; a re-read of the `delegate_isolation` identity at or after `ac94b1d5` |
| TXN-* | delegated workspace, journal, gates, parent CAS | core | core | **REACHED — REOPENED** *(was EFFECTIVE)* | `F20-SEAL@01a5b0ae`; `F20A-SEAL@9821ef76`; `RUN-30184651330`; `F21-VERDICT@f2d186f6`; `F21-REVERIFY@ac94b1d5`; `F23B-H1-RECOVERY`; `F22-VERDICT-2026-07-27`; `F28-CANDIDATE@32e2f57d`; `F28-MATRIX-651`; `CTRL01-PANEL-2026-07-28` | `BASE-2026-07-13` — Hermes 0.17.0 @ `dbe734be`; OpenClaw 2026.6.2 @ `11a0ad10` | Autonomous coding: **Core architectural lead, runtime certification required** (`GAP-AUDIT §3`). Probe: **no counterpart in either peer baseline.** No delegated-workspace/gated-merge/parent-CAS lifecycle exists in Hermes @ `dbe734be` (its `git worktree` references are LSP workspace management — `agent/lsp/workspace.py`, `agent/lsp/manager.py` — plus CI lint) or OpenClaw @ `11a0ad10` (references are `src/infra/update-runner.test.ts` and two plugin-install tests). This is Core's clearest unique capability | **DEMOTED AND REOPENED under the admission rule's contradictory-live-evidence clause — the only demotion in this refresh.** Two defects were measured **against the shipped binary** after EFFECTIVE was awarded. (1) **F21-04-03**: two parallel `Spawn` siblings collide on the journal-head CAS at `session_journal/reducer.rs:708`; the loser does not retry, its budget authority is reported **permanently faulted**, both siblings die, and the parent session is left with a nonterminal tool execution — 3 of 8 live Linux runs and **6 of 6** on Windows. (2) **`F23B-H1-RECOVERY`**: journals already on disk carrying an explicit `"effect_receipt":null` failed their checksum on read — silent, permanent user data loss in the product whose durability claim rests on journals surviving. **Both are now repaired**, but the repairs restore only part of the basis: F21-04-03 is re-proved **6/6 on Linux only**, with the Windows recurrence listed unverified in `F21-REVERIFY`'s own `behavior_unverified_items`, and Phase 22's journal-compatibility determination is Linux-only with the Windows M1–M5 legs never taken. **The `9821ef76` seal did not falsify F21-04-03 — it never ran two parallel siblings; Phase 21's attribution corpus was the first thing on this program to do so.** The tri-family proof that earned EFFECTIVE therefore never covered the defect, and EFFECTIVE was over-stated rather than invalidated. Panel **unanimous 4-0** that the row reopens and cannot retain EFFECTIVE. **Companion action owed and NOT performed by this lane: both defects belong in `FIELD-REGRESSIONS.md`, which this lane does not own.** `REQ-native-r12`/`r13` remain OPEN. Not OPERATOR_COMPLETE: no operator-facing supervision surface | Phases 21 + 22 + 23B-H1 + 28, 2026-07-28 | Windows re-proof of F21-04-03 and of Phase 22's M1–M5 legs; plans 28-03 (soak) and 28-04 (signed receipt) |
| GOAL-* | Goal, Task, Wait, Fleet, loop ownership | shared | core | **REACHED** *(was CONSTRUCTED)* | `F05-TRUTH-2`; `F05-TRUTH-4`; `F22-VERDICT-2026-07-29`; `F22-C3-FIVE-ENGINES`; `F22-MIDFLIGHT-STALE@5457710e`; `F22-LEARNEDPOLICY-WIRED@c5ca677c`; `F22-FLEET-WIRE`; `F24-02-AUTOMATION`; `GOAL-PANEL-2026-07-29` | `BASE-2026-07-13` — Hermes 0.17.0 @ `dbe734be`; OpenClaw 2026.6.2 @ `11a0ad10` | Formal orchestration: **Core lead in primitives**; durable async agents: **Core behind, especially OpenClaw**; loop and spend governance: **partially landed, not complete** (`GAP-AUDIT §3`). Probe: OpenClaw ships a durable task plane — `src/tasks/` with `task-completion-contract.ts`, `detached-task-runtime.ts`, `detached-task-runtime-state.ts`, `cron-task-cancel.ts` — plus `src/commitments/` (heartbeat policy, extraction, store, runtime) and `src/cron`, `src/flows`. Hermes ships `cron/` and `agent/` spawners. Core has the primitives but no durable Goal/Task/Wait kernel | **MOVES TO REACHED, on the ledger's OWN stated deciding test — and that test is quoted here because the previous refresh wrote it down against this exact row.** It said the deciding difference between this family and `REACH-*` was that `REACH-*` *"carries two-platform live product exercise on the criterion that passed and — unlike GOAL-*, has no mapped F05 identity recorded `runtime path unwired`"*. **Both conjuncts now hold for GOAL-\*, and one of them never should have been recorded against it.** **(1) The F05 blocker is gone, and half of it was never real.** `F05-TRUTH-2` (mid-flight monitor) was **STALE, not unwired**: the shipped `0.12.25` binary's own activation stream emits `declared → configured → constructed → ready → reached → outcome_changed → observed` plus `{"type":"mid_flight_monitor_decision","directive":"replan","reason":"repeated_error"}`, so BOTH columns of that row were false at `2ecdfdf5` and after; one-variable negative control takes the decision and occurrence counts 1→0 while `ready` stays 1 (`F22-MIDFLIGHT-STALE@5457710e`). `F05-TRUTH-4` (learned policy) **was** real and was worse than the row recorded — `AgentExecutorConfig` carried a `pub learned_policy` field with **zero readers in the entire workspace** while its doc claimed `dispatch_once` consulted it — and is now wired as a **narrowing-only** pre-filter (the policy gate is consulted first and its denial is final), live-proven in one run where the parent (`Root`) reads a file and the delegated child (`SubAgent`) gets ``Denied by sub-agent learned policy: Read matched rule `*` ``, with a no-policy control arm in which the child reads it (`F22-LEARNEDPOLICY-WIRED@c5ca677c`). **(2) The two-platform live exercise was already there** and this ledger already called it *"REACHED-kind on its own"*: `F22-FLEET-WIRE`, `kill -9` mid-fanout on the shipped release, Linux **and** Windows, 12/12/12 effects, counting gate falsified in the same run. **Two of the five recorded blockers are also simply no longer true.** C3 — *"never attempted; five engines still return five types"* — is superseded: all five engines now have a **production path** to one canonical Goal transition, live-proven against the shipped release, after the root cause was found to be that `goal open` **hard-coded `GoalStrategy::Fleet`** so the product could not express four of its five strategies (`F22-C3-FIVE-ENGINES`). C1 moved from *"failed on two of three surfaces"* to **all three observe**. **Panel 3-0 REACHED (codex / gemini / kimi-k3), plus an internal adversarial pass that argued for holding at CONSTRUCTED and lost.** Its case, preserved because it is right about the sub-areas: the previous refresh listed *"phase goal NOT ACHIEVED"* and *"every requirement OPEN"* as blockers in their own right, and treating them as non-blocking imports the `REACH-*` precedent onto a row whose own author did not; and GOAL-\*'s passed criterion (C2, fleet claims) is one member of five while the family's **namesake** concept, C3 "one loop owner", is PARTIAL rather than PASSED. It lost because both points argue against EFFECTIVE, not against REACHED — `REACH-*` itself promoted with **three of four criteria NOT MET**, so criterion incompleteness was already ruled non-decisive by this ledger. **Emphatically NOT EFFECTIVE, and the boundary is sharp:** the phase goal is **NOT ACHIEVED for the third time**, and it now fails on one word — all three surfaces **observe**, and only the CLI can **control**, because the host→core Goal command must be answered in `crates/wcore-cli/src/main.rs`, the shared file every lane is fenced out of; the two declared Goal producer fixtures are **0 of 49 on disk**, awaiting the single `wcore-contract generate` pass no lane may run; C3's live proof is **Linux only** and attachment is **opt-in**, so an engine run with no Goal is unenforced; C4's reconnect / preemption / missed-interval clauses are untouched; C5's Windows M1–M5 legs were never taken. **One limitation is new and is recorded rather than smoothed:** `F05-TRUTH-4`'s *runtime outcome proof* column stays **None in practice** — `OutputSink::emit_capability_activation` is a default no-op only `ProtocolSink` overrides, and every spawned child gets `NullSink` or `ChannelSink`, so **no sub-agent capability activation of any kind is observable on any topology in this tree**. **Open question for F30 — carried forward unresolved:** whether the Delta's *"durable async agents: Core behind, especially OpenClaw"* still holds now that a Core fleet ledger survives kill/restart on both platforms and five engines terminate through one transition | Phases 22 + 24-02, 2026-07-29 | Criterion 1's control half — ONE host→core Goal command answered in the fenced `wcore-cli/src/main.rs` — which is the highest-leverage item left in this family; seam request `SR-22-C1`'s single contract regeneration to materialise the two declared Goal fixtures; a Windows leg for C3; `ChannelSink` forwarding capability activations so sub-agent F05 outcome proofs become observable |
| CONT-* | governed skills, session recovery, memory, index, cache economics | shared | core | REACHED *(unchanged)* | `F05-RECEIPT@0825c92d`; `F05-TRUTH-5`; `F05-TRUTH-7`; `F05-TRUTH-8`; `F05-TRUTH-1`; `F05-TRUTH-3`; `F23A-DISPOSITION`; `F23B-01-SESSION`; `F23B-02-MEMORY`; `F23B-03-INDEX`; `F23B-04-CLOCK`; `F23B-H1-RECOVERY` | `BASE-2026-07-13` — Hermes 0.17.0 @ `dbe734be`; OpenClaw 2026.6.2 @ `11a0ad10` | Memory: **Core architectural lead, outcome proof needed**; self-improving skills: **Core stronger research machinery, weaker governance/completion**; crash recovery: **Core behind** — WAL is not a complete turn/event journal (`GAP-AUDIT §3`). Probe: OpenClaw ships `src/memory`, `src/sessions`, `src/transcripts`, `src/trajectory`, `src/skills`, `packages/memory-host-sdk`; Hermes ships `skills/`, `optional-skills/`, `agent/` memory paths and `agent/curator_backup.py` | **DOES NOT MOVE, on a family-not-member basis, and the sub-areas diverged sharply.** Materially stronger: the **index** sub-area closed every mandatory F23-06 clause with all three platform legs PASS on real hardware, nonce-bound, each against a binary whose `--build-info` SHA was asserted before measurement (`F23B-03-INDEX`), with only the OPTIONAL semantic layer deferred; **session recovery** ships every operator verb on Linux and closes live Windows UAT defect D2 (`F23B-01-SESSION`); **memory** provenance and its four controls run through the unmodified `MemoryAccessGate` (`F23B-02-MEMORY`). Unmoved or negative: **governed skills is Success Criterion 1 NOT MET** — `promote`, `revoke` and `rollback` do not exist, `run_skills_promote` fails closed at `wcore-cli/src/main.rs:2408`, and "cannot execute before governed promotion" is satisfied **only by the absence of any promotion path**, which 23A itself records as a vacuous satisfaction; **F23A-01-H2 is open and committed red** (any errored tool call kills the session). **Cache economics is still not started** — F23-04 was never executed, so `F05-TRUTH-1` and `F05-TRUTH-3` remain `Unavailable: no production constructor`, exactly as at the last refresh. The **multi-day journey has day one only**, Linux and Windows; **macOS NOT ACHIEVED, nothing run**, and the journey **cannot close before `2026-07-30T23:54:26Z`**. EFFECTIVE would claim the family; two of its five sub-areas are unbuilt. **Open question for F30 — not resolved here:** whether "crash recovery: Core behind" survives `F23B-01-SESSION` + `F23B-H1-RECOVERY` | Phases 23A + 23B, 2026-07-28 | 23B-04 Tasks 2-3, not closable before `2026-07-30T23:54:26Z`; governed promotion (F23-01) and cache economics (F23-04), both unbuilt |
| GATEWAY-* | service, automation, channels, typed API | shared | core | **CONSTRUCTED** *(was ABSENT)* | `PEER-PROBE-2026-07-26`; `GAP-AUDIT-2026-07-13 §3`; `F24-01-GATEWAY`; `F24-02-AUTOMATION`; `F24-B-LIFECYCLE`; `F24-C-ARRIVAL`; `F24-04-TYPEDCLIENT`; `CTRL01-PANEL-2026-07-28` | `BASE-2026-07-13` — Hermes 0.17.0 @ `dbe734be`; OpenClaw 2026.6.2 @ `11a0ad10` | Persistent gateway/service: **Core/Wayland behind**; channels: **Wayland behind** (`GAP-AUDIT §3`). Probe — the widest measured gap: Hermes ships a full `gateway/` package (`pairing.py`, `delivery.py`, `drain_control.py`, `platform_registry.py`, `channel_directory.py`, `authz_mixin.py`, `relay/`, `platforms/`, `builtin_hooks/`, `memory_monitor.py`, `code_skew.py`) plus `tui_gateway/`, `cron/`, `apps/`, `web/`, `website/`. OpenClaw ships `src/gateway`, `src/daemon`, `src/channels`, `src/pairing`, `src/node-host`, `src/hooks`, `src/commitments`, `packages/gateway-protocol`, `packages/gateway-client`, `packages/sdk`. Core has fragmented headless surfaces and **no persistent gateway runtime** | **THE LARGEST MOVE IN THIS REFRESH — three levels — and still short of REACHED.** `ABSENT` was true in 2026-07-26 and is now false: a persistent gateway runtime exists and was driven on real `systemctl --user` against the real release binary, where **systemd itself** recorded the `kill -9` and restarted the service with a new pid — nothing in the run restarted it (`F24-B-LIFECYCLE`); drain moved `Draining (pending 12)` → `Drained (pending 0)` with all twelve abandoned by name and recorded durably; delivery arrival was proved **at an independent sink** and **the gate failed first** — before the fix the outcome-unknown delivery produced two messages, after it exactly one (`F24-C-ARRIVAL`); and a typed client that severed its connection having received **zero bytes** recovered an event produced entirely after it left (`F24-04-TYPEDCLIENT`). **Why not REACHED**, on the family basis: the phase goal is "on **every OS family**" and **two of three have no gateway evidence at all** — the macOS CI binary **provably does not carry this code** (`--help \| grep -cE "^\s+gateway"` → `0`, measured; blocked on a CI-trigger change that is a PR action), and the Windows gateway path was never exercised. Criterion 5 (setup-to-recovery journeys on macOS, Linux **and** Windows) is untouched, 24-04's own four tasks were never started, the phase is **NOT closed**, and nine channel adapters still inherit `supports_outbound_idempotency() == false` — for which an outcome-unknown delivery is now correctly *abandoned*, which is safe and honest and is not the same thing as delivered. Panel 2-1 CONSTRUCTED; the internal adversarial pass argued REACHED and lost — its case is preserved below and is right about the Linux service-lifecycle and typed-client sub-areas taken alone. **Open question for F30 — not resolved here:** the Delta's "Core has … **no persistent gateway runtime**" is a statement about Core that this phase falsified; the peer half of the comparison is untouched and still stands at the pinned baseline | Phase 24 (24-01, 24-02, 24-B, 24-C, 24-04), 2026-07-28 | Phase 24 carry-over: the macOS and Windows gateway legs, and Criterion 5's three-platform journey |
| REACH-* | backends, nodes, plugins | shared | core | **REACHED** *(was SOURCE)* | `PEER-PROBE-2026-07-26`; `GAP-AUDIT-2026-07-13 §3`; `F25-01-BACKENDS`; `F25-02-PLUGINS`; `F25-03-NODES`; `F25-04-FAILCLOSED` | `BASE-2026-07-13` — Hermes 0.17.0 @ `dbe734be`; OpenClaw 2026.6.2 @ `11a0ad10` | Remote execution: **Core behind Hermes**; extension distribution: **architecture competitive, ecosystem behind** (`GAP-AUDIT §3`). Probe: Hermes ships **seven** execution backends behind one `base.py` contract — `tools/environments/{local,docker,ssh,singularity,modal,managed_modal,daytona}.py` plus `file_sync.py`. OpenClaw ships `src/node-host`, `src/plugins`, `src/plugin-sdk`, `src/plugin-state`, `packages/plugin-sdk`, `packages/plugin-package-contract`, `extensions/`. Core has `wcore-plugin-api` and sandbox/worktree assurance but **no user-facing execution-backend matrix** | **MOVES TO REACHED — the strongest Core-side result in this refresh, and the only family carrying a MET Success Criterion.** The previous limitation said plainly "the F25-01 provider-neutral contract does not exist"; it exists, and three of its four reference backends **ran the same deterministic task through the shipped binary and diffed to EQUIVALENT** (`F25-01-BACKENDS`). All twelve `wayland-core plugin` verbs plus all four negative cases were driven through the shipped **release** binary on **Linux and Windows**, with approval enforced at load time and bound to the plugin directory's SHA-256 so an update invalidates consent — **Success Criterion 3 MET on Linux, MET-with-one-divergence on Windows** (`F25-02-PLUGINS`). Node identity is attested inside the signed receipt and survived all five disruptions (`F25-03-NODES`); all five hostile compromise cases fail closed on both hosts (`F25-04-FAILCLOSED`). **The deciding difference from GOAL-\* and GATEWAY-\*, stated because those calls are close:** this family carries **two-platform live product exercise on the criterion that passed** and — unlike GOAL-* — has **no mapped F05 identity recorded `runtime path unwired`**. **Not EFFECTIVE:** three of four criteria are NOT MET. C1 fails on the hibernating cloud leg, unexercised for want of a credential only Sean can mint. C2 fails because everything was exercised against a separate machine *identity* but **not a second physical host** — no SSH trust relationship exists between `hetzner-dsm` and `SeanD@seandesktop` and creating one is reserved to Sean. C4 fails because SSH and cloud cannot be enumerated on the proof hosts, so their orphan counts are `NOT MEASURED` and "across every reference backend" is not satisfied by two. **Open question for F30 — not resolved here:** the Delta's "Core has … **no user-facing execution-backend matrix**" is a statement about Core that `wayland-core backend` falsified; the seven-backend Hermes comparison is untouched | Phase 25, 2026-07-28 | A Sean-minted cloud credential (C1); an authorized second-host SSH trust relationship (C2); SSH/cloud orphan enumeration (C4) |
| PORT-* | import, export, backup, restore | shared | core | **REACHED** *(was SOURCE)* | `PEER-PROBE-2026-07-26`; `GAP-AUDIT-2026-07-13 §3`; `F26-01-DISCOVERY`; `F26-03-BACKUP` | `BASE-2026-07-13` — Hermes 0.17.0 @ `dbe734be`; OpenClaw 2026.6.2 @ `11a0ad10` | Migration: **Core behind** — no complete OpenClaw import and incomplete profile migration (`GAP-AUDIT §3`). Probe — **both peers migrate from each other, and Core is the only party with no reciprocal path**: Hermes ships `hermes_cli/migrate.py`, `hermes_cli/backup.py`, `hermes_cli/subcommands/backup.py`, `hermes_cli/codex_runtime_plugin_migration.py`, `agent/curator_backup.py` and an explicit `optional-skills/migration/openclaw-migration/scripts/openclaw_to_hermes.py`. OpenClaw ships `docs/install/migrating-hermes.md`, `docs/install/migrating-claude.md`, `docs/cli/{backup,migrate}.md`, `docs/plugins/reference/migrate-hermes.md`, `extensions/anthropic/cli-migration.ts`, `extensions/codex/src/migration/{apply,auth}.ts`, `apps/macos/…/UserDefaultsMigration.swift` | **MOVES TO REACHED on the discovery and export/backup/restore halves; the import half is untouched.** Discovery against **Sean's real Hermes and OpenClaw installs on macOS with live credentials** extracted 7 real secret values and produced **0 hits** in either emitted document, with both homes unmutated and byte-identical reruns — the redaction is structural and the non-mutation is measured by tree digest, not asserted (`F26-01-DISCOVERY`). Backup, restore and exact rollback are claimed with the Windows interruption legs run, and the path produced a real product defect found and fixed **at the product** rather than at the fixture: `atomic_write`'s tempfile round trip reached Win32 without long-path handling and failed with **os error 3** at a 320-character non-verbatim absolute path — a function 41 modules call (`F26-03-BACKUP`). **CORRECTED 2026-07-30 by `lane/port-import` — the previous text of this cell was false at HEAD.** It read *"plans 26-02 and 26-04 were never started ... the F26-02 quarantine contract is still unbuilt ... nothing has yet imported anything"*. All four clauses are refuted: 26-02 landed 2026-07-28 (`ec9794b1`…`a170ee24`), 26-04 the same day (`1c13e9a2`…`0a75efd9`); `migrate/quarantine.rs` is 1132 lines; `apply_plan()` mutates behind an `ApplyGuard`. The cell was authored citing only `Phase 26 (26-01, 26-03)` and never refreshed after the other two lanes landed. **Criteria 2 and 4 are MET, and independently re-verified rather than taken from the building lanes' own summaries:** 78 tests over four migrate binaries pass with **0 ignored, 0 filtered out**; **19 of 19 hostile corpora execute, 0 unrun**; source non-mutation is proven by a digest that self-tests red on content *and* mode changes, **0 mutated over 19**; and the real binary contains an executable carrying `trusted: true` / `auto_promote: true` / `wayland_quarantine: exempt` (`quarantined=1`) while passing a persona whose prose merely contains shell syntax (`quarantined=0`, `imported=2` — not silently dropped). **Mutation-tested in both directions:** blinding the classifier reddens the catch direction (10 failures incl. the live-negative leg); an over-broad classifier reddens the pass-through direction (5 failures incl. `t2`). Two non-blocking findings: the **hostile suite's own** pass-through case is a permanently-green gate — no classifier mutation can redden it, because its corpus is a persona and the persona path is never content-classified (`F-PI-01`, MEDIUM) — and `classify_data_kind` has zero production call sites despite a comment claiming it makes the contract measurable (`F-PI-02`, LOW). **Open question for F30 — not resolved here:** the Delta's "Core is the only party with no reciprocal path" is now false outright, for apply as well as discovery | Phase 26 (26-01, 26-02, 26-03, 26-04); re-verified by `lane/port-import`, 2026-07-30 | `F-PI-01` (hostile pass-through case has no reachable fail state), `F-PI-02` (dead `classify_data_kind`) — both BACKLOG, non-blocking |
| MEDIA-* | attachment, browser/CUA/web, generation, voice | shared | core | SOURCE *(unchanged)* | `PEER-PROBE-2026-07-26`; `GAP-AUDIT-2026-07-13 §3`; `F27-VERDICT`; `F05-NEG-PERSISTS@2ecdfdf5` | `BASE-2026-07-13` — Hermes 0.17.0 @ `dbe734be`; OpenClaw 2026.6.2 @ `11a0ad10` | Browser and computer use: **competitive engine; behind device product**; voice/mobile/devices: **whole-Wayland behind** (`GAP-AUDIT §3`). Probe: OpenClaw ships `packages/{media-core,media-generation-core,media-understanding-common,speech-core,web-content-core}` and `src/{image-generation,media-generation,media-understanding,music-generation,video-generation,media,tts,talk,web-search,web-fetch,link-understanding}`. Hermes ships `plugins/browser/{browser_use,browserbase,firecrawl}` and `tools/computer_use`. Core has `wcore-browser` and `wcore-cua` crates with policy boundaries — engine-competitive, product-behind | **DOES NOT MOVE. The cleanest non-move in this refresh: the phase ran and closed four of five criteria NOT MET.** `F27-VERDICT` grades the goal **NOT ACHIEVED** with 0/5 requirements complete. C2 is unmet because **nothing is published** — the previous limitation said `wcore-browser`/`wcore-cua` publish no live readiness truth, and that is still exactly true; `browser_suite` and `computer_use` still report `true` on a box with no browser binary and no display, measured invariant across five single-variable observations with the very next operation failing `spawn camoufox: No such file or directory`. **This ledger's own new evidence corroborates it independently**: the `ready` frame captured at `2ecdfdf5` inside `F05-NEG-PERSISTS@2ecdfdf5` carries `"browser_suite":true,"computer_use":true` on that same machine. C3 is unmet because none of the four generation shapes was exercised. C4 is unmet because **no audio ever flowed and no interruption ever occurred on any machine** — recorded by the phase as an execution shortfall, not an environmental impossibility. C5 is unmet because **zero packaged smokes ran on zero platforms**, and the previous limitation's "no deterministic media corpus has run on native macOS/Windows" stands unchanged. What did land is real and small: one bounded open-once magic-byte document intake and an image-degradation gate on the Anthropic and Gemini builders, both live-proved (C1 PARTIAL). A repair does not promote a family whose four other criteria are NOT MET | Phase 27, 2026-07-28 | Phase 27 carry-over: the fenced readiness protocol seam (`.planning/SEAM-REQUESTS/27.md`), the four generation shapes, any voice leg at all, and packaged smokes |
| NATIVE-* | macOS/Linux/Windows packaged certification | shared | shared | **REACHED** *(was SOURCE)* | `F20A-SEAL@9821ef76`; `RUN-30184651330`; `PEER-PROBE-2026-07-26`; `F28-CANDIDATE@32e2f57d`; `F28-MATRIX-651`; `F28-CONTROL-WEDGE`; `F-28-02-001-FIXED`; `CTRL01-PANEL-2026-07-28` | `BASE-2026-07-13` — Hermes 0.17.0 @ `dbe734be`; OpenClaw 2026.6.2 @ `11a0ad10` | Cross-platform contract: **unproven until packaged E5 matrix** (`GAP-AUDIT §3`). Probe: OpenClaw ships real native app targets — `apps/{macos,ios,android,macos-mlx-tts,swabble}` — plus `appcast.xml` and `deploy/`. Hermes ships `packaging/homebrew`, `docker/`, `nix/` and `docker-compose.windows.yml`. Core has explicit cross-platform architecture and CI intent but **no packaged native product target** | **MOVES TO REACHED. Panel unanimous 4-0.** The previous limitation's two named absences are discharged: "no E5 matrix" is false — the matrix exists and ran; and the 20A datapoint is no longer the only native evidence. **651 of 651 cells carry an outcome and none is skipped; all 147 unskippable critical cells ran; the set was not narrowed** (`F28-MATRIX-651`). Every family ran **the CI release artifact itself**, sha256-asserted on the host before the run, against a candidate with **6 of 6 per-target digests bound** (`F28-CANDIDATE@32e2f57d`) — and the recurring "no macOS binary is obtainable" belief was refuted by downloading one and executing it, not by argument. Sandbox greens carry a **differential** activeness observation, so absence of a violation is not expressible as a green. **Why not EFFECTIVE, and this is the honesty point that matters most in this row: 24 cells are still RED at the certified candidate.** They were macOS sandbox activeness, root-caused as an **observability** gap rather than a containment gap and disposed **FIXED** (`F-28-02-001-FIXED`) — but **the matrix has never been re-run at any candidate carrying that fix**, so the certified candidate `32e2f57d` still reads macos 192-**24**-0. There is no soak (28-03 not landed), no signed platform-binding receipt and no finding adjudication (28-04 not landed), the phase is not closed, and the certification **explicitly covers `32e2f57d` and not the current tip**. `PACKAGED_PROVEN` is nowhere near. The observability control returned `wedge-clearable` but **does not generalise** — `seandesktop` is one physical box and `KR-06` must not close on it. **Open question for F30 — not resolved here:** the Delta's "no packaged native product target" distinguishes a *product* target (installer/app bundle) from a per-target CI binary; Core still has the former absent and now certifies the latter | Phase 28 (28-01, 28-02, F-28-02-001), 2026-07-28 | A matrix re-run at a candidate carrying the F-28-02-001 fix; plans 28-03 (soak) and 28-04 (signed receipt + adjudication + phase verdict) |
| SUPPLY-* | provenance, SBOM, signing, update, rollback | shared | shared | **CONSTRUCTED** *(was SOURCE)* | `F03-RECEIPT@1c644ccd`; `PEER-PROBE-2026-07-26`; `F29-CENSUS`; `F29-SBOM@5028fe28`; `F29-DENY-FAIL`; `F29-REPRO-VARIANCE`; `F29-02-H1`; `CTRL01-PANEL-2026-07-28` | `BASE-2026-07-13` — Hermes 0.17.0 @ `dbe734be`; OpenClaw 2026.6.2 @ `11a0ad10` | No `GAP-AUDIT §3` dimension covers supply chain; delta is probe-derived. Probe: OpenClaw has npm trusted publishing with provenance (`scripts/openclaw-npm-publish.sh:44` passes `--provenance`; `.github/workflows/openclaw-npm-release.yml:622` has a `Verify prepared tarball provenance` step) plus an update/rollback surface (`src/cli/update-cli.ts`, `src/infra/update-runner.ts`, `appcast.xml`, `scripts/make_appcast.sh`). **Hermes has no SBOM, cosign, provenance or SLSA match anywhere in `.github/` or `scripts/` at `dbe734be`** — a real negative finding, so OpenClaw alone sets the peer bar. **Neither peer ships an SBOM at baseline**, so Core's F29-01 SBOM requirement has no counterpart to match — it would be a lead if proven | **MOVES ONE LEVEL ON THE MINORITY PANEL POSITION, and the reasoning is stated because the majority went the other way.** Codex and gemini both held SOURCE; kimi-k3 said CONSTRUCTED, and **the minority is taken because its evidence is about what exists rather than what is absent.** `SOURCE` is a claim that only code is present; what happened here is **execution against the real product**: an SBOM was generated from the real locked graph — 865 components, 447,364 bytes, `5028fe28…` — and proved **byte-identical from checkouts at different absolute paths** after a genuine defect in the serial derivation was found and fixed (`F29-SBOM@5028fe28`); `cargo deny check` **executed for the first time in the repository's history** against 1,017 real crates (`F29-DENY-FAIL`); and reproducibility was measured on two real 320-second release builds (`F29-REPRO-VARIANCE`). Calling that `SOURCE` would be as inaccurate as inflating it. The previous limitation's "Signing primitives ≠ a release supply chain" is superseded in part: a signed release manifest, a role-scoped trust root and a closed four-state release ledger now exist. **Everything that blocks REACHED is recorded here rather than smoothed:** all of the above is proved **only against a throwaway Ed25519 key generated at run time into a temp directory — no real trust root is bound and no release has ever gone through any of it**; the dependency-policy verdict is **RED** (`advisories FAILED, bans ok, licenses FAILED, exit 5`) and is deliberately **not** chained into `check-all`; reproducibility is **DOCUMENTED-VARIANCE class `path_prefix`** — the shipped release is reproducible **only accidentally**, because GitHub runners always check out at the same absolute path; **nine HIGH census findings**; `grep -rn 'environment:' .github/workflows/` returns **zero**, so no job declares the only native manual-approval gate GitHub offers; and **rollback rehearsal does not exist anywhere in CI**. **One HIGH is OPEN and unfixed: `F29-02-H1`** — `.cargo/audit.toml` silences two RUSTSEC advisories on a stated "sole path" when the real graph has **three**, two of them through `wcore-tools`, which parses **user-supplied** docx/pptx/xlsx; `0194` is reachable. Three of the family's five named members (update identity, revocation/rotation, rollback) are entirely unbuilt. **Open question for F30 — not resolved here:** the Delta's "it would be a lead if proven" conditional is now discharged **on Core's side only**; whether that constitutes a lead is F30-03's call, not this refresh's | Phase 29 (29-01, 29-02), 2026-07-28 | Plans 29-03 (install/update identity, revocation, key rotation) and 29-04 (tamper corpus, four-state separation, phase verdict); disposition of `F29-02-H1` |

---

## Maturity panel — 2026-07-28

Six rows were genuinely ambiguous and were put to the four-way panel with an identical prompt
(`CTRL01-PANEL-2026-07-28`). The prompt carried the enum, eight binding rules, and the evidence
**for and against** each row. Splits are recorded verbatim, including where the adopted answer
lost the vote.

| Row | codex-sol | gemini-3.1-pro | kimi-k3 | Internal adversarial | Adopted | Basis |
|---|---|---|---|---|---|---|
| AUTH-* | CONSTRUCTED (+reopen) | CONSTRUCTED | REACHED | CONSTRUCTED | **CONSTRUCTED** | majority — 2-1 external, 3-1 with the internal pass |
| TXN-* | REACHED + REOPENED | REACHED + REOPENED | REOPENED → REACHED | REACHED + REOPENED | **REACHED + REOPENED** | unanimous, 4-0 |
| GOAL-* | SOURCE | CONSTRUCTED | REACHED | CONSTRUCTED | **CONSTRUCTED** | no majority; middle position on a checkable fact, both dissents recorded |
| GATEWAY-* | CONSTRUCTED | CONSTRUCTED | REACHED | REACHED | **CONSTRUCTED** | external majority 2-1; the internal pass lost |
| NATIVE-* | REACHED | REACHED | REACHED | REACHED | **REACHED** | unanimous, 4-0 |
| SUPPLY-* | SOURCE | SOURCE | CONSTRUCTED | CONSTRUCTED | **CONSTRUCTED** | **minority-by-external-count taken, 2-2 overall** — see below |

Four rows were **not** put to the panel because their evidence was not close: CONT-* (held at
REACHED), REACH-* (SOURCE → REACHED), PORT-* (SOURCE → REACHED) and MEDIA-* (held at SOURCE).
Their reasoning is in the Limitation column, and REACH-*'s cell states the deciding difference
from the two rows that were capped below REACHED, so the calls can be checked against each other.

**Where the adopted answer is not the majority, and why.**

- **SUPPLY-\*** — codex and gemini both anchored on absence (no rollback rehearsal, no revocation,
  throwaway keys) and concluded `SOURCE`. Every one of those reasons is true and every one is
  recorded in the Limitation column, but none is an argument about the level: `SOURCE` asserts that
  only code is present, and an SBOM generated from the real locked graph, a `cargo deny` run against
  1,017 real crates and two real 320-second release builds are executions, not source presence. The
  minority position is the one that engages the enum's own definition, so it is taken.
- **GATEWAY-\*** — the internal adversarial pass argued for `REACHED` and lost to the two external
  members. Its case was strong and is preserved: systemd-witnessed recovery, drain by name,
  independent-sink arrival that **failed first and then passed**, and a severed client recovering a
  post-disconnect event are all textbook REACHED-kind evidence. It loses on the family rule — the
  phase goal says "on every OS family", and two of three families have **no gateway evidence at
  all**, one of them because the shipped macOS binary was *measured* not to contain the code.
- **AUTH-\* was not reopened, though one member said it should be.** codex-sol attached a REOPEN to
  its CONSTRUCTED verdict on the strength of the persisting `delegate_isolation` negative. It is not
  taken, and the reason is checkable: a reopen requires evidence that *contradicts* the recorded
  level, and this evidence *agrees* with it — `Unavailable: isolation not enforced` is what the row
  already carried, and CONSTRUCTED is what it already was. The finding changes the row's
  justification, not its standing. gemini-3.1-pro and kimi-k3 attached no reopen.
- **GOAL-\*** — no majority existed at all (SOURCE / CONSTRUCTED / REACHED). The middle was taken on
  a checkable fact rather than by splitting the difference: this row's own mapped F05 evidence
  records two capabilities as `Unavailable: runtime path unwired`, so the ledger's own evidence says
  the family's runtime path is not wired. That fact is also what distinguishes GOAL-* from REACH-*,
  which carries no such recorded negative and did move to REACHED.

**Panel hygiene.** All three externals were invoked in the forms known to silently drop a vote:
`gemini` with `--skip-trust`; `kimi` by absolute path with unanchored extraction; `codex` with the
**last** matching block taken, since it repeats its final block verbatim and the earlier copy was
discarded. Each returned six distinct per-row verdicts with reasoning, so no member's vote was lost
to a malformed invocation. The panel was given the evidence against each row as prominently as the
evidence for it.

---

## F03/F05 retroactive evidence map

This section discharges the admission rule's first clause. Both receipts were read in full; every
mapping below cites the specific claim it rests on.

**`F03-RECEIPT@1c644ccd`** — the evidence substrate for the whole ledger, not a capability row of
its own. It maps to two families:

- **AUTH-***: render-all-then-scan publication (a provider/canary secret in any projection rejects
  the whole bundle before persistence); redacted JSON/JSONL/JUnit/console/Markdown projections with
  no raw prompt, model output, tool payload, stderr, call ID, secret or worktree path; typed egress
  and filesystem evidence fields. **Counter-evidence in the same receipt:** those egress/filesystem
  and provider attempt/retry/token/cache fields are recorded `Unavailable`, and the receipt states
  the design intent — absent measurements "cannot be represented by plausible zero values" and
  therefore fail the milestone gate rather than becoming fake success.
- **SUPPLY-***: SHA-256 content addressing over the canonical body; detached, domain-separated
  Ed25519 signatures; an external verification policy binding receipt → trusted key, source commit,
  binary digest, repository, ref, workflow, authority; separated integrity / authority / release-gate
  decisions. **Bounded by:** local receipts are always non-authoritative and no trusted CI signer is
  bound. **Refresh note 2026-07-28:** `F29-CENSUS` found that the release path *already* carries
  keyless Sigstore SLSA provenance independently of F03's primitives, and `F29-SBOM@5028fe28` adds
  an artifact F03 never had. Neither changes the bound: the signer is still out of band.

**`F05-RECEIPT@0825c92d`** — the origin of this ledger's maturity vocabulary. F05's typed stages
(`declared`, `configured`, `constructed`, `ready`, `reached`, `outcome_changed`, `observed`,
`unavailable`) are the direct ancestor of the `ABSENT → … → PACKAGED_PROVEN` enum above, which is
why F05's per-capability truth table can be mapped into row maturity without reinterpretation.

All eight audited capability identities are mapped; none is dropped. The **2026-07-28 status**
column records what phases 21-29 did or did not change about each identity.

| # | F05 capability | F05 effective startup truth | Runtime outcome proof | Ledger row | Status at 2026-07-28 |
|---|---|---|---|---|---|
| 1 | Pricing refresher | Unavailable: no production constructor | None | CONT-* (cache economics) | **Unchanged.** F23-04 was never started (`F23B-02-MEMORY`) |
| 2 | Mid-flight monitor | ~~Unavailable: runtime path unwired~~ → **Ready** | ~~None~~ → **`reached → outcome_changed → observed`** | GOAL-* (loop ownership) | **CORRECTED 2026-07-29 — this row was STALE, and BOTH columns were false.** The shipped `0.12.25` binary's own activation stream emits the full ready chain plus `mid_flight_monitor_decision`; the monitor is consulted at `engine.rs` in both the provider and tool loops. One-variable negative control 1→0. `F22-MIDFLIGHT-STALE@5457710e` |
| 3 | Cooldown tracker | Unavailable: no production constructor | None | CONT-* (cache economics) | **Unchanged.** F23-04 was never started |
| 4 | Learned policy | ~~Unavailable: runtime path unwired~~ → **Ready from a constructed on-disk policy** (else `disabled_by_config`) | **still None — emitted but structurally unobservable, see below** | GOAL-* (loop ownership) | **WIRED 2026-07-29.** The row was real and understated it: `AgentExecutorConfig.learned_policy` had **zero readers in the workspace** while its doc claimed otherwise. Now a narrowing-only sub-agent pre-filter, gate-first, live-proven parent-vs-child in one run. `RuntimePathUnwired` is no longer reachable for it. `F22-LEARNEDPOLICY-WIRED@c5ca677c` |
| 5 | Smart handoff | Ready from concrete memory construction | Successful episode persistence | CONT-* (memory) | Reinforced by `F23B-02-MEMORY` — provenance plus four gated controls |
| 6 | Delegate isolation | **Unavailable: isolation not enforced** | None | AUTH-* (sandbox/isolation) | **CONFIRMED PERSISTING.** See below |
| 7 | Procedure skill drafting | Ready from concrete memory construction | Successful quarantine staging | CONT-* (governed skills) | Quarantine confirmed inert across sixteen routes (`F23A-DISPOSITION`); promotion still absent |
| 8 | Legacy auto-skill drafting | Ready | Successful draft write | CONT-* (governed skills) | Unchanged |

Three capabilities reach runtime outcome proof after a real side effect — that is what lifts CONT-*
from `SOURCE` to `REACHED`. Five are honest negatives; per the receipt, "an unavailable row is an
honesty result, not capability completion."

**AMENDMENT, 2026-07-29 (lane `lane/22-remaining`).** Rows 2 and 4 are re-measured above and no
longer read `runtime path unwired`, so **four** capabilities now reach runtime outcome proof and
**three** are honest negatives. Two further rows are **flagged stale but NOT edited**, because they
map to `CONT-*` and belong to whoever owns that row: at this SHA the shipped binary reports
**`cooldown_tracker` `ready`** (this table says "no production constructor") and
**`pricing_refresher` `unavailable / disabled_by_config`** (this table says "no production
constructor"). Both were observed in the same capture as row 2. The pattern is worth naming: **this
table has now been wrong about three of its eight rows in the same direction — understating what
the product does** — because it was transcribed from a 2026-07-13 receipt and re-asserted at each
refresh without re-reading the binary. A row that says a capability is dead is exactly as
falsifiable as one that says it works, and is cheaper to leave unchecked.

**One limitation that is NOT closed and must not be read as closed.** Row 4's *runtime outcome
proof* column stays `None` **in practice**. The occurrence triple is emitted on a real narrowing,
but `OutputSink::emit_capability_activation` (`output/mod.rs:240`) has a **default no-op** body that
only `ProtocolSink` overrides, and every spawned child receives `NullSink` (`Delegate`) or
`ChannelSink` (`Spawn` / workflow runner) — neither overrides it. Since `Root` bypasses the
pre-filter by design, the occurrence can only ever fire inside a child, and every child discards it.
Generalised, and this is the useful form: **no sub-agent capability activation of any kind is
observable on any topology in this tree.**

**The previously unresolved mapping is now resolved — as a negative.** The 2026-07-26 ledger
recorded that Phase 20 "plausibly supersedes" the `delegate_isolation` finding but that **no
Phase 20 artifact re-ran the F05 capability gate**, so there was no evidence either way. There is
now evidence, and it runs against the product: the shipped binary's own capability-activation
stream emits

```
{"type":"capability_activation","capability":"delegate_isolation","stage":"unavailable","reason":"isolation_not_enforced"}
```

**18 times at SHA `2ecdfdf5`** (`F05-NEG-PERSISTS@2ecdfdf5`), a commit that is a **descendant of
the Phase 20A seal `9821ef76`**, and again on Windows in Phase 25's lifecycle lab. So Phase 20 did
**not** clear the F05 negative; the negative survived it and is emitted by the product.

Three qualifications, stated so this is not over-read:

1. This is the **product's own activation stream**, not a fresh execution of the F05 capability
   activation gate harness. It is stronger than absence of evidence and weaker than a re-run gate.
2. The observations are at `2ecdfdf5`, which is **earlier than** the Phase 21 authority repair at
   `ac94b1d5`. Nothing has re-read the identity at or after `ac94b1d5`, and this refresh did not
   run the binary to do so.
3. The identity is `delegate_isolation` specifically. It is **not** a statement about macOS or
   Windows sandbox containment generally, both of which `F28-CONTROL-WEDGE` and
   `F-28-02-001-FIXED` measure separately and more favourably.

AUTH-* therefore keeps the negative and stays at `CONSTRUCTED`, now on measured grounds rather
than on the absence of a measurement. The backlog item tracking this (`BACKLOG.md` OOP-1, "the
`delegate_isolation` F05 identity has not been re-gated") should be updated to record that the
question now has an answer; that file is outside this lane.

---

## CTRL-01 disposition

**Status: OPEN — reopened by this refresh.** It was `CLOSED for admission purposes` on 2026-07-26.
The schema condition is met again, but a row is reopened and its required companion action has not
been performed, so a closed status would be false.

Quoting the close condition: *"CTRL-01 remains open until every active row uses the declared
maturity enum and has a pinned peer baseline, security owner, exact evidence IDs, delta,
limitation, and refresh phase."*

| Close-condition clause | State |
|---|---|
| Every active row uses the declared maturity enum | **MET** — all 10 rows use `SOURCE`/`CONSTRUCTED`/`REACHED` from the declared enum. Zero `PENDING`. `ABSENT` is no longer used by any row. `REOPENED` on TXN-* annotates a valid enum value; it is not a substitute for one. |
| Pinned peer baseline | **MET, unchanged** — Hermes 0.17.0 @ `dbe734be`, OpenClaw 2026.6.2 @ `11a0ad10`. Not re-pinned by this refresh. Zero `UNPINNED`. |
| Security owner | **MET** — 8 rows `core`, 2 rows (`NATIVE-*`, `SUPPLY-*`) `shared`. Unchanged. |
| Exact evidence IDs | **MET** — every row cites resolvable IDs; 34 new IDs added, each naming an artifact verified present on disk at `42e1f2b2`. Zero `*-PENDING`. |
| Delta | **MET, unchanged and deliberately not re-derived** — every row still carries its `GAP-AUDIT §3` verdict and/or `PEER-PROBE-2026-07-26` finding at the pinned baseline. |
| Limitation | **MET** — every row states its own honest boundary; eight of the ten rows additionally flag a Delta clause as possibly stale **for F30 to adjudicate**. |
| Refresh phase | **MET** — all 10 rows refreshed to their own phases, 2026-07-28. No row still reads "Phase 20 + 20A close". |
| Bootstrap and map accepted F03/F05 evidence | **MET** — all 8 F05 identities carry a 2026-07-28 status; the one previously unresolved mapping is resolved as a negative. |

### Why it cannot close anyway

**Two things are missing, and neither is inside this lane.**

1. **TXN-\* is REOPENED and its companion action is unperformed.** The admission rule says
   contradictory live evidence "reopens the row **and enters `FIELD-REGRESSIONS.md`**". Two
   shipped-binary defects meet that description — the parallel-sibling journal-head CAS collision
   (F21-04-03) and the legacy `"effect_receipt":null` journal read failure (23B-H1). Both are
   repaired, but `FIELD-REGRESSIONS.md` currently holds five `FIELD-*` entries and neither of these
   is among them. **This lane owns only `COMPETITIVE-LEDGER.md` and did not edit that register.**
   Until those entries exist and the Windows re-proofs are taken, TXN-* is an open row and CTRL-01
   is open with it.
2. **The remaining carried limitation is unchanged and is Phase 30's by design.** Every delta in
   this ledger is still static-source, not runtime-measured. `GAP-AUDIT §3` and
   `PEER-PROBE-2026-07-26` compare source structure. **No correctness, recovery, cost or
   cognitive-tax number is claimed anywhere above, and none may be quoted from this ledger as a
   benchmark.** F30-03 owns that and has not run.

The 2026-07-26 disposition's claim that "no row is left open for want of an external input" is
**no longer true**, and that is recorded rather than quietly dropped. Three rows now name an input
reserved to Sean: REACH-* C1 needs a minted cloud credential; REACH-* C2 needs an authorized
second-host SSH trust relationship; GATEWAY-*'s macOS leg needs a CI-trigger change that is a PR
action. None of these blocks the ledger's schema; each blocks a specific promotion.

### Per-row disposition, 2026-07-28

| Row | Before | After | Disposition |
|---|---|---|---|
| AUTH-* | CONSTRUCTED | **CONSTRUCTED** | **Held.** Real enforcement landed and the carried negative is now measured rather than unknown. Held on Criterion 3 NOT MET, Linux-only evidence, and four dimensions holding by absence of a channel. |
| TXN-* | EFFECTIVE | **REACHED — REOPENED** | **Demoted and reopened.** The only demotion. Contradictory live evidence against the shipped binary; repairs re-proved Linux-only. `FIELD-REGRESSIONS.md` entries owed. |
| GOAL-* | SOURCE | **CONSTRUCTED** *(SUPERSEDED 2026-07-29 → REACHED, see the single-row refresh at the foot of this file)* | **Promoted one level.** Durable kernel, CLI projection, enforced fixed-loop bound and persistent scheduling exist. Capped below REACHED by two F05 identities still recorded `runtime path unwired`. |
| CONT-* | REACHED | **REACHED** | **Held.** Index sub-area materially stronger on three platforms; governed promotion and cache economics still unbuilt; multi-day journey has day one only. |
| GATEWAY-* | ABSENT | **CONSTRUCTED** | **Promoted three levels.** `ABSENT` was true and is now false. Capped below REACHED because two of three OS families have no gateway evidence at all. |
| REACH-* | SOURCE | **REACHED** | **Promoted.** Two-platform live product exercise and the only MET Success Criterion in phases 21-29. Three of four criteria still NOT MET. |
| PORT-* | SOURCE | **REACHED** | **Promoted.** Real-credential discovery against Sean's actual peer installs; backup/restore/rollback claimed with a real Windows product defect fixed. ~~The entire import half is unbuilt.~~ **Corrected 2026-07-30:** the import half IS built (26-02, 26-04, landed 2026-07-28) and was independently re-verified by `lane/port-import` — 19/19 hostile corpora run, quarantine mutation-tested in both directions, source non-mutation digest-proven. |
| MEDIA-* | SOURCE | **SOURCE** | **Held.** The phase ran and closed four of five criteria NOT MET. Readiness is still unpublished and still dishonest at HEAD. |
| NATIVE-* | SOURCE | **REACHED** | **Promoted.** Digest-bound packaged artifacts executed on all three families, 651/651 with zero skips. 24 cells still RED at the certified candidate; no soak; no receipt. |
| SUPPLY-* | SOURCE | **CONSTRUCTED** | **Promoted one level on the minority panel position.** Real execution against the real graph. Nothing bound to a real trust root; policy verdict RED; one HIGH open. |

Six rows moved up, one moved down, three held. **Every held row is held with a rewritten
justification**, because an unchanged level for a changed reason is a result, not an omission.

### Refresh obligations for the next admitted phase

- Deltas remain frozen at `BASE-2026-07-13`. `HEAD-2026-07-26` (Hermes 0.18.2 @ `d59b79fa`,
  OpenClaw 2026.7.2 @ `3659c85e`) is the declared forward target; moving the baseline requires
  re-deriving every delta cell, not editing the version strings in place. **This refresh did not
  move it and did not re-read either peer tree.**
- **Eight Delta clauses are flagged possibly-stale about Core and are left for F30 to adjudicate:**
  AUTH-* "operationally unproven"; GOAL-* "durable async agents: Core behind, especially OpenClaw";
  CONT-* "crash recovery: Core behind"; GATEWAY-* "Core has … no persistent gateway runtime";
  REACH-* "no user-facing execution-backend matrix"; PORT-* "Core is the only party with no
  reciprocal path"; NATIVE-* "no packaged native product target" (a *product* target is still
  absent; a per-target CI binary is now certified — the two are not the same claim); SUPPLY-*
  "it would be a lead if proven". **None was edited.**
- `REQ-native-r12` and `r13` remain open against `9821ef76` and, with the reopening above, bound
  TXN-* well below `PACKAGED_PROVEN`.
- **Two registers outside this lane are stale against what has landed and should be reconciled by
  their owners:** `ROADMAP.md`'s progress table still reads "Not started" for phases 21, 22, 23,
  24, 25, 26, 28 and 29; `REQUIREMENTS.md`'s Phase 24 disposition still says only 24-01 executed
  (24-02, 24-03, 24-04, 24-B and 24-C have since landed) and its Phase 23B block still records
  23B-02/03/04 as not started (all three have summaries on disk). This ledger cites the phase
  SUMMARY and VERDICT artifacts directly and is not derived from either register.
- Any contradictory live evidence reopens the affected row and enters `FIELD-REGRESSIONS.md`.

*CTRL-01 refreshed 2026-07-28 at base `42e1f2b2383597baa0856b212f66916117d1290a`, against phases
21, 22, 23A, 23B, 24, 25, 26, 27, 28 (plans 01-02) and 29 (plans 01-02), read from their own
SUMMARY and VERDICT artifacts. Peer baselines were not re-read, not re-pinned, and no peer
comparison was authored — F30 owns the first comparison and has not run.*

---

# SINGLE-ROW REFRESH — 2026-07-29, lane `lane/22-remaining`

**Scope: `GOAL-*` only.** No peer tree was re-read, no baseline was re-pinned, no other row was
re-graded. Every other row and every Delta stands exactly as at the 2026-07-28 refresh. Two
`CONT-*` F05 identities are flagged stale in the amendment above and deliberately **not** edited —
re-grading them belongs to the row's owner.

## Disposition

| Row | Before | After | Basis |
|---|---|---|---|
| `GOAL-*` | CONSTRUCTED (2026-07-28) | **REACHED** | The 2026-07-28 refresh wrote down its own deciding test against this row. Both of its conjuncts now hold, and one of the two blockers was never true. Panel 3-0 with an internal adversarial dissent recorded in the row. |

## New evidence IDs

**`F22-MIDFLIGHT-STALE@5457710e`** —
`.planning/phases/22-supervision-durable-goals-fleet-loops/22-REMAINING-EVIDENCE/midflight/`
(`RESULT.md`, `wl22r/stream.jsonl`, `wl22r-neg2/stream.jsonl`, both `canned-requests.log`).
`wayland-core 0.12.25` release built on `hetzner-dsm` at `5457710e`, driven `--json-stream`
against a local canned OpenAI-compatible endpoint (no credential; the provider identity is read
back from the product's own log line and the endpoint's request log, per LANE-BRIEF §3b-ii).
Positive: 27 `capability_activation` events including the full mid-flight chain through
`observed`, plus one `mid_flight_monitor_decision`. One-variable negative control at
`CANNED_TOOL_TURNS=3`: 2 tool errors, **0** decisions, **0** occurrences, `ready` unchanged.
Both arms `PRODUCT_RC=0`, so exit status distinguishes nothing.

**`F22-LEARNEDPOLICY-WIRED@c5ca677c`** — same directory, `learnedpolicy/`. Release binary built
from `lane/22-remaining`. One run, one on-disk `permissions.toml` (`Read` = `deny-always`), two
`Read` calls differing only in caller class, read back from the product's own conversation state
(a `Delegate` child's tool results never reach the parent stream):
`last_tool_result[parent:2] = "     1\tparent probe content"` versus
`last_tool_result[child:2]  = "Denied by sub-agent learned policy: Read matched rule `*`"`.
Control arm with no policy file: the child reads it, and the capability reports
`disabled_by_config` instead of `ready`. Compile-time falsification: `actor_acl_test`
**8 run / 8 passed / 0 skipped** (at base: 1 of 6, five `#[ignore]`d); severing only the
pre-filter's input takes `sub_agent_with_deny_policy_short_circuits` **RED at rc=100** and
nothing else.

**`F22-C3-FIVE-ENGINES`** — `22-C3-SUMMARY.md` (lane `lane/22-c3-goal`). All five engines given a
production path to one canonical Goal transition, live against the shipped release; root cause was
`goal open` hard-coding `GoalStrategy::Fleet`. Graded PARTIAL by its own lane: opt-in attachment,
zero engine signatures changed, Linux only.

**`F22-VERDICT-2026-07-29`** —
`.planning/phases/22-supervision-durable-goals-fleet-loops/22-PHASE-VERDICT.md`, the
`UPDATE — 2026-07-29` section. Supersedes both earlier gradings. Phase goal **NOT ACHIEVED** for
the third time.

**`GOAL-PANEL-2026-07-29`** — cross-audit panel on the promotion, `22-REMAINING-EVIDENCE/panel/`.
`codex` gpt-5.6-sol, `gemini` 3.1-pro-preview, `kimi` K3, each asked the same question with the
ledger's own recorded blockers quoted verbatim: **3-0 REACHED**, no abstentions, each vote
extracted unanchored from its own capture. Plus an internal adversarial pass arguing to hold at
CONSTRUCTED, whose surviving case is recorded inside the row rather than discarded.

## What this refresh does NOT claim

- **Not EFFECTIVE.** Phase 22's goal is NOT ACHIEVED; its Criterion 1 is NOT MET on the
  *control* half for two of three surfaces; C3, C4 and C5 are all PARTIAL.
- **No CTRL-01 movement.** CTRL-01 stays **OPEN**; this refresh does not touch its close
  conditions, and the `FIELD-REGRESSIONS.md` entries `TXN-*` owes are still owed by that row.
- **No Delta re-derivation.** The `GOAL-*` Delta text is byte-unchanged and still bound to
  `BASE-2026-07-13`; the open question it carries for F30 is carried forward, not answered.
