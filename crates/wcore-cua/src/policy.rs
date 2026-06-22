//! `CuaPolicy` — gating layer between `CuaTool` and the platform backend.
//!
//! Three rule kinds:
//! 1. **Forbidden apps** — outright reject (`Reject`). The agent cannot
//!    drive these apps at all. Use for things like password managers
//!    where any synthesized input is unsafe.
//! 2. **Approval-required apps** — route to `Suspend` so the
//!    orchestration layer can emit an S4 `ApprovalRequired` event and
//!    wait for the host. Mapped onto `CuaError::PolicySuspended` at the
//!    tool layer.
//! 3. **Forbidden key combos** — reject specific keystrokes
//!    regardless of frontmost app. Used to ban hard-to-recover-from
//!    shortcuts (`cmd+q+system`, `ctrl+alt+del`). Applied to BOTH
//!    `CuaOp::Key` and `CuaOp::Type` (an LLM may try to bypass the gate
//!    by submitting a forbidden combo as literal text via `Type`).
//!
//! In addition, every app the agent has not driven before — across
//! sessions — routes to `Suspend` on first contact (first-time-per-app
//! approval). The host turns the `Suspend` into a HITL prompt; once
//! approved the app is recorded in the persistent seen set via
//! [`CuaPolicy::mark_app_seen`] and subsequent ops on the same app
//! skip the prompt.

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use parking_lot::Mutex;
use serde::{Deserialize, Serialize};

use crate::op::CuaOp;

/// Outcome of `CuaPolicy::check_op`. The tool layer translates each
/// variant into a `CuaError` (Reject → `PolicyDenied`, Suspend →
/// `PolicySuspended`, Allow → continue).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CuaPolicyOutcome {
    Allow,
    Reject { reason: String },
    Suspend { reason: String },
}

/// Persistent policy state. Cheap to clone — `seen_apps` is wrapped in
/// `Arc<Mutex<...>>` so the tool can carry a shared instance across
/// per-op dispatches.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CuaPolicy {
    /// Apps that route to `Suspend` on every op (host approves each
    /// time). Bundle id / window class / AumId match.
    #[serde(default)]
    pub require_approval_for_app: Vec<String>,

    /// Apps the agent cannot drive at all.
    #[serde(default)]
    pub forbidden_apps: Vec<String>,

    /// Key combinations that are rejected outright, e.g. `"cmd+q+system"`.
    /// Match is case-insensitive against `CuaOp::Key::keys` AND against
    /// normalized `CuaOp::Type::text` (so the LLM can't bypass the gate
    /// by typing the combo as literal text).
    #[serde(default)]
    pub forbidden_key_combos: Vec<String>,

    /// When `true`, the FIRST op against a new (not-yet-seen) app routes
    /// to `Suspend` so the host can approve the app. Default `true`.
    #[serde(default = "default_true")]
    pub first_time_per_app_approval: bool,

    /// Plugin id that owns this policy (composite key for the persistent
    /// seen-apps store — `(plugin_id, app_id)`). Set by the host when
    /// the policy is loaded from a `CuaToolSpec`. Defaults to a
    /// "wayland-cua" fallback so tests need not always set it explicitly.
    #[serde(default = "default_plugin_id")]
    pub plugin_id: String,

    /// Apps the policy has already seen. Persisted across op dispatches
    /// via the `Arc<Mutex<...>>` wrapper in `state`. Loaded from disk
    /// on first call to [`CuaPolicy::seen_apps_initialize`] (idempotent).
    #[serde(skip)]
    state: PolicyState,
}

#[derive(Debug, Clone, Default)]
struct PolicyState {
    seen_apps: Arc<Mutex<HashSet<String>>>,
    /// Backing path for the persistent seen-apps store. `None` =
    /// in-memory only (test usage). Set by
    /// [`CuaPolicy::with_seen_apps_path`].
    seen_apps_path: Arc<Mutex<Option<PathBuf>>>,
    /// Once-flag for the lazy load from `seen_apps_path`. Without
    /// this, repeated `check_op` calls would re-read the file every
    /// dispatch.
    loaded: Arc<Mutex<bool>>,
}

fn default_true() -> bool {
    true
}

fn default_plugin_id() -> String {
    "wayland-cua".to_string()
}

