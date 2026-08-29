---
issue: 305
repo: FerroxLabs/wayland
title: "[Feature]: improve Win/WSL interop"
status: open
last_verified_commit: 43848f75
criteria:
  - id: c1
    text: "acp serve boots in a headless WSL with no OS secret service, persisting its server key rather than hard-exiting"
    state: met
    evidence: "test:crates/wcore-cli/src/acp.rs::headless_first_run_persists_to_the_profile_file_at_0600"
    owner: core
    note: "the key is minted once at 0600 and read back on the next boot, so a Win/WSL client does not re-pair after every Core restart"
  - id: c2
    text: "A project-scoped approval allowlist is reachable over REST so a working directory under an enabled entry auto-resolves instead of gating"
    state: met
    evidence: "test:crates/wcore-acp/tests/project_allowlist_rest.rs::a_session_under_an_enabled_project_never_shows_its_host_a_gate"
    owner: core
    note: "Built wcore_acp::allowlist::ProjectAllowlist plus three REST routes (GET/PUT /v1/approvals/projects, DELETE /v1/approvals/projects/{id}) and an optional 'cwd' on session/create. A session whose cwd is under an ENABLED entry has each ApprovalRequired resolved by the server through the same TurnEngine::resolve_approval path a host would use, and the gate frame is not forwarded, so nothing prompts. Persisted to <home>/acp-projects.json so a grant survives the Core restart #305 is about; a malformed file refuses the launch rather than silently starting empty. cwd is NOT a hint - it becomes the directory the session's engine is built in (EngineTurnEngine::session_for) - so an uncovered path is REFUSED at create; the default list is empty, so every pre-existing deployment is byte-identical (no cwd accepted, nothing auto-approved). Containment is Path::starts_with (component-wise), so /srv/webapp does not cover /srv/webapp-staging; both sides must be absolute and free of . and .. components. Editing the list is Admin in roles::required_role, reading it is Viewer. RED ARM observed on hetzner 2026-08-29: deleted the auto_resolve_approvals call in send_message, touched the file, re-ran - 'a session under an ENABLED project must not prompt; frames=[ToolCall { ... }, ApprovalRequired { ... reason: mutating tool Write requires approval ... }, ToolResult { ... }, Done { ... }]' / 'Summary: 6 tests run: 5 passed, 1 failed'. The five that stayed green include the disabled-entry and no-cwd controls, so the failing assertion discriminates the feature and not the harness. Restored, touched, 174/174 green."
  - id: c3
    text: "An approval or exec timeout surfaces an Error frame instead of hanging silently"
    state: met
    evidence: "test:crates/wcore-acp/tests/turn_stall_disclosure.rs::an_unanswered_approval_gate_is_disclosed_as_an_error_frame"
    owner: core
    note: "AcpServer::guard_stall wraps every turn's event stream: DEFAULT_TURN_STALL_TIMEOUT (600s) with no frame at all ends the stream with one terminal MessageEvent::Error carrying the new ErrorCode::Timeout (-32006) and a message naming what happened. Every frame restarts the clock, so only true silence trips it, and a caller who wants the old behaviour can pass with_turn_stall_timeout(None). It sits BEFORE the tee, so the disclosure is appended to the resumable event log as well as delivered live - a client that dropped mid-stall is told on resume. The disclosure reaches the PROTOCOL STREAM, not a log: the tests read it off a real SSE body over a live listener as a parsed MessageEvent, which a tracing line could never have done (RUST_LOG is unset for ordinary users, so only ERROR reaches stderr and an SSE host sees none of it). RED ARM observed on hetzner 2026-08-29: forced the guard off with 'match None::<Duration>', touched the file, re-ran - an_unanswered_approval_gate_is_disclosed_as_an_error_frame FAILED [20.06s] 'the prompt stream must terminate on its own. Hanging here IS the defect under test: an approval gate nobody answers left the caller with no frame and no ending.: Elapsed(())'; a_tool_that_never_returns_is_disclosed_too FAILED [20.06s]; the_disclosure_is_in_the_resumable_event_log FAILED [20.01s]; 'Summary: 6 tests run: 3 passed, 3 failed'. Each arm carries an outer 20s timeout so the regression fails the suite instead of hanging CI. The two green controls are the completing-turn arm and the guard-disabled known-negative, which proves the disclosure comes from the guard and not from something else in the stack. Restored, touched, 174/174 green."
  - id: c4
    text: "Desktop autodetects a local WSL Core and offers detected-versus-manual endpoint and key settings"
    state: blocked
    owner: desktop
    note: "the probe needs no Core change since /openapi.json is already unauthenticated, but the settings UI and the allowlist popup are Desktop's surface"
---

The reporter runs their web projects inside WSL and Wayland on Windows, and
reading a WSL codebase from the Windows engine hangs. The cause is not
WSL-specific: the sandbox is chosen by host OS rather than by where the files
live, so the Windows AppContainer probe re-runs on every command. Running Core
inside WSL sidesteps it entirely, because Linux uses a different sandbox.

The reporter's design was built on the actual server source, and its key finding
holds: `wayland-core acp serve` already provides REST, SSE, WebSocket, sessions,
approvals and auth, and `/openapi.json` is unauthenticated, so autodetection over
`127.0.0.1` needs no Core change at all.

Three of the four criteria are met (c1, c2, c3); c4 is Desktop's. The server key is now persisted rather than
minted per restart, which is what makes pairing survive a Core restart — an
earlier lane that minted a fresh key each start was dropped rather than merged,
recorded on the issue so nobody re-lands it. The REST allowlist half now exists (c2) and the silent-hang half is disclosed
on the stream (c3). The Desktop half (c4) is still not core's to build: the
probe needs no Core change, and the settings UI and the allowlist popup are
Desktop's surface. Desktop drives the new routes:
`GET/PUT /v1/approvals/projects`, `DELETE /v1/approvals/projects/{id}`, and the
optional `cwd` on `POST /v1/sessions`. All three are in `/openapi.json`.
