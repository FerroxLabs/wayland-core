//! Translation between Honcho wire shapes and `wcore-user-model` types.
//!
//! Honcho exposes a single `UserProfile` with a free-form
//! `preferences: HashMap<String, String>`. `UserBrief` and `Preferences`
//! are richer + typed. The mapping is best-effort and lossless in both
//! directions for the fields we actually carry; unknown keys round-trip
//! through `Preferences::tags` so downstream Honcho writes by other
//! clients are not clobbered.

use std::collections::BTreeMap;

use wayland_honcho::{DialecticInference as HonchoInference, UserProfile};
use wcore_user_model::brief::{DialecticInference, UserBrief, UserStyle};
use wcore_user_model::observation::{Observation, Outcome};
use wcore_user_model::preferences::{ExpertiseLevel, Preferences};

/// Reserved key prefixes Honcho stores on behalf of the engine. These
/// are mapped onto typed UserBrief / Preferences fields; anything else
/// in `UserProfile::preferences` lands in `Preferences::tags` so it
/// survives a round-trip.
const KEY_NAME: &str = "wayland.name";
const KEY_SUMMARY: &str = "wayland.summary";
const KEY_LAST_OBSERVED_TS: &str = "wayland.last_observed_ts";
const KEY_STYLE_FORMALITY: &str = "wayland.style.formality";
const KEY_STYLE_ENERGY: &str = "wayland.style.energy";
const KEY_STYLE_TERSENESS: &str = "wayland.style.terseness";
const KEY_STYLE_EMOJI_USE: &str = "wayland.style.emoji_use";
const EXPERTISE_PREFIX: &str = "wayland.expertise.";

/// Honcho `UserProfile` → engine `UserBrief`. Unknown keys are ignored
/// here (they flow through `profile_to_preferences`).
pub fn profile_to_brief(profile: &UserProfile) -> UserBrief {
    let name = profile.preferences.get(KEY_NAME).cloned();
    let summary = profile
        .preferences
        .get(KEY_SUMMARY)
        .cloned()
        .unwrap_or_default();
    let last_observed_ts = profile
        .preferences
        .get(KEY_LAST_OBSERVED_TS)
        .and_then(|s| s.parse::<i64>().ok())
        .unwrap_or_default();
    let style = UserStyle {
        formality: read_f32(&profile.preferences, KEY_STYLE_FORMALITY),
        energy: read_f32(&profile.preferences, KEY_STYLE_ENERGY),
        terseness: read_f32(&profile.preferences, KEY_STYLE_TERSENESS),
        emoji_use: read_f32(&profile.preferences, KEY_STYLE_EMOJI_USE),
    };
    UserBrief {
        name,
        summary,
        style,
        last_observed_ts,
        // v0.8.1 U3 — dialectic is layered in by the adapter's
        // `brief()` impl after this mapping; profile-to-brief itself
        // has no access to the representations endpoint.
        dialectic: Vec::new(),
    }
}

/// Honcho `UserProfile` → engine `Preferences`. Everything not claimed
/// by `profile_to_brief` (and not an `EXPERTISE_PREFIX`-prefixed entry)
/// passes through as a free-form tag.
pub fn profile_to_preferences(profile: &UserProfile) -> Preferences {
    let mut expertise: BTreeMap<String, ExpertiseLevel> = BTreeMap::new();
    let mut tags: BTreeMap<String, String> = BTreeMap::new();
    for (key, value) in &profile.preferences {
        if let Some(domain) = key.strip_prefix(EXPERTISE_PREFIX) {
            if let Some(level) = parse_expertise(value) {
                expertise.insert(domain.to_string(), level);
            } else {
                // Unparseable expertise label — keep as a tag so the
                // information is not silently dropped.
                tags.insert(key.clone(), value.clone());
            }
            continue;
        }
        if is_brief_key(key) {
            // Already surfaced via `profile_to_brief`.
            continue;
        }
        tags.insert(key.clone(), value.clone());
    }
    Preferences { expertise, tags }
}

/// Translate one `Observation` into a sequence of `(key, value)` pairs
/// the Honcho adapter writes via `learn_preference`.
///
/// The mapping is intentionally additive — each observation appends
/// observable signals; Honcho is responsible for whatever aggregation it
/// applies. Variants without a Honcho equivalent return an empty Vec; the
/// caller logs + skips at the call site (this lets tests assert "no
/// writes" without parsing tracing output).
pub fn observation_to_writes(obs: &Observation) -> Vec<(String, String)> {
    let mut writes: Vec<(String, String)> = Vec::new();
    if obs.ts_secs > 0 {
        writes.push((KEY_LAST_OBSERVED_TS.to_string(), obs.ts_secs.to_string()));
    }
    if let Some(fp) = obs.style_fingerprint {
        writes.push((KEY_STYLE_FORMALITY.to_string(), fp[0].to_string()));
        writes.push((KEY_STYLE_ENERGY.to_string(), fp[1].to_string()));
        writes.push((KEY_STYLE_TERSENESS.to_string(), fp[2].to_string()));
        writes.push((KEY_STYLE_EMOJI_USE.to_string(), fp[3].to_string()));
    }
    if let (Some(outcome), Some(domain)) = (obs.outcome, obs.hint.domain.as_deref()) {
        let tag = match outcome {
            Outcome::Accepted | Outcome::Praised => "accepted",
            Outcome::Rejected | Outcome::Corrected => "rejected",
            Outcome::Ignored => "ignored",
            // Outcome is #[non_exhaustive]. Future variants land here
            // and are recorded as "unknown" rather than silently
            // dropped — caller / Honcho can decide what to do.
            _ => "unknown",
        };
        writes.push((format!("{domain}.last_outcome"), tag.to_string()));
    }
    writes
}

