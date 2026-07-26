---
phase: 23A-governed-skills
plan: "02"
type: execute
wave: 2
depends_on:
  - "23A-01"
files_modified:
  - crates/wcore-memory/src/schema/v6_skill_governance.sql
  - crates/wcore-memory/src/schema/mod.rs
  - crates/wcore-skills/src/governance.rs
  - crates/wcore-skills/src/lib.rs
  - crates/wcore-skills/src/loader.rs
  - crates/wcore-skills/Cargo.toml
  - crates/wcore-skills/tests/governed_promotion.rs
  - crates/wcore-agent/src/bootstrap.rs
  - crates/wcore-cli/src/skills_cmd.rs
  - crates/wcore-cli/src/lib.rs
  - crates/wcore-cli/src/main.rs
  - crates/wcore-cli/tests/skills_lifecycle_cmd.rs
  - Cargo.lock
  - scripts/f23a-promotion-drive.sh
  - scripts/f23a-promotion-drive.ps1
  - .planning/phases/23A-governed-skills/23A-02-LIVE-EVIDENCE.md
autonomous: true
requirements:
  - F23-01
domain: code
must_haves:
  truths:
    - "THE PRODUCT ALREADY TELLS THE USER THIS IS MISSING, IN ITS OWN WORDS, AND THAT SENTENCE IS THIS PLAN'S DEFINITION OF DONE. `crates/wcore-cli/src/main.rs:2405-2412` is a function whose entire body rejects: promotion is suspended until F23 supplies a governed transaction that binds one reviewed procedure id to one canonical skill artifact, and it rejects before UUID parsing, database access or filesystem mutation. `crates/wcore-cli/tests/skills_lifecycle_cmd.rs` pins that containment in three cases. This plan supplies the transaction the comment names. The three suspension tests are REPLACED by governed-promotion tests that assert a strictly stronger property — that promotion without a recorded review and a passed policy still fails closed, with a specific reason — and replacing them is a real behaviour change, not a weakening. Deleting them without a stronger successor would be."
    - "PROMOTION SPANS TWO STORES THAT ARE NOT CONNECTED TODAY, AND A NON-ATOMIC PROMOTION IS WORSE THAN NO PROMOTION. The quarantine verdict is computed on disk by `loader::is_generated_draft` (loader.rs:463-474) from the manifest and the body under the resolved user skills directory. The lifecycle status lives in a completely different place: the P4 procedures table, which `transition_procedure` (main.rs:2429-2483) moves through `ProcedureStatus` (`wcore-memory/src/v2_types.rs:359-390`, where `can_transition_to` permits Staged to Active or Archived, Active to Archived or Pinned, and Pinned to Active or Archived). Nothing binds a procedure row to the skill directory it came from. Flip one without the other and the product either reports Active while the loader still quarantines, or unquarantines on disk while the row says Staged. Promotion must be ONE transaction whose partial failure leaves the pre-promotion state exactly intact, and the failure path must be proved by injecting a failure between the two writes rather than reasoned about."
    - "THE PROMOTION RECORD MUST NOT LIVE INSIDE THE DIRECTORY IT GOVERNS, AND IT MUST BIND THE EXACT BYTES IT PROMOTED. 23A-01's census measures whether the current verdict — computed from a manifest and a body that both sit inside the quarantined directory — is forgeable and whether that forgery is reachable. Whatever it concludes, the governed record introduced here does not repeat the pattern: it lives in the memory database beside the procedures table it transacts with, and it carries the SHA-256 of the exact promoted artifact bytes. A body edited after promotion no longer matches its record and falls back to quarantined without any operator action. `sha2` is already a workspace dependency at `Cargo.toml:404` and `rusqlite` is already bundled at `:394`, so this adds no new package to the workspace lock — only a new dependency edge for `wcore-skills`."
    - "A QUARANTINED DRAFT CAN NEVER ACCUMULATE PRODUCTION SUCCESS EVIDENCE, SO ANY POLICY KEYED ON USE COUNT IS UNSATISFIABLE BY CONSTRUCTION. This is the load-bearing design trap in F23-01's `evaluate` stage. A staged procedure has `use_count = 0` because it is quarantined, and it stays quarantined until promoted, and the obvious promotion policy asks for a success ratio over uses. `Curator::score` (curate.rs:146-154) already patches around it with a 0.5 Bayesian prior for zero-use rows, and `Curator::run` explicitly skips the success-ratio archive rule for Staged drafts because they have not been used. Evaluation for promotion therefore must be computed from evidence that exists BEFORE any production use: the recorded trajectories that produced the draft — the drafter writes `evidence_count` and `signature` into the manifest (`auto_skill/drafter.rs:102-111`) and `PatternDetector` carries `repeat_count` and `input_shape` (`draft.rs:67-113`) — plus any explicit operator judgement. A policy an artifact cannot satisfy is not a gate, it is a permanent denial wearing a gate's clothes, and it will be discovered and softened later by someone who does not know why it was there."
    - "THE LOADER HAS NO MEMORY HANDLE TODAY, SO THE GOVERNANCE LOOKUP IS INJECTED AND ITS ABSENCE MUST FAIL CLOSED. `loader::is_generated_draft` is pure filesystem and async, and the public entry points `load_all_skills` and `load_catalog` take no store handle. The crate already carries the extension idiom for exactly this situation — `load_all_skills_with_bundled` and `load_catalog_with_bundled` exist so bootstrap can pass a caller-owned catalog. Follow it: add governance-aware entry points that accept an optional oracle, and have the existing entry points delegate with none. When no oracle is supplied the behaviour must be byte-identical to today — every generated draft quarantined — so that every current caller, every test and every path this plan did not think about degrades to the safe state rather than to the open one."
    - "REVIEW IS NOT A BOOLEAN IN A FILE THE GENERATOR WROTE. The drafter already writes `needs_review: true` into its own manifest (`drafter.rs:108`), and `loader.rs:443-446` states in-source that review flags are not activation authority — which is correct, because the generator wrote the flag. Governed review must be an operator act recorded against a specific artifact hash by a specific actor at a specific time, in the ledger, and it must be a distinct step from promotion so that reviewing and promoting can be separately observed, separately refused, and separately revoked in 23A-03."
    - "A REPORTED RED IS WORTH FAR MORE THAN AN ENGINEERED GREEN. Never weaken an assertion, add an ignore or allow attribute, raise a timeout, re-gate, or delete an inconvenient test to reach a gate. Never widen a policy threshold after seeing it fail. Findings at CRITICAL or HIGH must be fixed or disproved; MEDIUM and below go to `.planning/BACKLOG.md` and DO NOT BLOCK."
  artifacts:
    - path: crates/wcore-skills/src/governance.rs
      provides: "The governed lifecycle kernel: the promotion record type bound to an artifact content hash, the evaluation inputs drawn from pre-use draft evidence, the policy decision, the review record, and the single atomic promote transaction across the ledger and the procedure row"
    - path: crates/wcore-memory/src/schema/v6_skill_governance.sql
      provides: "The governance ledger tables, applied as schema version 6 beside the procedures table and outside any skill directory, with the artifact hash as the binding key"
    - path: crates/wcore-skills/src/loader.rs
      provides: "The governance-aware load entry points: an injected oracle that can un-quarantine exactly the artifact hashes the ledger promoted, with absence of the oracle failing closed to today's behaviour"
    - path: crates/wcore-cli/src/skills_cmd.rs
      provides: "The operator surface — the skills subcommand's list, show, review and promote verbs with stable stdout tokens and distinct exit codes, following the agent, cron, profile and migrate subcommand pattern already in the lib"
    - path: crates/wcore-skills/tests/governed_promotion.rs
      provides: "Coverage of the atomicity failure path, the unsatisfiable-policy trap, the content-hash rebinding, the fail-closed-without-oracle default and the ledger's append-only property"
    - path: crates/wcore-cli/tests/skills_lifecycle_cmd.rs
      provides: "The replaced lifecycle tests: governed promotion succeeds only with a recorded review and a passed policy, and fails closed with a specific reason otherwise — strictly stronger than the three suspension cases they replace"
    - path: scripts/f23a-promotion-drive.sh
      provides: "The Linux live driver proving the same draft 23A-01 saw refused now executes after governed promotion, and that an unreviewed sibling still does not"
    - path: .planning/phases/23A-governed-skills/23A-02-LIVE-EVIDENCE.md
      provides: "The recorded live promotion outcome per platform: the exact invocations, the observed before-and-after behaviour of the same draft, the exit codes and the unreviewed control's refusal"
  key_links:
    - from: crates/wcore-skills/src/governance.rs
      to: crates/wcore-memory/src/schema/v6_skill_governance.sql
      via: "the promotion record persisted in the memory database beside the procedures table it transacts with, never inside the skill directory it governs"
      pattern: "kernel-to-ledger"
    - from: crates/wcore-skills/src/loader.rs
      to: crates/wcore-skills/src/governance.rs
      via: "the injected oracle consulted by the generated-provenance classifier, keyed on the artifact content hash, absent means quarantined"
      pattern: "oracle-injection"
    - from: crates/wcore-cli/src/skills_cmd.rs
      to: crates/wcore-skills/src/governance.rs
      via: "the subcommand dispatch — every operator verb reaches exactly one kernel transaction"
      pattern: "cli-to-engine"
    - from: crates/wcore-agent/src/bootstrap.rs
      to: crates/wcore-skills/src/loader.rs
      via: "the session boot passing the oracle so a promotion taken at the CLI is visible to the next session's catalog"
      pattern: "boot-wiring"
