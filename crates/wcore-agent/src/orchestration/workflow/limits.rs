//! Resource bounds for the Dynamic Workflows engine.
//!
//! A workflow RON document is **attacker-controlled text** — it may be authored
//! by an LLM (and thus hallucinated) or supplied directly by an untrusted host.
//! Without explicit bounds, a malicious or malformed document can drive the
//! engine into unbounded sub-agent dispatch, allocate huge collections (OOM),
//! or overflow the stack at parse time (RON 0.8 has no built-in recursion
//! limit). Every limit below has a single documented `pub const` so the cap is
//! auditable in one place and the same value is enforced at every call site.
//!
//! The bounds, in order of how an attack would hit them:
//!
//! 1. [`MAX_RON_BYTES`] / [`MAX_NESTING_DEPTH`] — guard `ron::from_str` *before*
//!    it runs, so an oversized or deeply-nested document is rejected with a
//!    typed error instead of overflowing the stack (an uncatchable abort).
//! 2. [`MAX_WORKFLOW_NODES`] — caps the lowered graph size so a document that
//!    expands into an enormous node set is rejected.
//! 3. [`MAX_OVER_CARDINALITY`] — caps the runtime `over:` collection a
//!    no-barrier pipeline streams, so a huge injected array cannot allocate one
//!    future per item.
//! 4. [`MAX_TOTAL_DISPATCHES`] — the central backstop: a per-run counter that
//!    increments before *every* sub-agent dispatch (single, fan-out, fleet,
//!    pipeline item, schema retry, loop iteration). It is the only bound robust
//!    to all shapes, including retries and runtime-injected arrays.

use std::sync::atomic::{AtomicUsize, Ordering};

/// Hard cap on the number of sub-agent dispatches a single workflow run may
/// perform across every path (Kahn single + fan-out, fleet fan-out per
/// sub-config, pipeline per-item-per-stage, schema retries, loop iterations).
///
/// This is the central DoS backstop: even if every other bound is somehow
/// circumvented, a run can never dispatch more than this many sub-agents.
pub const MAX_TOTAL_DISPATCHES: usize = 1000;

/// Maximum byte length of a workflow RON document. Rejected *before*
/// `ron::from_str` so a multi-megabyte payload never reaches the parser. 256
/// KiB is far larger than any legitimate hand- or LLM-authored workflow.
pub const MAX_RON_BYTES: usize = 262_144;

/// Maximum bracket/paren/brace nesting depth permitted in a workflow RON
/// document (and in a schema definition body). RON 0.8 recurses without a
/// depth limit, so a deeply-nested document overflows the stack during parse —
/// an uncatchable abort. A cheap byte-scan rejects anything deeper than this
/// before the recursive parser runs.
pub const MAX_NESTING_DEPTH: usize = 64;

/// Maximum number of nodes a workflow may lower to. A document whose phases
/// expand past this is rejected so the graph walk cannot be driven into a
/// pathological size.
pub const MAX_WORKFLOW_NODES: usize = 256;

/// Maximum number of items a no-barrier `over:` collection may contain. The
/// collection is resolved from the *running state* at execution time (so it can
/// be runtime-injected by the caller), and each item seeds an independent
/// future — capping it bounds future allocation before the pipeline builds them.
pub const MAX_OVER_CARDINALITY: usize = 500;

/// Scan `src` for the maximum bracket/paren/brace nesting depth, returning the
/// first depth that exceeds [`MAX_NESTING_DEPTH`] (as `Err(depth)`), else `Ok`.
///
/// This is a cheap structural pre-check, not a parser: it counts `(`, `[`, `{`
/// as descents and `)`, `]`, `}` as ascents, ignoring everything inside string
/// literals (so brackets in a prompt string do not inflate the depth) and
/// honouring RON's `\` escape inside strings. It deliberately does not validate
/// that brackets match — `ron::from_str` does that — it only bounds depth so the
/// recursive parser cannot blow the stack.
pub fn check_nesting_depth(src: &str) -> Result<(), usize> {
    let mut depth: usize = 0;
    let mut in_string = false;
    let mut escaped = false;
    for b in src.bytes() {
        if in_string {
            if escaped {
                escaped = false;
            } else if b == b'\\' {
                escaped = true;
            } else if b == b'"' {
                in_string = false;
            }
            continue;
        }
        match b {
            b'"' => in_string = true,
            b'(' | b'[' | b'{' => {
                depth += 1;
                if depth > MAX_NESTING_DEPTH {
                    return Err(depth);
                }
            }
            b')' | b']' | b'}' => depth = depth.saturating_sub(1),
            _ => {}
        }
    }
    Ok(())
}

