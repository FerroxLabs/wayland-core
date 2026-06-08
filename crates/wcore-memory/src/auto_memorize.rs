//! Auto-memorize SessionEnd trigger for wcore-memory.
//!
//! Ports the SessionEnd dream-runner hook pattern from
//! `ijfw/mcp-server/src/dream/runner.mjs`: at session end, score candidate
//! facts surfaced during the session and persist the ones above threshold
//! to long-term memory.
//!
//! # Privacy
//!
//! Auto-memorize is ON by default (2026-06-04 smart-default decision). The
//! user can opt OUT by writing `off` to a decision file at
//! `wayland_config_dir()/auto-memorize.consent`, or with the
//! `WAYLAND_AUTO_MEMORIZE=off` env kill switch. Resolved via
//! `wcore_config::config::wayland_config_dir()` so `WAYLAND_HOME`
//! hermetically sandboxes the file (F-010, #270). With an opt-out recorded,
//! `run_session_end` returns a `ConsentNotGranted` skip — no facts leave the
//! session.
//!
//! # Rollback
//!
//! Set `WAYLAND_AUTO_MEMORIZE=off` in the environment to make
//! `consent_granted()` return `false` unconditionally, even when the
//! consent file exists. This is the one-shot kill switch for emergency
//! disable without touching disk.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// Environment variable that, when set to `"off"` (case-insensitive),
/// forces `consent_granted()` to return `false` regardless of the
/// presence of the consent file.
pub const ENV_AUTO_MEMORIZE: &str = "WAYLAND_AUTO_MEMORIZE";

/// Filename of the opt-in consent flag.
pub const CONSENT_FILE_NAME: &str = "auto-memorize.consent";

// ---------------------------------------------------------------------------
// Consent gate
// ---------------------------------------------------------------------------

/// Returns the path of the opt-in consent file.
///
/// Auto-memorize is OFF unless this file exists on disk (and the
/// `WAYLAND_AUTO_MEMORIZE` env var is not set to `"off"`).
///
/// Resolution: `wcore_config::config::wayland_config_dir()` so
/// `WAYLAND_HOME` hermetically sandboxes the consent flag alongside the
/// rest of the engine's on-disk state (F-010, #270). The helper has its
/// own `PathBuf::from("wayland-core")` fallback when the platform exposes
/// no config dir, so this function never panics.
pub fn consent_file_path() -> PathBuf {
    wcore_config::config::wayland_config_dir().join(CONSENT_FILE_NAME)
}

/// Returns `true` when auto-memorize is enabled for this session.
///
/// Smart default (2026-06-04): auto-memorize is **ON by default**. The decision
/// file at `consent_file_path()` is now an explicit OPT-OUT marker rather than
/// an opt-in flag.
///
/// Decision order:
///   1. If `WAYLAND_AUTO_MEMORIZE=off` (case-insensitive), return `false`
///      regardless of the file. This is the hard kill switch / rollback.
///   2. If the decision file exists and its contents opt out
///      (`off`/`opt-out`/`false`/`no`/`disable`), return `false`.
///   3. Otherwise — absent file, unreadable file, or any other contents
///      (including a legacy `opt-in` file) — return `true`.
pub fn consent_granted() -> bool {
    consent_granted_at(&consent_file_path())
}

/// Testable variant of [`consent_granted`] that accepts an explicit
/// decision-file path. Used by the inline tests with a tempdir-backed
/// path to avoid mutating the user's real config dir.
///
/// Honors the `WAYLAND_AUTO_MEMORIZE=off` env override the same way as
/// [`consent_granted`], and the same default-ON / explicit-opt-out semantics.
pub fn consent_granted_at(path: &Path) -> bool {
    if env_forces_off() {
        return false;
    }
    // Default ON. Only an explicit opt-out recorded in the decision file
    // disables it; absence (or a legacy "opt-in" file) keeps it on.
    match std::fs::read_to_string(path) {
        Ok(contents) => !matches!(
            contents.trim().to_ascii_lowercase().as_str(),
            "off" | "opt-out" | "optout" | "false" | "no" | "disable"
        ),
        Err(_) => true,
    }
}

