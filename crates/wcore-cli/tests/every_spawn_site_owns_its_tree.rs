//! FerroxLabs/wayland-core#352 — the sweep must not rot.
//!
//! #1156 fixed five `acp serve` sites and built the shared guard;
//! `crates/wcore-cli/tests/support/owned_tree.rs` is that guard, and #352 swept
//! it across every remaining site in this crate's test tree. A sweep is a
//! one-time state, not a property: the next author adds one more
//! `Command::new(binary()).spawn()` and nothing anywhere says no. That is how
//! forty-four sites accumulated in the first place.
//!
//! This file is the ratchet. It reads this crate's own test sources and refuses
//! any process-spawning expression that is not handed to `OwnedTree`.
//!
//! ## Why the SWEEP is graded from source text
//!
//! There is no runtime hook that could see an unwrapped site. Each integration
//! test file is its own binary, the leak is observable only as a process still
//! alive after the binary exited, and nothing inside the process can observe
//! its own afterlife. Whether every site is WRAPPED is in the source or
//! nowhere.
//!
//! ## ...and why wrapping alone is not the claim (wayland-core#385)
//!
//! A scan for `OwnedTree::new(` cannot tell a working guard from an empty
//! struct, and for a while it did not have to: this ratchet was GREEN through
//! the entire period `OwnedTree`'s descendant walk was dead, while one
//! behavioural test carried the whole ownership claim on its own. Cited as
//! class closure for #352 / #1156, that is a gate certifying a sweep that owns
//! nothing.
//!
//! So the ratchet now ends with two assertions the source text cannot fake:
//!
//! * [`assert_the_guard_actually_owns_the_tree`] asks the KERNEL — the `/proc`
//!   walk must see a real grandchild on Unix, the Job Object must contain it on
//!   Windows, and dropping the guard must kill it.
//! * [`assert_the_behavioural_twins_are_armed`] refuses to let the fuller
//!   behavioural tests be deleted, skipped, narrowed to another platform or
//!   listed in a file that silences them without this ratchet going red with
//!   them.
//!
//! ## Why the twins' attributes are graded by ALLOWLIST
//!
//! That second assertion used to name the spellings it refused: `#[ignore` and
//! `#[cfg(`. `#[cfg_attr(not(target_os = "macos"), ignore)]` matches NEITHER,
//! and is simultaneously a skip and a platform-condition; so is a file-level
//! `#![cfg_attr(cond, cfg(any()))]`, which is not `#![cfg(`. Both spellings
//! leave the twin unrun with this ratchet green, and `cfg_attr(..., ignore)` is
//! this repo's own house idiom for a platform skip -- twenty-five live sites,
//! one of whose module docs teaches it.
//!
//! Enumerating skips cannot work: the alphabet is open, and an attribute macro
//! nobody has written yet skips just as well as `#[ignore]`. So the attributes
//! are graded the other way round. Whitespace is stripped first, then:
//!
//! * the file must carry EXACTLY ONE inner attribute, and it must be the gate;
//! * the twin's own attribute block must be EXACTLY `#[test]`.
//!
//! "is a skip" is undecidable over an open alphabet; "is not `#[test]`" is
//! decidable and total. Any other attribute, in any spelling, reds this ratchet
//! -- including ones that do not exist yet. Loosening either bound is a
//! deliberate edit to this file, which is what a ratchet is for.
//!
//! ## What this still does not see (named, not hidden)
//!
//! The allowlist is closed over ATTRIBUTES. It is not closed over:
//!
//! * a body-internal skip -- an early `return`, or a `cfg!(...)` guard around
//!   the assertions. The twin still runs and still passes, vacuously. That is
//!   why [`assert_the_guard_actually_owns_the_tree`] below drives the kernel
//!   check from THIS binary rather than trusting the twins: a hollowed twin
//!   does not leave the ownership claim unproven.
//! * a nextest `default-filter` that excludes the twin's binary without naming
//!   the test. [`QUARANTINE_LISTS`] covers the two by-name lists only.
//! * a CI job that never invokes the twin's binary at all.
//!
//! Grading "the twin RUNS" directly would mean shelling out to
//! `cargo nextest list` from inside a test nextest is already running: a nested
//! cargo build under the test harness, which can rebuild, contend on the build
//! lock, and hard-fail wherever cargo is absent. A gate that cannot be trusted
//! to pass is worth no more than one that cannot fail, so the static allowlist
//! plus the named gaps above is the honest instrument.
//!
//! ## Why it is not fooled by its own text
//!
//! [`blank_noncode`] replaces every comment and every string/char literal with
//! spaces before anything is matched. So the spawn spellings quoted in this
//! module's own prose and fixtures are invisible to it, a `;` inside an
//! `expect("...")` message cannot move a statement boundary, and a `(` inside
//! one cannot unbalance the parenthesis walk.

