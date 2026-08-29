---
issue: 1204
repo: FerroxLabs/wayland
kind: defect
title: "The #1162 error message ships with runs of literal spaces mid-sentence, and the same collapse appears in acp.rs and main.rs"
status: open
last_verified_commit: 9de21aa1
criteria:
  - id: c1
    text: "The message at cache_cmd.rs:486 and the sibling bail at :476 read as one sentence with single spaces between words"
    state: not-met
    owner: core
    note: "Filed 2026-08-29 by the 0.13.12 close-sweep (worklist defect D22, found while verifying wayland#1162). Nothing has been done. The measured finding, verbatim: The user-facing error message that #1162 was specifically asked to improve ships with mangled inter-word whitespace: runs of ~10 and ~22 literal spaces mid-sentence. Observed verbatim from the built binary: `Error: no cache ledger for 'aa55aa55-0002' in <dir>. Ledgers are keyed by the engine's internal conversation id, not by the session id you set with --session-id; session 'aa55aa55-0002' is either unknown to the session store at <dir> or was recorded by a build that did not persist its conversation id. Run \`wayland-core cache list\` to see the ids that exist.` Confirmed literal in the source with `cat -A`, not a terminal artifact — the string is a single-line literal whose `\`-newline continuations were collapsed while keeping the source indentation. Wider pattern, same shape, at crates/wcore-cli/src/acp.rs:266 and crates/wcore-cli/src/main.rs:8616 and :8624."
  - id: c2
    text: "A test asserts the message SHAPE, not only that it contains 'cache list'"
    state: not-met
    owner: core
    note: "Filed 2026-08-29 by the 0.13.12 close-sweep (worklist defect D22). Nothing has been done; the measurement is on c1 and the file:line anchors are in the prose below."
  - id: c3
    text: "The same collapse at acp.rs:266 and main.rs:8616/:8624 is fixed in the same pass, or a lint refuses a user-facing string literal containing three or more consecutive spaces"
    state: not-met
    owner: core
    note: "Filed 2026-08-29 by the 0.13.12 close-sweep (worklist defect D22). Nothing has been done; the measurement is on c1 and the file:line anchors are in the prose below."
---

The user-facing error message that #1162 was specifically asked to improve ships with mangled inter-word whitespace: runs of ~10 and ~22 literal spaces mid-sentence. Observed verbatim from the built binary: `Error: no cache ledger for 'aa55aa55-0002' in <dir>. Ledgers are keyed by the engine's internal conversation id, not by the session id you set with --session-id; session 'aa55aa55-0002' is either unknown to the session store at <dir> or was recorded by a build that did not persist its conversation id. Run \`wayland-core cache list\` to see the ids that exist.` Confirmed literal in the source with `cat -A`, not a terminal artifact — the string is a single-line literal whose `\`-newline continuations were collapsed while keeping the source indentation. Wider pattern, same shape, at crates/wcore-cli/src/acp.rs:266 and crates/wcore-cli/src/main.rs:8616 and :8624.

**Where.** crates/wcore-cli/src/cache_cmd.rs:486 (and the sibling bail at :476, `...but its ledger could not be read: {e}`)

**Why it matters.** The ticket's own fix shape says 'at minimum make the error name the real key and point at `cache list`, so the message stops implying the run was never recorded.' This message is the deliverable, and it is the first thing a scripted/CI operator sees on the failure path. No test asserts on its shape — `an_unknown_session_id_still_fails_and_says_where_to_look` only checks `err.contains('cache list')`, which a mangled string satisfies. Cosmetic, not functional; it does not block closing #1162, but it should be filed rather than shipped silently, and the same collapse in acp.rs/main.rs suggests a tooling pass did this repo-wide.

Criteria are taken verbatim from the issue's Acceptance section. Nothing has been done: this entry exists so the release gate counts the work rather than anyone having to remember it.