---

<objective>
Supply the governed promotion transaction the product's own source comment says is missing: bind one reviewed procedure to one exact skill artifact through a single atomic transaction against a ledger that lives outside the artifact's directory, and prove through the shipped `wayland-core` binary on Linux and Windows that the same generated draft 23A-01 watched get refused now executes after promotion — while an unreviewed sibling still does not.

Purpose: F23-01 requires generated skills to move through detect, draft, quarantine, evaluate, review and policy, and promote. Detection, drafting and quarantine already ship. Promotion is a function whose entire body is a rejection. Everything downstream of Phase 23A begins from the contract this plan establishes, which is why the ROADMAP orders 23A before 23B: 23B is admitted only from the 23A contract.
Output: One schema-versioned governance ledger; one atomic promote transaction with a proved failure path; an evaluation policy computed from evidence a quarantined artifact can actually have; an operator review step distinct from promotion; the operator surface for both; and one recorded live before-and-after per platform from the real binary.
</objective>

<execution_context>
@$HOME/.codex/gsd-core/workflows/execute-plan.md
@$HOME/.codex/gsd-core/templates/summary.md
</execution_context>

<context>
@AGENTS.md
@.planning/HANDOFF-2026-07-26-phase20-20A-complete.md
@.planning/phases/23A-governed-skills/23A-01-SURFACE-CENSUS.md
@crates/wcore-skills/src/loader.rs
@crates/wcore-skills/src/draft.rs
@crates/wcore-skills/src/curate.rs
@crates/wcore-memory/src/schema/mod.rs
@crates/wcore-memory/src/v2_types.rs
@crates/wcore-agent/src/auto_skill/drafter.rs
@crates/wcore-cli/tests/skills_lifecycle_cmd.rs
@crates/wcore-cli/src/agent_cmd.rs
</context>

<execution_rules>

**THE TWO AMENDED PHASE RULES — verbatim, and they bound this plan.**
- Findings at CRITICAL or HIGH must be fixed or disproved. MEDIUM and below are logged to `.planning/BACKLOG.md` and DO NOT BLOCK execution.
- Execution begins when no CRITICAL or HIGH finding is open, or after 2 review rounds, whichever comes first. A third round is NOT permitted; it escalates to Sean.

**DEPENDENCY.** This plan begins from 23A-01's recorded census. It preserves every gate that census resolved to GATED — promotion adds a narrow, hash-keyed exception to exactly one of them and must not widen any other. If 23A-01 terminated ESCALATED, read its blast radius before starting and do not re-enter the escalated surface.

**TERMINATION CRITERION (hard).** This plan ends in exactly one of three states and writes its SUMMARY in all three:
1. **COMPLETE** — the ledger, the transaction, the policy, the review step and the operator surface ship; the atomicity failure path and the fail-closed default are proved; and the live drivers prove the before-and-after on both platforms.
2. **PARTIAL-WITH-OPEN-CLAUSE** — one named sub-behaviour cannot be closed honestly. Record it as an OPEN clause with its evidence and its reason and stop. Phase 20A closed with four requirements explicitly open and that was the correct outcome.
3. **ESCALATED** — the transaction cannot be made atomic without a change reaching outside this plan's declared files. Record the blast radius and stop.
Under no circumstances does this plan create additional plans or extend its own task list.

