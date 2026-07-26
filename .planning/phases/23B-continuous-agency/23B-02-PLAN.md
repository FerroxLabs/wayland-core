---
phase: 23B-continuous-agency
plan: "02"
type: execute
wave: 2
depends_on:
  - "23B-01"
files_modified:
  - crates/wcore-memory/src/provenance.rs
  - crates/wcore-memory/src/lib.rs
  - crates/wcore-memory/src/staleness.rs
  - crates/wcore-user-model/src/lib.rs
  - crates/wcore-agent/src/slash/memory.rs
  - crates/wcore-agent/src/cache_diagnostics.rs
  - crates/wcore-agent/src/compact/state.rs
  - crates/wcore-cli/src/tui/commands/mod.rs
  - crates/wcore-agent/tests/context_economics_test.rs
  - crates/wcore-cli/tests/memory_control_lifecycle.rs
  - scripts/f23-context-economics-drive.sh
  - scripts/f23-context-economics-drive.ps1
  - .planning/phases/23B-continuous-agency/evidence/
  - .planning/phases/23B-continuous-agency/23B-02-LIVE-EVIDENCE.md
autonomous: true
requirements:
  - F23-03
  - F23-04
domain: code
must_haves:
  truths:
    - "SUCCESS CRITERIA 3 AND 4 ARE ONE SURFACE FROM THE USER'S SEAT, WHICH IS WHY THEY SHARE A PLAN. Both answer the same question a user actually asks: what went into my prompt, why, and what did it cost. Memory activation and recall provenance decide WHAT is in the context window; cache hit and invalidation reasons, token-pressure state and compaction quality decide WHAT THAT COSTS. They read the same session, are inspected from the same TUI, and share the same acceptance mechanism — reading the real outbound request body and comparing it against what the product claimed. Splitting them would duplicate that mechanism twice and prove it neither time."
    - "THE ONLY HONEST PROOF THAT A FACT WAS FORGOTTEN IS THAT IT IS ABSENT FROM THE NEXT PROMPT. A test that asserts a database row was deleted proves a row was deleted. The user's claim is that the model no longer sees it. `crates/wcore-cli/tests/support/mock_llm.rs` already exposes `RecordedRequest` and `received_requests`, which read the actual POST body the real provider sent. Every forgetting, privacy and retention assertion in this plan is made against that recorded body, not against internal state. The same mechanism proves activation truth: what the product SAYS it recalled must equal what actually reached the provider."
    - "THE SUBSTRATE IS ALREADY LARGE AND MOSTLY BUILT — THIS PLAN EXPOSES AND CONTROLS IT, IT DOES NOT REBUILD IT. `wcore-memory` already ships a SQLite-backed five-partition by three-tier store with a deny-by-default `MemoryAccessGate`, an append-only audit log in its own database, a staleness module, a contradiction resolver, a CDC changelog, consolidation, and a hybrid retriever fusing FTS5 BM25 with a vector pass by reciprocal rank fusion. `wcore-user-model` already ships expertise and preference learning. `wcore-agent/src/cache_diagnostics.rs` already computes hit rate and names a break cause across SystemPromptChanged, ToolsChanged, TtlExpiry and FirstRequest, with a warm-session warn ratio and a warm-after-round-trips threshold. `wcore-compact` already ships folding, sanitization, semantic compaction, TOON encoding and level policy, and `wcore-agent/src/compact/` ships auto, degrade, emergency, estimate and micro. What is missing is provenance on recall, operator control over forgetting and privacy, and a surface where a human can see any of it."
    - "PROACTIVE NUDGES ARE BOUNDED OR THEY ARE NOT SHIPPED. F23-03 says bounded proactive nudges. An unbounded nudge path is a background actor that spends tokens and money without a turn the user asked for. The bound must be explicit — a rate, a per-session cap, and an off switch that is honoured — and the bound must be proved by driving past it and observing refusal, not by reading the constant."
    - "COST TRUTH MEANS THE NUMBER MATCHES THE PROVIDER'S NUMBER. F23-04 requires cost-regression thresholds. A threshold over a self-reported estimate regresses against itself and detects nothing. The reported spend must be reconciled against the token counts the provider actually returned in the recorded response, and the reconciliation delta must be recorded."
    - "A REPORTED RED IS WORTH FAR MORE THAN AN ENGINEERED GREEN. Never weaken an assertion, add an ignore or allow attribute, raise a timeout, re-gate, or delete an inconvenient test to reach a gate. If memory forgetting cannot be proved absent from the outbound body, that is a RED to report, not a test to soften."
    - "A GATE THAT CANNOT GO RED IS WORSE THAN NO GATE, AND THIS PLAN ALREADY SHIPPED ONE. The previous revision closed the Windows leg with `ssh host '...' | grep -v CLIXML | grep -v '^<Objs'`. A pipeline's exit status is the LAST command's, so that reported grep's status, not ssh's: any surviving output line greened the gate even when the remote build failed, and grep's exit 1 on empty output meant it reddened on silent success. Two further instances of the same class are closed here — `cargo clippy` passing on a host tree that does not contain this plan's new modules, and a `git diff --stat FETCH_HEAD` gate.rs check that depended on a FETCH_HEAD written by an earlier, separate ssh session. For every command written into a `<verify>` block, answer 'what makes this go red?' before writing it. If the honest answer is 'nothing' or 'only if output is empty', it is not a verification."
    - "THE macOS LEG HAD NO BINARY AND NO EXECUTABLE STEP, AND THE ARTIFACT IT NAMED DOES NOT EXIST. The previous revision drove macOS against 'a prebuilt artifact from the macOS CI job'. Measured against `.github/workflows/`: `ci.yml` uploads only `nextest-junit-${{ matrix.os }}` JUnit XML and no binary of any kind, and `release.yml` builds Darwin binaries only on a `v*-wayland-*` tag push or an explicit dispatch — both Sean-only, as is pushing. No such artifact is reachable from inside this phase, and not one automated gate command executed anything on macOS: the macOS rows were closed by grepping an evidence file the executor itself wrote, which is a tautology. The macOS leg now runs the real driver locally against the binary `scripts/f23-macos-binary.sh` resolves, and every leg's binary must prove its own provenance through `--build-info`."
  artifacts:
    - path: crates/wcore-memory/src/provenance.rs
      provides: "Recall provenance — for each item placed in the context window, which partition and tier it came from, which retrieval modality selected it, its rank and score, its age and staleness, and the correction and forget operations over those records"
    - path: crates/wcore-agent/src/slash/memory.rs
      provides: "The operator control surface: activation truth, provenance display, correct, forget, privacy, retention and nudge controls, replacing the current show and clear pair"
    - path: crates/wcore-agent/src/cache_diagnostics.rs
      provides: "Operator-visible cache hit and invalidation reasons plus token-pressure state, extended from the existing break-cause detection"
    - path: crates/wcore-agent/tests/context_economics_test.rs
      provides: "The recorded-outbound-body coverage proving activation truth, forgetting, privacy, retention, cache invalidation cause, compaction quality and cost reconciliation"
    - path: scripts/f23-context-economics-drive.sh
      provides: "The live driver exercising memory control and context economics against the shipped binary on every platform"
    - path: .planning/phases/23B-continuous-agency/23B-02-LIVE-EVIDENCE.md
      provides: "The recorded live outcome per control per platform, including the measured cost-reconciliation delta and the nudge bound driven past its limit"
  key_links:
    - from: crates/wcore-agent/src/slash/memory.rs
      to: crates/wcore-memory/src/provenance.rs
      via: "the runtime handler variant carrying a live memory API, already the established pattern in this file"
      pattern: "surface-to-store"
    - from: crates/wcore-agent/tests/context_economics_test.rs
      to: crates/wcore-cli/tests/support/mock_llm.rs
      via: "the recorded-request helper reading the actual outbound POST body"
      pattern: "observable-truth"
    - from: crates/wcore-agent/src/cache_diagnostics.rs
      to: crates/wcore-cli/src/tui/commands/mod.rs
      via: "the /cost and /compact handlers rendering hit rate, invalidation cause, token pressure and reconciled spend"
      pattern: "diagnostics-to-tui"
