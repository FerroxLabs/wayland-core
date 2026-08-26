//! The README's live-evidence claim must agree with `docs/delivery-semantics.md`.
//!
//! # Why this file exists
//!
//! `docs/delivery-semantics.md` is enforced against the code by
//! `delivery_semantics_declaration.rs`, so it cannot drift from what the
//! adapters actually do. The README is not, and the README is what an external
//! reviewer reads first. It names ten messaging platforms and, until this test,
//! said nothing about which of them had ever been driven at a real destination
//! — so the two documents could disagree indefinitely and nothing would notice.
//!
//! The specific hazard is not hypothetical. Slack and Discord each held an
//! `exactly-once` claim on the strength of a `mockito` fixture and each produced
//! **two** messages the first time a delivery key was replayed at the real
//! platform. A README that quietly generalises "ten platforms" into "ten proven
//! platforms" is the same error one layer up.
//!
//! # What is asserted
//!
//! `docs/delivery-semantics.md` §2 carries a *"Replay measured at a real
//! destination?"* column. A row whose cell says `NOT MEASURED` has no live
//! evidence; every other row does. That partition is the fact. This test
//! requires the README to state the same partition.
//!
//! **Three states since 2026-07-31, not two.** Matrix's guarantee acquired a
//! precondition (exactly-once below its length cap, at-least-once above), and
//! only the below-cap half has been driven live. Its cell therefore says both
//! `Yes` and `NOT MEASURED`, which a substring test for either one reads
//! wrongly. Such a cell must carry the explicit [`SCOPED_DRIVEN`] phrase and
//! counts as driven; a cell claiming both WITHOUT that phrase is rejected as
//! ambiguous rather than guessed at. See
//! [`the_comparator_rejects_a_mixed_cell_that_does_not_declare_its_scope`].
//!
//! This test requires the README to state the same partition:
//!
//! 1. the spelled count of driven platforms matches the doc's count;
//! 2. the spelled count of the remainder matches the doc's count;
//! 3. every driven platform is named on the driven side of the README bullet;
//! 4. every not-measured platform is named on the *other* side, and neither set
//!    leaks across — so moving one platform to the wrong side reddens this;
//! 5. the README links the enforced document, so the pointer cannot be dropped.
//!
//! # It runs in BOTH directions
//!
//! A gate that cannot fail proves nothing, and a gate that cannot pass proves
//! less. The comparator is a pure function of two strings, so the tests below
//! feed it doctored inputs:
//!
//! - [`the_comparator_rejects_a_stale_count`],
//!   [`the_comparator_rejects_a_platform_on_the_wrong_side`] and
//!   [`the_comparator_rejects_a_dropped_link`] construct the disagreements and
//!   assert they are reported — the gate can fail.
//! - [`the_comparator_passes_when_a_seventh_row_becomes_measured`] constructs a
//!   *different world* — Telegram driven live, README updated to match — and
//!   asserts zero problems. So the pass state is reachable under a changed
//!   fact, which is what distinguishes a gate that tracks the criterion from
//!   one that is merely green today.
//! - Each doctoring step asserts that it actually changed the string. A
//!   `str::replace` that matches nothing silently returns the original, which
//!   would make every control above vacuous.

use std::collections::BTreeMap;

const README: &str = include_str!("../../../README.md");
const DECLARATION: &str = include_str!("../../../docs/delivery-semantics.md");

/// The stable half of the README bullet's lead-in. The count immediately
/// precedes it and is deliberately **not** part of the anchor: if the anchor
/// carried the number, a legitimately changed count would make the bullet
/// unfindable and this test would report "missing" instead of "stale", which is
/// the less useful of the two failures.
const DRIVEN_ANCHOR: &str = " of the ten have been driven at the real platform.";

/// Introduces the not-measured side of the same bullet, and is the split point
/// between the two sides.
const OTHER_ANCHOR: &str = "The other ";

/// The markdown link target the README must keep.
const DOC_LINK: &str = "](docs/delivery-semantics.md)";

