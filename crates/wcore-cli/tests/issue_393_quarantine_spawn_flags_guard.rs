//! `FerroxLabs/wayland-core#393` — the ownership DECISION, graded on a host
//! that runs.
//!
//! # Why this file exists
//!
//! #393's two behavioural tests — `quarantine_process_tree_windows.rs` and
//! `quarantine_console_authority_windows.rs` — are both `#![cfg(windows)]` and
//! therefore compile to zero tests on every host our gates execute on today.
//! While that is true the fix can be deleted and every green stays green.
//!
//! This file does NOT replace them and cannot. It grades a strictly smaller
//! property, and the boundary is stated rather than implied:
//!
//! * WHAT IS GRADED HERE: the *decision* — that the flags the quarantine spawn
//!   is made with are the OR of `DETACHED_PROCESS` (#338) and `CREATE_SUSPENDED`
//!   (#393), that those two symbols carry their real Win32 values, and that the
//!   composed constant is applied at exactly one spawn site with nothing
//!   re-setting it afterwards.
//! * WHAT IS NOT GRADED HERE, ON ANY HOST BUT WINDOWS: that the flags reach the
//!   kernel and have their effect. `std::process::Command` has no
//!   `creation_flags` on Unix at all, so on this host the flags are never
//!   applied to anything. The child having no console (#338 c1) and the job
//!   owning every descendant (#393 c1/c2) are only observable from a real
//!   Windows process, and they stay `not-met` until SeanDesktop runs them.
//!
//! # Why the value arm is worth having even though it cannot fail on its own
//!
//! `assert_eq!(QUARANTINE_SPAWN_FLAGS, 0x8 | 0x4)` has no oracle independent of
//! the constant; it can only fail when someone EDITS the constant. That is
//! exactly what it is for, and it catches one class the source text cannot:
//! `DETACHED_PROCESS` mistyped as `0x0000_0010` still reads as
//! `DETACHED_PROCESS | CREATE_SUSPENDED` in every scan, and `0x10` is
//! `CREATE_NEW_CONSOLE` — the precise inverse of the #338 reduction, spelled
//! with the right words. The bit values below are the Win32 ground truth
//! (`processthreadsapi.h` / `CreateProcessW` dwCreationFlags), not a copy of
//! what the product happens to say.
//!
//! # Why the wiring arm is a source scan, and what that costs
//!
//! `creation_flags` is a SETTER, not an OR. The regression #393's constant
//! exists to prevent is a SECOND `creation_flags` call landing after the first
//! and silently dropping `DETACHED_PROCESS`. Nothing inside a Unix process can
//! observe a Windows creation flag, so whether there is exactly one such call
//! on the production path is in the source or nowhere — the same argument
//! `every_spawn_site_owns_its_tree.rs` makes for its wrapping ratchet, and the
//! same cost: a deliberate refactor of the spawn path reds this file and has to
//! be re-argued here. That is what a ratchet is.
//!
//! It is NOT fooled by its own subject's prose: `quarantine.rs` names
//! `creation_flags` five times in comments before it calls it twice in code, so
//! [`blank_noncode`] replaces every comment and every string/char literal with
//! spaces before anything is matched. Both polarities of that reader are proven
//! on synthetic sources in the same test call, because a scanner that finds
//! nothing reads exactly like a clean tree.
//!
//! `blank_noncode` is a second copy of the one in
//! `every_spawn_site_owns_its_tree.rs`. Each integration test is its own
//! binary, that file is live on this lane, and hoisting it into `tests/support/`
//! is a change to a file this ticket has no business touching — so it is
//! duplicated deliberately and named here rather than quietly.

use wcore_cli::plugin::quarantine::{DETACHED_PROCESS, QUARANTINE_SPAWN_FLAGS};

/// `CreateProcessW` `dwCreationFlags`, from the Win32 headers — the ground
/// truth the product's constants are pinned AGAINST, not a restatement of them.
const WIN32_DETACHED_PROCESS: u32 = 0x0000_0008;
const WIN32_CREATE_SUSPENDED: u32 = 0x0000_0004;
const WIN32_CREATE_NEW_CONSOLE: u32 = 0x0000_0010;

const QUARANTINE_SRC: &str = "crates/wcore-cli/src/plugin/quarantine.rs";

fn quarantine_source_path() -> std::path::PathBuf {
    std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src/plugin/quarantine.rs")
}

