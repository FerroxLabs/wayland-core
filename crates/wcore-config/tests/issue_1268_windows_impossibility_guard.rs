//! wayland#1268 c4 — no doc comment may assert, of Windows, the structural
//! impossibility that `atomic_io.rs` itself already records as false.
//!
//! # The defect this exists to stop recurring
//!
//! `#1248`'s lane gated two end-to-end notice tests to Linux/macOS and wrote
//! the reason into three places as a property of the PLATFORM:
//!
//! ```text
//! Windows publishes with `ReplaceFileW` and restores with a plain replacing
//! rename, which hands nothing back to judge, so no save can be intercepted
//! there at all.
//! ```
//!
//! That was false in the tree it was committed to, and `atomic_io.rs` had
//! already corrected exactly this reading in its own words — "was simply
//! wrong about `lpBackupFileName`". The damage was not the sentence: it was
//! that a true residual (*the notice path is untested on Windows*) became a
//! false one (*it cannot happen on Windows*), and the false one is what
//! justified filing no follow-up.
//!
//! A comment cannot be type-checked, so this is the check. It is deliberately
//! narrow: it flags a SENTENCE only when that sentence is about WINDOWS, about
//! the DISPLACED-SAVE subject `atomic_io.rs:442-451` governs, and asserts an
//! IMPOSSIBILITY — and it exempts a sentence that is correcting such a claim,
//! because the correction necessarily quotes it.
//!
//! # Sentence granularity, and why it is not a detail
//!
//! This guard first graded whole comment BLOCKS. MEASURED: re-injecting the
//! exact historical sentence directly above the doc comment that corrects it
//! did NOT redden it — the two runs of `//` lines joined into one block, the
//! correction's own marker exempted that block, and the offence rode in on
//! its neighbour's exemption. A false claim standing next to a true one is
//! still a false claim, and the place a new one is most likely to be written
//! is exactly beside the correction. So the exemption now reaches one
//! sentence, never a block, and the adjacency case is a control below.
//!
//! # Why it is not vacuous
//!
//! Three controls run before the sweep, and the sweep refuses a run that did
//! not walk the tree:
//!
//! * the historical false sentence MUST be flagged (a checker that flags
//!   nothing would otherwise pass on any tree);
//! * the corrected sentence that replaced it MUST NOT be (a checker that
//!   flags everything is equally worthless, and would force the next author
//!   to delete the correction to get green);
//! * an ordinary Windows comment on an unrelated subject MUST NOT be;
//! * and fewer than 20 files or 500 comment lines fails the test outright,
//!   because an empty offender list off an empty scan reads exactly like a
//!   clean tree.

use std::path::{Path, PathBuf};

/// Does this SENTENCE assert, of Windows, an impossibility about the
/// displaced-save path?
///
/// All three signal classes must be present in it, and no correction marker.
fn asserts_windows_impossibility(sentence: &str) -> bool {
    let text = sentence.to_lowercase();

    // 1. It is about Windows or about the Win32 primitive in question.
    let windows = ["windows", "replacefile", "win32"]
        .iter()
        .any(|k| text.contains(k));

    // 2. It is about the subject `atomic_io.rs:442-451` governs: whether the
    //    displaced file is handed back under a name the caller chooses.
    let subject = [
        "intercepted_save",
        "intercepted save",
        "displaced",
        "lpbackupfilename",
        "backup name",
        "exchanged_out",
        "publish_displacing",
        "hands nothing back",
    ]
    .iter()
    .any(|k| text.contains(k));

    // 3. It states that the thing cannot happen, rather than that it is
    //    untested, unmeasured or unexercised.
    let impossibility = [
        "cannot",
        "can never",
        "can not",
        "impossible",
        "structurally always",
        "hands nothing back",
        "nothing to judge",
        "unreachable",
        "not reachable",
        "never reachable",
        "no save can be intercepted",
    ]
    .iter()
    .any(|k| text.contains(k));

    // 4. …unless the comment is CORRECTING such a claim. A correction has to
    //    quote the false sentence to be readable, so without this exemption
    //    the only way to a green tree would be to delete the correction —
    //    which is the outcome #1268 was filed about.
    let correcting = [
        "was false",
        "is false",
        "simply wrong",
        "was wrong",
        "corrects",
        "correction",
        "not because",
        "is reachable",
        "no longer gated",
        "earlier reading",
        "earlier version",
        "used to say",
        "#1268",
        "wayland#1268",
    ]
    .iter()
    .any(|k| text.contains(k));

    windows && subject && impossibility && !correcting
}

/// Every sentence of every consecutive run of `//`-prefixed lines in `source`.
///
/// A run is joined first so a sentence wrapped across lines is graded whole,
/// then split on `.` — the guard's unit is the CLAIM, and a comment carries
/// several. Fragments shorter than a claim can be are dropped, so a bare
/// `Ok(..)` line or a table row cannot become an offender.
fn comment_sentences(source: &str) -> Vec<String> {
    let mut runs = Vec::new();
    let mut current: Vec<&str> = Vec::new();
    for line in source.lines() {
        let trimmed = line.trim_start();
        if let Some(rest) = trimmed.strip_prefix("//") {
            current.push(rest.trim_start_matches(['/', '!']).trim());
        } else if !current.is_empty() {
            runs.push(current.join(" "));
            current.clear();
        }
    }
    if !current.is_empty() {
        runs.push(current.join(" "));
    }
    runs.iter()
        .flat_map(|run| run.split('.'))
        .map(str::trim)
        .filter(|s| s.len() >= 20)
        .map(str::to_owned)
        .collect()
}

