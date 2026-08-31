//! TEMPORARY bisecting instrumentation for wayland-core#395 c2. Not for merge.
use std::sync::atomic::{AtomicU64, Ordering};

pub struct Slot {
    pub name: &'static str,
    pub nanos: AtomicU64,
    pub calls: AtomicU64,
    pub bytes: AtomicU64,
}

macro_rules! slots {
    ($($id:ident => $name:literal),* $(,)?) => {
        $(pub static $id: Slot = Slot { name: $name, nanos: AtomicU64::new(0), calls: AtomicU64::new(0), bytes: AtomicU64::new(0) };)*
        pub static ALL: &[&Slot] = &[$(&$id),*];
    };
}

slots! {
    REDACT_TOOL_OUTPUT => "redact_tool_output",
    REDACT_ACTIVE_TOKENS => "  redact_active_tokens",
    PII_SCRUB => "  PIIScrubber::scrub",
    ESTIMATE_TOKENS => "estimate_tokens_from_messages",
    SHED => "shed_tool_outputs_until_under",
    STREAMING_REDACTOR => "StreamingRedactor::push",
    HOOK_SCRUB => "hooks::redact_tool_payload",
}

pub fn record(slot: &Slot, start: std::time::Instant, bytes: usize) {
    slot.nanos.fetch_add(start.elapsed().as_nanos() as u64, Ordering::Relaxed);
    slot.calls.fetch_add(1, Ordering::Relaxed);
    slot.bytes.fetch_add(bytes as u64, Ordering::Relaxed);
}

pub fn dump(tag: &str) {
    wcore_safety::pii_perf::dump();
    for s in ALL {
        let n = s.nanos.load(Ordering::Relaxed);
        if s.calls.load(Ordering::Relaxed) == 0 { continue; }
        eprintln!(
            "PERF {tag} {:<32} {:>10.4}s calls={} bytes={}",
            s.name, n as f64 / 1e9, s.calls.load(Ordering::Relaxed), s.bytes.load(Ordering::Relaxed)
        );
    }
}
