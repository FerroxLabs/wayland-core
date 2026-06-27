//! Onboarding surface (surface 01) — the first-run connect/configure flow.
//!
//! A five-step flow: **Connect → Validating → AddMore → Name → Ready**.
//!
//! Auth is **API-key only**. There is deliberately NO OAuth path anywhere
//! in this surface — no row, no key, no "sign in with…" affordance (a
//! non-working choice is the worst kind of friction).
//!
//! The three real paths the Connect step offers, in display order:
//!  1. Paste an API key — the provider is detected from the key prefix.
//!     The recognizer checks the **most-specific prefix first** so an
//!     OpenRouter `sk-or-v1-…` key is never mistaken for OpenAI's bare
//!     `sk-`. A genuinely ambiguous bare `sk-` key (OpenAI / DeepSeek /
//!     Moonshot all share it) — or an unrecognized one — opens a provider
//!     picker rather than guessing. The key is then **validated with a
//!     live API call** against *the detected provider's* endpoint; only a
//!     key that provider accepts advances the flow. After a key validates
//!     the user may add another provider or continue, and is finally
//!     asked for a display name.
//!  2. Use Ollama — a local provider, no key, no name prompt. The local
//!     server is probed for reachability before the flow claims a
//!     connection, and the provider selection is persisted so the next
//!     launch does not re-onboard.
//!  3. Skip for now — defer provider setup. The config layer carries a
//!     `[default] read_only` posture for this path (see
//!     `wcore_config::DefaultConfig::read_only`), but the writer wiring that
//!     persists it from onboarding and the engine gate that refuses outbound
//!     calls when it is set are wired separately. Until both land, onboarding
//!     deliberately does NOT promise "no API calls" as an enforced guarantee
//!     here — it is framed as deferred setup only.
//!
//! On entry the surface scans `std::env` for the common provider API-key
//! variables (`ANTHROPIC_API_KEY`, `OPENROUTER_API_KEY`, …). If any are
//! set it announces them on the Connect step and offers to connect them
//! directly — each still runs through the same live validation.
//!
//! Config persistence: completing the API-key flow writes every gathered
//! provider + the display name into the global `config.toml` via
//! [`crate::tui::engine_bridge::write_onboarding_config`]. Completion then
//! switches straight to the Workspace surface — onboarding does **not**
//! emit any slash command (an earlier version emitted `/setup`, which is
//! not a registered command and surfaced as "Unknown command: /setup").
//!
//! Validation runs off the render loop: a `spawn_blocking` task performs
//! the bounded HTTP request and reports back over an `mpsc` channel that
//! [`OnboardingSurface::render`] drains each frame. The UI never blocks.

use std::sync::mpsc::{Receiver, Sender, channel};

use ratatui::Frame;
use ratatui::crossterm::event::{Event as CrosstermEvent, KeyCode, KeyEvent};
use ratatui::layout::{Alignment, Constraint, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, Paragraph};
use tui_input::Input;
use tui_input::backend::crossterm::to_input_request;

use crate::provider_keys::{
    Detected, EnvKey, Provider, ValidationOutcome, detect_provider, scan_env_keys,
    validate_key_blocking,
};
use crate::tui::app::App;
use crate::tui::engine_bridge::OnboardingProvider;
use crate::tui::theme::Theme;
use crate::tui::widgets::wayland_banner;

use super::{Surface, SurfaceAction, SurfaceId};

/// A real placeholder for the empty API-key field — visibly a *prompt*,
/// not a fake-looking pre-filled key.
const KEY_PLACEHOLDER: &str = "paste your provider API key here";

/// Placeholder for the empty display-name field.
const NAME_PLACEHOLDER: &str = "your name";

/// Which connect path the user has chosen / is on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Path {
    /// Paste an API key (provider prefix-detected, then live-validated).
    ApiKey,
    /// Use a local Ollama install (reachability-probed, then persisted).
    Ollama,
    /// Skip provider setup for now (deferred setup; the `read_only` posture
    /// it maps to lives in config — enforcement wired separately).
    Skip,
}

impl Path {
    /// The three connect options, in display order. "Enter an API key"
    /// is first (and the default selection); "Skip" is deliberately last
    /// so the read-only escape hatch never reads as the primary action.
    const ALL: [Path; 3] = [Path::ApiKey, Path::Ollama, Path::Skip];

    /// The option label — framed by what the user *gets*.
    fn label(self) -> &'static str {
        match self {
            Path::ApiKey => "Enter an API key — any major provider",
            Path::Ollama => "Use Ollama — a local model, no API key needed",
            Path::Skip => "Skip for now (set up a provider later from /config)",
        }
    }

    /// The single-key shortcut shown on the right of the option row.
    fn hint(self) -> char {
        match self {
            Path::ApiKey => '\u{23ce}', // ⏎
            Path::Ollama => 'o',
            Path::Skip => 's',
        }
    }
}

/// A message from the spawned validation task back to the surface.
struct ValidationMsg {
    /// The provider the key was validated against.
    provider: Provider,
    /// Whether the provider accepted the key.
    outcome: ValidationOutcome,
}

/// Which of the onboarding steps is showing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Step {
    /// Step 1: pick a path and (for the API-key path) enter the key.
    Connect,
    /// Step 1b: the entered key's prefix was ambiguous or unrecognized —
    /// the user picks the provider from a list before validation.
    PickProvider,
    /// Step 2: a live validation request is in flight.
    Validating,
    /// D003: the Ollama path's reachability probe is in flight. Resolves to
    /// the Ready step once `localhost:11434` answers (or fails to).
    ProbingOllama,
    /// Step 3: a key validated — add another provider, or continue.
    AddMore,
    /// Step 4: "what should I call you?" — a display-name prompt.
    Name,
    /// Step 5: a brief confirmation, then into the workspace.
    Ready,
}

/// The choice on the AddMore step.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum AddMoreChoice {
    /// Loop back to Connect to enter another provider's key.
    AddAnother,
    /// Move on to the name prompt.
    Continue,
}

impl AddMoreChoice {
    const ALL: [AddMoreChoice; 2] = [AddMoreChoice::AddAnother, AddMoreChoice::Continue];

    fn label(self) -> &'static str {
        match self {
            AddMoreChoice::AddAnother => "Add another provider",
            AddMoreChoice::Continue => "Continue",
        }
    }
}

/// The choice the Ready step offers when a config already exists on disk.
///
/// Onboarding never silently clobbers a hand-tuned config. When one is
/// found the Ready step asks the user to decide — keep the existing file
/// or overwrite it with the freshly gathered providers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ReadyChoice {
    /// Overwrite the existing config with the new one.
    Overwrite,
    /// Keep the existing config untouched; proceed without writing.
    Keep,
}

impl ReadyChoice {
    const ALL: [ReadyChoice; 2] = [ReadyChoice::Overwrite, ReadyChoice::Keep];

    fn label(self) -> &'static str {
        match self {
            ReadyChoice::Overwrite => "Overwrite — use the providers I just connected",
            ReadyChoice::Keep => "Keep it — leave my existing config untouched",
        }
    }
}

/// The first-run connect/configure surface.
pub struct OnboardingSurface {
    /// Which step is currently shown.
    step: Step,
    /// Which connect path the cursor is on (Connect step).
    selected: Path,
    /// True once the user has committed to the API-key path and the key
    /// field has focus.
    editing_key: bool,
    /// The API-key text field — `tui-input` state.
    key: Input,
    /// The display-name text field — `tui-input` state (Name step).
    name: Input,
    /// Which AddMore option the cursor is on.
    add_more: AddMoreChoice,
    /// The path that was actually used to complete onboarding.
    completed_via: Option<Path>,
    /// Providers gathered so far — each with its validated key. The
    /// API-key path appends here once a key validates OK.
    providers: Vec<(Provider, String)>,
    /// The most recent validation outcome, shown inline. `None` before
    /// any validation has resolved.
    last_validation: Option<(Provider, ValidationOutcome)>,
    /// Provider API keys discovered in the environment on entry. Empty
    /// when none are set — onboarding then says nothing about them.
    env_keys: Vec<EnvKey>,
    /// Index of the highlighted provider on the PickProvider step.
    pick_cursor: usize,
    /// The provider a validation request is currently in flight against,
    /// so the Validating card can name it. `None` outside Validating.
    validating_provider: Option<Provider>,
    /// The outcome of the config write performed when onboarding
    /// completes. `Some(Ok(_))` carries the written config path,
    /// `Some(Err(_))` the failure message.
    write_result: Option<Result<String, String>>,
    /// When a config already exists on disk at completion, the Ready step
    /// asks the user to decide. `Some((path, cursor))` carries the
    /// existing config's path and the highlighted choice; `None` when no
    /// conflict needs resolving (no config existed, or a non-key path).
    existing_config: Option<(std::path::PathBuf, ReadyChoice)>,
    /// Receiver half of the validation channel — drained each frame in
    /// `render`. `None` until a validation is first spawned.
    validation_rx: Option<Receiver<ValidationMsg>>,
    /// D003 — outcome of the Ollama reachability probe. `None` while the
    /// probe is in flight (or before it runs); `Some(true)` once the local
    /// server answered; `Some(false)` if it did not. The Ready headline
    /// only claims "Connected to Ollama." on `Some(true)`.
    ollama_reachable: Option<bool>,
    /// Receiver half of the Ollama-probe channel — drained each frame in
    /// `render`. `None` until the probe is first spawned.
    ollama_probe_rx: Option<Receiver<bool>>,
    /// Whether `config.toml` already existed at the moment this surface was
    /// constructed.  Captured once and used by `finish_with_config` to decide
    /// whether to show the Overwrite / Keep prompt.  Gating on this snapshot
    /// instead of a live `path.exists()` check avoids a spurious conflict
    /// dialog when `connect_all_env_keys` pre-writes the file mid-flow.
    config_existed_at_start: bool,
}

impl Default for OnboardingSurface {
    fn default() -> Self {
        Self::new()
    }
}

impl OnboardingSurface {
    /// Construct a fresh onboarding surface on the Connect step. Scans
    /// the process environment for already-set provider API keys so the
    /// Connect step can offer to connect them.
    pub fn new() -> Self {
        Self::with_env_keys(scan_env_keys(|var| std::env::var(var).ok()))
    }

    /// Construct an onboarding surface with an explicit set of detected
    /// environment keys — the test seam for the env-detection UI without
    /// mutating the real process environment.
    fn with_env_keys(env_keys: Vec<EnvKey>) -> Self {
        let config_existed_at_start = crate::tui::engine_bridge::onboarding_config_path().exists();
        Self {
            step: Step::Connect,
            selected: Path::ApiKey,
            editing_key: false,
            key: Input::default(),
            name: Input::default(),
            add_more: AddMoreChoice::Continue,
            completed_via: None,
            providers: Vec::new(),
            last_validation: None,
            env_keys,
            pick_cursor: 0,
            validating_provider: None,
            write_result: None,
            existing_config: None,
            validation_rx: None,
            ollama_reachable: None,
            ollama_probe_rx: None,
            config_existed_at_start,
        }
    }

    /// The API key currently in the field, trimmed.
    fn api_key(&self) -> &str {
        self.key.value().trim()
    }

    /// True when at least one detected environment key has not yet been
    /// connected. Used to default the AddMore cursor to "Add another" so a
    /// reflexive Enter never skips an unconnected env key.
    fn has_unconnected_env_keys(&self) -> bool {
        self.env_keys
            .iter()
            .any(|env| !self.providers.iter().any(|(p, _)| *p == env.provider))
    }

    /// The AddMore cursor default after a key validates: "Add another"
    /// while detected env keys remain unconnected, "Continue" otherwise.
    /// Keeps the env-key path and the manual-key path consistent — both
    /// land on AddMore, and both nudge the user toward the unconnected
    /// keys instead of skipping straight to the name prompt.
    fn default_add_more_choice(&self) -> AddMoreChoice {
        if self.has_unconnected_env_keys() {
            AddMoreChoice::AddAnother
        } else {
            AddMoreChoice::Continue
        }
    }

    /// The prefix-recognition result for the current key text.
    fn detection(&self) -> Detected {
        detect_provider(self.key.value())
    }

    /// Move the path cursor by `delta` rows, wrapping within `Path::ALL`.
    fn move_cursor(&mut self, delta: isize) {
        let len = Path::ALL.len() as isize;
        let cur = Path::ALL
            .iter()
            .position(|&p| p == self.selected)
            .unwrap_or(0) as isize;
        let next = ((cur + delta) % len + len) % len;
        self.selected = Path::ALL[next as usize];
    }

    /// Move the AddMore cursor by `delta`, wrapping.
    fn move_add_more(&mut self, delta: isize) {
        let len = AddMoreChoice::ALL.len() as isize;
        let cur = AddMoreChoice::ALL
            .iter()
            .position(|&c| c == self.add_more)
            .unwrap_or(0) as isize;
        let next = ((cur + delta) % len + len) % len;
        self.add_more = AddMoreChoice::ALL[next as usize];
    }

