//! Protocol bridge — drains engine `ProtocolEvent`s into `App`.
//!
//! FROZEN Wave-0 signature; T0.5 implements the body.
//!
//! The TUI consumes the same `ProtocolEvent` stream as the `--json-stream`
//! host protocol, but in-process: `AgentEngine` emits events over a
//! `tokio::mpsc` channel and `spawn_bridge` is the single task that
//! decodes every variant into mutations on the shared `Arc<Mutex<App>>`.
//! Surfaces never touch the channel — they only read `App`.
//!
//! The decode itself is the pure function [`apply_event`]: it takes
//! `&mut App` and one `ProtocolEvent` and performs the corresponding view
//! mutation with no I/O. `spawn_bridge` is the thin async shell — it just
//! loops `recv()` and forwards each event to `apply_event` under the lock.
//! Keeping the decode pure makes the whole event→view mapping unit-
//! testable against the T0.5 fixtures without a live channel or runtime.

use std::sync::{Arc, Mutex};
use std::time::Instant;

use tokio::sync::Notify;
use tokio::sync::mpsc::UnboundedReceiver;
use wcore_protocol::events::{ProtocolEvent, ToolStatus};
use wcore_types::message::{ContentBlock, Message, Role};

use crate::tui::anim::AnimId;
use crate::tui::app::{
    App, DiffModel, StreamingPhase, SubAgentStatus, SubAgentView, ToolCardModel, ToolCardStatus,
    TurnRole, TurnView, WorkflowNodeView, WorkflowView,
};
use crate::tui::render::markdown::render_markdown;
use crate::tui::theme::Theme;
use crate::tui::tool_formatters::formatter_for;
use crate::tui::turn_element::TurnElement;

/// Per-turn cap for the Sources block. Matches `widgets::sources_block::
/// MAX_SOURCES` — declared here too so the bridge can drop overflow
/// before pushing the element, keeping `TurnView` storage bounded.
const SOURCES_MAX_PER_TURN: usize = 10;

/// Spawn the bridge task: drain `engine_rx` and apply each
/// `ProtocolEvent` to the shared `App`.
///
/// The channel is unbounded so a burst of events during a turn never
/// back-pressures the engine task. The bridge task ends cleanly when
/// every sender half is dropped (`recv()` returns `None`).
///
/// v0.9.2 audit H2 (idle-wake latency): `wake` is the render loop's
/// idle-wake signal. The render loop owns `engine_rx` indirectly (the
/// bridge does), so it cannot `select!` on the channel directly; instead,
/// the bridge calls `wake.notify_one()` after every applied event, and the
/// loop's idle `select!` has a `wake.notified()` arm. A bridge push at a
/// fully-idle prompt (first streamed token, an error turn, a budget toast)
/// then wakes the loop within ~one frame instead of waiting out the up-to-
/// 200ms `IDLE_SLICE` input poll. `notify_one` is cheap and a stored
/// permit means a notify that races ahead of the `notified()` await is not
/// lost — the next idle park returns immediately.
pub fn spawn_bridge(
    mut engine_rx: UnboundedReceiver<ProtocolEvent>,
    app: Arc<Mutex<App>>,
    wake: Arc<Notify>,
) {
    tokio::spawn(async move {
        while let Some(event) = engine_rx.recv().await {
            // A poisoned lock means a surface panicked mid-mutation; the
            // TUI is already unwinding, so there is nothing useful to do
            // but stop draining. Recover the inner guard from the
            // `PoisonError` so the streaming spinner is still cleared —
            // a wedged "working" indicator must never be a panic's
            // legacy (AUDIT-D D2, defense in depth behind D1).
            match app.lock() {
                Ok(mut guard) => apply_event(&mut guard, event),
                Err(poisoned) => {
                    poisoned.into_inner().session.streaming_active = false;
                    wake.notify_one();
                    break;
                }
            }
            // Wake the idle render loop so this mutation paints promptly
            // rather than waiting out the next idle input-poll slice.
            wake.notify_one();
        }
        // The engine event channel closed — every sender half is gone, so
        // no terminal `StreamEnd` can ever arrive now. Force the streaming
        // state off as a final mutation so the spinner cannot stick on
        // (AUDIT-D D2). A poisoned lock here is recovered for the same
        // reason: the flag matters more than the poison.
        match app.lock() {
            Ok(mut guard) => guard.session.streaming_active = false,
            Err(poisoned) => poisoned.into_inner().session.streaming_active = false,
        }
    });
}

/// Decode one `ProtocolEvent` into the matching `App` mutation.
///
/// Pure with respect to I/O: every effect is a field write on `app`. This
/// is the single authority for the event → view mapping and the unit-test
/// seam — feeding a fixture stream through `apply_event` must drive `App`
/// to the expected state. Variants with no view impact are explicit
/// no-ops so an unknown-but-valid event never panics the bridge.
pub fn apply_event(app: &mut App, event: ProtocolEvent) {
    apply_event_inner(app, event);
    // D009 (render-livelock): keep the retained transcript bounded so the
    // per-frame re-wrap the render loop pays under the App mutex cannot grow
    // without bound on a long/chatty run. `trim_history` is a no-op while a
    // turn is in flight or while under the cap, so a normal session never
    // pays for it — it only rolls oldest turns off the front once the
    // transcript has genuinely run long.
    app.session.trim_history();
}