/// #393 c3, the half that is arithmetic rather than kernel behaviour.
///
/// The trap the ticket names is that composing the two flags by calling
/// `creation_flags` twice drops one of them. The fix is a single OR. This
/// grades the OR — including that the two bits are the bits they are named
/// after, which is the mutation a source scan is blind to.
#[test]
fn the_quarantine_spawn_flags_are_both_win32_bits_ored_together() {
    assert_eq!(
        DETACHED_PROCESS, WIN32_DETACHED_PROCESS,
        "quarantine.rs's DETACHED_PROCESS is {DETACHED_PROCESS:#010x}, but \
         CreateProcessW's DETACHED_PROCESS is {WIN32_DETACHED_PROCESS:#010x}. \
         The name is not the flag: {WIN32_CREATE_NEW_CONSOLE:#010x} is \
         CREATE_NEW_CONSOLE, which gives the quarantine child a console of its \
         own and is the exact inverse of the reduction \
         FerroxLabs/wayland-core#338 shipped"
    );

    assert_eq!(
        QUARANTINE_SPAWN_FLAGS & DETACHED_PROCESS,
        DETACHED_PROCESS,
        "QUARANTINE_SPAWN_FLAGS ({QUARANTINE_SPAWN_FLAGS:#010x}) does not \
         contain DETACHED_PROCESS ({DETACHED_PROCESS:#010x}). It is applied at \
         the quarantine spawn site AFTER harden_against_credential_prompt has \
         set DETACHED_PROCESS, and creation_flags OVERWRITES — so a composed \
         value that is not a superset of the hardening's own value silently \
         REVOKES it and reopens FerroxLabs/wayland-core#338's Windows console \
         reduction (#393 c3)"
    );

    assert_eq!(
        QUARANTINE_SPAWN_FLAGS & WIN32_CREATE_SUSPENDED,
        WIN32_CREATE_SUSPENDED,
        "QUARANTINE_SPAWN_FLAGS ({QUARANTINE_SPAWN_FLAGS:#010x}) does not \
         contain CREATE_SUSPENDED ({WIN32_CREATE_SUSPENDED:#010x}). Without it \
         the child runs before WindowsJobObject::attach can take it, and every \
         descendant it creates in that window escapes the job permanently — \
         which is the leak #393 reports"
    );

    assert_eq!(
        QUARANTINE_SPAWN_FLAGS,
        WIN32_DETACHED_PROCESS | WIN32_CREATE_SUSPENDED,
        "QUARANTINE_SPAWN_FLAGS is {QUARANTINE_SPAWN_FLAGS:#010x}, which is not \
         exactly DETACHED_PROCESS | CREATE_SUSPENDED. A third flag here is a \
         creation-time decision about the quarantine child's console, job or \
         priority that nothing in this repo has argued for; adding one is a \
         deliberate edit to this test as well (#393 c3)"
    );

    // Anti-vacuity for the two masking assertions above: a zero mask satisfies
    // `x & 0 == 0` for every x, so prove the bits are distinct and non-zero
    // before the masks are believed.
    assert_ne!(WIN32_DETACHED_PROCESS, 0);
    assert_ne!(WIN32_CREATE_SUSPENDED, 0);
    assert_ne!(
        WIN32_DETACHED_PROCESS & WIN32_CREATE_SUSPENDED,
        WIN32_DETACHED_PROCESS,
        "the two flags overlap, so one mask would pass for the other"
    );
}