    /// Spawn the live key-validation request for `provider` on a blocking
    /// task and move to the Validating step. The HTTP work runs off the
    /// render loop; the result arrives over `validation_rx`, drained in
    /// `render`. Validation always hits the endpoint for `provider` — the
    /// fix for keys being checked against the wrong API.
    fn start_validation(&mut self, provider: Provider) {
        let key = self.api_key().to_string();
        let (tx, rx) = channel::<ValidationMsg>();
        self.validation_rx = Some(rx);
        self.last_validation = None;
        self.validating_provider = Some(provider);
        self.step = Step::Validating;
        spawn_validation(provider, key, tx);
    }

    /// Resolve the entered key to a provider and either start validation
    /// (a unique prefix) or open the provider picker (ambiguous /
    /// unrecognized). Called when the user submits the key field.
    fn submit_key(&mut self) {
        match self.detection() {
            Detected::One(provider) => {
                self.editing_key = false;
                self.start_validation(provider);
            }
            Detected::Ambiguous | Detected::Unknown => {
                // Never guess — let the user choose the provider.
                self.editing_key = false;
                self.pick_cursor = 0;
                self.step = Step::PickProvider;
            }
        }
    }

    /// Drain any validation result that arrived since the last frame and
    /// fold it into the surface state. Called at the top of `render` so
    /// the UI reflects a resolved validation on the very next frame.
    fn poll_validation(&mut self) {
        let Some(rx) = self.validation_rx.as_ref() else {
            return;
        };
        // `try_recv` is non-blocking — at most one message is ever sent
        // per spawned task, so a single drain per frame is sufficient.
        if let Ok(msg) = rx.try_recv() {
            self.validation_rx = None;
            self.validating_provider = None;
            let resolved = matches!(msg.outcome, ValidationOutcome::Ok);
            self.last_validation = Some((msg.provider, msg.outcome));
            if resolved {
                // Record the freshly validated provider + key, then ask
                // whether to add another or continue. The cursor defaults
                // to "Add another" while detected env keys remain
                // unconnected, so a reflexive Enter never skips them.
                self.providers
                    .push((msg.provider, self.api_key().to_string()));
                self.key = Input::default();
                self.add_more = self.default_add_more_choice();
                self.step = Step::AddMore;
            } else {
                // A rejected key returns to the Connect step with the
                // field re-focused so the user can fix or replace it.
                self.step = Step::Connect;
                self.editing_key = true;
            }
        }
    }

    /// The gathered providers, in `OnboardingProvider` form for the
    /// config writer.
    fn gathered_providers(&self) -> Vec<OnboardingProvider> {
        self.providers
            .iter()
            .map(|(p, key)| OnboardingProvider {
                slug: p.slug().to_string(),
                api_key: key.clone(),
            })
            .collect()
    }

    /// Write the gathered providers + display name to the global config.
    /// `overwrite` replaces an existing file; folds the outcome into
    /// `write_result`.
    fn write_config(&mut self, overwrite: bool) {
        let providers = self.gathered_providers();
        let name = self.name.value().trim().to_string();
        let name_opt = (!name.is_empty()).then_some(name.as_str());
        self.write_result = Some(
            crate::tui::engine_bridge::write_onboarding_config(&providers, name_opt, overwrite)
                .map(|path| path.display().to_string())
                .map_err(|e| e.to_string()),
        );
    }

    /// Complete the API-key flow after the Name step.
    ///
    /// On a true first run (no config on disk when onboarding started) the
    /// config is written straight away.  When a config genuinely pre-existed
    /// before onboarding began, the Ready step shows an explicit Overwrite /
    /// Keep choice instead of silently clobbering it.
    ///
    /// The check uses `config_existed_at_start` (captured at construction)
    /// rather than a live `path.exists()` call so that the pre-write performed
    /// by `connect_all_env_keys` does not trigger a spurious conflict dialog.
    fn finish_with_config(&mut self) -> SurfaceAction {
        self.completed_via = Some(Path::ApiKey);
        if self.config_existed_at_start {
            // A config was already on disk when the user launched — defer the
            // write and let them decide on the Ready step.
            let path = crate::tui::engine_bridge::onboarding_config_path();
            self.existing_config = Some((path, ReadyChoice::Keep));
            self.write_result = None;
        } else {
            self.existing_config = None;
            self.write_config(true); // overwrite=true: we own this file (we wrote the stub)
        }
        self.step = Step::Ready;
        SurfaceAction::None
    }

    /// Complete a non-key path (Ollama / Skip).
    ///
    /// - **Ollama** (D003): records the choice, then starts a reachability
    ///   probe against the local server and advances to `ProbingOllama`.
    ///   The probe outcome (drained in `render`) decides whether Ready
    ///   claims "Connected" and persists the provider selection.
    /// - **Skip**: records the choice and shows Ready immediately. No config
    ///   write here — the `read_only` posture persistence is wired through
    ///   the onboarding config writer separately (see the module doc).
    fn finish_non_key(&mut self, via: Path) -> SurfaceAction {
        self.completed_via = Some(via);
        match via {
            Path::Ollama => {
                self.start_ollama_probe();
            }
            _ => {
                self.step = Step::Ready;
            }
        }
        SurfaceAction::None
    }

    /// Start the D003 Ollama reachability probe and move to the
    /// `ProbingOllama` step. The probe runs off the render loop; its result
    /// arrives over `ollama_probe_rx`, drained in `render`. Without a tokio
    /// runtime (plain unit tests) the spawn is skipped and the probe never
    /// resolves on its own — the test seam drives it instead.
    fn start_ollama_probe(&mut self) {
        let (tx, rx) = channel::<bool>();
        self.ollama_reachable = None;
        self.ollama_probe_rx = Some(rx);
        self.step = Step::ProbingOllama;
        spawn_ollama_probe(tx);
    }

    /// Drain an Ollama-probe result that arrived since the last frame.
    /// Records reachability, persists the Ollama provider selection so the
    /// next launch does not re-onboard (D003: the old path persisted
    /// nothing), then advances to Ready. Called at the top of `render`.
    fn poll_ollama_probe(&mut self) {
        let Some(rx) = self.ollama_probe_rx.as_ref() else {
            return;
        };
        if let Ok(reachable) = rx.try_recv() {
            self.ollama_probe_rx = None;
            self.ollama_reachable = Some(reachable);
            self.persist_ollama_selection();
            self.step = Step::Ready;
        }
    }

    /// Persist the Ollama provider into `config.toml` so the next launch
    /// resolves a configured provider instead of re-running onboarding.
    /// Writes `[default] provider = "ollama"` + `[providers.ollama]` with
    /// the sentinel key the local OpenAI-compat surface expects. An existing
    /// config is never clobbered — the Ready step's Overwrite/Keep choice
    /// owns that decision for the API-key path, and the Ollama path simply
    /// records the write outcome for display.
    fn persist_ollama_selection(&mut self) {
        let ollama = OnboardingProvider {
            slug: "ollama".to_string(),
            api_key: "ollama".to_string(),
        };
        let path = crate::tui::engine_bridge::onboarding_config_path();
        if path.exists() {
            // Do not clobber a hand-tuned config silently.
            self.write_result = None;
        } else {
            self.write_result = Some(
                crate::tui::engine_bridge::write_onboarding_config(&[ollama], None, false)
                    .map(|p| p.display().to_string())
                    .map_err(|e| e.to_string()),
            );
        }
    }

    // ── Connect-step input ────────────────────────────────────────────

    /// Key handling while on the Connect step.
    fn handle_connect_key(&mut self, key: KeyEvent, _app: &mut App) -> SurfaceAction {
        if self.editing_key {
            return self.handle_key_field(key);
        }
        // A digit `1`-`9` connects the matching environment key, if one
        // was detected. Checked before the path shortcuts so it never
        // collides with them.
        if let KeyCode::Char(c) = key.code
            && let Some(idx) = c.to_digit(10)
        {
            let idx = idx as usize;
            if idx >= 1 && idx <= self.env_keys.len() {
                self.connect_env_key(idx - 1);
                return SurfaceAction::None;
            }
        }
        // `a` brings in every detected environment key at once (only
        // offered when 2+ were detected — see the Connect render).
        if key.code == KeyCode::Char('a') && self.env_keys.len() >= 2 {
            return self.connect_all_env_keys();
        }
        match key.code {
            KeyCode::Up | KeyCode::Char('k') => {
                self.move_cursor(-1);
                SurfaceAction::None
            }
            KeyCode::Down | KeyCode::Char('j') => {
                self.move_cursor(1);
                SurfaceAction::None
            }
            KeyCode::Char('o') => {
                self.selected = Path::Ollama;
                self.finish_non_key(Path::Ollama)
            }
            KeyCode::Char('s') => {
                self.selected = Path::Skip;
                self.finish_non_key(Path::Skip)
            }
            KeyCode::Enter => match self.selected {
                Path::ApiKey => {
                    self.editing_key = true;
                    SurfaceAction::None
                }
                Path::Ollama => self.finish_non_key(Path::Ollama),
                Path::Skip => self.finish_non_key(Path::Skip),
            },
            _ => SurfaceAction::None,
        }
    }

    /// Connect the environment key at `idx`.
    ///
    /// Persists the provider choice to `config.toml` immediately (without the
    /// key — the key stays in the environment variable and is read by the
    /// engine on the next boot).  This persistence is the fix for the
    /// "onboarding re-runs on every relaunch" bug: the first-run gate checks
    /// for the existence of `config.toml`, so writing it here ensures the
    /// next launch lands on Workspace instead of re-entering onboarding.
    ///
    /// Env keys skip the live network round-trip a pasted key takes: the
    /// provider was explicitly exported by the user and the engine validates
    /// it on first real use, so onboarding stays **offline-safe** (important
    /// for local/air-gapped setups).  The flow still lands on the **AddMore**
    /// step — never straight to Name/Ready — so a user with several exported
    /// keys can connect the others (mirrors the validated-key path in
    /// `poll_validation`).
    fn connect_env_key(&mut self, idx: usize) {
        let Some(env) = self.env_keys.get(idx).cloned() else {
            return;
        };
        // Persist the provider choice on disk now — before showing AddMore —
        // so a mid-flow quit still leaves the selection.  The key is NOT
        // written; the engine reads it from the environment on next boot.
        crate::tui::engine_bridge::persist_env_provider_selection(env.provider.slug());
        // Record the provider (empty key — it lives in the env var, never on
        // disk) so the AddMore / Name / Ready cards can name it and
        // `has_unconnected_env_keys` sees it as connected.
        if !self.providers.iter().any(|(p, _)| *p == env.provider) {
            self.providers.push((env.provider, String::new()));
        }
        // Default the AddMore cursor to "Add another" while keys remain,
        // "Continue" otherwise — same as the validated-key path.
        self.add_more = self.default_add_more_choice();
        self.step = Step::AddMore;
    }

    /// Connect EVERY detected environment key at once. A convenience
    /// fast-path for the common case where a developer has several provider
    /// keys exported: it gathers all of them (the first detected provider
    /// becomes the default), skipping the one-at-a-time validate/add-another
    /// loop. The keys come from the user's own environment, so this trades
    /// the single-key path's endpoint validation for speed; the per-number
    /// path still validates one key honestly. Already-gathered providers are
    /// not duplicated. Routes to the Name step so the setup can still be
    /// named before the config (with all `[providers.*]` blocks) is written.
    fn connect_all_env_keys(&mut self) -> SurfaceAction {
        for env in self.env_keys.clone() {
            if !self.providers.iter().any(|(p, _)| *p == env.provider) {
                self.providers.push((env.provider, env.value));
            }
        }
        if self.providers.is_empty() {
            return SurfaceAction::None;
        }
        // Persist the first provider's slug now — same rationale as
        // `connect_env_key`: the first-run gate checks for config.toml, so
        // writing it here ensures a mid-flow quit still leaves a valid
        // selection and the next launch lands on Workspace.
        let first_slug = self.providers[0].0.slug();
        crate::tui::engine_bridge::persist_env_provider_selection(first_slug);
        self.step = Step::Name;
        SurfaceAction::None
    }

    /// Key handling while the API-key text field has focus.
    fn handle_key_field(&mut self, key: KeyEvent) -> SurfaceAction {
        match key.code {
            // Submit the key — only a non-empty key advances. A unique
            // prefix goes straight to validation; an ambiguous or
            // unrecognized one opens the provider picker.
            KeyCode::Enter => {
                if !self.api_key().is_empty() {
                    self.submit_key();
                }
                SurfaceAction::None
            }
            // Esc backs out of the field to the path list.
            KeyCode::Esc => {
                self.editing_key = false;
                SurfaceAction::None
            }
            _ => {
                let event = CrosstermEvent::Key(key);
                if let Some(req) = to_input_request(&event) {
                    self.key.handle(req);
                }
                SurfaceAction::None
            }
        }
    }

    // ── PickProvider-step input ───────────────────────────────────────

