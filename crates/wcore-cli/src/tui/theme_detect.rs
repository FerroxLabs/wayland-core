//! Terminal background detection (Wave 8 / §5 / Q1).
//!
//! `detect_light_mode()` decides whether the host terminal is painting on a
//! LIGHT background so `Theme::for_mode(ThemeMode::Auto)` can pick the light
//! Hearth palette. The decision is a 5-tier fallback modelled on the prior
//! Wayland Python engine's theme detection:
//!
//! 1. **Explicit `/theme` setting** — handled by the caller (a `ThemeMode`
//!    of `Light`/`Dark` never reaches this module; only `Auto` does).
//! 2. **`COLORFGBG`** — the de-facto env var (`fg;bg`, sometimes
//!    `fg;default;bg`); a background ANSI index ≥ 11 is the light end of the
//!    16-colour ramp, so it signals a light terminal.
//! 3. **`$TERM_PROGRAM` heuristic** — Apple Terminal's stock profile is a
//!    light background, so absent a `COLORFGBG` we treat it as light.
//! 4. **Truecolor probe** — handled by the caller (`Theme::for_mode` already
//!    branches on truecolor for the palette depth; there is no reliable
//!    background-colour probe without an OSC 11 round-trip, which the sync
//!    detect path deliberately avoids).
//! 5. **Default dark** — the safe fallback; the TUI's home look is dark, so
//!    an unknown terminal stays dark rather than flashing a light palette.

/// Parse a `COLORFGBG` value into a light/dark verdict.
///
/// The env var is `fg;bg` (some terminals emit `fg;default;bg`); the LAST
/// `;`-separated field is the background ANSI colour index. An index ≥ 11
/// sits in the light half of the 16-colour ramp, so it means a light
/// background. Returns `None` when the value has no parseable trailing index
/// (e.g. `"default"` or garbage), letting the caller fall through to the
/// next tier.
///
/// Examples: `"0;15"` → `Some(true)` (white bg), `"15;0"` → `Some(false)`
/// (black bg), `"7;0"` → `Some(false)`, `"garbage"` → `None`.
pub fn parse_colorfgbg(v: &str) -> Option<bool> {
    v.rsplit(';')
        .next()?
        .trim()
        .parse::<u32>()
        .ok()
        .map(|idx| idx >= 11)
}

/// Detect whether the terminal background is light (the 5-tier fallback —
/// see the module docs). Pure with respect to its inputs save for the env
/// reads; the parse logic lives in [`parse_colorfgbg`] so it is unit-tested
/// without touching process-global env.
pub fn detect_light_mode() -> bool {
    // Tier 2 — COLORFGBG. A parseable trailing index is authoritative.
    if let Ok(v) = std::env::var("COLORFGBG")
        && let Some(light) = parse_colorfgbg(&v)
    {
        return light;
    }
    // Tier 3 — TERM_PROGRAM heuristic. Apple Terminal's default is light.
    if let Ok(tp) = std::env::var("TERM_PROGRAM")
        && tp == "Apple_Terminal"
    {
        return true;
    }
    // Tier 5 — default dark (tier 4 truecolor probe is the caller's job).
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn colorfgbg_light_vs_dark() {
        // White-on-black inverse: bg index 15 ⇒ light.
        assert_eq!(parse_colorfgbg("0;15"), Some(true));
        // Black bg index 0 ⇒ dark.
        assert_eq!(parse_colorfgbg("15;0"), Some(false));
        // Unparseable trailing field ⇒ no verdict, fall through.
        assert_eq!(parse_colorfgbg("garbage"), None);
    }

    #[test]
    fn colorfgbg_threshold_is_eleven() {
        // 10 is the last dark-half index; 11 is the first light-half index.
        assert_eq!(parse_colorfgbg("0;10"), Some(false));
        assert_eq!(parse_colorfgbg("0;11"), Some(true));
    }

    #[test]
    fn colorfgbg_handles_three_field_form_and_whitespace() {
        // Some terminals emit `fg;default;bg` — the LAST field is the bg.
        assert_eq!(parse_colorfgbg("15;default;0"), Some(false));
        assert_eq!(parse_colorfgbg("0;default;15"), Some(true));
        // Trailing whitespace must not defeat the parse.
        assert_eq!(parse_colorfgbg("0;15 "), Some(true));
    }

    #[test]
    fn colorfgbg_empty_or_no_index_is_none() {
        assert_eq!(parse_colorfgbg(""), None);
        assert_eq!(parse_colorfgbg("0;default"), None);
    }

    #[test]
    fn detect_light_mode_reads_colorfgbg_then_falls_back_to_dark() {
        // `detect_light_mode` reads process-global env. This is the only
        // test in this module that sets COLORFGBG/TERM_PROGRAM, so there is
        // no concurrent reader to race with inside this binary.
        //
        // SAFETY: single-threaded test body; set/remove are paired and the
        // prior values are restored at the end.
        let prior_fgbg = std::env::var_os("COLORFGBG");
        let prior_tp = std::env::var_os("TERM_PROGRAM");

        // Tier 2: COLORFGBG light.
        unsafe { std::env::set_var("COLORFGBG", "0;15") };
        unsafe { std::env::remove_var("TERM_PROGRAM") };
        assert!(detect_light_mode(), "COLORFGBG=0;15 must detect light");

        // Tier 2: COLORFGBG dark.
        unsafe { std::env::set_var("COLORFGBG", "15;0") };
        assert!(!detect_light_mode(), "COLORFGBG=15;0 must detect dark");

        // Tier 5: nothing set ⇒ default dark.
        unsafe { std::env::remove_var("COLORFGBG") };
        unsafe { std::env::remove_var("TERM_PROGRAM") };
        assert!(
            !detect_light_mode(),
            "an unset environment must default to dark"
        );

        // Tier 3: Apple Terminal with no COLORFGBG ⇒ light.
        unsafe { std::env::set_var("TERM_PROGRAM", "Apple_Terminal") };
        assert!(
            detect_light_mode(),
            "Apple_Terminal with no COLORFGBG must detect light"
        );

        unsafe {
            match prior_fgbg {
                Some(v) => std::env::set_var("COLORFGBG", v),
                None => std::env::remove_var("COLORFGBG"),
            }
            match prior_tp {
                Some(v) => std::env::set_var("TERM_PROGRAM", v),
                None => std::env::remove_var("TERM_PROGRAM"),
            }
        }
    }
}
