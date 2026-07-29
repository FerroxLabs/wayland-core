//! `/usermodel` — see and control what the agent believes about you.
//!
//! 23B-C3, user-model half of criterion 3.
//!
//! # Why this is not a `/memory` sub-action
//!
//! `/memory` acts on the `wcore-memory` partitions. The user model that
//! actually reaches the provider is a different thing entirely: it is
//! `wcore-user-model`'s brief + preferences, rendered into the system prompt by
//! `crate::user_context`. The `wcore-memory` P5 `user_model` k/v partition —
//! the one `MemoryApi::update_user_model` writes — has **no** production read
//! into any prompt; it is a display surface only. Putting these controls under
//! `/memory` would have implied they act on the partition `/memory show`
//! prints, which they do not.
//!
//! # No stub variant
//!
//! Unlike `MemoryHandler`, this handler has no `Stub`. It is registered only
//! when a real `CorrectionStore` was opened, so the command either works or is
//! not present. A stub would advertise a control that stores nothing.

use std::sync::Arc;

use wcore_user_model::{CorrectionStore, UserCorrection};

use super::{SlashError, SlashHandler, SlashInvocation, SlashOutcome};

/// `/usermodel` handler. Carries the same `CorrectionStore` and `user_id` the
/// bootstrap render site read from, so a correction made here is the one the
/// next session's prompt is built from.
#[derive(Clone)]
pub struct UserModelHandler {
    store: CorrectionStore,
    user_id: Arc<str>,
}

impl std::fmt::Debug for UserModelHandler {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("UserModelHandler")
            .field("user_id", &self.user_id)
            .finish_non_exhaustive()
    }
}

impl UserModelHandler {
    pub fn new(store: CorrectionStore, user_id: impl AsRef<str>) -> Self {
        Self {
            store,
            user_id: Arc::from(user_id.as_ref()),
        }
    }
}

impl SlashHandler for UserModelHandler {
    fn name(&self) -> &str {
        "usermodel"
    }

    fn one_line_help(&self) -> &str {
        "See and correct what the agent believes about you."
    }

    fn invoke(&self, invocation: &SlashInvocation) -> Result<SlashOutcome, SlashError> {
        match invocation.args.split_first() {
            None => self.show(),
            Some((first, rest)) => match first.as_str() {
                "show" | "why" => self.show(),
                "correct" | "set" => self.correct(rest),
                "forget" | "unset" => self.forget(rest),
                other => Err(SlashError::Bad(format!(
                    "/usermodel: unknown sub-action '{other}'. Try: /usermodel show | \
                     /usermodel correct <key> <value> | /usermodel forget <key>"
                ))),
            },
        }
    }
}

impl UserModelHandler {
    fn show(&self) -> Result<SlashOutcome, SlashError> {
        let corrections = block_on(self.store.corrections(&self.user_id));
        let mut out = String::new();
        if corrections.is_empty() {
            out.push_str(
                "/usermodel: you have corrected nothing. The agent is running purely on \
                 what it inferred from your behaviour.\n\
                 Correct it with: /usermodel correct <key> <value>   \
                 (e.g. /usermodel correct expertise.rust expert)\n",
            );
            return Ok(handled(out));
        }
        out.push_str(&format!(
            "/usermodel: {} correction(s), all of which override what the agent inferred:\n",
            corrections.len()
        ));
        for c in corrections.iter() {
            out.push_str(&format!("  - {} = {}{}\n", c.key, c.value, age(c)));
        }
        out.push_str(
            "These reach the model in the system prompt, and the inferred value each one \
             replaces is withheld.\n",
        );
        Ok(handled(out))
    }

    fn correct(&self, args: &[String]) -> Result<SlashOutcome, SlashError> {
        let Some((key, value_parts)) = args.split_first() else {
            return Err(SlashError::Bad(
                "/usermodel correct <key> <value> — e.g. \
                 `/usermodel correct expertise.rust expert`, \
                 `/usermodel correct style \"blunt, no preamble\"`"
                    .to_string(),
            ));
        };
        if value_parts.is_empty() {
            return Err(SlashError::Bad(format!(
                "/usermodel correct {key} <value> — a correction needs a value. \
                 To remove one, use `/usermodel forget {key}`."
            )));
        }
        let value = value_parts.join(" ");
        let ts = now_secs();
        match block_on(self.store.correct(&self.user_id, key, &value, ts)) {
            // A store failure must reach the user. Reporting "ok" for a
            // correction that never hit disk is the exact defect this whole
            // layer exists to prevent — it would not survive the session end
            // it is supposed to survive.
            Err(e) => Ok(handled(format!(
                "/usermodel correct: FAILED, nothing was stored: {e}\n"
            ))),
            Ok(previous) => {
                let key = wcore_user_model::correction::normalise_key(key);
                let mut out = match previous {
                    Some(UserCorrection { value: old, .. }) => {
                        format!("/usermodel correct: {key} = {value}  (was: {old})\n")
                    }
                    None => format!("/usermodel correct: {key} = {value}\n"),
                };
                // Say plainly when it takes effect. The user-context block is
                // assembled once at bootstrap, so a correction made mid-session
                // does not rewrite the prompt already in flight. Implying
                // otherwise would be a control that reports more than it did.
                out.push_str(
                    "Stored. It overrides the agent's inferred value from your next \
                     session onward — the current session's system prompt was already \
                     built.\n",
                );
                Ok(handled(out))
            }
        }
    }