/// The phrase `docs/delivery-semantics.md` §2 uses for a row with no live
/// evidence. Every other row in that column asserts one.
const NOT_MEASURED: &str = "NOT MEASURED";

/// The phrase a row uses when its evidence is **split**: measured live for part
/// of the guarantee's range, unmeasured for the rest.
///
/// Added 2026-07-31 for the Matrix row, and the reason it is an explicit
/// literal rather than a cleverer parse matters. `NOT MEASURED` used to be a
/// sound binary test because every cell was wholly one thing or the other.
/// Matrix is now genuinely both — driven at matrix.org below its 32,768-char
/// cap, never driven above it — so its cell contains `Yes` *and* `NOT
/// MEASURED`, and a substring check for either one alone silently returns the
/// wrong answer.
///
/// It counts as **driven**: the README bullet's claim is "this platform has
/// been driven at the real platform", and Matrix has. The above-cap gap is a
/// scope note on the guarantee, recorded in §4.1, not a claim that no live run
/// ever happened.
///
/// Requiring the exact phrase, rather than inferring "mixed" from the presence
/// of both substrings, keeps the dangerous direction closed: a genuinely
/// unmeasured row that happens to contain the word "Yes" in prose still
/// classifies as not-measured, because it will not carry this literal.
const SCOPED_DRIVEN: &str = "BELOW the cap: Yes";

/// Row labels as they appear in the first column of the §2 prose table.
///
/// The trailing `**` matters: `| **WhatsApp` alone would also match the
/// `| **WhatsApp bridge**` row, which is a *different* adapter and deliberately
/// outside the ten.
fn prose_label(platform: &str) -> &'static str {
    match platform {
        "slack" => "**Slack**",
        "matrix" => "**Matrix**",
        "discord" => "**Discord**",
        "telegram" => "**Telegram**",
        "sms" => "**Twilio SMS**",
        "whatsapp" => "**WhatsApp**",
        "email" => "**Email**",
        "signal" => "**Signal**",
        "imessage" => "**iMessage**",
        "msteams" => "**MS Teams**",
        other => panic!("no prose label known for {other:?}"),
    }
}

/// How the README spells each platform, which is not always how the table does.
fn readme_name(platform: &str) -> &'static str {
    match platform {
        "slack" => "Slack",
        "matrix" => "Matrix",
        "discord" => "Discord",
        "telegram" => "Telegram",
        "sms" => "Twilio SMS",
        "whatsapp" => "WhatsApp",
        "email" => "email",
        "signal" => "Signal",
        "imessage" => "iMessage",
        "msteams" => "MS Teams",
        other => panic!("no README name known for {other:?}"),
    }
}

const PLATFORMS: &[&str] = &[
    "slack", "matrix", "discord", "telegram", "sms", "whatsapp", "email", "signal", "imessage",
    "msteams",
];

fn spelled(word: &str) -> Option<usize> {
    match word.to_ascii_lowercase().as_str() {
        "zero" => Some(0),
        "one" => Some(1),
        "two" => Some(2),
        "three" => Some(3),
        "four" => Some(4),
        "five" => Some(5),
        "six" => Some(6),
        "seven" => Some(7),
        "eight" => Some(8),
        "nine" => Some(9),
        "ten" => Some(10),
        _ => None,
    }
}

/// The byte range of §2, so the row lookups below cannot wander into another table.
///
/// **Added 2026-08-26.** These helpers always claimed to read §2 and in fact read the whole
/// document, taking the first row whose first cell matched. That was correct only because §2
/// happened to be the first table using those labels — a property of document order, not of
/// anything asserted. §4.2's per-adapter cap table now uses the same labels, and
/// `doctor_evidence_cell`'s `hits == 1` guard caught it, exactly as it was written to.
fn section_2_bounds(doc: &str) -> (usize, usize) {
    let lo = doc
        .find("## 2. The table")
        .expect("docs/delivery-semantics.md has lost its §2 heading");
    let rest = &doc[lo..];
    let hi = lo
        + rest
            .find("\n## 3.")
            .expect("§2 is not terminated by a §3 heading");
    (lo, hi)
}

