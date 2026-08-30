---
issue: 1195
repo: FerroxLabs/wayland
kind: defect
title: "Workflows and scheduled tasks run blanket auto-approve on every backend except Claude"
status: open
last_verified_commit: 70a47aaed
criteria:
  - id: c1
    text: "The engine's gating behaviour under each of the three session modes is MEASURED on the real json-stream wire (not inferred from a predicate unit test) and pinned, so a change to any mode's classification reddens"
    state: met
    evidence: "test:crates/wcore-cli/tests/permission_mode_matrix.rs::auto_edit_moves_only_the_builtin_write_edit_pair"
    owner: core
    note: "Three arms, one real binary each, mode applied exactly as Desktop applies it (WAYLAND_ALLOW_WIRE_FORCE=1 + a wire set_mode), verdict read from real stdout: approval_required = gated, call_announced = auto-run. The pre-existing predicate test (wcore_agent::confirm::typed_policy_confirmation_matrix_is_fail_closed) grades ToolConfirmer, which the json-stream path never calls -- there the decision is orchestration's needs_approval and the frame is synthesized by GatingProtocolWriter. Two predicates, one wire; they disagreed (see c4)."
  - id: c2
    text: "`default` gates every class the MODE governs -- info/edit/exec -- and the measurement records that it does NOT gate the shipped `[tools] allow_list`, so the mode's real posture is stated rather than assumed"
    state: met
    evidence: "test:crates/wcore-cli/tests/permission_mode_matrix.rs::default_gates_everything_the_mode_governs"
    owner: core
    note: "Measured: todo -> approval_required(info), Write -> approval_required(edit), Bash -> approval_required(exec). Read and WebFetch auto-run, because default_allow_list ships them. `default` is therefore NOT 'asks about everything' on a stock install."
  - id: c3
    text: "`auto_edit` moves EXACTLY the built-in Write/Edit pair from gated to auto and widens nothing else; `force` gates nothing the mode governs"
    state: met
    evidence: "test:crates/wcore-cli/tests/permission_mode_matrix.rs::force_gates_nothing_the_mode_governs"
    owner: core
    note: "auto_edit is name-scoped, never category-scoped: `is_auto_approved_tool_cmd` auto-approves only `category == edit && name in {Write, Edit}` (file:crates/wcore-protocol/src/lib.rs:547:SessionMode::AutoEdit => {). Measured live: under auto_edit `todo` (Info, not allow-listed) still gates, and it gates carrying `reason: \"info\"` -- which is the composition hazard recorded in c6."
  - id: c4
    text: "A tool the engine parks on REGARDLESS of posture -- AskUserQuestion, and any call the path-boundary classifier escalated -- surfaces its gate frame to the host under `force` too, instead of leaving the engine parked on a request no host was shown"
    state: met
    evidence: "test:crates/wcore-cli/tests/permission_mode_matrix.rs::force_still_surfaces_a_question_to_the_host"
    owner: core
    note: "DEFECT FOUND BY THE MEASUREMENT, live on 70a47aaed before this change. GatingProtocolWriter re-derived parked-ness from the approval POSTURE (is_auto_approved_tool_cmd), which says auto-approved under force, and suppressed the frame -- while orchestration's needs_approval names AskUserQuestion unconditionally and still parked. Measured wire under force: `tool_request` for AskUserQuestion, then silence for the life of the turn; no approval_required, no tool_running, no tool_result. A host keying on approval_required (the D012 gate frame, and what acp_engine's relay projects to ACP clients) waits forever. wcore_agent::confirm::ask_user_question_always_requires_a_host_response passed throughout -- it grades the predicate, not the wire. Fixed at file:crates/wcore-cli/src/main.rs:4246:let parks_regardless_of_posture = by skipping the suppression for the two reasons the engine parks on regardless of posture. The TUI's ChannelEmitter never had the hole: it synthesizes unconditionally."
  - id: c5
    text: "The #1099 path-boundary escalation reaches the host in `default` and `auto_edit`, and its ABSENCE under `force` is a recorded asymmetry rather than an accident"
    state: met
    evidence: "test:crates/wcore-cli/tests/permission_mode_matrix.rs::a_read_outside_the_workspace_escalates_in_every_mode_except_force"
    owner: core
    note: "Measured: default and auto_edit both emit tool_request carrying escalation.kind = path_boundary (target /etc/hostname, suggested_root /etc) plus approval_required(info), even though Read is allow-listed -- the boundary forces the gate past the allow-list. Under force the classifier is suppressed (file:crates/wcore-agent/src/orchestration/mod.rs:3227:let path_boundary = if globally_approved || recovered_approval {) so no card is raised. MEASURED CONSEQUENCE, and it corrects the framing this was reported under: the suppression is NOT a filesystem escape. The read auto-runs and is then refused by the WorkspacePolicy -- 'path \"/etc/hostname\" is outside sandbox root' -- and a Write to a path outside the root is refused the same way under both auto_edit and force (the artifact was checked for on disk and does not exist). The cost is capability loss: under force the user meets the dead-end refusal #1099's escalation exists to replace, and Desktop's own hard-refuse of type path_boundary is unreachable because no card ever arrives. Their WCoreManager.ts already documents this. Left as-is deliberately: making force gate a boundary would change the documented meaning of the mode, and the real fix is that an unattended session should not be in force at all (c7)."
  - id: c6
    text: "The two-layer composition is stated: which tool NAMES core gates under auto_edit but whose CATEGORY the host then auto-approves"
    state: met
    evidence: "file:crates/wcore-protocol/src/lib.rs:547:SessionMode::AutoEdit => {"
    owner: core
    note: "Core gates by NAME under AutoEdit; Desktop's tryAutoApprove (WCoreManager.ts, verified against FerroxLabs/wayland origin/main c889ab33b) approves by CATEGORY -- `type === 'edit' || type === 'info'`. So every tool core deliberately gates under auto_edit whose category is Info or Edit is auto-approved one layer up. The set is derived mechanically from `fn category()` over every `impl Tool for` in the workspace: category in {Info, Edit}, minus {Write, Edit} (the pair the mode itself grants), minus the shipped allow_list (never gated at all), minus AskUserQuestion (Desktop routes it to type 'question' by NAME) and minus any call carrying a path_boundary escalation (routed by escalation, refused in every mode). That leaves, on this tree: Edit-category -- gitlab_api, notion_api, Rollback, text_to_speech, meet_join, meet_leave, meet_say, spotify_devices, spotify_library, spotify_playback, spotify_playlists, spotify_queue; Info-category -- todo, RepoMap, record_episode, Archive, assert_fact, clarify, doc_extract, EnterPlanMode, ExitPlanMode, image_generate, image_inspect, Jsonl, kubectl, linear_api, markdown_table, meet_status, meet_transcript, pdf_extract, postgres_schema, render_artifact, session_search, sql_query, video_analyze. The rule is what makes it complete, not the list: any new tool declaring Info or Edit joins it automatically."
  - id: c7
    text: "Workflow and cron sessions select a declared approval mode instead of inheriting `yoloMode: true`, and the unattended posture drops the 'info' arm of the host-side category match"
    state: blocked
    owner: desktop
    handoff: "FerroxLabs/wayland#1249"
    note: "The product fix is Desktop's (src/process/services/workflow/, src/process/services/cron/). Core's half is c1-c6: the measured matrix that tells them what each mode buys. Recommendation carried in the #1195 comment, with the measurement behind it."
  - id: c8
    text: "Tool-driven egress to a host on the shipped allowlist is shape-checked -- method, path, query -- rather than admitted on the host match alone, so an unattended run cannot exfiltrate to an allow-listed apex (a WebFetch of https://api.anthropic.com/?leak=<secret>) without an approval in any mode"
    state: blocked
    owner: maintainer
    handoff: "FerroxLabs/wayland#1264"
    note: "RE-SCOPED, and the text it replaces is REFUTED. The original c8 -- `network egress is reachable with no approval in EVERY mode, because WebFetch and web ship on the default allow_list` -- conflated two independent gates. (1) The TOOL allow-list entry is deliberate and documented: the network tools reach the network, not the host fs (file:crates/wcore-agent/src/channel_tools.rs:59:SSRF is gated separately by the egress). (2) A SEPARATE egress policy ships ON by default (file:crates/wcore-config/src/config.rs:388:Master switch for the egress gate. On by default.) over a narrow first-party allowlist (file:crates/wcore-agent/src/egress/defaults.rs:8:cover exfil-shaped first-party traffic (provider/tool-backend APIs).); a non-allowlisted destination is Ask or Exfil, never silent. (3) That gate is independent of ApprovalPolicy: the doorbell is attached per session at file:crates/wcore-agent/src/bootstrap.rs:3034:attach the consent doorbell only to this session and bridge_doorbell.rs contains no Bypass, force or ApprovalPolicy token (grep empty; known-positive control on the same file: 7 hits for `fn `). So mode selection was never what decided this, and `reachable with no approval in every mode` was false for every host off the allowlist. TWO BRIEFED FACTS ALSO DO NOT HOLD on this tree and are corrected here, so nobody re-derives them: `install_consent_doorbell` and `approval_surface_available` do not exist anywhere in crates/ (both greps empty against a working control) -- the doorbell is gated on the session owning a policy, not on surface availability; and the gate does NOT fail closed when it cannot ask -- ConsentDecision is Once/Always/No, there is no Unavailable arm and no `refused without asking you` string in crates/ (greps empty, control positive), while file:crates/wcore-agent/src/egress/consent.rs:13:no doorbell is set and a data-less read falls back to *allow* documents the fail-OPEN fallback deliberately, on the argument that the Exfil verdict stays hard-denied regardless. THE REAL RESIDUAL, measured: classify() is handed only method + url (file:crates/wcore-agent/src/egress/policy.rs:150:classify(request.method(), url, &allow) -- headers and body are never read at all) and returns Allow on the host match at file:crates/wcore-agent/src/egress/classify.rs:229:allow.domain_allowed(&registrable) || allow.host_allowed(&host) BEFORE it reads the method (classify.rs line 245) or the path/query (line 254). The shape check exists but is structurally unreachable for allow-listed hosts: file:crates/wcore-agent/src/egress/classify.rs:273:fn get_carries_data has exactly one call site and it sits on the non-allowlisted branch. So every subdomain of the ~40 default WELL_KNOWN_DOMAINS is an unmetered channel, and several of them (github.com, notion.so, linear.app) carry attacker-readable content surfaces. WebFetch cannot POST -- the backend is hardcoded to .get(&req.url) in tool_backends/http_fetch.rs -- but it takes an unrestricted URL string with no query inspection at either layer, so the query is attacker-chosen. This is not a slip: two tests pin the behaviour as intended (classify.rs post_to_allowlisted_host_is_allowed, policy.rs allowlisted_post_is_allowed) and defaults.rs states the allowlist exists precisely to exempt first-party exfil SHAPE. No test anywhere covers allow-listed-host exfiltration (two controlled greps returned zero against positive controls). DECISION OWED, and it is a product call not a lane call: whether to split the allowlist grant in two, so provider/LLM traffic keeps its unconditional Allow while TOOL-driven traffic to the same apex is shape-checked like any other host. It cannot be closed by narrowing the allowlist -- that denies the agent's own LLM POSTs -- and closing it reverses behaviour every existing install depends on plus the two tests above; that is the same class of call already parked on this file for the --i-accept-exfil-risk interlock (config.rs SecurityConfig doc). Egress measured at 70a47aaed; crates/wcore-agent/src/egress/ and web_fetch.rs are byte-identical at 497f0991d."
