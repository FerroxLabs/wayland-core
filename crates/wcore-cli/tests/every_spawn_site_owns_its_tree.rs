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
//! ## Why source text and not a runtime check
//!
//! There is no runtime hook that could see this. Each integration test file is
//! its own binary, the leak is observable only as a process still alive after
//! the binary exited, and nothing inside the process can observe its own
//! afterlife. The information is in the source or nowhere.
//!
//! ## Why it is not fooled by its own text
//!
//! [`blank_noncode`] replaces every comment and every string/char literal with
//! spaces before anything is matched. So the spawn spellings quoted in this
//! module's own prose and fixtures are invisible to it, a `;` inside an
//! `expect("...")` message cannot move a statement boundary, and a `(` inside
//! one cannot unbalance the parenthesis walk.

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
/// Empty, and that is the point: #352 left no remainder. An entry here is a
/// deliberate, reviewed exception, not a place to park a failing scan.
const ALLOWED_UNOWNED: &[(&str, u32, &str)] = &[];

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
        for entry in std::fs::read_dir(&dir).expect("read the test tree").flatten() {
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

    let wrapped =
        "fn t() { let mut c = OwnedTree::new(Command::new(binary()).spawn().unwrap()); }";
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
