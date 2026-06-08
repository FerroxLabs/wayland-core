use super::{SlashError, SlashHandler, SlashInvocation, SlashOutcome};

#[derive(Debug)]
pub struct HelpHandler;

impl SlashHandler for HelpHandler {
    fn name(&self) -> &str {
        "help"
    }
    fn one_line_help(&self) -> &str {
        "Show this help."
    }
    fn invoke(&self, _: &SlashInvocation) -> Result<SlashOutcome, SlashError> {
        // The dispatcher knows the registered commands; the help handler
        // returns a generic banner and a hint to use `--help` for full
        // CLI documentation. The TUI layer (3.C.4) overlays the dispatcher's
        // help_lines() output above this message.
        let body = "\
Available slash commands:
  /help              Show this help.
  /agent             List or switch agents.
  /style             Set the response style.
  /memory            Inspect or clear memory.
  /plugin            List / install / remove plugins.
  /skill             List / show / run a skill.
  /clear             Clear the screen.
  /exit              Exit the session.

Pass `--help` to wayland-core for full CLI documentation.";
        Ok(SlashOutcome::Handled {
            output: Some(body.to_string()),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::slash::parse;

    #[test]
    fn help_returns_banner() {
        let inv = parse("/help").unwrap();
        let out = HelpHandler.invoke(&inv).unwrap();
        match out {
            SlashOutcome::Handled { output: Some(s) } => {
                assert!(s.contains("/help"));
                assert!(s.contains("/exit"));
            }
            other => panic!("got {other:?}"),
        }
    }
}
