---
issue: 377
repo: FerroxLabs/wayland-core
kind: defect
title: "@dir with an escaping spelling resolves to a silently empty payload: no files, no error, no warning"
status: open
last_verified_commit: 9de21aa1
criteria:
  - id: c1
    text: "@../repo/ and @/abs/outside/ either attach their files or are REFUSED with a message the user sees -- never Ok with files: []"
    state: not-met
    owner: core
    note: "Filed 2026-08-29 by the 0.13.12 close-sweep (worklist defect D3, found while verifying FerroxLabs/wayland-core#335 — '@-ref: absolute paths escape the workspace root and skip the gitignore check'). Nothing has been done. The measured finding, verbatim: `@dir` with any spelling that escapes the lexical root resolves to a SILENTLY EMPTY payload — no error, no warning, no skipped-file count. Measured on origin/integ/f13 (5eb2d1ef), not modelled: `@../repo/` against a workspace containing `top.txt` and `src/a.rs` returns `Ok(AtPayload { kind: Dir, files: [], text: '', warnings: [] })`; `@/abs/path/outside/` against a directory containing `a.txt` and `b.txt` returns the same. Control in the same run: `@<abs path of the workspace root>/` correctly returns `files: [a.txt]`, so the query is not broken — only the escaping spellings vanish."
  - id: c2
    text: "The `continue` in walk_dir that drops an entry increments the skipped counter, so AtWarning::SkippedFiles is emitted whenever any entry is dropped"
    state: not-met
    owner: core
    note: "Filed 2026-08-29 by the 0.13.12 close-sweep (worklist defect D3). Nothing has been done; the measurement is on c1 and the file:line anchors are in the prose below."
  - id: c3
    text: "A test drives resolve() for both escaping spellings and asserts the outcome, shown RED against today's code"
    state: not-met
    owner: core
    note: "Filed 2026-08-29 by the 0.13.12 close-sweep (worklist defect D3). Nothing has been done; the measurement is on c1 and the file:line anchors are in the prose below."
  - id: c4
    text: "The control stays green: @<absolute path of the workspace root>/ still attaches its files"
    state: not-met
    owner: core
    note: "Filed 2026-08-29 by the 0.13.12 close-sweep (worklist defect D3). Nothing has been done; the measurement is on c1 and the file:line anchors are in the prose below."
---

`@dir` with any spelling that escapes the lexical root resolves to a SILENTLY EMPTY payload — no error, no warning, no skipped-file count. Measured on origin/integ/f13 (5eb2d1ef), not modelled: `@../repo/` against a workspace containing `top.txt` and `src/a.rs` returns `Ok(AtPayload { kind: Dir, files: [], text: '', warnings: [] })`; `@/abs/path/outside/` against a directory containing `a.txt` and `b.txt` returns the same. Control in the same run: `@<abs path of the workspace root>/` correctly returns `files: [a.txt]`, so the query is not broken — only the escaping spellings vanish.

**Where.** crates/wcore-cli/src/tui/commands/at_ref_resolve.rs:339 — `let rel = match rel_to_root(&path, root) { Some(r) => r, None => continue };` inside `walk_dir`. `resolve_dir` walks `full = resolve_under_root(path, root)`, but every entry is then stripped against the NON-canonical `root`, so an escaping spelling makes `rel_to_root` return `None` for every entry. The `continue` does NOT do `*skipped += 1`, so no `AtWarning::SkippedFiles` is emitted either. The empty payload then reaches `render_payload` (crates/wcore-cli/src/tui/commands/at_ref_send.rs:254-259), whose `payload.files.is_empty()` branch is commented 'shouldn't happen for File/Dir, but be defensive' and emits a bare `▌ <label>` with an empty body.

**Why it matters.** Reachable straight from user composer text — `AtRef::parse('@/foo/bar/')` and `@../other/` both parse to `AtRef::Dir` (at_ref_parse.rs:137). The user attaches a directory, is shown a label as if it succeeded, and the model receives nothing. Silent context loss with a false success signal. It also directly contradicts the Q1 decision this issue exists to implement: 'A — leave escaping attachments working' is delivered for `@file` (proven: `@../outside/note.log` attaches) but not for `@dir`, where an escaping spelling neither attaches nor is refused. Note the code comment proves the authors never expected an empty Dir payload — the exact state an escaping spelling produces.

Criteria are taken verbatim from the issue's Acceptance section. Nothing has been done: this entry exists so the release gate counts the work rather than anyone having to remember it.
