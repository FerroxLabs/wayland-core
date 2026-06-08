//! The TUI color theme — the Hearth Palette.
//!
//! FROZEN Wave-0 contract: the `Theme` struct and its field set are the
//! integration boundary every widget and surface paints against. T0.4
//! fills `hearth()` and `no_color()` with the real token values from the
//! brand spec.
//!
//! Every field is a `ratatui::style::Color` so call sites compose directly
//! into `Style`/`Span` without conversion.
//!
//! Brand rule §07: FLAT color only — a single accent (the orange), no
//! gradient or shadow emulation. Widgets use these tokens as-is and never
//! synthesize intermediate shades.

use ratatui::style::Color;

use super::theme_detect;

/// The light/dark mode the TUI resolves its [`Theme`] from (W8 / §5 / Q1).
///
/// `Light`/`Dark` are explicit user choices (via `/theme`); `Auto` defers to
/// the terminal-background heuristic in [`theme_detect::detect_light_mode`].
/// The default is `Dark` — the TUI's home look — so an unconfigured session
/// never flashes a light palette.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ThemeMode {
    /// Force the light Hearth palette.
    Light,
    /// Force the dark Hearth palette (the default).
    #[default]
    Dark,
    /// Resolve light vs dark from the terminal background (`COLORFGBG`,
    /// `TERM_PROGRAM`), defaulting to dark when undetermined.
    Auto,
}

/// The TUI color theme. FROZEN Wave-0 contract.
///
/// All fields are `ratatui::style::Color`. The `orange*` family is the
/// brand accent; the `surface*`/`bg`/`border` family is structural
/// chrome; `text*` is foreground copy; `success`/`warning`/`error` are
/// the status palette.
#[derive(Debug, Clone, Copy)]
pub struct Theme {
    /// The primary brand accent (Hearth orange).
    pub orange: Color,
    /// The accent in a hover/active state.
    pub orange_hover: Color,
    /// A muted/desaturated accent for secondary emphasis.
    pub orange_muted: Color,
    /// A light accent tint for subtle highlights.
    pub orange_light: Color,
    /// The base background color of the whole UI.
    pub bg: Color,
    /// A raised panel/surface background.
    pub surface: Color,
    /// A further-elevated surface (modals, overlays).
    pub surface_elevated: Color,
    /// A surface in a hover/focus state.
    pub surface_hover: Color,
    /// The border color for panels and dividers.
    pub border: Color,
    /// The primary foreground text color.
    pub text: Color,
    /// Dimmed foreground text (secondary copy).
    pub text_dim: Color,
    /// Muted foreground text (tertiary copy, placeholders).
    pub text_muted: Color,
    /// v0.9.3 — running-glyph grey (#c8c8c8 dark / #585858 light).
    pub text_running: Color,
    /// The success status color.
    pub success: Color,
    /// The warning status color.
    pub warning: Color,
    /// The error status color.
    pub error: Color,
    /// The markdown heading color (used by `render_markdown` for H1-H3).
    /// Added by v0.9.0 W2 C1; sits inside the existing palette without
    /// disturbing brand chrome.
    pub heading: Color,
    /// The markdown link color (used by `render_markdown` for `[text](url)`).
    pub link: Color,
}

impl Theme {
    /// The Hearth Palette — the default themed look.
    ///
    /// Token values are the Forge Suite brand spec §09. Each hex is
    /// converted to a 24-bit `Color::Rgb` so the look is identical on any
    /// truecolor terminal (the only mode the TUI targets when themed).
    pub fn hearth() -> Self {
        Self {
            orange: Color::Rgb(0xff, 0x6b, 0x35),
            orange_hover: Color::Rgb(0xff, 0x82, 0x55),
            orange_muted: Color::Rgb(0xcc, 0x55, 0x29),
            orange_light: Color::Rgb(0xff, 0xb3, 0x99),
            bg: Color::Rgb(0x0d, 0x0d, 0x0d),
            surface: Color::Rgb(0x14, 0x14, 0x14),
            surface_elevated: Color::Rgb(0x1a, 0x1a, 0x1a),
            surface_hover: Color::Rgb(0x26, 0x26, 0x26),
            border: Color::Rgb(0x33, 0x33, 0x33),
            text: Color::Rgb(0xf0, 0xf0, 0xf0),
            text_dim: Color::Rgb(0xaa, 0xaa, 0xaa),
            text_muted: Color::Rgb(0x77, 0x77, 0x77),
            text_running: Color::Rgb(0xc8, 0xc8, 0xc8),
            success: Color::Rgb(0x34, 0xd3, 0x99),
            warning: Color::Rgb(0xfb, 0xbf, 0x24),
            error: Color::Rgb(0xf8, 0x71, 0x71),
            // Cyan-leaning teal — readable against the dark chrome and
            // distinct from both `orange` (inline code) and `success`.
            heading: Color::Rgb(0x6c, 0xc9, 0xd0),
            // Soft blue — the conventional link affordance, distinct from
            // `heading` so [text](url) reads as interactive, not structural.
            link: Color::Rgb(0x7a, 0xa2, 0xf7),
        }
    }