---

Core's half of wayland#1195, measured.

The ticket's current position (comment 3) was that the fix may be as small as
"have workflow and cron sessions select `default` or `auto_edit` instead of
inheriting `yoloMode: true`", and named the one thing nobody had checked: what
the engine actually gates under each mode. This is that measurement, taken on
the real `--json-stream` wire rather than from the predicate, plus the two
things the measurement exposed.

What the modes buy, measured:

| class (representative)     | `default`     | `auto_edit`   | `force` |
|----------------------------|---------------|---------------|---------|
| Info, allow-listed (Read)  | auto          | auto          | auto    |
| Info, not allow-listed (todo) | GATE info  | GATE info     | auto    |
| Edit (Write)               | GATE edit     | auto          | auto    |
| Exec (Bash)                | GATE exec     | GATE exec     | auto    |
| Mcp, allow-listed (WebFetch) | auto        | auto          | auto    |
| Read outside the workspace | GATE + path_boundary | GATE + path_boundary | auto, then refused by the workspace policy |
| AskUserQuestion            | GATE info     | GATE info     | GATE info (only after the c4 fix) |

`auto_edit` is the mode the ticket hoped for and it does what its name says:
file edits proceed, exec stops. Two caveats decide how much that is worth, and
both are recorded above -- the shipped allow_list means no mode gates reads or
network fetches, and the host's category-shaped auto-approval widens
`auto_edit` well past what core granted (c6).

The network half of that caveat is smaller than it first looked, and c8 now
records the correction rather than the original claim. A second, independent
gate -- the egress policy, on by default and unaffected by session mode --
already Asks or denies for every host off a narrow first-party allowlist, so
no mode ever reached an arbitrary URL unprompted. What survives is narrower and
real: `classify` returns Allow on the host match before it looks at the method
or the query, so an allow-listed apex takes any shape silently. Closing that is
a maintainer decision, not a lane change -- see c8.