    /// Key handling on the provider picker — shown when a key's prefix is
    /// ambiguous or unrecognized. Arrow keys move the cursor; Enter
    /// validates the entered key against the chosen provider; Esc returns
    /// to the key field to fix or replace the key.
    fn handle_pick_provider_key(&mut self, key: KeyEvent) -> SurfaceAction {
        let len = Provider::ALL.len();
        match key.code {
            KeyCode::Up | KeyCode::Char('k') => {
                self.pick_cursor = (self.pick_cursor + len - 1) % len;
                SurfaceAction::None
            }
            KeyCode::Down | KeyCode::Char('j') => {
                self.pick_cursor = (self.pick_cursor + 1) % len;
                SurfaceAction::None
            }
            KeyCode::Enter | KeyCode::Char(' ') => {
                let provider = Provider::ALL[self.pick_cursor];
                self.start_validation(provider);
                SurfaceAction::None
            }
            KeyCode::Esc => {
                // Back to the key field to correct the key.
                self.step = Step::Connect;
                self.editing_key = true;
                SurfaceAction::None
            }
            _ => SurfaceAction::None,
        }
    }

    // ── Validating-step input ─────────────────────────────────────────

    /// Key handling while a validation request is in flight. The only
    /// affordance is Esc — cancel the wait and return to the key field.
    /// The spawned task is left to finish; its message is simply ignored
    /// because `validation_rx` is dropped.
    fn handle_validating_key(&mut self, key: KeyEvent) -> SurfaceAction {
        if key.code == KeyCode::Esc {
            self.validation_rx = None;
            self.step = Step::Connect;
            self.editing_key = true;
        }
        SurfaceAction::None
    }

    // ── ProbingOllama-step input ──────────────────────────────────────

    /// Key handling while the Ollama reachability probe is in flight. Esc
    /// cancels back to the Connect path list (the probe task is left to
    /// finish; its message is ignored because `ollama_probe_rx` is dropped).
    fn handle_probing_ollama_key(&mut self, key: KeyEvent) -> SurfaceAction {
        if key.code == KeyCode::Esc {
            self.ollama_probe_rx = None;
            self.ollama_reachable = None;
            self.completed_via = None;
            self.step = Step::Connect;
        }
        SurfaceAction::None
    }

    // ── AddMore-step input ────────────────────────────────────────────

    /// Key handling on the "add another / continue" step.
    fn handle_add_more_key(&mut self, key: KeyEvent) -> SurfaceAction {
        match key.code {
            KeyCode::Up | KeyCode::Char('k') => {
                self.move_add_more(-1);
                SurfaceAction::None
            }
            KeyCode::Down | KeyCode::Char('j') => {
                self.move_add_more(1);
                SurfaceAction::None
            }
            KeyCode::Enter | KeyCode::Char(' ') => match self.add_more {
                AddMoreChoice::AddAnother => {
                    // Loop back to Connect with a fresh key field.
                    self.step = Step::Connect;
                    self.selected = Path::ApiKey;
                    self.editing_key = true;
                    self.key = Input::default();
                    SurfaceAction::None
                }
                AddMoreChoice::Continue => {
                    self.step = Step::Name;
                    SurfaceAction::None
                }
            },
            _ => SurfaceAction::None,
        }
    }

    // ── Name-step input ───────────────────────────────────────────────

    /// Key handling on the display-name prompt. Enter persists config
    /// and completes; a blank name is allowed (the field is optional).
    fn handle_name_key(&mut self, key: KeyEvent) -> SurfaceAction {
        match key.code {
            KeyCode::Enter => self.finish_with_config(),
            KeyCode::Esc => {
                // Back to the AddMore step to change provider choices.
                self.step = Step::AddMore;
                SurfaceAction::None
            }
            _ => {
                let event = CrosstermEvent::Key(key);
                if let Some(req) = to_input_request(&event) {
                    self.name.handle(req);
                }
                SurfaceAction::None
            }
        }
    }

    // ── Ready-step input ──────────────────────────────────────────────

    /// Key handling on the Ready step.
    ///
    /// With no existing-config conflict any confirming key enters the
    /// workspace. When a config already exists the step shows an
    /// Overwrite / Keep choice: arrows move the cursor, Enter applies it
    /// (Overwrite writes the new config, Keep leaves the old one) and
    /// then enters the workspace. Esc steps back to reconnect.
    fn handle_ready_key(&mut self, key: KeyEvent) -> SurfaceAction {
        // ── Existing-config decision ─────────────────────────────────
        if let Some((_, cursor)) = self.existing_config {
            match key.code {
                KeyCode::Up | KeyCode::Char('k') | KeyCode::Down | KeyCode::Char('j') => {
                    // Two choices — any move just toggles.
                    let next = match cursor {
                        ReadyChoice::Overwrite => ReadyChoice::Keep,
                        ReadyChoice::Keep => ReadyChoice::Overwrite,
                    };
                    if let Some(slot) = self.existing_config.as_mut() {
                        slot.1 = next;
                    }
                    return SurfaceAction::None;
                }
                KeyCode::Enter | KeyCode::Char(' ') => {
                    if cursor == ReadyChoice::Overwrite {
                        self.write_config(true);
                    }
                    // Either way the conflict is resolved.
                    self.existing_config = None;
                    return SurfaceAction::Switch(SurfaceId::Workspace);
                }
                KeyCode::Esc => {
                    self.step = Step::Connect;
                    self.completed_via = None;
                    self.existing_config = None;
                    self.editing_key = false;
                    return SurfaceAction::None;
                }
                _ => return SurfaceAction::None,
            }
        }

        // ── No conflict — the plain Ready step ───────────────────────
        match key.code {
            KeyCode::Enter | KeyCode::Char(' ') => SurfaceAction::Switch(SurfaceId::Workspace),
            KeyCode::Esc => {
                // Back to Connect to change the choice. The gathered
                // providers stay — a config write that already happened
                // is not undone here.
                self.step = Step::Connect;
                self.completed_via = None;
                self.editing_key = false;
                SurfaceAction::None
            }
            _ => SurfaceAction::None,
        }
    }

    // ── Rendering ─────────────────────────────────────────────────────

    /// Draw the Connect step.
    fn render_connect(&self, frame: &mut Frame, area: Rect, theme: &Theme) {
        // The environment-key announcement, when present, adds one header
        // row plus one row per detected key. The card grows to fit it.
        let env_rows = if self.env_keys.is_empty() {
            0
        } else {
            (self.env_keys.len() + 2) as u16 // header + keys + trailing gap
        };

        // The card is wide enough (80 cols) to give the full WAYLAND
        // banner room — its widest art row is 72 columns.
        let card = centered(area, 80, 26 + env_rows);

        let block = card_block(theme, " Connect a provider ");
        let inner = block.inner(card);
        frame.render_widget(block, card);

        let [
            brand,
            status,
            env,
            _gap0,
            label,
            field,
            detect,
            _gap1,
            opts,
            _gap2,
            foot,
        ] = Layout::vertical([
            Constraint::Length(9),        // WAYLAND banner hero
            Constraint::Length(1),        // connection-status subtitle
            Constraint::Length(env_rows), // environment-key block
            Constraint::Length(1),        // gap
            Constraint::Length(1),        // field label
            Constraint::Length(3),        // key input
            Constraint::Length(1),        // detect / validation line
            Constraint::Length(1),        // gap
            Constraint::Length(3),        // three option rows
            Constraint::Length(1),        // gap
            Constraint::Length(1),        // step footer
        ])
        .areas(inner);

        // ── Brand ─────────────────────────────────────────────────────
        // The full WAYLAND banner is the onboarding hero. The banner
        // widget paints the wordmark, the tagline, and the `/` hint; the
        // dynamic connection-status line sits just below it.
        wayland_banner(frame, brand, theme);

        let connected = self.providers.len();
        let subtitle = if connected == 0 {
            "connect a provider to begin".to_string()
        } else {
            format!(
                "{connected} provider{} connected · add another or continue",
                if connected == 1 { "" } else { "s" }
            )
        };
        frame.render_widget(
            Paragraph::new(Line::from(Span::styled(
                subtitle,
                Style::default().fg(theme.text_muted),
            )))
            .alignment(Alignment::Center),
            status,
        );

        // ── Environment-key announcement ─────────────────────────────
        // Only drawn when at least one provider API key was found in the
        // environment. Each is numbered so the user can connect it with a
        // single digit press.
        if !self.env_keys.is_empty() {
            let mut env_lines: Vec<Line> = Vec::with_capacity(self.env_keys.len() + 2);
            env_lines.push(Line::from(Span::styled(
                "Detected in your environment — press the number to connect:",
                Style::default()
                    .fg(theme.orange)
                    .add_modifier(Modifier::BOLD),
            )));
            for (i, env) in self.env_keys.iter().enumerate() {
                env_lines.push(Line::from(vec![
                    Span::styled(
                        format!("  {} ", i + 1),
                        Style::default()
                            .fg(theme.orange)
                            .add_modifier(Modifier::BOLD),
                    ),
                    Span::styled(env.var, Style::default().fg(theme.text)),
                    Span::styled(" → ", Style::default().fg(theme.text_muted)),
                    Span::styled(env.provider.label(), Style::default().fg(theme.text_dim)),
                ]));
            }
            // With more than one key detected, offer a one-press shortcut to
            // bring them ALL in at once (the common case for a dev with
            // several provider keys exported) instead of connecting one at a
            // time through the add-another loop.
            if self.env_keys.len() >= 2 {
                env_lines.push(Line::from(vec![
                    Span::styled(
                        "  a ",
                        Style::default()
                            .fg(theme.orange)
                            .add_modifier(Modifier::BOLD),
                    ),
                    Span::styled("add all detected keys", Style::default().fg(theme.text)),
                ]));
            }
            env_lines.push(Line::from(""));
            frame.render_widget(Paragraph::new(env_lines), env);
        }

        // ── Key field label ──────────────────────────────────────────
        let label_text = if self.env_keys.is_empty() {
            "Paste your provider API key — we'll detect which provider."
        } else {
            "Or paste a different API key — we'll detect which provider."
        };
        frame.render_widget(
            Paragraph::new(Line::from(Span::styled(
                label_text,
                Style::default().fg(theme.text_dim),
            ))),
            label,
        );

        // ── Key input field ──────────────────────────────────────────
        self.render_key_field(frame, field, theme);

        // ── Detect / validation line ─────────────────────────────────
        // After a rejected key the validation result is shown so the
        // user sees *why* the field is back in focus.
        if let Some((provider, ValidationOutcome::Failed(reason))) = self.last_validation.as_ref() {
            let line = Line::from(vec![
                Span::styled("✗ ", Style::default().fg(theme.error)),
                Span::styled(provider.label(), Style::default().fg(theme.text)),
                Span::styled(format!(" — {reason}"), Style::default().fg(theme.error)),
            ]);
            frame.render_widget(Paragraph::new(line), detect);
        } else if !self.api_key().is_empty() {
            let detect_line = match self.detection() {
                Detected::One(p) => Line::from(vec![
                    Span::styled("• ", Style::default().fg(theme.orange)),
                    Span::styled(p.label(), Style::default().fg(theme.text)),
                    Span::styled(
                        " detected — press ⏎ to validate.",
                        Style::default().fg(theme.text_muted),
                    ),
                ]),
                Detected::Ambiguous => Line::from(Span::styled(
                    "Ambiguous sk- key — press ⏎ to pick the provider.",
                    Style::default().fg(theme.text_muted),
                )),
                Detected::Unknown => Line::from(Span::styled(
                    "Unrecognized key — press ⏎ to pick the provider.",
                    Style::default().fg(theme.text_muted),
                )),
            };
            frame.render_widget(Paragraph::new(detect_line), detect);
        }

        // ── Option rows ──────────────────────────────────────────────
        let mut opt_lines = Vec::with_capacity(Path::ALL.len());
        for path in Path::ALL {
            let on = path == self.selected;
            let marker = if on { "▸ " } else { "  " };
            let style = if on {
                Style::default().fg(theme.text).add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(theme.text_dim)
            };
            let accent = if on {
                Style::default().fg(theme.orange)
            } else {
                Style::default().fg(theme.text_muted)
            };
            opt_lines.push(Line::from(vec![
                Span::styled(marker, accent),
                Span::styled(path.label(), style),
                Span::styled(
                    format!("   {}", path.hint()),
                    Style::default().fg(theme.text_muted),
                ),
            ]));
        }
        frame.render_widget(Paragraph::new(opt_lines), opts);

        // ── Step footer ──────────────────────────────────────────────
        frame.render_widget(self.step_footer(theme), foot);
    }

    /// Draw the API-key input field — a bordered box that turns orange
    /// when focused.
    fn render_key_field(&self, frame: &mut Frame, area: Rect, theme: &Theme) {
        self.render_input_field(
            frame,
            area,
            theme,
            &self.key,
            KEY_PLACEHOLDER,
            self.editing_key,
        );
    }