---

<objective>
Make Success Criteria 3 and 4 true through the shipped product: a user can see and control what memory and user-model state activates, where each recalled item came from, and how to correct, forget, privacy-scope and retention-bound it; and can see honest cache hit and invalidation reasons, token-pressure state, compaction quality and reconciled cost.

Purpose: F23-03 and F23-04 are the two halves of context economics — what enters the prompt and what that costs. The substrate is largely built (a five-by-three memory grid with a deny-by-default access gate and audit log, a hybrid FTS5-plus-vector retriever, a cache-break-cause detector, and a full compaction stack). What is missing is provenance, operator control, and any surface where a human can see it. Today `/memory` offers only show and clear, and its default handler is a back-compatibility stub that returns placeholder strings.
Output: Recall provenance records and the correction, forgetting, privacy and retention operations over them; a real `/memory` control surface; operator-visible cache and compaction truth through `/cost` and `/compact`; coverage that asserts against the actual outbound provider request body; and captured live evidence per platform.
</objective>

<execution_context>
@$HOME/.codex/gsd-core/workflows/execute-plan.md
@$HOME/.codex/gsd-core/templates/summary.md
</execution_context>

<context>
@AGENTS.md
@.planning/HANDOFF-2026-07-26-phase20-20A-complete.md
@crates/wcore-agent/src/slash/memory.rs
@crates/wcore-agent/src/cache_diagnostics.rs
@crates/wcore-memory/src/retrieve.rs
@crates/wcore-memory/src/gate.rs
@crates/wcore-memory/src/audit.rs
@crates/wcore-cli/tests/support/mock_llm.rs
</context>

<execution_rules>

**THE TWO AMENDED PHASE RULES — verbatim, and they bound this plan.**

- Findings at CRITICAL or HIGH must be fixed or disproved. MEDIUM and below are logged to BACKLOG and DO NOT BLOCK execution.
- Execution begins when no CRITICAL or HIGH finding is open, or after 2 review rounds, whichever comes first. A third round is NOT permitted; it escalates to Sean.

**TERMINATION CRITERION FOR THIS PLAN (hard).** This plan exposes and controls two existing subsystems once and proves them once per platform. It terminates in exactly one of three states, and in all three it writes its SUMMARY and stops:
1. **Complete** — every F23-03 control and every F23-04 truth is driven against the shipped binary with its observable outcome captured, and the cost reconciliation delta is recorded.
2. **Complete with named open controls** — one or more controls could not be closed honestly. Record each as OPEN with its blocking evidence, mark the affected requirement incomplete, and stop.
3. **Escalated** — a CRITICAL or HIGH finding requires a change outside this plan's declared files. Record it with severity and stop.
This plan does NOT create additional plans and does NOT extend its own task list.

**SCOPE BOUNDARY (hard).** Session operator verbs belong to 23B-01 and are an admitted input here. The repository index belongs to 23B-03 — memory retrieval and repository retrieval are different subsystems and this plan touches neither `wcore-repomap` nor the index CLI. The multi-day journey and terminal acceptance belong to 23B-04. Governed skill promotion is Phase 23A's and is an admitted input. Note that `wcore-memory` carries a procedure lifecycle whose promote and archive operations are surfaced today through root CLI flags; that lifecycle is 23A's contract and this plan must not modify it.

**DO NOT WIDEN THE MEMORY ACCESS GATE.** `gate.rs` is deny-by-default across the five-by-three grid: a system token has full access, the main-agent token has partition one through four read and write within valid tier cells and is DENIED partition five because the user model is system-only, and sub-agent tokens are deny-by-default with an explicit policy enumerating scopes. Every new operation in this plan runs THROUGH that gate with an appropriate token. A new operation that bypasses the gate, or that needs the grid widened to work, is a CRITICAL finding and stops the plan.

**THE `tui/commands/mod.rs` SEAM.** This plan and 23B-03 both need the TUI command registry. That is why they are consecutive waves. This plan's registry edit covers only the memory, cost and compact entries; it must not touch the repomap entry.

**NON-NEGOTIABLE.** A REPORTED RED IS WORTH FAR MORE THAN AN ENGINEERED GREEN. The specific temptations here are to prove forgetting by asserting a deleted row instead of an absent prompt fragment, to prove cost truth against the product's own estimate instead of the provider's returned counts, and to prove a nudge is bounded by reading the constant instead of driving past it. All three are engineered greens and all three are forbidden.

**ENVIRONMENT.**
- Linux (authoritative Cargo proof): `ssh -o BatchMode=yes hetzner-dsm`, `/root/wayland`.
- Windows (native live): `ssh -o BatchMode=yes SeanD@seandesktop`, checkout `C:\ferrox-win`, cargo at `C:\Users\seand\.cargo\bin\cargo.exe`. The remote default shell is PowerShell, so an `ssh` command string is PowerShell source and must end with an explicit `exit $LASTEXITCODE` for the status to propagate. `cargo fmt --all` fails there with os error 206. Windows clippy runs with warnings denied BEFORE tests.
- macOS (native live): THIS Mac. See the macOS binary decision below. `cargo fmt --all -- --check` is the local formatting gate.
- ALWAYS `/usr/bin/grep` on the Mac with `-F` for literals — the ambient grep silently drops lines.