/// v0.8.1 U3 — translate one Honcho dialectic inference into the
/// engine's `wcore-user-model` shape. The two structs are kept in lock-step
/// in source even though the F2 invariant forbids `wayland-honcho` from
/// depending on `wcore-user-model`; this is the seam.
pub fn honcho_inf_to_user_model(inf: HonchoInference) -> DialecticInference {
    DialecticInference {
        kind: inf.kind,
        subject: inf.subject,
        value: inf.value,
        confidence: inf.confidence,
        evidence_count: inf.evidence_count,
    }
}

fn is_brief_key(key: &str) -> bool {
    matches!(
        key,
        KEY_NAME
            | KEY_SUMMARY
            | KEY_LAST_OBSERVED_TS
            | KEY_STYLE_FORMALITY
            | KEY_STYLE_ENERGY
            | KEY_STYLE_TERSENESS
            | KEY_STYLE_EMOJI_USE
    )
}

fn read_f32(map: &std::collections::HashMap<String, String>, key: &str) -> f32 {
    map.get(key)
        .and_then(|s| s.parse::<f32>().ok())
        .unwrap_or_default()
}

fn parse_expertise(s: &str) -> Option<ExpertiseLevel> {
    match s {
        "novice" => Some(ExpertiseLevel::Novice),
        "intermediate" => Some(ExpertiseLevel::Intermediate),
        "expert" => Some(ExpertiseLevel::Expert),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;
    use wcore_user_model::observation::ToolHint;

    fn profile_with(pairs: &[(&str, &str)]) -> UserProfile {
        let mut preferences = HashMap::new();
        for (k, v) in pairs {
            preferences.insert((*k).to_string(), (*v).to_string());
        }
        UserProfile {
            user_id: "alice".to_string(),
            preferences,
        }
    }

    #[test]
    fn brief_maps_name_summary_ts_and_style() {
        let p = profile_with(&[
            (KEY_NAME, "Alice"),
            (KEY_SUMMARY, "Senior Rust engineer"),
            (KEY_LAST_OBSERVED_TS, "1234"),
            (KEY_STYLE_FORMALITY, "0.7"),
            (KEY_STYLE_ENERGY, "0.4"),
            (KEY_STYLE_TERSENESS, "0.6"),
            (KEY_STYLE_EMOJI_USE, "0.1"),
        ]);
        let brief = profile_to_brief(&p);
        assert_eq!(brief.name.as_deref(), Some("Alice"));
        assert_eq!(brief.summary, "Senior Rust engineer");
        assert_eq!(brief.last_observed_ts, 1234);
        assert!((brief.style.formality - 0.7).abs() < 1e-6);
        assert!((brief.style.emoji_use - 0.1).abs() < 1e-6);
    }

    #[test]
    fn preferences_split_expertise_and_tags() {
        let p = profile_with(&[
            ("wayland.expertise.rust", "expert"),
            ("wayland.expertise.react", "intermediate"),
            ("rust.last_outcome", "accepted"),
            (KEY_NAME, "Alice"), // brief key — should NOT appear in tags
        ]);
        let prefs = profile_to_preferences(&p);
        assert_eq!(prefs.expertise.get("rust"), Some(&ExpertiseLevel::Expert));
        assert_eq!(
            prefs.expertise.get("react"),
            Some(&ExpertiseLevel::Intermediate)
        );
        assert_eq!(
            prefs.tags.get("rust.last_outcome").map(String::as_str),
            Some("accepted")
        );
        assert!(!prefs.tags.contains_key(KEY_NAME));
    }

    #[test]
    fn observation_with_style_emits_four_axes() {
        let obs = Observation {
            style_fingerprint: Some([0.8, 0.5, 0.6, 0.1]),
            ts_secs: 100,
            ..Observation::default()
        };
        let writes = observation_to_writes(&obs);
        // ts + 4 style axes = 5 writes.
        assert_eq!(writes.len(), 5);
        assert!(writes.iter().any(|(k, _)| k == KEY_STYLE_FORMALITY));
        assert!(writes.iter().any(|(k, _)| k == KEY_LAST_OBSERVED_TS));
    }

    #[test]
    fn observation_outcome_with_domain_emits_outcome_tag() {
        let obs = Observation {
            outcome: Some(Outcome::Accepted),
            hint: ToolHint {
                domain: Some("rust".to_string()),
                ..Default::default()
            },
            ..Observation::default()
        };
        let writes = observation_to_writes(&obs);
        assert!(
            writes
                .iter()
                .any(|(k, v)| k == "rust.last_outcome" && v == "accepted")
        );
    }

    #[test]
    fn observation_outcome_without_domain_is_dropped() {
        // No tracing assertion (would need a subscriber); just check the
        // write list is empty for a no-signal observation.
        let obs = Observation {
            outcome: Some(Outcome::Accepted),
            ..Observation::default()
        };
        let writes = observation_to_writes(&obs);
        assert!(writes.is_empty());
    }
}