#[path = "support/mod.rs"]
mod support;

use std::time::{Duration, Instant};
use support::process_tree_fixture::{force_kill, spawn_detaching_parent};
use wcore_types::process_liveness::process_is_alive;

/// The behavioural twins this file leans on, and the ONE inner attribute each
/// is allowed to carry. Together the pair covers every platform, which is what
/// makes "either twin is `#![cfg]`-gated" acceptable and a SECOND inner
/// attribute -- of ANY spelling, not just a second `#![cfg(` -- not.
///
/// `(file, its only permitted inner attribute, the test it must declare)`.
const BEHAVIOURAL_TWINS: &[(&str, &str, &str)] = &[
    (
        "harness_owns_spawned_trees.rs",
        "#![cfg(unix)]",
        "dropping_the_guard_kills_a_detached_grandchild_and_reaps_the_direct_child",
    ),
    (
        "harness_owns_spawned_trees_windows.rs",
        "#![cfg(windows)]",
        "dropping_the_guard_kills_a_detached_grandchild_on_windows",
    ),
];

/// Files that would silence a test without touching its source.
const QUARANTINE_LISTS: &[&str] = &[
    ".config/known-failing-tests.txt",
    ".config/flaky-allowlist.txt",
];

fn tests_root() -> std::path::PathBuf {
    std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests")
}

fn repo_root() -> std::path::PathBuf {
    // <repo>/crates/wcore-cli -> <repo>
    std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(2)
        .expect("wcore-cli sits two levels under the workspace root")
        .to_path_buf()
}

/// `s` with every whitespace character removed.
///
/// Attribute comparisons run over this, so they are over the attributes
/// THEMSELVES and not over their formatting: `# [ ignore ]` and `#[ignore]`
/// normalise to one string, and neither can hide behind a line break.
fn strip_ws(s: &str) -> String {
    s.chars().filter(|c| !c.is_whitespace()).collect()
}

/// The attribute block the declaration at `decl_at` carries, whitespace removed.
///
/// The previous item ends at a `}` (a fn or an impl) or a `;` (a `use` or a
/// `mod`), whichever is later; with neither, the declaration is the first thing
/// in the file and the whole head is its attribute block.
///
/// Callers pass ALREADY-BLANKED code, so a `}` or a `#[` inside a comment or a
/// string literal can neither move the boundary nor invent an attribute.
fn attrs_before(code: &str, decl_at: usize) -> String {
    let head = &code[..decl_at];
    let boundary = match (head.rfind('}'), head.rfind(';')) {
        (Some(a), Some(b)) => Some(a.max(b)),
        (Some(a), None) => Some(a),
        (None, Some(b)) => Some(b),
        (None, None) => None,
    };
    let attrs = match boundary {
        Some(b) => &head[b + 1..],
        None => head,
    };
    strip_ws(attrs)
}

/// Is `needle` named by a non-comment, non-blank line of `list`?
///
/// Split out so the assertion below and its own positive control run the SAME
/// predicate — a query that silently fails returns nothing, and nothing reads
/// as "not quarantined".
fn list_names(list: &str, needle: &str) -> bool {
    list.lines()
        .map(str::trim)
        .filter(|l| !l.is_empty() && !l.starts_with('#'))
        .any(|l| l.contains(needle))
}