/// Parse §2's table into `platform -> was it driven at a real destination`.
///
/// Reads the **last** cell of each row rather than searching the whole line,
/// because the earlier cells discuss the platform's dedup primitive in prose and
/// a whole-row search would collide with it.
fn driven_per_platform(doc: &str, problems: &mut Vec<String>) -> BTreeMap<&'static str, bool> {
    let mut out = BTreeMap::new();
    let (lo, hi) = section_2_bounds(doc);
    let section = &doc[lo..hi];

    for platform in PLATFORMS {
        let label = prose_label(platform);
        let needle = format!("| {label}");
        let Some(row) = section
            .lines()
            .find(|l| l.trim_start().starts_with(&needle))
        else {
            problems.push(format!(
                "docs/delivery-semantics.md has no §2 table row starting with {label} \
                 for {platform:?}"
            ));
            continue;
        };

        // `| a | b | c | d | e | f |` splits into 8 pieces: an empty string,
        // the six cells, and another empty string. Anything else means the
        // table's shape changed and the cell index below would be silently
        // wrong, so it is reported rather than assumed.
        let parts: Vec<&str> = row.split('|').collect();
        if parts.len() != 8 {
            problems.push(format!(
                "§2 row for {platform:?} has {} pipe-separated pieces, expected 8 — the table \
                 shape changed and this parser can no longer locate the evidence column",
                parts.len()
            ));
            continue;
        }
        let evidence = parts[parts.len() - 2].trim();

        // Ordered, because the cases overlap. A scoped cell contains BOTH
        // "Yes" and "NOT MEASURED", so it has to be recognised before either
        // of the plain checks can misread it.
        let driven = if evidence.contains(SCOPED_DRIVEN) {
            true
        } else if evidence.contains(NOT_MEASURED) {
            false
        } else if evidence.contains("Yes") {
            true
        } else {
            problems.push(format!(
                "§2 evidence cell for {platform:?} says none of {SCOPED_DRIVEN:?}, \
                 {NOT_MEASURED:?} or \"Yes\", so it cannot be classified: {evidence:?}"
            ));
            continue;
        };

        // A cell that claims both without using the scoped phrase is
        // ambiguous, and guessing at it is how a partial measurement gets
        // recorded as a whole one. Reject it and make the author say which.
        if !evidence.contains(SCOPED_DRIVEN)
            && evidence.contains(NOT_MEASURED)
            && evidence.contains("Yes")
        {
            problems.push(format!(
                "§2 evidence cell for {platform:?} claims both \"Yes\" and {NOT_MEASURED:?} but \
                 does not carry {SCOPED_DRIVEN:?}, so it is ambiguous about what was actually \
                 measured: {evidence:?}"
            ));
            continue;
        }

        out.insert(*platform, driven);
    }

    out
}

