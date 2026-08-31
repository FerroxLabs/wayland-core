//! Measurement instrument for FerroxLabs/wayland-core#395 c2.
//!
//! #395 measured ~100 s per megabyte of tool-result payload through
//! `AgentEngine::run()` in the test profile and could not name the function
//! carrying it (hetzner has no `perf` and no `gdb`). Every tool result on the
//! turn loop passes through `wcore_agent::output_redaction::redact_tool_output`,
//! which is `PIIScrubber::scrub` plus an exact-token replace. This probe times
//! `scrub` alone, against payload size, so the per-byte term can be attributed
//! to it or ruled out by measurement rather than by reading.
//!
//! `#[ignore]`d, so it costs the suite nothing.
//!
//! ```
//! cargo nextest run -p wcore-safety --test scrub_cost_probe --run-ignored all --no-capture
//! cargo nextest run --release -p wcore-safety --test scrub_cost_probe --run-ignored all --no-capture
//! ```
use wcore_safety::PIIScrubber;

#[test]
#[ignore = "measurement instrument for wayland-core#395; run with --run-ignored all"]
fn scrub_cost_probe() {
    let profile = if cfg!(debug_assertions) { "debug" } else { "release" };
    for bytes in [60_000usize, 120_000, 240_000, 480_000] {
        // The exact fixture shape wayland-core#395 and wayland#1235 measure:
        // "x".repeat(N), a single whitespace-free run.
        let payload = "x".repeat(bytes);
        let start = std::time::Instant::now();
        let out = PIIScrubber.scrub(&payload);
        let elapsed = start.elapsed();
        eprintln!(
            "SCRUB profile={profile} bytes={bytes} secs={:.3} ns_per_byte={:.1} out_len={}",
            elapsed.as_secs_f64(),
            elapsed.as_secs_f64() * 1e9 / bytes as f64,
            out.len()
        );
    }
}
