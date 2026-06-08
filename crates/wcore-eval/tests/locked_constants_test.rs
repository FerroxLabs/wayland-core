//! Wave RC (audit MAJOR #10) — SHA-256 pinning of the W10A LOCKED
//! scoring constants.
//!
//! Any drift in `LOCKED` (whether a weight, saturation reference,
//! acceptance cutoff, or model allowlist entry) flips the digest and
//! breaks this test. Bumping the pinned hash MUST be a deliberate
//! decision documented alongside an acceptance-gate re-validation; do
//! NOT update `EXPECTED` mechanically.

use sha2::{Digest, Sha256};
use wcore_eval::LOCKED;

/// Lower-case hex of `Sha256::finalize`.
fn hex_lower(bytes: &[u8]) -> String {
    let mut s = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        s.push_str(&format!("{b:02x}"));
    }
    s
}

/// Stable serialization of every LOCKED constant in deterministic
/// order. f64 values are hashed via their little-endian IEEE 754
/// bit-pattern so 0.1 + 0.2 ≠ 0.3 issues do not slip through.
fn digest_locked() -> String {
    let mut hasher = Sha256::new();
    hasher.update(LOCKED.w_outcome().to_le_bytes());
    hasher.update(LOCKED.w_cost().to_le_bytes());
    hasher.update(LOCKED.w_size().to_le_bytes());
    hasher.update(LOCKED.cost_saturate_usd().to_le_bytes());
    hasher.update(LOCKED.tokens_saturate().to_le_bytes());
    hasher.update((LOCKED.size_saturate_bytes() as u64).to_le_bytes());
    hasher.update(LOCKED.acceptance_cutoff().to_le_bytes());
    // Allowlist: length-prefixed entries so order + boundaries are
    // hashed deterministically and re-ordering or splitting an entry
    // is detected.
    hasher.update((LOCKED.model_allowlist().len() as u64).to_le_bytes());
    for entry in LOCKED.model_allowlist() {
        hasher.update((entry.len() as u64).to_le_bytes());
        hasher.update(entry.as_bytes());
    }
    hex_lower(&hasher.finalize())
}

/// Pinned SHA-256 digest of the LOCKED scoring constants. If you are
/// here because the assertion failed: confirm with the audit owner
/// that the change is intentional (precision/recall ≥ 0.80 re-verified
/// via `vx just eval-gate`) and replace this with the printed actual.
const EXPECTED: &str = "cca4a59a995f0eec578e8ee54947cb9004772e8346b11e23a702ff960caf5673";

#[test]
fn locked_scorer_constants_have_not_drifted() {
    let actual = digest_locked();
    assert_eq!(
        actual, EXPECTED,
        "DefaultScorer LOCKED constants drifted; bump EXPECTED to {actual} AFTER confirming with author that the change is intentional + revalidate acceptance gate"
    );
}

#[test]
fn locked_constants_match_w10a_defaults() {
    // Belt-and-suspenders explicit equality so the digest mismatch
    // diagnostic above can be cross-checked. If this fails, the
    // change is structural (someone retuned weights), not encoding
    // (e.g. byte order).
    assert_eq!(LOCKED.w_outcome(), 0.7);
    assert_eq!(LOCKED.w_cost(), 0.2);
    assert_eq!(LOCKED.w_size(), 0.1);
    assert_eq!(LOCKED.cost_saturate_usd(), 0.05);
    assert_eq!(LOCKED.tokens_saturate(), 2_000);
    assert_eq!(LOCKED.size_saturate_bytes(), 2_048);
    assert_eq!(LOCKED.acceptance_cutoff(), 0.65);
    assert_eq!(
        LOCKED.model_allowlist(),
        &["claude-sonnet-4-7", "claude-opus-4-7", "claude-haiku-4-5"]
    );
}

#[test]
fn weights_sum_to_one_exactly_within_tolerance() {
    let sum = LOCKED.w_outcome() + LOCKED.w_cost() + LOCKED.w_size();
    assert!(
        (sum - 1.0).abs() < 1e-9,
        "weights drifted from 1.0 sum: {sum}"
    );
}