**GATE DISCIPLINE — every command in a `<verify>` block must be able to go RED. Three hard rules, each closing a defect this plan actually shipped.**

1. **A gate is NEVER a pipeline into a filter.** `ssh host 'cmd' | grep -v CLIXML | grep -v "^<Objs"` reports GREP's exit status, not ssh's. Any surviving output line greens it even when the remote build failed, and grep's exit 1 on EMPTY output means it reddens on silent success. The Windows gate in the previous revision had exactly that shape and could not detect failure. The correct form redirects, captures the status on the NEXT line, asserts on it, and only then reads the log:
   `ssh -o BatchMode=yes HOST "…; exit \$LASTEXITCODE" > LOG 2>&1; rc=$?; test "$rc" -eq 0 && /usr/bin/grep -qF "MARKER" LOG`
   Filtering CLIXML noise while READING a log for a human is fine; it is fatal only when the pipeline IS the gate.
2. **Never read an exit code from a block that also emits output.** In PowerShell, `$x = & { cargo … | Tee-Object …; $LASTEXITCODE }` returns an ARRAY of every output line plus the code, so `if ($x -ne 0)` is an always-truthy array filter. That bug made an all-PASS 12/12 + 6/6 Windows soak report failure; the fix and its post-mortem are in `scripts/wayland-e2e-windows-soak.ps1:174-190` and `:244-255`. Read `$LASTEXITCODE` on the line AFTER the pipeline, and always end a driver with an explicit `exit`.
3. **Never let a gate pass on a tree that does not contain the work, and never depend on state an earlier ssh session left behind.** `cargo clippy --workspace -- -D warnings` on a host synced to the last PUSHED tip is clean and proves nothing about modules that do not exist there yet. Every remote gate therefore pins the exact commit under test and asserts a file THIS PLAN CREATES is present, in the same `&&` chain, before the compiler runs: take `SHA=$(/usr/bin/git rev-parse HEAD)` locally, `git checkout -q --detach $SHA` on the host, then `test -f <file this plan creates> && cargo …`. The previous revision's zero-diff check on `gate.rs` compared against `FETCH_HEAD` — a ref written by a DIFFERENT ssh session, so it silently compared against whatever that host last fetched; it now compares against an explicitly recorded base commit. **Commit this task's declared files and get the working branch onto `gh` BEFORE running a remote gate.** Do not respond to a missing SHA by dropping the assertion.

**macOS BINARY SOURCE — DECIDED IN 23B-01 AND CARRIED HERE, WITH ITS BASIS AND ITS MEASUREMENTS.** The previous revision drove the macOS leg against "a prebuilt artifact from the macOS CI job". That artifact does not exist and cannot be produced from inside this phase. Measured, not assumed: `.github/workflows/ci.yml:204-208` uploads only `nextest-junit-${{ matrix.os }}` — JUnit XML, no binary of any kind, on any branch; `.github/workflows/release.yml:1-24` fires only on a `v*-wayland-*` tag push, a `workflow_call`, or an explicit `workflow_dispatch`, and its Darwin targets at `:70-74` therefore never build for `plan/f20-unified-audit-repair`. Tagging, releasing, dispatching and pushing are all Sean-only, so no CI run producing a macOS binary can be triggered from inside plan execution. **Decision: the macOS leg builds its own binary on this Mac, through `scripts/f23-macos-binary.sh`, which 23B-01 owns and this plan consumes unchanged.** Basis: HANDOFF §3 item 7 — "This Mac CAN compile the workspace. The old 'never compiles on Mac' note is a workflow convention, not a fact" — plus the pinned toolchain `1.95.0-aarch64-apple-darwin` present under `~/.rustup/toolchains` and matching `rust-toolchain.toml`. **The convention's real purpose is preserved exactly: `hetzner-dsm` stays the sole authority for clippy, nextest and the aggregate proof. The Mac build produces a DRIVE TARGET, never a proof verdict, and is isolated in `--target-dir target/f23-macos`, which the existing `/target/` ignore rule already covers.** `WAYLAND_F23_MACOS_BIN` overrides with a binary built elsewhere; either way the resolver asserts the binary's own `--build-info` source SHA equals the commit under test, so a stale artifact reddens instead of silently proving the wrong code. If the Mac build fails, that is a RED to record: the macOS rows go OPEN with the compiler's exact error under this plan's termination state 2. It is never a silent skip. If `scripts/f23-macos-binary.sh` is absent because 23B-01 did not land it, STOP and record that as a blocking dependency rather than improvising a second resolver.
- Always `git fetch origin plan/f20-unified-audit-repair` explicitly; both hosts' refspecs are pinned to an unrelated branch. In the Mac repo `origin` is a stale local worktree; the real remote is `gh`.
- NO push to main, merge, PR, tag, release, deployment or issue closure.

**AGENTS.md discipline.** Surgical diffs. No hardcoded provider quirks — provider differences go through `ProviderCompat`, never a conditional on a base URL. No duplicate code across crates: recall provenance belongs in `wcore-memory` (the lowest crate where it semantically belongs), not copied into `wcore-agent`. Errors: thiserror for the public memory API, anyhow for internal propagation. Clippy-clean with warnings denied. Keep new modules under 1000 lines.

**Git hygiene.** `/usr/bin/git` on the Mac. Stage the exact paths in `files_modified`, never `-A`, never `.`. No `Co-Authored-By` trailers.
</execution_rules>

<tasks>

