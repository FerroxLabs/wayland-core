//! Exact-secret redaction before evaluation data leaves the runner.

use std::collections::BTreeSet;

use crate::runner::{Failure, ScenarioResult};

const REDACTED: &str = "[REDACTED]";

#[derive(Clone, Default)]
pub(crate) struct SecretRedactor {
    secrets: Vec<String>,
}

impl SecretRedactor {
    pub(crate) fn from_secret(secret: Option<String>) -> Self {
        Self {
            secrets: secret
                .filter(|value| value.len() >= 8)
                .into_iter()
                .collect(),
        }
    }

    pub(crate) fn text(&self, value: impl Into<String>) -> (String, bool) {
        let mut value = value.into();
        let mut detected = false;
        for secret in &self.secrets {
            if value.contains(secret) {
                value = value.replace(secret, REDACTED);
                detected = true;
            }
        }
        (value, detected)
    }

    pub(crate) fn value(&self, value: &mut serde_json::Value) -> bool {
        match value {
            serde_json::Value::String(text) => {
                let (redacted, detected) = self.text(std::mem::take(text));
                *text = redacted;
                detected
            }
            serde_json::Value::Array(values) => values
                .iter_mut()
                .fold(false, |detected, value| self.value(value) || detected),
            serde_json::Value::Object(values) => {
                let mut detected = false;
                let original = std::mem::take(values);
                for (key, mut value) in original {
                    let (key, key_detected) = self.text(key);
                    detected |= key_detected;
                    detected |= self.value(&mut value);
                    values.insert(key, value);
                }
                detected
            }
            serde_json::Value::Null | serde_json::Value::Bool(_) | serde_json::Value::Number(_) => {
                false
            }
        }
    }

    pub(crate) fn result(&self, mut result: ScenarioResult) -> ScenarioResult {
        let mut sinks = BTreeSet::new();
        redact(&mut result.name, self, "name", &mut sinks);
        redact(&mut result.final_text, self, "stdout", &mut sinks);
        redact(&mut result.stderr_tail, self, "stderr", &mut sinks);
        for turn in &mut result.turn_results {
            redact(&mut turn.prompt, self, "prompt", &mut sinks);
            redact(&mut turn.assistant_text, self, "stdout", &mut sinks);
        }
        for entry in &mut result.trace.entries {
            redact(&mut entry.call_id, self, "trace.call_id", &mut sinks);
            redact(&mut entry.tool_name, self, "trace.tool_name", &mut sinks);
            redact(&mut entry.input, self, "trace.input", &mut sinks);
            redact(&mut entry.output, self, "trace.output", &mut sinks);
        }
        for info in &mut result.info_events {
            redact(info, self, "info", &mut sinks);
        }
        for failure in &mut result.failures {
            redact_failure(failure, self, &mut sinks);
        }
        for sink in sinks {
            result.failures.push(Failure::SecretDetected { sink });
        }
        result.passed = result.failures.is_empty();
        result
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn redacts_nested_protocol_values_at_capture_time() {
        const SECRET: &str = "secret-canary-123";
        let redactor = SecretRedactor::from_secret(Some(SECRET.to_string()));
        let mut value = serde_json::json!({
            "message": SECRET,
            "nested": ["safe", format!("prefix-{SECRET}-suffix")],
        });
        value
            .as_object_mut()
            .expect("test object")
            .insert(format!("key-{SECRET}"), serde_json::json!("value"));

        assert!(redactor.value(&mut value));
        let retained = value.to_string();
        assert!(!retained.contains(SECRET));
        assert!(retained.contains(REDACTED));
    }
}

fn redact(
    value: &mut String,
    redactor: &SecretRedactor,
    sink: &'static str,
    sinks: &mut BTreeSet<String>,
) {
    let (redacted, detected) = redactor.text(std::mem::take(value));
    *value = redacted;
    if detected {
        sinks.insert(sink.to_string());
    }
}

fn redact_failure(failure: &mut Failure, redactor: &SecretRedactor, sinks: &mut BTreeSet<String>) {
    match failure {
        Failure::Crashed { stderr_tail, .. } | Failure::Hung { stderr_tail } => {
            redact(stderr_tail, redactor, "failure.stderr", sinks);
        }
        Failure::ExpectedToolMissing(value)
        | Failure::ForbiddenToolUsed(value)
        | Failure::SessionBrick { error: value }
        | Failure::RunnerError(value) => redact(value, redactor, "failure", sinks),
        Failure::AssertionFailed {
            assertion,
            observed,
        }
        | Failure::TraceFailed {
            assertion,
            observed,
        } => {
            redact(assertion, redactor, "failure.assertion", sinks);
            redact(observed, redactor, "failure.observed", sinks);
        }
        Failure::SkippedInStrict { missing_key } => {
            redact(missing_key, redactor, "failure", sinks);
        }
        Failure::OverTime { .. }
        | Failure::OverCost { .. }
        | Failure::CostMissing
        | Failure::StepsExceeded { .. }
        | Failure::SecretDetected { .. } => {}
    }
}