    /// Draw a generic single-line input field (key or name).
    fn render_input_field(
        &self,
        frame: &mut Frame,
        area: Rect,
        theme: &Theme,
        input: &Input,
        placeholder: &str,
        focused: bool,
    ) {
        let border = if focused { theme.orange } else { theme.border };
        let block = Block::default()
            .borders(Borders::ALL)
            .border_style(Style::default().fg(border))
            .style(Style::default().bg(theme.bg));
        let text_area = block.inner(area);
        frame.render_widget(block, area);

        let value = input.value();
        let line = if value.is_empty() {
            Line::from(vec![
                Span::styled("› ", Style::default().fg(theme.orange)),
                Span::styled(placeholder, Style::default().fg(theme.text_muted)),
            ])
        } else {
            Line::from(vec![
                Span::styled("› ", Style::default().fg(theme.orange)),
                Span::styled(value, Style::default().fg(theme.text)),
            ])
        };
        frame.render_widget(Paragraph::new(line), text_area);

        if focused {
            let cx = text_area.x + 2 + input.visual_cursor() as u16;
            let cx = cx.min(text_area.x + text_area.width.saturating_sub(1));
            frame.set_cursor_position((cx, text_area.y));
        }
    }

    /// Draw the Validating step — a spinner-free "validating…" card.
    fn render_validating(&self, frame: &mut Frame, area: Rect, theme: &Theme) {
        let card = centered(area, 60, 11);
        let block = card_block(theme, " Validating ");
        let inner = block.inner(card);
        frame.render_widget(block, card);

        let provider_name = self
            .validating_provider
            .map(Provider::label)
            .unwrap_or("provider");
        let lines = vec![
            Line::from(""),
            Line::from(Span::styled(
                "Wayland",
                Style::default()
                    .fg(theme.orange)
                    .add_modifier(Modifier::BOLD),
            )),
            Line::from(""),
            Line::from(vec![
                Span::styled("⟳ ", Style::default().fg(theme.orange)),
                Span::styled(
                    format!("Validating your {provider_name} key…"),
                    Style::default().fg(theme.text).add_modifier(Modifier::BOLD),
                ),
            ]),
            Line::from(Span::styled(
                "Checking the key with a live request.",
                Style::default().fg(theme.text_muted),
            )),
            Line::from(""),
            Line::from(Span::styled(
                "esc to cancel",
                Style::default().fg(theme.text_dim),
            )),
        ];
        frame.render_widget(Paragraph::new(lines).alignment(Alignment::Center), inner);
    }

    /// Draw the ProbingOllama step (D003) — a "checking the local server"
    /// card shown while the reachability probe is in flight.
    fn render_probing_ollama(&self, frame: &mut Frame, area: Rect, theme: &Theme) {
        let card = centered(area, 60, 11);
        let block = card_block(theme, " Ollama ");
        let inner = block.inner(card);
        frame.render_widget(block, card);

        let lines = vec![
            Line::from(""),
            Line::from(Span::styled(
                "Wayland",
                Style::default()
                    .fg(theme.orange)
                    .add_modifier(Modifier::BOLD),
            )),
            Line::from(""),
            Line::from(vec![
                Span::styled("⟳ ", Style::default().fg(theme.orange)),
                Span::styled(
                    "Checking for a local Ollama server…",
                    Style::default().fg(theme.text).add_modifier(Modifier::BOLD),
                ),
            ]),
            Line::from(Span::styled(
                "Probing localhost:11434.",
                Style::default().fg(theme.text_muted),
            )),
            Line::from(""),
            Line::from(Span::styled(
                "esc to cancel",
                Style::default().fg(theme.text_dim),
            )),
        ];
        frame.render_widget(Paragraph::new(lines).alignment(Alignment::Center), inner);
    }

    /// Draw the PickProvider step — a scrollable provider list shown when
    /// the entered key's prefix was ambiguous or unrecognized.
    fn render_pick_provider(&self, frame: &mut Frame, area: Rect, theme: &Theme) {
        let list_rows = Provider::ALL.len() as u16;
        let card = centered(area, 60, list_rows + 8);
        let block = card_block(theme, " Pick a provider ");
        let inner = block.inner(card);
        frame.render_widget(block, card);

        let [head, _gap, list, foot] = Layout::vertical([
            Constraint::Length(2),         // heading
            Constraint::Length(1),         // gap
            Constraint::Length(list_rows), // provider list
            Constraint::Length(1),         // footer
        ])
        .areas(inner);

        let heading = match self.detection() {
            Detected::Ambiguous => "That sk- key could belong to several providers.",
            _ => "We couldn't recognize that key's prefix.",
        };
        frame.render_widget(
            Paragraph::new(vec![
                Line::from(Span::styled(
                    "Which provider issued it?",
                    Style::default().fg(theme.text).add_modifier(Modifier::BOLD),
                )),
                Line::from(Span::styled(heading, Style::default().fg(theme.text_muted))),
            ])
            .alignment(Alignment::Center),
            head,
        );

        let mut rows: Vec<Line> = Vec::with_capacity(Provider::ALL.len());
        for (i, provider) in Provider::ALL.iter().enumerate() {
            let on = i == self.pick_cursor;
            let marker = if on { "▸ " } else { "  " };
            let style = if on {
                Style::default().fg(theme.text).add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(theme.text_dim)
            };
            rows.push(Line::from(vec![
                Span::styled(marker, Style::default().fg(theme.orange)),
                Span::styled(provider.label(), style),
            ]));
        }
        // Scroll the list so `pick_cursor` is always painted: on a terminal
        // too short to show every provider, keep the highlighted row in view
        // instead of letting the marker walk off the bottom edge (the v0.9.6
        // "provider picker can't scroll" fix — previously the cursor moved
        // onto an unpainted row and the list looked frozen).
        let visible = list.height as usize;
        let scroll_y = self.pick_cursor.saturating_sub(visible.saturating_sub(1)) as u16;
        frame.render_widget(
            Paragraph::new(rows)
                .alignment(Alignment::Center)
                .scroll((scroll_y, 0)),
            list,
        );

        frame.render_widget(
            Paragraph::new(Line::from(Span::styled(
                "↑↓ choose · ⏎ validate · esc to edit the key",
                Style::default().fg(theme.text_dim),
            )))
            .alignment(Alignment::Center),
            foot,
        );
    }

    /// Draw the AddMore step — "add another provider, or continue?".
    fn render_add_more(&self, frame: &mut Frame, area: Rect, theme: &Theme) {
        let card = centered(area, 60, 13);
        // D045: these providers had their API keys saved, not reachability
        // probed. The card title and per-provider line must say "saved", not
        // "connected" — claiming a connection nothing tested is the same
        // false-status class fixed on the completion headlines.
        let block = card_block(theme, " Keys saved ");
        let inner = block.inner(card);
        frame.render_widget(block, card);

        let mut lines = vec![
            Line::from(""),
            Line::from(Span::styled(
                "Wayland",
                Style::default()
                    .fg(theme.orange)
                    .add_modifier(Modifier::BOLD),
            )),
            Line::from(""),
        ];
        // A ✓ line per gathered provider.
        for (provider, _) in &self.providers {
            lines.push(Line::from(vec![
                Span::styled("✓ ", Style::default().fg(theme.success)),
                Span::styled(provider.label(), Style::default().fg(theme.text)),
                Span::styled(" - saved", Style::default().fg(theme.text_muted)),
            ]));
        }
        lines.push(Line::from(""));
        for choice in AddMoreChoice::ALL {
            let on = choice == self.add_more;
            let marker = if on { "▸ " } else { "  " };
            let style = if on {
                Style::default().fg(theme.text).add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(theme.text_dim)
            };
            lines.push(Line::from(vec![
                Span::styled(marker, Style::default().fg(theme.orange)),
                Span::styled(choice.label(), style),
            ]));
        }
        frame.render_widget(Paragraph::new(lines).alignment(Alignment::Center), inner);
    }

    /// Draw the Name step — "what should I call you?".
    fn render_name(&self, frame: &mut Frame, area: Rect, theme: &Theme) {
        let card = centered(area, 60, 12);
        let block = card_block(theme, " Almost done ");
        let inner = block.inner(card);
        frame.render_widget(block, card);

        let [brand, _gap0, prompt, field, _gap1, foot] = Layout::vertical([
            Constraint::Length(1), // brand
            Constraint::Length(1), // gap
            Constraint::Length(1), // prompt
            Constraint::Length(3), // name input
            Constraint::Length(1), // gap
            Constraint::Length(1), // footer
        ])
        .areas(inner);

        frame.render_widget(
            Paragraph::new(Line::from(Span::styled(
                "Wayland",
                Style::default()
                    .fg(theme.orange)
                    .add_modifier(Modifier::BOLD),
            )))
            .alignment(Alignment::Center),
            brand,
        );
        frame.render_widget(
            Paragraph::new(Line::from(Span::styled(
                "What should I call you?",
                Style::default().fg(theme.text).add_modifier(Modifier::BOLD),
            )))
            .alignment(Alignment::Center),
            prompt,
        );
        // The name field always has focus on this step.
        self.render_input_field(frame, field, theme, &self.name, NAME_PLACEHOLDER, true);
        frame.render_widget(
            Paragraph::new(Line::from(Span::styled(
                "press ⏎ to finish · esc to go back",
                Style::default().fg(theme.text_dim),
            )))
            .alignment(Alignment::Center),
            foot,
        );
    }

    /// Draw the Ready step — a short confirmation.
    ///
    /// Every line is rendered through a `Paragraph` with `Wrap` so no text
    /// runs past the card border or truncates a path mid-string — the card
    /// grows taller when an existing-config conflict needs the extra rows.
    fn render_ready(&self, frame: &mut Frame, area: Rect, theme: &Theme) {
        // A conflict adds the Overwrite/Keep choice + an explanatory line,
        // so the card is taller in that case.
        let conflict = self.existing_config.is_some();
        // Stage 4c — +1 in BOTH branches to fit the `/crucible` discovery tip
        // line below. The card has a FIXED height; a Line past the inner area
        // renders nothing, which would silently clip the tip.
        let card_h = if conflict { 18 } else { 14 };
        let card = centered(area, 64, card_h);
        let block = card_block(theme, " Ready ");
        let inner = block.inner(card);
        frame.render_widget(block, card);

        let (head, detail) = match self.completed_via {
            Some(Path::ApiKey) => {
                // D045: a key was entered and persisted — never reachability
                // probed. Say "saved", not "connected"; the first real prompt
                // is what verifies the key actually reaches the provider.
                let names: Vec<&str> = self.providers.iter().map(|(p, _)| p.label()).collect();
                let head = match names.len() {
                    0 => "API key saved.".to_string(),
                    1 => format!("API key saved for {}.", names[0]),
                    _ => format!("API keys saved for {}.", names.join(" + ")),
                };
                (head, "You won't need to enter these again.")
            }
            Some(Path::Ollama) => match self.ollama_reachable {
                // Only claim a connection once the local server answered the
                // reachability probe — never assert "Connected" blind (D003).
                Some(true) => (
                    "Connected to Ollama.".to_string(),
                    "Using your local model — no API calls leave this machine.",
                ),
                Some(false) => (
                    "Ollama not reachable.".to_string(),
                    "Saved the choice, but nothing answered at localhost:11434. \
                     Start Ollama, then send a prompt.",
                ),
                // Probe still in flight (or skipped with no runtime in tests).
                None => (
                    "Checking Ollama…".to_string(),
                    "Probing the local server at localhost:11434.",
                ),
            },
            Some(Path::Skip) => (
                // D045: no provider was chosen, nothing was connected. Frame
                // this as deferral, never as a connection.
                "Skip for now.".to_string(),
                "Browse code freely — connect a provider any time from /config.",
            ),
            None => (
                "Almost there.".to_string(),
                "Pick how to connect on the previous step.",
            ),
        };

        let name = self.name.value().trim();
        let greeting = if matches!(self.completed_via, Some(Path::ApiKey)) && !name.is_empty() {
            format!("Welcome, {name}.")
        } else {
            "Wayland".to_string()
        };

        let mut lines = vec![
            Line::from(""),
            Line::from(Span::styled(
                greeting,
                Style::default()
                    .fg(theme.orange)
                    .add_modifier(Modifier::BOLD),
            )),
            Line::from(""),
            Line::from(vec![
                Span::styled("✓ ", Style::default().fg(theme.success)),
                Span::styled(
                    head,
                    Style::default().fg(theme.text).add_modifier(Modifier::BOLD),
                ),
            ]),
            Line::from(Span::styled(detail, Style::default().fg(theme.text_muted))),
        ];

        if let Some((path, cursor)) = self.existing_config.as_ref() {
            // A config already exists — ask the user to decide rather than
            // dumping a raw error. The path is shortened (home → `~`) so
            // it wraps cleanly and never truncates mid-string.
            lines.push(Line::from(""));
            lines.push(Line::from(Span::styled(
                format!("We found an existing config at {}", short_path(path)),
                Style::default().fg(theme.warning),
            )));
            for choice in ReadyChoice::ALL {
                let on = choice == *cursor;
                let marker = if on { "▸ " } else { "  " };
                let style = if on {
                    Style::default().fg(theme.text).add_modifier(Modifier::BOLD)
                } else {
                    Style::default().fg(theme.text_dim)
                };
                lines.push(Line::from(vec![
                    Span::styled(marker, Style::default().fg(theme.orange)),
                    Span::styled(choice.label(), style),
                ]));
            }
        } else if let Some(result) = self.write_result.as_ref() {
            let status = match result {
                Ok(path) => Line::from(Span::styled(
                    format!("Saved to {}", short_path(std::path::Path::new(path))),
                    Style::default().fg(theme.text_dim),
                )),
                Err(msg) => Line::from(Span::styled(
                    format!("Config not saved: {msg}"),
                    Style::default().fg(theme.warning),
                )),
            };
            lines.push(status);
        }

        // Stage 4c — surface the `/crucible` council so users discover it once
        // they reach the workspace. Dim so it reads as a tip, not a step.
        lines.push(Line::from(""));
        lines.push(Line::from(Span::styled(
            "Tip: /crucible <question> convenes a cross-vendor council to cross-check a hard call.",
            Style::default().fg(theme.text_dim),
        )));

        lines.push(Line::from(""));
        let foot = if conflict {
            "↑↓ choose · ⏎ apply · esc to change your choice"
        } else {
            "Press ⏎ to open the workspace · esc to change your choice"
        };
        lines.push(Line::from(Span::styled(
            foot,
            Style::default().fg(theme.text_dim),
        )));
        frame.render_widget(
            Paragraph::new(lines)
                .alignment(Alignment::Center)
                .wrap(ratatui::widgets::Wrap { trim: true }),
            inner,
        );
    }