**SCOPE BOUNDARY (hard).** This plan builds evaluate, review and promote. It does NOT build observe, revoke or rollback — 23A-03 owns those, and this plan must leave the ledger shaped so they are possible without a migration. It does not build the end-to-end journey driver or take the phase disposition — 23A-04 owns those. It does not touch Phase 23B's surface: operator session lifecycle, memory and user-model controls, cache and compaction economics, the repository index and the multi-day journey are planned under `.planning/phases/23B-continuous-agency/` and are not duplicated, referenced as dependencies, or contradicted here.

**FOUR-PLAN CAP.** This phase has exactly 4 plans. Do not propose a fifth.

**ENVIRONMENT.**
- Repository: `/Users/seandonahoe/dev/waylandcore-ferrox`, branch `plan/f20-unified-audit-repair`. NEVER touch `/Users/seandonahoe/dev/waylandcore`.
- NEVER run Cargo on this Mac. `cargo fmt --all -- --check` is the only cargo command used locally.
- Linux authority: `ssh -o BatchMode=yes hetzner-dsm`, `/root/wayland`.
- Windows: `ssh -o BatchMode=yes SeanD@seandesktop`, checkout `C:\ferrox-win`, PowerShell default shell, cargo at `C:\Users\seand\.cargo\bin\cargo.exe`. `cargo fmt --all` FAILS there with os error 206. Windows CI runs clippy `-D warnings` BEFORE tests, so a lint failure means tests never execute.
- Both hosts' fetch refspecs are pinned to an unrelated branch. ALWAYS `git fetch origin plan/f20-unified-audit-repair`.
- Mac `grep` is rtk-proxied and SILENTLY DROPS LINES. ALWAYS `/usr/bin/grep`, `-F` for literals. Use `/usr/bin/git` on the Mac.
- In `cmd`, `set VAR=x && ...` appends a TRAILING SPACE and Rust silently ignores it. Use `set "VAR=x"` or `$env:VAR='x'` and PROVE it took effect.
- Push the work branch to `gh` so the hosts can fetch. NO push to main, merge, PR, tag, release, deployment or issue closure — Sean-only.
- No git write commands in this repository beyond the executor's commit discipline.

**THE SELF-PASSING GATE BAN (hard).**
- `ssh host 'cmd' | grep -v CLIXML` is FORBIDDEN as a gate; the pipeline's status is grep's. Filter for READING only.
- Reading an exit code from a block that also emits output is FORBIDDEN; read it on the line AFTER the pipeline.
- Every remote gate redirects to a file, captures the status on the next line, and exits with that status.
- Do NOT close any behaviour by grepping an evidence file this plan wrote.

**MIGRATION DISCIPLINE (hard).** The memory schema is at version 5 with five ordered migrations applied by `apply_migrations`. Version 6 is ADDITIVE: it creates new tables and touches no existing one. An existing version-5 database must upgrade to 6 and keep every procedure row, and that must be proved by opening a seeded v5 database and reading its rows back after the upgrade — not asserted from the SQL's shape. `Cargo.lock` changes only by the new dependency edge for `wcore-skills`; no package version moves.

**AGENTS.md discipline.** Place new functionality in the lowest crate where it semantically belongs — the kernel in `wcore-skills`, the ledger schema in `wcore-memory`, the operator verbs in `wcore-cli`. No duplicate logic across crates. Surgical diffs. Clippy-clean at `-D warnings`. `thiserror` for the public error type, `anyhow` internally, no `unwrap()` in production code. Stage exact paths, never `-A`, never `.`. No `Co-Authored-By` trailers.
</execution_rules>

<tasks>

<task type="auto" tdd="true">
  <name>Task 1: The governance ledger and the atomic promote transaction, with the evaluation the artifact can actually satisfy</name>
  <files>crates/wcore-memory/src/schema/v6_skill_governance.sql, crates/wcore-memory/src/schema/mod.rs, crates/wcore-skills/src/governance.rs, crates/wcore-skills/src/lib.rs, crates/wcore-skills/src/loader.rs, crates/wcore-skills/Cargo.toml, crates/wcore-skills/tests/governed_promotion.rs, crates/wcore-agent/src/bootstrap.rs, Cargo.lock</files>
  <read_first>crates/wcore-memory/src/schema/mod.rs (the version constant, the ordered apply chain and the per-version apply helpers — the exact shape a sixth migration must follow), crates/wcore-memory/src/schema/v5_procedure_latency.sql (the most recent migration, as the style reference), crates/wcore-memory/src/v2_types.rs (the Procedure struct, ProcedureStatus and its transition table), crates/wcore-skills/src/loader.rs (is_generated_draft, its single call site, and the _with_bundled extension idiom every public entry point already follows), crates/wcore-skills/src/curate.rs (the score function's zero-use Bayesian prior and the explicit skip of the success-ratio rule for staged drafts — the evidence that use-count-based policy is unsatisfiable pre-promotion), crates/wcore-agent/src/auto_skill/drafter.rs (the manifest fields the drafter actually writes: the auto-drafted marker, the timestamp, the signature, the evidence count, the review flag, the score and the scorer), crates/wcore-skills/src/draft.rs (PatternDetector's repeat count and input shape, and the deterministic procedure UUID derivation), crates/wcore-agent/src/bootstrap.rs (where the catalog is loaded at session boot, so the oracle reaches the live session)</read_first>
  <behavior>
    - Test 1: with no oracle supplied, every generated draft is quarantined exactly as today — the existing loader tests still pass unchanged and a new case asserts the default explicitly.
    - Test 2: a promotion whose artifact hash is in the ledger un-quarantines exactly that artifact and nothing else, including a sibling draft in the same directory tree.
    - Test 3: editing the promoted body by one byte after promotion returns it to quarantined with no operator action, because the recorded hash no longer matches.
    - Test 4: promotion without a recorded review fails closed with a distinct, specific reason and leaves both the ledger and the procedure row untouched.
    - Test 5: promotion whose evaluation does not meet policy fails closed with a distinct, specific reason and leaves both stores untouched.
    - Test 6 (atomicity): a failure injected between the ledger write and the procedure transition leaves the pre-promotion state exactly intact — the draft is still quarantined and the row is still Staged. Proved by injection, not by reasoning about ordering.
    - Test 7: the policy is satisfiable by a real drafted artifact using only pre-use evidence, demonstrated by promoting one the drafter actually produced. A policy no artifact can satisfy fails this test.
    - Test 8: the ledger is append-only — a promotion, a later revocation-shaped entry and a re-promotion all remain readable in order, so 23A-03 can build observe and rollback without a migration.
    - Test 9: a database seeded at schema version 5 upgrades to 6 and every pre-existing procedure row reads back identical.
  </behavior>
  <action>Start with the migration because everything else keys on it. Add the sixth versioned SQL file beside the existing five, following their style, and extend the version constant and the ordered apply chain the same way version 5 did. The migration is purely additive — it creates new tables and alters nothing that exists. Design the tables so that a promotion, a review and a revocation are all rows in an append-only history keyed on the artifact hash and the procedure id, with the current state derived by reading the history rather than by mutating a status column. That shape is what lets 23A-03 add revoke and rollback without a seventh migration, and it is what makes the history observable at all.