    /// A color-free theme honoring `NO_COLOR`. Every field resolves to
    /// `Color::Reset` so the terminal paints with its own default
    /// foreground/background and the UI renders monochrome.
    pub fn no_color() -> Self {
        let r = Color::Reset;
        Self {
            orange: r,
            orange_hover: r,
            orange_muted: r,
            orange_light: r,
            bg: r,
            surface: r,
            surface_elevated: r,
            surface_hover: r,
            border: r,
            text: r,
            text_dim: r,
            text_muted: r,
            text_running: r,
            success: r,
            warning: r,
            error: r,
            heading: r,
            link: r,
        }
    }

    /// The Hearth Palette mapped to the nearest xterm 256-colour indices.
    ///
    /// These are approximate matches for the Rgb tokens in `hearth()` so
    /// the brand feel is preserved on terminals that support 256-colour
    /// (xterm-256color) but not truecolor (F-058). The mapping was derived
    /// from the standard 256-colour cube, not by eye.
    pub fn hearth_256() -> Self {
        Self {
            // #ff6b35 ≈ xterm 202 (OrangeRed1 — closest orange in the cube)
            orange: Color::Indexed(202),
            // #ff8255 ≈ xterm 209 (Salmon1)
            orange_hover: Color::Indexed(209),
            // #cc5529 ≈ xterm 166 (DarkOrange3)
            orange_muted: Color::Indexed(166),
            // #ffb399 ≈ xterm 216 (LightSalmon1)
            orange_light: Color::Indexed(216),
            // #0d0d0d ≈ xterm 16 (nearest to near-black)
            bg: Color::Indexed(16),
            // #141414 ≈ xterm 233 (Grey7)
            surface: Color::Indexed(233),
            // #1a1a1a ≈ xterm 234 (Grey11)
            surface_elevated: Color::Indexed(234),
            // #262626 ≈ xterm 235 (Grey15)
            surface_hover: Color::Indexed(235),
            // #333333 ≈ xterm 236 (Grey19)
            border: Color::Indexed(236),
            // #f0f0f0 ≈ xterm 255 (Grey93)
            text: Color::Indexed(255),
            // #aaaaaa ≈ xterm 248 (Grey54)
            text_dim: Color::Indexed(248),
            // #777777 ≈ xterm 243 (Grey46)
            text_muted: Color::Indexed(243),
            // #c8c8c8 ≈ xterm 251 (Grey78) — running glyph grey
            text_running: Color::Indexed(251),
            // #34d399 ≈ xterm 79 (MediumAquamarine)
            success: Color::Indexed(79),
            // #fbbf24 ≈ xterm 220 (Gold1)
            warning: Color::Indexed(220),
            // #f87171 ≈ xterm 210 (LightCoral)
            error: Color::Indexed(210),
            // #6cc9d0 ≈ xterm 80 (DarkSlateGray2 / light cyan)
            heading: Color::Indexed(80),
            // #7aa2f7 ≈ xterm 111 (LightSteelBlue)
            link: Color::Indexed(111),
        }
    }

