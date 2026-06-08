//! M5.2 — side-by-side trace diff.
//!
//! [`Differ::compare`] walks two traces in parallel and emits a
//! [`DiffEntry`] per position. [`Differ::first_divergence`] returns the
//! first non-[`DiffKind::Unchanged`] entry, which is the load-bearing
//! debugging primitive ("where did the two traces start to disagree?").
//!
//! Relies only on structural [`PartialEq`] of [`TraceEvent`], not [`Eq`],
//! so it composes cleanly with `serde_json::Value` payloads inside
//! `TraceEvent::ToolCall`.

use crate::trace::{Trace, TraceEvent};
use serde::Serialize;

#[derive(Debug, Clone, Serialize)]
pub struct DiffEntry {
    pub index: usize,
    pub left: Option<TraceEvent>,
    pub right: Option<TraceEvent>,
    pub kind: DiffKind,
}

#[derive(Debug, Clone, Copy, Serialize, PartialEq, Eq)]
pub enum DiffKind {
    Added,
    Removed,
    Changed,
    Unchanged,
}

pub struct Differ;

impl Differ {
    /// Compare two traces position-by-position. The output length is
    /// `max(left.events.len(), right.events.len())` so missing events on
    /// either side surface as [`DiffKind::Added`] / [`DiffKind::Removed`].
    pub fn compare(left: &Trace, right: &Trace) -> Vec<DiffEntry> {
        let max = left.events.len().max(right.events.len());
        let mut out = Vec::with_capacity(max);
        for i in 0..max {
            let l = left.events.get(i).cloned();
            let r = right.events.get(i).cloned();
            let kind = match (&l, &r) {
                (Some(a), Some(b)) if a == b => DiffKind::Unchanged,
                (Some(_), Some(_)) => DiffKind::Changed,
                (Some(_), None) => DiffKind::Removed,
                (None, Some(_)) => DiffKind::Added,
                // i < max by construction, so at least one of left/right
                // has an event at this index — `(None, None)` is unreachable.
                (None, None) => unreachable!("Differ::compare index past both traces"),
            };
            out.push(DiffEntry {
                index: i,
                left: l,
                right: r,
                kind,
            });
        }
        out
    }

    /// First non-`Unchanged` entry, if any.
    pub fn first_divergence(left: &Trace, right: &Trace) -> Option<DiffEntry> {
        Self::compare(left, right)
            .into_iter()
            .find(|d| d.kind != DiffKind::Unchanged)
    }
}
