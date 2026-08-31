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
    state: met
    evidence: "test:crates/wcore-agent/tests/egress_tool_origin_test.rs::webfetch_of_an_allowlisted_apex_with_a_query_payload_is_refused"
    owner: maintainer
    note: "RE-GRADED blocked -> met by lane f13-authority, against the decision recorded in .planning/DECISIONS.md ('Egress: split the allowlist grant by traffic origin'), which is what FerroxLabs/wayland#1264 c4 asks for instead of leaving this blocked. The decision owed was taken: SPLIT. Tool-driven egress to an allowlisted host is shape-checked on method, path and query; provider traffic to the same apex keeps its unconditional Allow. Measured on the real WebFetch surface, and the red arm was measured LIVE -- with the classify.rs:229 early return restored the payload request was dispatched and answered by github.com with HTTP 200. Full grading is in .planning/ledger/wayland-1264.md; c7 of this ticket remains blocked on Desktop and is untouched."

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