/// c3 (FerroxLabs/wayland-core#385). The behavioural twins are the only thing
/// that can tell a working guard from a stub, so the wrapping ratchet must go
/// red if either one is deleted, skipped, narrowed to a third platform, or
/// listed in a file that silences it.
///
/// This is a SOURCE + CONFIG check on purpose: availability is a static
/// property, and a test that has been skipped cannot report on itself.
///
/// The attribute half is an ALLOWLIST -- exactly one inner attribute, and
/// exactly `#[test]` on the twin -- because a denylist of skip spellings cannot
/// be closed. The module doc carries the argument, and names the three channels
/// this still does not see.
fn assert_the_behavioural_twins_are_armed() {
    let quarantine: Vec<(&str, String)> = QUARANTINE_LISTS
        .iter()
        .map(|rel| {
            let path = repo_root().join(rel);
            let body = std::fs::read_to_string(&path).unwrap_or_else(|e| {
                panic!(
                    "{rel} could not be read ({e}). A quarantine list this check \
                     cannot open is one it cannot see an entry in, and an \
                     unreadable file would read exactly like an empty one"
                )
            });
            (*rel, body)
        })
        .collect();

    // POSITIVE CONTROL for the query above, in the same call. `known-failing-
    // tests.txt` is legitimately empty today, so "found nothing" proves nothing
    // about the predicate until the predicate is shown to be able to find
    // something.
    let synthetic = "# a comment naming dropping_the_guard\nsome::other::test  reason\n";
    assert!(
        list_names(synthetic, "some::other::test"),
        "the quarantine-list query cannot find a line that IS there, so its \
         verdict on the real lists means nothing"
    );
    assert!(
        !list_names(synthetic, "dropping_the_guard"),
        "the quarantine-list query matched a COMMENT, so every list would read \
         as quarantining everything its prose mentions"
    );

    // POSITIVE CONTROL for the attribute allowlist, in the same call and on
    // synthetic sources, so both polarities are proven before the real twins
    // are graded. A reader that cannot see an added attribute would accept
    // everything, and "the block is exactly #[test]" would mean nothing.
    let clean = "fn a() { }\n#[test]\nfn t(";
    let skipped = "fn a() { }\n#[test]\n#[cfg_attr(any(), ignore)]\nfn t(";
    assert_eq!(
        attrs_before(clean, clean.find("fn t(").expect("synthetic decl")),
        "#[test]",
        "the attribute reader cannot see a bare #[test] block, so its verdict \
         on the real twins means nothing"
    );
    assert_ne!(
        attrs_before(skipped, skipped.find("fn t(").expect("synthetic decl")),
        "#[test]",
        "the attribute reader missed an added attribute, so every skip spelling \
         would read as an unadorned #[test]"
    );

    for (rel, gate, test_name) in BEHAVIOURAL_TWINS {
        let path = tests_root().join(rel);
        let src = std::fs::read_to_string(&path).unwrap_or_else(|e| {
            panic!(
                "{rel} is the behavioural half of FerroxLabs/wayland-core#352 / \
                 FerroxLabs/wayland#1156 and it could not be read ({e}). The \
                 wrapping ratchet in this file cannot distinguish a working \
                 guard from a stub on its own, so deleting or renaming that \
                 file is not a refactor — it is removing the only instrument \
                 that can (FerroxLabs/wayland-core#385 c3)"
            )
        });
        // Comments and literals blanked first, so the twin's own prose quoting
        // `#[ignore]` or a second `#![cfg(` cannot convict it.
        let code = blank_noncode(&src);

        // Whitespace-insensitive, so an attribute cannot hide in its layout.
        let dense = strip_ws(&code);
        assert!(
            dense.contains(&strip_ws(gate)),
            "{rel} no longer carries {gate}. The pair covers every platform \
             only while each half keeps its own gate (wayland-core#385 c3)"
        );
        // ALLOWLIST, not a denylist. `#![cfg_attr(cond, cfg(any()))]` compiles
        // the whole file out on a platform and is not `#![cfg(`, so counting
        // cfg gates could never have caught it; counting ALL inner attributes
        // catches it and everything else of its shape.
        let inner = dense.matches("#![").count();
        assert_eq!(
            inner, 1,
            "{rel} carries {inner} inner attributes; exactly one ({gate}) is \
             allowed. A second one -- in ANY spelling, `#![cfg(`, \
             `#![cfg_attr(`, or an attribute that does not exist yet -- narrows \
             the twin to a platform subset or compiles it out entirely, and \
             leaves a gap no instrument covers (wayland-core#385 c3)"
        );

        let decl = format!("fn {test_name}(");
        let at = code.find(&decl).unwrap_or_else(|| {
            panic!(
                "{rel} no longer declares `{test_name}`, the behavioural test \
                 the wrapping ratchet leans on (wayland-core#385 c3)"
            )
        });
        let attrs = attrs_before(&code, at);
        assert!(
            attrs.contains("#[test]"),
            "`{test_name}` in {rel} is no longer a #[test] (wayland-core#385 c3)"
        );
        // ALLOWLIST again, and the reason it is not a list of forbidden
        // spellings: `#[ignore]`, `#[cfg(...)]`, `#[cfg_attr(cond, ignore)]`,
        // `#[cfg_attr(cond, should_panic)]` and any future skipping attribute
        // are all simply "not `#[test]`", which is decidable. "is a skip" is
        // not.
        assert_eq!(
            attrs, "#[test]",
            "`{test_name}` in {rel} carries attributes beyond `#[test]` -- the \
             block reads as `{attrs}`. Anything other than `#[test]` there can \
             skip the only behavioural proof of tree ownership, quarantine it, \
             or narrow it to a platform (`#[ignore]`, `#[cfg(...)]`, \
             `#[cfg_attr(cond, ignore)]`, `#[cfg_attr(cond, should_panic)]`), \
             so the block is graded as an allowlist rather than against a list \
             of spellings that could never be closed (wayland-core#385 c3)"
        );

        for (list, body) in &quarantine {
            assert!(
                !list_names(body, test_name),
                "{list} names `{test_name}`, which silences or excuses the only \
                 behavioural proof that the guard owns a tree. The wrapping \
                 ratchet cannot cover for it (wayland-core#385 c3)"
            );
        }
    }
}