/// Returns `true` when `WAYLAND_AUTO_MEMORIZE` is set to `"off"`
/// (case-insensitive). Any other value, or an unset var, returns `false`.
fn env_forces_off() -> bool {
    match std::env::var(ENV_AUTO_MEMORIZE) {
        Ok(v) => v.eq_ignore_ascii_case("off"),
        Err(_) => false,
    }
}

// ---------------------------------------------------------------------------
// Public types
// ---------------------------------------------------------------------------

/// A single fact candidate surfaced during a session — the unit that the
/// SessionEnd trigger scores and either persists or drops.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FactCandidate {
    /// Subject of the (S, P, O) triple (e.g. `"user"`, a project name).
    pub subject: String,
    /// Predicate / relation (e.g. `"prefers"`, `"works-on"`).
    pub predicate: String,
    /// Object / value of the triple.
    pub object: String,
    /// Confidence in `[0.0, 1.0]`. Candidates below
    /// [`AutoMemorize::min_confidence`] are filtered out before persistence.
    pub confidence: f32,
}

/// A digest of one session, supplied at SessionEnd.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionDigest {
    pub session_id: String,
    pub turn_count: u32,
    pub fact_candidates: Vec<FactCandidate>,
}

/// Why the SessionEnd trigger skipped persisting facts.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum SkipReason {
    /// Consent file missing (or `WAYLAND_AUTO_MEMORIZE=off`).
    ConsentNotGranted,
    /// Fewer surviving candidates than `min_facts_to_persist`.
    BelowFactThreshold,
    /// Every candidate fell below `min_confidence`.
    BelowConfidenceThreshold,
}

/// Structured outcome of one SessionEnd trigger invocation.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AutoMemorizeReport {
    /// `true` iff the persist closure was invoked.
    pub triggered: bool,
    /// Populated when `triggered == false`.
    pub skipped_reason: Option<SkipReason>,
    /// Number of facts the persist closure reported as written.
    pub facts_persisted: usize,
}

/// Configuration for the SessionEnd auto-memorize trigger.
#[derive(Debug, Clone, Copy)]
pub struct AutoMemorize {
    /// Minimum number of surviving candidates required to fire the
    /// persist closure. Default: 1.
    pub min_facts_to_persist: usize,
    /// Minimum per-candidate confidence to survive the threshold filter.
    /// Default: 0.5.
    pub min_confidence: f32,
}

impl Default for AutoMemorize {
    fn default() -> Self {
        Self {
            min_facts_to_persist: 1,
            min_confidence: 0.5,
        }
    }
}

impl AutoMemorize {
    /// Runs the SessionEnd trigger over `digest`, invoking `persist` only
    /// when consent is granted AND enough candidates survive the
    /// confidence filter.
    ///
    /// The `persist` closure receives the filtered fact slice and returns
    /// the number of facts actually written to storage. Callers wire this
    /// to the `v2` memory API (e.g. `MemoryApi::write_fact`); this module
    /// stays storage-agnostic so it can be unit-tested without a DB.
    pub fn run_session_end<F>(&self, digest: SessionDigest, persist: F) -> AutoMemorizeReport
    where
        F: FnOnce(&[FactCandidate]) -> usize,
    {
        self.run_session_end_with_consent(digest, consent_granted(), persist)
    }

    /// Variant of [`run_session_end`] that takes the consent decision as
    /// an explicit argument. Used by the inline tests to drive the full
    /// state machine without touching the real consent file location.
    fn run_session_end_with_consent<F>(
        &self,
        digest: SessionDigest,
        consent: bool,
        persist: F,
    ) -> AutoMemorizeReport
    where
        F: FnOnce(&[FactCandidate]) -> usize,
    {
        if !consent {
            return AutoMemorizeReport {
                triggered: false,
                skipped_reason: Some(SkipReason::ConsentNotGranted),
                facts_persisted: 0,
            };
        }

        let total_candidates = digest.fact_candidates.len();
        let filtered: Vec<FactCandidate> = digest
            .fact_candidates
            .into_iter()
            .filter(|c| c.confidence >= self.min_confidence)
            .collect();

        if filtered.len() < self.min_facts_to_persist {
            // Distinguish "everything was below confidence" from "not enough
            // facts survived even though some passed". This is the same
            // contract as the IJFW dream-runner's two-stage skip log lines.
            let reason = if total_candidates > 0 && filtered.is_empty() {
                SkipReason::BelowConfidenceThreshold
            } else {
                SkipReason::BelowFactThreshold
            };
            return AutoMemorizeReport {
                triggered: false,
                skipped_reason: Some(reason),
                facts_persisted: 0,
            };
        }

        let written = persist(&filtered);
        AutoMemorizeReport {
            triggered: true,
            skipped_reason: None,
            facts_persisted: written,
        }
    }
}