<task type="auto" tdd="true">
  <name>Task 1: Recall provenance and the memory control operations, through the existing access gate</name>
  <files>crates/wcore-memory/src/provenance.rs, crates/wcore-memory/src/lib.rs, crates/wcore-memory/src/staleness.rs, crates/wcore-user-model/src/lib.rs, .planning/phases/23B-continuous-agency/evidence/23B-02-base-sha.txt</files>
  <read_first>crates/wcore-memory/src/retrieve.rs (`search_basic`: the FTS5 BM25 pass joined against the episodes table, the vector pass preferring the dimension-aware KNN path with an O(n) cosine fallback for rows written earlier, the reciprocal-rank fusion, the session-diversity cap and the per-modality limit — every one of these is a provenance fact that is currently computed and then discarded), crates/wcore-memory/src/gate.rs (the deny-by-default grid, the three token kinds, and the per-agent scope policy — every new operation runs through this), crates/wcore-memory/src/audit.rs (the append-only audit log schema in its own database, and the note that gate denials are the primary write rate — forgetting and correction are audit-worthy events of the same kind), crates/wcore-memory/src/staleness.rs (what staleness already means here before adding an age dimension), crates/wcore-memory/src/v2_types.rs (Hit, Query, Partition, Tier, AccessToken), crates/wcore-memory/src/cdc.rs (the changelog — a forget must be represented here, not only as a row deletion), crates/wcore-user-model/src/expertise.rs and preference_learner.rs (what the user model infers and therefore what a user must be able to correct)</read_first>
  <behavior>
    - Every item a retrieval places in the context window carries a provenance record naming its partition, tier, the retrieval modality that selected it (lexical, vector, graph or fused), its rank and fused score, its age and its staleness verdict.
    - Provenance is produced by the same retrieval that returns the hits, so it cannot describe a different selection than the one that happened.
    - A correction to a recalled item updates the stored item and records the correction in the audit log with the operator as actor.
    - Forgetting an item removes it from every subsequent retrieval AND is represented in the change-data-capture changelog, so a downstream consumer sees a deletion rather than silently missing a row.
    - A privacy scope applied to a partition or tier causes retrieval to exclude it, and the exclusion is reported rather than silent.
    - A retention bound marks items past their retain-until as expired; expired items are excluded from retrieval and reported as expired rather than deleted without trace.
    - Every operation is refused for a token whose grid cell does not permit it, the refusal is written to the audit log, and no operation widens the grid.
    - The user model exposes what it has inferred about the user in a form a human can read, and a user correction to an inferred expertise or preference persists and overrides later inference rather than being re-learned away on the next turn.
    - Proactive nudges are bounded by an explicit per-session cap and an off switch; a request past the cap is refused and the refusal is observable.
  </behavior>
  <action>BEFORE writing a single line of code, record the base commit this plan starts from: `mkdir -p .planning/phases/23B-continuous-agency/evidence` then write `git rev-parse HEAD` into `.planning/phases/23B-continuous-agency/evidence/23B-02-base-sha.txt` and commit that file first. The zero-diff proof for `gate.rs` compares against THAT commit. The previous revision compared against `FETCH_HEAD`, a ref written by a different ssh session, so it silently compared against whatever the host happened to have fetched last — the check could pass while `gate.rs` had in fact been widened.

Create `crates/wcore-memory/src/provenance.rs` and declare it in `lib.rs`. Do not modify the retrieval algorithm — attach provenance to what `search_basic` already computes. The modality, rank and fused score exist inside that function today and are discarded; capture them at the point of fusion so the record cannot diverge from the selection.

Write tests first, one per behavior bullet, against a real temporary memory database built through the existing open path so the fixtures exercise the actual schema. Confirm each fails for the right reason before implementing.

Route every new operation through `MemoryAccessGate` with an appropriate token, and write correction, forget, privacy and retention events to the existing audit log — those are the same class of event the log was built for. Represent a forget in the change-data-capture changelog as well as in the store, so a consumer sees a deletion rather than a row that quietly vanished. Extend `staleness.rs` with the age dimension provenance needs rather than creating a second staleness notion.

In `wcore-user-model`, expose the inferred expertise and preference state for human reading and add a user-correction path that persists and takes precedence over subsequent inference. Verify the precedence by running inference again after a correction and asserting the correction survives.

Implement the nudge bound as an explicit per-session cap plus an off switch in configuration, and prove it by requesting past the cap and observing refusal. Do not implement the nudge delivery path itself — F23-03 requires the bound; delivery scheduling is Phase 24's persistent runtime and is explicitly out of scope here.

Addresses F23-03; marks no requirement complete.</action>
  <verify>
    <automated>cd /Users/seandonahoe/dev/waylandcore-ferrox &amp;&amp; cargo fmt --all -- --check</automated>
    <automated>cd /Users/seandonahoe/dev/waylandcore-ferrox &amp;&amp; SHA=$(/usr/bin/git rev-parse HEAD) &amp;&amp; ssh -o BatchMode=yes hetzner-dsm "cd /root/wayland &amp;&amp; git fetch -q origin plan/f20-unified-audit-repair &amp;&amp; git checkout -q --detach $SHA &amp;&amp; test -f crates/wcore-memory/src/provenance.rs &amp;&amp; cargo clippy -p wcore-memory -p wcore-user-model --all-targets -- -D warnings"</automated>
    <automated>cd /Users/seandonahoe/dev/waylandcore-ferrox &amp;&amp; SHA=$(/usr/bin/git rev-parse HEAD) &amp;&amp; ssh -o BatchMode=yes hetzner-dsm "cd /root/wayland &amp;&amp; git checkout -q --detach $SHA &amp;&amp; cargo nextest run -p wcore-memory -p wcore-user-model --profile ci --no-tests=fail --no-fail-fast"</automated>
    <automated>cd /Users/seandonahoe/dev/waylandcore-ferrox &amp;&amp; SHA=$(/usr/bin/git rev-parse HEAD) &amp;&amp; BASE=$(cat .planning/phases/23B-continuous-agency/evidence/23B-02-base-sha.txt) &amp;&amp; test -n "$BASE" &amp;&amp; ssh -o BatchMode=yes hetzner-dsm "cd /root/wayland &amp;&amp; git checkout -q --detach $SHA &amp;&amp; git cat-file -e $BASE &amp;&amp; git diff --quiet $BASE $SHA -- crates/wcore-memory/src/gate.rs"</automated>
  </verify>
  <done>`provenance.rs` exists and is declared; every recalled item carries partition, tier, modality, rank, fused score, age and staleness. Correction, forget, privacy and retention run through the unmodified access gate and are audited; `gate.rs` shows a zero diff against the base commit recorded in `evidence/23B-02-base-sha.txt`. Forgets appear in the change-data-capture changelog. The user model exposes inferred state and honours a user correction across a re-inference. The nudge cap refuses past its limit. Every pre-existing `wcore-memory` test still passes unchanged.</done>