    /// The Hearth Palette re-tuned for a LIGHT terminal background (W8 / §5).
    ///
    /// The brand accent is **PINNED** to `#ff6b35` in light mode (audit MED —
    /// no `orange_muted` substitution, no desaturation). Legibility on a
    /// near-white background is achieved entirely by re-tuning the SURROUNDING
    /// contrast (background near-white, text near-black, chrome/border
    /// darkened) — never by changing the accent value itself.
    ///
    /// The `orange*` family and the status palette (`success`/`warning`/
    /// `error`) carry the same brand values as `hearth()`; only the
    /// structural chrome (`bg`/`surface*`/`border`/`text*`) and the two
    /// markdown roles (`heading`/`link`) are darkened for white-background
    /// contrast.
    pub fn hearth_light() -> Self {
        Self {
            // Brand accent family — PINNED, identical to `hearth()`.
            orange: Color::Rgb(0xff, 0x6b, 0x35),
            orange_hover: Color::Rgb(0xff, 0x82, 0x55),
            orange_muted: Color::Rgb(0xcc, 0x55, 0x29),
            orange_light: Color::Rgb(0xff, 0xb3, 0x99),
            // Structural chrome re-tuned for a light background.
            bg: Color::Rgb(0xfa, 0xfa, 0xfa),
            surface: Color::Rgb(0xf0, 0xf0, 0xf0),
            surface_elevated: Color::Rgb(0xe8, 0xe8, 0xe8),
            surface_hover: Color::Rgb(0xe0, 0xe0, 0xe0),
            border: Color::Rgb(0xcc, 0xcc, 0xcc),
            // Foreground copy — near-black for contrast on white.
            text: Color::Rgb(0x1a, 0x1a, 0x1a),
            text_dim: Color::Rgb(0x55, 0x55, 0x55),
            text_muted: Color::Rgb(0x88, 0x88, 0x88),
            text_running: Color::Rgb(0x58, 0x58, 0x58),
            // Status palette — same brand values as `hearth()`.
            success: Color::Rgb(0x34, 0xd3, 0x99),
            warning: Color::Rgb(0xfb, 0xbf, 0x24),
            error: Color::Rgb(0xf8, 0x71, 0x71),
            // Darker teal — the dark-mode `#6cc9d0` is illegible on white.
            heading: Color::Rgb(0x16, 0x7a, 0x82),
            // Darker blue — the dark-mode `#7aa2f7` washes out on white.
            link: Color::Rgb(0x2a, 0x52, 0xc7),
        }
    }

    /// The light Hearth Palette mapped to the nearest xterm 256-colour
    /// indices — the light-mode counterpart of `hearth_256()` for terminals
    /// that report a light background but lack truecolor.
    ///
    /// The accent indices are identical to `hearth_256()` (the accent is
    /// pinned); only the chrome/text/heading/link indices shift toward the
    /// light end of the greyscale ramp.
    pub fn hearth_light_256() -> Self {
        Self {
            // Brand accent family — identical indices to `hearth_256()`.
            orange: Color::Indexed(202),
            orange_hover: Color::Indexed(209),
            orange_muted: Color::Indexed(166),
            orange_light: Color::Indexed(216),
            // #fafafa ≈ xterm 231 (near-white)
            bg: Color::Indexed(231),
            // #f0f0f0 ≈ xterm 255 (Grey93)
            surface: Color::Indexed(255),
            // #e8e8e8 ≈ xterm 254 (Grey89)
            surface_elevated: Color::Indexed(254),
            // #e0e0e0 ≈ xterm 253 (Grey85)
            surface_hover: Color::Indexed(253),
            // #cccccc ≈ xterm 252 (Grey82)
            border: Color::Indexed(252),
            // #1a1a1a ≈ xterm 234 (Grey11 — near-black)
            text: Color::Indexed(234),
            // #555555 ≈ xterm 240 (Grey35)
            text_dim: Color::Indexed(240),
            // #888888 ≈ xterm 245 (Grey50)
            text_muted: Color::Indexed(245),
            // #585858 ≈ xterm 240 (Grey35) — running glyph grey (light)
            text_running: Color::Indexed(240),
            // Status palette — same indices as `hearth_256()`.
            success: Color::Indexed(79),
            warning: Color::Indexed(220),
            error: Color::Indexed(210),
            // #167a82 ≈ xterm 30 (DarkCyan — legible on white)
            heading: Color::Indexed(30),
            // #2a52c7 ≈ xterm 26 (DodgerBlue3 — legible on white)
            link: Color::Indexed(26),
        }
    }

