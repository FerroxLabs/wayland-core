//! Benchmark: parse a ~1 KB Anthropic SSE chunk via `parse_sse_data`.
//!
//! Uses the public `anthropic_shared::parse_sse_data` function which is the
//! hot path for every streaming response token.

use criterion::{Criterion, black_box, criterion_group, criterion_main};
use wcore_providers::anthropic_shared::{StreamState, parse_sse_data};

/// A realistic `content_block_delta` payload carrying ~1 KB of text.
fn sse_payload() -> String {
    let text = "a".repeat(900);
    format!(
        r#"{{"type":"content_block_delta","index":0,"delta":{{"type":"text_delta","text":"{text}"}}}}"#
    )
}

fn bench_parse_sse_chunk(c: &mut Criterion) {
    let data = sse_payload();

    c.bench_function("parse_sse_data_1kb_text_delta", |b| {
        b.iter(|| {
            let mut state = StreamState::new();
            let events = parse_sse_data(
                black_box("content_block_delta"),
                black_box(&data),
                &mut state,
            );
            black_box(events);
        });
    });
}

criterion_group!(benches, bench_parse_sse_chunk);
criterion_main!(benches);