    /// The progress footer. Mirrors the active step.
    fn step_footer(&self, theme: &Theme) -> Paragraph<'static> {
        let on_connect = matches!(self.step, Step::Connect | Step::Validating);
        let connect_style = if on_connect {
            Style::default().fg(theme.orange)
        } else {
            Style::default().fg(theme.text_muted)
        };
        let ready_style = if on_connect {
            Style::default().fg(theme.text_muted)
        } else {
            Style::default().fg(theme.orange)
        };
        let line = Line::from(vec![
            Span::styled("● Connect", connect_style),
            Span::styled("   ─   ", Style::default().fg(theme.border)),
            Span::styled("○ Ready", ready_style),
        ]);
        Paragraph::new(line).alignment(Alignment::Center)
    }
}

/// Build the bordered card block shared by every onboarding step.
fn card_block(theme: &Theme, title: &'static str) -> Block<'static> {
    Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(theme.border))
        .style(Style::default().bg(theme.surface))
        .title(Span::styled(
            title,
            Style::default()
                .fg(theme.text_muted)
                .add_modifier(Modifier::BOLD),
        ))
}

/// Shorten a filesystem path for display: the user's home directory is
/// collapsed to `~`. The result is wrapped (never truncated) by the
/// caller's `Paragraph`, so this only trims the leading home prefix —
/// it never cuts a path mid-string.
fn short_path(path: &std::path::Path) -> String {
    let full = path.display().to_string();
    if let Some(home) = std::env::var_os("HOME") {
        let home = home.to_string_lossy();
        if !home.is_empty()
            && let Some(rest) = full.strip_prefix(home.as_ref())
        {
            return format!("~{rest}");
        }
    }
    full
}

/// Carve a `w × h` rectangle centered inside `area`, clamped so it never
/// exceeds the available space on small terminals.
fn centered(area: Rect, w: u16, h: u16) -> Rect {
    let w = w.min(area.width);
    let h = h.min(area.height);
    let x = area.x + (area.width.saturating_sub(w)) / 2;
    let y = area.y + (area.height.saturating_sub(h)) / 2;
    Rect::new(x, y, w, h)
}

/// Spawn the live key-validation request on a blocking task.
///
/// The HTTP work uses `reqwest::blocking` on a `tokio::task::spawn_blocking`
/// worker so the async render loop never blocks. The result is sent over
/// `tx`; a send failure (the surface dropped the receiver, e.g. the user
/// cancelled) is ignored.
///
/// If no tokio runtime is present (unit tests that never submit a key),
/// the spawn is skipped — `validation_rx` then simply never resolves,
/// which the test seam [`OnboardingSurface::inject_validation_for_test`]
/// works around.
fn spawn_validation(provider: Provider, key: String, tx: Sender<ValidationMsg>) {
    // `Handle::try_current` is `Err` outside a runtime — guard so the
    // surface is constructible and drivable in plain `#[test]` fns.
    if tokio::runtime::Handle::try_current().is_err() {
        return;
    }
    tokio::task::spawn_blocking(move || {
        let outcome = validate_key_blocking(provider, &key);
        let _ = tx.send(ValidationMsg { provider, outcome });
    });
}

/// The local Ollama server's OpenAI-compat base URL — the address probed
/// for reachability on the Ollama onboarding path. Kept in sync with
/// `wcore_providers::ollama::OLLAMA_DEFAULT_BASE_URL`.
const OLLAMA_PROBE_URL: &str = "http://localhost:11434/v1/models";

/// Spawn the D003 Ollama reachability probe on a blocking task.
///
/// Mirrors [`spawn_validation`]: the bounded HTTP request runs on a
/// `tokio::task::spawn_blocking` worker so the render loop never blocks,
/// and the boolean result is sent over `tx`. Outside a tokio runtime
/// (plain unit tests) the spawn is skipped — the probe then never resolves
/// on its own and the test seam drives the outcome instead.
fn spawn_ollama_probe(tx: Sender<bool>) {
    if tokio::runtime::Handle::try_current().is_err() {
        return;
    }
    tokio::task::spawn_blocking(move || {
        let _ = tx.send(probe_ollama_blocking());
    });
}

/// A lightweight GET against the local Ollama server's model-list endpoint.
/// Any HTTP response (even an error status) proves the server is up and
/// answering; a connection/DNS/timeout failure means it is not reachable.
/// Deliberately read-only — it never spends tokens or mutates state.
///
/// B8-1: routed through the `wcore_egress::EgressClient` chokepoint (via the
/// shared [`crate::provider_keys::egress_get_status`] bridge) so the local
/// probe is subject to the egress policy like every other outbound call;
/// there is no bare `reqwest::blocking::Client` here.
fn probe_ollama_blocking() -> bool {
    use crate::provider_keys::EgressGetStatus;
    let status = crate::provider_keys::egress_get_status(|client| {
        client
            .get(OLLAMA_PROBE_URL)
            .timeout(std::time::Duration::from_secs(2))
    });
    // Any HTTP response — even a 4xx/5xx error status — proves the server is
    // up and answering. Only a transport failure or a policy deny means "not
    // reachable" (the previous `reqwest::send().is_ok()` had the same shape:
    // a status was a success, a transport error was a failure).
    matches!(status, EgressGetStatus::Status(_))
}

impl Surface for OnboardingSurface {
    fn id(&self) -> SurfaceId {
        SurfaceId::Onboarding
    }

    /// FIX-2 — onboarding is a text-entry flow (API keys, base URLs), so `/` is
    /// literal here; the Router must not divert it to the command palette.
    fn consumes_slash(&self, _app: &App) -> bool {
        true
    }

    fn render(&mut self, frame: &mut Frame, area: Rect, _app: &App, theme: &Theme) {
        // Fold in any validation result that arrived since the last
        // frame *before* drawing, so a resolved validation is reflected
        // immediately.
        self.poll_validation();
        self.poll_ollama_probe();

        frame.render_widget(Clear, area);
        frame.render_widget(Block::default().style(Style::default().bg(theme.bg)), area);
        match self.step {
            Step::Connect => self.render_connect(frame, area, theme),
            Step::PickProvider => self.render_pick_provider(frame, area, theme),
            Step::Validating => self.render_validating(frame, area, theme),
            Step::ProbingOllama => self.render_probing_ollama(frame, area, theme),
            Step::AddMore => self.render_add_more(frame, area, theme),
            Step::Name => self.render_name(frame, area, theme),
            Step::Ready => self.render_ready(frame, area, theme),
        }
    }

    fn handle_key(&mut self, key: KeyEvent, app: &mut App) -> SurfaceAction {
        match self.step {
            Step::Connect => self.handle_connect_key(key, app),
            Step::PickProvider => self.handle_pick_provider_key(key),
            Step::Validating => self.handle_validating_key(key),
            Step::ProbingOllama => self.handle_probing_ollama_key(key),
            Step::AddMore => self.handle_add_more_key(key),
            Step::Name => self.handle_name_key(key),
            Step::Ready => self.handle_ready_key(key),
        }
    }

    /// Insert a bracketed-paste blob into whichever onboarding text field
    /// has focus. Without this override the surface inherited the trait's
    /// no-op default, so a pasted API key was silently dropped — the field
    /// could only be *typed* into, which is brutal for 100-char keys (the
    /// `WorkspaceSurface` composer never had this bug because it overrides
    /// `handle_paste`; onboarding just never got the equivalent).
    ///
    /// The key and name fields are single-line, so CR/LF in the paste (e.g.
    /// a trailing newline after a key copied from a docs page) are stripped
    /// rather than inserted literally. The pasted text is appended to the
    /// field's current value; the cursor lands at the end (the `Input::new`
    /// contract, matching the workspace composer's paste path).
    fn handle_paste(&mut self, text: String, _app: &mut App) {
        let cleaned: String = text.replace(['\r', '\n'], "");
        if cleaned.is_empty() {
            return;
        }
        // Route to the focused single-line field. Anywhere without a text
        // field (path list, provider picker, validating, …) a stray paste
        // is a no-op — same as a stray keystroke there.
        let field = match self.step {
            Step::Connect if self.editing_key => &mut self.key,
            Step::Name => &mut self.name,
            _ => return,
        };
        let current = field.value().to_string();
        *field = Input::new(format!("{current}{cleaned}"));
    }
}

#[cfg(test)]
impl OnboardingSurface {
    /// Test seam: synthesize a validation result without a live HTTP
    /// call. Mirrors exactly what `poll_validation` does on a real
    /// message, so a `#[test]` fn can drive the post-validation flow
    /// (AddMore / Name / Ready) deterministically and offline.
    fn inject_validation_for_test(&mut self, provider: Provider, outcome: ValidationOutcome) {
        let resolved = matches!(outcome, ValidationOutcome::Ok);
        self.validation_rx = None;
        self.validating_provider = None;
        self.last_validation = Some((provider, outcome));
        if resolved {
            self.providers.push((provider, self.api_key().to_string()));
            self.key = Input::default();
            self.add_more = self.default_add_more_choice();
            self.step = Step::AddMore;
        } else {
            self.step = Step::Connect;
            self.editing_key = true;
        }
    }

    /// Test seam (D003): resolve the Ollama probe without a live HTTP call.
    /// Mirrors exactly what `poll_ollama_probe` does on a real message —
    /// records reachability and advances to Ready — but skips the config
    /// persistence (which would touch the process-global config path) so a
    /// `#[test]` can assert the Ready headline offline and hermetically.
    fn inject_ollama_probe_for_test(&mut self, reachable: bool) {
        self.ollama_probe_rx = None;
        self.ollama_reachable = Some(reachable);
        self.step = Step::Ready;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;
    use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    fn char(c: char) -> KeyEvent {
        KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE)
    }

    /// Render the surface to a `TestBackend` and return the flattened
    /// buffer text — used for render-snapshot assertions.
    fn render_text(surface: &mut OnboardingSurface, w: u16, h: u16) -> String {
        let app = App::new();
        let theme = Theme::hearth();
        let mut terminal = Terminal::new(TestBackend::new(w, h)).expect("test terminal");
        terminal
            .draw(|f| surface.render(f, f.area(), &app, &theme))
            .expect("render onboarding");
        let buf = terminal.backend().buffer();
        let mut out = String::new();
        for y in 0..h {
            for x in 0..w {
                out.push_str(buf[(x, y)].symbol());
            }
            out.push('\n');
        }
        out
    }

    fn type_str(surface: &mut OnboardingSurface, app: &mut App, s: &str) {
        for c in s.chars() {
            surface.handle_key(char(c), app);
        }
    }

    /// A surface with **no** environment keys — deterministic regardless
    /// of the shell that runs the test suite. Most tests use this so the
    /// Connect-step layout and copy are fixed; the env-detection tests
    /// build their own surface via `with_env_keys`.
    fn fresh() -> OnboardingSurface {
        OnboardingSurface::with_env_keys(Vec::new())
    }

    /// Build an `EnvKey` for env-detection tests.
    fn env_key(var: &'static str, provider: Provider, value: &str) -> EnvKey {
        EnvKey {
            var,
            provider,
            value: value.to_string(),
        }
    }

