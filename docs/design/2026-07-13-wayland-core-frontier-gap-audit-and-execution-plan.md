# Wayland Core Frontier Gap Audit and Execution Plan

> **Status:** Current-state static audit and delivery program
> **Date:** 2026-07-13
> **Scope:** Wayland Core engine and standalone TUI; Core's contract with Wayland Desktop; parity against pinned Hermes Agent and OpenClaw snapshots
> **Evidence baseline:** Wayland Core `112c91c03564d0d5fd2672dc0f76846bd8756a58`; Hermes Agent `dbe734beff0caf5e8ee2acbe4277db7f6cf84a21`; OpenClaw `11a0ad10e91a50d5a0e636494eea4d7ad3eaf9fc`
> **Evaluation charter:** [2026-07-13-wayland-core-frontier-evaluation-program.md](2026-07-13-wayland-core-frontier-evaluation-program.md)

## 1. Executive verdict

Wayland Core is a strong agent engine with several frontier-shaped subsystems. It is not yet a frontier-certified product. The difference is not marketing polish. It is the gap between possessing sophisticated mechanisms and proving that those mechanisms are reachable, effective, recoverable, cross-platform, and understandable in the packaged product.

The current position is:

- **Core leads or is unusually strong** in provider neutrality, typed security boundaries, sandbox and egress architecture, formal orchestration, transactional autonomous coding through Anvil, memory architecture, multi-provider deliberation, streaming, and host integration protocols.
- **Core is broadly competitive** in ordinary interactive coding, raw tool coverage, MCP breadth, browser/computer-use primitives, skills, hooks, and standalone operation.
- **Core trails Hermes and OpenClaw** in durable background-agent operation, persistent gateway lifecycle, remote execution, channel depth, migration, extension distribution, and several everyday operator controls such as inspect, steer, retry, undo, and delivery of completed background work.
- **Core's highest risks are completion risks:** dormant or partially wired assets, two overlapping autonomous-skill paths with different governance, incomplete mid-turn recovery, optional rather than useful default resource limits, live hang/wedge defects, process-global security state, and an evaluation system that cannot yet certify the claims.
- **Wayland Desktop is not “the problem.”** Desktop is the intended primary control plane. The architectural requirement is that Desktop make policy and orchestration usable while Core remains the non-bypassable enforcement and execution boundary. A missing GUI is Desktop work; an unenforced rule is Core work; a missing event or command is protocol work.

The most important strategic decision is to stop treating feature presence as completion. For each claimed capability, Wayland needs proof of this chain:

```text
declared -> configured -> constructed -> reached -> changes outcome -> observed -> survives restart
```

Core does not need a rewrite. It needs a finishing program: establish honest evidence, close the safety and recovery spine, convert orchestration into durable agency, then add the product surfaces that make those primitives usable.

### 1.1 Confidence and limits

This audit is detailed enough to set priorities and begin implementation. It is based on current source inspection, the prior eight-document adversarial dossier, current board items, direct inspection of the pinned Hermes/OpenClaw source snapshots, and the current Desktop/Core integration path.

It is **not** a runtime security certification or a statistically valid product benchmark. The existing evaluation runner is incomplete, several comparisons are static, and the three packaged products have not yet been driven through an identical repeated task corpus on all three operating-system families. Any statement below marked **unproven** must be converted into executable evidence before it becomes a release or marketing claim.

The earlier “7 of 12 frontier dimensions” result is useful internal framing, not a certification. The old master plan is also not current truth: it was pinned to Core `v0.12.21`, describes Phase 0 as unstarted, and predates work that partially landed. Its L1-L7 architecture remains useful; its status and schedule do not.

## 2. Product boundary used for the audit

```text
Wayland product
  ├─ Wayland Desktop
  │    ├─ primary GUI and operator control plane
  │    ├─ enterprise administration, profiles, setup, background-work UX
  │    └─ bundled Wayland Core over JSON stream
  └─ Wayland Core standalone
       ├─ same execution and enforcement engine
       ├─ native TUI/CLI
       └─ headless, ACP, REST, A2A, MCP and JSON-stream surfaces
```

This produces four different parity questions:

1. **Engine parity:** Can Core plan, execute, recover, coordinate, and enforce safely?
2. **Standalone parity:** Can a terminal user operate those capabilities without Desktop?
3. **Desktop contract parity:** Does the protocol expose enough state and control for Desktop to manage the engine completely?
4. **Whole-product parity:** Can Wayland as shipped match the background, gateway, channel, migration, administration, and ecosystem workflows of the peers?

No issue should be assigned merely to the component where the symptom is visible. Security enforcement, cancellation, persistence, resource accounting, child lifecycle, and trust precedence belong in Core. Presentation, setup, visual supervision, and enterprise-policy authoring belong primarily in Desktop. Commands, events, state snapshots, error semantics, and version negotiation belong in the protocol contract.

## 3. Comparative scorecard

The labels deliberately avoid fake numerical precision. **Lead** means the current static architecture or implementation is materially stronger. **Competitive** means no decisive outcome gap is evident. **Behind** means a peer exposes a meaningfully more complete workflow. **Unproven** means Core has promising machinery but lacks sufficient runtime evidence.