/// Compare the README's live-evidence bullet against the enforced document.
///
/// Returns one string per disagreement, empty when they agree.
fn disagreements(readme: &str, doc: &str) -> Vec<String> {
    let mut problems = Vec::new();
    let driven = driven_per_platform(doc, &mut problems);
    if driven.len() != PLATFORMS.len() {
        return problems;
    }

    let Some(bullet) = readme.lines().find(|l| l.contains(DRIVEN_ANCHOR)) else {
        problems.push(format!(
            "README.md contains no line carrying {DRIVEN_ANCHOR:?}, so it makes no statement \
             about which platforms have live evidence"
        ));
        return problems;
    };

    let Some(split) = bullet.find(OTHER_ANCHOR) else {
        problems.push(format!(
            "README live-evidence bullet contains no {OTHER_ANCHOR:?}, so the not-measured \
             platforms are not separated from the driven ones"
        ));
        return problems;
    };
    let (head, tail) = bullet.split_at(split);

    // The two counts the README states in words.
    let head_prefix = &head[..head.find(DRIVEN_ANCHOR).expect("anchor is in head")];
    let claimed_driven = head_prefix
        .split_whitespace()
        .next_back()
        .unwrap_or("")
        .trim_matches('*');
    let claimed_other = tail[OTHER_ANCHOR.len()..]
        .split_whitespace()
        .next()
        .unwrap_or("");

    let expect_driven = driven.values().filter(|d| **d).count();
    let expect_other = driven.values().filter(|d| !**d).count();

    match spelled(claimed_driven) {
        Some(n) if n == expect_driven => {}
        Some(n) => problems.push(format!(
            "README says {n} platform(s) have been driven at the real platform; \
             docs/delivery-semantics.md §2 shows {expect_driven}"
        )),
        None => problems.push(format!(
            "README's driven count {claimed_driven:?} is not a spelled number this test knows"
        )),
    }
    match spelled(claimed_other) {
        Some(n) if n == expect_other => {}
        Some(n) => problems.push(format!(
            "README says {n} platform(s) have no live evidence; \
             docs/delivery-semantics.md §2 shows {expect_other}"
        )),
        None => problems.push(format!(
            "README's remainder count {claimed_other:?} is not a spelled number this test knows"
        )),
    }

    // Each platform must be named on its own side, and on no other. Containment
    // alone would pass a bullet that lists all ten in one breath; the partition
    // is the claim being checked.
    for (platform, was_driven) in &driven {
        let name = readme_name(platform);
        let (want, want_side, avoid, avoid_side) = if *was_driven {
            (head, "driven", tail, "not-measured")
        } else {
            (tail, "not-measured", head, "driven")
        };
        if !want.contains(name) {
            problems.push(format!(
                "docs/delivery-semantics.md §2 puts {platform:?} on the {want_side} side, but \
                 the README does not name {name:?} there"
            ));
        }
        if avoid.contains(name) {
            problems.push(format!(
                "README names {name:?} on the {avoid_side} side, but \
                 docs/delivery-semantics.md §2 puts {platform:?} on the {want_side} side"
            ));
        }
    }

    if !bullet.contains(DOC_LINK) {
        problems.push(format!(
            "README live-evidence bullet does not link {DOC_LINK:?}, so a reader has no route \
             to the enforced per-adapter table"
        ));
    }

    problems
}

/// Replace the final cell of one §2 row, and prove the replacement happened.
fn doctor_evidence_cell(doc: &str, label: &str, new_cell: &str) -> String {
    let needle = format!("| {label}");
    let (lo, hi) = section_2_bounds(doc);
    let mut hits = 0usize;
    let mut offset = 0usize;
    let out = doc
        .lines()
        .map(|line| {
            let here = offset;
            offset += line.len() + 1;
            if here < lo || here >= hi || !line.trim_start().starts_with(&needle) {
                return line.to_string();
            }
            let mut parts: Vec<&str> = line.split('|').collect();
            let last = parts.len() - 2;
            parts[last] = new_cell;
            hits += 1;
            parts.join("|")
        })
        .collect::<Vec<_>>()
        .join("\n");
    assert_eq!(
        hits, 1,
        "doctoring {label} touched {hits} rows, expected exactly 1 — the control would \
         otherwise be testing an unmodified document"
    );
    out
}

/// A `str::replace` whose pattern is absent returns the original unchanged, so a
/// control built from one would silently assert nothing.
fn replace_once(haystack: &str, from: &str, to: &str) -> String {
    assert!(
        haystack.contains(from),
        "control cannot be constructed: {from:?} is not present in the input"
    );
    haystack.replace(from, to)
}

#[test]
fn the_readme_agrees_with_the_enforced_delivery_semantics() {
    let problems = disagreements(README, DECLARATION);
    assert!(
        problems.is_empty(),
        "README.md and docs/delivery-semantics.md disagree about which platforms have been \
         driven at a real destination:\n  {}",
        problems.join("\n  ")
    );
}