    /// Drive the surface to the AddMore step with one validated provider
    /// — without a live HTTP call, via the test seam.
    fn connect_one_provider(
        surface: &mut OnboardingSurface,
        app: &mut App,
        key_text: &str,
        provider: Provider,
    ) {
        surface.handle_key(key(KeyCode::Enter), app); // open key field
        type_str(surface, app, key_text);
        surface.editing_key = false;
        surface.inject_validation_for_test(provider, ValidationOutcome::Ok);
    }

    #[test]
    fn id_is_onboarding() {
        assert_eq!(OnboardingSurface::new().id(), SurfaceId::Onboarding);
    }

    #[test]
    fn connect_step_renders_brand_and_three_paths() {
        let mut surface = fresh();
        let text = render_text(&mut surface, 90, 32);
        // The Connect intro is the full WAYLAND banner — its tagline is a
        // reliable fingerprint regardless of whether the wide ASCII art
        // or the degraded wordmark renders for the test geometry.
        assert!(
            text.contains("the autonomous AI agent"),
            "WAYLAND banner missing"
        );
        assert!(text.contains("Connect a provider"), "card title missing");
        assert!(text.contains("Enter an API key"), "api-key path missing");
        assert!(text.contains("Ollama"), "ollama path missing");
        // D004: the Skip path is reframed as deferred setup, not an
        // (unenforced) "read-only / no API calls" promise.
        assert!(text.contains("Skip for now"), "skip path missing");
    }

    #[test]
    fn no_oauth_path_anywhere_in_the_flow() {
        // The hard constraint: NO OAuth — no row, no copy, no shortcut.
        let mut surface = fresh();
        let connect = render_text(&mut surface, 90, 28).to_lowercase();
        assert!(!connect.contains("oauth"), "OAuth leaked into Connect");
        assert!(
            !connect.contains("sign in"),
            "'sign in' leaked into Connect"
        );

        let mut app = App::new();
        surface.handle_key(key(KeyCode::Char('o')), &mut app); // → Ready
        let ready = render_text(&mut surface, 90, 28).to_lowercase();
        assert!(!ready.contains("oauth"), "OAuth leaked into Ready");
        assert!(!ready.contains("sign in"), "'sign in' leaked into Ready");
    }

    #[test]
    fn no_config_toml_jargon_leaks_to_the_user() {
        let mut surface = fresh();
        let mut app = App::new();
        type_str(&mut surface, &mut app, "ignored");
        let connect = render_text(&mut surface, 90, 28).to_lowercase();
        assert!(!connect.contains("config.toml"), "TOML jargon leaked");
        assert!(!connect.contains("init-config"), "init flag leaked");
    }

    #[test]
    fn footer_shows_two_steps_not_three() {
        let mut surface = fresh();
        let text = render_text(&mut surface, 90, 28);
        assert!(text.contains("Connect"), "Connect step missing");
        assert!(text.contains("Ready"), "Ready step missing");
        assert!(
            !text.contains("Configure"),
            "third 'Configure' step present"
        );
    }

    #[test]
    fn empty_key_field_shows_a_real_placeholder_not_a_fake_key() {
        let mut surface = fresh();
        let text = render_text(&mut surface, 90, 28);
        assert!(
            text.contains("paste your provider API key"),
            "placeholder missing"
        );
        assert!(!text.contains("sk-ant"), "fake pre-filled key present");
        assert!(!text.contains("••••"), "fake masked key present");
    }

    #[test]
    fn arrow_keys_cycle_the_path_selection() {
        let mut surface = fresh();
        let mut app = App::new();
        assert_eq!(surface.selected, Path::ApiKey);
        surface.handle_key(key(KeyCode::Down), &mut app);
        assert_eq!(surface.selected, Path::Ollama);
        surface.handle_key(key(KeyCode::Down), &mut app);
        assert_eq!(surface.selected, Path::Skip);
        surface.handle_key(key(KeyCode::Down), &mut app);
        assert_eq!(surface.selected, Path::ApiKey);
        surface.handle_key(key(KeyCode::Up), &mut app);
        assert_eq!(surface.selected, Path::Skip);
    }

    #[test]
    fn enter_on_api_key_opens_the_field_without_advancing() {
        let mut surface = fresh();
        let mut app = App::new();
        let action = surface.handle_key(key(KeyCode::Enter), &mut app);
        assert!(matches!(action, SurfaceAction::None));
        assert!(surface.editing_key, "key field did not gain focus");
        assert_eq!(surface.step, Step::Connect, "advanced before a key");
    }

    #[test]
    fn empty_key_submit_does_not_start_validation() {
        let mut surface = fresh();
        let mut app = App::new();
        surface.handle_key(key(KeyCode::Enter), &mut app); // open field
        let action = surface.handle_key(key(KeyCode::Enter), &mut app); // submit empty
        assert!(matches!(action, SurfaceAction::None));
        assert_eq!(surface.step, Step::Connect, "blank key advanced the flow");
    }

    #[test]
    fn submitting_a_key_moves_to_the_validating_step() {
        // No tokio runtime in a plain `#[test]` — `spawn_validation`
        // skips the spawn, but the step transition still happens.
        let mut surface = fresh();
        let mut app = App::new();
        surface.handle_key(key(KeyCode::Enter), &mut app); // open field
        type_str(&mut surface, &mut app, "sk-ant-api03-abc");
        surface.handle_key(key(KeyCode::Enter), &mut app); // submit
        assert_eq!(surface.step, Step::Validating);
        let text = render_text(&mut surface, 90, 28);
        assert!(text.contains("Validating"), "validating card missing");
    }

    #[test]
    fn paste_fills_the_api_key_field_with_newline_stripped() {
        // The onboarding paste regression: pasting an API key (the only
        // sane way to enter a 100-char secret) was a silent no-op because
        // the surface inherited the trait's default `handle_paste`.
        let mut surface = fresh();
        let mut app = App::new();
        surface.handle_key(key(KeyCode::Enter), &mut app); // focus the key field
        assert!(surface.editing_key, "key field did not gain focus");
        // Clipboard blobs often carry a trailing newline — it must not land
        // in the value.
        surface.handle_paste("sk-ant-api03-pasted-key\n".to_string(), &mut app);
        assert_eq!(
            surface.api_key(),
            "sk-ant-api03-pasted-key",
            "pasted key did not fill the field (or kept the newline)"
        );
    }

    #[test]
    fn paste_appends_to_typed_text_in_the_key_field() {
        // A paste lands at the cursor (end), appended to whatever was
        // already typed — the same contract as the workspace composer.
        let mut surface = fresh();
        let mut app = App::new();
        surface.handle_key(key(KeyCode::Enter), &mut app); // focus the key field
        type_str(&mut surface, &mut app, "sk-ant-");
        surface.handle_paste("api03-rest-of-key".to_string(), &mut app);
        assert_eq!(surface.api_key(), "sk-ant-api03-rest-of-key");
    }

    #[test]
    fn paste_is_a_no_op_when_no_text_field_is_focused() {
        // On the path list (field not yet opened) a stray paste must not
        // mutate anything — same as a stray keystroke there.
        let mut surface = fresh();
        let mut app = App::new();
        assert!(!surface.editing_key, "field should start unfocused");
        surface.handle_paste("sk-ant-stray".to_string(), &mut app);
        assert!(
            surface.api_key().is_empty(),
            "paste leaked into an unfocused field"
        );
    }

    // NOTE: the recognizer / validation-endpoint / env-scan tests moved
    // with their code into `crate::provider_keys` — see the `tests`
    // module there. The tests below cover the onboarding *flow* that
    // consumes the recognizer.

    #[test]
    fn connect_step_announces_environment_keys_when_present() {
        let mut surface = OnboardingSurface::with_env_keys(vec![
            env_key("ANTHROPIC_API_KEY", Provider::Anthropic, "sk-ant-env"),
            env_key("OPENROUTER_API_KEY", Provider::OpenRouter, "sk-or-v1-env"),
        ]);
        let text = render_text(&mut surface, 90, 40);
        assert!(
            text.contains("Detected in your environment"),
            "env announcement missing"
        );
        assert!(text.contains("ANTHROPIC_API_KEY"), "env var name missing");
        assert!(text.contains("OPENROUTER_API_KEY"), "env var name missing");
        // The actual secret value must never be rendered.
        assert!(!text.contains("sk-ant-env"), "env key VALUE leaked");
        assert!(!text.contains("sk-or-v1-env"), "env key VALUE leaked");
    }

    #[test]
    fn connect_step_says_nothing_about_env_when_none_set() {
        let mut surface = fresh();
        let text = render_text(&mut surface, 90, 40);
        assert!(
            !text.contains("Detected in your environment"),
            "env announcement shown with no env keys"
        );
    }

    #[test]
    fn pressing_a_digit_connects_the_matching_env_key() {
        let mut surface = OnboardingSurface::with_env_keys(vec![env_key(
            "ANTHROPIC_API_KEY",
            Provider::Anthropic,
            "sk-ant-env",
        )]);
        let mut app = App::new();
        surface.handle_key(char('1'), &mut app);
        // Env keys connect offline — the provider is recorded and the flow
        // lands on AddMore with no network-validation round-trip. The key
        // VALUE is never loaded into the field (it stays in the env var).
        assert_eq!(surface.step, Step::AddMore);
        assert!(
            surface
                .providers
                .iter()
                .any(|(p, _)| *p == Provider::Anthropic),
            "the matching provider must be recorded"
        );
        assert!(
            surface.key.value().is_empty(),
            "env key VALUE must not be loaded into the field"
        );
    }

    #[test]
    fn connecting_an_env_key_lands_on_add_more_not_name() {
        // Regression: an env-detected key that validates must land on the
        // AddMore step so the user can connect the OTHER env keys — it
        // must never skip straight to the Name prompt.
        let mut surface = OnboardingSurface::with_env_keys(vec![
            env_key("ANTHROPIC_API_KEY", Provider::Anthropic, "sk-ant-env"),
            env_key("OPENAI_API_KEY", Provider::OpenAi, "sk-proj-env"),
        ]);
        let mut app = App::new();
        // Connect env key #1. Env keys connect offline, so there is no
        // intermediate Validating step — it lands on AddMore directly.
        surface.handle_key(char('1'), &mut app);
        assert_eq!(surface.step, Step::AddMore, "env key skipped AddMore");
        assert_ne!(surface.step, Step::Name, "env key jumped to Name");
        // A second key remains unconnected, so the cursor nudges the user
        // toward "Add another" rather than skipping the other key.
        assert_eq!(surface.add_more, AddMoreChoice::AddAnother);
    }

    #[test]
    fn pressing_a_adds_all_detected_keys_and_lands_on_name() {
        // The one-press shortcut: with several keys exported, `a` brings them
        // ALL in (first = default) and goes straight to naming, skipping the
        // one-at-a-time validate/add-another loop.
        let mut surface = OnboardingSurface::with_env_keys(vec![
            env_key("ANTHROPIC_API_KEY", Provider::Anthropic, "sk-ant-env"),
            env_key("OPENAI_API_KEY", Provider::OpenAi, "sk-proj-env"),
            env_key("OPENROUTER_API_KEY", Provider::OpenRouter, "sk-or-v1-env"),
        ]);
        let mut app = App::new();
        surface.handle_key(char('a'), &mut app);
        assert_eq!(
            surface.gathered_providers().len(),
            3,
            "all three detected keys are gathered"
        );
        assert_eq!(surface.step, Step::Name, "add-all routes to the Name step");
    }

    #[test]
    fn pressing_a_is_inert_with_a_single_detected_key() {
        // The shortcut is only offered (render) and only fires at 2+ keys;
        // with one key, `a` must fall through to the normal path handler and
        // never trigger add-all.
        let mut surface = OnboardingSurface::with_env_keys(vec![env_key(
            "ANTHROPIC_API_KEY",
            Provider::Anthropic,
            "sk-ant-env",
        )]);
        let mut app = App::new();
        surface.handle_key(char('a'), &mut app);
        assert_ne!(
            surface.step,
            Step::Name,
            "single-key 'a' must not trigger add-all"
        );
    }

    #[test]
    fn add_more_defaults_to_add_another_while_env_keys_remain() {
        // With a second detected env key still unconnected the AddMore
        // cursor defaults to `AddAnother`, so a reflexive Enter connects
        // the next key instead of skipping it.
        let mut surface = OnboardingSurface::with_env_keys(vec![
            env_key("ANTHROPIC_API_KEY", Provider::Anthropic, "sk-ant-env"),
            env_key("OPENAI_API_KEY", Provider::OpenAi, "sk-proj-env"),
        ]);
        let mut app = App::new();
        surface.handle_key(char('1'), &mut app);
        surface.inject_validation_for_test(Provider::Anthropic, ValidationOutcome::Ok);
        assert_eq!(
            surface.add_more,
            AddMoreChoice::AddAnother,
            "cursor should nudge toward the unconnected env key"
        );
        // The reflexive Enter on AddAnother loops back to Connect — where
        // the second env key is still reachable by its digit.
        surface.handle_key(key(KeyCode::Enter), &mut app);
        assert_eq!(surface.step, Step::Connect);
        // The Connect step still re-focuses the manual key field; back out
        // of it so the digit shortcut for the env key works.
        surface.editing_key = false;
        surface.handle_key(char('2'), &mut app);
        surface.inject_validation_for_test(Provider::OpenAi, ValidationOutcome::Ok);
        // Both env keys connected — nothing left, cursor defaults to
        // Continue.
        assert_eq!(
            surface.add_more,
            AddMoreChoice::Continue,
            "cursor should default to Continue once env keys are exhausted"
        );
    }

