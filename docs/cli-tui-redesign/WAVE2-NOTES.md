# Wave 2 Integration Notes

Wave 1 delivered all 9 surfaces plus the command registry, palette, and
`@`-reference engine — merged into `feat/cli-tui`. Build green, 263 lib
tests pass, clippy clean. Every surface is presentation-complete and
tested against fixtures; **none is wired into the live engine or the
router yet.** This file collects the concrete wiring points the Wave-1
agents flagged, so Wave 2 (T2.1 engine wiring / T2.2 routing / T2.3
dispatch) has them in one place.

## Router wiring (T2.2)

- `surfaces/mod.rs::make_surface()` still returns `StubSurface` for every
  `SurfaceId`. Rewrite it to construct the real surfaces: `OnboardingSurface`,
  `WorkspaceSurface`, `SubAgentsSurface`, `PaletteSurface`,
  `PlanReviewSurface`, `ConfigSurface`, `PluginsSurface`,
  `DiagnosticsSurface`. Each is a `pub struct` in its `surfaces/<name>.rs`
  with a public constructor.
- Surfaces currently each mirror `StubSurface`'s global keys
  (`q`/`p`/`Tab`/`BackTab`) so the router stays navigable. T2.2 may
  centralize this through the `keybind` layer (`keybind.rs` is implemented
  but not yet consumed).
- `SurfaceId::Plugins` is a primary TAB (in `SurfaceId::TABS`) but
  `PluginsSurface` maps `Esc → CloseOverlay`. Decide: keep it a tab (then
  `Esc` is a near-no-op) or route it as an overlay. `PaletteSurface` is
  correctly an overlay (opened via `OpenOverlay(Palette)`).

## Engine event wiring (T2.1)

- `protocol_bridge::spawn_bridge` + `apply_event` decode all ~31
  `ProtocolEvent`s into `App`. Wire the real `AgentEngine` → in-process
  `mpsc<ProtocolEvent>` → `spawn_bridge`.
- `App` has **no `path_map` field** and no activity-feed field;
  `WorkspaceSurface`'s right rail renders an empty `TreeModel`. Add
  `App.path_map: TreeModel` (additive) and populate it in `apply_event`
  from tool activity — per AUDIT the path map is host-derived from tool
  calls, there is no engine event for it.
- Engine-facing `SurfaceAction`s (`SendMessage`, `Command`, `Approve`,
  `Deny`, `SetMode`) are inert in the router today — route them to the
  engine bridge.

## Per-surface wiring points

- **Onboarding** (`onboarding.rs`): emits `Command("/setup provider=…")`
  on completion; the API key is in private `OnboardingSurface::key` — add
  an accessor for the real config write. `wcore_config::init_config()` is
  the wrong primitive (no provider/key args, fixed side effects) — use or
  add a proper config writer.
- **Config** (`config.rs`): the `.wayland-core.toml` write is deferred —
  `save()` sets a `save_pending` flag and promotes a baseline. Behavioral
  settings (approval / plan-first / stop-after / compaction) are
  surface-local defaults, not on `ConfigView` (which carries only
  provider/model/prompt_caching/memory_enabled). Decide: extend
  `ConfigView` or seed from `Config` directly.
- **Diagnostics** (`diagnostics.rs`): `doctor` only exposes a print-based
  `run()` with private `CheckResult`/`Outcome` types. Add a structured
  `doctor::collect()`. The surface has `doctor_is_wired()` (→ false) and
  `set_doctor`/`set_cost`/`set_memory` setters to populate. `/memory`
  delete emits `Command("/memory delete <id>")`.
- **Plan review** (`plan_review.rs`): `PlanReviewSurface::set_plan()` —
  populate from the `EnterPlanMode` tool payload; the router should
  `Switch(PlanReview)` on that event. Run → `Command("/exit-plan-mode")`,
  Discard → `Switch(Workspace)`, Keep planning → `None`.
- **Plugins** (`plugins.rs`): lists from `plugin::install::list_installed()`
  + `Registry::load_default()`; install/remove emit
  `Command("/plugins install|remove <name>")`. `on_enter` reloads. An
  undocumented `r` manual-reload key exists — keep or drop in Wave 3.
- **Palette / registry** (`palette.rs`, `commands/mod.rs`): 22 built-in
  commands in 6 intent groups; `CommandRegistry::register()` supports
  runtime extension — wire user-invocable skills into it. `dispatch()`
  returns `Run`/`Help`/`DidYouMean`/`Unknown`.
- **@-refs** (`commands/at_refs.rs`): `@file`/`@dir` are fully functional;
  `@symbol`/`@diff`/`@url`/`@session`/`@output` resolve to deferred
  placeholder payloads — wire the repomap index, git tooling, network
  fetch, and session lookup. The composer (in `WorkspaceSurface`) wires
  `@` autocomplete + `/` dispatch in T2.2.

## Contract notes

- `SurfaceAction` now derives `Debug` (added at the Wave-1 gate so surface
  tests can assert on returned actions). Still NOT `Clone` by design.
- `ProtocolEvent` is not `Clone` — fixture-subset tests use
  `into_iter().take(n)`. Add `Clone` to `wcore-protocol`'s `ProtocolEvent`
  if Wave 2 needs it.

## Known polish items (Wave 3)

- `commands/at_refs.rs` is ~1471 lines (~770 production + ~700 inline
  tests) — over the 1000-line guideline. Splitting needs a `mod` line in
  `commands/mod.rs`.