/// BEHAVIOURAL HALF OF THE RATCHET — FerroxLabs/wayland-core#385 c1/c2.
///
/// The scan above grades WRAPPING. It cannot tell `OwnedTree` from an empty
/// struct, and it stayed green through the entire period the descendant walk
/// was dead. This asks the KERNEL whether the guard actually owns the tree it
/// was handed, on the same fixture the twins use:
///
/// * Unix — the guard's `/proc` walk must SEE the grandchild the fixture
///   created. A stubbed walk returns an empty list and fails here.
/// * Windows — the grandchild must be INSIDE the guard's Job Object. A guard
///   that owns only the leaf fails here.
///
/// Then, on both, dropping the guard must actually kill it. Deliberately NOT a
/// second copy of the twins: they drive the full panic-unwind exit path and the
/// direct child's reaping; this asks only whether the mechanism is alive, which
/// is precisely what the ratchet could not see.
fn assert_the_guard_actually_owns_the_tree() {
    let (guard, grandchild) = spawn_detaching_parent().into_parts();
    let direct = guard.id();

    // Anti-vacuity: a fixture that never got off the ground would let every
    // assertion below pass for the wrong reason.
    assert!(
        process_is_alive(direct),
        "the fixture's direct child {direct} is not running, so nothing below \
         grades the guard"
    );
    assert!(
        process_is_alive(grandchild),
        "the fixture's grandchild {grandchild} is not running, so nothing below \
         grades the guard"
    );

    #[cfg(unix)]
    {
        let seen = support::owned_tree::descendants(direct);
        if !seen.contains(&grandchild) {
            // Clean up before reporting, so a failing assertion cannot leave
            // behind the orphan it is complaining about.
            force_kill(grandchild);
            force_kill(direct);
            panic!(
                "`OwnedTree`'s descendant walk reported {seen:?} for pid \
                 {direct}, which does not include its own grandchild \
                 {grandchild}. The walk is a stub or has stopped reading \
                 /proc, so every guard in this crate owns a LEAF and leaks the \
                 TREE — and the wrapping scan above would still be green \
                 (FerroxLabs/wayland-core#385 c1)"
            );
        }
    }
    #[cfg(windows)]
    {
        let inside = guard
            .job()
            .expect("the guard must hold a Job Object on Windows")
            .contains(grandchild)
            .expect("ask the kernel whether the grandchild is in the job");
        if !inside {
            force_kill(grandchild);
            force_kill(direct);
            panic!(
                "the grandchild {grandchild} is not inside the Job Object \
                 `OwnedTree` built for pid {direct}, so the guard owns the LEAF \
                 and leaks the TREE — and the wrapping scan above would still \
                 be green (FerroxLabs/wayland-core#385 c1)"
            );
        }
    }

    // Ownership is the kill, not the sighting.
    drop(guard);
    let deadline = Instant::now() + Duration::from_secs(10);
    let mut gone = false;
    while Instant::now() < deadline {
        if !process_is_alive(grandchild) {
            gone = true;
            break;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    force_kill(grandchild);
    force_kill(direct);
    assert!(
        gone,
        "the grandchild {grandchild} outlived the guard that owned pid \
         {direct}. The guard sees the tree and does not kill it \
         (FerroxLabs/wayland-core#385 c1)"
    );
}

/// Spawn spellings that hand back a live child process this crate then owns.
///
/// `spawn_command` is `portable_pty`'s, and it is here for the same reason the
/// others are: `support/pty.rs` spawned the product binary through it and
/// killed only the leaf.
///
/// Deliberately NOT matched: `tokio::spawn`, `std::thread::spawn` and
/// `Pty::spawn` — no leading `.`, because they are path calls, and none of them
/// returns an OS process.
const SPAWN_SPELLINGS: &[&str] = &[".spawn()", ".spawn_command("];

/// The floor the sweep landed on. A scan that suddenly sees far fewer sites has
/// stopped reading the tree, and would then pass by seeing nothing at all.
const MINIMUM_KNOWN_SITES: usize = 40;

/// Sites that genuinely may not be owned, each with the reason.
///
/// #352 left no remainder, and an entry here is a deliberate, reviewed
/// exception, not a place to park a failing scan. The bar an entry must clear
/// is that owning the child would DESTROY what the test measures — not that
/// owning it is inconvenient.
const ALLOWED_UNOWNED: &[(&str, u32, &str)] = &[(
    "quarantine_process_tree_windows.rs",
    115,
    "FerroxLabs/wayland-core#393. This spawn is not in a test body: it runs in \
     the re-executed `alias` ROLE, which is the fixture standing in for a `git` \
     credential helper, and the descendant it starts MUST outlive it — that \
     survival is the whole thing #393 is about. `OwnedTree` would kill the \
     descendant as the alias returned and every arm would then assert a death \
     the product did not cause. The lifetime is bounded instead: the descendant \
     sleeps `DESCENDANT_LIFETIME` (300 s) and exits, the liveness control \
     `taskkill /T /F`s the pid it measured, and both graded arms are ASSERTING \
     that the production teardown already killed it.",
)];

/// Replace every comment and string/char literal with spaces, preserving byte
/// offsets and line breaks so reported line numbers stay true.
fn blank_noncode(src: &str) -> String {
    let bytes = src.as_bytes();
    let mut out = vec![b' '; bytes.len()];
    let mut i = 0;
    while i < bytes.len() {
        let keep = |out: &mut Vec<u8>, i: usize| out[i] = bytes[i];
        match bytes[i] {
            b'\n' => {
                out[i] = b'\n';
                i += 1;
            }
            b'/' if i + 1 < bytes.len() && bytes[i + 1] == b'/' => {
                while i < bytes.len() && bytes[i] != b'\n' {
                    i += 1;
                }
            }
            b'/' if i + 1 < bytes.len() && bytes[i + 1] == b'*' => {
                i += 2;
                let mut depth = 1usize;
                while i < bytes.len() && depth > 0 {
                    if bytes[i] == b'\n' {
                        out[i] = b'\n';
                    }
                    if i + 1 < bytes.len() && bytes[i] == b'/' && bytes[i + 1] == b'*' {
                        depth += 1;
                        i += 2;
                    } else if i + 1 < bytes.len() && bytes[i] == b'*' && bytes[i + 1] == b'/' {
                        depth -= 1;
                        i += 2;
                    } else {
                        i += 1;
                    }
                }
            }
            b'r' => {
                // A raw string: `r`, some `#`s, then a quote. Anything else is
                // an ordinary identifier byte.
                let mut j = i + 1;
                while j < bytes.len() && bytes[j] == b'#' {
                    j += 1;
                }
                if j < bytes.len() && bytes[j] == b'"' {
                    let hashes = j - i - 1;
                    let mut k = j + 1;
                    loop {
                        if k >= bytes.len() {
                            i = bytes.len();
                            break;
                        }
                        if bytes[k] == b'\n' {
                            out[k] = b'\n';
                        }
                        if bytes[k] == b'"'
                            && bytes[k + 1..].iter().take(hashes).all(|b| *b == b'#')
                            && k + hashes < bytes.len()
                        {
                            i = k + 1 + hashes;
                            break;
                        }
                        k += 1;
                    }
                } else {
                    keep(&mut out, i);
                    i += 1;
                }
            }
            b'"' => {
                i += 1;
                while i < bytes.len() {
                    match bytes[i] {
                        b'\\' => i += 2,
                        b'"' => {
                            i += 1;
                            break;
                        }
                        b'\n' => {
                            out[i] = b'\n';
                            i += 1;
                        }
                        _ => i += 1,
                    }
                }
            }
            _ => {
                keep(&mut out, i);
                i += 1;
            }
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// Byte ranges covered by an `OwnedTree::new(...)` call, parentheses balanced.
/// `code` must already be [`blank_noncode`]ed.
fn owned_spans(code: &str) -> Vec<(usize, usize)> {
    let bytes = code.as_bytes();
    let mut spans = Vec::new();
    let needle = "OwnedTree::new(";
    let mut from = 0;
    while let Some(rel) = code[from..].find(needle) {
        let open = from + rel + needle.len() - 1;
        let mut depth = 0usize;
        let mut k = open;
        while k < bytes.len() {
            match bytes[k] {
                b'(' => depth += 1,
                b')' => {
                    depth -= 1;
                    if depth == 0 {
                        break;
                    }
                }
                _ => {}
            }
            k += 1;
        }
        spans.push((open, k.min(bytes.len())));
        from = open + 1;
    }
    spans
}

/// Every spawn spelling in `code` that is NOT inside an `OwnedTree::new(...)`,
/// as 1-based line numbers.
fn ungoverned(code: &str) -> Vec<u32> {
    let spans = owned_spans(code);
    let mut out = Vec::new();
    for spelling in SPAWN_SPELLINGS {
        let mut from = 0;
        while let Some(rel) = code[from..].find(spelling) {
            let at = from + rel;
            if !spans.iter().any(|(s, e)| at > *s && at < *e) {
                out.push(code[..at].matches('\n').count() as u32 + 1);
            }
            from = at + 1;
        }
    }
    out.sort_unstable();
    out
}

fn total_sites(code: &str) -> usize {
    SPAWN_SPELLINGS
        .iter()
        .map(|s| code.matches(s).count())
        .sum()
}

fn test_sources() -> Vec<(String, String)> {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests");
    let mut out = Vec::new();
    let mut stack = vec![root.clone()];
    while let Some(dir) = stack.pop() {
        for entry in std::fs::read_dir(&dir)
            .expect("read the test tree")
            .flatten()
        {
            let path = entry.path();
            if path.is_dir() {
                stack.push(path);
            } else if path.extension().is_some_and(|e| e == "rs") {
                let rel = path
                    .strip_prefix(&root)
                    .expect("under tests/")
                    .to_string_lossy()
                    .replace('\\', "/");
                out.push((rel, std::fs::read_to_string(&path).expect("read source")));
            }
        }
    }
    out.sort();
    out
}

/// THE RATCHET. A new spawn site that does not hand its child to `OwnedTree`
/// fails here, naming the file and line.
#[test]
fn every_spawn_site_in_this_crates_tests_hands_its_child_to_the_guard() {
    let mut offenders: Vec<String> = Vec::new();
    let mut sites = 0usize;
    let mut files_with_sites = 0usize;

    for (rel, src) in test_sources() {
        let code = blank_noncode(&src);
        let n = total_sites(&code);
        if n == 0 {
            continue;
        }
        sites += n;
        files_with_sites += 1;
        for line in ungoverned(&code) {
            if ALLOWED_UNOWNED
                .iter()
                .any(|(f, l, _)| *f == rel && *l == line)
            {
                continue;
            }
            offenders.push(format!("{rel}:{line}"));
        }
    }

    // Anti-vacuity: a scan that found nothing would pass every assertion below
    // for the wrong reason. #352 swept 44 sites across 24 files.
    assert!(
        sites >= MINIMUM_KNOWN_SITES,
        "the scan found only {sites} spawn sites, which is below the {MINIMUM_KNOWN_SITES} \
         #352 swept — the scan has stopped reading the test tree, not the tree stopped spawning"
    );
    assert!(
        files_with_sites >= 20,
        "only {files_with_sites} files carried a spawn site; the walk is not reaching the tree"
    );

    assert!(
        offenders.is_empty(),
        "these spawn sites do not hand their child to `OwnedTree`, so nothing kills the \
         process TREE when the test body panics or returns early (FerroxLabs/wayland-core#352, \
         FerroxLabs/wayland#1156):\n  {}\n\nWrap the spawn: \
         `let mut child = OwnedTree::new(cmd.spawn().expect(..));` and add \
         `use support::owned_tree::OwnedTree;`.",
        offenders.join("\n  ")
    );

    // FerroxLabs/wayland-core#385. Everything above grades WRAPPING: it is
    // satisfied by an `OwnedTree` that owns nothing, and it was — it stayed
    // green through the whole period the descendant walk was dead, while the
    // single behavioural test carried the entire claim. These two make THIS
    // test unable to pass in that state, which is what lets #352 / #1156 keep
    // citing it as class closure.
    assert_the_behavioural_twins_are_armed();
    assert_the_guard_actually_owns_the_tree();
}

/// POSITIVE CONTROL for the ratchet above.
///
/// Without this the ratchet could be satisfied by a predicate that never
/// reports anything — which is exactly what a stale scan degrades into. Both
/// polarities are graded on synthetic sources, so neither depends on the state
/// of the real tree.
#[test]
fn the_ratchet_detects_an_ungoverned_site_and_accepts_a_governed_one() {
    let bare = "fn t() { let mut c = Command::new(binary()).spawn().unwrap(); }";
    assert_eq!(
        ungoverned(&blank_noncode(bare)),
        vec![1],
        "an unwrapped spawn must be reported"
    );

    let wrapped = "fn t() { let mut c = OwnedTree::new(Command::new(binary()).spawn().unwrap()); }";
    assert!(
        ungoverned(&blank_noncode(wrapped)).is_empty(),
        "a wrapped spawn must be accepted"
    );

    // The balance walk must not be fooled by a parenthesis inside a message,
    // which would close the span early and report the site as ungoverned.
    let with_parens =
        "fn t() { let mut c = OwnedTree::new(cmd.spawn().expect(\"spawn (the binary)\")); }";
    assert!(
        ungoverned(&blank_noncode(with_parens)).is_empty(),
        "a `(` inside a literal must not end the OwnedTree span"
    );

    // ...and a spawn merely MENTIONED in a comment or a string is not a site.
    let quoted = "fn t() { let s = \"x.spawn()\"; /* y.spawn() */ }";
    assert!(
        ungoverned(&blank_noncode(quoted)).is_empty(),
        "a spawn spelling inside a literal or comment must not count as a site"
    );
    assert_eq!(
        total_sites(&blank_noncode(quoted)),
        0,
        "blanking must remove quoted spawn spellings from the count too"
    );

    // The pty spelling is graded by the same rule.
    let pty = "fn t() { let c = pair.slave.spawn_command(cmd).unwrap(); }";
    assert_eq!(
        ungoverned(&blank_noncode(pty)),
        vec![1],
        "an unwrapped `spawn_command` must be reported"
    );
}