    /// True when the terminal advertises truecolor support. Checks
    /// `$COLORTERM` for the standard `truecolor` / `24bit` values, then
    /// falls back to `$TERM_PROGRAM` (iTerm.app, WezTerm, Ghostty) and
    /// the `$TERM` suffix `-direct`. Returns `false` when the terminal
    /// does not advertise truecolor or the environment is unclear.
    fn terminal_has_truecolor() -> bool {
        // The canonical check: COLORTERM=truecolor or COLORTERM=24bit.
        if let Some(v) = std::env::var_os("COLORTERM") {
            let lower = v.to_string_lossy().to_lowercase();
            if lower == "truecolor" || lower == "24bit" {
                return true;
            }
        }
        // Well-known truecolor emulators that set TERM_PROGRAM.
        if let Some(v) = std::env::var_os("TERM_PROGRAM") {
            let prog = v.to_string_lossy().to_lowercase();
            if ["iterm.app", "wezterm", "ghostty", "alacritty", "kitty"]
                .iter()
                .any(|p| prog.contains(p))
            {
                return true;
            }
        }
        // $TERM ending in "-direct" is another truecolor signal
        // (used by some terminal multiplexers in pass-through mode).
        if let Some(v) = std::env::var_os("TERM")
            && v.to_string_lossy().ends_with("-direct")
        {
            return true;
        }
        false
    }

    /// Pick the theme for the current environment:
    /// - `no_color()` when `NO_COLOR` is set and non-empty
    ///   ([no-color.org] convention).
    /// - `hearth()` (24-bit Rgb) when the terminal advertises truecolor
    ///   via `$COLORTERM` / `$TERM_PROGRAM` / `$TERM`.
    /// - `hearth_256()` (xterm 256-colour indices) otherwise — a graceful
    ///   fallback that preserves the brand intent on stock Linux consoles
    ///   and older SSH sessions without truecolor (F-058).
    ///
    /// [no-color.org]: https://no-color.org/
    pub fn detect() -> Self {
        match std::env::var_os("NO_COLOR") {
            Some(v) if !v.is_empty() => Self::no_color(),
            _ => {
                if Self::terminal_has_truecolor() {
                    Self::hearth()
                } else {
                    Self::hearth_256()
                }
            }
        }
    }

    /// The light counterpart of [`detect`] (W8 / §5): honors `NO_COLOR`
    /// first, then picks the truecolor or 256-colour LIGHT palette by the
    /// same capability branch. The accent is `#ff6b35` in both depths.
    fn detect_light() -> Self {
        match std::env::var_os("NO_COLOR") {
            Some(v) if !v.is_empty() => Self::no_color(),
            _ => {
                if Self::terminal_has_truecolor() {
                    Self::hearth_light()
                } else {
                    Self::hearth_light_256()
                }
            }
        }
    }

