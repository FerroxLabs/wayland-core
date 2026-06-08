use super::{SlashError, SlashHandler, SlashInvocation, SlashOutcome};

#[derive(Debug)]
pub struct StyleHandler;

impl SlashHandler for StyleHandler {
    fn name(&self) -> &str {
        "style"
    }
    fn one_line_help(&self) -> &str {
        "Set the response style (terse / playful / focused / match-me)."
    }
    fn invoke(&self, invocation: &SlashInvocation) -> Result<SlashOutcome, SlashError> {
        let Some(arg) = invocation.args.first() else {
            return Ok(SlashOutcome::Handled {
                output: Some(
                    "style options: terse | playful | focused | match-me\n\
                     usage: /style <option>"
                        .to_string(),
                ),
            });
        };
        match arg.as_str() {
            "terse" | "playful" | "focused" | "match-me" => {
                Ok(SlashOutcome::SetStyle(format!("Respond in a {arg} style.")))
            }
            other => Err(SlashError::Bad(format!(
                "unknown style '{other}'. Options: terse | playful | focused | match-me"
            ))),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::slash::parse;

    #[test]
    fn accepts_valid_styles() {
        for style in &["terse", "playful", "focused", "match-me"] {
            let inv = parse(&format!("/style {style}")).unwrap();
            let out = StyleHandler.invoke(&inv).unwrap();
            let SlashOutcome::SetStyle(directive) = out else {
                panic!("expected SetStyle, got {out:?}");
            };
            assert!(directive.contains(style), "got: {directive}");
        }
    }

    #[test]
    fn no_arg_shows_options() {
        let inv = parse("/style").unwrap();
        let out = StyleHandler.invoke(&inv).unwrap();
        let SlashOutcome::Handled { output: Some(s) } = out else {
            panic!();
        };
        assert!(s.contains("terse"));
    }

    #[test]
    fn rejects_unknown() {
        let inv = parse("/style chaotic").unwrap();
        assert!(matches!(StyleHandler.invoke(&inv), Err(SlashError::Bad(_))));
    }
}