| Dimension | Wayland Core | Hermes Agent | OpenClaw | Current verdict and owner |
|---|---|---|---|---|
| Provider neutrality | Broad, provider-neutral engine with data-driven compatibility and pricing | Strong multi-provider agent | Strong provider/model support | **Core lead**, subject to live failover and pricing proof; Core |
| Interactive coding loop | Rich tools, checkpoints, context management, hooks, skills | Mature interactive loop | Mature interactive loop | **Competitive but reliability-unproven**; Core |
| Raw tool breadth | Roughly peer-level and already extensive | Extensive | Extensive | **Parity**; do not chase tool count; Core/plugins |
| Autonomous coding | Anvil worktrees, gates, strict-improvement checks, receipts | Capable but less formally transactional | Capable with mature agent lifecycle | **Core architectural lead**, runtime certification required; Core |
| Sandbox and egress | Cross-platform sandbox crate, workspace controls, egress policy and doorbell concepts | Useful isolation and remote backends | Permission/sandbox controls | **Core architectural lead, operationally unproven**; Core |
| Permission UX | Typed approval machinery and host protocol; force semantics are overloaded | Direct terminal experience | Mature user-facing permission flow | **Mixed/behind in product semantics**; Core + protocol + Desktop |
| Crash recovery | User-input WAL, checkpoints, sessions | Session persistence | Persistent gateway/session model | **Core behind**: WAL is not a complete turn/event journal; Core |
| Loop and spend governance | LoopGuard, FailureGuard, optional caps, budget machinery | Practical session controls | Mature long-running lifecycle | **Partially landed, not complete**: proactive defaults and monitor wiring remain; Core |
| Formal orchestration | Spawn, swarm, mesh, fleet, workflows, MoP/Crucible, Anvil | Functional multi-agent/MoA | Strong subagent lifecycle | **Core lead in primitives**; Core |
| Durable async agents | Internal spawners, but user lifecycle is incomplete | Useful background/remote execution | Nonblocking, persistent, thread-bound agents with inspect/steer/nest/delivery | **Core behind**, especially OpenClaw; Core + protocol + Desktop/TUI |
| Multi-agent governance | Typed routing, budget and provenance components | Functional MoA | Operational subagent lifecycle | **Core promising/unproven**; Core |
| Memory | Typed stores, ACL/audit concepts, consolidation, decay, hybrid retrieval | Useful persistent memory | Strong product-integrated memory/persona | **Core architectural lead**, outcome proof needed; Core |
| Self-improving skills | Pattern/draft/curation/GEPA assets | Autonomous skill workflow is productized | Large skill ecosystem | **Core stronger research machinery, weaker governance/completion**; Core |
| MCP and protocol | Client/server support plus typed JSON host protocol, ACP/REST/A2A | MCP support | Gateway/protocol ecosystem | **Core lead in breadth, lifecycle gaps remain**; Core + protocol |
| Browser and computer use | Dedicated multi-backend crates and policy boundaries | Browser/computer tools | Browser plus rich device/node surfaces | **Competitive engine; behind device product**; Core/plugins + Desktop |
| Persistent gateway/service | Headless surfaces exist; lifecycle is fragmented | Desktop/dashboard and gateways | Gateway is the product backbone | **Core/Wayland behind**; Core service primitives + Desktop |
| Channels | Channel crate and protocol work exist | Messaging integrations | More than twenty mature channels and bindings | **Wayland behind**; plugins + Desktop, not Core binary |
| Voice, mobile and devices | CUA/voice-related components exist | Voice/product surfaces | Mobile nodes, voice, canvas, device actions | **Whole-Wayland behind**; Desktop/plugins |
| Remote execution | No equally complete user-facing backend matrix | Local, Docker, SSH, Singularity, Modal, Daytona and Python RPC | Remote nodes/gateway model | **Core behind Hermes**; signed backend/plugin contract |
| TUI operator controls | Native TUI and standalone engine | Strong retry/undo/personality/insight workflows | Strong terminal operation | **Core behind in everyday controls**; Core TUI |
| Migration | Partial Hermes import work | N/A | N/A | **Core behind**: no complete OpenClaw import and incomplete profile migration; Core + Desktop |
| Extension distribution | Plugin API and packaging architecture | Skills/extensions | Populated extension ecosystem | **Architecture competitive, ecosystem behind**; signing/registry + Desktop |
| Enterprise control plane | Core has enforcement ingredients; Desktop is intended control plane | Product surfaces, weaker evidence of formal managed floor | Strong operations model | **Potential lead, currently unproven**; Core enforcement + Desktop administration |
| Evaluation and assurance | Rich test assets and incomplete scenario runner | No clear formal lead | No clear formal lead | **Core opportunity, not current lead**; assurance tooling |
| Cross-platform contract | Explicit macOS/Linux/Windows architecture and CI intent | Cross-platform product | Multi-platform gateway/apps/nodes | **Unproven until packaged E5 matrix**; Core + Desktop |

Two corrections matter. Hermes is not merely a CLI: the pinned source includes Desktop/dashboard/web/TUI and messaging gateway work. OpenClaw is terminal-first but is not merely a CLI either: its persistent Gateway, Control UI, companion applications, and mobile/node model are central to the product. Wayland must compare itself with those whole workflows while still assigning implementation to the correct layer.

## 4. Where Wayland Core is stronger

### 4.1 Provider-neutral execution