</task>
<task type="auto" tdd="true">
  <name>Task 2: Cache, compaction and cost truth, asserted against the real outbound request body</name>
  <files>crates/wcore-agent/src/cache_diagnostics.rs, crates/wcore-agent/src/compact/state.rs, crates/wcore-agent/src/slash/memory.rs, crates/wcore-cli/src/tui/commands/mod.rs, crates/wcore-agent/tests/context_economics_test.rs</files>
  <read_first>crates/wcore-agent/src/cache_diagnostics.rs in full (the prompt snapshot pairing request-side hashes with response-side cache tokens, the healthy, partial-miss and full-miss diagnostic shapes, the four break causes, and the warn-ratio and warm-after-round-trips constants — this is the existing engine for F23-04's invalidation reasons), crates/wcore-agent/src/compact/estimate.rs and state.rs (how token pressure is currently estimated and held), crates/wcore-agent/src/compact/auto.rs and degrade.rs and emergency.rs (the escalating compaction ladder — the quality gate must be expressed over these, not beside them), crates/wcore-compact/src/level.rs and fold.rs and sanitize.rs (what a compaction level actually does and what it promises to preserve), crates/wcore-agent/src/slash/memory.rs in full (the Stub and Runtime variants and the note that the CLI construction path swaps the stub for the runtime one right after engine bootstrap — the new controls belong on the Runtime variant and the Stub must remain back-compatible or every existing test breaks), crates/wcore-cli/src/tui/commands/mod.rs (the registry entries for /memory, /cost and /compact and their one-line help idiom), crates/wcore-cli/tests/support/mock_llm.rs (RecordedRequest and received_requests — how a test reads the actual POST body, and note that received_requests is async so a synchronous test must block on it)</read_first>
  <behavior>
    - What the product reports as recalled into the context window equals what actually appears in the outbound provider request body, asserted by reading the recorded request rather than internal state.
    - Forgetting an item and taking one more turn produces an outbound body that does not contain that item, asserted against the recorded body.
    - A privacy scope excluding a partition produces an outbound body containing nothing from that partition, and the exclusion is reported to the user.
    - After a deliberate mid-session system-prompt change, the reported cache diagnostic names the system-prompt cause; after a deliberate tool-set change it names the tools cause; and a first request names the first-request cause.
    - Token-pressure state is reported with the numbers it was computed from, and crossing the auto-compaction threshold reports the transition rather than compacting silently.
    - Compaction reports a quality verdict naming what was preserved and what was folded, and a compaction that would drop content the level promises to preserve is refused rather than performed.
    - Reported spend for a turn is reconciled against the token counts the provider actually returned in the recorded response, and the reconciliation delta is reported.
    - A cost regression past a configured threshold is reported as a regression with the baseline it was compared against.
    - `/memory`, `/cost` and `/compact` render all of the above, and the pre-existing Stub variant of the memory handler keeps its current back-compatible strings so no existing test changes.
  </behavior>
  <action>Extend `cache_diagnostics.rs` to carry the invalidation reason and the token counts it was derived from into an operator-facing shape, rather than only into a telemetry event. Do not change the break-cause detection logic — it already distinguishes the four causes correctly; this is an exposure change.

Extend `compact/state.rs` to report token-pressure state with its inputs, and to report the transition when the auto-compaction threshold is crossed. Express the compaction quality verdict over the existing ladder in `auto.rs`, `degrade.rs` and `emergency.rs` and the level policy in `wcore-compact` — a compaction that would drop what its level promises to preserve is refused. Do not add a second compaction path beside the existing ladder.

Add the cost reconciliation: compare the reported spend for a turn against the token counts the provider actually returned and report the delta. Route pricing through the existing pricing crate; do not hardcode a provider's rates and do not branch on a provider base URL — provider differences belong in the compat layer.

Rework `/memory` on its Runtime variant into the real control surface: activation truth, provenance display, correct, forget, privacy, retention and the nudge switch. Leave the Stub variant's strings exactly as they are so the pre-v0.8.0 back-compatibility contract and every test resting on it survive. Update the `/memory`, `/cost` and `/compact` registry entries and their one-line help to match the new capability; do not touch the repomap entry, which belongs to 23B-03.

Write `crates/wcore-agent/tests/context_economics_test.rs` against the mock provider server, asserting every behavior bullet by reading the recorded outbound request body. For the forgetting and privacy cases, plant a distinctive run-time-generated value into memory, confirm it appears in the recorded body on one turn, apply the control, and confirm it is absent from the next recorded body. That before-and-after pair is the acceptance; an absence alone proves nothing because the value might never have been recalled.

Addresses F23-03 and F23-04; marks no requirement complete.</action>
  <verify>
    <automated>cd /Users/seandonahoe/dev/waylandcore-ferrox &amp;&amp; cargo fmt --all -- --check</automated>
    <automated>cd /Users/seandonahoe/dev/waylandcore-ferrox &amp;&amp; SHA=$(/usr/bin/git rev-parse HEAD) &amp;&amp; ssh -o BatchMode=yes hetzner-dsm "cd /root/wayland &amp;&amp; git fetch -q origin plan/f20-unified-audit-repair &amp;&amp; git checkout -q --detach $SHA &amp;&amp; test -f crates/wcore-agent/tests/context_economics_test.rs &amp;&amp; cargo clippy --workspace --all-targets -- -D warnings"</automated>
    <automated>cd /Users/seandonahoe/dev/waylandcore-ferrox &amp;&amp; SHA=$(/usr/bin/git rev-parse HEAD) &amp;&amp; ssh -o BatchMode=yes hetzner-dsm "cd /root/wayland &amp;&amp; git checkout -q --detach $SHA &amp;&amp; cargo nextest run -p wcore-agent --profile ci --test context_economics_test --no-tests=fail --no-fail-fast"</automated>
    <automated>cd /Users/seandonahoe/dev/waylandcore-ferrox &amp;&amp; SHA=$(/usr/bin/git rev-parse HEAD) &amp;&amp; ssh -o BatchMode=yes hetzner-dsm "cd /root/wayland &amp;&amp; git checkout -q --detach $SHA &amp;&amp; cargo nextest run -p wcore-agent -p wcore-compact -p wcore-cli --profile ci --no-tests=fail --no-fail-fast"</automated>
    <automated>cd /Users/seandonahoe/dev/waylandcore-ferrox &amp;&amp; test "$(/usr/bin/grep -v '^ *//' crates/wcore-agent/tests/context_economics_test.rs | /usr/bin/grep -cF 'received_requests')" -ge 6</automated>
  </verify>
  <done>Cache invalidation cause, token-pressure state, compaction quality verdict and reconciled spend are all operator-visible through `/cost` and `/compact`. `/memory`'s Runtime variant carries the full control set and its Stub variant is byte-unchanged. `context_economics_test.rs` asserts at least six behaviors against the actual recorded outbound request body, including a before-and-after pair for forgetting and for privacy. Workspace clippy clean with warnings denied; every pre-existing test in `wcore-agent`, `wcore-compact` and `wcore-cli` still passes.</done>
</task>

<task type="auto">
  <name>Task 3: LIVE — drive memory control and context economics through the shipped binary on Linux, macOS and Windows</name>
  <files>scripts/f23-context-economics-drive.sh, scripts/f23-context-economics-drive.ps1, crates/wcore-cli/tests/memory_control_lifecycle.rs, .planning/phases/23B-continuous-agency/evidence/, .planning/phases/23B-continuous-agency/23B-02-LIVE-EVIDENCE.md</files>
  <read_first>scripts/f23-macos-binary.sh and scripts/f23-session-operator-drive.sh from 23B-01 (the established driver family for this phase — the `--binary` / `--sha` / `--nonce` argument contract, the `--build-info` provenance assertion before anything runs, the hermetic home, the per-verb transcripts, the nonce-bound terminal PASS marker, and non-zero exit on a missing observable outcome; reuse its idioms so the drivers read as one family, and consume the macOS binary resolver rather than writing a second one), scripts/wayland-e2e-windows-soak.ps1 lines 174-190 and 244-255 (the worked example of PowerShell exit-code capture and the post-mortem on the `$x = &amp; { … ; $LASTEXITCODE }` array-filter bug that reported a fully passing run as a failure), crates/wcore-cli/tests/support/pty.rs (Pty spawn, wait_for, send, screen_text, quit — and write_config plus harden_child_env for a hermetic home the TUI can boot in without a real provider key), crates/wcore-cli/tests/harness_tui_flow.rs (the unix-only gate and the exact reason recorded for it: the container-terminal backend in the headless hosted runner never surfaced the spawned binary's output to the master end), crates/wcore-cli/tests/memory_show_test.rs (the existing coverage of the root memory-show flag, which must keep working)</read_first>
  <behavior>
    - Each drive script takes `--binary <path>`, `--sha <commit>` and `--nonce <hex>` — the same contract 23B-01's driver established — refuses to run if the binary is missing or not executable, and asserts the binary's own `--build-info` source SHA equals `--sha` before exercising anything, so a stale binary reddens instead of silently proving old code.
    - Each drive script emits exactly one terminal marker, `F23_02_DRIVE=PASS platform=&lt;linux|macos|windows&gt; nonce=&lt;the nonce it was given&gt;`, and emits it ONLY after every control passed. Any failure exits non-zero and emits no PASS marker. The nonce is generated by the caller at run time, so a stale log from an earlier run cannot satisfy the caller's check.
    - On macOS the binary comes from `scripts/f23-macos-binary.sh`, which 23B-01 owns; this plan consumes it unchanged and does not write a second resolver.
    - It creates a throw-away home and workspace, seeds real memory content through real turns against the local mock provider, and removes both on exit including on failure.
    - Each control is exercised with an exact invocation and its observable outcome, exit code and on-disk or on-screen consequence are captured to a per-control transcript.
    - The forgetting proof is a before-and-after pair over the captured outbound request bodies, and the driver exits non-zero if the value was never present before the forget.
    - The cache invalidation proof deliberately changes the system prompt mid-session and captures the reported cause, then deliberately changes the tool set and captures that cause.
    - The compaction proof drives a session past the auto-compaction threshold and captures the reported transition and quality verdict.
    - The cost proof captures the reported spend, the provider-returned token counts, and the reconciliation delta.
    - The nudge proof drives past the configured cap and captures the refusal.
    - The TUI leg drives `/memory`, `/cost` and `/compact` over a PTY and captures the rendered screen text as the observation record.
    - The driver never treats a missing observable outcome as a skip.
  </behavior>
  <action>Write `scripts/f23-context-economics-drive.sh` and its PowerShell port, reusing the argument handling (`--binary`, `--sha`, `--nonce`), the `--build-info` provenance assertion, the hermetic-home construction, the transcript layout, the nonce-bound terminal marker and the cleanup traps established by 23B-01's driver so the two read as one family. The PowerShell port reads `$LASTEXITCODE` on the line AFTER any pipeline and never as the trailing value of a `&amp; { … }` block, and always ends with an explicit `exit` — copy the discipline and the post-mortem comment from `scripts/wayland-e2e-windows-soak.ps1:174-190`.

Seed real memory by running actual turns against the local mock provider, planting a run-time-generated value. Then exercise, capturing the invocation, stdout, exit code and consequence of each: show what activated this turn and where each item came from; correct one item; forget the planted value and take one more turn; apply a privacy scope excluding a partition and take one more turn; set a retention bound and observe an expired item reported as expired; change the system prompt mid-session and capture the reported cache cause; change the tool set and capture that cause; drive past the auto-compaction threshold and capture the transition and quality verdict; capture reported spend against the provider-returned counts and record the delta; and drive past the nudge cap and capture the refusal.

The forgetting and privacy proofs are before-and-after pairs over the captured outbound bodies. Exit non-zero if the planted value was not present BEFORE the control was applied — an absence that was always absent proves nothing, and this is the specific way this proof gets faked.

Run the driver three times, each against the exact commit under test and each with a nonce the caller generates at run time. Linux on `hetzner-dsm` against a release binary built there after `git checkout -q --detach $SHA`. Windows on `SeanDesktop` through the PowerShell port after the same detached checkout on `C:\ferrox-win`; the remote default shell is PowerShell, so the ssh command string ends with an explicit `exit $LASTEXITCODE` and is NEVER piped into a filter. macOS on this Mac against the binary `scripts/f23-macos-binary.sh` resolves — a real local invocation of the real product, not an evidence-file grep. Each leg's ssh or local exit status is the primary gate; the nonce-bound terminal marker in the captured log is the second, independent one.

Then run the TUI leg on Linux and macOS: drive `/memory`, `/cost` and `/compact` as real keystrokes over a PTY, wait for the rendered anchors, and write the captured screen text into the run directory. On Windows, carry forward 23B-01's recorded measurement of whether the terminal backend surfaces output on real hardware — if 23B-01 measured that it does, run the TUI leg there too; if it measured that it does not, record these controls as observed via the command surface only and do NOT claim a TUI observation that did not happen.

Add `crates/wcore-cli/tests/memory_control_lifecycle.rs` as the committed, always-run regression form of the driver's assertions, so the live evidence is reproducible in CI and not only in a one-off script run.

Write `23B-02-LIVE-EVIDENCE.md` as a table: one row per control per platform with the exact invocation, observed outcome, exit code, consequence and a PASS, RED or OPEN verdict; plus the measured cost-reconciliation delta and the nudge cap that was driven past. State F23-03's and F23-04's dispositions separately — one may complete while the other does not. Marks F23-03 and F23-04 complete only if every row for that requirement is PASS on all three platforms.</action>
  <verify>
    <automated>cd /Users/seandonahoe/dev/waylandcore-ferrox &amp;&amp; test -x scripts/f23-macos-binary.sh &amp;&amp; test -x scripts/f23-context-economics-drive.sh &amp;&amp; test -f scripts/f23-context-economics-drive.ps1 &amp;&amp; bash -n scripts/f23-context-economics-drive.sh</automated>
    <automated>cd /Users/seandonahoe/dev/waylandcore-ferrox &amp;&amp; mkdir -p .planning/phases/23B-continuous-agency/evidence &amp;&amp; NONCE=$(/usr/bin/openssl rand -hex 8) &amp;&amp; SHA=$(/usr/bin/git rev-parse HEAD) &amp;&amp; L=.planning/phases/23B-continuous-agency/evidence/23B-02-linux-drive.log &amp;&amp; ssh -o BatchMode=yes hetzner-dsm "cd /root/wayland &amp;&amp; git fetch -q origin plan/f20-unified-audit-repair &amp;&amp; git checkout -q --detach $SHA &amp;&amp; cargo build --release -p wcore-cli --bin wayland-core &amp;&amp; bash scripts/f23-context-economics-drive.sh --binary target/release/wayland-core --sha $SHA --nonce $NONCE" > "$L" 2>&amp;1; rc=$?; test "$rc" -eq 0 &amp;&amp; /usr/bin/grep -qF "F23_02_DRIVE=PASS platform=linux nonce=$NONCE" "$L"</automated>
    <automated>cd /Users/seandonahoe/dev/waylandcore-ferrox &amp;&amp; mkdir -p .planning/phases/23B-continuous-agency/evidence &amp;&amp; test "$(uname -s)" = Darwin &amp;&amp; NONCE=$(/usr/bin/openssl rand -hex 8) &amp;&amp; SHA=$(/usr/bin/git rev-parse HEAD) &amp;&amp; BIN=$(bash scripts/f23-macos-binary.sh) &amp;&amp; L=.planning/phases/23B-continuous-agency/evidence/23B-02-macos-drive.log &amp;&amp; bash scripts/f23-context-economics-drive.sh --binary "$BIN" --sha "$SHA" --nonce "$NONCE" > "$L" 2>&amp;1; rc=$?; test "$rc" -eq 0 &amp;&amp; /usr/bin/grep -qF "F23_02_DRIVE=PASS platform=macos nonce=$NONCE" "$L"</automated>
    <automated>cd /Users/seandonahoe/dev/waylandcore-ferrox &amp;&amp; mkdir -p .planning/phases/23B-continuous-agency/evidence &amp;&amp; NONCE=$(/usr/bin/openssl rand -hex 8) &amp;&amp; SHA=$(/usr/bin/git rev-parse HEAD) &amp;&amp; L=.planning/phases/23B-continuous-agency/evidence/23B-02-windows-drive.log &amp;&amp; ssh -o BatchMode=yes SeanD@seandesktop "Set-Location C:\ferrox-win; git fetch -q origin plan/f20-unified-audit-repair; git checkout -q --detach $SHA; if (\$LASTEXITCODE -ne 0) { exit 91 }; cargo build --release -p wcore-cli --bin wayland-core; if (\$LASTEXITCODE -ne 0) { exit 90 }; powershell -NoProfile -ExecutionPolicy Bypass -File scripts\f23-context-economics-drive.ps1 -Binary target\release\wayland-core.exe -Sha $SHA -Nonce $NONCE; exit \$LASTEXITCODE" > "$L" 2>&amp;1; rc=$?; test "$rc" -eq 0 &amp;&amp; /usr/bin/grep -qF "F23_02_DRIVE=PASS platform=windows nonce=$NONCE" "$L"</automated>
    <automated>cd /Users/seandonahoe/dev/waylandcore-ferrox &amp;&amp; SHA=$(/usr/bin/git rev-parse HEAD) &amp;&amp; ssh -o BatchMode=yes hetzner-dsm "cd /root/wayland &amp;&amp; git checkout -q --detach $SHA &amp;&amp; test -f crates/wcore-cli/tests/memory_control_lifecycle.rs &amp;&amp; cargo nextest run -p wcore-cli --profile ci --test memory_control_lifecycle --test memory_show_test --no-tests=fail --no-fail-fast"</automated>
    <!-- Control coverage is asserted against the CAPTURED DRIVE LOGS written by the three gates above, never against 23B-02-LIVE-EVIDENCE.md. Grepping that table proved only that the executor typed into it. RED at base: no log exists. -->
    <automated>cd /Users/seandonahoe/dev/waylandcore-ferrox &amp;&amp; for P in linux macos windows; do L=.planning/phases/23B-continuous-agency/evidence/23B-02-$P-drive.log; test -f "$L" || exit 1; N=$(/usr/bin/grep -oE 'nonce=[0-9a-f]{16}' "$L" | tail -1 | cut -d= -f2); test -n "$N" || exit 1; /usr/bin/grep -qF "F23_02_DRIVE=PASS platform=$P nonce=$N" "$L" || exit 1; for C in activation provenance correct forget privacy retention cache-system cache-tools token-pressure compaction-quality cost-reconcile nudge-cap; do /usr/bin/grep -qE "F23_02_CONTROL=$C platform=$P status=PASS exit=[0-9]+ nonce=$N" "$L" || exit 1; done; done</automated>
    <!-- The before-and-after pair is the whole acceptance for forgetting and privacy, so it is gated as four separate captured markers rather than as a prose row: an absence that was always absent must not pass. RED at base. -->
    <automated>cd /Users/seandonahoe/dev/waylandcore-ferrox &amp;&amp; for P in linux macos windows; do L=.planning/phases/23B-continuous-agency/evidence/23B-02-$P-drive.log; /usr/bin/grep -qE '^F23_02_FORGET_BODY_BEFORE_CONTAINS_NONCE=true$' "$L" &amp;&amp; /usr/bin/grep -qE '^F23_02_FORGET_BODY_AFTER_CONTAINS_NONCE=false$' "$L" &amp;&amp; /usr/bin/grep -qE '^F23_02_PRIVACY_BODY_BEFORE_CONTAINS_NONCE=true$' "$L" &amp;&amp; /usr/bin/grep -qE '^F23_02_PRIVACY_BODY_AFTER_CONTAINS_NONCE=false$' "$L" &amp;&amp; /usr/bin/grep -qE '^F23_02_COST_RECONCILE_DELTA=-?[0-9][0-9.]*$' "$L" &amp;&amp; /usr/bin/grep -qE '^F23_02_NUDGE_CAP_REFUSED=true$' "$L" || exit 1; done</automated>
  </verify>
  <done>All three drive legs ran against the exact commit under test and each exited zero with its own fresh nonce echoed in the terminal PASS marker: Linux over ssh to `hetzner-dsm`, Windows over ssh to `SeanDesktop` with the status carried by an explicit `exit $LASTEXITCODE` and never through a pipeline, and macOS by invoking the real binary locally through 23B-01's `scripts/f23-macos-binary.sh`. Each binary's `--build-info` source SHA equalled the commit under test. `23B-02-LIVE-EVIDENCE.md` carries at least thirty-six control-by-platform rows with verdicts, the three run nonces, the measured cost-reconciliation delta, and the nudge cap driven past its limit. The forgetting and privacy rows each cite a before-and-after pair over captured outbound request bodies. `memory_control_lifecycle.rs` reproduces the driver's assertions in CI and the pre-existing `memory_show_test` still passes. No control is claimed as TUI-observed on a platform where the TUI was not driven. F23-03 and F23-04 carry separate, explicit dispositions.</done>
</task>

</tasks>

<threat_model>
## Trust Boundaries

| Boundary | Description |
|----------|-------------|
| memory store → outbound provider request | Recalled memory content leaves the machine inside a prompt sent to a third-party provider |
| operator → memory control operations | Operator-supplied partition, tier, item id and privacy scope cross into the deny-by-default access grid |
| sub-agent token → memory grid | A delegated actor reads and writes memory under an enumerated scope policy |
| provider response → cost and cache reporting | Provider-returned token counts and cache fields are trusted inputs to spend and diagnostics |

## STRIDE Threat Register

| Threat ID | Category | Component | Severity | Disposition | Mitigation Plan |
|-----------|----------|-----------|----------|-------------|-----------------|
| T-23B02-01 | Information Disclosure | forgotten or privacy-scoped content still reaching the provider | critical | mitigate | Acceptance is a before-and-after pair over the recorded outbound request body, not a deleted row; the driver exits non-zero if the value was never present before the control (Task 2, Task 3) |
| T-23B02-02 | Elevation of Privilege | new memory operations bypassing `MemoryAccessGate` | high | mitigate | Every new operation runs through the unmodified gate with an appropriate token; `gate.rs` must show a zero diff, asserted mechanically (Task 1) |
| T-23B02-03 | Elevation of Privilege | user-model partition five reachable by a main-agent token | high | mitigate | Partition five stays system-only; the user-model correction path uses a system token via an audited operation and does not widen the grid (Task 1) |
| T-23B02-04 | Information Disclosure | `/cost` and `/compact` output leaking prompt or tool-argument content | high | mitigate | Diagnostics carry counts, causes and identifiers only — the same opaque-identifier discipline `wcore-protocol`'s recovery snapshots already enforce; asserted against captured screen text (Task 2, Task 3) |
| T-23B02-05 | Repudiation | correction, forget, privacy and retention leaving no trace | medium | mitigate | All four write to the existing append-only audit database, and a forget is additionally represented in the change-data-capture changelog (Task 1) |
| T-23B02-06 | Denial of Service | unbounded proactive nudges spending tokens without a user turn | medium | mitigate | Explicit per-session cap plus an off switch, proved by driving past the cap and observing refusal (Task 1, Task 3) |
| T-23B02-07 | Spoofing | cost figures self-reported rather than provider-derived | medium | mitigate | Reported spend is reconciled against the provider-returned token counts in the recorded response and the delta is reported (Task 2, Task 3) |
| T-23B02-08 | Tampering | compaction silently dropping content its level promised to preserve | medium | mitigate | Quality verdict expressed over the existing compaction ladder and level policy; a compaction that would violate its level is refused, not performed (Task 2) |
| T-23B02-SC | Tampering | package-manager installs | low | accept | This plan adds NO new external crate; `rusqlite`, `sqlite-vec`, `sha2`, `portable-pty` and the mock-server dependency are already workspace dependencies. A newly required crate triggers the Package Legitimacy Gate and a blocking human checkpoint before install, and this plan STOPS rather than installing |
</threat_model>

<verification>
- Workspace clippy clean with warnings denied on Linux and Windows.
- `cargo fmt --all -- --check` clean, run on the Mac.
- `cargo nextest run --profile ci --no-fail-fast` green on `hetzner-dsm` for `wcore-memory`, `wcore-user-model`, `wcore-agent`, `wcore-compact` and `wcore-cli`, with every pre-existing test unchanged.
- `crates/wcore-memory/src/gate.rs` shows a zero diff against the base commit recorded in `evidence/23B-02-base-sha.txt`, asserted with `git diff --quiet $BASE $SHA` — not against a `FETCH_HEAD` written by some earlier ssh session.
- Every remote gate pinned the exact commit under test with `git checkout -q --detach $SHA` and asserted a file this plan creates is present before the compiler ran, so no gate could pass on a tree lacking the work.
- No gate in this plan is a pipeline into a filter, and no exit code is read from a block that also emits output. Each of the three drive legs is closed by its own process exit status first and by a caller-generated nonce echoed in the log second.
- The macOS leg ran a real `wayland-core` binary on this Mac, resolved and provenance-checked by 23B-01's `scripts/f23-macos-binary.sh`; no macOS row is closed by grepping the evidence file alone.
- Both live drivers exit zero on Linux and macOS and the PowerShell port exits zero on Windows.
- `23B-02-LIVE-EVIDENCE.md` carries at least thirty-six control-by-platform rows with verdicts, the three run nonces, and the measured cost-reconciliation delta.

<human-check>The forgetting and privacy rows in `23B-02-LIVE-EVIDENCE.md` each cite a captured outbound request body from BEFORE the control containing the planted value, and one from AFTER it not containing it. A row citing only the after-state is not acceptable evidence and must be re-run.</human-check>
</verification>

<success_criteria>
- Success Criterion 3: memory and user-model activation, recall provenance, correction, forgetting, privacy, retention, provider choice and bounded nudges are all visible and controllable from the shipped product, and forgetting and privacy are proved by absence from the actual outbound prompt.
- Success Criterion 4: cache hit rate and named invalidation cause, token-pressure state, compaction quality verdict and provider-reconciled cost are all visible from the shipped product, and a cost regression past a configured threshold is reported with its baseline.
- The memory access grid is unmodified and every new operation is audited.
- Every existing test still passes; nothing was weakened, ignored, re-gated, timed out differently, or deleted to reach a gate.
</success_criteria>

<output>
Create `.planning/phases/23B-continuous-agency/23B-02-SUMMARY.md` when done, recording the termination state, the per-control verdict distribution across the three platforms, the measured cost-reconciliation delta, the nudge cap value and the observed refusal, the Windows TUI backend disposition carried forward from 23B-01, and the separate dispositions of F23-03 and F23-04.
</output>