/// The event → view decode body. Split out so [`apply_event`] can apply the
/// post-event transcript trim ([`crate::tui::app::SessionView::trim_history`])
/// once, after any branch that appends a turn, without threading the trim
/// through every arm.
fn apply_event_inner(app: &mut App, event: ProtocolEvent) {
    match event {
        // ── Streaming lifecycle ──────────────────────────────────────
        ProtocolEvent::StreamStart { .. } => {
            app.session.streaming_active = true;
            app.session.streaming.clear();
            app.session.thinking.clear();
            // D037: mark how many files the session had already touched when
            // this turn opened. `touched_files()` is session-cumulative +
            // deduped; the slice from this watermark onward at StreamEnd is
            // exactly the set THIS turn touched, so the post-turn "files
            // changed" card lists only this turn's deliverable, not the whole
            // session. `record_touched_file` only appends, so the watermark
            // stays a valid prefix length for the lifetime of the turn.
            app.mark_touched_files_watermark();
            // Reset the reasoning filter so a runaway unclosed `<think>`
            // block from a prior turn (or a previous cancelled stream)
            // cannot suppress the visible output of the new turn.
            app.session.reasoning_filter.reset();
            // W3 D3: enter the Thinking phase + reset turn clock + delta
            // watchdog + per-turn token counter.
            let now = Instant::now();
            app.session.phase = StreamingPhase::Thinking;
            app.session.phase_started_at = now;
            app.session.turn_started_at = now;
            app.session.last_delta_at = now;
            app.session.tokens_out = 0;
            // v0.9.2 W6 (SPEC §4): pick a fresh per-turn verb seed ONCE here.
            // A Knuth multiplicative hash over the previous seed gives a
            // deterministic-yet-varying nonce so consecutive turns land on
            // different pool indices without an RNG dependency. The streaming
            // status widget reads `turn_verb_seed` to keep its verb constant
            // for the whole turn (replaces the old time-based rotation).
            app.session.turn_verb_seed = app
                .session
                .turn_verb_seed
                .wrapping_add(1)
                .wrapping_mul(2_654_435_761);
            // v0.9.2 W1 (Task 1.5, SPEC §1A): a turn is in flight, so the
            // single shared animation clock must tick. Subscribe the spinner
            // + streaming-status widgets; StallLerp is read by W6's color
            // lerp after a no-delta stall (subscribing here is harmless and
            // keeps the clock alive for it). StreamEnd / Error / cancel
            // release these so an idle prompt drops back to zero-CPU dwell.
            app.anim.subscribe(AnimId::Spinner, false);
            app.anim.subscribe(AnimId::StreamingStatus, false);
            app.anim.subscribe(AnimId::StallLerp, false);
        }
        ProtocolEvent::TextDelta { text, .. } => {
            // Ignore a delta that arrives while no stream is active. A
            // cancelled turn's task can emit one last `TextDelta` after
            // `cancel()` has already sent the synthetic `StreamEnd`
            // (`abort()` is asynchronous — AUDIT-D D7); appending it here
            // would leave a stale fragment in `streaming` that no
            // `StreamEnd` flushes. Dropping it keeps the cancel clean.
            if app.session.streaming_active {
                // Route the delta through the per-session reasoning
                // filter so `<think>`/`<reasoning>`/`<thinking>` blocks
                // emitted by raw open-weights models (DeepSeek-R1, etc.)
                // never reach the visible streaming buffer. The filter
                // handles tags that split across token-chunk boundaries.
                let filtered = app.session.reasoning_filter.process(&text);
                app.session.streaming.push_str(&filtered);
                // W3 D3: any visible delta transitions the phase to
                // Drafting (resets WrappingUp if a stalled stream resumed)
                // and refreshes the delta watchdog.
                let now = Instant::now();
                if app.session.phase != StreamingPhase::Drafting {
                    app.session.phase = StreamingPhase::Drafting;
                    app.session.phase_started_at = now;
                }
                app.session.last_delta_at = now;
            }
        }
        ProtocolEvent::Thinking { text, .. } => {
            // Same late-event guard as `TextDelta` (AUDIT-D D7).
            if app.session.streaming_active {
                app.session.thinking.push_str(&text);
                // W3 D3: only re-enter Thinking from Idle — a stray Thinking
                // event mid-drafting should not flip the label back.
                if app.session.phase == StreamingPhase::Idle {
                    app.session.phase = StreamingPhase::Thinking;
                    app.session.phase_started_at = Instant::now();
                }
            }
        }
        ProtocolEvent::StreamEnd { usage, .. } => {
            // W3 D3: roll up output_tokens from the optional Usage payload
            // before flushing — the status widget shows the per-turn total
            // just before phase → Idle.
            if let Some(u) = usage.as_ref() {
                app.session.tokens_out = app.session.tokens_out.saturating_add(u.output_tokens);
            }
            // ── v0.9.3 W1.3 — persist captured reasoning as Thinking ─────
            // The reasoning filter at `:154` strips <think>/<reasoning>/
            // <thinking> blocks from the visible stream; v0.9.3 W1.2
            // extended the filter to also accumulate the stripped content
            // into a capture buffer. Drain it here. `secs` is the turn-level
            // proxy (`now − turn_started_at`) per SPEC v1.3 §0 criterion
            // #11; `tokens` is `StreamEnd.usage.output_tokens` (default 0
            // when usage is None — the projection gates the `· N tok` meta
            // on `tokens > 0` so a 0 reads cleanly without tail meta).
            let captured = app.session.reasoning_filter.take_captured();
            let thinking_to_push: Option<TurnElement> = if captured.is_empty() {
                None
            } else {
                let now = Instant::now();
                let secs = now.duration_since(app.session.turn_started_at).as_secs();
                let tokens = usage.as_ref().map(|u| u.output_tokens).unwrap_or(0);
                Some(TurnElement::Thinking {
                    body: captured,
                    secs,
                    tokens,
                })
            };
            // D037: build the post-turn "files changed" card from the files
            // this turn touched (the cumulative `touched_files()` slice from
            // the StreamStart watermark onward). Computed once here so each
            // turn-completion branch below can attach it to the same assistant
            // turn it lands the Markdown/Sources/Thinking on, keeping the
            // deliverable a single reviewable unit. `None` when the turn
            // touched no files (a chat-only turn shows no empty card).
            let files_changed_to_push: Option<TurnElement> = files_changed_this_turn(app);
            // Flush whatever streamed into a completed assistant turn as a
            // single `TurnElement::Markdown`. An empty buffer can
            // legitimately occur (tool-only turn); skip the empty flush so
            // the transcript stays clean. The live `thinking` buffer is
            // ephemeral live-state only — it is cleared here without ever
            // landing on the turn's elements. W2 may later split the
            // streamed body at safe-split points; A2 keeps it as one
            // Markdown element per turn.
            //
            // W3 D4: in addition to the Markdown element, build a
            // `TurnElement::Sources(urls)` from (a) inline markdown link
            // URLs in the streamed body and (b) the per-tool formatter
            // `extract_urls` over every tool card on this turn. Merged +
            // deduped + capped — see [`collect_turn_urls`].
            // v0.9.1.2 F12: if an in-flight assistant turn was opened
            // mid-stream (because a ToolRequest interleaved tool cards
            // with text), append any remaining streaming text + the
            // sources element to THAT turn instead of pushing a new one.
            // Otherwise fall back to the original push-a-fresh-turn path.
            if let Some(idx) = app.session.in_flight_turn_idx {
                let body = std::mem::take(&mut app.session.streaming);
                let urls = collect_turn_urls(&body, &app.session.tool_cards);
                if !body.is_empty() {
                    app.session.turns[idx]
                        .elements
                        .push(TurnElement::Markdown(body));
                }
                if !urls.is_empty() {
                    app.session.turns[idx]
                        .elements
                        .push(TurnElement::Sources(urls));
                }
                // v0.9.3 W1.3 — append captured reasoning as a Thinking
                // element on the same in-flight assistant turn. Direct
                // field mutation per the diagnostics.rs:717 precedent
                // (and the Markdown/Sources pushes directly above).
                if let Some(thinking) = thinking_to_push {
                    app.session.turns[idx].elements.push(thinking);
                }
                // D037 — the "files changed" card rides last on the same
                // in-flight turn so it reads as the turn's footer.
                if let Some(files) = files_changed_to_push {
                    app.session.turns[idx].elements.push(files);
                }
                app.session.in_flight_turn_idx = None;
            } else if !app.session.streaming.is_empty() {
                let body = std::mem::take(&mut app.session.streaming);
                let urls = collect_turn_urls(&body, &app.session.tool_cards);
                let mut elements: Vec<TurnElement> = Vec::with_capacity(
                    1 + usize::from(!urls.is_empty())
                        + usize::from(thinking_to_push.is_some())
                        + usize::from(files_changed_to_push.is_some()),
                );
                elements.push(TurnElement::Markdown(body));
                if !urls.is_empty() {
                    elements.push(TurnElement::Sources(urls));
                }
                // v0.9.3 W1.3 — captured reasoning rides on the same
                // freshly-pushed assistant turn.
                if let Some(thinking) = thinking_to_push {
                    elements.push(thinking);
                }
                // D037 — "files changed" card as the turn's footer.
                if let Some(files) = files_changed_to_push {
                    elements.push(files);
                }
                app.session.turns.push(TurnView {
                    role: TurnRole::Assistant,
                    elements,
                });
            } else {
                // Tool-only turn (no streamed body) AND no in-flight turn
                // (rare — a tool call without any preceding text would
                // have opened one). Push a Sources-only turn so any URLs
                // produced via tool results aren't lost. An empty URL set
                // keeps the transcript clean — no phantom turn is
                // created.
                //
                // v0.9.3 W1.3 — if reasoning was captured even though the
                // visible stream is empty (a reasoning-only turn), surface
                // it so the user still sees what the model thought.
                let urls = collect_turn_urls("", &app.session.tool_cards);
                let mut elements: Vec<TurnElement> = Vec::new();
                if !urls.is_empty() {
                    elements.push(TurnElement::Sources(urls));
                }
                if let Some(thinking) = thinking_to_push {
                    elements.push(thinking);
                }
                // D037 — a file-only turn (tools edited files, no streamed
                // body) still gets a "files changed" card; this makes
                // `elements` non-empty so the turn is pushed and the
                // deliverable is visible rather than buried.
                if let Some(files) = files_changed_to_push {
                    elements.push(files);
                }
                if !elements.is_empty() {
                    app.session.turns.push(TurnView {
                        role: TurnRole::Assistant,
                        elements,
                    });
                }
                app.session.streaming.clear();
            }
            app.session.thinking.clear();
            app.session.streaming_active = false;
            // W3 D3: terminal phase → Idle (the widget reads Idle as "do
            // not render the working line at all").
            app.session.phase = StreamingPhase::Idle;
            app.session.phase_started_at = Instant::now();
            // v0.9.2 W1 (Task 1.5): the turn is done — release the clock
            // subscriptions so `wants_tick()` goes false and the render loop
            // drops to its idle long-dwell (zero-CPU at the prompt). Cancel
            // routes through a synthetic StreamEnd, so this also covers it.
            app.anim.unsubscribe(AnimId::Spinner);
            app.anim.unsubscribe(AnimId::StreamingStatus);
            app.anim.unsubscribe(AnimId::StallLerp);
            // D019: a completed turn is a `/rewind` checkpoint. Snapshot the
            // files the agent touched this session so the user can restore to
            // this point. Best-effort: a capture failure (i/o, permissions)
            // must never panic or abort the turn — a missing checkpoint is
            // degraded, not fatal. Skip entirely when nothing was touched so
            // a chat-only turn leaves no empty checkpoint.
            capture_turn_checkpoint(app);
        }

        // ── Tool-call lifecycle ──────────────────────────────────────
        ProtocolEvent::ToolRequest { call_id, tool, .. } => {
            let summary = summarize_args(&tool.name, &tool.args);
            let edit_preview = edit_preview_from_args(&tool.name, &tool.args);
            // Host-derive the path map: any file-touching tool feeds the
            // right-rail tree. There is no engine event for a path map —
            // it is reconstructed from tool args here (per AUDIT).
            if let Some(path) = touched_path(&tool.name, &tool.args) {
                app.path_map.insert_path(&normalize_path(&path));
                // D019/B1b: STAGE the real on-disk path for `/rewind`, keyed by
                // call_id — do NOT record it yet. The path_map node above is
                // project-relative (for display) and shows the requested file
                // immediately; the checkpoint store, however, must only snapshot
                // files that were actually approved and run. A tool whose
                // approval is denied (or that is cancelled pre-run) would
                // otherwise have its path captured at turn end. The staged path
                // is promoted to the on-disk touched-files set at `ToolRunning`
                // (post-approval) and dropped at `ToolCancelled`.
                app.stash_pending_touch(call_id.clone(), std::path::PathBuf::from(&path));
            }
            // The engine surfaces plan mode as an `EnterPlanMode` tool
            // call; capture its payload so the router can switch to the
            // plan-review surface. `ExitPlanMode` clears it again.
            // v0.9.2 W11-integ (SPEC §2 #8): `ExitPlanMode` is the SAME
            // event that both clears the live plan AND creates the
            // ExitPlanMode tool card. Snapshot the plan body BEFORE the
            // clear so the card carries it (the component reads
            // `card.plan_body`, never the live `app.plan`). All other
            // tools leave the captured body `None`.
            let mut captured_plan_body: Option<String> = None;
            match tool.name.as_str() {
                "EnterPlanMode" => app.plan = Some(plan_from_args(&tool.args)),
                "ExitPlanMode" => {
                    captured_plan_body = app.plan.as_ref().map(|p| p.body.clone());
                    app.plan = None;
                }
                _ => {}
            }
            let input_pretty = pretty_input(&tool.args);
            // v0.9.1.2 F12: tool cards must land inline with the assistant
            // text that introduced them — not in a trailing block at the
            // end of the transcript. Flush any accumulated streaming text
            // as a `Markdown` element FIRST, then push a `ToolCard(call_id)`
            // element at this position so the renderer walks element order.
            // The in-flight turn is opened lazily here if a TextDelta
            // hasn't already done so.
            flush_streaming_into_in_flight_turn(app);
            let turn_idx = ensure_in_flight_assistant_turn(app);
            app.session.turns[turn_idx]
                .elements
                .push(TurnElement::ToolCard(call_id.clone()));
            // W3 D3: a tool request transitions phase to CallingTool so
            // the status widget shows "calling <tool>" while the request
            // is in flight (before approval / before execution begins).
            app.session.phase = StreamingPhase::CallingTool(tool.name.clone());
            app.session.phase_started_at = Instant::now();
            app.session.tool_cards.push(ToolCardModel {
                call_id,
                tool_name: tool.name,
                summary,
                // A tool that needs approval surfaces as `AwaitingApproval`
                // once the engine sends `ApprovalRequired`; until then the
                // request itself reads as `Running` (the engine has
                // accepted the call). The downstream `ApprovalRequired`
                // handler downgrades the matching card if approval is
                // actually pending.
                status: ToolCardStatus::Running,
                output: None,
                edit_preview,
                input_pretty,
                approval_reason: String::new(),
                // v0.9.2 W4/W11-integ: ExitPlanMode carries the plan body
                // snapshotted just above (before `app.plan` was cleared);
                // all other tools leave it None.
                plan_body: captured_plan_body,
                crucible_plan: None,
            });
        }
        ProtocolEvent::ToolRunning {
            call_id, tool_name, ..
        } => {
            // B1b: the tool has cleared approval and is now executing, so the
            // path staged at `ToolRequest` is a genuine touch — promote it into
            // the `/rewind` touched-files set. A denied tool never reaches this
            // arm, so its staged path is never recorded.
            app.promote_pending_touch(&call_id);
            if let Some(card) = find_card(app, &call_id) {
                card.status = ToolCardStatus::Running;
            }
            // W3 D3: transition to RunningTool — the widget now shows
            // "running <tool>" while execution is in flight. Read the
            // tool name from the event directly so we avoid a card lookup
            // on the hot path.
            app.session.phase = StreamingPhase::RunningTool(tool_name);
            app.session.phase_started_at = Instant::now();
        }
        ProtocolEvent::ToolResult {
            call_id,
            status,
            output,
            ..
        } => {
            // B1b: a tool that produced a result definitely executed
            // post-approval, so promote its staged path here too. This is the
            // safety net for any path where `ToolRunning` is skipped; promotion
            // is idempotent, so a tool that already promoted at `ToolRunning` is
            // a no-op. A denied tool emits `ToolCancelled` (drop), never a
            // `ToolResult`, so this never records a denied touch.
            app.promote_pending_touch(&call_id);
            if let Some(card) = find_card(app, &call_id) {
                card.status = match status {
                    ToolStatus::Success => ToolCardStatus::Ok,
                    ToolStatus::Error => ToolCardStatus::Err,
                };
                card.output = Some(output);
            }
        }
        ProtocolEvent::ToolChunk { call_id, chunk, .. } => {
            if let Some(card) = find_card(app, &call_id) {
                card.output.get_or_insert_with(String::new).push_str(&chunk);
            }
        }
        ProtocolEvent::ToolCancelled { call_id, .. } => {
            // B1b: a cancelled/denied tool never ran, so drop its staged path
            // without recording it — it must not be snapshotted at turn end.
            app.drop_pending_touch(&call_id);
            if let Some(card) = find_card(app, &call_id) {
                card.status = ToolCardStatus::Cancelled;
            }
        }
        ProtocolEvent::ToolPanicked {
            call_id,
            panic_message,
            ..
        } => {
            // A panic is a tool failure: mark the card `Err` and record
            // the panic text as its output so the card stays informative.
            if let Some(card) = find_card(app, &call_id) {
                card.status = ToolCardStatus::Err;
                card.output = Some(format!("Tool panicked: {panic_message}"));
            }
        }
        ProtocolEvent::ApprovalRequired {
            call_id,
            reason,
            plan,
            ..
        } => {
            // Mark the matching card as awaiting the user's decision and
            // park the reason on the card so the approval modal can show
            // it. If the card has not arrived yet (request/approval
            // reorder), record the reason as a system notice so it is not
            // lost — the next `ToolRequest` for the same call_id will
            // pick up `AwaitingApproval` directly.
            //
            // v0.9.1.2 F14: also drive the discoverability triplet —
            // (1) phase → AwaitingApproval so the status widget stops
            // saying "Brewing/Calling" and tells the user input is
            // required, (2) force-scroll flag so the awaiting card is
            // pulled into view if the transcript was anchored elsewhere,
            // (3) terminal bell so the OS notification surfaces the
            // pending decision in an unfocused window. Without this
            // bundle the user can sit indefinitely at a "Brewing… 4m 31s"
            // line while the engine quietly waits for `y`.
            let tool_name = match find_card(app, &call_id) {
                Some(card) => {
                    card.status = ToolCardStatus::AwaitingApproval;
                    card.approval_reason = reason;
                    // Crucible Stage 4: snapshot the typed proposal card off
                    // the event so the CrucibleComponent renders it from the
                    // card model (mirrors how `approval_reason` is parked here).
                    // `None` for every non-council approval.
                    card.crucible_plan = plan;
                    Some(card.tool_name.clone())
                }
                // B2.5 — an egress consent (`egress:` call_id) has no tool card
                // of its own. Synthesize an AwaitingApproval card so the
                // existing y(once)/a(always)/n(deny) approval UI engages; the
                // `egress:` prefix routes the decision to the egress bridge
                // (see `TuiEngine::approve`).
                None if call_id.starts_with("egress:") => {
                    app.session.tool_cards.push(ToolCardModel {
                        call_id: call_id.clone(),
                        tool_name: "egress".to_string(),
                        summary: reason.clone(),
                        status: ToolCardStatus::AwaitingApproval,
                        output: None,
                        edit_preview: None,
                        input_pretty: String::new(),
                        approval_reason: reason,
                        plan_body: None,
                        crucible_plan: None,
                    });
                    Some("egress".to_string())
                }
                None => {
                    push_system(app, format!("Approval required: {reason}"));
                    None
                }
            };
            if let Some(tool) = tool_name {
                let pending_count = app
                    .session
                    .tool_cards
                    .iter()
                    .filter(|c| matches!(c.status, ToolCardStatus::AwaitingApproval))
                    .count();
                app.session.phase = StreamingPhase::AwaitingApproval {
                    tool,
                    pending_count,
                };
                app.session.phase_started_at = Instant::now();
                app.force_scroll_to_pending_approval = true;
                // Best-effort terminal bell — `\x07` flashes/beeps on most
                // terminals. We use `print!` to stderr deliberately so it
                // does not interleave with the ratatui frame on stdout.
                // Flushing failure is silent: a missing bell is non-fatal,
                // the on-screen badge + phase label still telegraph the
                // pending state.
                eprint!("\x07");
                let _ = std::io::Write::flush(&mut std::io::stderr());
            }
        }

        // ── Sub-agents ───────────────────────────────────────────────
        ProtocolEvent::SubAgentEvent {
            parent_call_id,
            agent_name,
            inner,
        } => {
            apply_sub_agent_event(app, parent_call_id, agent_name, inner);
        }

        // ── Workflows (ForgeFlows-Live lifecycle) ────────────────────
        ProtocolEvent::WorkflowStarted {
            workflow_id, name, ..
        } => {
            // One view per run, keyed by `workflow_id`. ADOPT the last view if
            // it is unfinished AND either still pending (a node arrived first,
            // order-tolerance) OR already this run's id; otherwise PUSH a new
            // view so each sequential run gets its own (fixes the merge bug
            // where a second run reused the first's view).
            let adopt = app.workflows.last().is_some_and(|w| {
                w.finished.is_none() && (w.key == PENDING_WORKFLOW_KEY || w.key == workflow_id)
            });
            if adopt {
                let view = app.workflows.last_mut().expect("adopt implies non-empty");
                view.key = workflow_id;
                view.name = name;
                view.finished = None;
            } else {
                app.workflows.push(WorkflowView {
                    key: workflow_id,
                    name,
                    nodes: Vec::new(),
                    finished: None,
                });
            }
        }
        ProtocolEvent::WorkflowFinished {
            workflow_id,
            succeeded,
        } => {
            // Resolve the run by its `workflow_id`; fall back to the last
            // unfinished view if the started event was never seen.
            let idx = app
                .workflows
                .iter()
                .position(|w| w.key == workflow_id)
                .or_else(|| app.workflows.iter().rposition(|w| w.finished.is_none()));
            if let Some(idx) = idx {
                app.workflows[idx].finished = Some(succeeded);
            }
        }

        // ── Config / context / session ───────────────────────────────
        ProtocolEvent::ConfigChanged { capabilities } => {
            app.config.memory_enabled = capabilities.non_destructive_compact;
            // `mcp` advertisement does not map onto a `ConfigView` field;
            // the meaningful config snapshot fields (provider/model) are
            // not carried by `Capabilities`, so only what is present is
            // applied. Provider/model updates ride on `Info`/engine wiring.
        }
        ProtocolEvent::SessionCost {
            session_id,
            total_cost_usd,
            per_turn,
        } => {
            // v0.9.1 W1-B: cost is a status concern, not a transcript
            // event. The status bar reads from `app.cost.total_cost_usd`
            // and the diagnostics `/cost` screen renders the per-turn
            // breakdown. Emitting a system-turn for every cost update
            // (one per assistant turn) spammed the transcript with
            // "Session cost: $0.0034 across 1 turn(s)" lines that buried
            // the conversation. Drop the transcript emission; keep the
            // structured payload on `App`.
            //
            // v0.9.2 W10 (SPEC §1B): route the write through the transient
            // `Store` so an identical cost payload is a no-op (no redraw) and
            // a real change bumps `transient_revision`. `set_transient`
            // mirrors the new value onto the canonical `app.cost` field that
            // the status bar / `/cost` screen read.
            let next_cost = Some(crate::tui::app::SessionCostView {
                session_id,
                total_cost_usd,
                per_turn: per_turn
                    .into_iter()
                    .map(|t| crate::tui::app::TurnCostView {
                        turn: t.turn,
                        model: t.model,
                        provider: t.provider,
                        cost_usd: t.cost_usd,
                    })
                    .collect(),
            });
            app.set_transient(|prev| crate::tui::state::TransientSlice {
                cost: next_cost.clone(),
                ..prev.clone()
            });
        }
        ProtocolEvent::McpReady { name, tools } => {
            // v0.9.1 W1-B: MCP readiness is a status concern, not a
            // transcript event. Record it on `app.mcp_status` so
            // `/doctor` and the right-rail Activity panel can surface it
            // without polluting the conversation. A burst of
            // `McpReady` events at session start used to emit one
            // "MCP server X ready — N tool(s)" turn each, drowning the
            // first user message.
            let tool_count = tools.len();
            // v0.9.2 W5 (Task 5.6, SPEC §3 S7): surface readiness as a
            // transient status-bar toast rather than a transcript turn. The
            // status bar renders `app.toast` for a short dwell then clears it
            // (auto-dismiss rides the existing tick via `toast_at`). The
            // structured `mcp_status` write stays — `/doctor` and the
            // right-rail Activity panel still read the count from there.
            //
            // v0.9.2 W10 (SPEC §1B): route the toast + mcp_status write (the
            // WIRE-BRIDGE toast emit, reconciled) through the transient
            // `Store` in ONE atomic `set_transient` so they bump
            // `transient_revision` together. `set_transient` mirrors the new
            // toast / toast_at / mcp_status onto the canonical fields the
            // status bar + `/doctor` read. A `now()` timestamp always
            // differs, so a readiness toast is never a no-op (it should
            // always paint), while a repeated identical cost payload still
            // short-circuits.
            let toast_msg = format!("{name} ready · {tool_count} tools");
            let now = Instant::now();
            app.set_transient(|prev| {
                let mut mcp_status = prev.mcp_status.clone();
                mcp_status.insert(
                    name.clone(),
                    crate::tui::app::McpServerStatus::Ready { tool_count },
                );
                crate::tui::state::TransientSlice {
                    toast: Some(toast_msg.clone()),
                    toast_at: Some(now),
                    mcp_status,
                    ..prev.clone()
                }
            });
        }
        ProtocolEvent::McpFailed { name, reason } => {
            // Companion to McpReady: record the failure (with its cause) on
            // `mcp_status` so `/doctor` can surface *why* the server's tools
            // never appeared, plus a transient toast. Not a transcript turn —
            // a broken server is a status concern, not conversation.
            let toast_msg = format!("{name} failed · {reason}");
            let now = Instant::now();
            app.set_transient(|prev| {
                let mut mcp_status = prev.mcp_status.clone();
                mcp_status.insert(
                    name.clone(),
                    crate::tui::app::McpServerStatus::Failed {
                        reason: reason.clone(),
                    },
                );
                crate::tui::state::TransientSlice {
                    toast: Some(toast_msg.clone()),
                    toast_at: Some(now),
                    mcp_status,
                    ..prev.clone()
                }
            });
        }
        ProtocolEvent::BudgetExceeded {
            reason,
            observed,
            limit,
        } => {
            push_system(
                app,
                format!("Budget exceeded ({reason}): {observed} > {limit}"),
            );
        }

        // ── Notices ──────────────────────────────────────────────────
        ProtocolEvent::Error { error, .. } => {
            // v0.9.1 W2 cycle-2 BLOCKER 3: providers (Anthropic, OpenAI,
            // etc.) embed a full JSON envelope in `error.message` for HTTP
            // failures — e.g. `API error 401: {"type":"error","error":
            // {"type":"authentication_error","message":"invalid x-api-key"},
            // "request_id":"req_..."}`. Rendered raw, the transcript dumps
            // the whole envelope; the user has to read braces to find the
            // one human-readable line. Sanitize: peel the JSON if present
            // and surface the inner `.error.message` (or `.message`) only.
            let pretty = sanitize_provider_error_message(&error.message);
            // v0.9.1.1 H3: previously rendered as `Error [engine_error]:
            // …` / `Error [engine_panic]: …`. The class name is an
            // internal enum, not user-facing — `engine_error` reads as
            // jargon ("what's an engine error?") while `engine_panic`
            // implies a crash. Map known internal classes to a clean
            // user-facing label; route the full class + message to
            // tracing so the log file still carries the diagnostic
            // signal for `/doctor` / `~/.wayland/logs`. Unknown codes
            // fall back to the raw chip so a new error class shows up
            // in the rail rather than vanishing silently.
            tracing::debug!(
                target: "wcore_cli::tui::error",
                code = %error.code,
                retryable = error.retryable,
                "engine error event: {}",
                pretty
            );
            let rail_label = error_class_label(&error.code);
            let rendered = match rail_label {
                Some(label) => format!("{label}: {pretty}"),
                None => format!("Error [{}]: {}", error.code, pretty),
            };
            push_system(app, rendered);
            // An `Error` is a terminal outcome for the turn: clear the
            // streaming state so the spinner cannot stick on if no
            // `StreamEnd` follows (AUDIT-D D2). Flush any partial stream
            // into a turn first so streamed-then-errored output is not
            // lost. `StreamEnd`, when it does follow, then finds an empty
            // buffer and is a clean no-op.
            // v0.9.1.2 F12: same in-flight-turn handling as StreamEnd —
            // partial text on an interleaved turn is appended; partial
            // text without an in-flight turn opens a fresh one.
            if let Some(idx) = app.session.in_flight_turn_idx {
                let body = std::mem::take(&mut app.session.streaming);
                let urls = collect_turn_urls(&body, &app.session.tool_cards);
                if !body.is_empty() {
                    app.session.turns[idx]
                        .elements
                        .push(TurnElement::Markdown(body));
                }
                if !urls.is_empty() {
                    app.session.turns[idx]
                        .elements
                        .push(TurnElement::Sources(urls));
                }
                app.session.in_flight_turn_idx = None;
            } else if !app.session.streaming.is_empty() {
                // Same flush shape as `StreamEnd` (W3 D4): partial stream
                // → Markdown element, with a Sources element attached
                // when URLs were harvested from the body + tool cards,
                // so streamed-then-errored citations are not lost.
                let body = std::mem::take(&mut app.session.streaming);
                let urls = collect_turn_urls(&body, &app.session.tool_cards);
                let mut elements: Vec<TurnElement> =
                    Vec::with_capacity(if urls.is_empty() { 1 } else { 2 });
                elements.push(TurnElement::Markdown(body));
                if !urls.is_empty() {
                    elements.push(TurnElement::Sources(urls));
                }
                app.session.turns.push(TurnView {
                    role: TurnRole::Assistant,
                    elements,
                });
            }
            app.session.thinking.clear();
            app.session.streaming_active = false;
            // W3 D3: error is terminal — return to Idle so the working
            // line disappears together with the cleared streaming flag.
            app.session.phase = StreamingPhase::Idle;
            app.session.phase_started_at = Instant::now();
            // v0.9.2 W1 (Task 1.5): a failed turn is just as terminal as a
            // clean StreamEnd — release the clock so a turn that errors with
            // no following StreamEnd cannot leave `wants_tick()` pinned true
            // and burn idle CPU forever.
            app.anim.unsubscribe(AnimId::Spinner);
            app.anim.unsubscribe(AnimId::StreamingStatus);
            app.anim.unsubscribe(AnimId::StallLerp);
        }
        ProtocolEvent::Info { message, .. } => {
            push_system(app, message);
        }
        ProtocolEvent::ProviderCircuitEvent {
            primary,
            fallback,
            state,
            error,
        } => {
            let mut msg = format!("Provider `{primary}` circuit {state}");
            if let Some(fb) = fallback {
                msg.push_str(&format!(" → fallback `{fb}`"));
            }
            if let Some(err) = error {
                msg.push_str(&format!(" ({err})"));
            }
            push_system(app, msg);
        }
        ProtocolEvent::PluginRegistrationFailed {
            plugin_name,
            surface,
            message,
            ..
        } => {
            push_system(
                app,
                format!("Plugin `{plugin_name}` failed to register {surface}: {message}"),
            );
        }
        ProtocolEvent::BrowserPolicyDenied { url, reason, .. } => {
            push_system(app, format!("Browser blocked `{url}`: {reason}"));
        }
        ProtocolEvent::CuaPolicyDenied { op, reason, .. } => {
            push_system(app, format!("Computer-use op `{op}` blocked: {reason}"));
        }

        // ── No view impact in Wave 0 ─────────────────────────────────
        // These variants carry diagnostics or capability handshakes that
        // no Wave-0 `App` field models. They are explicit no-ops so an
        // unknown-but-valid event can never crash the bridge.
        ProtocolEvent::Ready { .. }
        | ProtocolEvent::TraceEvent { .. }
        | ProtocolEvent::PluginEvent { .. }
        | ProtocolEvent::EvolutionEvent { .. }
        | ProtocolEvent::BrowserEvent { .. }
        | ProtocolEvent::CuaEvent { .. }
        | ProtocolEvent::Suspend { .. }
        | ProtocolEvent::ApprovalResume { .. }
        | ProtocolEvent::CompactOffload { .. }
        | ProtocolEvent::Pong => {}
    }
}

/// Find the in-flight tool card matching `call_id`, if any.
fn find_card<'a>(app: &'a mut App, call_id: &str) -> Option<&'a mut ToolCardModel> {
    app.session
        .tool_cards
        .iter_mut()
        .find(|c| c.call_id == call_id)
}