Core's internal provider boundary is a real strategic advantage. The engine operates on provider-neutral request, event, message, and content types; provider quirks are intended to flow through `ProviderCompat`; multiple first-party providers and a broad provider catalog already exist. This is a better base for enterprise routing, data-residency policies, price-aware selection, model substitution, and private endpoints than a product built around one provider's semantics.

The qualification is that failover behavior, cooldown state, and refreshed pricing are not yet proven as one production path. `PricingRefresher` and `CooldownTracker` exist, but current static inspection did not find complete production construction and outcome evidence. The architecture leads; the claim remains below production grade until failover tests prove compatible substitution without tool-protocol or safety regression.

### 4.2 Formal orchestration and autonomous coding

Core has a deeper orchestration vocabulary than a normal “spawn another prompt” implementation: subagents, swarm/mesh/fleet concepts, declarative workflows, provider deliberation, and Anvil's isolated-worktree/gate/receipt model. The strongest piece is not worker count. It is the ability to make delegated work transactional and verifiable.

That crown jewel must not be diluted into a universal concurrent free-for-all. The ordinary `Delegate` path still needs the same isolation, approval inheritance, cancellation, and receipt guarantees. “One hundred workers” is not a moat until budgets, no-progress detection, durable child state, and merge conflict behavior are proven under load.

### 4.3 Security architecture

Core contains unusually serious security components for a general agent:

- platform-specific sandbox backends behind one contract;
- egress policy and approval concepts;
- workspace trust and protected paths;
- typed approval modes and protocol events;
- separation between provider-neutral engine and host UI;
- plugin API isolation boundaries;
- an explicit effort to centralize platform behavior and avoid shell interpolation.

This is stronger engineering intent than a permissive tool loop wrapped in confirmation prompts. The remaining work is to eliminate bypass ambiguity and process-global leakage, prove the boundary on real platforms, and make Smart Default pleasant enough that users do not reflexively select Force.

### 4.4 Typed memory and evolution machinery

Core's memory work is more structured than a folder of prompt fragments: it includes typed memory, provenance/audit concepts, access control, consolidation/decay, and hybrid retrieval. Its skills/evolution machinery includes pattern detection, draft writing, curation, user modeling, and GEPA-style evaluation loops.

The problem is not sophistication. It is split lifecycle governance. A newer draft path stages changes more safely, while a legacy memory-backed `SkillDrafter` can still be installed and observed under different gating. Core should retain the research machinery and replace the two pipelines with one auditable state machine.

### 4.5 Protocol and embedding breadth

The typed JSON-stream protocol gives Wayland a clean Desktop/Core boundary, and Core also exposes other embedding and interoperability paths. This makes Core useful as both the engine bundled with Desktop and an independent terminal/headless agent. Neither use case should be sacrificed.

The next step is not to add another transport. It is to certify one lifecycle across all transports: approvals, child agents, cancellation, budgets, recovery, capability negotiation, errors, and state resynchronization.

## 5. Where Hermes and OpenClaw are stronger

### 5.1 OpenClaw: durable product operation

OpenClaw's main advantage is operational completeness. Background agents are first-class user objects: they can be started nonblockingly, bound to threads, listed, inspected, steered, nested, and delivered back into the interaction. Its persistent Gateway supplies a coherent lifecycle for sessions, channels, devices, and control surfaces.

Core has many of the computational primitives but not yet one durable user-facing child model. A child must stop being an in-memory task and become a persistent resource with identity, parentage, policy snapshot, workspace, budget, status, event cursor, result, cancellation state, and restart behavior.

OpenClaw is also ahead in channel depth, native channel affordances, mobile/device nodes, canvas/voice experiences, and automation such as heartbeats and standing orders. Most of that should not be compiled into `wcore-agent`. Core should supply the durable session, scheduler, policy, protocol, and delivery primitives; signed plugins and Desktop should provide setup and product surfaces.

### 5.2 Hermes: execution reach and operator experience

Hermes is materially ahead in remote execution breadth. Its local, Docker, SSH, Singularity, Modal, Daytona, and Python RPC paths let the same agent operate in very different environments. Wayland should answer this with a narrow execution-backend contract, not hardcoded remote providers in the engine. Each backend needs signed identity, explicit policy, secret routing, artifact transfer, cancellation, resource limits, and an attested receipt.

Hermes also presents several agent controls more directly: retry, undo, personality/insight surfaces, and a more productized autonomous skill experience. These are not superficial. Retry and undo reduce the cost of failure; insight and state visibility reduce operator uncertainty; safe skill installation makes extensibility usable.

Hermes has broader migration behavior, including imports from OpenClaw-related persona, memory, skills, settings, secrets, and assets. Core's importer work is partial. Migration is a product adoption feature and a security boundary: imported executable content must be inert until reviewed and trusted.

### 5.3 Both peers: finishing discipline

The peers' practical advantage is that more capability is connected to a complete operator loop. Core contains multiple examples of assets that exist, test locally, or are exported but are not demonstrably constructed and reached in production. The prior dossier called this “built but not wired.” Current source inspection still supports that concern for pricing refresh, mid-flight monitoring, cooldown tracking, learned policy, and parts of handoff/isolation behavior.

This is the central product risk. Adding more subsystems before establishing activation proof will increase the distance between the architecture diagram and the shipped behavior.

