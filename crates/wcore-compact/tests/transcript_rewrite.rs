//! Integration test for T2-B2 transcript-rewrite primitive.
//!
//! Realistic PII-scrub scenario: an SSN-pattern rule is applied across a
//! mixed-role transcript and we assert that every entry's SSN substring
//! is gone after the rewrite.

use regex::Regex;
use wcore_compact::{ChunkRole, RewriteRule, TranscriptEntry, rewrite_transcript_entries};

fn entry(id: u64, role: ChunkRole, content: &str) -> TranscriptEntry {
    TranscriptEntry {
        id,
        role,
        content: content.to_string(),
        timestamp_ms: 1_700_000_000_000 + id,
    }
}

#[test]
fn rewrite_realistic_scrub_pii_pattern() {
    let entries = vec![
        entry(1, ChunkRole::User, "My SSN is 123-45-6789, please update."),
        entry(
            2,
            ChunkRole::Assistant,
            "Confirmed 123-45-6789 on file; secondary record 987-65-4321.",
        ),
        entry(
            3,
            ChunkRole::Tool,
            "lookup_result: { ssn: 555-44-3333, status: ok }",
        ),
        entry(
            4,
            ChunkRole::User,
            "No identifiers in this message — just a question.",
        ),
        entry(
            5,
            ChunkRole::System,
            "Policy reminder: never echo 000-00-0000.",
        ),
    ];

    let rules = vec![RewriteRule {
        pattern: Regex::new(r"\d{3}-\d{2}-\d{4}").expect("ssn regex compiles"),
        replacement: "[SSN]".to_string(),
        role_filter: None,
    }];

    let result = rewrite_transcript_entries(entries, &rules);

    // 1 + 2 + 1 + 0 + 1 = 5 total replacements across all entries.
    assert_eq!(result.changes, 5);
    assert_eq!(result.rewritten.len(), 5);

    // Sensitive substrings must be gone from every entry.
    let ssn_re = Regex::new(r"\d{3}-\d{2}-\d{4}").unwrap();
    for e in &result.rewritten {
        assert!(
            !ssn_re.is_match(&e.content),
            "entry {} still contains SSN-shaped substring: {}",
            e.id,
            e.content
        );
    }

    // Replacement token must appear in entries that originally had SSNs.
    assert!(result.rewritten[0].content.contains("[SSN]"));
    assert!(result.rewritten[1].content.contains("[SSN]"));
    assert!(result.rewritten[2].content.contains("[SSN]"));
    // Entry 4 had no SSN — should remain byte-identical.
    assert_eq!(
        result.rewritten[3].content,
        "No identifiers in this message — just a question."
    );
    // Entry 5 had its policy-reminder zeros scrubbed too.
    assert!(result.rewritten[4].content.contains("[SSN]"));

    // ids and timestamps preserved.
    for (i, e) in result.rewritten.iter().enumerate() {
        let expected_id = (i as u64) + 1;
        assert_eq!(e.id, expected_id);
        assert_eq!(e.timestamp_ms, 1_700_000_000_000 + expected_id);
    }
}