Then write the kernel in `wcore-skills`. It owns four things and no more: computing the artifact hash over the exact promoted bytes; gathering the evaluation inputs; deciding policy; and performing the promote transaction. Add `sha2` from the workspace to this crate's dependencies — it is already a workspace dependency and `rusqlite` is already bundled, so the lock changes only by a dependency edge and no package version moves. Verify that with the lock diff before trusting it.

Get the evaluation right, because this is where the design fails quietly. A quarantined draft has never run, so its use count is zero permanently and any policy keyed on a success ratio over uses can never be satisfied. The evidence that DOES exist before promotion is what the drafter and the detector recorded: how many repeated turns produced the pattern, the shape of the tool sequence and its inputs, the signature, and the operator's own judgement at review. Build the policy from those. Then prove the policy is satisfiable by promoting an artifact the drafter actually produced — if no real artifact can pass, the policy is a permanent denial wearing a gate's clothes and someone will soften it later without knowing why it existed. State the chosen thresholds and the reasoning in-source. A threshold chosen after seeing the measurement is legitimate and must be recorded as such; a threshold quietly widened after a failure is not.

Make the transaction atomic and prove it by injection. Write the ledger entry and transition the procedure row so that a failure at any point leaves the pre-promotion state exactly as it was: draft quarantined, row Staged, no orphan ledger entry that a later read would interpret as a promotion. Then inject a failure between the two writes and assert both stores are untouched. Reasoning about ordering is not proof; the injection is.

Wire the loader last. Add governance-aware entry points that take an optional oracle, following the same extension idiom the bundled-catalog variants already use, and have the existing entry points delegate with none. Absence of the oracle must be byte-identical to today's behaviour, so every existing caller and every path this plan did not consider degrades to quarantined rather than to open. Then pass the oracle from session boot so a promotion taken at the CLI is visible to the next session's catalog — without that wiring the transaction is real and invisible.

Log every MEDIUM and below finding to `.planning/BACKLOG.md`.

Implements the evaluate, review-record and promote stages of F23-01; marks no requirement complete.</action>
  <verify>
    <automated>cd /Users/seandonahoe/dev/waylandcore-ferrox &amp;&amp; cargo fmt --all -- --check</automated>
    <automated>cd /Users/seandonahoe/dev/waylandcore-ferrox &amp;&amp; test -f crates/wcore-memory/src/schema/v6_skill_governance.sql &amp;&amp; test -f crates/wcore-skills/src/governance.rs</automated>
    <automated>cd /Users/seandonahoe/dev/waylandcore-ferrox &amp;&amp; test "$(/usr/bin/grep -cE 'CURRENT_VERSION: u32 = 6' crates/wcore-memory/src/schema/mod.rs)" -ge 1</automated>
    <automated>cd /Users/seandonahoe/dev/waylandcore-ferrox &amp;&amp; test "$(/usr/bin/grep -cF 'pub mod governance;' crates/wcore-skills/src/lib.rs)" -ge 1 &amp;&amp; test "$(/usr/bin/grep -cF 'sha2' crates/wcore-skills/Cargo.toml)" -ge 1</automated>
    <automated>cd /Users/seandonahoe/dev/waylandcore-ferrox &amp;&amp; test "$(/usr/bin/grep -cF 'is_generated_draft' crates/wcore-skills/src/loader.rs)" -ge 1</automated>
    <automated>cd /Users/seandonahoe/dev/waylandcore-ferrox &amp;&amp; /usr/bin/git diff -- Cargo.lock | /usr/bin/grep -cE '^[-+]version = ' | { read n; test "$n" -eq 0 || { echo "Cargo.lock moved a package version"; exit 1; }; }</automated>
    <automated>SHA=$(cd /Users/seandonahoe/dev/waylandcore-ferrox &amp;&amp; /usr/bin/git rev-parse HEAD); ssh -o BatchMode=yes hetzner-dsm "cd /root/wayland &amp;&amp; git fetch origin plan/f20-unified-audit-repair &amp;&amp; git checkout --detach $SHA &amp;&amp; git rev-parse HEAD &amp;&amp; cargo nextest run --profile ci -p wcore-skills -p wcore-memory --no-fail-fast" &gt; /tmp/f23a-02-kernel-linux.log 2&gt;&amp;1; rc=$?; tail -60 /tmp/f23a-02-kernel-linux.log; exit $rc</automated>
    <automated>SHA=$(cd /Users/seandonahoe/dev/waylandcore-ferrox &amp;&amp; /usr/bin/git rev-parse HEAD); ssh -o BatchMode=yes hetzner-dsm "cd /root/wayland &amp;&amp; git rev-parse HEAD &amp;&amp; cargo build --locked --workspace --all-features &amp;&amp; cargo nextest run --profile ci --workspace --no-fail-fast" &gt; /tmp/f23a-02-aggregate-linux.log 2&gt;&amp;1; rc=$?; tail -40 /tmp/f23a-02-aggregate-linux.log; exit $rc</automated>
  </verify>
  <done>Schema version 6 exists as a purely additive migration in the established style, and a database seeded at version 5 upgrades with every procedure row reading back identical. The kernel owns hashing, evaluation, policy and the promote transaction, and nothing else. The policy is computed from pre-use draft evidence and is proved satisfiable by an artifact the drafter actually produced, with its thresholds and reasoning recorded in-source. The transaction's atomicity is proved by injecting a failure between the two writes and observing both stores untouched. The ledger is append-only and reads back in order. The loader's governance-aware entry points exist, absence of the oracle is byte-identical to today, and boot passes the oracle so a CLI promotion is visible next session. `Cargo.lock` moved no package version. The `wcore-skills` and `wcore-memory` suites and the full workspace aggregate are green on Hetzner at the pinned SHA.</done>