## 6. What Wayland Core needs to fix

### 6.1 Frontier blockers

| ID | Gap | Current evidence | Consequence | Exit criterion | Ownership / board evidence |
|---|---|---|---|---|---|
| F0 | Evaluation cannot certify the product | `wayland-eval` and report renderers are still stubs; scenarios default toward Force; secrets are written into generated TOML; cleanup, hermeticity, per-turn limits, and result telemetry are incomplete | Strengths and regressions remain assertions | Packaged-binary runner produces machine-readable security, reliability, cost, process, filesystem and approval evidence under explicit postures | Assurance; existing scenario crate |
| F1 | Autonomous skill governance is split | Config defaults lifecycle on while docs say off; boolean merge can defeat project opt-out; legacy drafter and newer staged pipeline have different gates and promotion semantics | Repository/user behavior can create or activate code under surprising authority | One state machine: detect -> draft -> quarantine -> evaluate -> review/policy -> promote -> revoke, with an 8-cell precedence test | Core; #564, #693, #694 |
| F2 | Capability reachability is not enforced | Current static inspection still finds components with no complete production construction/call chain | False capability claims and dead investment | Every advertised capability has activation test, capability status, reason when unavailable, and observable outcome delta | Core/protocol; #660, #661, #664 |
| F3 | Loop and spend governance is incomplete | `LoopGuard` and `FailureGuard` exist; `max_turns` and cost caps default unset; `MidFlightMonitor` appears dark; some cost enforcement occurs after a paid turn | Runaway cost, repeated tool failures, poor long-run safety | Useful proactive defaults, pre-call and mid-flight enforcement, no-progress detection, graceful continuation, budget event stream, live adversarial proof | Core; #174, #559, #690 |
| F4 | Recovery is not crash-complete | Session WAL records user input, not the full assistant/tool/approval/budget/checkpoint event history | A crash can lose or ambiguously repeat mid-turn side effects | Append-only turn journal with idempotency keys, tool intent/result states, approval decisions, budget charges, checkpoints, recovery cursor and crash injection suite | Core/protocol; #457, #636, #691 |
| F5 | Operator modes and trust semantics are ambiguous | `--force` aliases `--yolo` and `--dangerously-skip-permissions`; sandbox bypass is separately configurable; host mode changes and policy precedence require one contract | Users cannot reason about what protection remains | Smart Default, Managed Enterprise and Explicit Dangerous Session are separate typed postures; remote/project content cannot activate dangerous mode; managed floor is monotonic | Core/protocol/Desktop; #241, #583, #657 |
| F6 | Security state is not completely session-scoped | Board evidence identifies process-global egress and MCP scope/lifecycle issues | One session can affect another in a long-lived Desktop process | All mutable enforcement state is scoped to runtime/session/child identity; parallel isolation tests prove no cross-talk | Core; #569, #605, #613, #614 |
| F7 | Known live reliability failures remain | Bash hang/runaway loop, browser failures/collisions, fork wedge, context-death and Windows/WSL issues remain active | Frontier claims fail on ordinary work | Deterministic reproductions, bounded cancellation, orphan cleanup, and packaged regression tests for each failure class | Core; #287, #305, #552, #636, #862 |
| F8 | Residual secret/filesystem/egress boundaries need closure | Active items cover central output redaction, nonregular/UNC paths, `.env` exposure and Bash egress | Data exfiltration or unsafe local access remains possible | Central classification/redaction, safe file semantics, egress mediation for every execution route, adversarial corpus on all OS families | Core; #584, #644, #667, #673 |
| F9 | Cross-platform behavior is designed but not certified | Local Mac cannot exercise other `cfg` branches; Windows/WSL defects remain; sandbox backends require real spawn tests | “Cross-platform” can mean compile-only | Packaged T5 matrix on macOS, Linux and Windows including sandbox, paths, process trees, cancellation, Unicode, long paths, WSL and installers | Core + release engineering |
| F10 | Desktop/Core conformance is not a release gate | Desktop launches Core over JSON stream and also launches standalone TUI, but lifecycle completeness is not automatically checked | Host and engine can drift in approvals, modes, errors and capabilities | Versioned capability negotiation, golden command/event tests, reconnect/state-sync tests, and N/N-1 compatibility policy | Protocol + Desktop + Core |

### 6.2 Capability and parity gaps

