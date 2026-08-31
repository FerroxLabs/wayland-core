//! Measurement instrument for FerroxLabs/wayland-core#395 c1/c2 and
//! FerroxLabs/wayland#1235 ask 1.
//!
//! #395 measured ~100 s per megabyte of tool-result payload through
//! `AgentEngine::run()` in the test profile and could NOT name the function
//! carrying it (hetzner has neither `perf` nor `gdb`, so a profiler was not
//! available). Every tool result on the turn loop passes through
//! `wcore_agent::output_redaction::redact_tool_output` — `PIIScrubber::scrub`
//! plus an exact-token replace — and it passes through it TWICE per tool call
//! (`orchestration/mod.rs`, once before truncation and once after compaction).
//! This probe times `scrub` alone against payload size and payload SHAPE, so
//! the per-byte term is attributed by measurement rather than by reading.
//!
//! The shape arm is the control that says WHICH branch of `scrub` carries the
//! cost. `solid` is the fixture #1235 and #395 both use — `"x".repeat(N)`, one
//! whitespace-free run, which `base64_candidates` / `wrapped_base64_candidates`
//! match end to end and hand to `decoded_contains_secret`. `spaced` is the same
//! byte count broken by a space every 8 characters, so no token is long enough
//! (>= 24) to be a base64 candidate at all. Everything else is identical.
//!
//! `#[ignore]`d, so it costs the suite nothing.
//!
//! ```
//! cargo nextest run -p wcore-safety --test scrub_cost_probe --run-ignored all --no-capture
//! cargo nextest run --release -p wcore-safety --test scrub_cost_probe --run-ignored all --no-capture
//! ```
use wcore_safety::PIIScrubber;

fn solid(bytes: usize) -> String {
    "x".repeat(bytes)
}

/// Same byte count, but no token reaches the 24-character base64-candidate
/// floor, so the encoded-secret branch never runs.
fn spaced(bytes: usize) -> String {
    let mut out = String::with_capacity(bytes);
    while out.len() < bytes {
        out.push_str("xxxxxxx ");
    }
    out.truncate(bytes);
    out
}

#[test]
#[ignore = "measurement instrument for wayland-core#395; run with --run-ignored all"]
fn scrub_cost_probe() {
    let profile = if cfg!(debug_assertions) { "debug" } else { "release" };
    for (shape, make) in [
        ("solid", solid as fn(usize) -> String),
        ("spaced", spaced as fn(usize) -> String),
    ] {
        for bytes in [60_000usize, 120_000, 240_000, 480_000] {
            let payload = make(bytes);
            let start = std::time::Instant::now();
            let out = PIIScrubber.scrub(&payload);
            let elapsed = start.elapsed();
            eprintln!(
                "SCRUB profile={profile} shape={shape} bytes={bytes} secs={:.4} ns_per_byte={:.1} out_len={}",
                elapsed.as_secs_f64(),
                elapsed.as_secs_f64() * 1e9 / bytes as f64,
                out.len()
            );
        }
    }
}