    fn forget(&self, args: &[String]) -> Result<SlashOutcome, SlashError> {
        let Some(key) = args.first() else {
            return Err(SlashError::Bad(
                "/usermodel forget <key> — drops one of YOUR corrections, returning that \
                 subject to whatever the agent infers. It does not erase the inference."
                    .to_string(),
            ));
        };
        match block_on(self.store.forget(&self.user_id, key)) {
            Err(e) => Ok(handled(format!(
                "/usermodel forget: FAILED, nothing was removed: {e}\n"
            ))),
            // A miss is reported as a miss. `/memory`'s doctrine, and the
            // right one: a user who mistypes a key and is told "ok" believes
            // a correction is gone when it is still in the prompt.
            Ok(None) => Ok(handled(format!(
                "/usermodel forget: no correction named '{}' — nothing removed.\n",
                wcore_user_model::correction::normalise_key(key)
            ))),
            Ok(Some(removed)) => Ok(handled(format!(
                "/usermodel forget: dropped {} (was: {}). That subject returns to whatever \
                 the agent infers, from your next session onward.\n",
                removed.key, removed.value
            ))),
        }
    }
}

fn age(c: &UserCorrection) -> String {
    if c.ts_secs <= 0 {
        return String::new();
    }
    let delta = now_secs().saturating_sub(c.ts_secs);
    if delta < 0 {
        return String::new();
    }
    let days = delta / 86_400;
    if days > 0 {
        format!("   (set {days}d ago)")
    } else {
        format!("   (set {}h ago)", delta / 3_600)
    }
}

fn now_secs() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

fn handled(output: String) -> SlashOutcome {
    SlashOutcome::Handled {
        output: Some(output),
    }
}

// `SlashHandler::invoke` is synchronous and `CorrectionStore` is async.
// Reuse `slash::memory`'s runtime-flavour-aware helper rather than writing a
// second one: the naive version panics on a current-thread runtime, which is
// exactly the shape the tests below run under.
use super::memory::block_on;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::slash::parse;

    fn handler() -> UserModelHandler {
        UserModelHandler::new(CorrectionStore::in_memory(), "default")
    }

    fn run(h: &UserModelHandler, line: &str) -> String {
        let inv = parse(line).expect("parsed");
        match h.invoke(&inv).expect("invoked") {
            SlashOutcome::Handled { output: Some(s) } => s,
            other => panic!("expected Handled output, got {other:?}"),
        }
    }

    #[test]
    fn bare_show_says_nothing_is_corrected_rather_than_printing_an_empty_list() {
        let out = run(&handler(), "/usermodel");
        assert!(out.contains("corrected nothing"), "got: {out}");
        assert!(
            out.contains("/usermodel correct"),
            "must say how; got: {out}"
        );
    }

    #[test]
    fn correct_then_show_round_trips_and_reports_the_previous_value() {
        let h = handler();
        let first = run(&h, "/usermodel correct expertise.rust expert");
        assert!(first.contains("expertise.rust = expert"), "got: {first}");
        assert!(
            first.contains("next session"),
            "must say when it takes effect; got: {first}"
        );

        let second = run(&h, "/usermodel correct Expertise.Rust novice");
        assert!(
            second.contains("was: expert"),
            "must report what actually changed, not echo the request; got: {second}"
        );

        let shown = run(&h, "/usermodel show");
        assert!(shown.contains("expertise.rust = novice"), "got: {shown}");
        assert!(
            !shown.contains("expert\n"),
            "the superseded value must be gone; got: {shown}"
        );
    }

    #[test]
    fn multi_word_values_survive_tokenisation() {
        let h = handler();
        run(&h, "/usermodel correct style blunt, no preamble");
        let shown = run(&h, "/usermodel show");
        assert!(
            shown.contains("style = blunt, no preamble"),
            "a value must not be truncated at the first space; got: {shown}"
        );
    }

    #[test]
    fn forget_reports_a_miss_as_a_miss() {
        let h = handler();
        let out = run(&h, "/usermodel forget nosuchkey");
        assert!(
            out.contains("nothing removed"),
            "a miss must never read as success; got: {out}"
        );
    }

    #[test]
    fn forget_removes_only_the_named_correction() {
        let h = handler();
        run(&h, "/usermodel correct name Sean");
        run(&h, "/usermodel correct expertise.rust expert");
        let out = run(&h, "/usermodel forget NAME");
        assert!(out.contains("dropped name"), "got: {out}");
        let shown = run(&h, "/usermodel show");
        assert!(!shown.contains("name = Sean"), "got: {shown}");
        assert!(shown.contains("expertise.rust = expert"), "got: {shown}");
    }

    #[test]
    fn correct_without_a_value_is_refused_rather_than_storing_blank() {
        let h = handler();
        let inv = parse("/usermodel correct expertise.rust").unwrap();
        assert!(
            h.invoke(&inv).is_err(),
            "a keyless/valueless correction must be refused"
        );
        let shown = run(&h, "/usermodel show");
        assert!(shown.contains("corrected nothing"), "got: {shown}");
    }

    #[test]
    fn unknown_subaction_lists_the_real_ones() {
        let h = handler();
        let inv = parse("/usermodel wibble").unwrap();
        let err = h.invoke(&inv).unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("correct"), "got: {msg}");
        assert!(msg.contains("forget"), "got: {msg}");
    }
}