fn rust_sources(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            rust_sources(&path, out);
        } else if path.extension().is_some_and(|e| e == "rs") {
            out.push(path);
        }
    }
}

#[test]
fn no_doc_comment_claims_the_displaced_save_path_is_impossible_on_windows() {
    // ---- controls, before the sweep ------------------------------------
    const HISTORICAL_FALSE_CLAIM: &str = "Windows publishes with `ReplaceFileW` and restores \
         with a plain replacing rename, which hands nothing back to judge, so no save can be \
         intercepted there at all.";
    assert!(
        asserts_windows_impossibility(HISTORICAL_FALSE_CLAIM),
        "the checker does not flag the exact sentence #1268 was filed about, so a clean \
         sweep below would mean nothing"
    );

    const CORRECTED_CLAIM: &str = "On Windows `publish_displacing` returns \
         `Swap::Displaced(backup)` via `ReplaceFileW`'s `lpBackupFileName`, so \
         `intercepted_save: Some(..)` is reachable and the path is merely unexercised here.";
    assert!(
        !asserts_windows_impossibility(CORRECTED_CLAIM),
        "the checker flags the CORRECTION as an offence, which would force the next author \
         to delete it to get green"
    );

    const UNRELATED_WINDOWS_COMMENT: &str = "On Windows a path at or past this length cannot \
         be opened without the extended-length prefix.";
    assert!(
        !asserts_windows_impossibility(UNRELATED_WINDOWS_COMMENT),
        "the checker flags an ordinary Windows comment on an unrelated subject, so it is a \
         keyword alarm rather than a guard"
    );

    // ADJACENCY. The hole this guard shipped with for one commit: at block
    // granularity, the historical claim written directly above its own
    // correction was masked by the correction's exemption, and the red arm
    // came back green. Graded here on the real extractor, over a source
    // fragment shaped exactly like the one that defeated it.
    const OFFENCE_BESIDE_ITS_CORRECTION: &str = "\
    // Windows publishes with `ReplaceFileW` and restores with a plain\n\
    // replacing rename, which hands nothing back to judge, so no save can be\n\
    // intercepted there at all.\n\
    /// NO LONGER GATED TO UNIX (FerroxLabs/wayland#1268 c2). On Windows\n\
    /// `publish_displacing` returns `Swap::Displaced(backup)`, so the\n\
    /// intercepted_save path is reachable there.\n";
    assert!(
        comment_sentences(OFFENCE_BESIDE_ITS_CORRECTION)
            .iter()
            .any(|s| asserts_windows_impossibility(s)),
        "an offending sentence adjacent to a correcting one is not flagged, so the guard \
         is blind exactly where a new false claim would be written"
    );

    // ---- the sweep -------------------------------------------------------
    let workspace = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .expect("crates/<crate> has two ancestors")
        .to_path_buf();

    let mut files = Vec::new();
    for crate_name in ["wcore-tools", "wcore-config"] {
        for sub in ["src", "tests"] {
            rust_sources(
                &workspace.join("crates").join(crate_name).join(sub),
                &mut files,
            );
        }
    }
    assert!(
        files.len() >= 20,
        "the walk found {} sources under crates/wcore-tools and crates/wcore-config, so it \
         did not run over them and a clean result would mean nothing",
        files.len()
    );

    // THIS FILE IS THE ONE EXCLUSION, and it is asserted rather than assumed.
    // The guard quotes the offence it exists to catch -- in its module doc and
    // in the controls above -- so grading itself would make it permanently red
    // and force the quotes out, which is how the evidence gets deleted. The
    // count is pinned at one: a second exclusion would be a way to launder a
    // real offender into an exempt file.
    let self_path = Path::new(file!())
        .file_name()
        .expect("this test file has a name")
        .to_owned();
    let excluded = files
        .iter()
        .filter(|f| f.file_name() == Some(self_path.as_os_str()))
        .count();
    assert_eq!(
        excluded, 1,
        "expected to find and exclude exactly this guard file; found {excluded}. If it is 0 \
         the walk is not covering tests/ and the sweep grades less than it claims"
    );

    let (mut comment_lines, mut offenders) = (0usize, Vec::new());
    for file in &files {
        if file.file_name() == Some(self_path.as_os_str()) {
            continue;
        }
        let source = std::fs::read_to_string(file).expect("readable source");
        comment_lines += source
            .lines()
            .filter(|l| l.trim_start().starts_with("//"))
            .count();
        for sentence in comment_sentences(&source) {
            if asserts_windows_impossibility(&sentence) {
                offenders.push(format!("{}: {sentence}", file.display()));
            }
        }
    }
    assert!(
        comment_lines >= 500,
        "only {comment_lines} comment lines were examined; the walk stopped matching the \
         tree it grades"
    );
    assert!(
        offenders.is_empty(),
        "these comments assert, of Windows, an impossibility about the displaced-save path \
         that `wcore_config::atomic_io` itself records as false (wayland#1268 c4). Say the \
         path is UNEXERCISED there, which is true, not that it cannot happen:\n{offenders:#?}"
    );
}