/// v0.9.1.2 F12: open (or reuse) the in-flight assistant turn and return
/// its index in `session.turns`.
///
/// Called by the bridge when assistant content needs to land on a turn
/// in document order (a `ToolCard` element must interleave with previously
/// streamed text). If `session.in_flight_turn_idx` is `Some`, that turn
/// is reused; otherwise an empty assistant `TurnView` is pushed and its
/// index is recorded. `StreamEnd` / `Error` clear the index.
fn ensure_in_flight_assistant_turn(app: &mut App) -> usize {
    if let Some(idx) = app.session.in_flight_turn_idx {
        return idx;
    }
    app.session.turns.push(TurnView {
        role: TurnRole::Assistant,
        elements: Vec::new(),
    });
    let idx = app.session.turns.len() - 1;
    app.session.in_flight_turn_idx = Some(idx);
    idx
}

/// v0.9.1.2 F12: if `session.streaming` has buffered text, append it as
/// a [`TurnElement::Markdown`] to the in-flight assistant turn (opening
/// one if needed) and clear the buffer.
///
/// Called by the bridge BEFORE pushing a [`TurnElement::ToolCard`] so the
/// element order on the turn is `[Markdown("text before tool"), ToolCard,
/// Markdown("text after tool"), ToolCard, ...]` in document order. The
/// streaming buffer then keeps accumulating the NEXT chunk of text until
/// the next interleave point.
fn flush_streaming_into_in_flight_turn(app: &mut App) {
    if app.session.streaming.is_empty() {
        return;
    }
    let body = std::mem::take(&mut app.session.streaming);
    let idx = ensure_in_flight_assistant_turn(app);
    app.session.turns[idx]
        .elements
        .push(TurnElement::Markdown(body));
}

/// Collect citation URLs for the current turn (W3 D4).
///
/// Two sources are merged, in this order so insertion order matches what
/// the user reads:
///
/// 1. Inline markdown links in the assistant body — re-runs the W2 C1
///    `render_markdown` parser over `body` and discards the rendered
///    lines, keeping only the URL list. Cheap (pulldown-cmark is fast),
///    and avoids a second URL extractor going out of sync with what the
///    markdown widget shows.
/// 2. Per-tool `extract_urls` over every tool card in this turn — the
///    formatter dispatcher is Total, so unknown tool names safely fall
///    through to `generic` (empty URL list).
///
/// Deduplication preserves first-seen order. The result is capped at
/// [`SOURCES_MAX_PER_TURN`]; overflow is dropped silently — the Sources
/// block is a citation hint, not an exhaustive log (W3 D4 scope).
fn collect_turn_urls(body: &str, tool_cards: &[ToolCardModel]) -> Vec<String> {
    // We render with a throwaway theme — only the second tuple element
    // (the URLs) matters here, so the styling choice has no observable
    // effect. `no_color` keeps the call deterministic regardless of
    // environment (avoids touching `Theme::detect` and its env reads).
    let theme = Theme::no_color();
    let mut out: Vec<String> = Vec::new();
    let mut seen: std::collections::HashSet<String> = std::collections::HashSet::new();

    // (1) Inline markdown link URLs from the body. Discard the rendered
    // lines — W3 still renders the transcript through `turn.text()` for
    // most readers; the W2 markdown-in-transcript wiring is a v0.9.x
    // stretch and doesn't change the URL source-of-truth.
    let (_lines, body_urls) = render_markdown(body, &theme);
    for u in body_urls {
        if seen.insert(u.clone()) {
            out.push(u);
            if out.len() >= SOURCES_MAX_PER_TURN {
                return out;
            }
        }
    }

    // (2) URLs from each tool card's payload. The card's `output` is
    // the raw tool-result string; per-tool formatters know how to pluck
    // citations from their own payload shape. Cards whose output is not
    // valid JSON (a `Bash` stdout dump, say) yield zero URLs — a JSON
    // parse error is treated as "no citable URLs", not a crash.
    for card in tool_cards {
        let payload: serde_json::Value = match card.output.as_deref() {
            Some(s) => serde_json::from_str(s).unwrap_or(serde_json::Value::Null),
            None => serde_json::Value::Null,
        };
        for u in formatter_for(&card.tool_name).extract_urls(&payload) {
            if seen.insert(u.clone()) {
                out.push(u);
                if out.len() >= SOURCES_MAX_PER_TURN {
                    return out;
                }
            }
        }
    }

    out
}

/// Append a `System` turn carrying `text` to the transcript.
///
/// System turns carry a single `TurnElement::Markdown` so notices
/// (errors, info banners, costs, MCP-ready notifications) flow through
/// the same per-element renderer as assistant turns. The variant choice
/// is intentional: notices are short prose, not citations or thinking.
fn push_system(app: &mut App, text: String) {
    app.session.turns.push(TurnView {
        role: TurnRole::System,
        elements: vec![TurnElement::Markdown(text)],
    });
}

/// D037 — build the "files changed this turn" element from the per-turn
/// touched-file delta.
///
/// `App::touched_files()` is session-cumulative and deduped; the slice from
/// the StreamStart watermark ([`App::touched_files_watermark`]) onward is
/// exactly the set of files THIS turn touched. We render each path to the
/// project-relative display form the right-rail tree already uses
/// ([`normalize_path`]) so the card matches the rest of the UI rather than
/// dumping machine-specific absolute paths.
///
/// Returns `None` when the turn touched no files so a chat-only turn leaves
/// no empty card. The bridge attaches the returned element to the assistant
/// turn it just completed, making the deliverable one reviewable unit (D037)
/// instead of prose the user has to reconstruct.
fn files_changed_this_turn(app: &App) -> Option<TurnElement> {
    let watermark = app.touched_files_watermark().min(app.touched_files().len());
    let changed = &app.touched_files()[watermark..];
    if changed.is_empty() {
        return None;
    }
    let paths: Vec<String> = changed
        .iter()
        .map(|p| normalize_path(&p.to_string_lossy()))
        .collect();
    Some(TurnElement::FilesChanged(paths))
}

/// D019 — capture a `/rewind` checkpoint at turn end.
///
/// Snapshots every file the agent touched this session into the per-session
/// checkpoint store, labelled with the touched-file count so `/rewind`'s
/// listing reads honestly. Best-effort by contract: a turn with no touched
/// files captures nothing (no empty checkpoint for a chat-only turn), and a
/// capture i/o failure is swallowed — a missing snapshot is degraded, never a
/// reason to crash the bridge mid-turn.
fn capture_turn_checkpoint(app: &mut App) {
    if app.touched_files().is_empty() {
        return;
    }
    let files: Vec<std::path::PathBuf> = app.touched_files().to_vec();
    let label = format!("turn end · {} file(s)", files.len());
    // B1b: clone the cheap store handle (two PathBufs) so the actual capture can
    // run OFF the App render lock. `capture_turn_checkpoint` is called from
    // `apply_event_inner`, which runs UNDER the `std::sync::Mutex<App>` guard the
    // render loop also takes; `store.capture` does full-session filesystem I/O
    // (it writes every touched file's bytes into the checkpoint dir), so doing it
    // inline stalls every frame on a big turn. We are always on the bridge's
    // tokio task here, so offload the I/O to the blocking pool and return
    // immediately — the lock is released without waiting on disk.
    let store = app.checkpoint_store().clone();
    // Ignore the JoinHandle and the capture Result deliberately: best-effort
    // capture (see doc comment) — a snapshot failure is degraded, never fatal.
    // On the live bridge we are on a tokio runtime, so offload the blocking fs
    // I/O to the blocking pool and return immediately (the App render lock is
    // released without waiting on disk). Off-runtime callers (sync unit tests,
    // any non-async path) have no pool to offload to, so capture inline rather
    // than panic in `Handle::current()`.
    match tokio::runtime::Handle::try_current() {
        Ok(handle) => {
            handle.spawn_blocking(move || {
                let _ = store.capture(label, files);
            });
        }
        Err(_) => {
            let _ = store.capture(label, files);
        }
    }
}

/// v0.9.1.1 H3 — map a `ProtocolEvent::Error.code` (an internal class
/// name like `engine_error`, `engine_panic`, `rate_limit`) to a clean
/// user-facing label for the Activity rail / transcript.
///
/// Returns `None` for unknown codes so the caller falls back to the
/// raw chip — a new error class added in a future patch shows up in
/// the rail rather than disappearing silently. The full class +
/// message is still routed to `tracing::debug!` at the call site, so
/// the log file (`~/.wayland/logs/wayland-core.log`) carries the
/// internal name for `/doctor` / operator diagnosis.
fn error_class_label(code: &str) -> Option<&'static str> {
    match code {
        // `engine_error` is the generic engine-level failure (provider
        // error, retry budget exhausted, ApiError bubbled out of
        // `engine.run`). User-facing label is the bare "Error" — the
        // detail in `pretty` carries the actual cause.
        "engine_error" => Some("Error"),
        // `engine_panic` fires only from the `TerminalGuard` Drop —
        // the turn task panicked or was aborted before it sent its own
        // terminal events. "Turn ended unexpectedly" matches the
        // message body already shipped in that path (engine_bridge.rs
        // line ~443) and avoids the "panic" jargon that implies a
        // crash the user should report.
        "engine_panic" => Some("Turn ended unexpectedly"),
        // `rate_limit` is the provider's 429 — surface it as the
        // user-actionable phrase rather than the internal enum.
        "rate_limit" => Some("Rate limit"),
        _ => None,
    }
}

/// v0.9.1 W2 cycle-2 BLOCKER 3: peel a provider-error JSON envelope down
/// to its human-readable line.
///
/// Provider SDKs (anthropic, openai, ...) wrap HTTP failures in a JSON
/// envelope and emit it as a single string — e.g.
///
/// ```text
/// API error 401: {"type":"error","error":{"type":"authentication_error",
/// "message":"invalid x-api-key"},"request_id":"req_..."}
/// ```
///
/// Rendered raw the user has to read JSON braces to find the one
/// meaningful sentence. This function:
///
/// 1. Strips a leading `API error N: ` / `API error N — ` prefix if
///    present, preserving the status code.
/// 2. Parses the remainder as JSON; if it parses and carries a nested
///    `error.message` (Anthropic shape) or top-level `message` field
///    (generic shape), uses that.
/// 3. Falls back to the input string truncated to 500 chars so a
///    non-JSON error still renders cleanly.
///
/// The output never contains `{` / `}` for valid provider envelopes —
/// the caller can prepend `Error: ` and produce a one-line transcript
/// notice that matches mockup §6 error-card aesthetic.
fn sanitize_provider_error_message(raw: &str) -> String {
    const MAX_LEN: usize = 500;
    let trimmed = raw.trim();

    // (0) Peel a wrapper `<Word> error: …` prefix produced by `AgentError`
    // / `ProviderError` `Display` impls (e.g. `Provider error: API error
    // 401: {JSON}`). Without this strip, `split_api_error_prefix` would
    // miss the inner `API error …` and the JSON envelope would leak.
    // v0.9.1 W2 cycle-3 BLOCKER 3 fix.
    let after_wrapper = strip_wrapper_error_prefix(trimmed);

    // (1) Peel an `API error <code>: …` prefix so the inner JSON can be
    // parsed. We keep the status code so the rendered notice still tells
    // the user what kind of failure it was (401 vs 500 vs ...).
    let (prefix, body) = match split_api_error_prefix(after_wrapper) {
        Some((code, rest)) => (format!("API {code}: "), rest),
        None => (String::new(), after_wrapper),
    };

    // (2) If the body parses as JSON, try `.error.message` then `.message`.
    if let Ok(value) = serde_json::from_str::<serde_json::Value>(body)
        && let Some(msg) = extract_error_message(&value)
    {
        return truncate_to(&format!("{prefix}{msg}"), MAX_LEN);
    }

    // (3) Fall back: the (wrapper-stripped) string, truncated. We keep
    // the wrapper strip even on the fallback path so a `Network error:
    // timeout` notice doesn't double-prefix in the transcript.
    truncate_to(after_wrapper, MAX_LEN)
}

/// Strip a single leading `<Word> error: ` wrapper produced by Rust
/// `Display` impls on error enums (e.g. `Provider error: …`, `Network
/// error: …`). Returns the suffix on match, or the input unchanged when
/// the pattern doesn't apply.
///
/// The pattern is intentionally narrow: a single ASCII-alpha word,
/// followed by a literal ` error: `, anchored at the start. Multi-word
/// wrappers and non-ASCII labels fall through to preserve them in the
/// transcript verbatim.
fn strip_wrapper_error_prefix(s: &str) -> &str {
    let separator = " error: ";
    let Some(sep_pos) = s.find(separator) else {
        return s;
    };
    let prefix = &s[..sep_pos];
    if prefix.is_empty() || !prefix.chars().all(|c| c.is_ascii_alphabetic()) {
        return s;
    }
    // Avoid eating an inner `API error` prefix — that's handled by
    // `split_api_error_prefix` in step (1). Only strip when the wrapper
    // word is something else (Provider / Network / MCP / ...).
    if prefix.eq_ignore_ascii_case("api") {
        return s;
    }
    &s[sep_pos + separator.len()..]
}

/// Split `API error <code>: <rest>` (or `… — <rest>`) into `(code, rest)`.
/// Returns `None` if the prefix is absent. The code is preserved verbatim
/// so a non-numeric or unusual provider prefix still works.
fn split_api_error_prefix(s: &str) -> Option<(&str, &str)> {
    let lower = s.to_ascii_lowercase();
    let after = lower.strip_prefix("api error ")?;
    // Find the first separator (`: ` or ` — `) that ends the code.
    let separator = ": ";
    let sep_pos = after.find(separator)?;
    let code = &s["api error ".len().."api error ".len() + sep_pos];
    let rest_start = "api error ".len() + sep_pos + separator.len();
    if rest_start >= s.len() {
        return None;
    }
    Some((code.trim(), s[rest_start..].trim_start()))
}

/// Extract the human-readable message from a parsed JSON value.
///
/// Tries the Anthropic shape (`.error.message`), then the generic shape
/// (`.message`), then the OpenAI shape (`.error.message` is the same as
/// Anthropic, but `.error` may be a string instead of an object).
fn extract_error_message(value: &serde_json::Value) -> Option<String> {
    // .error.message (object)
    if let Some(inner) = value.get("error") {
        if let Some(msg) = inner.get("message").and_then(|v| v.as_str()) {
            return Some(msg.to_string());
        }
        // .error as bare string (some providers).
        if let Some(s) = inner.as_str() {
            return Some(s.to_string());
        }
    }
    // .message at top level.
    if let Some(msg) = value.get("message").and_then(|v| v.as_str()) {
        return Some(msg.to_string());
    }
    None
}

/// Truncate `s` to at most `max` chars, appending an ellipsis if cut.
/// Char-based, not byte-based, so it never splits a UTF-8 codepoint.
fn truncate_to(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        s.to_string()
    } else {
        let head: String = s.chars().take(max.saturating_sub(1)).collect();
        format!("{head}…")
    }
}

/// Pretty-print a tool's input args as JSON for the approval modal. A
/// `null`/missing arg renders as an empty string so the modal does not
/// show a stray `null` line. Two-space indentation matches the
/// convention used by every other JSON surface in the codebase.
fn pretty_input(args: &serde_json::Value) -> String {
    if args.is_null() {
        return String::new();
    }
    serde_json::to_string_pretty(args).unwrap_or_else(|_| args.to_string())
}

/// Build a short, human-readable argument preview for a tool card.
///
/// Picks the most salient field per tool family (path for file tools,
/// command for `Bash`, pattern for search tools) and falls back to a
/// compact JSON rendering when nothing matches.
///
/// v0.9.1.1 B4-hunt: routes through the per-tool formatter's
/// `format_args` hook FIRST so tools like `text_to_speech` (which used
/// to dump `{"text":"..."}` into the inline approval card) render a
/// friendly preview. Tools without a `format_args` override fall back
/// to the hand-rolled cases below, then to a clamped JSON rendering.
fn summarize_args(tool_name: &str, args: &serde_json::Value) -> String {
    // First refusal: the per-tool formatter's args preview.
    if let Some(s) = formatter_for(tool_name).format_args(args) {
        return s;
    }

    let str_field = |key: &str| args.get(key).and_then(|v| v.as_str());

    match tool_name {
        "Read" | "Write" | "Edit" => str_field("file_path")
            .map(str::to_string)
            .unwrap_or_default(),
        "Bash" => str_field("command").map(str::to_string).unwrap_or_default(),
        "Grep" => str_field("pattern").map(str::to_string).unwrap_or_default(),
        "Glob" => str_field("pattern").map(str::to_string).unwrap_or_default(),
        _ => {
            // Unknown tool with no formatter override: clamp the args
            // JSON hard so the card header never overflows AND so a long
            // `text` field cannot dominate the line. Single-line, ASCII
            // ellipsis.
            let mut s = args.to_string();
            if s.len() > 80 {
                s.truncate(77);
                s.push_str("...");
            }
            s
        }
    }
}

/// The file path a tool touches, if it is a file-oriented tool. Used to
/// host-derive the right-rail path map from tool activity — there is no
/// engine event for a path map (AUDIT). `None` for tools that touch no
/// single file (`Bash`, `Grep`, `Glob`, plan-mode tools, …).
fn touched_path(tool_name: &str, args: &serde_json::Value) -> Option<String> {
    match tool_name {
        "Read" | "Write" | "Edit" => args
            .get("file_path")
            .and_then(|v| v.as_str())
            .map(str::to_string),
        _ => None,
    }
}

/// Normalize a touched path to the `/`-separated, relative form the path
/// map expects: backslashes become forward slashes (Windows), and an
/// absolute path is reduced to its trailing components so the tree shows
/// the project-relative shape rather than a machine-specific root.
fn normalize_path(path: &str) -> String {
    let unix = path.replace('\\', "/");
    // Drop a leading `/` or drive prefix — the tree is project-relative.
    unix.trim_start_matches('/').to_string()
}

/// Build a `PlanView` from an `EnterPlanMode` tool's request args.
///
/// The `EnterPlanMode` tool input carries the plan body as a `plan` (or
/// `content`) string field; the title falls back to the first non-empty
/// line. A missing body yields an empty plan (rendered as the surface's
/// explicit empty state, never a blank screen).
fn plan_from_args(args: &serde_json::Value) -> crate::tui::app::PlanView {
    let body = args
        .get("plan")
        .or_else(|| args.get("content"))
        .or_else(|| args.get("description"))
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    let title = args
        .get("title")
        .and_then(|v| v.as_str())
        .map(str::to_string)
        .or_else(|| {
            body.lines()
                .find(|l| !l.trim().is_empty())
                .map(str::to_string)
        })
        .unwrap_or_else(|| "Proposed plan".to_string());
    crate::tui::app::PlanView { title, body }
}

/// Extract a `DiffModel` from an `Edit`/`Write` tool's request args.
///
/// `ProtocolEvent::ToolRequest` already carries the full tool input as
/// `ToolInfo.args` (a `serde_json::Value`). The `Edit` tool's schema is
/// `{file_path, old_string, new_string}` and `Write`'s is
/// `{file_path, content}` — so the path plus old/new content is fully
/// reachable here. No new protocol event is required (T0.5 verification:
/// the edit-preview is derivable, so we DO NOT add `ToolEditPreview`).
///
/// For `Write` the prior file body is not in the args (the tool reads it
/// at execution time), so `old` is left empty — the diff widget then
/// renders a clean all-additions preview, which is correct for the
/// new-file case and an acceptable approximation for an overwrite.
fn edit_preview_from_args(tool_name: &str, args: &serde_json::Value) -> Option<DiffModel> {
    let str_field = |key: &str| args.get(key).and_then(|v| v.as_str());

    match tool_name {
        "Edit" => {
            let path = str_field("file_path")?;
            let old = str_field("old_string")?;
            let new = str_field("new_string")?;
            Some(DiffModel {
                path: path.to_string(),
                old: old.to_string(),
                new: new.to_string(),
            })
        }
        "Write" => {
            let path = str_field("file_path")?;
            let new = str_field("content")?;
            Some(DiffModel {
                path: path.to_string(),
                old: String::new(),
                new: new.to_string(),
            })
        }
        _ => None,
    }
}