| ID | Gap | Correct response | Exit criterion |
|---|---|---|---|
| P1 | Durable asynchronous child sessions | Build one persistent child-agent resource model used by Spawn, Delegate, workflow, swarm and host surfaces | Start, list, inspect, steer, cancel, resume and receive results after parent or process restart |
| P2 | Transactional delegation | Extend Anvil-grade workspace isolation, gates and receipts to ordinary delegated coding work | Concurrent delegates cannot overwrite one another or parent state; merge is explicit and recoverable |
| P3 | Semantic failover | Wire pricing refresh, cooldown tracking and typed failover policy into the real provider loop | Controlled failures select compatible alternatives within policy/budget and preserve tool semantics |
| P4 | Dynamic MCP lifecycle | Finish idempotent discovery, connect/reconnect, scoping, deferred exposure, cancellation and cleanup | MCP add/remove/restart cannot leak tools or authority between sessions |
| P5 | Multimodal/document workflows | Complete the active document/image handling path and expose honest capability status | Repeated file/image/PDF tasks pass through TUI and Desktop with bounded resource use |
| P6 | Standalone TUI controls | Add durable run/child inspection, retry/regenerate, checkpoint undo, budget and authority views | A terminal user can recover and supervise without editing state files or restarting blindly |
| P7 | Migration | Complete Hermes and OpenClaw import plans with provenance and quarantine | Dry-run report, selective import, secret remapping, inert executable content, rollback and equivalence tests |
| P8 | Remote execution | Define a signed execution-backend plugin contract; implement local container and SSH reference backends before cloud breadth | Same task contract, policy and receipt works locally and remotely; cancellation leaves no orphan |
| P9 | Gateway, channels and automation | Keep channel implementations in signed plugins and administration in Desktop; Core supplies durable sessions, schedules, policy and delivery | Background scheduled work survives restart, delivers once, and is visible and controllable in Desktop |
| P10 | Extension distribution | Add signing, provenance, compatibility, permissions, quarantine, update and revocation metadata around the existing plugin API | Enterprise allowlist and safe marketplace install/update/rollback work end to end |

### 6.3 Specific corrections to the old dossier

- **L7 is not wholly missing.** LoopGuard, FailureGuard, cancellation checks, optional `max_turns`, budget events, and honored charge results exist. The remaining work is wiring the richer monitor, setting useful proactive defaults, enforcing before and during expensive work, and proving behavior under live loops.
- **The skills opt-out finding is narrower but remains serious.** The newer `DraftWriter` path respects the engine gate. The legacy memory-backed path, default-on configuration, OR merge, and documentation mismatch mean the operator contract is still not reliable.
- **The WAL exists but is not a complete recovery journal.** It protects user input and session file integrity; it does not by itself prove exactly-once recovery of model turns, tool side effects, approvals, or charges.
- **Smart handoff has a call site.** It still needs activation and outcome proof; it should not be described as wholly absent.
- **Anvil/worktree isolation is real in some paths.** The gap is the ordinary Delegate/Spawn contract and one coherent child lifecycle, not a total absence of isolation.
- **Earlier phantom or corrected defects stay retracted.** The nonexistent `STATE.md` claim, corrected UTF-8/SSE claims, inflated unwrap count, and overstated test/run claims must not re-enter the backlog.

## 7. What not to build

The following work would make the project busier without making it frontier:

- Do not rewrite the agent loop or collapse Desktop into Core.
- Do not add raw tools merely to improve a feature-count table.
- Do not put twenty channel implementations in the Core binary. Use the plugin boundary and Desktop control plane.
- Do not weaken GEPA or the typed memory architecture to imitate a looser skill folder.
- Do not advertise worker count until persistent identity, budgets, cancellation, isolation and recovery work.
- Do not make dangerous mode remotely activatable, persistent, or available to repository configuration.
- Do not run the safety benchmark in Force by default; that measures capability after removing the boundary under test.
- Do not treat a unit test for a component as proof that the packaged agent uses it.
- Do not ship the entire L1-L7 program as one mega-release. The historic finishing-discipline problem requires small independently releasable vertical slices.

## 8. Execution program

The ordering below preserves functionality while improving safety. Each wave produces a usable release, and every new restriction must be accompanied by an operator-flow test and a prompt-budget measurement.

### Wave 0 — Truth and containment

**Objective:** Make the product measurable and stop the two most dangerous forms of drift: autonomous behavior that cannot be reliably disabled and capabilities that silently do nothing.

1. Finish the evaluation driver and real console/JSON/Markdown reports.
2. Stop writing live API keys into generated configuration; pass secret references through a dedicated environment/secret-provider mechanism and redact all diagnostics.
3. Make each scenario declare posture explicitly. Smart Default is the standard capability and security baseline; Force is a separately named diagnostic run.
4. Make evaluation environments hermetic across HOME/XDG/AppData/temp, Git/SSH/proxy variables, config discovery, process groups and network policy. Always run cleanup.
5. Expand result telemetry to approvals, policy decisions, sandbox backend, egress, filesystem changes, child processes, tokens, cost, retries, resources and orphan status.
6. Add a generated capability manifest and activation events. Refuse or label unavailable capabilities instead of silently registering inert surfaces.
7. Immediately contain the legacy skill drafter behind the same explicit lifecycle gate, then execute the complete global/project/memory precedence matrix.
8. Establish golden Desktop/Core command-event and capability-negotiation tests.

**Release gate:** At least E3 for the core safety corpus; zero secrets in configs/argv/reports; no scenario defaults to dangerous posture; every advertised capability is either activated with evidence or reported unavailable.

**Why first:** Without this wave, every later “done” claim can be another built-but-not-wired asset.

### Wave 1 — Safe-default execution spine

**Objective:** Bound ordinary work without turning Wayland into a confirmation-dialog generator.

1. Formalize three postures as separate typed policy bundles:
   - Smart Default: approvals may be bypassed for low-risk sandboxed workspace work; hard floor remains.
   - Managed Enterprise: organization floor cannot be weakened by user, project, host, child, plugin, MCP or environment.
   - Explicit Dangerous Session: local interactive, loudly named, time-bounded, nonpersistent, nonremote, and disableable by managed policy.