impl Default for CuaPolicy {
    /// Mirror the serde `#[serde(default = "default_true")]` attribute
    /// — `serde_json::from_str::<CuaPolicy>("{}")` and
    /// `CuaPolicy::default()` must return byte-identical values.
    /// Regression: the prior `#[derive(Default)]` set
    /// `first_time_per_app_approval = false`, mismatching the serde
    /// default and producing a policy that silently skipped the
    /// first-time gate.
    fn default() -> Self {
        Self {
            require_approval_for_app: Vec::new(),
            forbidden_apps: Vec::new(),
            forbidden_key_combos: Vec::new(),
            first_time_per_app_approval: default_true(),
            plugin_id: default_plugin_id(),
            state: PolicyState::default(),
        }
    }
}

impl CuaPolicy {
    /// Construct a permissive policy useful in tests + as a baseline
    /// that needs to opt INTO the first-time-per-app gate explicitly.
    /// Distinct from `Default::default()` which mirrors the serde
    /// "user did not specify" shape (first-time gate ON).
    pub fn permissive() -> Self {
        Self {
            require_approval_for_app: Vec::new(),
            forbidden_apps: Vec::new(),
            forbidden_key_combos: Vec::new(),
            first_time_per_app_approval: false,
            plugin_id: default_plugin_id(),
            state: PolicyState::default(),
        }
    }

    /// Configure the on-disk path for the persistent seen-apps store.
    /// Cross-session persistence — defaults to
    /// `dirs::data_dir()/wayland/cua/seen-apps.json`. Callers can
    /// override (tests, multi-host installations).
    pub fn with_seen_apps_path(self, path: PathBuf) -> Self {
        *self.state.seen_apps_path.lock() = Some(path);
        *self.state.loaded.lock() = false;
        self
    }

    /// Default on-disk path for the seen-apps store, rooted under the
    /// profile home so `WAYLAND_HOME` sandboxes it:
    /// `<profile_home>/cua/seen-apps.json`.
    ///
    /// On first access a one-time, best-effort migration
    /// ([`migrate_legacy_seen_apps`]) copies the pre-isolation
    /// `<data_dir>/wayland/cua/seen-apps.json` here — but ONLY when
    /// `WAYLAND_HOME` is unset. Under an explicit `WAYLAND_HOME` the user
    /// opted into an isolated profile and MUST NOT inherit another
    /// profile's HITL approval grants.
    ///
    /// NOTE (open risk O1): no production code currently calls this — the
    /// host builds `CuaTool` without setting `seen_apps_path`, so the
    /// store is in-memory only today. This function is the correct home
    /// for the path + migration the moment persistence is wired.
    pub fn default_seen_apps_path() -> PathBuf {
        let path = wcore_config::config::profile_home()
            .join("cua")
            .join("seen-apps.json");
        migrate_legacy_seen_apps(&path);
        path
    }

    /// Compose the composite key used in the seen-apps set:
    /// `<plugin_id>::<lowercased-app-id>`. Persisted and matched on
    /// this exact shape so two plugins sharing the same `app_id` are
    /// independently approved.
    fn key(&self, app_id: &str) -> String {
        format!("{}::{}", self.plugin_id, app_id.to_ascii_lowercase())
    }

    /// Lazily load the persistent seen-apps set from disk on first
    /// touch. No-op if the path is unset, the file doesn't exist, or
    /// the load has already run. Returns silently on IO/parse errors
    /// — a corrupt store should NOT block CUA operation; the next
    /// successful `mark_app_seen` will overwrite the file.
    fn ensure_loaded(&self) {
        let mut loaded = self.state.loaded.lock();
        if *loaded {
            return;
        }
        let path_guard = self.state.seen_apps_path.lock();
        let Some(path) = path_guard.as_ref().cloned() else {
            // No path configured → in-memory only.
            *loaded = true;
            return;
        };
        drop(path_guard);
        if let Ok(bytes) = std::fs::read(&path)
            && let Ok(entries) = serde_json::from_slice::<Vec<String>>(&bytes)
        {
            let mut seen = self.state.seen_apps.lock();
            for e in entries {
                seen.insert(e);
            }
        }
        *loaded = true;
    }