</task>

<task type="auto" tdd="true">
  <name>Task 2: The operator surface for review and promotion, replacing the suspension tests with strictly stronger ones</name>
  <files>crates/wcore-cli/src/skills_cmd.rs, crates/wcore-cli/src/lib.rs, crates/wcore-cli/src/main.rs, crates/wcore-cli/tests/skills_lifecycle_cmd.rs</files>
  <read_first>crates/wcore-cli/src/main.rs (the skills flag family near 447-473, its dispatch near 1400-1422, the suspended promote function near 2405-2412, the shared transition backend near 2429-2483, and the TopCmd subcommand enum near 596-730 — the exact pattern a new subcommand follows), crates/wcore-cli/src/lib.rs (how agent_cmd, cron, profile and migrate are exported so their logic is unit-testable from tests/ without spawning the binary), crates/wcore-cli/src/agent_cmd.rs (the closest existing shape: a flag-driven CRUD subcommand with a testable base-path injection), crates/wcore-cli/tests/skills_lifecycle_cmd.rs (all four current cases, the isolated memory root idiom and the exact assertions that are being replaced), crates/wcore-skills/src/governance.rs (the kernel surface this task drives — every verb reaches exactly one transaction)</read_first>
  <behavior>
    - Test 1: promotion with a recorded review and a passing policy succeeds, prints a stable confirmation token naming the artifact and the transition, and exits zero.
    - Test 2: promotion without a recorded review fails closed with a specific, distinct reason and a distinct nonzero exit code, and both stores are unchanged.
    - Test 3: promotion whose evaluation fails policy fails closed with its own specific reason and its own distinct exit code, distinguishable from the missing-review case.
    - Test 4: promotion of an unknown identifier and of a malformed identifier each fail closed with their own reasons — the property the three replaced suspension cases were pinning, now asserted against a live transaction rather than a blanket rejection.
    - Test 5: the review verb records an operator review against a specific artifact hash and is observable afterwards; reviewing does not by itself promote.
    - Test 6: a review recorded against one artifact hash does not authorise promotion of a different artifact, including a same-named draft whose body differs.
    - Test 7: the existing archive behaviour still works and its legal transitions are unchanged.
  </behavior>
  <action>Add the operator surface as a subcommand in the established pattern — a module in the lib exporting a subcommand enum, wired into the top-level subcommand list, exactly the way the agent, cron, profile and migrate surfaces already are. Put it in the lib rather than the binary so its logic is testable without spawning a process, which is how every sibling surface in this crate is structured.

Re-point the suspended promote flag at the same kernel transaction rather than leaving two promotion paths with different rules. One transaction, reachable from both spellings, or the flag and the subcommand will drift and one of them will become the soft path.

Give every failure its own reason and its own exit code. A script that cannot distinguish "you did not review this" from "this did not pass policy" from "no such artifact" cannot automate around any of them, and an operator cannot tell whether to review or to reject. Print stable tokens on success so the live drivers can observe the outcome without parsing prose.

Replace the three suspension cases deliberately and record why. They pin a containment that is being removed on purpose: promotion is no longer suspended, it is governed. Each replacement asserts a strictly stronger property — that promotion still fails closed without a recorded review, without a passing policy, on an unknown identifier and on a malformed identifier, each with its own reason. Do not simply delete them. Do not weaken the fourth case. Do not modify, rename, re-gate or delete any other test in this crate.

Prove the hash binding at this surface too: a review recorded against one artifact must not authorise promoting a different one, including a draft with the same name and different bytes. That is the case that catches a review-then-swap, and it belongs here because the identifier a human types is a name or an id, not a hash.

Implements the operator half of the evaluate, review and promote stages; marks no requirement complete.</action>
  <verify>
    <automated>cd /Users/seandonahoe/dev/waylandcore-ferrox &amp;&amp; cargo fmt --all -- --check</automated>
    <automated>cd /Users/seandonahoe/dev/waylandcore-ferrox &amp;&amp; test -f crates/wcore-cli/src/skills_cmd.rs &amp;&amp; test "$(/usr/bin/grep -cF 'pub mod skills_cmd;' crates/wcore-cli/src/lib.rs)" -ge 1</automated>
    <automated>cd /Users/seandonahoe/dev/waylandcore-ferrox &amp;&amp; test "$(/usr/bin/grep -cF 'skills_cmd' crates/wcore-cli/src/main.rs)" -ge 1</automated>
    <automated>cd /Users/seandonahoe/dev/waylandcore-ferrox &amp;&amp; test "$(/usr/bin/grep -cF 'skills_archive' crates/wcore-cli/src/main.rs)" -ge 1</automated>
    <automated>cd /Users/seandonahoe/dev/waylandcore-ferrox &amp;&amp; /usr/bin/git diff --stat -- crates/wcore-cli/</automated>
    <automated>SHA=$(cd /Users/seandonahoe/dev/waylandcore-ferrox &amp;&amp; /usr/bin/git rev-parse HEAD); ssh -o BatchMode=yes hetzner-dsm "cd /root/wayland &amp;&amp; git fetch origin plan/f20-unified-audit-repair &amp;&amp; git checkout --detach $SHA &amp;&amp; git rev-parse HEAD &amp;&amp; cargo nextest run --profile ci -p wcore-cli --no-fail-fast" &gt; /tmp/f23a-02-cli-linux.log 2&gt;&amp;1; rc=$?; tail -60 /tmp/f23a-02-cli-linux.log; exit $rc</automated>
    <automated>SHA=$(cd /Users/seandonahoe/dev/waylandcore-ferrox &amp;&amp; /usr/bin/git rev-parse HEAD); ssh -o BatchMode=yes SeanD@seandesktop "cmd /c \"cd /d C:\ferrox-win &amp;&amp; git fetch origin plan/f20-unified-audit-repair &amp;&amp; git checkout --detach $SHA &amp;&amp; git rev-parse HEAD &amp;&amp; cargo clippy --workspace --all-targets -- -D warnings &amp;&amp; cargo nextest run --profile ci -p wcore-cli -p wcore-skills --no-fail-fast\"; exit \$LASTEXITCODE" &gt; /tmp/f23a-02-cli-win.log 2&gt;&amp;1; rc=$?; /usr/bin/grep -v CLIXML /tmp/f23a-02-cli-win.log | tail -60; exit $rc</automated>
  </verify>
  <done>The subcommand exists in the lib in the established pattern and is wired into the top-level subcommand list; the legacy promote flag reaches the same single transaction. Every failure carries its own reason and its own distinct exit code, and success prints stable tokens. The three suspension cases are replaced by strictly stronger governed cases with the replacement rationale recorded, the fourth case is unweakened, and no other test in the crate was touched. A review recorded against one artifact hash does not authorise promoting a different artifact of the same name. The `wcore-cli` suite is green on Hetzner, and clippy at `-D warnings` plus the `wcore-cli` and `wcore-skills` suites are green on SEANDESKTOP at the same SHA — clippy first, because on Windows a lint failure means tests never run.</done>