    #[test]
    fn an_out_of_range_digit_does_nothing() {
        let mut surface = OnboardingSurface::with_env_keys(vec![env_key(
            "ANTHROPIC_API_KEY",
            Provider::Anthropic,
            "sk-ant-env",
        )]);
        let mut app = App::new();
        surface.handle_key(char('2'), &mut app); // only "1" is valid
        assert_eq!(surface.step, Step::Connect, "stray digit advanced flow");
    }

    #[test]
    fn an_unambiguous_key_skips_the_picker_and_validates() {
        let mut surface = fresh();
        let mut app = App::new();
        surface.handle_key(key(KeyCode::Enter), &mut app); // open field
        type_str(&mut surface, &mut app, "sk-or-v1-abc");
        surface.handle_key(key(KeyCode::Enter), &mut app); // submit
        // OpenRouter is unique — straight to Validating against OpenRouter.
        assert_eq!(surface.step, Step::Validating);
        assert_eq!(surface.validating_provider, Some(Provider::OpenRouter));
    }

    #[test]
    fn an_ambiguous_key_opens_the_provider_picker() {
        let mut surface = fresh();
        let mut app = App::new();
        surface.handle_key(key(KeyCode::Enter), &mut app); // open field
        type_str(&mut surface, &mut app, "sk-plainkey123");
        surface.handle_key(key(KeyCode::Enter), &mut app); // submit
        assert_eq!(
            surface.step,
            Step::PickProvider,
            "ambiguous key not routed to picker"
        );
        let text = render_text(&mut surface, 90, 40);
        assert!(text.contains("Pick a provider"), "picker card missing");
    }

    #[test]
    fn an_unrecognized_key_opens_the_provider_picker() {
        let mut surface = fresh();
        let mut app = App::new();
        surface.handle_key(key(KeyCode::Enter), &mut app);
        type_str(&mut surface, &mut app, "totally-unknown-format");
        surface.handle_key(key(KeyCode::Enter), &mut app);
        assert_eq!(surface.step, Step::PickProvider);
    }

    #[test]
    fn picking_a_provider_starts_validation_against_it() {
        let mut surface = fresh();
        let mut app = App::new();
        surface.handle_key(key(KeyCode::Enter), &mut app);
        type_str(&mut surface, &mut app, "sk-plainkey123");
        surface.handle_key(key(KeyCode::Enter), &mut app); // → PickProvider
        // Move to OpenAI (index 1) and choose it.
        surface.handle_key(key(KeyCode::Down), &mut app);
        surface.handle_key(key(KeyCode::Enter), &mut app);
        assert_eq!(surface.step, Step::Validating);
        assert_eq!(surface.validating_provider, Some(Provider::OpenAi));
    }

    #[test]
    fn esc_on_the_picker_returns_to_the_key_field() {
        let mut surface = fresh();
        let mut app = App::new();
        surface.handle_key(key(KeyCode::Enter), &mut app);
        type_str(&mut surface, &mut app, "sk-plainkey123");
        surface.handle_key(key(KeyCode::Enter), &mut app); // → PickProvider
        surface.handle_key(key(KeyCode::Esc), &mut app);
        assert_eq!(surface.step, Step::Connect);
        assert!(surface.editing_key, "key field not re-focused after esc");
    }

    #[test]
    fn connect_step_lists_api_key_first_and_skip_last() {
        // Bug #3: "Enter an API key" is the first option and the default;
        // "Skip" is last.
        assert_eq!(Path::ALL[0], Path::ApiKey);
        assert_eq!(Path::ALL[Path::ALL.len() - 1], Path::Skip);
        assert_eq!(
            fresh().selected,
            Path::ApiKey,
            "default is not the key path"
        );
        let mut surface = fresh();
        let text = render_text(&mut surface, 90, 30);
        let key_pos = text
            .find("Enter an API key")
            .expect("api-key option missing");
        let skip_pos = text.find("Skip").expect("skip option missing");
        assert!(
            key_pos < skip_pos,
            "Skip rendered before the API-key option"
        );
    }

    #[test]
    fn an_ok_validation_advances_to_add_more_and_records_the_provider() {
        let mut surface = fresh();
        let mut app = App::new();
        connect_one_provider(
            &mut surface,
            &mut app,
            "sk-ant-api03-abc",
            Provider::Anthropic,
        );
        assert_eq!(surface.step, Step::AddMore);
        assert_eq!(surface.providers.len(), 1, "provider not recorded");
        assert_eq!(surface.providers[0].0, Provider::Anthropic);
        assert_eq!(surface.providers[0].1, "sk-ant-api03-abc");
        // The key field is cleared for a possible second provider.
        assert!(surface.api_key().is_empty(), "key field not cleared");
    }

    #[test]
    fn a_rejected_key_returns_to_connect_with_the_field_focused() {
        let mut surface = fresh();
        let mut app = App::new();
        surface.handle_key(key(KeyCode::Enter), &mut app);
        type_str(&mut surface, &mut app, "sk-ant-bad");
        surface.editing_key = false;
        surface.inject_validation_for_test(
            Provider::Anthropic,
            ValidationOutcome::Failed("key rejected (401)".to_string()),
        );
        assert_eq!(surface.step, Step::Connect, "rejected key did not return");
        assert!(surface.editing_key, "key field not re-focused");
        assert!(surface.providers.is_empty(), "rejected key was recorded");
        // The rejection reason is shown inline on the Connect card.
        let text = render_text(&mut surface, 90, 28);
        assert!(text.contains("key rejected (401)"), "rejection not shown");
    }

    #[test]
    fn add_another_loops_back_to_connect_for_a_second_provider() {
        let mut surface = fresh();
        let mut app = App::new();
        connect_one_provider(
            &mut surface,
            &mut app,
            "sk-ant-api03-one",
            Provider::Anthropic,
        );
        // Choose "add another".
        surface.handle_key(key(KeyCode::Up), &mut app); // → AddAnother
        assert_eq!(surface.add_more, AddMoreChoice::AddAnother);
        surface.handle_key(key(KeyCode::Enter), &mut app);
        assert_eq!(surface.step, Step::Connect);
        assert!(surface.editing_key, "second key field not focused");

        // Enter a second provider's key.
        type_str(&mut surface, &mut app, "sk-proj-two");
        surface.editing_key = false;
        surface.inject_validation_for_test(Provider::OpenAi, ValidationOutcome::Ok);
        assert_eq!(surface.providers.len(), 2, "second provider not recorded");
        assert_eq!(surface.providers[1].0, Provider::OpenAi);
    }

    #[test]
    fn continue_moves_to_the_name_prompt() {
        let mut surface = fresh();
        let mut app = App::new();
        connect_one_provider(
            &mut surface,
            &mut app,
            "sk-ant-api03-abc",
            Provider::Anthropic,
        );
        assert_eq!(surface.add_more, AddMoreChoice::Continue);
        surface.handle_key(key(KeyCode::Enter), &mut app);
        assert_eq!(surface.step, Step::Name);
        let text = render_text(&mut surface, 90, 28);
        assert!(
            text.contains("What should I call you"),
            "name prompt missing"
        );
    }

    #[test]
    fn name_step_accepts_text_and_finishing_persists_and_shows_ready() {
        let mut surface = fresh();
        let mut app = App::new();
        connect_one_provider(
            &mut surface,
            &mut app,
            "sk-ant-api03-abc",
            Provider::Anthropic,
        );
        surface.handle_key(key(KeyCode::Enter), &mut app); // Continue → Name
        type_str(&mut surface, &mut app, "Sean");
        assert_eq!(surface.name.value(), "Sean");
        let action = surface.handle_key(key(KeyCode::Enter), &mut app); // finish
        // Finishing handles the config in-process — no command.
        assert!(matches!(action, SurfaceAction::None));
        assert_eq!(surface.step, Step::Ready);
        // On a true first run the write is attempted straight away; when a
        // global config already exists (the test host may have one) the
        // Ready step instead defers to an Overwrite/Keep choice. Exactly
        // one of the two states must hold.
        assert!(
            surface.write_result.is_some() ^ surface.existing_config.is_some(),
            "finish must either write the config or defer to a conflict choice"
        );
        let text = render_text(&mut surface, 90, 28);
        assert!(text.contains("Sean"), "name not echoed on Ready");
    }

    #[test]
    fn completing_onboarding_never_emits_a_setup_command() {
        // The /setup misfire regression guard: no step of the flow may
        // emit `SurfaceAction::Command` (an unregistered `/setup` line
        // surfaced as "Unknown command: /setup").
        let mut surface = fresh();
        let mut app = App::new();
        // API-key path.
        connect_one_provider(
            &mut surface,
            &mut app,
            "sk-ant-api03-abc",
            Provider::Anthropic,
        );
        let a = surface.handle_key(key(KeyCode::Enter), &mut app); // Continue → Name
        assert!(
            !matches!(a, SurfaceAction::Command(_)),
            "Command emitted on Continue"
        );
        let a = surface.handle_key(key(KeyCode::Enter), &mut app); // finish → Ready
        assert!(
            !matches!(a, SurfaceAction::Command(_)),
            "finish emitted Command"
        );
        // Ollama path.
        let mut ollama = fresh();
        let a = ollama.handle_key(key(KeyCode::Char('o')), &mut app);
        assert!(
            !matches!(a, SurfaceAction::Command(_)),
            "Ollama emitted Command"
        );
        // Skip path.
        let mut skip = fresh();
        let a = skip.handle_key(key(KeyCode::Char('s')), &mut app);
        assert!(
            !matches!(a, SurfaceAction::Command(_)),
            "Skip emitted Command"
        );
    }

    #[test]
    fn ready_card_text_wraps_within_the_card_border() {
        // Every Ready-card line must wrap inside the padded card — no line
        // may run edge-to-edge past the border. The card is 64 cols wide;
        // its inner content sits 1 col in from each border, so column 0
        // and the last column of the card must stay blank.
        let mut surface = fresh();
        let mut app = App::new();
        // Drive to Ready via the Skip path (a deterministic, no-write
        // path) and render onto a wide-but-finite terminal.
        surface.handle_key(key(KeyCode::Char('s')), &mut app);
        assert_eq!(surface.step, Step::Ready);
        let w = 90u16;
        let h = 30u16;
        let theme = Theme::hearth();
        let mut terminal = Terminal::new(TestBackend::new(w, h)).expect("test terminal");
        terminal
            .draw(|f| surface.render(f, f.area(), &app, &theme))
            .expect("render onboarding");
        let buf = terminal.backend().buffer();
        // The card is centered and 64 wide — find its border rows/cols by
        // locating the card title. Simpler: assert no rendered glyph row
        // is wider than the 64-col card by checking the card border `│`
        // brackets every text row. The robust invariant: the leftmost and
        // rightmost columns of the whole terminal stay blank (the card
        // never reaches them at 90 cols).
        for y in 0..h {
            assert_eq!(
                buf[(0, y)].symbol(),
                " ",
                "content reached terminal column 0 — card text overflowed"
            );
            assert_eq!(
                buf[(w - 1, y)].symbol(),
                " ",
                "content reached the last terminal column — card text overflowed"
            );
        }
    }

    #[test]
    fn ready_card_shows_the_crucible_discovery_tip() {
        // Stage 4c — the Ready card surfaces the `/crucible` council tip so a new
        // user discovers it. Rendered through the real render path; the assertion
        // would FAIL if `card_h` weren't bumped to fit the extra line (a clipped
        // line renders nothing past the fixed-height card's inner area).
        let mut surface = fresh();
        surface.completed_via = Some(Path::ApiKey);
        surface.step = Step::Ready;
        let text = render_text(&mut surface, 90, 30);
        assert!(
            text.contains("/crucible"),
            "the Ready card must surface the `/crucible` discovery tip:\n{text}"
        );
    }