    /// Resolve the live [`Theme`] for a [`ThemeMode`] (W8 / §5 / Q1).
    ///
    /// - `Dark` → [`detect`] (the existing dark path; preserves the
    ///   `NO_COLOR` and truecolor branches).
    /// - `Light` → [`detect_light`] (the light path; same `NO_COLOR` /
    ///   truecolor branches, accent pinned to `#ff6b35`).
    /// - `Auto` → light when [`theme_detect::detect_light_mode`] reports a
    ///   light terminal background, else dark — still honoring `NO_COLOR`
    ///   and the truecolor capability branch through the two helpers above.
    ///
    /// This is the single entry point the router calls to (re-)resolve the
    /// theme when `/theme <mode>` runs; the resolved `Theme` is what every
    /// surface and widget paints against.
    pub fn for_mode(mode: ThemeMode) -> Self {
        match mode {
            ThemeMode::Dark => Self::detect(),
            ThemeMode::Light => Self::detect_light(),
            ThemeMode::Auto => {
                if theme_detect::detect_light_mode() {
                    Self::detect_light()
                } else {
                    Self::detect()
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    /// Serializes the two tests that mutate process-global env vars
    /// (`NO_COLOR` / `COLORTERM` / `COLORFGBG` / `TERM_PROGRAM`). Rust runs
    /// tests in parallel by default; without this lock `for_mode_resolves_*`
    /// and `detect_honors_the_no_color_env_var` could clobber each other's
    /// env state mid-assertion. Each holds the guard for its whole body.
    static ENV_LOCK: Mutex<()> = Mutex::new(());

    #[test]
    fn hearth_uses_the_brand_accent_and_chrome_tokens() {
        let t = Theme::hearth();
        // The single brand accent (brand §07: one accent only).
        assert_eq!(t.orange, Color::Rgb(0xff, 0x6b, 0x35));
        assert_eq!(t.orange_light, Color::Rgb(0xff, 0xb3, 0x99));
        // Structural chrome.
        assert_eq!(t.bg, Color::Rgb(0x0d, 0x0d, 0x0d));
        assert_eq!(t.surface, Color::Rgb(0x14, 0x14, 0x14));
        assert_eq!(t.border, Color::Rgb(0x33, 0x33, 0x33));
        // Status palette.
        assert_eq!(t.success, Color::Rgb(0x34, 0xd3, 0x99));
        assert_eq!(t.warning, Color::Rgb(0xfb, 0xbf, 0x24));
        assert_eq!(t.error, Color::Rgb(0xf8, 0x71, 0x71));
    }

    #[test]
    fn light_theme_keeps_brand_accent_and_is_legible_on_white() {
        let t = Theme::hearth_light();
        // The accent is PINNED — identical to dark mode, no muting (audit MED).
        assert_eq!(t.orange, Color::Rgb(0xff, 0x6b, 0x35));
        assert_eq!(t.orange, Theme::hearth().orange);
        // bg is near-white, text near-black (legibility via surrounding
        // contrast, never by changing the accent).
        assert_eq!(t.bg, Color::Rgb(0xfa, 0xfa, 0xfa));
        if let Color::Rgb(r, _, _) = t.text {
            assert!(r < 0x40, "light text must be dark for contrast on white");
        } else {
            panic!("light text must be rgb");
        }
        // The full status palette is the same brand values as dark mode.
        assert_eq!(t.success, Theme::hearth().success);
        assert_eq!(t.warning, Theme::hearth().warning);
        assert_eq!(t.error, Theme::hearth().error);
    }

    #[test]
    fn light_256_pins_the_accent_index_and_lightens_chrome() {
        let t = Theme::hearth_light_256();
        // Accent index is pinned — identical to the dark 256 palette.
        assert_eq!(t.orange, Color::Indexed(202));
        assert_eq!(t.orange, Theme::hearth_256().orange);
        // bg is near-white on the 256 ramp; text near-black.
        assert_eq!(t.bg, Color::Indexed(231));
        assert_eq!(t.text, Color::Indexed(234));
    }

    #[test]
    fn no_color_theme_is_entirely_uncolored() {
        let t = Theme::no_color();
        // Every field must resolve to the terminal default — no Rgb leaks
        // through, or NO_COLOR would not be honored.
        for c in [
            t.orange,
            t.orange_hover,
            t.orange_muted,
            t.orange_light,
            t.bg,
            t.surface,
            t.surface_elevated,
            t.surface_hover,
            t.border,
            t.text,
            t.text_dim,
            t.text_muted,
            t.success,
            t.warning,
            t.error,
            t.heading,
            t.link,
        ] {
            assert_eq!(c, Color::Reset, "every no_color field must be Reset");
        }
    }

    #[test]
    fn hearth_and_no_color_are_distinct() {
        // A sanity check that the themed palette is not accidentally the
        // uncolored one — the orange accent must differ.
        assert_ne!(Theme::hearth().orange, Theme::no_color().orange);
    }

    #[test]
    fn for_mode_resolves_light_dark_auto() {
        let _guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        // `for_mode` branches on truecolor (hearth* vs hearth*_256) and on
        // NO_COLOR, both process-global. Force COLORTERM=truecolor and clear
        // NO_COLOR so the assertions are deterministic regardless of host.
        //
        // SAFETY: single-threaded test body; env mutations are paired and
        // restored at the end. This and `detect_honors_the_no_color_env_var`
        // are the only tests in this module touching these vars.
        let prior_colorterm = std::env::var_os("COLORTERM");
        let prior_no_color = std::env::var_os("NO_COLOR");
        let prior_fgbg = std::env::var_os("COLORFGBG");
        let prior_tp = std::env::var_os("TERM_PROGRAM");
        unsafe { std::env::set_var("COLORTERM", "truecolor") };
        unsafe { std::env::remove_var("NO_COLOR") };

        // Dark and Light resolve to the matching palette's bg.
        assert_eq!(Theme::for_mode(ThemeMode::Dark).bg, Theme::hearth().bg);
        assert_eq!(
            Theme::for_mode(ThemeMode::Light).bg,
            Theme::hearth_light().bg
        );
        // The accent is `#ff6b35` in BOTH modes (audit MED gate).
        assert_eq!(
            Theme::for_mode(ThemeMode::Light).orange,
            Color::Rgb(0xff, 0x6b, 0x35)
        );
        assert_eq!(
            Theme::for_mode(ThemeMode::Dark).orange,
            Color::Rgb(0xff, 0x6b, 0x35)
        );

        // Auto under a light COLORFGBG resolves to the light palette.
        unsafe { std::env::set_var("COLORFGBG", "0;15") };
        unsafe { std::env::remove_var("TERM_PROGRAM") };
        assert_eq!(
            Theme::for_mode(ThemeMode::Auto).bg,
            Theme::hearth_light().bg,
            "Auto under COLORFGBG=0;15 must be light"
        );
        // Auto under a dark COLORFGBG resolves to the dark palette.
        unsafe { std::env::set_var("COLORFGBG", "15;0") };
        assert_eq!(
            Theme::for_mode(ThemeMode::Auto).bg,
            Theme::hearth().bg,
            "Auto under COLORFGBG=15;0 must be dark"
        );

        // The default ThemeMode is Dark.
        assert_eq!(ThemeMode::default(), ThemeMode::Dark);

        unsafe {
            match prior_colorterm {
                Some(v) => std::env::set_var("COLORTERM", v),
                None => std::env::remove_var("COLORTERM"),
            }
            match prior_no_color {
                Some(v) => std::env::set_var("NO_COLOR", v),
                None => std::env::remove_var("NO_COLOR"),
            }
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

    #[test]
    fn detect_honors_the_no_color_env_var() {
        // `detect()` is the single entry point the live TUI must call so
        // `NO_COLOR` is respected at runtime. With the var set to a
        // non-empty value every field resolves to the terminal default;
        // unset (or empty) it returns the themed palette.
        //
        // `detect()` ALSO branches on truecolor capability (`hearth()` vs
        // `hearth_256()`), which depends on `COLORTERM`/`TERM_PROGRAM`/`TERM`.
        // CI runners (notably GitHub macOS) don't set those, so we force
        // `COLORTERM=truecolor` for the fall-through assertions to be
        // deterministic regardless of host environment.
        //
        // SAFETY: `set_var`/`remove_var` are process-global. The `ENV_LOCK`
        // guard serializes this test against `for_mode_resolves_light_dark_auto`
        // (the only other env-mutating test in this module), so there is no
        // concurrent reader/writer racing inside this binary.
        let _guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let prior_colorterm = std::env::var_os("COLORTERM");
        unsafe { std::env::set_var("COLORTERM", "truecolor") };

        unsafe { std::env::set_var("NO_COLOR", "1") };
        assert_eq!(
            Theme::detect().orange,
            Color::Reset,
            "NO_COLOR set must yield the uncolored theme"
        );
        // An empty value is the no-color.org convention for "unset".
        unsafe { std::env::set_var("NO_COLOR", "") };
        assert_eq!(
            Theme::detect().orange,
            Theme::hearth().orange,
            "an empty NO_COLOR must fall through to the themed palette"
        );
        unsafe { std::env::remove_var("NO_COLOR") };
        assert_eq!(
            Theme::detect().orange,
            Theme::hearth().orange,
            "an unset NO_COLOR must yield the themed palette"
        );

        unsafe {
            match prior_colorterm {
                Some(v) => std::env::set_var("COLORTERM", v),
                None => std::env::remove_var("COLORTERM"),
            }
        }
    }
}