    /// Persist the current seen-apps set to disk. Best-effort — IO
    /// errors are logged via `tracing::warn` but don't propagate (the
    /// in-memory set still holds for the current session).
    fn persist(&self) {
        let path_guard = self.state.seen_apps_path.lock();
        let Some(path) = path_guard.as_ref().cloned() else {
            return;
        };
        drop(path_guard);
        let entries: Vec<String> = {
            let seen = self.state.seen_apps.lock();
            seen.iter().cloned().collect()
        };
        if let Some(parent) = path.parent()
            && let Err(e) = std::fs::create_dir_all(parent)
        {
            tracing::warn!(error = %e, path = %path.display(), "cua: failed to create seen-apps dir");
            return;
        }
        match serde_json::to_vec(&entries) {
            Ok(bytes) => {
                if let Err(e) = std::fs::write(&path, &bytes) {
                    tracing::warn!(error = %e, path = %path.display(), "cua: failed to write seen-apps store");
                }
            }
            Err(e) => {
                tracing::warn!(error = %e, "cua: failed to serialize seen-apps");
            }
        }
    }

    /// Mark the given app id as already-approved. Persists to disk if a
    /// path was configured via [`Self::with_seen_apps_path`]. Used by
    /// `CuaTool::dispatch` AFTER a successful op (so the first-time gate
    /// only flips once the host actually approved + the op landed).
    pub fn mark_app_seen(&self, app_id: &str) {
        if app_id.is_empty() {
            return;
        }
        self.ensure_loaded();
        let key = self.key(app_id);
        let inserted = {
            let mut seen = self.state.seen_apps.lock();
            seen.insert(key)
        };
        if inserted {
            self.persist();
        }
    }

    /// Backwards-compat: prior callers used `check_action(op, app_id)`.
    /// Forwarded to [`Self::check_op`] which now covers all op kinds.
    pub fn check_action(&self, op: &CuaOp, app_id: &str) -> CuaPolicyOutcome {
        self.check_op(op, app_id)
    }

    /// True when at least one app-scoped rule is configured (forbidden
    /// apps, per-op approval, or the first-time-per-app gate). Used to
    /// decide whether an empty/unknown frontmost-app id must fail closed
    /// — if the operator configured app-scoped protection, an
    /// unresolved app id is treated as untrusted, not benign.
    fn has_app_scoped_rule(&self) -> bool {
        !self.forbidden_apps.is_empty()
            || !self.require_approval_for_app.is_empty()
            || self.first_time_per_app_approval
    }