    #[test]
    fn ready_step_offers_overwrite_keep_when_a_config_exists() {
        // When a config already exists the Ready step must present an
        // explicit Overwrite / Keep choice rather than dumping a raw
        // "Config not saved" error.
        let mut surface = fresh();
        surface.completed_via = Some(Path::ApiKey);
        surface.step = Step::Ready;
        surface.existing_config = Some((
            std::path::PathBuf::from("/Users/someone/.config/wayland-core/config.toml"),
            ReadyChoice::Keep,
        ));
        let text = render_text(&mut surface, 90, 30);
        assert!(
            text.contains("existing config"),
            "Ready card did not announce the existing config:\n{text}"
        );
        assert!(text.contains("Overwrite"), "Overwrite choice missing");
        assert!(text.contains("Keep it"), "Keep choice missing");
        // The raw clobber error must NOT appear — it was the bad UI.
        assert!(
            !text.contains("Config not saved"),
            "raw clobber error leaked into the Ready card"
        );
    }

    #[test]
    fn ready_keep_choice_proceeds_without_writing() {
        // `Keep` (the default cursor) + Enter must not write a config — it
        // just enters the workspace.
        let mut surface = fresh();
        let mut app = App::new();
        surface.completed_via = Some(Path::ApiKey);
        surface.step = Step::Ready;
        surface.existing_config = Some((
            std::path::PathBuf::from("/tmp/wayland-core/config.toml"),
            ReadyChoice::Keep,
        ));
        let action = surface.handle_key(key(KeyCode::Enter), &mut app);
        assert!(matches!(
            action,
            SurfaceAction::Switch(SurfaceId::Workspace)
        ));
        assert!(surface.write_result.is_none(), "Keep wrote a config");
        assert!(surface.existing_config.is_none(), "conflict not resolved");
    }

    #[test]
    fn ready_choice_toggles_between_overwrite_and_keep() {
        // Arrow keys toggle the two-option Ready choice.
        let mut surface = fresh();
        let mut app = App::new();
        surface.completed_via = Some(Path::ApiKey);
        surface.step = Step::Ready;
        surface.existing_config = Some((
            std::path::PathBuf::from("/tmp/wayland-core/config.toml"),
            ReadyChoice::Keep,
        ));
        surface.handle_key(key(KeyCode::Up), &mut app);
        assert_eq!(surface.existing_config.unwrap().1, ReadyChoice::Overwrite);
    }

    #[test]
    fn short_path_collapses_the_home_directory() {
        // The Ready card shortens a path under $HOME to a leading `~` so
        // it wraps cleanly — it never cuts a path mid-string.
        // SAFETY: single-threaded test, env restored immediately after.
        let prev = std::env::var_os("HOME");
        unsafe { std::env::set_var("HOME", "/Users/sean") };
        let shortened = short_path(std::path::Path::new(
            "/Users/sean/.config/wayland-core/config.toml",
        ));
        assert_eq!(shortened, "~/.config/wayland-core/config.toml");
        // A path outside HOME is returned unchanged.
        let outside = short_path(std::path::Path::new("/etc/wayland-core/config.toml"));
        assert_eq!(outside, "/etc/wayland-core/config.toml");
        match prev {
            Some(v) => unsafe { std::env::set_var("HOME", v) },
            None => unsafe { std::env::remove_var("HOME") },
        }
    }

    #[test]
    fn ready_step_enter_switches_to_workspace() {
        let mut surface = fresh();
        let mut app = App::new();
        // Ollama now routes through a reachability probe; resolve it via the
        // test seam (no live HTTP, no runtime) to reach Ready.
        surface.handle_key(key(KeyCode::Char('o')), &mut app);
        assert_eq!(surface.step, Step::ProbingOllama);
        surface.inject_ollama_probe_for_test(true);
        assert_eq!(surface.step, Step::Ready);
        let action = surface.handle_key(key(KeyCode::Enter), &mut app);
        assert!(
            matches!(action, SurfaceAction::Switch(SurfaceId::Workspace)),
            "completing onboarding must switch to Workspace"
        );
    }

    /// D003: picking Ollama probes the local server before claiming a
    /// connection. With no runtime the probe does not auto-resolve, so the
    /// surface parks on ProbingOllama rather than asserting "Connected".
    #[test]
    fn ollama_path_probes_before_claiming_connected() {
        let mut surface = fresh();
        let mut app = App::new();
        let action = surface.handle_key(key(KeyCode::Char('o')), &mut app);
        assert!(matches!(action, SurfaceAction::None));
        assert_eq!(
            surface.step,
            Step::ProbingOllama,
            "Ollama must probe reachability before Ready, not jump straight to a connection claim"
        );
        assert_eq!(surface.completed_via, Some(Path::Ollama));
    }

    /// D003: a reachable probe yields the honest "Connected to Ollama."
    /// headline; an unreachable one must NOT claim a connection.
    #[test]
    fn ollama_ready_headline_tracks_reachability() {
        let mut reachable = fresh();
        let mut app = App::new();
        reachable.handle_key(key(KeyCode::Char('o')), &mut app);
        reachable.inject_ollama_probe_for_test(true);
        let screen = render_text(&mut reachable, 90, 28);
        assert!(
            screen.contains("Connected to Ollama"),
            "a reachable probe must show the connected headline; got:\n{screen}"
        );

        let mut unreachable = fresh();
        unreachable.handle_key(key(KeyCode::Char('o')), &mut app);
        unreachable.inject_ollama_probe_for_test(false);
        let screen = render_text(&mut unreachable, 90, 28);
        assert!(
            !screen.contains("Connected to Ollama"),
            "an unreachable probe must NOT claim a connection; got:\n{screen}"
        );
        assert!(
            screen.contains("not reachable"),
            "an unreachable probe must say so honestly; got:\n{screen}"
        );
    }

    /// D004: the Skip path must not promise enforced "no API calls". It is
    /// reframed as a deferred setup, and the read-only posture lives in
    /// config (the enforcement gate is wired separately).
    #[test]
    fn skip_path_is_framed_as_defer_not_enforced_no_api_calls() {
        let mut surface = fresh();
        let mut app = App::new();
        let action = surface.handle_key(key(KeyCode::Char('s')), &mut app);
        assert!(matches!(action, SurfaceAction::None));
        assert_eq!(surface.step, Step::Ready);
        let ready = render_text(&mut surface, 90, 28);
        assert!(
            ready.contains("Skip for now"),
            "skip should be framed as deferred setup, not a connection; got:\n{ready}"
        );
        assert!(
            !ready.contains("no API calls"),
            "skip must not promise enforced 'no API calls' until the gate lands; got:\n{ready}"
        );
    }

    /// D045: the API-key path persists a key but never probes reachability,
    /// so the completion headline must say the key was *saved*, not that a
    /// connection was established.
    #[test]
    fn api_key_ready_headline_says_saved_not_connected() {
        let mut surface = fresh();
        surface.completed_via = Some(Path::ApiKey);
        surface
            .providers
            .push((Provider::Anthropic, "sk-ant-x".to_string()));
        surface.step = Step::Ready;
        let ready = render_text(&mut surface, 90, 28);
        assert!(
            ready.contains("API key saved"),
            "api-key completion must say the key was saved; got:\n{ready}"
        );
        assert!(
            !ready.contains("Connected"),
            "api-key path probed nothing — it must not claim a connection; got:\n{ready}"
        );
    }

    /// D045: the Skip path chooses no provider and connects nothing, so its
    /// completion headline must never imply a connection.
    #[test]
    fn skip_ready_headline_does_not_claim_connected() {
        let mut surface = fresh();
        surface.completed_via = Some(Path::Skip);
        surface.step = Step::Ready;
        let ready = render_text(&mut surface, 90, 28);
        assert!(
            ready.contains("Skip for now"),
            "skip completion must be framed as deferral; got:\n{ready}"
        );
        assert!(
            !ready.contains("Connected"),
            "skip connected nothing — it must not claim a connection; got:\n{ready}"
        );
    }

    #[test]
    fn esc_on_ready_returns_to_connect() {
        let mut surface = fresh();
        let mut app = App::new();
        surface.handle_key(key(KeyCode::Char('s')), &mut app); // Skip → Ready
        assert_eq!(surface.step, Step::Ready);
        let action = surface.handle_key(key(KeyCode::Esc), &mut app);
        assert!(matches!(action, SurfaceAction::None));
        assert_eq!(surface.step, Step::Connect, "esc did not return to Connect");
        assert!(surface.completed_via.is_none(), "completion not cleared");
    }

    /// D003: Esc while the Ollama probe is in flight cancels back to the
    /// Connect path list and clears the in-progress completion.
    #[test]
    fn esc_while_probing_ollama_returns_to_connect() {
        let mut surface = fresh();
        let mut app = App::new();
        surface.handle_key(key(KeyCode::Char('o')), &mut app);
        assert_eq!(surface.step, Step::ProbingOllama);
        surface.handle_key(key(KeyCode::Esc), &mut app);
        assert_eq!(surface.step, Step::Connect, "esc did not cancel the probe");
        assert!(surface.completed_via.is_none(), "completion not cleared");
    }

    #[test]
    fn esc_in_key_field_backs_out_to_path_list() {
        let mut surface = fresh();
        let mut app = App::new();
        surface.handle_key(key(KeyCode::Enter), &mut app); // open field
        assert!(surface.editing_key);
        surface.handle_key(key(KeyCode::Esc), &mut app);
        assert!(!surface.editing_key, "esc did not leave the key field");
        assert_eq!(surface.step, Step::Connect);
    }

    #[test]
    fn esc_while_validating_returns_to_the_key_field() {
        let mut surface = fresh();
        let mut app = App::new();
        surface.handle_key(key(KeyCode::Enter), &mut app);
        type_str(&mut surface, &mut app, "sk-ant-x");
        surface.handle_key(key(KeyCode::Enter), &mut app); // → Validating
        assert_eq!(surface.step, Step::Validating);
        surface.handle_key(key(KeyCode::Esc), &mut app);
        assert_eq!(surface.step, Step::Connect, "esc did not cancel validation");
        assert!(surface.editing_key, "key field not re-focused after cancel");
    }

    #[test]
    fn esc_on_name_step_returns_to_add_more() {
        let mut surface = fresh();
        let mut app = App::new();
        connect_one_provider(
            &mut surface,
            &mut app,
            "sk-ant-api03-abc",
            Provider::Anthropic,
        );
        surface.handle_key(key(KeyCode::Enter), &mut app); // Continue → Name
        assert_eq!(surface.step, Step::Name);
        surface.handle_key(key(KeyCode::Esc), &mut app);
        assert_eq!(surface.step, Step::AddMore, "esc did not return to AddMore");
    }

    #[test]
    fn backspace_edits_the_key_field() {
        let mut surface = fresh();
        let mut app = App::new();
        surface.handle_key(key(KeyCode::Enter), &mut app); // open field
        type_str(&mut surface, &mut app, "abc");
        assert_eq!(surface.key.value(), "abc");
        surface.handle_key(key(KeyCode::Backspace), &mut app);
        assert_eq!(surface.key.value(), "ab");
    }

    #[test]
    fn renders_without_panicking_on_a_tiny_terminal() {
        let mut surface = fresh();
        let _ = render_text(&mut surface, 20, 6);
        // Every step must clamp, not overflow.
        let mut app = App::new();
        surface.handle_key(key(KeyCode::Enter), &mut app);
        type_str(&mut surface, &mut app, "sk-ant-x");
        surface.handle_key(key(KeyCode::Enter), &mut app); // Validating
        let _ = render_text(&mut surface, 20, 6);
        surface.inject_validation_for_test(Provider::Anthropic, ValidationOutcome::Ok);
        let _ = render_text(&mut surface, 20, 6); // AddMore
        surface.handle_key(key(KeyCode::Enter), &mut app); // Name
        let _ = render_text(&mut surface, 20, 6);
    }

    #[test]
    fn add_more_card_lists_every_saved_provider() {
        let mut surface = fresh();
        let mut app = App::new();
        connect_one_provider(
            &mut surface,
            &mut app,
            "sk-ant-api03-one",
            Provider::Anthropic,
        );
        surface.handle_key(key(KeyCode::Up), &mut app); // AddAnother
        surface.handle_key(key(KeyCode::Enter), &mut app); // → Connect
        type_str(&mut surface, &mut app, "sk-proj-two");
        surface.editing_key = false;
        surface.inject_validation_for_test(Provider::OpenAi, ValidationOutcome::Ok);
        let text = render_text(&mut surface, 90, 28);
        assert!(text.contains("Anthropic"), "first provider not listed");
        assert!(text.contains("OpenAI"), "second provider not listed");
    }

    /// D045: the AddMore gather card lists API keys that were merely saved,
    /// never reachability probed. It must say "saved", not "connected" — the
    /// same false-connection class fixed on the completion headlines.
    #[test]
    fn add_more_card_says_saved_not_connected() {
        let mut surface = fresh();
        let mut app = App::new();
        connect_one_provider(
            &mut surface,
            &mut app,
            "sk-ant-api03-one",
            Provider::Anthropic,
        );
        let text = render_text(&mut surface, 90, 28);
        assert!(
            text.contains("saved"),
            "AddMore card must say the key was saved; got:\n{text}"
        );
        assert!(
            !text.contains("connected") && !text.contains("Connected"),
            "AddMore probed nothing — it must not claim a connection; got:\n{text}"
        );
    }
}