2. Split today's overloaded naming. Keep `--force` as approval bypass with containment retained. If full bypass is retained, name it literally (for example `--dangerously-bypass-approvals-and-sandbox`) and require an explicit local ceremony.
3. Complete the trust lattice. Untrusted repositories remain useful for read, edit and sandboxed normal work, while repository hooks, skills, MCP definitions and executable configuration stay inert until trust.
4. Finish L3's non-bypassable command floor and wire learned policy only as a narrowing/preapproval aid; it must never override hard denial or managed policy.
5. Finish L7's remaining runtime work: construct the mid-flight monitor, introduce conservative default session/turn/tool/output/cost envelopes, enforce before the next provider/tool expense, detect no progress, and expose “continue with new budget” rather than model-error semantics.
6. Centralize token/output secret redaction and close `.env`, nonregular path, UNC and shell-egress gaps.

**Release gate:** Representative edit/build/test work succeeds with near-zero prompts; destructive/out-of-workspace/secret/unknown-egress actions prompt or deny exactly once at the meaningful boundary; managed policy survives every lower-trust override attempt; loops stop within declared resource envelopes.

### Wave 2 — Recovery and resilience

**Objective:** Make an interrupted or failing agent cheaper to recover than to restart.

1. Implement L4 as an append-only event journal, not a larger transcript file. Record turn starts/completions, provider attempts, tool intents/results, approval decisions, budget reservations/charges, checkpoints, child lifecycle and delivery.
2. Give externally effectful operations idempotency keys and an explicit `prepared/running/succeeded/failed/unknown` state. Recovery must never blindly repeat an operation whose outcome is unknown.
3. Implement True Continue from a committed event cursor, including Desktop reconnect and standalone TUI affordances.
4. Complete L1 semantic failover by wiring cooldown and pricing assets into provider selection, enforcing compatibility and organization policy, and surfacing the reason and cost effect.
5. Close the known Bash hang, browser collision/failure, fork wedge, context-death and Windows/WSL reproductions with bounded cancellation and orphan-process assertions.
6. Finish MCP lifecycle and session scoping so reconnect/reload cannot duplicate tools or carry authority across sessions.

**Release gate:** A crash injected before/during/after model streaming, approval, file edit and shell execution recovers to an explicit safe state; no duplicate side effect; no orphan child; provider failover preserves tool and policy semantics; Desktop reconnects without state loss.

### Wave 3 — Durable agency

**Objective:** Turn Core's orchestration primitives into a coherent persistent agent product.

1. Define one durable child-agent record with identity, parent, graph/workflow node, policy snapshot, provider/model, budget, workspace, status, event cursor, timestamps, result, delivery target and cancellation state.
2. Route Spawn, Delegate, skills, workflows, swarm, fleet and Desktop-created background work through that lifecycle instead of parallel ad hoc lifecycles.
3. Extend Anvil's transactional workspace/gate/receipt semantics to delegated coding. Non-mutating research children may share read-only state; mutating children receive isolated workspaces.
4. Add approval inheritance: children receive no greater authority than the parent, grants are scope- and resource-specific, and escalation routes to the correct operator surface.
5. Expose start/list/inspect/log/steer/pause/cancel/resume/retry/deliver through a versioned protocol. Implement the same controls in standalone TUI.
6. Replace both autonomous skill paths with the single quarantine/evaluation/promotion state machine. Generated skills are data until promoted; promotion requires policy-defined evidence and preserves provenance/rollback.

**Release gate:** Start several children, terminate the parent and engine at adversarial points, restart, inspect exact state, resume eligible work, cancel the rest, merge only gated changes, and deliver each result once.

### Wave 4 — Product parity through the correct layers

**Objective:** Close the peer workflows without corrupting the Core/Desktop separation.

Core and protocol work:

- scheduler and standing-order primitives backed by durable sessions;
- delivery routing and exactly-once/outbox semantics;
- signed execution-backend contract with local-container and SSH reference implementations;
- signed plugin metadata, permission declaration, compatibility, provenance, revocation and rollback;
- migration parser APIs that return typed plans and quarantine executable imports;
- remaining multimodal/document capability paths and resource accounting.

Desktop work:

- background-work center showing children, status, authority, budget, workspace, logs and delivery;
- managed-policy authoring/distribution and clear effective-policy explanation;
- gateway/service lifecycle, diagnostics and upgrades;
- channel installation, account binding, thread/delivery mapping and native affordances;
- migration wizard with dry-run, conflict resolution, secret remapping, selective import and rollback;
- signed extension marketplace and enterprise allowlist;
- remote-execution setup and health views.

Standalone Core work:

- equivalent inspect/steer/cancel/resume/retry/undo/budget/authority commands;
- honest messages when a Desktop-owned setup flow is unavailable, with config/CLI alternatives;
- no security or recovery dependency on the GUI being present.

**Release gate:** The same background coding, scheduled automation, remote execution, migration and channel-delivery intents complete through Desktop and standalone-supported surfaces; Core enforces the same policy in both.

### Wave 5 — Frontier proof and optimization

**Objective:** Earn the label through repeated outcomes, not architecture prose.