/// Rebuild a transcript (`TurnView`s + `ToolCardModel`s) from a restored
/// session's `messages`, so a `--resume` / `--continue` boot repaints the
/// prior conversation instead of opening a blank screen.
///
/// This is the history → view twin of [`apply_event`] (which is the event →
/// view authority). There is deliberately NO `UserMessage` protocol event —
/// live user turns are added locally at composer-submit — so resume cannot
/// simply replay messages as events; it reconstructs the view model directly.
/// Crucially it reuses the SAME private helpers the live `ToolRequest` path
/// uses (`summarize_args`, `pretty_input`, `edit_preview_from_args`,
/// `touched_path`) so a restored tool card is byte-identical to one produced
/// live — no separate, drift-prone formatting path.
///
/// Mapping (walking messages oldest-first):
/// - `User` text block → a `User` turn with a `Markdown` element.
/// - `Assistant` text → `Markdown`; `Thinking` → a persisted `Thinking`
///   element (duration/tokens unknown post-hoc → 0); `ToolUse` → a
///   `ToolCard(id)` element plus a `ToolCardModel` (status `Running` until a
///   later `ToolResult` completes it).
/// - `User`/`Tool` `ToolResult` block → correlated by `tool_use_id` to its
///   card, setting the card's final status (`Ok`/`Err`) and output. Creates
///   no turn of its own.
/// - `System` text → a `System` turn.
///
/// Returns `(turns, tool_cards)` to seed `SessionView`. A tool whose result
/// never arrived (e.g. an interrupted final turn) is left `Running`, matching
/// what actually happened.
pub fn hydrate_history(messages: &[Message]) -> (Vec<TurnView>, Vec<ToolCardModel>) {
    let mut turns: Vec<TurnView> = Vec::new();
    let mut cards: Vec<ToolCardModel> = Vec::new();

    let complete_card =
        |cards: &mut Vec<ToolCardModel>, tool_use_id: &str, output: String, is_error: bool| {
            if let Some(card) = cards.iter_mut().find(|c| c.call_id == tool_use_id) {
                card.status = if is_error {
                    ToolCardStatus::Err
                } else {
                    ToolCardStatus::Ok
                };
                card.output = Some(output);
            }
        };

    for msg in messages {
        match msg.role {
            Role::User => {
                let mut turn = TurnView::new(TurnRole::User);
                for block in &msg.content {
                    match block {
                        ContentBlock::Text { text } => {
                            turn.elements.push(TurnElement::Markdown(text.clone()));
                        }
                        ContentBlock::ToolResult {
                            tool_use_id,
                            content,
                            is_error,
                        } => complete_card(&mut cards, tool_use_id, content.clone(), *is_error),
                        // A user message carries no tool_use / thinking blocks.
                        _ => {}
                    }
                }
                if !turn.elements.is_empty() {
                    turns.push(turn);
                }
            }
            Role::Tool => {
                // Some providers route tool results under a dedicated Tool
                // role rather than folding them into a User message.
                for block in &msg.content {
                    if let ContentBlock::ToolResult {
                        tool_use_id,
                        content,
                        is_error,
                    } = block
                    {
                        complete_card(&mut cards, tool_use_id, content.clone(), *is_error);
                    }
                }
            }
            Role::Assistant => {
                let mut turn = TurnView::new(TurnRole::Assistant);
                for block in &msg.content {
                    match block {
                        ContentBlock::Text { text } => {
                            turn.elements.push(TurnElement::Markdown(text.clone()));
                        }
                        ContentBlock::Thinking { thinking } => {
                            turn.elements.push(TurnElement::Thinking {
                                body: thinking.clone(),
                                secs: 0,
                                tokens: 0,
                            });
                        }
                        ContentBlock::ToolUse {
                            id, name, input, ..
                        } => {
                            turn.elements.push(TurnElement::ToolCard(id.clone()));
                            cards.push(ToolCardModel {
                                call_id: id.clone(),
                                tool_name: name.clone(),
                                summary: summarize_args(name, input),
                                // Completed below by the matching ToolResult;
                                // an interrupted call legitimately stays Running.
                                status: ToolCardStatus::Running,
                                output: None,
                                edit_preview: edit_preview_from_args(name, input),
                                input_pretty: pretty_input(input),
                                approval_reason: String::new(),
                                plan_body: None,
                                crucible_plan: None,
                            });
                        }
                        ContentBlock::ToolResult { .. } => {}
                    }
                }
                if !turn.elements.is_empty() {
                    turns.push(turn);
                }
            }
            Role::System => {
                let mut turn = TurnView::new(TurnRole::System);
                for block in &msg.content {
                    if let ContentBlock::Text { text } = block {
                        turn.elements.push(TurnElement::Markdown(text.clone()));
                    }
                }
                if !turn.elements.is_empty() {
                    turns.push(turn);
                }
            }
        }
    }

    (turns, cards)
}

/// Apply a `SubAgentEvent`: create or update the `SubAgentView` keyed by
/// `parent_call_id`, then fold the wrapped inner event into its feed.
///
/// `inner` is a serialized `ProtocolEvent` (the protocol keeps it as a
/// `Value` to avoid a recursive variant). We read its `type` tag to
/// decide how the sub-agent's progress is reflected: text/tool events
/// become feed lines, `StreamEnd` bumps the turn count, errors mark it
/// failed, usage updates the token tally.
fn apply_sub_agent_event(
    app: &mut App,
    parent_call_id: String,
    agent_name: String,
    inner: serde_json::Value,
) {
    // Find or create the view for this sub-agent.
    let idx = app
        .session
        .sub_agents
        .iter()
        .position(|s| s.id == parent_call_id);
    let idx = match idx {
        Some(i) => i,
        None => {
            app.session.sub_agents.push(SubAgentView {
                id: parent_call_id.clone(),
                // Clone: `agent_name` is still needed below for the Phase 2
                // workflow-grouping call (this create path fires once per node).
                name: agent_name.clone(),
                status: SubAgentStatus::Running,
                turns: 0,
                tokens: 0,
                feed: Vec::new(),
            });
            // v0.9.3 S0.9 — first-ever spawn marks onboarding (W7). The bridge
            // auto-creates a SubAgentView when an event arrives for an unknown
            // id; that insertion IS the spawn moment.
            if app.onboarding_state.first_spawn_seen.is_none() {
                app.onboarding_state.first_spawn_seen = Some(std::time::Instant::now());
            }
            app.session.sub_agents.len() - 1
        }
    };

    // v0.9.3 S0.9 — feed stale-watchdog (Sec-H2 mitigation). Staleness is
    // computed on-demand from this map by `StaleWatchdog::check` + the render
    // path, keeping `SubAgentView` FROZEN per `app.rs:582-584`.
    app.agent_last_event
        .insert(parent_call_id.clone(), std::time::Instant::now());

    let kind = inner.get("type").and_then(|v| v.as_str()).unwrap_or("");
    let view = &mut app.session.sub_agents[idx];

    match kind {
        "text_delta" => {
            if let Some(text) = inner.get("text").and_then(|v| v.as_str()) {
                push_feed_line(view, text);
            }
        }
        "tool_request" => {
            if let Some(name) = inner
                .get("tool")
                .and_then(|t| t.get("name"))
                .and_then(|v| v.as_str())
            {
                view.feed.push(format!("→ {name}"));
            }
        }
        "tool_result" => {
            if let Some(name) = inner.get("tool_name").and_then(|v| v.as_str()) {
                view.feed.push(format!("✓ {name}"));
            }
        }
        "stream_end" => {
            view.turns += 1;
            if let Some(usage) = inner.get("usage") {
                let input = usage.get("input_tokens").and_then(|v| v.as_u64());
                let output = usage.get("output_tokens").and_then(|v| v.as_u64());
                view.tokens += input.unwrap_or(0) + output.unwrap_or(0);
            }
        }
        "error" => {
            view.status = SubAgentStatus::Failed;
            if let Some(msg) = inner
                .get("error")
                .and_then(|e| e.get("message"))
                .and_then(|v| v.as_str())
            {
                view.feed.push(format!("error: {msg}"));
            }
        }
        "info" => {
            // The spawn-tool drain emits a terminal `Info` when a
            // sub-agent finishes; treat it as the completion signal.
            if let Some(msg) = inner.get("message").and_then(|v| v.as_str()) {
                push_feed_line(view, msg);
            }
            view.status = SubAgentStatus::Done;
        }
        _ => {
            // Other inner events (thinking, chunks, diagnostics) do not
            // need a dedicated feed line.
        }
    }

    // ForgeFlows-Live Phase 2 — ALSO group `"workflow:<node_id>"`-prefixed
    // events under the Workflows tab. This is ADDITIONAL: the
    // `session.sub_agents` population above is unchanged, so the SubAgents
    // tab keeps every workflow node too. The `view` borrow has dropped by
    // here (the `match` ended), so this runs against `&mut app` directly.
    if let Some(node_id) = parent_call_id.strip_prefix("workflow:") {
        apply_workflow_node_event(app, node_id, &agent_name, kind, &inner);
    }

    // v0.9.3 S0.9 — terminal-status branches feed the glow fader so the
    // 30s done-glow can fade out on the strip + sub-agent row. The `view`
    // borrow above has dropped by the end of the `match`, so this runs
    // against `&mut app` directly. `kind` is "error" or "info" for the
    // two terminal branches in this bridge (`SubAgentStatus::Failed` and
    // `SubAgentStatus::Done`); see the matched arms above.
    if matches!(kind, "error" | "info") {
        app.agent_glow
            .record_terminal(parent_call_id.clone(), std::time::Instant::now());
        // v0.9.3 S0.11 — ensure AnimId::TerminalGlow is subscribed for the
        // 30s fade. `subscribe` is idempotent (clock contract verified at
        // anim/clock.rs:59 + the `subscribe_is_idempotent` test). `true`
        // passes `keep_alive: true` because the glow must outlive any one
        // upstream stream — the GlowFader prune logic at Router::tick_active
        // calls `unsubscribe(AnimId::TerminalGlow)` only when the LAST entry
        // has faded (`glow.prune(now) == true`), so we don't drop the clock
        // tick mid-fade for a co-running second terminal event. Adjacent
        // v0.9.2 sites at lines 137-139 pass `false` (per-stream lifetimes);
        // TerminalGlow's longer-than-stream lifetime is why we differ.
        app.anim.subscribe(AnimId::TerminalGlow, true);
    }
}

/// Append `text` to a sub-agent's feed as whole lines, skipping blanks.
fn push_feed_line(view: &mut SubAgentView, text: &str) {
    for line in text.lines() {
        let line = line.trim_end();
        if !line.is_empty() {
            view.feed.push(line.to_string());
        }
    }
}

/// Sentinel `key` for a `WorkflowView` created by a node event that arrived
/// before its `WorkflowStarted`. The double-underscore prefix can't collide
/// with a real plan name (the engine's `workflow_id` is `plan.meta.name`).
/// The next `WorkflowStarted` adopts the pending view, swapping in the real
/// `workflow_id`.
const PENDING_WORKFLOW_KEY: &str = "__pending__";

/// ForgeFlows-Live — resolve the index of the CURRENT workflow run a
/// `"workflow:<node_id>"` node event attaches to. The wire format carries no
/// `workflow_id` on node events (only the `"workflow:"` prefix), and the
/// engine runs workflows sequentially through one FIFO sink, so the current
/// run is the last view in `app.workflows` IF it is unfinished. If the list
/// is empty or the last view is already finished (a new run's node arrived
/// before its `WorkflowStarted`), push a fresh pending view and return it —
/// the upcoming `WorkflowStarted` will adopt it.
fn current_workflow_for_node(app: &mut App) -> usize {
    let attach_to_last = app.workflows.last().is_some_and(|w| w.finished.is_none());
    if attach_to_last {
        return app.workflows.len() - 1;
    }
    app.workflows.push(WorkflowView {
        key: PENDING_WORKFLOW_KEY.to_string(),
        name: "Workflow".to_string(),
        nodes: Vec::new(),
        finished: None,
    });
    app.workflows.len() - 1
}

/// ForgeFlows-Live Phase 2 — fold a `"workflow:<node_id>"`-prefixed inner
/// event into the Workflows-tab view. Find-or-create the single workflow
/// group (MVP key `"workflow"`), then find-or-create the node by
/// `node_id`, and apply the SAME fold the SubAgentView path uses so the
/// two stay consistent: `text_delta` → feed line, `tool_request` /
/// `tool_result` → marker line, `stream_end` → tokens, `info` → Done,
/// `error` → Failed.
///
/// `kind` and `inner` are the already-parsed event tag + payload from
/// `apply_sub_agent_event`, so this never re-reads `parent_call_id`.
fn apply_workflow_node_event(
    app: &mut App,
    node_id: &str,
    agent_name: &str,
    kind: &str,
    inner: &serde_json::Value,
) {
    let wf_idx = current_workflow_for_node(app);
    let workflow = &mut app.workflows[wf_idx];

    let node_idx = match workflow.nodes.iter().position(|n| n.node_id == node_id) {
        Some(i) => i,
        None => {
            workflow.nodes.push(WorkflowNodeView {
                node_id: node_id.to_string(),
                agent_name: agent_name.to_string(),
                status: SubAgentStatus::Running,
                feed: Vec::new(),
                tokens: 0,
            });
            workflow.nodes.len() - 1
        }
    };
    let node = &mut workflow.nodes[node_idx];

    match kind {
        "text_delta" => {
            if let Some(text) = inner.get("text").and_then(|v| v.as_str()) {
                push_node_feed_line(node, text);
            }
        }
        "tool_request" => {
            if let Some(name) = inner
                .get("tool")
                .and_then(|t| t.get("name"))
                .and_then(|v| v.as_str())
            {
                node.feed.push(format!("→ {name}"));
            }
        }
        "tool_result" => {
            if let Some(name) = inner.get("tool_name").and_then(|v| v.as_str()) {
                node.feed.push(format!("✓ {name}"));
            }
        }
        "stream_end" => {
            if let Some(usage) = inner.get("usage") {
                let input = usage.get("input_tokens").and_then(|v| v.as_u64());
                let output = usage.get("output_tokens").and_then(|v| v.as_u64());
                node.tokens += input.unwrap_or(0) + output.unwrap_or(0);
            }
        }
        "error" => {
            node.status = SubAgentStatus::Failed;
            if let Some(msg) = inner
                .get("error")
                .and_then(|e| e.get("message"))
                .and_then(|v| v.as_str())
            {
                node.feed.push(format!("error: {msg}"));
            }
        }
        "info" => {
            if let Some(msg) = inner.get("message").and_then(|v| v.as_str()) {
                push_node_feed_line(node, msg);
            }
            node.status = SubAgentStatus::Done;
        }
        _ => {}
    }
}

