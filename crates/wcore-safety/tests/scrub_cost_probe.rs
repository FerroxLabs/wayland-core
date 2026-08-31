//! Measurement instrument for FerroxLabs/wayland-core#395 c1/c2 and
//! FerroxLabs/wayland#1235 ask 1.
//!
//! #395 measured ~100 s per megabyte of tool-result payload through
//! `AgentEngine::run()` in the test profile and could NOT name the function
//! carrying it (hetzner has neither `perf` nor `gdb`, so a profiler was not
//! available). Every tool result on the turn loop passes through
//! `wcore_agent::output_redaction::redact_tool_output` — `PIIScrubber::scrub`
//! plus an exact-token replace — and it passes through it TWICE per tool call
//! (`orchestration/mod.rs:2594` before truncation and `:2607` after
//! compaction). This probe times `scrub` alone against payload size and
//! payload SHAPE, so the per-byte term is attributed by measurement.
//!
//! THE SHAPE ARM IS THE CONTROL, and its first cut was wrong in a way worth
//! recording: it broke the payload with SPACES, and a space is inside
//! `wrapped_base64_candidates`' own character class
//! (`[A-Za-z0-9+/_=\r\n\t ]`), so the spaced arm was still one candidate run
//! end to end and controlled nothing — it measured within 12% of the solid arm
//! and would have read as "the encoded-secret branch is not the cost". The
//! separator has to be a byte outside BOTH candidate classes; `.` is.
//!
//! * `solid` — `"x".repeat(N)`, the fixture #1235 and #395 both use. One
//!   whitespace-free run that both candidate regexes match end to end and hand
//!   to `decoded_contains_secret`.
//! * `dotted` — the same byte count with a `.` every 8 characters. No run of
//!   the candidate alphabet reaches the 24-character floor, so no candidate is
//!   produced and the base64 branch never executes. Everything else is equal.
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

/// Same byte count, separated by a byte that is in neither
/// `base64_candidates`' class nor `wrapped_base64_candidates`', so no
/// candidate run is ever formed.
fn dotted(bytes: usize) -> String {
    let mut out = String::with_capacity(bytes + 8);
    while out.len() < bytes {
        out.push_str("xxxxxxx.");
    }
    out.truncate(bytes);
    out
}

#[test]
#[ignore = "measurement instrument for wayland-core#395; run with --run-ignored all"]
fn scrub_cost_probe() {
    let profile = if cfg!(debug_assertions) { "debug" } else { "release" };
    for bytes in [60_000usize, 120_000, 240_000, 480_000] {
        for (shape, make) in [
            ("solid", solid as fn(usize) -> String),
            ("dotted", dotted as fn(usize) -> String),
        ] {
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
