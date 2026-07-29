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

use wcore_user_model::{CorrectionStore, UserCorrection, UserModelBackend};

use super::{SlashError, SlashHandler, SlashInvocation, SlashOutcome};

/// `/usermodel` handler. Carries the same `CorrectionStore` and `user_id` the
/// bootstrap render site read from, so a correction made here is the one the
/// next session's prompt is built from.
///
/// It also carries the `UserModelBackend` so `show` can display what the agent
/// **inferred**, not only what the user corrected. A control that lets you
/// change a belief without letting you read it is not "see and control" — it is
/// asking the user to correct something they were never shown.
#[derive(Clone)]
pub struct UserModelHandler {
    store: CorrectionStore,
    user_id: Arc<str>,
    backend: Option<Arc<dyn UserModelBackend>>,
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
            backend: None,
        }
    }

    /// Attach the inference backend so `show` can display the agent's inferred
    /// beliefs alongside the user's corrections.
    #[must_use]
    pub fn with_backend(mut self, backend: Arc<dyn UserModelBackend>) -> Self {
        self.backend = Some(backend);
        self
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
        let mut out = String::from("/usermodel: what the agent believes about you\n");

        // --- what the agent INFERRED, and whether a correction is overriding
        // it. Each line is marked with its origin so the two are never confused.
        match self.backend.as_ref() {
            None => out.push_str(
                "\n  inferred: (no user-model backend is installed this session — \
                 nothing is being inferred)\n",
            ),
            Some(b) => {
                let brief = block_on(b.brief(&self.user_id)).unwrap_or_default();
                let prefs = block_on(b.preferences(&self.user_id)).unwrap_or_default();
                let mut lines: Vec<String> = Vec::new();
                if let Some(n) = brief.name.as_deref().filter(|n| !n.is_empty()) {
                    lines.push(render_inferred("name", n, corrections.suppresses("name")));
                }
                if !brief.summary.is_empty() {
                    lines.push(render_inferred(
                        "summary",
                        brief.summary.trim(),
                        corrections.suppresses("summary"),
                    ));
                }
                let s = &brief.style;
                if s.formality.abs() > 0.05
                    || s.energy.abs() > 0.05
                    || s.terseness.abs() > 0.05
                    || s.emoji_use.abs() > 0.05
                {
                    lines.push(render_inferred(
                        "style",
                        &format!(
                            "formality={:.2}, energy={:.2}, terseness={:.2}, emoji_use={:.2}",
                            s.formality, s.energy, s.terseness, s.emoji_use
                        ),
                        corrections.suppresses("style"),
                    ));
                }
                for (domain, level) in &prefs.expertise {
                    lines.push(render_inferred(
                        &format!("expertise.{domain}"),
                        &format!("{level:?}"),
                        corrections.suppresses(&format!("expertise.{domain}")),
                    ));
                }
                for (k, v) in &prefs.tags {
                    lines.push(render_inferred(
                        &format!("tags.{k}"),
                        v,
                        corrections.suppresses(&format!("tags.{k}")),
                    ));
                }
                for inf in &brief.dialectic {
                    lines.push(render_inferred(
                        &format!("{}.{}", inf.kind, inf.subject),
                        &format!(
                            "{} (confidence {:.2}, {} observations)",
                            inf.value, inf.confidence, inf.evidence_count
                        ),
                        corrections.suppresses(&format!("dialectic.{}", inf.subject))
                            || corrections.suppresses(&format!("{}.{}", inf.kind, inf.subject)),
                    ));
                }
                if lines.is_empty() {
                    out.push_str(
                        "\n  inferred: nothing yet — the agent has not observed enough \
                         to form a view.\n",
                    );
                } else {
                    out.push_str("\n  INFERRED from your behaviour:\n");
                    for l in lines {
                        out.push_str(&format!("    {l}\n"));
                    }
                }
            }
        }

        // --- what the USER said, which outranks all of the above.
        if corrections.is_empty() {
            out.push_str(
                "\n  YOUR corrections: you have corrected nothing, so the agent is running \
                 purely on what it inferred above.\n\
                 Correct it with: /usermodel correct <key> <value>   \
                 (e.g. /usermodel correct expertise.rust expert)\n",
            );
        } else {
            out.push_str(&format!(
                "\n  YOUR corrections ({}) — these override what the agent inferred:\n",
                corrections.len()
            ));
            for c in corrections.iter() {
                out.push_str(&format!("    - {} = {}{}\n", c.key, c.value, age(c)));
            }
            out.push_str(
                "  These reach the model in the system prompt, and the inferred value each \
                 one replaces is withheld.\n",
            );
        }
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

/// One inferred belief, marked with whether a user correction is currently
/// overriding it. An overridden inference is still shown — hiding it would stop
/// the user seeing what the agent would fall back to if they dropped the
/// correction, which is the question `/usermodel forget` asks.
fn render_inferred(key: &str, value: &str, overridden: bool) -> String {
    if overridden {
        format!("- {key} = {value}   [OVERRIDDEN by your correction — not sent to the model]")
    } else {
        format!("- {key} = {value}   [inferred — sent to the model]")
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

    /// `show` must display what the agent INFERRED, not only what the user
    /// corrected. A control that lets you change a belief without showing it to
    /// you is asking the user to correct something they were never shown.
    #[test]
    fn show_displays_inferred_beliefs_and_marks_which_are_overridden() {
        use wcore_user_model::{LocalBackend, Observation};

        let rt = tokio::runtime::Builder::new_current_thread()
            .build()
            .unwrap();
        let backend = Arc::new(LocalBackend::in_memory());
        rt.block_on(async {
            backend
                .observe(
                    "default",
                    Observation {
                        style_fingerprint: Some([0.8, 0.6, 0.6, 0.2]),
                        ts_secs: 100,
                        ..Observation::default()
                    },
                )
                .await
                .unwrap();
        });
        let h = UserModelHandler::new(CorrectionStore::in_memory(), "default")
            .with_backend(backend as Arc<dyn UserModelBackend>);

        // Before any correction: the inference is shown AND marked as reaching
        // the model.
        let before = run(&h, "/usermodel show");
        assert!(
            before.contains("INFERRED from your behaviour"),
            "the agent's own beliefs must be visible; got: {before}"
        );
        assert!(
            before.contains("- style = formality="),
            "the inferred style must be shown with its values; got: {before}"
        );
        assert!(
            before.contains("[inferred — sent to the model]"),
            "each inferred line must say whether it reaches the model; got: {before}"
        );
        assert!(
            before.contains("you have corrected nothing"),
            "got: {before}"
        );

        // After correcting it: the inference is STILL shown, now marked
        // overridden, so the user can see what dropping the correction restores.
        run(&h, "/usermodel correct style blunt");
        let after = run(&h, "/usermodel show");
        assert!(
            after.contains("[OVERRIDDEN by your correction — not sent to the model]"),
            "an overridden inference must be labelled as not reaching the model; got: {after}"
        );
        assert!(
            after.contains("- style = formality="),
            "an overridden inference must remain VISIBLE — it is what /usermodel forget \
             restores; got: {after}"
        );
        assert!(after.contains("style = blunt"), "got: {after}");
    }

    #[test]
    fn show_without_a_backend_says_so_rather_than_implying_nothing_is_inferred() {
        // No backend installed is a different fact from "nothing inferred yet",
        // and conflating them would tell the user their profile is empty when
        // the truth is that nothing is looking.
        let out = run(&handler(), "/usermodel show");
        assert!(
            out.contains("no user-model backend is installed"),
            "got: {out}"
        );
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