/// The count is the cheapest thing to leave stale: someone drives an eighth
/// platform, updates the enforced table, and the README keeps its old number.
#[test]
fn the_comparator_rejects_a_stale_count() {
    let stale = replace_once(README, "Three of the ten", "Four of the ten");
    let problems = disagreements(&stale, DECLARATION);
    assert_eq!(problems.len(), 1, "got: {problems:?}");
    assert!(
        problems[0].contains("driven at the real platform"),
        "got: {problems:?}"
    );
}

/// Naming every platform somewhere in the bullet is not enough — the README has
/// to put each one on the correct side of the line.
#[test]
fn the_comparator_rejects_a_platform_on_the_wrong_side() {
    let moved = replace_once(
        README,
        "Slack, Discord and Matrix were each",
        "Slack, Discord, Telegram and Matrix were each",
    );
    let problems = disagreements(&moved, DECLARATION);
    assert!(
        problems.iter().any(|p| p.contains("\"Telegram\"")
            && p.contains("driven side")
            && p.contains("not-measured side")),
        "expected Telegram to be reported as claimed-driven while the table says \
         not-measured; got: {problems:?}"
    );
}

/// The scoped-evidence branch, in both directions.
///
/// Known-positive first: Matrix's real cell IS scoped today, and the whole
/// suite is green, so the branch is exercised by every other test here. What
/// this adds is the failure direction — a cell that claims a live "Yes" AND a
/// "NOT MEASURED" without saying which part is which must be REJECTED, not
/// silently resolved. Guessing is how half a measurement gets recorded as a
/// whole one, which is the error this whole file exists to catch one layer up.
#[test]
fn the_comparator_rejects_a_mixed_cell_that_does_not_declare_its_scope() {
    // Known-positive: the real document classifies cleanly.
    assert!(
        disagreements(README, DECLARATION).is_empty(),
        "the unmutated comparison must be green for this control to mean anything"
    );

    // Strip only the scoping phrase, leaving both claims standing.
    let ambiguous = replace_once(DECLARATION, SCOPED_DRIVEN, "Yes");
    let problems = disagreements(README, &ambiguous);
    assert!(
        problems
            .iter()
            .any(|p| p.contains("ambiguous about what was actually measured")),
        "a cell claiming both Yes and NOT MEASURED without the scoping phrase must be \
         rejected; got: {problems:?}"
    );
}

/// The link is the reader's only route from the summary to the enforced detail.
#[test]
fn the_comparator_rejects_a_dropped_link() {
    let unlinked = replace_once(README, DOC_LINK, "](docs/channels.md)");
    let problems = disagreements(&unlinked, DECLARATION);
    assert!(
        problems.iter().any(|p| p.contains("does not link")),
        "got: {problems:?}"
    );
}

/// **Can this gate pass under a changed fact?**
///
/// A gate stuck red measures nothing. This constructs the world in which
/// Telegram has been driven live — the enforced table updated, and the README
/// updated to match — and requires the comparator to go green. Together with
/// the rejection tests above, that pins the comparator to the criterion rather
/// than to today's answer.
#[test]
fn the_comparator_passes_when_a_seventh_row_becomes_measured() {
    let doc = doctor_evidence_cell(
        DECLARATION,
        prose_label("telegram"),
        " **Yes** — a replayed key produced **two** messages ",
    );

    let readme = replace_once(README, "Three of the ten", "Four of the ten");
    let readme = replace_once(
        &readme,
        "Slack, Discord and Matrix were each",
        "Slack, Discord, Telegram and Matrix were each",
    );
    let readme = replace_once(&readme, "The other seven — Telegram, ", "The other six — ");

    let problems = disagreements(&readme, &doc);
    assert!(
        problems.is_empty(),
        "the comparator has no reachable pass state for a newly-measured platform: {problems:?}"
    );
}

/// The comparator must not silently pass a README that says nothing at all,
/// which is the state this file was written to end.
#[test]
fn the_comparator_rejects_a_readme_with_no_claim() {
    let problems = disagreements("# A README with no live-evidence bullet\n", DECLARATION);
    assert_eq!(problems.len(), 1, "got: {problems:?}");
    assert!(
        problems[0].contains("no line carrying"),
        "got: {problems:?}"
    );
}