    /// Check whether an op against a given frontmost app is allowed.
    /// `app_id` is the platform-neutral frontmost-app identifier (see
    /// `ComputerUseBackend::frontmost_app`). Pass `""` if unknown.
    ///
    /// **Fail-closed on unknown app:** when the frontmost app id is
    /// empty/unknown AND any app-scoped rule is configured, the op
    /// routes to `Suspend` (HITL approval) rather than `Allow`. On
    /// Windows/Wayland `frontmost_app()` has no production probe and on
    /// macOS the `osascript` probe can fail (missing TCC grant, login
    /// window), so an empty id is the common case — treating it as
    /// "no app restrictions apply" would silently disable forbidden-app
    /// / require-approval / first-time-per-app gates. The
    /// app-independent forbidden-key-combo and Type control-char checks
    /// still run first and can hard-`Reject` before the Suspend.
    ///
    /// **Op coverage:** every variant routes through this function.
    /// `CuaOp::Type` is checked against the forbidden-key-combo list
    /// AND against a control-char / ANSI-escape denylist (an LLM
    /// otherwise bypasses the keycombo gate by submitting the combo as
    /// literal text or by smuggling control bytes through the text
    /// payload).
    pub fn check_op(&self, op: &CuaOp, app_id: &str) -> CuaPolicyOutcome {
        let app_lc = app_id.to_ascii_lowercase();

        // 1. Forbidden apps — hard deny.
        if !app_lc.is_empty() {
            for forbidden in &self.forbidden_apps {
                if forbidden.eq_ignore_ascii_case(&app_lc) {
                    return CuaPolicyOutcome::Reject {
                        reason: format!("app {app_id:?} is forbidden by policy"),
                    };
                }
            }
        }

        // 2. Forbidden key combos — hard deny on Key AND Type.
        match op {
            CuaOp::Key { keys, .. } => {
                let normalized = normalize_combo(keys);
                for combo in &self.forbidden_key_combos {
                    if matches_combo(combo, &normalized) {
                        return CuaPolicyOutcome::Reject {
                            reason: format!("key combo {keys:?} is forbidden by policy"),
                        };
                    }
                }
            }
            CuaOp::Type { text } => {
                // 2a. Control characters: null bytes, ANSI escapes, and
                // every C0 control char EXCEPT newline (\n = 0x0A) and
                // tab (\t = 0x09). An LLM submitting `\x1b[2J` to clear
                // a terminal screen, `\0` to truncate strings in C-style
                // consumers, or `\x07` (BEL) to spam a terminal is
                // refused outright.
                for c in text.chars() {
                    if (c.is_control() && c != '\n' && c != '\t') || c == '\u{0}' {
                        return CuaPolicyOutcome::Reject {
                            reason: format!(
                                "Type payload contains forbidden control character U+{:04X}",
                                c as u32
                            ),
                        };
                    }
                }
                // 2b. Forbidden key-combo names embedded as literal text
                // (Cmd+Q, ⌘Q, command-q, ^Q, etc.).
                let normalized = normalize_combo(text);
                for combo in &self.forbidden_key_combos {
                    if matches_combo(combo, &normalized) {
                        return CuaPolicyOutcome::Reject {
                            reason: format!("Type text matches forbidden key combo {combo:?}"),
                        };
                    }
                }
            }
            _ => {}
        }

        // 2c. Fail closed on unknown app. The key-combo / Type
        // control-char checks above are app-independent and have already
        // had their chance to hard-Reject. If we reach here with an
        // empty/unknown frontmost-app id while any app-scoped rule is
        // configured, we cannot prove the op targets a permitted app, so
        // route to Suspend (require approval). This closes the H-6/M-17
        // hole where a missing frontmost probe (Windows/Wayland have no
        // production probe; macOS osascript can fail) yielded an empty id
        // that skipped all app-scoped gates and silently Allowed.
        if app_lc.is_empty() && self.has_app_scoped_rule() {
            return CuaPolicyOutcome::Suspend {
                reason: "frontmost app could not be resolved; HITL approval required \
                         because app-scoped policy rules are configured"
                    .to_string(),
            };
        }

        // 3. Require-approval apps — route to Suspend every time.
        if !app_lc.is_empty() {
            for need_approval in &self.require_approval_for_app {
                if need_approval.eq_ignore_ascii_case(&app_lc) {
                    return CuaPolicyOutcome::Suspend {
                        reason: format!("app {app_id:?} requires HITL approval per op"),
                    };
                }
            }
        }

        // 4. First-time-per-app — route to Suspend on first encounter.
        if self.first_time_per_app_approval && !app_lc.is_empty() {
            self.ensure_loaded();
            let key = self.key(app_id);
            let seen = self.state.seen_apps.lock();
            if !seen.contains(&key) {
                // Don't mark it seen yet — the host's approval response
                // is what flips that. Otherwise a Suspend that never
                // returns approval would silently auto-approve next op.
                drop(seen);
                return CuaPolicyOutcome::Suspend {
                    reason: format!("first-time approval needed for app {app_id:?}"),
                };
            }
        }

        CuaPolicyOutcome::Allow
    }
}

/// Normalize a key-combo string (whether sourced from `CuaOp::Key.keys`
/// or `CuaOp::Type.text`) into a canonical lowercase form so the
/// forbidden-combo set matches across keyboard-shortcut spellings:
/// `Cmd+Q`, `command-q`, `^Q`, `⌘Q`, `Q\u{2318}` all normalize to
/// the same `cmd+q`. Idempotent.
fn normalize_combo(s: &str) -> String {
    // Pass 1: per-char expansion. Glyph modifiers get padded with `+`
    // separators on both sides so `⌘Q` → `+cmd+q`, `Q⌘` → `q+cmd+`.
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            // Common keyboard-shortcut unicode glyphs.
            '\u{2318}' => out.push_str("+cmd+"),   // ⌘ Command
            '\u{2325}' => out.push_str("+alt+"),   // ⌥ Option/Alt
            '\u{21E7}' => out.push_str("+shift+"), // ⇧ Shift
            '\u{2303}' => out.push_str("+ctrl+"),  // ⌃ Control
            '^' => out.push_str("+ctrl+"),
            // Separator normalization → `+`.
            ' ' | '\t' | '-' | '_' => out.push('+'),
            // ASCII-lowercase otherwise; keep multi-byte chars verbatim.
            c if c.is_ascii() => out.push(c.to_ascii_lowercase()),
            c => out.extend(c.to_lowercase()),
        }
    }
    // Expand the long-form modifier words so `command-q` → `cmd+q`.
    let out = out
        .replace("command", "cmd")
        .replace("option", "alt")
        .replace("windows", "win");
    // Collapse runs of `+` and trim leading/trailing.
    let mut collapsed = String::with_capacity(out.len());
    let mut prev_plus = true;
    for c in out.chars() {
        if c == '+' {
            if !prev_plus {
                collapsed.push('+');
            }
            prev_plus = true;
        } else {
            collapsed.push(c);
            prev_plus = false;
        }
    }
    collapsed.trim_matches('+').to_string()
}