/// #393's wiring: the composed constant is applied ONCE, at the production
/// spawn site, and nothing re-sets `creation_flags` after it.
///
/// This is the assertion that goes red when the fix is deleted. The value arm
/// above stays green when `run_hardened` stops applying the constant at all.
#[test]
fn the_composed_flags_are_applied_once_at_the_single_quarantine_spawn_site() {
    // ── Positive controls, before the real file is read ──────────────────
    //
    // Both polarities, on synthetic sources. A reader that sees nothing would
    // report a clean tree for a file with three calls in it; a reader that
    // sees comments would convict this file's own prose.
    let synthetic = concat!(
        "fn a(cmd: &mut C) {\n",
        "    // cmd.creation_flags(FROM_A_COMMENT);\n",
        "    let s = \"cmd.creation_flags(FROM_A_STRING)\";\n",
        "    cmd.creation_flags(REAL_ONE);\n",
        "}\n",
        "fn b(cmd: &mut C) {\n",
        "    cmd.creation_flags(REAL_TWO | ALSO);\n",
        "}\n",
    );
    let seen = creation_flag_calls(&blank_noncode(synthetic));
    let args: Vec<&str> = seen.iter().map(|(_, a)| a.as_str()).collect();
    assert_eq!(
        args,
        ["REAL_ONE", "REAL_TWO | ALSO"],
        "the creation_flags reader is wrong on a synthetic source: it must find \
         BOTH real calls (a reader that stops at the first would miss the \
         second one that overwrites it) and NEITHER the commented nor the \
         quoted one (a reader that sees prose would convict quarantine.rs for \
         its own doc comments). Its verdict on the real file means nothing \
         until both directions hold"
    );

    // ── The real file ────────────────────────────────────────────────────
    let path = quarantine_source_path();
    let src = std::fs::read_to_string(&path).unwrap_or_else(|e| {
        panic!(
            "{QUARANTINE_SRC} could not be read ({e}). An unreadable file reads \
             exactly like a file with no offending call in it, so this fails \
             rather than passes"
        )
    });
    assert!(
        src.len() > 40_000,
        "{QUARANTINE_SRC} is only {} bytes. It was 61,795 when this guard was \
         written; a file this small is a truncated or relocated read, and an \
         empty scan of it would pass",
        src.len()
    );
    let code = blank_noncode(&src);

    let harden = fn_body_span(&code, "pub fn harden_against_credential_prompt(");
    let hardened_run = fn_body_span(&code, "pub fn run_hardened(");

    let calls = creation_flag_calls(&code);
    assert_eq!(
        calls.len(),
        2,
        "{QUARANTINE_SRC} makes {} creation_flags calls; exactly two are \
         allowed. creation_flags is a SETTER, not an OR: every additional call \
         OVERWRITES whatever the previous one set, and the flag it drops when \
         the last writer is not QUARANTINE_SPAWN_FLAGS is DETACHED_PROCESS — \
         which reopens FerroxLabs/wayland-core#338. Found: {:?}",
        calls.len(),
        calls.iter().map(|(_, a)| a).collect::<Vec<_>>()
    );

    // The hardening's own call: DETACHED_PROCESS alone, so a caller that runs
    // the command itself still gets the #338 reduction.
    let in_harden: Vec<&(usize, String)> =
        calls.iter().filter(|(at, _)| harden.contains(at)).collect();
    assert_eq!(
        in_harden.len(),
        1,
        "harden_against_credential_prompt makes {} creation_flags calls; \
         exactly one is allowed",
        in_harden.len()
    );
    assert_eq!(
        in_harden[0].1, "DETACHED_PROCESS",
        "harden_against_credential_prompt sets creation_flags({}) rather than \
         the DETACHED_PROCESS constant this test pins by value. The superset \
         relation #393 c3 rests on — QUARANTINE_SPAWN_FLAGS contains what the \
         hardening sets — is over that symbol, and an inlined literal here \
         leaves the two free to drift apart",
        in_harden[0].1
    );

    // The production spawn site: the composed constant, applied last, before
    // the spawn it governs.
    let in_run: Vec<&(usize, String)> = calls
        .iter()
        .filter(|(at, _)| hardened_run.contains(at))
        .collect();
    assert_eq!(
        in_run.len(),
        1,
        "run_hardened makes {} creation_flags calls; exactly one is allowed, \
         because the last writer wins and a second one here is the #393 trap \
         itself",
        in_run.len()
    );
    assert_eq!(
        in_run[0].1, "QUARANTINE_SPAWN_FLAGS",
        "run_hardened sets creation_flags({}) rather than the composed \
         QUARANTINE_SPAWN_FLAGS constant. This is the ONE spawn site #393's \
         fix applies the OR at; anything else here either drops a flag or \
         moves the composition somewhere this guard cannot see",
        in_run[0].1
    );

    let spawn_at = code[hardened_run.clone()]
        .find(".spawn()")
        .map(|rel| hardened_run.start + rel)
        .unwrap_or_else(|| {
            panic!(
                "run_hardened no longer contains a `.spawn()`. It is the single \
                 quarantine spawn site the composed flags are applied at, and a \
                 guard that cannot find it is grading nothing (#393)"
            )
        });
    assert!(
        in_run[0].0 < spawn_at,
        "run_hardened applies the composed creation flags AFTER its `.spawn()`. \
         Creation flags are read by CreateProcessW at spawn time, so a call \
         that lands afterwards sets nothing and the child is created with \
         whatever the hardening left — no CREATE_SUSPENDED, and every \
         descendant born before WindowsJobObject::attach escapes the job (#393)"
    );

    // `WindowsJobObject::create_suspended` is the other writer of this same
    // field, and #393's doc argues at length that it is NOT called here.
    // Reading that argument out of the source is what makes it checkable.
    assert!(
        !code.contains("create_suspended("),
        "{QUARANTINE_SRC} calls WindowsJobObject::create_suspended, which is a \
         second `command.creation_flags(..)` under another name. Whichever of \
         it and QUARANTINE_SPAWN_FLAGS runs last silently erases the other, and \
         when the job wins the flag it erases is DETACHED_PROCESS — the #338 \
         reduction. The composition is deliberately done by the OR in \
         QUARANTINE_SPAWN_FLAGS instead (#393 c3)"
    );
}

