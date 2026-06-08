//! Console + machine-readable reports — plan §6.
//!
//! **T5 implements**; T1/T2 declares the type surface.

use crate::runner::ScenarioResult;

/// One run's summary — produced by the runner after every scenario,
/// rendered to stdout + JSON + Markdown by [`render_console`] /
/// [`render_json`] / [`render_markdown`] in T5.
#[derive(Debug, Clone, Default)]
pub struct Report {
    pub results: Vec<ScenarioResult>,
    pub total_cost_usd: f64,
    pub wall_time_secs: f64,
}

impl Report {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn push(&mut self, r: ScenarioResult) {
        self.total_cost_usd += r.cost_usd;
        self.wall_time_secs += r.wall_time.as_secs_f64();
        self.results.push(r);
    }

    pub fn passed(&self) -> usize {
        self.results.iter().filter(|r| r.passed).count()
    }
    pub fn failed(&self) -> usize {
        self.results.iter().filter(|r| !r.passed).count()
    }
}

/// **T5** — color-aware console rendering of a finished [`Report`].
///
/// **Not yet implemented (T5 wave).** Returns an explicit
/// "not implemented" marker string so the gate compiles and any caller
/// surfaces the unimplemented state visibly instead of panicking.
pub fn render_console(_r: &Report) -> String {
    String::from("[wcore-eval-scenarios] render_console: not implemented (T5 wave pending)")
}

/// **T5** — REPORT.md including verbatim prompt, final assistant text,
/// trace, stderr tail per M-10.
///
/// **Not yet implemented (T5 wave).** Returns an explicit markdown
/// "not implemented" marker so the gate compiles and any caller
/// surfaces the unimplemented state visibly instead of panicking.
pub fn render_markdown(_r: &Report) -> String {
    String::from(
        "# wcore-eval-scenarios report\n\n\
         _render_markdown is not implemented (T5 wave pending)._\n",
    )
}

/// **T5** — REPORT.json for diffing across runs.
///
/// **Not yet implemented (T5 wave).** Returns an explicit JSON object
/// with `"not_implemented": true` so the gate compiles and any caller
/// can detect the unimplemented state programmatically instead of
/// panicking.
pub fn render_json(_r: &Report) -> serde_json::Value {
    serde_json::json!({
        "not_implemented": true,
        "wave": "T5",
        "note": "wcore-eval-scenarios::report::render_json — T5 wave pending",
    })
}