/// A per-run sub-agent dispatch budget. Incremented before every dispatch on
/// every path; once the count would exceed [`MAX_TOTAL_DISPATCHES`] the run
/// aborts. Cheap to clone (an `Arc`-free `&` is threaded where possible; the
/// pipeline path needs `Send + Sync` so it holds an [`AtomicUsize`]).
#[derive(Debug, Default)]
pub struct DispatchBudget {
    used: AtomicUsize,
}

impl DispatchBudget {
    /// A fresh budget with zero dispatches used.
    pub fn new() -> Self {
        Self {
            used: AtomicUsize::new(0),
        }
    }

    /// Charge one dispatch. Returns `Ok(())` if the dispatch is within budget,
    /// or `Err(used_after)` — the count that *would* have been reached — if it
    /// would exceed [`MAX_TOTAL_DISPATCHES`]. On `Err` no dispatch should occur.
    pub fn try_charge(&self) -> Result<(), usize> {
        // `fetch_add` returns the prior value; the post-charge count is +1.
        let after = self.used.fetch_add(1, Ordering::SeqCst) + 1;
        if after > MAX_TOTAL_DISPATCHES {
            Err(after)
        } else {
            Ok(())
        }
    }

    /// Charge `n` dispatches at once (a fan-out wave dispatches a whole batch).
    /// Returns `Err(used_after)` if the batch would exceed the budget; on `Err`
    /// none of the batch should be dispatched.
    pub fn try_charge_n(&self, n: usize) -> Result<(), usize> {
        let after = self.used.fetch_add(n, Ordering::SeqCst) + n;
        if after > MAX_TOTAL_DISPATCHES {
            Err(after)
        } else {
            Ok(())
        }
    }

    /// The number of dispatches charged so far (for diagnostics/tests).
    pub fn used(&self) -> usize {
        self.used.load(Ordering::SeqCst)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn depth_scan_accepts_shallow() {
        assert!(check_nesting_depth("Workflow( phases: [ Phase( steps: [] ) ] )").is_ok());
    }

    #[test]
    fn depth_scan_rejects_deep() {
        let deep = "(".repeat(MAX_NESTING_DEPTH + 5);
        match check_nesting_depth(&deep) {
            Err(d) => assert_eq!(d, MAX_NESTING_DEPTH + 1),
            Ok(()) => panic!("expected a depth rejection"),
        }
    }

    #[test]
    fn depth_scan_ignores_brackets_inside_strings() {
        // 100 open-parens, but all inside a single string literal → depth 0.
        let src = format!("Agent(prompt: \"{}\")", "(".repeat(100));
        assert!(check_nesting_depth(&src).is_ok());
    }

    #[test]
    fn depth_scan_honours_string_escape() {
        // An escaped quote does not close the string, so the trailing parens
        // stay inside it and do not count.
        let src = format!("(prompt: \"a\\\"{}\")", "(".repeat(80));
        // One real `(` opens at depth 1, the rest are inside the string.
        assert!(check_nesting_depth(&src).is_ok());
    }

    #[test]
    fn budget_charges_and_rejects_past_limit() {
        let b = DispatchBudget::new();
        for _ in 0..MAX_TOTAL_DISPATCHES {
            assert!(b.try_charge().is_ok());
        }
        match b.try_charge() {
            Err(after) => assert_eq!(after, MAX_TOTAL_DISPATCHES + 1),
            Ok(()) => panic!("expected budget to be exhausted"),
        }
    }

    #[test]
    fn budget_batch_charge_rejects_over_limit() {
        let b = DispatchBudget::new();
        match b.try_charge_n(MAX_TOTAL_DISPATCHES + 1) {
            Err(after) => assert_eq!(after, MAX_TOTAL_DISPATCHES + 1),
            Ok(()) => panic!("expected batch to exceed budget"),
        }
    }
}