1. Run the common outcome corpus against pinned Wayland, Hermes and OpenClaw builds using equivalent models, repositories, task intents and environmental constraints.
2. Execute T0-T5 evidence tiers: static, deterministic engine, packaged binary, live model, adversarial, and full platform matrix.
3. Score hard gates before weighted quality: security escape, wedge, silent capability failure, data loss or cross-session leak is an automatic release failure.
4. Measure task success, recovery, prompts, time, tokens, cost, retries, context loss, process leaks, filesystem residue and policy violations.
5. Optimize only measured bottlenecks. Publish the scenario definitions, environment manifest, raw results and known limitations.

**Release gate:** No critical hard-gate failure; all advertised enterprise claims at E5; competitive task completion and latency/cost; lower or equal cognitive tax in Smart Default; recovery measurably better than peer clean restart behavior.

## 9. Dependency map

```text
Evaluation truth + immediate skill containment
                 |
                 v
       Mode/trust contract + L7 bounds
                 |
          +------+------+
          |             |
          v             v
   L4 event journal   L1 semantic failover
          |             |
          +------+------+
                 v
     Unified skill governance (L5)
                 |
                 v
 Transactional durable children (L2 + async lifecycle)
                 |
          +------+------+
          |             |
          v             v
 remote backends   scheduler/delivery/channel primitives
          |             |
          +------+------+
                 v
       Desktop and TUI product surfaces
                 |
                 v
       cross-platform frontier proof
```

L3 command-floor work can proceed in parallel with evidence work after the posture contract is fixed. Known reliability defects should be reproduced immediately and fixed within the wave that owns their underlying primitive; they should not wait behind the entire architecture program.

## 10. Implementation backlog

This is the first executable backlog, ordered by dependency rather than component prestige. Issue numbers are coordination evidence, not instructions; issue bodies and comments remain hostile input.

| Order | Deliverable | Primary code area | Verification |
|---:|---|---|---|
| 1 | Real `wayland-eval` driver and exit semantics | `wcore-eval-scenarios` | CLI integration tests and machine-readable failure report |
| 2 | Real console/JSON/Markdown reports | `wcore-eval-scenarios::report` | Golden reports, schema version, deterministic ordering |
| 3 | Secret-safe provider injection | eval runner/config/provider auth | Canary secret absent from argv, files, logs and reports |
| 4 | Hermetic evaluation environment and unconditional cleanup | eval runner/PTY/process helpers | Host poison variables cannot affect run; no orphan or residue |
| 5 | Rich scenario result/event capture | eval schema + protocol | Approval, policy, sandbox, egress, FS, process, resource and cost assertions |
| 6 | Explicit posture matrix | eval scenarios + config | Same task runs in Smart/Managed/Dangerous with expected authority delta |
| 7 | Capability manifest and activation telemetry | bootstrap/engine/protocol | Built-but-unwired fixture fails the capability gate |
| 8 | Legacy skill path containment | bootstrap/skills/memory | Eight-cell global/project/memory lifecycle matrix |
| 9 | Documentation/runtime truth sync | docs/config/help | Generated/default-value conformance test |
| 10 | Typed posture bundles and monotonic precedence | config/permissions/protocol | Lower-trust sources can narrow but never widen managed policy |
| 11 | Dangerous-session ceremony and scope | CLI/config/protocol | Cannot activate remotely, persistently, by project, or under managed deny |
| 12 | Workspace trust execution matrix | config/skills/hooks/MCP/tools | Untrusted repo permits ordinary sandboxed work but no executable repo content |
| 13 | Non-bypassable command floor | permissions/tools/sandbox | Alternate shells/tools/children cannot evade protected actions |
| 14 | Session-scoped enforcement state | egress/MCP/permissions/runtime | Parallel sessions with conflicting policies show zero cross-talk |
| 15 | Central secret/output redaction | protocol/tool output/observability | Encoded/split/case variants and `.env` canaries never reach model/log/host |
| 16 | Construct and wire mid-flight monitor | engine/budget/observability | Live loop changes outcome and emits stop reason before cap overshoot |
| 17 | Smart default resource envelopes | config/budget/engine | Ordinary corpus completes; adversarial loop stops predictably |
| 18 | Crash-complete event journal | session/engine/protocol | Crash injection at every event boundary and replay invariant checks |
| 19 | Idempotent tool-effect state machine | tools/journal/checkpoints | Unknown outcomes require reconciliation, never automatic duplicate execution |
| 20 | True Continue and host resync | engine/protocol/TUI/Desktop | Process death and reconnect continue from committed cursor |
| 21 | Semantic failover and live cooldown/pricing wiring | providers/pricing/engine | Fault-injected provider matrix with typed reasons and budget policy |
| 22 | Hang/wedge regression suite | Bash/browser/fork/context/WSL | Deadline, cancellation, process-tree and orphan assertions |
| 23 | MCP idempotency, scoping and reconnect | MCP/engine/protocol | Add/remove/restart parallel-session stress suite |
| 24 | Durable child-agent schema/store | orchestration/session/protocol | Restart-safe lifecycle state machine tests |
| 25 | Unify all spawners on durable lifecycle | agent/tools/workflows/swarm/fleet | One list/inspect/cancel API observes every child kind |
| 26 | Transactional delegated workspace and receipts | Delegate/Anvil/worktree | Parallel conflicting edits never silently overwrite; gated merge/rollback |
| 27 | Approval and budget inheritance | orchestration/permissions/budget | Child cannot exceed parent authority or reserved budget |
| 28 | TUI and protocol supervision controls | CLI/TUI/protocol | Start/list/inspect/steer/cancel/resume/retry/undo acceptance flow |
| 29 | Unified skill quarantine and promotion | skills/evolve/memory/engine | Generated code cannot execute before policy/eval promotion; revoke/rollback works |
| 30 | Desktop conformance and background-work center | protocol + Desktop | N/N-1 golden contract, reconnect, policy display, child supervision UAT |