/// Append `text` to a workflow node's feed as whole lines, skipping
/// blanks. Mirrors [`push_feed_line`] for the workflow path.
fn push_node_feed_line(node: &mut WorkflowNodeView, text: &str) {
    for line in text.lines() {
        let line = line.trim_end();
        if !line.is_empty() {
            node.feed.push(line.to_string());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tui::app::App;
    use crate::tui::fixtures;
    use serde_json::json;
    use wcore_protocol::events::{ErrorInfo, OutputType, ToolCategory, ToolInfo};

    // ── hydrate_history (resume repaint) ─────────────────────────────────

    fn user_text(text: &str) -> Message {
        Message::new(Role::User, vec![ContentBlock::Text { text: text.into() }])
    }
    fn assistant_text(text: &str) -> Message {
        Message::new(
            Role::Assistant,
            vec![ContentBlock::Text { text: text.into() }],
        )
    }

    #[test]
    fn hydrate_rebuilds_a_plain_two_turn_exchange() {
        let msgs = vec![user_text("remember TOKEN_42"), assistant_text("noted")];
        let (turns, cards) = hydrate_history(&msgs);

        assert!(cards.is_empty(), "no tools → no cards");
        assert_eq!(turns.len(), 2);
        assert_eq!(turns[0].role, TurnRole::User);
        assert!(turns[0].text().contains("TOKEN_42"));
        assert_eq!(turns[1].role, TurnRole::Assistant);
        assert!(turns[1].text().contains("noted"));
    }

    #[test]
    fn hydrate_preserves_order_across_many_turns() {
        let msgs = vec![
            user_text("alpha"),
            assistant_text("ack one"),
            user_text("beta"),
            assistant_text("ack two"),
            user_text("gamma"),
            assistant_text("ack three"),
        ];
        let (turns, _) = hydrate_history(&msgs);
        assert_eq!(turns.len(), 6);
        // oldest-first, alternating roles, content preserved in order
        let joined: Vec<String> = turns.iter().map(|t| t.text()).collect();
        assert!(joined[0].contains("alpha"));
        assert!(joined[1].contains("ack one"));
        assert!(joined[4].contains("gamma"));
        assert!(joined[5].contains("ack three"));
    }

    #[test]
    fn hydrate_reconstructs_a_completed_tool_card() {
        // Assistant calls Write, then the tool result comes back (user role).
        let msgs = vec![
            user_text("write the file"),
            Message::new(
                Role::Assistant,
                vec![ContentBlock::ToolUse {
                    id: "toolu_1".into(),
                    name: "Write".into(),
                    input: json!({"file_path": "/tmp/x.txt", "content": "hi"}),
                    extra: None,
                }],
            ),
            Message::new(
                Role::User,
                vec![ContentBlock::ToolResult {
                    tool_use_id: "toolu_1".into(),
                    content: "wrote 2 bytes".into(),
                    is_error: false,
                }],
            ),
            assistant_text("done"),
        ];
        let (turns, cards) = hydrate_history(&msgs);

        // One card, completed Ok, with output and an edit preview (Write).
        assert_eq!(cards.len(), 1);
        let card = &cards[0];
        assert_eq!(card.call_id, "toolu_1");
        assert_eq!(card.tool_name, "Write");
        assert_eq!(card.status, ToolCardStatus::Ok);
        assert_eq!(card.output.as_deref(), Some("wrote 2 bytes"));
        assert!(
            card.edit_preview.is_some(),
            "Write should reconstruct an edit preview, same as the live path"
        );
        assert_eq!(
            card.summary, "/tmp/x.txt",
            "summary matches live summarize_args"
        );

        // The tool result message creates NO turn; the assistant tool-use turn
        // carries a ToolCard element referencing the card by id.
        let assistant_tool_turn = &turns[1];
        assert_eq!(assistant_tool_turn.role, TurnRole::Assistant);
        assert!(
            assistant_tool_turn
                .elements
                .iter()
                .any(|e| matches!(e, TurnElement::ToolCard(id) if id == "toolu_1")),
            "the assistant turn must reference the tool card inline"
        );
        // user("write the file"), assistant(toolcard), assistant("done") = 3 turns
        // (the tool-result user message contributes no turn).
        assert_eq!(turns.len(), 3);
    }

    #[test]
    fn hydrate_marks_an_errored_tool_result() {
        let msgs = vec![
            Message::new(
                Role::Assistant,
                vec![ContentBlock::ToolUse {
                    id: "toolu_err".into(),
                    name: "Bash".into(),
                    input: json!({"command": "false"}),
                    extra: None,
                }],
            ),
            Message::new(
                Role::User,
                vec![ContentBlock::ToolResult {
                    tool_use_id: "toolu_err".into(),
                    content: "exit code 1".into(),
                    is_error: true,
                }],
            ),
        ];
        let (_, cards) = hydrate_history(&msgs);
        assert_eq!(cards.len(), 1);
        assert_eq!(cards[0].status, ToolCardStatus::Err);
        assert_eq!(cards[0].output.as_deref(), Some("exit code 1"));
    }

    #[test]
    fn hydrate_leaves_an_unmatched_tool_call_running() {
        // A tool call whose result never arrived (e.g. interrupted final turn).
        let msgs = vec![Message::new(
            Role::Assistant,
            vec![ContentBlock::ToolUse {
                id: "toolu_open".into(),
                name: "Read".into(),
                input: json!({"file_path": "/tmp/y"}),
                extra: None,
            }],
        )];
        let (_, cards) = hydrate_history(&msgs);
        assert_eq!(cards.len(), 1);
        assert_eq!(
            cards[0].status,
            ToolCardStatus::Running,
            "an uncompleted call legitimately stays Running, matching reality"
        );
        assert!(cards[0].output.is_none());
    }

    #[test]
    fn hydrate_keeps_assistant_thinking_blocks() {
        let msgs = vec![Message::new(
            Role::Assistant,
            vec![
                ContentBlock::Thinking {
                    thinking: "let me reason".into(),
                },
                ContentBlock::Text {
                    text: "the answer".into(),
                },
            ],
        )];
        let (turns, _) = hydrate_history(&msgs);
        assert_eq!(turns.len(), 1);
        let has_thinking = turns[0]
            .elements
            .iter()
            .any(|e| matches!(e, TurnElement::Thinking { body, .. } if body == "let me reason"));
        assert!(has_thinking, "persisted thinking must survive resume");
        assert!(turns[0].text().contains("the answer"));
    }

    #[test]
    fn hydrate_of_empty_history_yields_nothing() {
        let (turns, cards) = hydrate_history(&[]);
        assert!(turns.is_empty() && cards.is_empty());
    }

    #[test]
    fn streaming_lifecycle_flushes_into_an_assistant_turn() {
        let mut app = App::new();
        apply_event(
            &mut app,
            ProtocolEvent::StreamStart {
                msg_id: "m1".into(),
            },
        );
        assert!(app.session.streaming_active);

        apply_event(
            &mut app,
            ProtocolEvent::TextDelta {
                text: "Hello, ".into(),
                msg_id: "m1".into(),
            },
        );
        apply_event(
            &mut app,
            ProtocolEvent::TextDelta {
                text: "world.".into(),
                msg_id: "m1".into(),
            },
        );
        assert_eq!(app.session.streaming, "Hello, world.");

        apply_event(
            &mut app,
            ProtocolEvent::StreamEnd {
                msg_id: "m1".into(),
                finish_reason: wcore_protocol::events::FinishReason::Stop,
                usage: None,
                agent_run_id: None,
            },
        );
        assert!(!app.session.streaming_active);
        assert!(app.session.streaming.is_empty());
        assert_eq!(app.session.turns.len(), 1);
        assert_eq!(app.session.turns[0].role, TurnRole::Assistant);
        // The flushed buffer lands as a single Markdown element — not a
        // mix of variants, not multiple Markdowns per turn. W2 may split
        // on safe-split points later; A2's contract is one Markdown per
        // turn for the data-shape refactor.
        assert_eq!(app.session.turns[0].elements.len(), 1);
        match &app.session.turns[0].elements[0] {
            TurnElement::Markdown(s) => assert_eq!(s, "Hello, world."),
            other => panic!("expected Markdown element, got {other:?}"),
        }
        // The `.text()` accessor preserves the old human-readable view.
        assert_eq!(app.session.turns[0].text(), "Hello, world.");
    }

    #[test]
    fn thinking_buffer_accumulates_and_clears_on_stream_end() {
        // The `ProtocolEvent::Thinking` event updates `session.thinking`
        // (the transient live-state buffer the streaming renderer reads),
        // which is cleared on `StreamEnd`. v0.9.3 W1.3 introduces a
        // separate persistence path for `<think>` tags inline in
        // `TextDelta`, captured via the reasoning filter and emitted as
        // `TurnElement::Thinking { body, secs, tokens }` at StreamEnd.
        // The two paths do NOT cross: the `Thinking` event never feeds the
        // capture buffer. A stream that only fires `Thinking` events (no
        // `TextDelta` with reasoning tags) leaves the filter's capture
        // buffer empty, so no Thinking element is pushed and a
        // thinking-only turn still flushes nothing.
        let mut app = App::new();
        apply_event(
            &mut app,
            ProtocolEvent::StreamStart {
                msg_id: "m1".into(),
            },
        );
        apply_event(
            &mut app,
            ProtocolEvent::Thinking {
                text: "reasoning...".into(),
                msg_id: "m1".into(),
            },
        );
        assert_eq!(app.session.thinking, "reasoning...");
        apply_event(
            &mut app,
            ProtocolEvent::StreamEnd {
                msg_id: "m1".into(),
                finish_reason: wcore_protocol::events::FinishReason::Stop,
                usage: None,
                agent_run_id: None,
            },
        );
        assert!(app.session.thinking.is_empty());
        // Per the above: the `Thinking` event path is independent of the
        // capture buffer; with no `TextDelta` containing `<think>` tags,
        // the buffer is empty and no turn is pushed.
        assert!(
            app.session.turns.is_empty(),
            "ProtocolEvent::Thinking alone must not flush a turn"
        );
    }

    #[test]
    fn stream_with_thinking_block_emits_thinking_element_v093() {
        // v0.9.3 W1.3 — `<think>…</think>` arriving inline in TextDelta is
        // stripped from the visible stream by the reasoning filter and
        // captured into its accumulator; StreamEnd drains the accumulator
        // and emits a `TurnElement::Thinking { body, secs, tokens }` on
        // the resulting turn. `tokens` is sourced from
        // `StreamEnd.usage.output_tokens` per SPEC v1.3 §0 #11.
        let mut app = App::new();
        apply_event(
            &mut app,
            ProtocolEvent::StreamStart {
                msg_id: "m1".into(),
            },
        );
        apply_event(
            &mut app,
            ProtocolEvent::TextDelta {
                text: "Hello <think>secret reasoning</think> world".into(),
                msg_id: "m1".into(),
            },
        );
        apply_event(
            &mut app,
            ProtocolEvent::StreamEnd {
                msg_id: "m1".into(),
                finish_reason: wcore_protocol::events::FinishReason::Stop,
                usage: Some(wcore_protocol::events::Usage {
                    input_tokens: 5,
                    output_tokens: 42,
                    cache_read_tokens: None,
                    cache_write_tokens: None,
                    active_window_percent: None,
                }),
                agent_run_id: None,
            },
        );
        assert_eq!(app.session.turns.len(), 1, "one assistant turn flushed");
        let elements = &app.session.turns[0].elements;
        // The visible stream contains "Hello  world" (reasoning stripped).
        let has_markdown = elements
            .iter()
            .any(|e| matches!(e, TurnElement::Markdown(s) if s == "Hello  world"));
        assert!(
            has_markdown,
            "expected Markdown 'Hello  world'; got {elements:?}"
        );
        // The captured reasoning rides as a Thinking element with the
        // correct body + token count.
        let has_thinking = elements.iter().any(|e| {
            matches!(
                e,
                TurnElement::Thinking { body, tokens, .. }
                    if body == "secret reasoning" && *tokens == 42
            )
        });
        assert!(
            has_thinking,
            "expected Thinking 'secret reasoning'+42tok; got {elements:?}"
        );
    }

    #[test]
    fn stream_without_reasoning_does_not_emit_thinking_element_v093() {
        // v0.9.3 W1.3 — a plain stream (no `<think>` tags) must leave the
        // capture buffer empty and produce no Thinking element. Regression
        // guard against accidental zero-body Thinking pushes.
        let mut app = App::new();
        apply_event(
            &mut app,
            ProtocolEvent::StreamStart {
                msg_id: "m1".into(),
            },
        );
        apply_event(
            &mut app,
            ProtocolEvent::TextDelta {
                text: "no tags here".into(),
                msg_id: "m1".into(),
            },
        );
        apply_event(
            &mut app,
            ProtocolEvent::StreamEnd {
                msg_id: "m1".into(),
                finish_reason: wcore_protocol::events::FinishReason::Stop,
                usage: None,
                agent_run_id: None,
            },
        );
        let elements = &app.session.turns[0].elements;
        assert!(
            !elements
                .iter()
                .any(|e| matches!(e, TurnElement::Thinking { .. })),
            "no Thinking element expected; got {elements:?}"
        );
    }

    #[test]
    fn tool_request_then_result_drives_card_status() {
        let mut app = App::new();
        apply_event(
            &mut app,
            ProtocolEvent::ToolRequest {
                msg_id: "m1".into(),
                call_id: "c1".into(),
                tool: ToolInfo {
                    name: "Bash".into(),
                    category: ToolCategory::Exec,
                    args: json!({"command": "ls -la"}),
                    description: "list".into(),
                },
            },
        );
        assert_eq!(app.session.tool_cards.len(), 1);
        assert_eq!(app.session.tool_cards[0].status, ToolCardStatus::Running);
        assert_eq!(app.session.tool_cards[0].summary, "ls -la");

        apply_event(
            &mut app,
            ProtocolEvent::ToolResult {
                msg_id: "m1".into(),
                call_id: "c1".into(),
                tool_name: "Bash".into(),
                status: ToolStatus::Success,
                output: "total 0".into(),
                output_type: OutputType::Text,
                metadata: None,
            },
        );
        assert_eq!(app.session.tool_cards[0].status, ToolCardStatus::Ok);
        assert_eq!(app.session.tool_cards[0].output.as_deref(), Some("total 0"));
    }

    #[test]
    fn tool_error_result_marks_card_err() {
        let mut app = App::new();
        apply_event(
            &mut app,
            tool_request("c1", "Read", json!({"file_path": "x"})),
        );
        apply_event(
            &mut app,
            ProtocolEvent::ToolResult {
                msg_id: "m1".into(),
                call_id: "c1".into(),
                tool_name: "Read".into(),
                status: ToolStatus::Error,
                output: "not found".into(),
                output_type: OutputType::Text,
                metadata: None,
            },
        );
        assert_eq!(app.session.tool_cards[0].status, ToolCardStatus::Err);
    }

    #[test]
    fn tool_chunk_appends_to_card_output() {
        let mut app = App::new();
        apply_event(
            &mut app,
            tool_request("c1", "Bash", json!({"command": "x"})),
        );
        apply_event(
            &mut app,
            ProtocolEvent::ToolChunk {
                msg_id: "m1".into(),
                call_id: "c1".into(),
                tool_name: "Bash".into(),
                chunk: "line1\n".into(),
            },
        );
        apply_event(
            &mut app,
            ProtocolEvent::ToolChunk {
                msg_id: "m1".into(),
                call_id: "c1".into(),
                tool_name: "Bash".into(),
                chunk: "line2\n".into(),
            },
        );
        assert_eq!(
            app.session.tool_cards[0].output.as_deref(),
            Some("line1\nline2\n")
        );
    }

    #[test]
    fn tool_cancelled_marks_card_cancelled() {
        let mut app = App::new();
        apply_event(
            &mut app,
            tool_request("c1", "Bash", json!({"command": "x"})),
        );
        apply_event(
            &mut app,
            ProtocolEvent::ToolCancelled {
                msg_id: "m1".into(),
                call_id: "c1".into(),
                reason: "user".into(),
            },
        );
        assert_eq!(app.session.tool_cards[0].status, ToolCardStatus::Cancelled);
    }

    #[test]
    fn cancelled_tool_path_is_not_recorded_for_rewind_b1b() {
        // B1b: a tool path staged at ToolRequest but then CANCELLED (e.g.
        // approval denied) must never reach the /rewind touched-files set, so a
        // denied edit is never snapshotted at turn end.
        let mut app = App::new();
        apply_event(
            &mut app,
            tool_request("c1", "Write", json!({"file_path": "secret.txt"})),
        );
        // Staged on request, but NOT yet recorded (pre-approval).
        assert!(
            app.touched_files().is_empty(),
            "a requested-but-unapproved path must not be recorded yet"
        );
        apply_event(
            &mut app,
            ProtocolEvent::ToolCancelled {
                msg_id: "m1".into(),
                call_id: "c1".into(),
                reason: "denied".into(),
            },
        );
        assert!(
            app.touched_files().is_empty(),
            "a cancelled tool's path must never be recorded for /rewind"
        );
    }

    #[test]
    fn running_tool_path_is_recorded_for_rewind_b1b() {
        // B1b: once a tool clears approval and runs, its staged path is promoted
        // into the touched-files set so /rewind can snapshot it.
        let mut app = App::new();
        apply_event(
            &mut app,
            tool_request("c1", "Write", json!({"file_path": "notes.txt"})),
        );
        assert!(
            app.touched_files().is_empty(),
            "not recorded before the tool runs"
        );
        apply_event(
            &mut app,
            ProtocolEvent::ToolRunning {
                msg_id: "m1".into(),
                call_id: "c1".into(),
                tool_name: "Write".into(),
            },
        );
        assert_eq!(app.touched_files().len(), 1);
        assert!(app.touched_files()[0].ends_with("notes.txt"));
    }

    #[test]
    fn approval_required_downgrades_matching_card() {
        let mut app = App::new();
        apply_event(
            &mut app,
            tool_request("c1", "Bash", json!({"command": "x"})),
        );
        apply_event(
            &mut app,
            ProtocolEvent::ApprovalRequired {
                call_id: "c1".into(),
                resume_token: "t".into(),
                correlation_id: "t".into(),
                reason: "exec".into(),
                context: "running x".into(),
                plan: None,
            },
        );
        assert_eq!(
            app.session.tool_cards[0].status,
            ToolCardStatus::AwaitingApproval
        );
    }

    #[test]
    fn approval_required_without_a_card_becomes_a_system_notice() {
        let mut app = App::new();
        apply_event(
            &mut app,
            ProtocolEvent::ApprovalRequired {
                call_id: "missing".into(),
                resume_token: "t".into(),
                correlation_id: "t".into(),
                reason: "exec".into(),
                context: "ctx".into(),
                plan: None,
            },
        );
        assert_eq!(app.session.turns.len(), 1);
        assert_eq!(app.session.turns[0].role, TurnRole::System);
        assert_eq!(app.session.turns[0].elements.len(), 1);
        assert!(matches!(
            app.session.turns[0].elements[0],
            TurnElement::Markdown(_)
        ));
        assert!(app.session.turns[0].text().contains("Approval required"));
    }

    #[test]
    fn edit_request_yields_a_renderable_diff_model() {
        let mut app = App::new();
        apply_event(
            &mut app,
            tool_request(
                "c1",
                "Edit",
                json!({
                    "file_path": "src/lib.rs",
                    "old_string": "fn old() {}",
                    "new_string": "fn new() {}",
                }),
            ),
        );
        let preview = app.session.tool_cards[0]
            .edit_preview
            .as_ref()
            .expect("Edit request must carry an edit preview");
        assert_eq!(preview.path, "src/lib.rs");
        assert_eq!(preview.old, "fn old() {}");
        assert_eq!(preview.new, "fn new() {}");
    }

    #[test]
    fn write_request_yields_a_diff_model_with_empty_old() {
        let mut app = App::new();
        apply_event(
            &mut app,
            tool_request(
                "c1",
                "Write",
                json!({"file_path": "new.txt", "content": "fresh body"}),
            ),
        );
        let preview = app.session.tool_cards[0]
            .edit_preview
            .as_ref()
            .expect("Write request must carry an edit preview");
        assert_eq!(preview.path, "new.txt");
        assert!(preview.old.is_empty());
        assert_eq!(preview.new, "fresh body");
    }

    #[test]
    fn non_edit_tool_has_no_edit_preview() {
        let mut app = App::new();
        apply_event(
            &mut app,
            tool_request("c1", "Bash", json!({"command": "ls"})),
        );
        assert!(app.session.tool_cards[0].edit_preview.is_none());
    }

    #[test]
    fn file_tool_request_populates_the_path_map() {
        let mut app = App::new();
        apply_event(
            &mut app,
            tool_request("c1", "Edit", json!({"file_path": "src/lib.rs"})),
        );
        // The path map gained the `src` dir with a `lib.rs` leaf.
        assert_eq!(app.path_map.roots.len(), 1);
        assert_eq!(app.path_map.roots[0].name, "src");
        assert_eq!(app.path_map.roots[0].children[0].name, "lib.rs");
    }

    #[test]
    fn non_file_tool_request_leaves_path_map_empty() {
        let mut app = App::new();
        apply_event(
            &mut app,
            tool_request("c1", "Bash", json!({"command": "ls"})),
        );
        assert!(app.path_map.roots.is_empty());
    }

    #[test]
    fn enter_plan_mode_tool_populates_app_plan() {
        let mut app = App::new();
        apply_event(
            &mut app,
            tool_request(
                "c1",
                "EnterPlanMode",
                json!({"plan": "Step one\nStep two", "title": "Refactor auth"}),
            ),
        );
        let plan = app.plan.as_ref().expect("EnterPlanMode must set app.plan");
        assert_eq!(plan.title, "Refactor auth");
        assert!(plan.body.contains("Step one"));
    }

    #[test]
    fn exit_plan_mode_tool_clears_app_plan() {
        let mut app = App::new();
        apply_event(
            &mut app,
            tool_request("c1", "EnterPlanMode", json!({"plan": "x"})),
        );
        assert!(app.plan.is_some());
        apply_event(&mut app, tool_request("c2", "ExitPlanMode", json!({})));
        assert!(app.plan.is_none(), "ExitPlanMode must clear the plan");
    }

    #[test]
    fn exit_plan_mode_card_captures_plan_body_before_clear() {
        // v0.9.2 W11-integ (SPEC §2 #8): the ExitPlanMode tool card must
        // snapshot the live plan body onto `card.plan_body` on the SAME
        // event that clears `app.plan` — otherwise the exit_plan component
        // renders "(plan body unavailable)".
        let mut app = App::new();
        apply_event(
            &mut app,
            tool_request(
                "c1",
                "EnterPlanMode",
                json!({"plan": "Step one\nStep two", "title": "Refactor auth"}),
            ),
        );
        apply_event(&mut app, tool_request("c2", "ExitPlanMode", json!({})));

        let card = app
            .session
            .tool_cards
            .iter()
            .find(|c| c.call_id == "c2")
            .expect("ExitPlanMode card must exist");
        let body = card
            .plan_body
            .as_ref()
            .expect("ExitPlanMode card must carry the captured plan body");
        assert!(body.contains("Step one"), "captured body: {body:?}");
        assert!(body.contains("Step two"), "captured body: {body:?}");
        // The live plan is still cleared.
        assert!(app.plan.is_none(), "ExitPlanMode still clears app.plan");
    }

    #[test]
    fn non_exit_plan_tool_leaves_plan_body_none() {
        let mut app = App::new();
        apply_event(
            &mut app,
            tool_request("c1", "Bash", json!({"command": "ls"})),
        );
        let card = app
            .session
            .tool_cards
            .iter()
            .find(|c| c.call_id == "c1")
            .expect("Bash card must exist");
        assert!(
            card.plan_body.is_none(),
            "only ExitPlanMode captures a plan body"
        );
    }

    #[test]
    fn sub_agent_event_registers_and_updates_a_view() {
        let mut app = App::new();
        apply_event(
            &mut app,
            ProtocolEvent::SubAgentEvent {
                parent_call_id: "spawn:reviewer".into(),
                agent_name: "reviewer".into(),
                inner: json!({"type": "text_delta", "text": "looking...", "msg_id": "s1"}),
            },
        );
        assert_eq!(app.session.sub_agents.len(), 1);
        let sa = &app.session.sub_agents[0];
        assert_eq!(sa.id, "spawn:reviewer");
        assert_eq!(sa.name, "reviewer");
        assert_eq!(sa.status, SubAgentStatus::Running);
        assert_eq!(sa.feed, vec!["looking...".to_string()]);

        // A second event for the same parent updates the SAME view.
        apply_event(
            &mut app,
            ProtocolEvent::SubAgentEvent {
                parent_call_id: "spawn:reviewer".into(),
                agent_name: "reviewer".into(),
                inner: json!({
                    "type": "stream_end",
                    "msg_id": "s1",
                    "finish_reason": "stop",
                    "usage": {"input_tokens": 30, "output_tokens": 70},
                }),
            },
        );
        assert_eq!(app.session.sub_agents.len(), 1);
        assert_eq!(app.session.sub_agents[0].turns, 1);
        assert_eq!(app.session.sub_agents[0].tokens, 100);
    }

    #[test]
    fn sub_agent_error_marks_the_view_failed() {
        let mut app = App::new();
        apply_event(
            &mut app,
            ProtocolEvent::SubAgentEvent {
                parent_call_id: "spawn:x".into(),
                agent_name: "x".into(),
                inner: json!({
                    "type": "error",
                    "error": {"code": "boom", "message": "it broke", "retryable": false},
                }),
            },
        );
        assert_eq!(app.session.sub_agents[0].status, SubAgentStatus::Failed);
    }

    /// W5.5 F1 regression: `ChannelSink::emit_error` now relays
    /// `ProtocolEvent::Error` (not `Info`). The exact serialization is
    /// `{"type":"error","error":{"code":"sub_agent_error","message":"...","retryable":false}}`.
    /// This test verifies the bridge maps THAT shape to `SubAgentStatus::Failed`.
    /// Before the fix, `emit_error` relayed `ProtocolEvent::Info` which the
    /// bridge mapped to `SubAgentStatus::Done` — a crashed sub-agent looked green.
    #[test]
    fn emit_error_relay_shape_maps_to_failed_w55_f1() {
        let mut app = App::new();
        // This is the exact JSON shape that ChannelSink::emit_error now produces
        // via ProtocolEvent::Error { msg_id: None, error: ErrorInfo { code:
        // "sub_agent_error", message: "...", retryable: false } }.
        apply_event(
            &mut app,
            ProtocolEvent::SubAgentEvent {
                parent_call_id: "spawn:0:failed-agent".into(),
                agent_name: "failed-agent".into(),
                inner: json!({
                    "type": "error",
                    "error": {
                        "code": "sub_agent_error",
                        "message": "engine crashed: API 500",
                        "retryable": false
                    }
                }),
            },
        );
        assert_eq!(
            app.session.sub_agents[0].status,
            SubAgentStatus::Failed,
            "W5.5 F1: the error relay shape produced by ChannelSink::emit_error must \
             map to SubAgentStatus::Failed, not Done. Before the fix, emit_error relayed \
             ProtocolEvent::Info which the bridge treated as Done (ghost-green)."
        );
        // Confirm the error message appears in the feed.
        let feed = &app.session.sub_agents[0].feed;
        assert!(
            feed.iter().any(|l| l.contains("engine crashed")),
            "error message must appear in feed. Feed: {:?}",
            feed
        );
    }

    // ── v0.9.4 W1 relay-substrate dormancy guard ─────────────────────

    /// (a) v0.9.4 W1 — two SubAgentEvents with DIFFERENT parent_call_ids must
    /// produce TWO distinct SubAgentViews in app.session.sub_agents. This is the
    /// key regression guard for the v0.9.3 dormancy: before the fix, one shared
    /// parent_call_id caused both tasks to collapse into a single row.
    #[test]
    fn two_distinct_sub_agent_events_produce_two_views_v094_w1() {
        let mut app = App::new();

        // Task 0: agent-a with its own per-task parent_call_id.
        apply_event(
            &mut app,
            ProtocolEvent::SubAgentEvent {
                parent_call_id: "spawn:0:agent-a".into(),
                agent_name: "agent-a".into(),
                inner: json!({"type": "text_delta", "text": "working on A...", "msg_id": "s0"}),
            },
        );
        // Task 1: agent-b with a DIFFERENT parent_call_id.
        apply_event(
            &mut app,
            ProtocolEvent::SubAgentEvent {
                parent_call_id: "spawn:1:agent-b".into(),
                agent_name: "agent-b".into(),
                inner: json!({"type": "text_delta", "text": "working on B...", "msg_id": "s1"}),
            },
        );

        // THE assertion that caught the v0.9.3 dormancy: must be 2, not 1.
        assert_eq!(
            app.session.sub_agents.len(),
            2,
            "two distinct parent_call_ids must create two distinct SubAgentViews; \
             if len==1, the per-task keying fix (W1.1) is missing"
        );

        let view_a = app
            .session
            .sub_agents
            .iter()
            .find(|s| s.id == "spawn:0:agent-a")
            .expect("SubAgentView for agent-a must exist");
        let view_b = app
            .session
            .sub_agents
            .iter()
            .find(|s| s.id == "spawn:1:agent-b")
            .expect("SubAgentView for agent-b must exist");

        assert_eq!(view_a.name, "agent-a");
        assert_eq!(view_b.name, "agent-b");
        assert_eq!(view_a.status, SubAgentStatus::Running);
        assert_eq!(view_b.status, SubAgentStatus::Running);
        // Each view has its own distinct feed.
        assert!(
            view_a.feed.iter().any(|l| l.contains("working on A")),
            "agent-a feed must contain its own text"
        );
        assert!(
            view_b.feed.iter().any(|l| l.contains("working on B")),
            "agent-b feed must contain its own text"
        );
    }

    /// (b) v0.9.4 W1.1b — a terminal "info" event sets SubAgentStatus::Done.
    /// Before the fix, spawn_one_with_extras never called emit_info(), so the
    /// view's status was permanently stuck at Running.
    #[test]
    fn terminal_info_event_sets_status_done_v094_w1b() {
        let mut app = App::new();

        // Prime the view with a text event (Running state).
        apply_event(
            &mut app,
            ProtocolEvent::SubAgentEvent {
                parent_call_id: "spawn:0:solo".into(),
                agent_name: "solo".into(),
                inner: json!({"type": "text_delta", "text": "processing...", "msg_id": "s0"}),
            },
        );
        assert_eq!(app.session.sub_agents[0].status, SubAgentStatus::Running);

        // Terminal info event: bridge must flip status to Done.
        apply_event(
            &mut app,
            ProtocolEvent::SubAgentEvent {
                parent_call_id: "spawn:0:solo".into(),
                agent_name: "solo".into(),
                inner: json!({"type": "info", "msg_id": "", "message": "sub-agent 'solo' completed (1 turns)"}),
            },
        );
        assert_eq!(
            app.session.sub_agents[0].status,
            SubAgentStatus::Done,
            "terminal 'info' event must set status to Done (W1.1b); \
             if still Running, emit_info() was not called before the ChannelSink dropped"
        );
    }

    #[test]
    fn error_and_info_events_become_system_turns() {
        let mut app = App::new();
        apply_event(
            &mut app,
            ProtocolEvent::Error {
                msg_id: None,
                error: ErrorInfo {
                    code: "rate_limit".into(),
                    message: "slow down".into(),
                    retryable: true,
                },
            },
        );
        apply_event(
            &mut app,
            ProtocolEvent::Info {
                msg_id: "m1".into(),
                message: "heads up".into(),
            },
        );
        assert_eq!(app.session.turns.len(), 2);
        assert!(app.session.turns.iter().all(|t| t.role == TurnRole::System));
        // Each system notice lands as a single Markdown element (no
        // Sources, no Thinking) per the A2 contract.
        for turn in &app.session.turns {
            assert_eq!(turn.elements.len(), 1);
            assert!(
                matches!(turn.elements[0], TurnElement::Markdown(_)),
                "system notices must be a single Markdown element"
            );
        }
        // v0.9.1.1 H3: the internal class name `rate_limit` is mapped
        // to a user-facing label ("Rate limit"). The raw class name no
        // longer leaks into the transcript — that ships to the log file
        // via `tracing::debug!` for operator diagnosis instead.
        let rendered = app.session.turns[0].text();
        assert!(!rendered.contains("rate_limit"), "got: {rendered}");
        assert!(rendered.contains("Rate limit"), "got: {rendered}");
        assert_eq!(app.session.turns[1].text(), "heads up");
    }

    #[test]
    fn engine_error_classname_does_not_leak_to_activity_rail_v0911() {
        // v0.9.1.1 H3 regression — the raw class name `engine_error`
        // / `engine_panic` is an internal enum, not user-facing. The
        // transcript must show a clean "Error: ..." / "Turn ended
        // unexpectedly: ..." prefix; the raw class is routed to
        // `tracing::debug!` for operator diagnosis.
        let mut app = App::new();
        apply_event(
            &mut app,
            ProtocolEvent::Error {
                msg_id: None,
                error: ErrorInfo {
                    code: "engine_error".into(),
                    message: "API 400: invalid_request_error tool_use…".into(),
                    retryable: false,
                },
            },
        );
        let rendered = app.session.turns[0].text();
        assert!(
            !rendered.contains("engine_error"),
            "raw class name leaked into rail: {rendered}"
        );
        assert!(
            rendered.starts_with("Error:"),
            "expected clean `Error:` prefix, got: {rendered}"
        );

        apply_event(
            &mut app,
            ProtocolEvent::Error {
                msg_id: None,
                error: ErrorInfo {
                    code: "engine_panic".into(),
                    message: "The turn ended unexpectedly".into(),
                    retryable: true,
                },
            },
        );
        let rendered = app.session.turns[1].text();
        assert!(
            !rendered.contains("engine_panic"),
            "raw class name leaked into rail: {rendered}"
        );
        assert!(
            rendered.contains("Turn ended unexpectedly"),
            "expected user-friendly label, got: {rendered}"
        );
    }

    #[test]
    fn unknown_error_codes_fall_back_to_raw_chip_v0911() {
        // v0.9.1.1 H3: an error code not in the known-class map (a
        // future class added in a later patch) must still surface in
        // the rail rather than vanish silently. The fallback renders
        // the raw `[code]` chip; this is the documented safety net.
        let mut app = App::new();
        apply_event(
            &mut app,
            ProtocolEvent::Error {
                msg_id: None,
                error: ErrorInfo {
                    code: "novel_class_not_yet_mapped".into(),
                    message: "experimental failure mode".into(),
                    retryable: false,
                },
            },
        );
        let rendered = app.session.turns[0].text();
        assert!(
            rendered.contains("novel_class_not_yet_mapped"),
            "fallback should render unknown class as a chip, got: {rendered}"
        );
    }

    #[test]
    fn session_cost_no_longer_creates_transcript_turn() {
        // v0.9.1 W1-B Bug 1: `SessionCost` used to push a system turn
        // ("Session cost: $X across N turn(s)") which spammed the
        // transcript — one such turn per assistant turn. The fix is to
        // store the structured payload on `app.cost` only; the status
        // bar reads `app.cost.total_cost_usd` and the diagnostics
        // `/cost` screen renders the per-turn breakdown.
        let mut app = App::new();
        apply_event(
            &mut app,
            ProtocolEvent::SessionCost {
                session_id: "s1".into(),
                total_cost_usd: 0.1234,
                per_turn: vec![],
            },
        );
        assert_eq!(
            app.session.turns.len(),
            0,
            "SessionCost must not create a transcript turn"
        );
        let cost = app.cost.as_ref().expect("app.cost should be populated");
        assert_eq!(cost.session_id, "s1");
        assert!((cost.total_cost_usd - 0.1234).abs() < 1e-9);
    }

    #[test]
    fn mcp_ready_no_longer_creates_transcript_turn() {
        // v0.9.1 W1-B Bug 2: `McpReady` used to push "MCP server X
        // ready — N tool(s)" turns at session start, drowning the first
        // user message when several MCP servers were configured. The
        // fix is to write to `app.mcp_status` so `/doctor` and the
        // right-rail Activity panel can surface the count without
        // polluting the conversation.
        let mut app = App::new();
        apply_event(
            &mut app,
            ProtocolEvent::McpReady {
                name: "github".into(),
                tools: vec!["search".into(), "fetch".into()],
            },
        );
        assert_eq!(
            app.session.turns.len(),
            0,
            "McpReady must not create a transcript turn"
        );
        let status = app
            .mcp_status
            .get("github")
            .expect("app.mcp_status should record the server");
        match status {
            crate::tui::app::McpServerStatus::Ready { tool_count } => {
                assert_eq!(*tool_count, 2);
            }
            other => panic!("expected Ready, got {other:?}"),
        }
    }

    #[test]
    fn mcp_failed_records_the_cause_on_status_without_a_transcript_turn() {
        // A3: a failed MCP server must surface its cause on `mcp_status`
        // (so `/doctor` can show *why* the tools never appeared) and a
        // transient toast — never a transcript turn.
        let mut app = App::new();
        apply_event(
            &mut app,
            ProtocolEvent::McpFailed {
                name: "stripe".into(),
                reason: "spawn node: ENOENT".into(),
            },
        );
        assert_eq!(
            app.session.turns.len(),
            0,
            "McpFailed must not create a transcript turn"
        );
        match app
            .mcp_status
            .get("stripe")
            .expect("app.mcp_status should record the failed server")
        {
            crate::tui::app::McpServerStatus::Failed { reason } => {
                assert_eq!(reason, "spawn node: ENOENT");
            }
            other => panic!("expected Failed, got {other:?}"),
        }
        let toast = app.toast.as_ref().expect("McpFailed should set a toast");
        assert!(toast.contains("stripe"), "toast names the server: {toast}");
        assert!(toast.contains("failed"), "toast marks the failure: {toast}");
    }

    #[test]
    fn mcp_ready_sets_a_transient_toast_not_a_transcript_turn() {
        // v0.9.2 W5 (Task 5.6 / SPEC §3 S7): McpReady is demoted to a
        // status-bar toast. The bridge sets `app.toast` + `app.toast_at`
        // (the status bar renders + auto-dismisses); it must NOT push a
        // transcript turn.
        let mut app = App::new();
        assert!(app.toast.is_none(), "toast must start empty");
        apply_event(
            &mut app,
            ProtocolEvent::McpReady {
                name: "github".into(),
                tools: vec!["search".into(), "fetch".into()],
            },
        );
        assert_eq!(
            app.session.turns.len(),
            0,
            "McpReady must not create a transcript turn"
        );
        let toast = app.toast.as_ref().expect("McpReady should set a toast");
        assert!(toast.contains("github"), "toast names the server: {toast}");
        assert!(toast.contains('2'), "toast carries the tool count: {toast}");
        assert!(
            app.toast_at.is_some(),
            "toast_at must be stamped for auto-dismiss"
        );
    }

    #[test]
    fn stream_start_subscribes_the_clock_and_stream_end_releases_it() {
        // v0.9.2 W1 (Task 1.5 / SPEC §1A): a turn in flight pins the shared
        // animation clock; an idle prompt releases it so the render loop can
        // drop to its zero-CPU long-dwell.
        let mut app = App::new();
        assert!(
            !app.anim.wants_tick(),
            "fresh app is idle — clock wants no tick"
        );
        apply_event(
            &mut app,
            ProtocolEvent::StreamStart {
                msg_id: "m1".into(),
            },
        );
        assert!(
            app.anim.wants_tick(),
            "StreamStart must subscribe the clock"
        );
        apply_event(
            &mut app,
            ProtocolEvent::StreamEnd {
                msg_id: "m1".into(),
                finish_reason: wcore_protocol::events::FinishReason::Stop,
                usage: None,
                agent_run_id: None,
            },
        );
        assert!(
            !app.anim.wants_tick(),
            "StreamEnd must unsubscribe the clock"
        );
    }

    #[test]
    fn stream_error_releases_the_clock() {
        // A failed turn is terminal too — an Error with no following
        // StreamEnd must not pin the clock and burn idle CPU.
        let mut app = App::new();
        apply_event(
            &mut app,
            ProtocolEvent::StreamStart {
                msg_id: "m1".into(),
            },
        );
        assert!(app.anim.wants_tick());
        apply_event(
            &mut app,
            ProtocolEvent::Error {
                msg_id: Some("m1".into()),
                error: ErrorInfo {
                    code: "engine_error".into(),
                    message: "boom".into(),
                    retryable: false,
                },
            },
        );
        assert!(
            !app.anim.wants_tick(),
            "Error must release the clock subscriptions"
        );
    }

    #[test]
    fn stream_start_sets_a_nonzero_per_turn_verb_seed_that_varies() {
        // v0.9.2 W6 (Task 6.2 / SPEC §4): the verb seed is picked ONCE per
        // StreamStart so the streaming-status verb stays constant for the
        // whole turn, and consecutive turns land on different pool indices.
        let mut app = App::new();
        assert_eq!(
            app.session.turn_verb_seed, 0,
            "seed starts at 0 between turns"
        );
        apply_event(
            &mut app,
            ProtocolEvent::StreamStart {
                msg_id: "m1".into(),
            },
        );
        let first = app.session.turn_verb_seed;
        assert_ne!(first, 0, "StreamStart must pick a non-zero seed");
        apply_event(
            &mut app,
            ProtocolEvent::StreamEnd {
                msg_id: "m1".into(),
                finish_reason: wcore_protocol::events::FinishReason::Stop,
                usage: None,
                agent_run_id: None,
            },
        );
        apply_event(
            &mut app,
            ProtocolEvent::StreamStart {
                msg_id: "m2".into(),
            },
        );
        assert_ne!(
            app.session.turn_verb_seed, first,
            "consecutive turns must produce different seeds"
        );
    }

    #[test]
    fn plugin_hook_event_no_longer_creates_transcript_turn() {
        // v0.9.1 W1-B Bug 3: lifecycle plugin events (`PluginEvent`)
        // must not push system turns. They are a no-op in the bridge
        // (the explicit no-op arm at the bottom of `apply_event`).
        // Only `PluginRegistrationFailed` — an error — surfaces in the
        // transcript, which is correct per HTML mockup §7 routing
        // (errors stay in conversation).
        let mut app = App::new();
        apply_event(
            &mut app,
            ProtocolEvent::PluginEvent {
                plugin_name: "example-plugin".into(),
                event_type: "on_session_start".into(),
                payload: serde_json::Value::Null,
            },
        );
        assert_eq!(
            app.session.turns.len(),
            0,
            "PluginEvent (lifecycle) must not create a transcript turn"
        );
    }

    #[test]
    fn approval_required_sets_card_status_without_surface_push() {
        // v0.9.1 W1-B: the protocol bridge handler for ApprovalRequired
        // never pushed a surface (the centered-modal auto-open lived in
        // `surfaces::Router::sync_approval_modal`, which has been
        // removed). The handler's job is to flip the matching card's
        // status to AwaitingApproval and stash the reason on the card —
        // the inline approval card renders from that.
        let mut app = App::new();
        let tool = wcore_protocol::events::ToolInfo {
            name: "Bash".into(),
            category: wcore_protocol::events::ToolCategory::Exec,
            args: serde_json::json!({"cmd": "ls"}),
            description: "run a shell command".into(),
        };
        apply_event(
            &mut app,
            ProtocolEvent::ToolRequest {
                msg_id: "m1".into(),
                call_id: "call-1".into(),
                tool,
            },
        );
        apply_event(
            &mut app,
            ProtocolEvent::ApprovalRequired {
                call_id: "call-1".into(),
                resume_token: "tok-1".into(),
                correlation_id: String::new(),
                reason: "writes a file".into(),
                context: String::new(),
                plan: None,
            },
        );

        assert_eq!(
            app.session.tool_cards.len(),
            1,
            "ToolRequest must have created exactly one card"
        );
        let card = &app.session.tool_cards[0];
        assert_eq!(card.status, ToolCardStatus::AwaitingApproval);
        assert_eq!(card.approval_reason, "writes a file");
        // No surface field exists on `App` for an overlay — but assert
        // overlay is empty so a future regression cannot reintroduce a
        // modal auto-push via this handler.
        assert!(
            app.overlay.is_none(),
            "ApprovalRequired must not open an overlay"
        );
        // v0.9.1.2 F12: ToolRequest opens an in-flight assistant turn
        // and parks a `TurnElement::ToolCard(call_id)` in it (the
        // interleave anchor). The ApprovalRequired handler must NOT push
        // an additional system turn beyond that — exactly one turn must
        // exist, and it must be the in-flight assistant turn with the
        // card placeholder.
        assert_eq!(
            app.session.turns.len(),
            1,
            "ToolRequest opens exactly one in-flight assistant turn"
        );
        assert_eq!(
            app.session.turns[0].role,
            TurnRole::Assistant,
            "the in-flight turn must be an assistant turn, not a system notice"
        );
        match &app.session.turns[0].elements[..] {
            [TurnElement::ToolCard(id)] => assert_eq!(id, "call-1"),
            other => panic!("expected single ToolCard(call-1) element, got {other:?}"),
        }
    }

    // ── AUDIT-D D2 — streaming state self-heals on every terminal path ─

    #[test]
    fn error_event_clears_streaming_active() {
        // `StreamEnd` is no longer the *only* event that clears the
        // streaming spinner — an `Error` is a terminal outcome too, so it
        // must drop `streaming_active` (AUDIT-D D2). Otherwise an engine
        // error with no following `StreamEnd` strands the spinner.
        let mut app = App::new();
        apply_event(
            &mut app,
            ProtocolEvent::StreamStart {
                msg_id: "m1".into(),
            },
        );
        assert!(app.session.streaming_active, "stream must be live first");

        apply_event(
            &mut app,
            ProtocolEvent::Error {
                msg_id: Some("m1".into()),
                error: ErrorInfo {
                    code: "engine_error".into(),
                    message: "boom".into(),
                    retryable: false,
                },
            },
        );
        assert!(
            !app.session.streaming_active,
            "an Error event must clear streaming_active"
        );
    }

    #[test]
    fn error_event_flushes_partial_stream_into_a_turn() {
        // An error after some text streamed must not lose that text: the
        // partial stream is flushed into an assistant turn before the
        // streaming buffer is cleared (AUDIT-D D2).
        let mut app = App::new();
        apply_event(
            &mut app,
            ProtocolEvent::StreamStart {
                msg_id: "m1".into(),
            },
        );
        apply_event(
            &mut app,
            ProtocolEvent::TextDelta {
                text: "partial answer".into(),
                msg_id: "m1".into(),
            },
        );
        apply_event(
            &mut app,
            ProtocolEvent::Error {
                msg_id: Some("m1".into()),
                error: ErrorInfo {
                    code: "engine_error".into(),
                    message: "boom".into(),
                    retryable: false,
                },
            },
        );
        assert!(app.session.streaming.is_empty(), "stream buffer flushed");
        // The transcript holds the flushed partial turn + the error notice.
        let partial = app
            .session
            .turns
            .iter()
            .find(|t| t.role == TurnRole::Assistant)
            .expect("partial streamed text must be preserved as a turn");
        // The partial stream lands as a single `Markdown` element, same
        // shape as a clean `StreamEnd` flush.
        assert_eq!(partial.elements.len(), 1);
        match &partial.elements[0] {
            TurnElement::Markdown(s) => assert_eq!(s, "partial answer"),
            other => panic!("expected Markdown element, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn closed_engine_channel_clears_streaming_active() {
        // When every sender half of the engine channel is dropped, no
        // terminal `StreamEnd` can ever arrive — `spawn_bridge` must
        // force `streaming_active` off on channel close so the spinner
        // cannot stick on (AUDIT-D D2, defense in depth behind D1).
        let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
        let app = Arc::new(Mutex::new(App::new()));
        // Seed a live stream.
        tx.send(ProtocolEvent::StreamStart {
            msg_id: "m1".into(),
        })
        .expect("seed event");

        spawn_bridge(rx, app.clone(), Arc::new(Notify::new()));
        // Drop the sender — the channel closes, the bridge drains the
        // seeded event and then sees `recv()` return `None`.
        drop(tx);
        // Let the bridge task run to completion.
        for _ in 0..50 {
            tokio::task::yield_now().await;
            if !app.lock().unwrap().session.streaming_active {
                break;
            }
        }
        assert!(
            !app.lock().unwrap().session.streaming_active,
            "a closed engine channel must clear streaming_active"
        );
    }

    #[tokio::test]
    async fn bridge_wakes_render_loop_on_event_v092() {
        // v0.9.2 audit H2: a bridge push must signal the render loop's
        // `wake` Notify so the idle `select!` resolves immediately instead
        // of waiting out the up-to-200ms IDLE_SLICE input poll. We prove
        // the signal point by sending one event and asserting `notified()`
        // resolves — a `timeout` guards against a regression that drops the
        // wake (the test would hang otherwise).
        let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
        let app = Arc::new(Mutex::new(App::new()));
        let wake = Arc::new(Notify::new());

        spawn_bridge(rx, app.clone(), wake.clone());
        // A bridge push at idle — the engine's first streamed token.
        tx.send(ProtocolEvent::StreamStart {
            msg_id: "m1".into(),
        })
        .expect("seed event");

        // The loop's idle arm: `wake.notified()`. It must resolve once the
        // bridge has applied the event. Notify stores one permit, so even
        // if the bridge signals before we await, the permit is not lost.
        let woke = tokio::time::timeout(std::time::Duration::from_secs(2), wake.notified()).await;
        assert!(
            woke.is_ok(),
            "bridge did not signal the render-loop wake on an inbound event"
        );
    }

    #[test]
    fn late_text_delta_after_stream_end_is_ignored() {
        // AUDIT-D D7: a stray `TextDelta` that arrives after the stream
        // is no longer active (a cancelled turn's task emitting one last
        // delta past the synthetic `StreamEnd`) must be dropped, not
        // appended — otherwise it leaves a stale fragment in `streaming`
        // that no `StreamEnd` ever flushes.
        let mut app = App::new();
        // No StreamStart — streaming is inactive.
        assert!(!app.session.streaming_active);
        apply_event(
            &mut app,
            ProtocolEvent::TextDelta {
                text: "stray late delta".into(),
                msg_id: "cancelled".into(),
            },
        );
        assert!(
            app.session.streaming.is_empty(),
            "a delta with no active stream must be ignored"
        );
        // A late delta must not create a phantom Assistant turn — the
        // invariant is "late deltas don't append to a closed turn".
        assert!(
            app.session.turns.is_empty(),
            "a late delta must not push a new turn"
        );
    }

    #[test]
    fn no_view_impact_variants_are_silent_no_ops() {
        let mut app = App::new();
        apply_event(&mut app, ProtocolEvent::Pong);
        apply_event(
            &mut app,
            ProtocolEvent::Suspend {
                reason: "approval".into(),
                resume_token: "t".into(),
            },
        );
        assert!(app.session.turns.is_empty());
        assert!(app.session.tool_cards.is_empty());
        assert!(!app.session.streaming_active);
    }

    // ── Fixture-driven end-to-end checks ─────────────────────────────

    #[test]
    fn conversation_fixture_reaches_expected_state() {
        let mut app = App::new();
        for ev in fixtures::full_conversation() {
            apply_event(&mut app, ev);
        }
        // The fixture is the engine-side stream only; it flushes one
        // assistant turn. (The user `TurnView` is added by the surface
        // router on submit, not by the bridge — there is no
        // `ProtocolEvent` for a user message.)
        assert_eq!(app.session.turns.len(), 1);
        assert_eq!(app.session.turns[0].role, TurnRole::Assistant);
        // Single Markdown element — one flush per stream, per A2 contract.
        assert_eq!(app.session.turns[0].elements.len(), 1);
        assert!(matches!(
            app.session.turns[0].elements[0],
            TurnElement::Markdown(_)
        ));
        assert_eq!(app.session.turns[0].text(), "Hello! How can I help?");
        assert!(!app.session.streaming_active);
        assert!(app.session.streaming.is_empty());
    }

    #[test]
    fn edit_tool_fixture_yields_a_renderable_diff_model() {
        let mut app = App::new();
        for ev in fixtures::edit_tool_call() {
            apply_event(&mut app, ev);
        }
        assert_eq!(app.session.tool_cards.len(), 1);
        let card = &app.session.tool_cards[0];
        assert_eq!(card.status, ToolCardStatus::Ok);
        let preview = card
            .edit_preview
            .as_ref()
            .expect("edit fixture must yield a DiffModel");
        assert_eq!(preview.path, "crates/wcore-cli/src/main.rs");
        assert!(preview.new.contains("run()"));
    }

    #[test]
    fn tool_call_with_approval_fixture_reaches_expected_state() {
        let mut app = App::new();
        for ev in fixtures::tool_call_with_approval() {
            apply_event(&mut app, ev);
        }
        assert_eq!(app.session.tool_cards.len(), 1);
        let card = &app.session.tool_cards[0];
        // request → approval → result: the final state is Ok.
        assert_eq!(card.status, ToolCardStatus::Ok);
        assert!(card.output.is_some());
    }

    #[test]
    fn sub_agent_spawn_fixture_registers_a_sub_agent() {
        let mut app = App::new();
        for ev in fixtures::sub_agent_spawn() {
            apply_event(&mut app, ev);
        }
        assert_eq!(app.session.sub_agents.len(), 1);
        let sa = &app.session.sub_agents[0];
        assert_eq!(sa.status, SubAgentStatus::Done);
        assert!(sa.turns >= 1);
        assert!(!sa.feed.is_empty());
    }

    // ── W3 D4 — Sources block collection on StreamEnd ────────────────

    /// Drive a complete stream — `StreamStart` → `TextDelta(body)` →
    /// `StreamEnd` — so the URL-collection path runs end-to-end. Used
    /// by the no-tool tests below where only the body matters.
    fn stream_with_body(app: &mut App, body: &str) {
        apply_event(
            app,
            ProtocolEvent::StreamStart {
                msg_id: "m1".into(),
            },
        );
        apply_event(
            app,
            ProtocolEvent::TextDelta {
                text: body.into(),
                msg_id: "m1".into(),
            },
        );
        apply_event(
            app,
            ProtocolEvent::StreamEnd {
                msg_id: "m1".into(),
                finish_reason: wcore_protocol::events::FinishReason::Stop,
                usage: None,
                agent_run_id: None,
            },
        );
    }

    #[test]
    fn stream_end_with_markdown_links_adds_sources_element() {
        // The streamed body contains a `[text](url)` link → the
        // assistant turn gets BOTH a `Markdown` element (the body) AND
        // a `Sources` element (the URL list).
        let mut app = App::new();
        stream_with_body(
            &mut app,
            "See [the docs](https://example.com/docs) for details.",
        );
        assert_eq!(app.session.turns.len(), 1);
        let turn = &app.session.turns[0];
        assert_eq!(turn.role, TurnRole::Assistant);
        // 1st element is the body markdown; 2nd is the Sources block.
        assert_eq!(turn.elements.len(), 2);
        assert!(matches!(turn.elements[0], TurnElement::Markdown(_)));
        match &turn.elements[1] {
            TurnElement::Sources(urls) => {
                assert_eq!(urls, &vec!["https://example.com/docs".to_string()]);
            }
            other => panic!("expected Sources element, got {other:?}"),
        }
    }

    #[test]
    fn stream_end_with_tool_urls_adds_sources_element() {
        // A `web` tool result carries URLs via its formatter's
        // `extract_urls`. The Sources element must include them even
        // when the body has no inline markdown links.
        let mut app = App::new();
        apply_event(
            &mut app,
            ProtocolEvent::StreamStart {
                msg_id: "m1".into(),
            },
        );
        apply_event(
            &mut app,
            ProtocolEvent::ToolRequest {
                msg_id: "m1".into(),
                call_id: "c1".into(),
                tool: ToolInfo {
                    name: "web".into(),
                    category: ToolCategory::Info,
                    args: json!({"query": "rust"}),
                    description: String::new(),
                },
            },
        );
        apply_event(
            &mut app,
            ProtocolEvent::ToolResult {
                msg_id: "m1".into(),
                call_id: "c1".into(),
                tool_name: "web".into(),
                status: ToolStatus::Success,
                output: json!({
                    "results": [
                        {"title": "T1", "url": "https://a.example/1"},
                        {"title": "T2", "url": "https://b.example/2"},
                    ]
                })
                .to_string(),
                output_type: OutputType::Text,
                metadata: None,
            },
        );
        apply_event(
            &mut app,
            ProtocolEvent::TextDelta {
                text: "Here is what I found.".into(),
                msg_id: "m1".into(),
            },
        );
        apply_event(
            &mut app,
            ProtocolEvent::StreamEnd {
                msg_id: "m1".into(),
                finish_reason: wcore_protocol::events::FinishReason::Stop,
                usage: None,
                agent_run_id: None,
            },
        );
        let turn = app
            .session
            .turns
            .iter()
            .find(|t| t.role == TurnRole::Assistant)
            .expect("assistant turn must exist");
        let sources = turn
            .elements
            .iter()
            .find_map(|e| match e {
                TurnElement::Sources(u) => Some(u),
                _ => None,
            })
            .expect("Sources element must be appended");
        assert_eq!(
            sources,
            &vec![
                "https://a.example/1".to_string(),
                "https://b.example/2".to_string(),
            ]
        );
    }

    #[test]
    fn stream_end_with_no_urls_omits_sources_element() {
        // Neither the body nor any tool card produced URLs → the turn
        // must carry the single Markdown element with NO Sources tail.
        let mut app = App::new();
        stream_with_body(&mut app, "Plain answer with no links.");
        let turn = &app.session.turns[0];
        assert_eq!(
            turn.elements.len(),
            1,
            "no-URL turn must remain single-Markdown"
        );
        assert!(
            !turn
                .elements
                .iter()
                .any(|e| matches!(e, TurnElement::Sources(_))),
            "no Sources element should be present"
        );
    }

    #[test]
    fn urls_deduped_across_markdown_and_tools() {
        // The same URL appearing in BOTH the body and a tool result
        // must surface only once. Dedup preserves the first sighting —
        // the body link comes before the tool URL list.
        let mut app = App::new();
        apply_event(
            &mut app,
            ProtocolEvent::StreamStart {
                msg_id: "m1".into(),
            },
        );
        apply_event(
            &mut app,
            ProtocolEvent::ToolRequest {
                msg_id: "m1".into(),
                call_id: "c1".into(),
                tool: ToolInfo {
                    name: "web".into(),
                    category: ToolCategory::Info,
                    args: json!({"q": "x"}),
                    description: String::new(),
                },
            },
        );
        apply_event(
            &mut app,
            ProtocolEvent::ToolResult {
                msg_id: "m1".into(),
                call_id: "c1".into(),
                tool_name: "web".into(),
                status: ToolStatus::Success,
                output: json!({
                    "results": [
                        {"title": "Dup", "url": "https://dup.example/x"},
                        {"title": "Uniq", "url": "https://uniq.example/y"},
                    ]
                })
                .to_string(),
                output_type: OutputType::Text,
                metadata: None,
            },
        );
        apply_event(
            &mut app,
            ProtocolEvent::TextDelta {
                text: "See [dup](https://dup.example/x).".into(),
                msg_id: "m1".into(),
            },
        );
        apply_event(
            &mut app,
            ProtocolEvent::StreamEnd {
                msg_id: "m1".into(),
                finish_reason: wcore_protocol::events::FinishReason::Stop,
                usage: None,
                agent_run_id: None,
            },
        );
        let turn = app
            .session
            .turns
            .iter()
            .find(|t| t.role == TurnRole::Assistant)
            .expect("assistant turn must exist");
        let sources = turn
            .elements
            .iter()
            .find_map(|e| match e {
                TurnElement::Sources(u) => Some(u.clone()),
                _ => None,
            })
            .expect("Sources element must be appended");
        // Two unique URLs, body link first (insertion order).
        assert_eq!(
            sources,
            vec![
                "https://dup.example/x".to_string(),
                "https://uniq.example/y".to_string(),
            ]
        );
    }

    #[test]
    fn sources_capped_at_ten_with_overflow_dropped() {
        // 15 URLs come in via tool result; the Sources element must
        // hold exactly 10. Mirrors `widgets::sources_block::MAX_SOURCES`
        // and the bridge's `SOURCES_MAX_PER_TURN` constant.
        let mut app = App::new();
        apply_event(
            &mut app,
            ProtocolEvent::StreamStart {
                msg_id: "m1".into(),
            },
        );
        let mut results = Vec::new();
        for i in 0..15 {
            results.push(json!({"title": format!("R{i}"), "url": format!("https://r{i}.com")}));
        }
        apply_event(
            &mut app,
            ProtocolEvent::ToolRequest {
                msg_id: "m1".into(),
                call_id: "c1".into(),
                tool: ToolInfo {
                    name: "web".into(),
                    category: ToolCategory::Info,
                    args: json!({}),
                    description: String::new(),
                },
            },
        );
        apply_event(
            &mut app,
            ProtocolEvent::ToolResult {
                msg_id: "m1".into(),
                call_id: "c1".into(),
                tool_name: "web".into(),
                status: ToolStatus::Success,
                output: json!({"results": results}).to_string(),
                output_type: OutputType::Text,
                metadata: None,
            },
        );
        apply_event(
            &mut app,
            ProtocolEvent::TextDelta {
                text: "many".into(),
                msg_id: "m1".into(),
            },
        );
        apply_event(
            &mut app,
            ProtocolEvent::StreamEnd {
                msg_id: "m1".into(),
                finish_reason: wcore_protocol::events::FinishReason::Stop,
                usage: None,
                agent_run_id: None,
            },
        );
        let turn = app
            .session
            .turns
            .iter()
            .find(|t| t.role == TurnRole::Assistant)
            .expect("assistant turn must exist");
        let sources = turn
            .elements
            .iter()
            .find_map(|e| match e {
                TurnElement::Sources(u) => Some(u),
                _ => None,
            })
            .expect("Sources element must be present");
        assert_eq!(sources.len(), 10, "must cap at SOURCES_MAX_PER_TURN");
        // Insertion order — first 10 of the 15.
        assert_eq!(sources[0], "https://r0.com");
        assert_eq!(sources[9], "https://r9.com");
    }

    #[test]
    fn tool_only_turn_with_urls_still_produces_sources_turn() {
        // A "tool-only" stream (no `TextDelta`) used to flush no turn.
        // W3 D4: if a tool result produced URLs, we still push an
        // Assistant turn carrying ONLY a `Sources` element, so the
        // citations are not lost when the model finishes silently
        // after a tool call.
        let mut app = App::new();
        apply_event(
            &mut app,
            ProtocolEvent::StreamStart {
                msg_id: "m1".into(),
            },
        );
        apply_event(
            &mut app,
            ProtocolEvent::ToolRequest {
                msg_id: "m1".into(),
                call_id: "c1".into(),
                tool: ToolInfo {
                    name: "web".into(),
                    category: ToolCategory::Info,
                    args: json!({}),
                    description: String::new(),
                },
            },
        );
        apply_event(
            &mut app,
            ProtocolEvent::ToolResult {
                msg_id: "m1".into(),
                call_id: "c1".into(),
                tool_name: "web".into(),
                status: ToolStatus::Success,
                output: json!({
                    "results": [{"title": "T", "url": "https://only-tool.example"}]
                })
                .to_string(),
                output_type: OutputType::Text,
                metadata: None,
            },
        );
        // No TextDelta — `streaming` is empty at StreamEnd.
        apply_event(
            &mut app,
            ProtocolEvent::StreamEnd {
                msg_id: "m1".into(),
                finish_reason: wcore_protocol::events::FinishReason::Stop,
                usage: None,
                agent_run_id: None,
            },
        );
        let turn = app
            .session
            .turns
            .iter()
            .find(|t| t.role == TurnRole::Assistant)
            .expect("a tool-only turn with URLs must still push an Assistant turn");
        // v0.9.1.2 F12: a `ToolRequest` opens an in-flight assistant turn
        // and parks a `ToolCard` placeholder at the tool's position, so
        // the turn carries `[ToolCard(call_id), Sources(urls)]` instead
        // of just `[Sources(urls)]`. Both elements must be present in
        // that order.
        assert_eq!(turn.elements.len(), 2);
        match &turn.elements[0] {
            TurnElement::ToolCard(id) => assert_eq!(id, "c1"),
            other => panic!("expected first element to be ToolCard, got {other:?}"),
        }
        match &turn.elements[1] {
            TurnElement::Sources(urls) => {
                assert_eq!(urls, &vec!["https://only-tool.example".to_string()]);
            }
            other => panic!("expected Sources as second element, got {other:?}"),
        }
    }

    #[test]
    fn tool_only_turn_without_urls_pushes_no_turn() {
        // A truly silent tool-only turn (no body, no URLs) must NOT
        // produce a phantom Sources turn — the transcript stays clean.
        let mut app = App::new();
        apply_event(
            &mut app,
            ProtocolEvent::StreamStart {
                msg_id: "m1".into(),
            },
        );
        apply_event(
            &mut app,
            ProtocolEvent::ToolRequest {
                msg_id: "m1".into(),
                call_id: "c1".into(),
                tool: ToolInfo {
                    name: "Bash".into(),
                    category: ToolCategory::Exec,
                    args: json!({"command": "echo x"}),
                    description: String::new(),
                },
            },
        );
        apply_event(
            &mut app,
            ProtocolEvent::ToolResult {
                msg_id: "m1".into(),
                call_id: "c1".into(),
                tool_name: "Bash".into(),
                status: ToolStatus::Success,
                output: "x\n".into(),
                output_type: OutputType::Text,
                metadata: None,
            },
        );
        apply_event(
            &mut app,
            ProtocolEvent::StreamEnd {
                msg_id: "m1".into(),
                finish_reason: wcore_protocol::events::FinishReason::Stop,
                usage: None,
                agent_run_id: None,
            },
        );
        // v0.9.1.2 F12: a `ToolRequest` opens an in-flight assistant
        // turn to host the `ToolCard` element so the renderer can place
        // the card inline. The turn carries exactly one element — the
        // `ToolCard(call_id)` placeholder. No phantom Sources element
        // is appended (none were harvested) and no second turn is
        // pushed.
        let assistant_turns: Vec<_> = app
            .session
            .turns
            .iter()
            .filter(|t| t.role == TurnRole::Assistant)
            .collect();
        assert_eq!(
            assistant_turns.len(),
            1,
            "ToolRequest opens exactly one in-flight assistant turn"
        );
        assert_eq!(
            assistant_turns[0].elements.len(),
            1,
            "tool-only-no-urls turn carries only the ToolCard placeholder"
        );
        match &assistant_turns[0].elements[0] {
            TurnElement::ToolCard(id) => assert_eq!(id, "c1"),
            other => panic!("expected ToolCard placeholder, got {other:?}"),
        }
    }

    /// Build a `ToolRequest` event with sane defaults for terse tests.
    fn tool_request(call_id: &str, name: &str, args: serde_json::Value) -> ProtocolEvent {
        let category = match name {
            "Edit" | "Write" => ToolCategory::Edit,
            "Bash" => ToolCategory::Exec,
            _ => ToolCategory::Info,
        };
        ProtocolEvent::ToolRequest {
            msg_id: "m1".into(),
            call_id: call_id.into(),
            tool: ToolInfo {
                name: name.into(),
                category,
                args,
                description: String::new(),
            },
        }
    }

    #[test]
    fn error_message_strips_json_envelope_from_provider_error() {
        // v0.9.1 W2 cycle-2 BLOCKER 3: an Anthropic 401 lands as a JSON
        // envelope embedded in `error.message`. The transcript notice
        // should surface ONLY the inner human-readable line, prefixed
        // with the status code.
        let raw = r#"API error 401: {"type":"error","error":{"type":"authentication_error","message":"invalid x-api-key"},"request_id":"req_abc123"}"#;
        let out = sanitize_provider_error_message(raw);
        assert_eq!(out, "API 401: invalid x-api-key");
        assert!(!out.contains('{'), "JSON braces leaked: {out}");
        assert!(!out.contains("request_id"), "request_id leaked: {out}");
    }

    #[test]
    fn error_message_handles_generic_top_level_message() {
        // OpenAI-style `{"message":"..."}` (no `error` wrapper).
        let raw = r#"{"message":"rate limit exceeded","code":429}"#;
        let out = sanitize_provider_error_message(raw);
        assert_eq!(out, "rate limit exceeded");
    }

    #[test]
    fn error_message_passes_through_a_plain_string() {
        // A non-JSON error message is preserved verbatim (trimmed).
        let raw = "connection reset by peer";
        let out = sanitize_provider_error_message(raw);
        assert_eq!(out, "connection reset by peer");
    }

    #[test]
    fn error_message_truncates_oversized_input() {
        // A 1000-char raw message is clamped to ~500 chars with an
        // ellipsis so the transcript never gets buried by one notice.
        let raw: String = "x".repeat(1000);
        let out = sanitize_provider_error_message(&raw);
        assert!(out.chars().count() <= 500, "not truncated: {}", out.len());
        assert!(out.ends_with('…'), "missing ellipsis: {out}");
    }

    #[test]
    fn error_message_handles_api_error_prefix_without_json() {
        // The provider sometimes emits a plain `API error N: text`
        // without an envelope. We still want the `API N: ` prefix.
        let raw = "API error 500: internal server error";
        let out = sanitize_provider_error_message(raw);
        // Falls through to plain-string truncation, preserving the full
        // original (the JSON parse path is skipped).
        assert!(out.contains("500"), "status code missing: {out}");
        assert!(out.contains("internal server error"), "body missing: {out}");
    }

    #[test]
    fn provider_error_wrapper_prefix_stripped_before_api_error_peel() {
        // v0.9.1 W2 cycle-3 BLOCKER 3 regression guard. The production
        // shape from `AgentError::Provider` Display is
        // `Provider error: API error 401: {JSON}` — cycle-2 missed the
        // outer wrapper so the JSON envelope leaked. After the fix the
        // wrapper is peeled, the API-error prefix is peeled, and the
        // inner `error.message` surfaces alone.
        let raw = r#"Provider error: API error 401: {"type":"error","error":{"type":"authentication_error","message":"invalid x-api-key"},"request_id":"req_abc123"}"#;
        let out = sanitize_provider_error_message(raw);
        assert_eq!(out, "API 401: invalid x-api-key");
        assert!(!out.contains('{'), "JSON braces leaked: {out}");
        assert!(!out.contains('}'), "JSON braces leaked: {out}");
        assert!(!out.contains("request_id"), "request_id leaked: {out}");
        assert!(
            !out.contains("authentication_error"),
            "inner error type leaked: {out}"
        );
        assert!(out.contains("401"), "status code missing: {out}");
        assert!(
            out.contains("invalid x-api-key"),
            "inner message missing: {out}"
        );
    }

    #[test]
    fn network_error_wrapper_prefix_stripped() {
        // A different wrapper word (Network) with no inner API-error
        // prefix or JSON envelope. Just strip the wrapper and surface
        // the suffix.
        let raw = "Network error: timeout after 30s";
        let out = sanitize_provider_error_message(raw);
        assert_eq!(out, "timeout after 30s");
        assert!(!out.contains("Network error"), "wrapper leaked: {out}");
    }

    #[test]
    fn unknown_wrapper_passes_through() {
        // No ` error: ` separator at all: pass through unchanged
        // (modulo trim + truncation, both inert here).
        let raw = "Some weird format message with no colon";
        let out = sanitize_provider_error_message(raw);
        assert_eq!(out, "Some weird format message with no colon");
    }

    // ── v0.9.1.2 F12 — tool cards render inline with text ──────────────

    #[test]
    fn tool_card_renders_inline_with_text_v0912() {
        // Stream `TextDelta("hello"), ToolRequest{write}, TextDelta("world"),
        // StreamEnd` and assert the in-flight assistant turn carries
        // elements `[Markdown("hello"), ToolCard(id), Markdown("world")]`
        // in that exact order. This is the document-order interleave that
        // closes the floating-heading-above-tool-card bug Sean caught on
        // 2026-05-27.
        let mut app = App::new();
        apply_event(
            &mut app,
            ProtocolEvent::StreamStart {
                msg_id: "m1".into(),
            },
        );
        apply_event(
            &mut app,
            ProtocolEvent::TextDelta {
                text: "hello".into(),
                msg_id: "m1".into(),
            },
        );
        apply_event(
            &mut app,
            ProtocolEvent::ToolRequest {
                msg_id: "m1".into(),
                call_id: "c1".into(),
                tool: ToolInfo {
                    name: "Write".into(),
                    category: ToolCategory::Edit,
                    args: json!({"file_path": "/tmp/x", "content": "y"}),
                    description: String::new(),
                },
            },
        );
        apply_event(
            &mut app,
            ProtocolEvent::TextDelta {
                text: "world".into(),
                msg_id: "m1".into(),
            },
        );
        apply_event(
            &mut app,
            ProtocolEvent::StreamEnd {
                msg_id: "m1".into(),
                finish_reason: wcore_protocol::events::FinishReason::Stop,
                usage: None,
                agent_run_id: None,
            },
        );
        assert_eq!(app.session.turns.len(), 1);
        let elements = &app.session.turns[0].elements;
        assert!(
            elements.len() >= 3,
            "expected >=3 elements (Markdown, ToolCard, Markdown), got {elements:?}"
        );
        match &elements[0] {
            TurnElement::Markdown(s) => assert_eq!(s, "hello"),
            other => panic!("first element should be Markdown(\"hello\"), got {other:?}"),
        }
        match &elements[1] {
            TurnElement::ToolCard(id) => assert_eq!(id, "c1"),
            other => panic!("second element should be ToolCard(c1), got {other:?}"),
        }
        match &elements[2] {
            TurnElement::Markdown(s) => assert_eq!(s, "world"),
            other => panic!("third element should be Markdown(\"world\"), got {other:?}"),
        }
        // The in-flight turn pointer is cleared on StreamEnd.
        assert_eq!(app.session.in_flight_turn_idx, None);
    }

    #[test]
    fn tool_cards_clear_on_session_new_v0912() {
        // `SessionView::clear` is the central "start fresh" path. Both
        // `turns` AND `tool_cards` MUST clear together — otherwise a stale
        // `TurnElement::ToolCard(call_id)` from a previous session could
        // resolve to a different card on the next session.
        let mut session = crate::tui::app::SessionView::default();
        // Push 3 cards.
        for i in 1..=3 {
            session.tool_cards.push(ToolCardModel {
                call_id: format!("c{i}"),
                tool_name: "Write".into(),
                summary: String::new(),
                status: ToolCardStatus::Running,
                output: None,
                edit_preview: None,
                input_pretty: String::new(),
                approval_reason: String::new(),
                plan_body: None,
                crucible_plan: None,
            });
        }
        // Push a turn.
        session.turns.push(TurnView {
            role: TurnRole::Assistant,
            elements: vec![TurnElement::Markdown("hi".into())],
        });
        session.streaming.push_str("partial");
        session.in_flight_turn_idx = Some(0);

        session.clear();

        assert!(session.tool_cards.is_empty());
        assert!(session.turns.is_empty());
        assert!(session.streaming.is_empty());
        assert!(session.thinking.is_empty());
        assert_eq!(session.in_flight_turn_idx, None);
        assert!(!session.streaming_active);
    }

    #[test]
    fn tool_card_no_longer_renders_in_trailing_block_v0912() {
        // The legacy "render every card from `session.tool_cards` as a
        // trailing block at the end of `render_turns`" path is gone. The
        // bridge invariant: the only path to render a card is via a
        // matching `TurnElement::ToolCard(id)` on a turn. An unreferenced
        // card in the `tool_cards` vec must NOT cause a panic — it just
        // doesn't render (the renderer's lookup-by-id misses, drops). We
        // assert the invariant at the data layer: `session.turns` is the
        // only source of `ToolCard` placeholders, and `tool_cards` is a
        // lookup table only.
        let mut app = App::new();
        // Stream a turn that references one card.
        apply_event(
            &mut app,
            ProtocolEvent::StreamStart {
                msg_id: "m1".into(),
            },
        );
        apply_event(
            &mut app,
            tool_request("c-real", "Read", json!({"file_path": "/x"})),
        );
        apply_event(
            &mut app,
            ProtocolEvent::StreamEnd {
                msg_id: "m1".into(),
                finish_reason: wcore_protocol::events::FinishReason::Stop,
                usage: None,
                agent_run_id: None,
            },
        );
        // Now inject an EXTRA unreferenced card in `tool_cards` (the
        // legacy "trailing block" would have rendered this; the new
        // architecture must not).
        app.session.tool_cards.push(ToolCardModel {
            call_id: "c-orphan".into(),
            tool_name: "Bash".into(),
            summary: String::new(),
            status: ToolCardStatus::Ok,
            output: None,
            edit_preview: None,
            input_pretty: String::new(),
            approval_reason: String::new(),
            plan_body: None,
            crucible_plan: None,
        });
        // The turn elements must contain ToolCard("c-real") and NOT
        // contain ToolCard("c-orphan") — the orphan card is in the
        // lookup table but unreachable from the document order.
        let referenced_ids: Vec<&String> = app
            .session
            .turns
            .iter()
            .flat_map(|t| {
                t.elements.iter().filter_map(|e| match e {
                    TurnElement::ToolCard(id) => Some(id),
                    _ => None,
                })
            })
            .collect();
        assert!(referenced_ids.iter().any(|id| *id == "c-real"));
        assert!(
            !referenced_ids.iter().any(|id| *id == "c-orphan"),
            "the orphan card must NOT be referenced by any turn element"
        );
    }

    // ── v0.9.1.2 F14 — approval discoverability ─────────────────────────

    #[test]
    fn awaiting_approval_phase_set_on_approval_required_v0912() {
        // ApprovalRequired must transition `session.phase` to
        // `AwaitingApproval{tool, pending_count}` so the status widget
        // can stop saying "Brewing/Calling" — which implies work is
        // happening when actually input is required.
        let mut app = App::new();
        apply_event(
            &mut app,
            tool_request(
                "call-1",
                "Write",
                json!({"file_path": "/tmp/x", "content": ""}),
            ),
        );
        apply_event(
            &mut app,
            ProtocolEvent::ApprovalRequired {
                call_id: "call-1".into(),
                resume_token: "tok".into(),
                correlation_id: String::new(),
                reason: "writes a file".into(),
                context: String::new(),
                plan: None,
            },
        );
        match &app.session.phase {
            StreamingPhase::AwaitingApproval {
                tool,
                pending_count,
            } => {
                assert_eq!(tool, "Write");
                assert_eq!(*pending_count, 1);
            }
            other => panic!("expected AwaitingApproval phase, got {other:?}"),
        }
    }

    #[test]
    fn force_scroll_to_pending_approval_set_v0912() {
        // The protocol bridge sets the one-shot scroll trigger so the
        // next render can pull the awaiting card into view.
        let mut app = App::new();
        assert!(!app.force_scroll_to_pending_approval, "starts unset");
        apply_event(
            &mut app,
            tool_request(
                "call-2",
                "Edit",
                json!({"file_path": "/x", "old_string": "a", "new_string": "b"}),
            ),
        );
        apply_event(
            &mut app,
            ProtocolEvent::ApprovalRequired {
                call_id: "call-2".into(),
                resume_token: "tok".into(),
                correlation_id: String::new(),
                reason: "edits a file".into(),
                context: String::new(),
                plan: None,
            },
        );
        assert!(
            app.force_scroll_to_pending_approval,
            "ApprovalRequired must arm the force-scroll trigger"
        );
    }

    #[test]
    fn egress_approval_synthesizes_a_card_so_the_yan_ui_engages() {
        // B2.5 — an egress consent (`egress:` call_id) has no prior tool card.
        // The bridge must synthesize an AwaitingApproval card so the existing
        // y(once)/a(always)/n(deny) approval UI picks it up.
        let mut app = App::new();
        apply_event(
            &mut app,
            ProtocolEvent::ApprovalRequired {
                call_id: "egress:abc-123".into(),
                resume_token: "egress:abc-123".into(),
                correlation_id: String::new(),
                reason: "Allow network access to `react.dev`? (data-less GET)".into(),
                context: String::new(),
                plan: None,
            },
        );
        let card = app
            .session
            .tool_cards
            .iter()
            .find(|c| c.call_id == "egress:abc-123")
            .expect("egress ApprovalRequired must synthesize a card");
        assert_eq!(card.status, ToolCardStatus::AwaitingApproval);
        assert_eq!(card.tool_name, "egress");
        assert!(card.approval_reason.contains("react.dev"));
        assert!(
            matches!(app.session.phase, StreamingPhase::AwaitingApproval { .. }),
            "the awaiting-approval phase must fire so the user is told input is required"
        );
    }

    #[test]
    fn non_egress_orphan_approval_does_not_synthesize_a_card() {
        // A non-egress approval with no matching card keeps the prior behavior:
        // a system notice, NOT a synthesized card.
        let mut app = App::new();
        let before = app.session.tool_cards.len();
        apply_event(
            &mut app,
            ProtocolEvent::ApprovalRequired {
                call_id: "call-orphan".into(),
                resume_token: "tok".into(),
                correlation_id: String::new(),
                reason: "something".into(),
                context: String::new(),
                plan: None,
            },
        );
        assert_eq!(
            app.session.tool_cards.len(),
            before,
            "a non-egress orphan approval must not synthesize a card"
        );
    }
}