</task>

<task type="auto">
  <name>Task 3: Prove the before-and-after through the shipped binary on Linux and Windows</name>
  <files>scripts/f23a-promotion-drive.sh, scripts/f23a-promotion-drive.ps1, .planning/phases/23A-governed-skills/23A-02-LIVE-EVIDENCE.md</files>
  <read_first>scripts/f23a-boundary-drive.sh (23A-01's driver: the expected-SHA assertion, the negative control and the exit discipline this driver mirrors so the two are comparable), .planning/phases/23A-governed-skills/23A-01-LIVE-EVIDENCE.md (the exact recorded refusal this plan must invert for the same draft — the before half of the before-and-after already exists and is not re-derived), crates/wcore-cli/src/skills_cmd.rs (the verbs, their stable stdout tokens and their exit codes, so the driver observes outcomes rather than parsing prose), justfile (the f01-packaged-driver-gate recipes near 163-185: how the real binary is built and pinned to a clean source SHA on each platform)</read_first>
  <behavior>
    - The driver produces a generated draft through the product's own drafting path and confirms the shipped binary refuses to run it — the same refusal 23A-01 recorded, re-observed here so the before-and-after is one continuous run rather than two disconnected claims.
    - The driver records an operator review and performs a governed promotion through the shipped binary, and the promotion succeeds with its stable token and a zero exit.
    - After promotion the SAME draft executes through the shipped binary, and the execution's observable effect is captured.
    - A sibling draft that was never reviewed is still refused in the same run — the control that distinguishes governed promotion from a global switch that unquarantined everything.
    - Editing the promoted body after promotion returns it to refused in the same run, proving the hash binding at the product surface and not only in the kernel.
    - The driver asserts its own checkout SHA before acting and exits with a distinct nonzero code on mismatch.
    - The driver exits nonzero on any deviation, including the unreviewed control succeeding or the post-edit execution succeeding.
  </behavior>
  <action>Mirror 23A-01's driver structure so the two are directly comparable, and keep the same discipline: assert the expected checkout SHA first with a distinct nonzero code on mismatch, then build the binary, then drive the sequence recording the exact invocation and captured output at each step.

Drive the full arc in ONE run: draft, observe the refusal, review, promote, observe the execution, then two controls. The first control is an unreviewed sibling that must still be refused — without it the run cannot distinguish governed promotion from a switch that unquarantined the whole directory. The second control edits the promoted body by one byte and must observe the refusal return, which is the only way to prove at the product surface that the promotion is bound to the exact bytes rather than to a name.

Exit nonzero on any deviation, and specifically on either control succeeding. A driver where every step is expected to pass and nothing is expected to fail cannot go red for the reason that matters.

On Windows use the trap-safe environment assignment form and prove the value took effect before trusting anything downstream. Invoke the PowerShell driver through the file form so its own exit status is what the gate reads, and never read an exit code from a block that also emits output.

Record in `23A-02-LIVE-EVIDENCE.md`, per platform: every invocation with its captured output and exit code; the same draft's behaviour before and after promotion, side by side; both controls' results; and the artifact hash the promotion bound. State explicitly that macOS is not covered here and that 23A-04 owns its disposition.

Closes the evaluate, review and promote stages of F23-01 at the product surface for Linux and Windows; marks no requirement complete — closure is claimed by 23A-04.</action>
  <verify>
    <automated>cd /Users/seandonahoe/dev/waylandcore-ferrox &amp;&amp; cargo fmt --all -- --check</automated>
    <automated>cd /Users/seandonahoe/dev/waylandcore-ferrox &amp;&amp; test -x scripts/f23a-promotion-drive.sh &amp;&amp; test -f scripts/f23a-promotion-drive.ps1 &amp;&amp; bash -n scripts/f23a-promotion-drive.sh</automated>
    <automated>cd /Users/seandonahoe/dev/waylandcore-ferrox &amp;&amp; test "$(/usr/bin/grep -cF 'WAYLAND_EXPECT_SHA' scripts/f23a-promotion-drive.sh)" -ge 1 &amp;&amp; test "$(/usr/bin/grep -cF 'WAYLAND_EXPECT_SHA' scripts/f23a-promotion-drive.ps1)" -ge 1</automated>
    <automated>cd /Users/seandonahoe/dev/waylandcore-ferrox &amp;&amp; test "$(/usr/bin/grep -ciF 'unreviewed' scripts/f23a-promotion-drive.sh)" -ge 1 &amp;&amp; test "$(/usr/bin/grep -ciF 'unreviewed' scripts/f23a-promotion-drive.ps1)" -ge 1</automated>
    <automated>SHA=$(cd /Users/seandonahoe/dev/waylandcore-ferrox &amp;&amp; /usr/bin/git rev-parse HEAD); ssh -o BatchMode=yes hetzner-dsm "cd /root/wayland &amp;&amp; git fetch origin plan/f20-unified-audit-repair &amp;&amp; git checkout --detach $SHA &amp;&amp; WAYLAND_EXPECT_SHA=$SHA bash scripts/f23a-promotion-drive.sh" &gt; /tmp/f23a-02-drive-linux.log 2&gt;&amp;1; rc=$?; tail -80 /tmp/f23a-02-drive-linux.log; exit $rc</automated>
    <automated>SHA=$(cd /Users/seandonahoe/dev/waylandcore-ferrox &amp;&amp; /usr/bin/git rev-parse HEAD); ssh -o BatchMode=yes SeanD@seandesktop "cmd /c \"cd /d C:\ferrox-win &amp;&amp; git fetch origin plan/f20-unified-audit-repair &amp;&amp; git checkout --detach $SHA\"; \$env:WAYLAND_EXPECT_SHA='$SHA'; powershell -NoProfile -File C:\ferrox-win\scripts\f23a-promotion-drive.ps1; exit \$LASTEXITCODE" &gt; /tmp/f23a-02-drive-win.log 2&gt;&amp;1; rc=$?; /usr/bin/grep -v CLIXML /tmp/f23a-02-drive-win.log | tail -80; exit $rc</automated>
    <automated>SHA=$(cd /Users/seandonahoe/dev/waylandcore-ferrox &amp;&amp; /usr/bin/git rev-parse HEAD); ssh -o BatchMode=yes hetzner-dsm "cd /root/wayland &amp;&amp; git rev-parse HEAD &amp;&amp; WAYLAND_EXPECT_SHA=$SHA bash scripts/f23a-boundary-drive.sh" &gt; /tmp/f23a-02-boundary-regress.log 2&gt;&amp;1; rc=$?; tail -40 /tmp/f23a-02-boundary-regress.log; exit $rc</automated>
  </verify>
  <done>One continuous run per platform drives draft, refusal, review, promotion, execution, and both controls. The unreviewed sibling is still refused and the post-edit body is refused again, and the driver exits nonzero if either control succeeds. Both drivers assert their checkout SHA first. 23A-01's boundary driver still passes at this SHA, so promotion did not widen the quarantine boundary it was built on top of. `23A-02-LIVE-EVIDENCE.md` records every invocation, its output, its exit code, the before-and-after side by side, both controls and the bound artifact hash, and states that macOS is 23A-04's disposition. No gate took its status from a pipeline.</done>
</task>

</tasks>

## What this plan does NOT change (scope fence)

- **The quarantine gates 23A-01 resolved as GATED.** Promotion adds one narrow, hash-keyed exception at the loader's generated-provenance classifier. No other filter is widened, and the boundary driver from 23A-01 must still pass at this plan's SHA.
- **Observe, revoke and rollback.** 23A-03 owns them. This plan only guarantees the ledger is shaped so they need no further migration.
- **The journey driver, the phase disposition and the macOS decision.** 23A-04 owns them.
- **Phase 23B's entire surface** — operator session lifecycle, memory and user-model controls, cache and compaction economics, the repository index and the multi-day journey, planned under `.planning/phases/23B-continuous-agency/`.
- **Detection and drafting.** `PatternDetector`, `DraftWriter` and `SkillDrafter` produce the artifacts this plan governs; their behaviour is unchanged.
- **The existing archive path and the procedure transition table.** `Staged → Archived` stays legal and the transition rules are not rewritten.
- **Any existing migration.** Version 6 is additive only; versions 1 through 5 are untouched.
- **No test is deleted, weakened, re-gated, ignored or allow-attributed.** The three suspension cases are REPLACED by strictly stronger governed cases with the rationale recorded; nothing else in that file or any other is touched.

<threat_model>
## Trust Boundaries

| Boundary | Description |
|----------|-------------|
| execution authority ← promotion record | Whatever the ledger says is promoted becomes executable, so the ledger is the new authority root for generated content |
| promotion record ← artifact bytes | A record that names a skill rather than binding its bytes authorises whatever those bytes later become |
| review ← the generator | The drafter writes its own review flag; an operator review must come from outside the artifact it reviews |
| ledger state ← partial failure | A transaction that fails between two stores leaves a state neither store agrees on, and the disagreement resolves toward whichever store the loader reads |

## STRIDE Threat Register

| Threat ID | Category | Component | Severity | Disposition | Mitigation Plan |
|-----------|----------|-----------|----------|-------------|-----------------|
| T-23A-02-01 | Elevation of Privilege | Promotion binds a name rather than the exact bytes, so a review-then-swap promotes content nobody reviewed | critical | mitigate | The record carries the SHA-256 of the promoted bytes; a one-byte edit returns the artifact to quarantined with no operator action, proved in the kernel and again at the product surface by the driver's post-edit control |
| T-23A-02-02 | Elevation of Privilege | The promotion exception is written wider than one artifact, unquarantining the whole generated corpus | critical | mitigate | Promotion un-quarantines exactly one artifact hash, proved against a sibling draft in the same tree; the driver's unreviewed-sibling control re-proves it through the shipped binary; 23A-01's boundary driver must still pass at this SHA |
| T-23A-02-03 | Tampering | A partial transaction leaves the artifact un-quarantined on disk while the procedure row still says Staged, or leaves an orphan ledger entry a later read treats as a promotion | high | mitigate | Atomicity is proved by injecting a failure between the two writes and asserting both stores untouched; reasoning about write ordering is explicitly not accepted as proof |
| T-23A-02-04 | Spoofing | The generator's own review flag is treated as review authority, so content reviews itself | high | mitigate | Review is a distinct operator act recorded in the ledger against a specific artifact hash by a specific actor at a specific time; the drafter's manifest flag is never read as authority, and the loader already states in-source that review flags are not activation authority |
| T-23A-02-05 | Denial of Service | The evaluation policy is keyed on use counts a quarantined artifact can never accumulate, so promotion is permanently impossible and the gate is later softened by someone who does not know why it existed | high | mitigate | Policy is computed from pre-use draft evidence, and satisfiability is proved by promoting an artifact the drafter actually produced; thresholds and their reasoning are recorded in-source |
| T-23A-02-06 | Elevation of Privilege | A caller or a code path that does not supply the governance oracle gets the open behaviour instead of the closed one | high | mitigate | Absence of the oracle is byte-identical to today — everything generated stays quarantined — and that default is asserted explicitly rather than inherited; the existing loader tests continue to pass unchanged |
| T-23A-02-07 | Tampering | The schema migration alters an existing table and silently damages procedure rows on upgrade | medium | mitigate | Version 6 is additive only; a database seeded at version 5 is upgraded and every pre-existing row is read back and compared, rather than the SQL being inspected for shape |
| T-23A-02-08 | Repudiation | Failures are indistinguishable, so an operator cannot tell whether to review, to reject, or to correct an identifier, and a script cannot automate around any of them | medium | mitigate | Each failure carries its own specific reason and its own distinct exit code, asserted separately at the CLI surface |
| T-23A-02-09 | Spoofing | A gate that cannot fail: a piped ssh status, an exit code read from an output-emitting block, or a driver run against a stale checkout | high | mitigate | Both shapes are banned by name; every remote gate redirects and exits with the captured status; the driver asserts its checkout SHA first with a distinct nonzero code |
| T-23A-02-SC | Tampering | npm/pip/cargo installs | low | accept | `sha2` and `rusqlite` are already workspace dependencies; the change adds one dependency edge and moves no package version, which is gate-checked against the lock diff |
</threat_model>

<verification>
Local gates (Mac, source level only — Cargo is never run here): `cargo fmt --all -- --check` clean; the sixth migration and the kernel module exist; the version constant reads 6; the kernel is exported and the hashing dependency is declared; the new subcommand module exists and is both exported and wired; the archive flag still exists; the lock diff moves no package version; the diffs over the CLI, skills and memory crates are surgical.

Authoritative gates (real hardware, status taken from the remote process and never from a pipeline): on Hetzner at the pinned SHA, the `wcore-skills` and `wcore-memory` suites pass, the `wcore-cli` suite passes, the full workspace builds with `--locked --workspace --all-features` and the full aggregate passes, the promotion driver runs green including both controls, and 23A-01's boundary driver still passes. On SEANDESKTOP at the same SHA, clippy at `-D warnings` passes FIRST — a lint failure there means tests never execute — then the `wcore-cli` and `wcore-skills` suites, then the promotion driver through the PowerShell file form.

Known unknowns to record rather than resolve here: whether an existing user's real memory database in the wild upgrades as cleanly as a seeded fixture; whether the chosen policy thresholds are right for corpora other than the drafts this workspace produces; and whether macOS behaves identically, which this plan does not measure and 23A-04 dispositions.
</verification>

<success_criteria>
- Promotion is a real transaction: the function whose body was a rejection now binds one reviewed procedure to one exact skill artifact, and the source comment that named the missing transaction is satisfied rather than deleted.
- The governance ledger lives in the memory database outside the artifact's directory, is append-only, is applied as an additive schema version 6, and a version-5 database upgrades with every procedure row identical.
- The promotion record binds the SHA-256 of the promoted bytes; a one-byte edit returns the artifact to quarantined with no operator action, proved in the kernel and again through the shipped binary.
- Promotion un-quarantines exactly one artifact and nothing else, proved against a sibling draft and re-proved live by the unreviewed control.
- Atomicity is proved by injecting a failure between the two writes and observing both stores untouched.
- The evaluation policy is computed from evidence a quarantined artifact can actually have, is proved satisfiable by a real drafted artifact, and its thresholds and reasoning are recorded in-source.
- Review is a distinct operator act recorded against a specific artifact hash, and a review of one artifact does not authorise promoting another of the same name.
- Absence of the governance oracle is byte-identical to today's fail-closed behaviour, asserted explicitly.
- Every failure carries its own reason and its own distinct exit code; success prints stable tokens.
- The three suspension tests are replaced by strictly stronger governed tests with the rationale recorded, and no other test is modified, renamed, re-gated or deleted.
- One continuous live run per platform shows the same draft refused, then reviewed, then promoted, then executing — with both controls still refused — and the driver exits nonzero if either control succeeds.
- 23A-01's boundary driver still passes at this plan's SHA, so promotion did not widen the boundary it was built on.
</success_criteria>

## Artifacts this plan produces
- `crates/wcore-memory/src/schema/v6_skill_governance.sql` and the extended apply chain — the append-only governance ledger.
- `crates/wcore-skills/src/governance.rs` — the hashing, evaluation, policy and atomic promote kernel.
- `crates/wcore-skills/src/loader.rs` — governance-aware entry points whose oracle-absent default is byte-identical to today.
- `crates/wcore-cli/src/skills_cmd.rs` — the operator review and promote surface with stable tokens and distinct exit codes.
- `crates/wcore-skills/tests/governed_promotion.rs` and the replaced `crates/wcore-cli/tests/skills_lifecycle_cmd.rs`.
- `scripts/f23a-promotion-drive.sh` and `scripts/f23a-promotion-drive.ps1` — the SHA-asserting, doubly-controlled live drivers.
- `.planning/phases/23A-governed-skills/23A-02-LIVE-EVIDENCE.md` — the recorded before-and-after per platform.
- `23A-02-SUMMARY.md`.

<output>
Create `.planning/phases/23A-governed-skills/23A-02-SUMMARY.md` using the standard GSD summary template. Record: the ledger's table shape and why it supports revoke and rollback without a seventh migration; the version-5-to-6 upgrade proof with the rows compared; the evaluation inputs chosen, the thresholds, the reasoning, and the real artifact that proved satisfiability; the atomicity injection and what both stores looked like afterwards; the oracle-absent default and how it was asserted; the boot wiring that makes a CLI promotion visible next session; each failure reason with its exit code; the replacement rationale for each suspension test and the stronger property its successor asserts; the lock diff showing no package version moved; the exact live invocations per platform with outputs, exit codes, the before-and-after side by side, both controls and the bound artifact hash; the confirmation that 23A-01's boundary driver still passes at this SHA; the Hetzner aggregate counts with every residual failure named; the Windows clippy-then-test result; the explicit statement that macOS is 23A-04's disposition; the recorded unknowns; and which of the three termination states the plan ended in. Mark no requirement complete — F23-01 closure is claimed by 23A-04.
</output>