After item 30, remote execution, migration, channels, marketplace, scheduler, multimodal completion and peer certification become independently shippable product tracks. They should consume the durable lifecycle and policy contracts rather than inventing new ones.

## 11. Release slicing

| Release | Promise | Must include | Must not claim yet |
|---|---|---|---|
| R0 — Honest Core | Advertised capabilities are measurable and autonomous skill behavior is contained | Wave 0 | Frontier, complete recovery, durable background parity |
| R1 — Bounded Core | Smart Default is useful; enterprise floor is non-bypassable; loops and spending are bounded | Wave 1 | Crash-complete, always-on durable agents |
| R2 — Recoverable Core | Interrupted work and provider failure recover safely | Wave 2 | Full OpenClaw gateway/channel parity |
| R3 — Durable Agency | Every child is persistent, governable and transactional | Wave 3 | Whole-product ecosystem parity |
| R4 — Wayland Product Parity | Desktop and standalone surfaces expose the completed Core lifecycle; migration/remote/channel tracks ship | Wave 4 | Frontier until comparative evidence passes |
| R5 — Frontier Evidence | Claims are backed by published repeated cross-platform outcomes | Wave 5 | Nothing beyond measured scope |

The previous 18-24 week solo estimate should not be treated as a commitment. A realistic program is dependency-driven: Waves 0-3 contain a serial safety/recovery spine, while Desktop, TUI, migration, remote backends and channels can parallelize once their contracts stabilize. Progress should be reported by passed release gates, not elapsed calendar estimates or feature percentage.

## 12. Decision metrics

Every release candidate should publish these measures by posture and platform:

- task success and artifact correctness;
- unrecovered crash, wedge and timeout rate;
- duplicate or unknown external side effects after recovery;
- prompts per successful task, plus approvals later shown unnecessary;
- denied actions, with true-positive and false-positive classification;
- time to first useful action and time to verified completion;
- input/output/cache tokens, provider calls and cost;
- compactions, lost-context symptoms and retries;
- peak memory/CPU, child-process count and orphan count;
- filesystem writes outside the declared workspace;
- network destinations attempted, allowed, prompted and denied;
- capability activation failures and silent fallback count;
- Desktop/Core protocol mismatch and reconnect failures;
- cross-session policy/state leakage;
- percentage of claims at E0-E5.

The weighted product score remains:

| Dimension | Weight |
|---|---:|
| Task correctness and completion | 30 |
| Reliability and recovery | 20 |
| Security and authority containment | 20 |
| Usability and cognitive tax | 15 |
| Cross-platform consistency | 10 |
| Performance and cost | 5 |

Critical security escape, silent capability failure, data loss, unrecoverable wedge, or cross-session policy leak is a hard failure before weighting.

## 13. The first two weeks

Do not begin with another broad feature wave. The first implementation slice should be:

1. Make `wayland-eval` execute a small but real packaged-binary corpus and emit versioned JSON.
2. Remove secrets from generated evaluation config and prove canaries never leak.
3. Add the posture field and stop defaulting safety scenarios to Force.
4. Add activation status for `PricingRefresher`, `MidFlightMonitor`, `CooldownTracker`, `LearnedPolicy`, smart handoff, Delegate isolation, and both skill-drafting paths.
5. Gate the legacy drafter and run the complete opt-out matrix.
6. Add deterministic reproductions for the Bash hang, fork wedge, context-death, browser failure/collision and cross-session egress state.
7. Freeze the typed Smart/Managed/Dangerous contract and Desktop/Core event vocabulary before changing permission behavior.

The first demonstration should show one ordinary repository task in Smart Default, one blocked secret/egress escalation, one intentional loop stopped by budget/no-progress policy, one injected crash with explicit recovery state, and the same run observed through both standalone Core and the Desktop JSON-stream host. That demonstration will reveal more about frontier readiness than another hundred static feature checks.

## 14. Final assessment

Wayland Core is closer to frontier at the architecture level than its current product evidence suggests. That is both the opportunity and the warning. Its provider abstraction, orchestration, sandbox/egress work, memory, Anvil transactions, and protocol design are real assets worth protecting. Hermes and OpenClaw should be copied where they are operationally better—durable agents, execution reach, migration, gateways, channels and operator controls—not where they are merely different.

The shortest credible path is:

```text
prove reality -> establish safe defaults -> make every turn recoverable
-> make every child durable and transactional -> expose it cleanly in Desktop/TUI
-> add product ecosystems through signed boundaries -> certify on real workloads
```

If Wayland executes that sequence, security will stop feeling like a layer that trips over the agent. It becomes the reason the agent can act more autonomously: authority is scoped, work is reversible, failures are recoverable, cost is bounded, and the operator is interrupted only when the boundary genuinely changes.