// ===========================================================================
// Unit tests
// ===========================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use serial_test::serial;
    use std::sync::{Mutex, OnceLock};

    // -- helpers --------------------------------------------------------------

    fn fact(subject: &str, predicate: &str, object: &str, confidence: f32) -> FactCandidate {
        FactCandidate {
            subject: subject.into(),
            predicate: predicate.into(),
            object: object.into(),
            confidence,
        }
    }

    fn digest_with(candidates: Vec<FactCandidate>) -> SessionDigest {
        SessionDigest {
            session_id: "sess-T2-D4".into(),
            turn_count: 7,
            fact_candidates: candidates,
        }
    }

    /// Process-wide mutex so env-mutating tests serialize even across
    /// `#[serial(env)]` groups in this module — the env var we toggle
    /// (`WAYLAND_AUTO_MEMORIZE`) is process-global.
    fn env_lock() -> &'static Mutex<()> {
        static L: OnceLock<Mutex<()>> = OnceLock::new();
        L.get_or_init(|| Mutex::new(()))
    }

    fn restore_env(key: &str, saved: Option<String>) {
        // SAFETY: only called inside an env_lock guard; single-threaded
        //   w.r.t. other env-mutating tests in this module.
        unsafe {
            match saved {
                Some(v) => std::env::set_var(key, v),
                None => std::env::remove_var(key),
            }
        }
    }

    // -- consent_granted_at ---------------------------------------------------

    #[test]
    #[serial(env)]
    fn consent_granted_at_true_when_path_missing() {
        // Smart default: a missing decision file means auto-memorize is ON.
        let _g = env_lock().lock().unwrap();
        let prior = std::env::var(ENV_AUTO_MEMORIZE).ok();
        // SAFETY: env_lock + #[serial(env)] serialize all env writes.
        unsafe { std::env::remove_var(ENV_AUTO_MEMORIZE) };

        let tmp = tempfile::tempdir().unwrap();
        let missing = tmp.path().join("auto-memorize.consent");
        assert!(!missing.exists());
        assert!(
            consent_granted_at(&missing),
            "absent decision file must default ON"
        );

        restore_env(ENV_AUTO_MEMORIZE, prior);
    }

    #[test]
    #[serial(env)]
    fn consent_granted_at_false_when_file_opts_out() {
        // An explicit opt-out written to the decision file disables it.
        let _g = env_lock().lock().unwrap();
        let prior = std::env::var(ENV_AUTO_MEMORIZE).ok();
        // SAFETY: env_lock + #[serial(env)] serialize all env writes.
        unsafe { std::env::remove_var(ENV_AUTO_MEMORIZE) };

        let tmp = tempfile::tempdir().unwrap();
        let decision = tmp.path().join("auto-memorize.consent");
        std::fs::write(&decision, b"off").unwrap();
        assert!(
            !consent_granted_at(&decision),
            "an 'off' decision file must opt out"
        );
        std::fs::write(&decision, b"opt-out").unwrap();
        assert!(
            !consent_granted_at(&decision),
            "'opt-out' must also opt out"
        );

        restore_env(ENV_AUTO_MEMORIZE, prior);
    }

    #[test]
    #[serial(env)]
    fn consent_granted_at_true_when_file_exists() {
        let _g = env_lock().lock().unwrap();
        let prior = std::env::var(ENV_AUTO_MEMORIZE).ok();
        // SAFETY: env_lock + #[serial(env)] serialize all env writes.
        unsafe { std::env::remove_var(ENV_AUTO_MEMORIZE) };

        let tmp = tempfile::tempdir().unwrap();
        let consent = tmp.path().join("auto-memorize.consent");
        std::fs::write(&consent, b"opt-in").unwrap();
        assert!(consent_granted_at(&consent));

        restore_env(ENV_AUTO_MEMORIZE, prior);
    }

    #[test]
    #[serial(env)]
    fn consent_granted_at_false_when_env_var_off() {
        let _g = env_lock().lock().unwrap();
        let prior = std::env::var(ENV_AUTO_MEMORIZE).ok();

        let tmp = tempfile::tempdir().unwrap();
        let consent = tmp.path().join("auto-memorize.consent");
        std::fs::write(&consent, b"opt-in").unwrap();
        assert!(consent.is_file(), "fixture must exist for this test");

        // SAFETY: env_lock + #[serial(env)] serialize all env writes.
        unsafe { std::env::set_var(ENV_AUTO_MEMORIZE, "off") };
        assert!(
            !consent_granted_at(&consent),
            "env=off must override the consent file"
        );

        // Sanity: a non-"off" value (e.g. "on") should NOT force-off.
        // SAFETY: env_lock + #[serial(env)] serialize all env writes.
        unsafe { std::env::set_var(ENV_AUTO_MEMORIZE, "on") };
        assert!(consent_granted_at(&consent));

        restore_env(ENV_AUTO_MEMORIZE, prior);
    }

    // -- consent_file_path ----------------------------------------------------

    #[test]
    fn consent_file_path_ends_in_expected_components() {
        let p = consent_file_path();
        let s = p.to_string_lossy();
        // The path flows through `wayland_config_dir()`, which always
        // ends in the `wayland-core` segment (or its `WAYLAND_HOME`
        // override). Asserting on `wayland-core` locks the helper as the
        // canonical resolver (F-010, #270).
        assert!(
            s.contains("wayland-core") || std::env::var_os("WAYLAND_HOME").is_some(),
            "consent path {s:?} should include 'wayland-core' subdir when WAYLAND_HOME is unset"
        );
        assert!(
            s.ends_with(CONSENT_FILE_NAME),
            "consent path {s:?} should end with 'auto-memorize.consent'"
        );
    }

    #[test]
    #[serial(env)]
    fn consent_file_path_honors_wayland_home() {
        // Hermeticity: when WAYLAND_HOME is set the consent file MUST
        // resolve inside that sandbox, not the user's real config dir
        // (F-010, #270). Same regression class as F-019.
        let _g = env_lock().lock().unwrap();
        let prior = std::env::var("WAYLAND_HOME").ok();
        let tmp = tempfile::tempdir().unwrap();
        // SAFETY: env_lock + #[serial(env)] serialize all env writes.
        unsafe {
            std::env::set_var("WAYLAND_HOME", tmp.path());
        }

        let p = consent_file_path();
        assert!(
            p.starts_with(tmp.path()),
            "consent path {:?} should be rooted at WAYLAND_HOME ({:?})",
            p,
            tmp.path()
        );
        assert!(p.ends_with(CONSENT_FILE_NAME));

        // SAFETY: see set_var above.
        unsafe {
            match prior {
                Some(v) => std::env::set_var("WAYLAND_HOME", v),
                None => std::env::remove_var("WAYLAND_HOME"),
            }
        }
    }

    // -- run_session_end ------------------------------------------------------

    #[test]
    fn run_session_end_skips_when_consent_false() {
        let am = AutoMemorize::default();
        let digest = digest_with(vec![fact("u", "prefers", "rust", 0.99)]);
        // Drive the state machine directly (consent=false → skip) so this test
        // is hermetic w.r.t. the real decision-file location + default-ON gate.
        let report = am.run_session_end_with_consent(digest, false, |_| 1);
        assert_eq!(
            report,
            AutoMemorizeReport {
                triggered: false,
                skipped_reason: Some(SkipReason::ConsentNotGranted),
                facts_persisted: 0,
            }
        );
    }

    #[test]
    fn run_session_end_persists_when_consent_granted_and_facts_above_threshold() {
        let am = AutoMemorize::default();
        let digest = digest_with(vec![
            fact("u", "prefers", "rust", 0.9),
            fact("u", "works-on", "wayland", 0.8),
        ]);
        let report = am.run_session_end_with_consent(digest, true, |facts| facts.len());
        assert!(report.triggered);
        assert_eq!(report.skipped_reason, None);
        assert_eq!(report.facts_persisted, 2);
    }

    #[test]
    fn run_session_end_below_confidence_threshold() {
        let am = AutoMemorize::default(); // min_confidence = 0.5
        let digest = digest_with(vec![
            fact("u", "prefers", "rust", 0.10),
            fact("u", "works-on", "wayland", 0.20),
        ]);
        let report = am.run_session_end_with_consent(digest, true, |_| {
            panic!("persist must not be invoked when nothing survives");
        });
        assert_eq!(
            report,
            AutoMemorizeReport {
                triggered: false,
                skipped_reason: Some(SkipReason::BelowConfidenceThreshold),
                facts_persisted: 0,
            }
        );
    }

    #[test]
    fn run_session_end_below_fact_threshold() {
        // Tune min_facts > number of candidates that pass the filter.
        let am = AutoMemorize {
            min_facts_to_persist: 3,
            min_confidence: 0.5,
        };
        let digest = digest_with(vec![
            fact("u", "prefers", "rust", 0.99),     // survives
            fact("u", "works-on", "wayland", 0.95), // survives
            fact("u", "ignores", "css", 0.10),      // filtered
        ]);
        let report = am.run_session_end_with_consent(digest, true, |_| {
            panic!("persist must not be invoked below fact threshold");
        });
        assert_eq!(
            report,
            AutoMemorizeReport {
                triggered: false,
                skipped_reason: Some(SkipReason::BelowFactThreshold),
                facts_persisted: 0,
            }
        );
    }

    #[test]
    fn run_session_end_persist_callback_invoked_with_filtered_facts() {
        let am = AutoMemorize::default();
        let kept = fact("u", "prefers", "rust", 0.99);
        let dropped = fact("u", "ignores", "css", 0.10);
        let digest = digest_with(vec![kept.clone(), dropped.clone()]);

        let mut seen: Vec<FactCandidate> = Vec::new();
        let report = am.run_session_end_with_consent(digest, true, |facts| {
            seen = facts.to_vec();
            facts.len()
        });

        assert!(report.triggered);
        assert_eq!(report.facts_persisted, 1);
        assert_eq!(seen, vec![kept]);
        assert!(
            !seen.contains(&dropped),
            "low-confidence fact must not reach persist closure"
        );
    }

    // -- defaults / wiring ----------------------------------------------------

    #[test]
    fn default_thresholds_sensible() {
        let am = AutoMemorize::default();
        assert_eq!(am.min_facts_to_persist, 1);
        assert!(
            (am.min_confidence - 0.5).abs() < f32::EPSILON,
            "min_confidence default drifted from 0.5"
        );
    }

    #[test]
    fn skip_reason_serializes_distinctly() {
        // All three variants must roundtrip and produce distinct JSON
        // payloads so external observers can disambiguate the skip cause.
        for r in [
            SkipReason::ConsentNotGranted,
            SkipReason::BelowFactThreshold,
            SkipReason::BelowConfidenceThreshold,
        ] {
            let s = serde_json::to_string(&r).unwrap();
            let back: SkipReason = serde_json::from_str(&s).unwrap();
            assert_eq!(r, back, "roundtrip failed for {r:?}");
        }
        let a = serde_json::to_string(&SkipReason::ConsentNotGranted).unwrap();
        let b = serde_json::to_string(&SkipReason::BelowFactThreshold).unwrap();
        let c = serde_json::to_string(&SkipReason::BelowConfidenceThreshold).unwrap();
        assert_ne!(a, b);
        assert_ne!(b, c);
        assert_ne!(a, c);
    }

    #[test]
    fn report_with_zero_candidates_is_below_fact_threshold() {
        // Edge case: consent granted but the session produced no candidates
        // at all. Must be BelowFactThreshold, not BelowConfidenceThreshold —
        // there was nothing to fall below confidence.
        let am = AutoMemorize::default();
        let digest = digest_with(vec![]);
        let report = am.run_session_end_with_consent(digest, true, |_| 99);
        assert_eq!(report.skipped_reason, Some(SkipReason::BelowFactThreshold));
        assert!(!report.triggered);
        assert_eq!(report.facts_persisted, 0);
    }
}
