//! Reusable invariant checkers for Proving Ground cells.
//!
//! Each function takes a slice of `RunRecord`s (one per launch of a
//! `Session`) and returns `Ok(())` when the invariant holds or
//! `Err(String)` with a human-readable failure message when it does not.
//!
//! Invariants are composable: a cell can assert several of them in
//! sequence so the diagnostic message points at the specific violation.

use super::record::RunRecord;

/// Assert that a scroll/reveal key sequence was able to bring `target` into
/// view.
///
/// `reached` is the result of [`super::reach_text`] — `true` if the target
/// appeared on screen after sending the reveal keys, `false` if it never did.
///
/// This is a thin wrapper so invariant failures report the target label and a
/// consistent message rather than a bare `assert!` with no context.
///
/// # Panics
///
/// Panics when `reached` is `false`, with a message naming the target.
pub fn content_reachable(target: &str, reached: bool) {
    assert!(
        reached,
        "content_reachable: '{target}' was not found on screen after sending \
         the canonical reveal keys — the surface may not be scrollable"
    );
}

/// Assert that after a connect run, a relaunch record lands on Workspace.
///
/// Specifically checks:
/// 1. At least two records are present (`records[0]` = connect run,
///    `records[1]` = relaunch run).
/// 2. The connect run's `config_toml` is `Some` and contains a provider
///    slug, confirming that onboarding wrote the selection to disk.
/// 3. The relaunch run's `final_screen` contains "Workspace" and does NOT
///    contain "connect a provider to begin" — the onboarding subtitle —
///    confirming the binary booted to Workspace rather than re-entering
///    onboarding.
pub fn config_persists(records: &[RunRecord]) -> Result<(), String> {
    if records.len() < 2 {
        return Err(format!(
            "config_persists requires at least 2 records (connect + relaunch), \
             got {}",
            records.len()
        ));
    }

    let connect_rec = &records[0];
    let relaunch_rec = &records[1];

    // The connect run must have written a config.toml with a provider.
    match &connect_rec.config_toml {
        None => {
            return Err(
                "connect run: config.toml was not written — onboarding must persist \
                 the provider choice to disk before the user quits"
                    .to_string(),
            );
        }
        Some(toml) if toml.trim().is_empty() => {
            return Err(
                "connect run: config.toml exists but is empty — expected at least \
                 a [default] provider entry"
                    .to_string(),
            );
        }
        Some(toml) => {
            // A minimal config must contain at least one provider slug.
            // We do not prescribe which slug — any known provider name is
            // sufficient evidence that onboarding made a choice.
            let known_slugs = [
                "anthropic",
                "openai",
                "openrouter",
                "gemini",
                "groq",
                "xai",
                "mistral",
                "deepseek",
                "fireworks",
                "together",
                "cerebras",
                "perplexity",
                "moonshot",
                "ollama",
            ];
            let has_provider = known_slugs.iter().any(|slug| toml.contains(slug));
            if !has_provider {
                return Err(format!(
                    "connect run: config.toml does not contain a known provider slug.\n\
                     config.toml contents:\n{toml}"
                ));
            }
        }
    }

    // The relaunch run must boot to Workspace, not Onboarding.
    if !relaunch_rec.final_screen.contains("Workspace") {
        return Err(format!(
            "relaunch: final screen does not contain 'Workspace' — binary may \
             have re-entered onboarding.\nfinal screen:\n{}",
            relaunch_rec.final_screen
        ));
    }
    if relaunch_rec
        .final_screen
        .contains("connect a provider to begin")
    {
        return Err(format!(
            "relaunch: final screen contains the onboarding subtitle \
             'connect a provider to begin' — binary re-entered onboarding \
             instead of landing on Workspace.\nfinal screen:\n{}",
            relaunch_rec.final_screen
        ));
    }

    Ok(())
}
