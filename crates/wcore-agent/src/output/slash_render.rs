//! v0.7.0 Task 3.C.4: render slash-command output with a visual border so
//! it is distinguishable from agent output in a normal terminal stream.
//!
//! Behaviour is intentionally simple — no ANSI when `no_color` is set,
//! a Unicode box border otherwise. The `Write` indirection lets tests
//! capture the exact bytes emitted.

use std::io::{self, Write};

use crate::slash::{SlashError, SlashOutcome};

const BORDER_TOP: &str = "┌─ slash ─────────────────────────────────────────────────┐";
const BORDER_MID: &str = "│";
const BORDER_BOT: &str = "└─────────────────────────────────────────────────────────┘";

const PLAIN_TOP: &str = "--- slash ---";
const PLAIN_BOT: &str = "-------------";

#[derive(Debug, Clone, Copy, Default)]
pub struct RenderConfig {
    pub no_color: bool,
}

/// Render a `SlashOutcome` into `w`. Handles all variants:
/// - `Handled { output: Some(s) }`  → boxed
/// - `Handled { output: None }`     → no output (e.g. `/clear`)
/// - `NotImplemented { message }`   → boxed with a tag
/// - `Exit`                         → boxed farewell line; the caller still
///   has to actually stop the loop
pub fn render_outcome<W: Write>(
    w: &mut W,
    outcome: &SlashOutcome,
    cfg: RenderConfig,
) -> io::Result<()> {
    match outcome {
        SlashOutcome::Handled { output: None } => Ok(()),
        SlashOutcome::Handled { output: Some(s) } => render_block(w, s, cfg),
        SlashOutcome::NotImplemented { message } => {
            render_block(w, &format!("[not implemented] {message}"), cfg)
        }
        SlashOutcome::SetStyle(_) => render_block(w, "(style updated)", cfg),
        SlashOutcome::ClearConversation => render_block(w, "(conversation cleared)", cfg),
        SlashOutcome::Exit => render_block(w, "(exiting session)", cfg),
    }
}

pub fn render_error<W: Write>(w: &mut W, err: &SlashError, cfg: RenderConfig) -> io::Result<()> {
    render_block(w, &format!("error: {err}"), cfg)
}

fn render_block<W: Write>(w: &mut W, body: &str, cfg: RenderConfig) -> io::Result<()> {
    if cfg.no_color {
        writeln!(w, "{PLAIN_TOP}")?;
        for line in body.lines() {
            writeln!(w, "{line}")?;
        }
        writeln!(w, "{PLAIN_BOT}")?;
    } else {
        writeln!(w, "{BORDER_TOP}")?;
        for line in body.lines() {
            writeln!(w, "{BORDER_MID} {line}")?;
        }
        writeln!(w, "{BORDER_BOT}")?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn capture<F>(_cfg: RenderConfig, f: F) -> String
    where
        F: FnOnce(&mut Vec<u8>) -> io::Result<()>,
    {
        let mut buf = Vec::new();
        f(&mut buf).expect("render");
        String::from_utf8(buf).expect("utf-8 output")
    }

    #[test]
    fn renders_handled_with_border_default() {
        let out = capture(RenderConfig::default(), |w| {
            render_outcome(
                w,
                &SlashOutcome::Handled {
                    output: Some("hello\nworld".to_string()),
                },
                RenderConfig::default(),
            )
        });
        assert!(out.contains("slash"));
        assert!(out.contains("hello"));
        assert!(out.contains("world"));
        assert!(out.contains('│')); // border
    }

    #[test]
    fn renders_handled_no_color() {
        let cfg = RenderConfig { no_color: true };
        let out = capture(cfg, |w| {
            render_outcome(
                w,
                &SlashOutcome::Handled {
                    output: Some("plain".to_string()),
                },
                cfg,
            )
        });
        assert!(out.contains("--- slash ---"));
        assert!(out.contains("plain"));
        assert!(!out.contains('│'));
    }

    #[test]
    fn handled_with_no_output_emits_nothing() {
        let out = capture(RenderConfig::default(), |w| {
            render_outcome(
                w,
                &SlashOutcome::Handled { output: None },
                RenderConfig::default(),
            )
        });
        assert!(out.is_empty());
    }

    #[test]
    fn renders_not_implemented_tag() {
        let out = capture(RenderConfig::default(), |w| {
            render_outcome(
                w,
                &SlashOutcome::NotImplemented {
                    message: "soon".to_string(),
                },
                RenderConfig::default(),
            )
        });
        assert!(out.contains("[not implemented]"));
        assert!(out.contains("soon"));
    }

    #[test]
    fn renders_exit_farewell() {
        let out = capture(RenderConfig::default(), |w| {
            render_outcome(w, &SlashOutcome::Exit, RenderConfig::default())
        });
        assert!(out.contains("exiting"));
    }

    #[test]
    fn renders_error() {
        let err = SlashError::Unknown("foo".to_string());
        let out = capture(RenderConfig::default(), |w| {
            render_error(w, &err, RenderConfig::default())
        });
        assert!(out.contains("error:"));
        assert!(out.contains("foo"));
    }

    #[test]
    fn boxed_output_uses_one_border_per_input_line() {
        let out = capture(RenderConfig::default(), |w| {
            render_outcome(
                w,
                &SlashOutcome::Handled {
                    output: Some("a\nb\nc".to_string()),
                },
                RenderConfig::default(),
            )
        });
        let border_lines = out.lines().filter(|l| l.starts_with('│')).count();
        assert_eq!(border_lines, 3);
    }
}
