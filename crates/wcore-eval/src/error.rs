//! Typed errors for the eval harness. Internal modules return `EvalError`
//! via `thiserror`; the CLI surfaces these to stderr.

use std::path::PathBuf;

#[derive(Debug, thiserror::Error)]
pub enum EvalError {
    #[error("reference corpus is empty (expected exactly 60 cases: 30 good + 30 bad)")]
    CorpusEmpty,

    #[error("reference corpus has wrong shape: {good} good + {bad} bad (expected 30 + 30)")]
    CorpusUnbalanced { good: usize, bad: usize },

    #[error("reference case at {path} is malformed: {reason}")]
    CaseMalformed { path: PathBuf, reason: String },

    #[error("skill body for case {case} not found at {path}")]
    SkillBodyMissing { case: String, path: PathBuf },

    #[error("trace fixture for case {case} not found at {path}")]
    TraceMissing { case: String, path: PathBuf },

    #[error("io error reading {path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },

    #[error("yaml parse error in {path}: {source}")]
    Yaml {
        path: PathBuf,
        #[source]
        source: serde_yaml::Error,
    },

    #[error("json parse error in {path}: {source}")]
    Json {
        path: PathBuf,
        #[source]
        source: serde_json::Error,
    },

    #[error(
        "acceptance gate failed: precision={precision:.3} (>={p_min:.2} required), \
         recall={recall:.3} (>={r_min:.2} required)"
    )]
    GateFailed {
        precision: f64,
        recall: f64,
        p_min: f64,
        r_min: f64,
    },
}