/// The byte range of the BODY of the function declared by `decl`, braces
/// matched.
///
/// Brace-matched rather than "to the next `}` in column 0", because #393's
/// ownership split lives in two METHODS -- `HardenedTree::disarm` and
/// `HardenedTree::drop` -- whose closing braces are indented. `code` must
/// already be [`blank_noncode`]ed, so a brace inside a comment or a string
/// literal can neither open nor close a body.
///
/// `decl` must be UNIQUE in the file: a second function of the same name would
/// otherwise silently hand back the wrong body and every assertion over it
/// would be about code nobody meant to grade.
fn fn_body_span(code: &str, decl: &str) -> std::ops::Range<usize> {
    let hits = code.matches(decl).count();
    assert_eq!(
        hits, 1,
        "{QUARANTINE_SRC} declares `{decl}` {hits} times; this guard addresses \
         it by name and can only be trusted while that name resolves to one \
         body (#393)"
    );
    let start = code.find(decl).expect("counted above");
    let bytes = code.as_bytes();
    let open = start
        + code[start..]
            .find('{')
            .unwrap_or_else(|| panic!("`{decl}` in {QUARANTINE_SRC} has no body"));
    let mut depth = 0usize;
    let mut k = open;
    let close = loop {
        assert!(
            k < bytes.len(),
            "`{decl}` in {QUARANTINE_SRC} has an unbalanced body"
        );
        match bytes[k] {
            b'{' => depth += 1,
            b'}' => {
                depth -= 1;
                if depth == 0 {
                    break k;
                }
            }
            _ => {}
        }
        k += 1;
    };
    open + 1..close
}

/// Every `.creation_flags(<arg>)` call in `code`, as `(byte offset, argument)`.
///
/// Parentheses are balanced, so a nested call in the argument cannot truncate
/// it. `code` must already be [`blank_noncode`]ed.
fn creation_flag_calls(code: &str) -> Vec<(usize, String)> {
    let bytes = code.as_bytes();
    let needle = "creation_flags(";
    let mut out = Vec::new();
    let mut from = 0;
    while let Some(rel) = code[from..].find(needle) {
        let open = from + rel + needle.len() - 1;
        let mut depth = 0usize;
        let mut k = open;
        let close = loop {
            if k >= bytes.len() {
                panic!("unbalanced parentheses after a creation_flags( at byte {open}");
            }
            match bytes[k] {
                b'(' => depth += 1,
                b')' => {
                    depth -= 1;
                    if depth == 0 {
                        break k;
                    }
                }
                _ => {}
            }
            k += 1;
        };
        let arg = code[open + 1..close]
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(" ");
        out.push((from + rel, arg));
        from = close + 1;
    }
    out
}

