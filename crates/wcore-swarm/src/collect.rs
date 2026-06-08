//! ResultCollector — finalize worker handles into wire-friendly results.
//!
//! `dispatch` already awaits every worker future and returns
//! `Vec<WorkerHandle>`. `collect` is therefore a synchronous transform
//! today, but is async on the locked surface to leave room for future
//! aggregation steps (e.g. tee-ing worker stdout to a SpanSink) without
//! breaking M5.6/M5.7 callers.

use crate::error::Result;
use crate::{SwarmResult, WorkerHandle};

pub struct ResultCollector;

impl ResultCollector {
    pub fn finalize(handles: Vec<WorkerHandle>) -> Result<Vec<SwarmResult>> {
        Ok(handles.into_iter().map(WorkerHandle::into_result).collect())
    }
}