/// Match a forbidden-combo entry against a normalized `Type` or `Key`
/// payload. Substring-aware so `Type("press cmd+q to quit")` triggers
/// when `cmd+q` is forbidden — defense-in-depth against literal-text
/// bypasses.
fn matches_combo(forbidden: &str, normalized: &str) -> bool {
    let forbidden_n = normalize_combo(forbidden);
    if forbidden_n.is_empty() {
        return false;
    }
    if normalized == forbidden_n {
        return true;
    }
    // Substring match on token boundary so `cmd+q` matches inside
    // a longer Type payload without false-positive on `acmd+q`.
    let boundaries = ['+', ' ', '\t', '\n'];
    if let Some(idx) = normalized.find(&forbidden_n) {
        let before_ok = idx == 0
            || normalized[..idx]
                .chars()
                .last()
                .is_some_and(|c| boundaries.contains(&c));
        let end = idx + forbidden_n.len();
        let after_ok = end == normalized.len()
            || normalized[end..]
                .chars()
                .next()
                .is_some_and(|c| boundaries.contains(&c));
        before_ok && after_ok
    } else {
        false
    }
}

/// One-time best-effort migration of the pre-isolation CUA seen-apps store
/// into the `WAYLAND_HOME`-rooted location.
///
/// Gated, idempotent, atomic:
///   * **Gate:** if `WAYLAND_HOME` is set, return immediately — an isolated
///     profile must NOT inherit shared legacy approval grants;
///   * if `new_path` already exists → no-op;
///   * if the legacy `<data_dir>/wayland/cua/seen-apps.json` is absent or
///     resolves to `new_path` → no-op;
///   * otherwise copy legacy → a temp sibling, then atomic-`rename` into
///     place so a concurrent reader never observes a torn file.
///
/// Every failure logs at `warn` and returns — a missing grant just
/// re-prompts the user; it must never crash the engine.
fn migrate_legacy_seen_apps(new_path: &Path) {
    // Explicit-isolation profiles never inherit shared legacy state.
    if std::env::var_os("WAYLAND_HOME").is_some() {
        return;
    }
    if new_path.exists() {
        return;
    }
    let Some(legacy) =
        dirs::data_dir().map(|d| d.join("wayland").join("cua").join("seen-apps.json"))
    else {
        return;
    };
    if legacy == new_path || !legacy.exists() {
        return;
    }
    let Some(parent) = new_path.parent() else {
        return;
    };
    if let Err(e) = std::fs::create_dir_all(parent) {
        tracing::warn!(error = %e, path = %new_path.display(),
            "cua: failed to create dir for seen-apps migration");
        return;
    }
    // Re-check after dir creation: a concurrent migrator may have won.
    if new_path.exists() {
        return;
    }
    // Atomic publish: copy to a temp sibling, then rename onto new_path so a
    // reader sees either absent-or-complete, never a half-written file.
    let tmp = parent.join(".seen-apps.json.migrating");
    if let Err(e) = std::fs::copy(&legacy, &tmp) {
        tracing::warn!(error = %e, from = %legacy.display(), to = %tmp.display(),
            "cua: failed to stage legacy seen-apps migration");
        let _ = std::fs::remove_file(&tmp);
        return;
    }
    if let Err(e) = std::fs::rename(&tmp, new_path) {
        tracing::warn!(error = %e, to = %new_path.display(),
            "cua: failed to publish migrated seen-apps store");
        let _ = std::fs::remove_file(&tmp);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::backend::{KeyMods, MouseButton};

    #[test]
    #[serial_test::serial]
    fn default_seen_apps_path_roots_under_wayland_home() {
        let tmp = tempfile::tempdir().unwrap();
        let prev = std::env::var_os("WAYLAND_HOME");
        unsafe { std::env::set_var("WAYLAND_HOME", tmp.path()) };
        let p = CuaPolicy::default_seen_apps_path();
        match prev {
            Some(v) => unsafe { std::env::set_var("WAYLAND_HOME", v) },
            None => unsafe { std::env::remove_var("WAYLAND_HOME") },
        }
        assert_eq!(p, tmp.path().join("cua").join("seen-apps.json"));
    }

    #[test]
    #[serial_test::serial]
    fn migration_skipped_when_wayland_home_set() {
        // With WAYLAND_HOME set, migration must be an unconditional no-op
        // (no inheritance of shared legacy grants) — even though new_path
        // is absent.
        let tmp = tempfile::tempdir().unwrap();
        let new_path = tmp.path().join("cua").join("seen-apps.json");
        let prev = std::env::var_os("WAYLAND_HOME");
        unsafe { std::env::set_var("WAYLAND_HOME", tmp.path()) };
        super::migrate_legacy_seen_apps(&new_path);
        match prev {
            Some(v) => unsafe { std::env::set_var("WAYLAND_HOME", v) },
            None => unsafe { std::env::remove_var("WAYLAND_HOME") },
        }
        assert!(!new_path.exists());
    }

    #[test]
    #[serial_test::serial]
    fn migration_is_idempotent_and_does_not_clobber() {
        // WAYLAND_HOME unset (single-profile upgrade case); new_path present
        // → migration must be a no-op and must not overwrite existing data.
        let tmp = tempfile::tempdir().unwrap();
        let new_path = tmp.path().join("cua").join("seen-apps.json");
        std::fs::create_dir_all(new_path.parent().unwrap()).unwrap();
        std::fs::write(&new_path, br#"["existing"]"#).unwrap();
        let prev = std::env::var_os("WAYLAND_HOME");
        unsafe { std::env::remove_var("WAYLAND_HOME") };
        super::migrate_legacy_seen_apps(&new_path);
        if let Some(v) = prev {
            unsafe { std::env::set_var("WAYLAND_HOME", v) }
        }
        assert_eq!(std::fs::read(&new_path).unwrap(), br#"["existing"]"#);
    }

    fn click() -> CuaOp {
        CuaOp::LeftClick {
            x: 10,
            y: 20,
            button: MouseButton::Left,
            mods: KeyMods::default(),
        }
    }

    #[test]
    fn permissive_policy_allows_everything() {
        let p = CuaPolicy::permissive();
        assert_eq!(p.check_op(&click(), "AnyApp"), CuaPolicyOutcome::Allow);
    }

    #[test]
    fn forbidden_app_is_rejected() {
        let p = CuaPolicy {
            forbidden_apps: vec!["1Password".into()],
            first_time_per_app_approval: false,
            ..CuaPolicy::permissive()
        };
        let outcome = p.check_op(&click(), "1Password");
        match outcome {
            CuaPolicyOutcome::Reject { reason } => {
                assert!(reason.to_lowercase().contains("forbidden"))
            }
            other => panic!("expected Reject, got {other:?}"),
        }
    }

    #[test]
    fn forbidden_key_combo_is_rejected() {
        let p = CuaPolicy {
            forbidden_key_combos: vec!["cmd+q+system".into()],
            first_time_per_app_approval: false,
            ..CuaPolicy::permissive()
        };
        let op = CuaOp::Key {
            keys: "cmd+q+system".into(),
            mods: KeyMods::default(),
        };
        assert!(matches!(
            p.check_op(&op, "Finder"),
            CuaPolicyOutcome::Reject { .. }
        ));
    }

    #[test]
    fn require_approval_app_routes_to_suspend() {
        let p = CuaPolicy {
            require_approval_for_app: vec!["Keychain Access".into()],
            first_time_per_app_approval: false,
            ..CuaPolicy::permissive()
        };
        let outcome = p.check_op(&click(), "Keychain Access");
        match outcome {
            CuaPolicyOutcome::Suspend { reason } => {
                assert!(reason.to_lowercase().contains("approval"));
            }
            other => panic!("expected Suspend, got {other:?}"),
        }
    }

    #[test]
    fn first_time_per_app_routes_to_suspend_then_allows_after_mark() {
        let p = CuaPolicy {
            first_time_per_app_approval: true,
            ..CuaPolicy::permissive()
        };
        let first = p.check_op(&click(), "TextEdit");
        assert!(matches!(first, CuaPolicyOutcome::Suspend { .. }));

        // Until we mark, repeated calls still suspend.
        let still = p.check_op(&click(), "TextEdit");
        assert!(matches!(still, CuaPolicyOutcome::Suspend { .. }));

        p.mark_app_seen("TextEdit");
        let after = p.check_op(&click(), "TextEdit");
        assert_eq!(after, CuaPolicyOutcome::Allow);
    }

    #[test]
    fn empty_app_id_still_checks_key_combos_before_failing_closed() {
        let p = CuaPolicy {
            forbidden_key_combos: vec!["ctrl+alt+del".into()],
            first_time_per_app_approval: true,
            ..CuaPolicy::permissive()
        };
        // The app-independent forbidden-combo gate still hard-Rejects
        // even when the frontmost app is unknown.
        let op = CuaOp::Key {
            keys: "ctrl+alt+del".into(),
            mods: KeyMods::default(),
        };
        assert!(matches!(
            p.check_op(&op, ""),
            CuaPolicyOutcome::Reject { .. }
        ));
        // A non-key op with empty app id no longer Allows: with the
        // first-time-per-app gate configured we cannot prove the target
        // app is permitted, so we fail closed to Suspend (H-6/M-17).
        assert!(matches!(
            p.check_op(&click(), ""),
            CuaPolicyOutcome::Suspend { .. }
        ));
    }

    #[test]
    fn empty_app_id_with_forbidden_apps_fails_closed_to_suspend() {
        // H-6/M-17 regression: an unresolved frontmost app id (the
        // production case on Windows/Wayland, and on macOS when the
        // osascript probe fails) must NOT skip the configured
        // forbidden-app gate and silently Allow. It must Suspend.
        let p = CuaPolicy {
            forbidden_apps: vec!["1Password".into()],
            first_time_per_app_approval: false,
            ..CuaPolicy::permissive()
        };
        let outcome = p.check_op(&click(), "");
        match outcome {
            CuaPolicyOutcome::Suspend { reason } => {
                assert!(reason.to_lowercase().contains("frontmost"));
            }
            other => panic!("expected Suspend (fail closed), got {other:?}"),
        }
    }

    #[test]
    fn empty_app_id_with_require_approval_fails_closed_to_suspend() {
        let p = CuaPolicy {
            require_approval_for_app: vec!["Keychain Access".into()],
            first_time_per_app_approval: false,
            ..CuaPolicy::permissive()
        };
        assert!(matches!(
            p.check_op(&click(), ""),
            CuaPolicyOutcome::Suspend { .. }
        ));
    }

    #[test]
    fn empty_app_id_with_no_app_rules_still_allows() {
        // Fail-closed must not over-fire: a fully permissive policy with
        // NO app-scoped rules continues to Allow an unknown app id (the
        // forbidden-key-combo / Type checks still apply on those op
        // kinds, but a plain click is permitted).
        let p = CuaPolicy::permissive();
        assert!(!p.has_app_scoped_rule());
        assert_eq!(p.check_op(&click(), ""), CuaPolicyOutcome::Allow);
    }

    #[test]
    fn normalize_combo_handles_unicode_glyphs() {
        assert_eq!(normalize_combo("⌘Q"), "cmd+q");
        assert_eq!(normalize_combo("Cmd+Q"), "cmd+q");
        assert_eq!(normalize_combo("command-Q"), "cmd+q");
        assert_eq!(normalize_combo("^Q"), "ctrl+q");
        assert_eq!(normalize_combo("Q⌘"), "q+cmd");
    }

    #[test]
    fn check_action_is_alias_for_check_op() {
        let p = CuaPolicy {
            forbidden_apps: vec!["foo".into()],
            first_time_per_app_approval: false,
            ..CuaPolicy::permissive()
        };
        assert!(matches!(
            p.check_action(&click(), "foo"),
            CuaPolicyOutcome::Reject { .. }
        ));
    }
}