/// Replace every comment and string/char literal with spaces, preserving byte
/// offsets and line breaks.
///
/// A second copy of the reader in `every_spawn_site_owns_its_tree.rs` — see
/// this module's doc for why it is duplicated rather than hoisted.
fn blank_noncode(src: &str) -> String {
    let bytes = src.as_bytes();
    let mut out = vec![b' '; bytes.len()];
    let mut i = 0;
    while i < bytes.len() {
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
                    out[i] = bytes[i];
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
                out[i] = bytes[i];
                i += 1;
            }
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// #393's ownership split, which is the half of the fix that decides whether a
/// SUCCESSFUL install is a product regression.
///
/// `HardenedTree` holds a kill-on-close Job Object. Two exits, opposite
/// actions, and swapping them is silent:
///
/// * `disarm` is the ONE site that claims a tree is finished. It must
///   `release` the job — merely dropping the handle kills the tree, and the
///   tree at that point includes `git-credential-cache--daemon`, which is
///   shared with the operator's other `git` operations.
/// * `drop` is every failing exit, including one nobody has written yet. It
///   must `terminate`.
///
/// # What this can and cannot see, said plainly
///
/// The whole Windows half of that struct is `#[cfg(windows)]` and the type it
/// holds — `wcore_types::job_object::WindowsJobObject` — is behind a
/// `#![cfg(windows)]` module, so NONE of it compiles here and no Linux test can
/// execute it. Making it executable would mean generifying `HardenedTree` over
/// a trait and driving a Linux fake, which grades the fake.
///
/// So this grades the DECISION from source, in the same terms as the flags
/// above: which of the two calls appears on which exit. It cannot see a
/// `release` that has stopped releasing — that is #393 c1/c2 and needs the box.
/// What it does close is the swap and the deletion, which are the edits a
/// reviewer reading two adjacent `if let Some(job)` blocks is least likely to
/// catch.
#[test]
fn the_job_is_released_on_the_finished_exit_and_terminated_on_every_other() {
    // Positive control for the body reader, on a synthetic with two adjacent
    // methods: it must return the SECOND one's body when asked for it, and not
    // run on into or back out of its neighbour. A reader that returned the
    // whole impl block would find both calls in both bodies and could never
    // detect the swap this test exists for.
    let synthetic = "impl T {\n    fn a(&mut self) {\n        x.release();\n    }\n    fn b(&mut self) {\n        if q { x.terminate(); }\n    }\n}\n";
    let b = fn_body_span(synthetic, "fn b(&mut self)");
    assert!(
        synthetic[b.clone()].contains("x.terminate()") && !synthetic[b].contains("x.release()"),
        "the body reader cannot separate two adjacent methods, so its verdict \
         on disarm and drop means nothing"
    );

    let src = std::fs::read_to_string(quarantine_source_path())
        .unwrap_or_else(|e| panic!("{QUARANTINE_SRC} could not be read ({e})"));
    let code = blank_noncode(&src);

    let disarm = &code[fn_body_span(&code, "fn disarm(&mut self)")];
    let drop_body = &code[fn_body_span(&code, "fn drop(&mut self)")];

    assert!(
        disarm.contains("job.release()"),
        "HardenedTree::disarm does not release the Job Object. disarm is the \
         one site that claims a tree is FINISHED rather than abandoned; a job \
         handle merely dropped there kills on close, so a SUCCESSFUL plugin \
         install would take out git-credential-cache--daemon with it — a \
         process shared with the operator's other git operations (#393)"
    );
    assert!(
        !disarm.contains("job.terminate()"),
        "HardenedTree::disarm TERMINATES the Job Object. That is the failing \
         exit's action on the succeeding exit: every completed install would \
         kill the tree it just finished with, including git's shared \
         credential daemon (#393)"
    );
    assert!(
        drop_body.contains("job.terminate()"),
        "HardenedTree::drop does not terminate the Job Object. Drop is every \
         failing exit — the wall clock, the drain grace, the try_wait error \
         FerroxLabs/wayland-core#379 did not name, and any exit added later — \
         and without the terminate each of them reaps the leaf and leaves \
         every helper git spawned running, which is the whole of #393 c1"
    );
    assert!(
        !drop_body.contains("job.release()"),
        "HardenedTree::drop RELEASES the Job Object. Release is the finished \
         exit's action on the abandoned one: it hands the tree its freedom on \
         exactly the paths #393 exists to kill it on"
    );
    // Both take() the handle rather than borrowing it, so neither exit can act
    // twice and disarm's release cannot be followed by drop's terminate.
    for (name, body) in [("disarm", disarm), ("drop", drop_body)] {
        assert!(
            body.contains("self.job.take()"),
            "HardenedTree::{name} does not `take()` the job out of the guard. \
             disarm runs and is then followed by drop on the same value, so a \
             handle left in place lets the release be overtaken by a terminate \
             (#393)"
        );
    }
    // The unix half of the same ownership, so a mutation that guts the Windows
    // arm cannot pass by having gutted the other one too.
    assert!(
        drop_body.contains("terminate_hardened_tree("),
        "HardenedTree::drop no longer tears down the process GROUP either, so \
         the unix arm of FerroxLabs/wayland-core#379 is gone with it"
    );
}
