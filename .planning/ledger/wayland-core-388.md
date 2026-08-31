---
issue: 388
repo: FerroxLabs/wayland-core
kind: defect
title: "GitTool reconstructs VCS content-store bytes in a Contained workspace (split from #244 c3)"
status: open
last_verified_commit: 30fd6cfde
criteria:
  - id: c1
    text: "— In a `WorkspacePolicy::contained` workspace, `Git(op=diff, rev=…)` does not return the content of a file that `is_secret_path_static` denies, whether or not that file is named in `path`. The whole-repo form is the one that matters: it needs no path argument."
    state: met
    evidence: test:crates/wcore-tools/tests/git_content_store_deny.rs::a_contained_diff_withholds_a_denied_files_hunks_and_says_so
    owner: core
    note: "MEASURED on hetzner at lane/f13-vcs-store. `cargo test -p wcore-tools --test git_content_store_deny` -> 5 passed. `a_contained_diff_withholds_a_denied_files_hunks_and_says_so` drives the WHOLE-REPO form (`Git(op=diff, rev=HEAD~1)`, no `path`) through `execute_with_ctx` -- the production entry point -- in a real git repo whose object store holds the only copy of `.env`, and the canary WLCANARY-COMMITTED-388 is absent. `a_contained_diff_withholds_a_denied_file_named_in_path` covers the named-path form. RED ARM, compiled first (`cargo check -p wcore-tools --tests` clean): delete the filter from `GitTool::execute_with_ctx` and 4 of the 5 arms go red with the canary in the returned diff; the status/log control stays green."
  - id: c2
    text: "— The withholding is reported, not silent: the caller is told a file's hunks were withheld and which file, in the same shape `grep_policy`'s footer uses. A diff that silently drops a hunk is a diff the model will reason from as if it were complete."
    state: met
    evidence: symbol:crates/wcore-tools/src/git.rs::withhold_denied_hunks
    owner: core
    note: "The `diff --git` header is KEPT and the hunks replaced by `[Git] hunks withheld: .env is denied for content reads in this workspace posture`, with a footer `[Git] 1 file(s)' hunks withheld (.env)` in the shape `grep_policy::Filtered::footer` uses. Asserted by the same test (`out.contains(\"hunks withheld\") && out.contains(\".env\")`)."
  - id: c3
    text: "— The wrong-refusal control holds: an ordinary source file's hunks still come back from the same `git diff` invocation, and `Git(op=status)` / `Git(op=log)` are unaffected."
    state: met
    evidence: test:crates/wcore-tools/tests/git_content_store_deny.rs::contained_status_and_log_are_unaffected
    owner: core
    note: "Wrong-refusal control asserted in the SAME invocation: `src/main.rs`'s hunks carry WLCANARY-ORDINARY-OK and come back. `contained_status_and_log_are_unaffected` pins `Git(op=status)` (branch reported) and `Git(op=log)` (both commits listed, and no footer, because nothing was withheld)."
  - id: c4
    text: "— `Git(op=blame)` is graded under the same posture, with a fixture where the path DOES exist in the named revision (the probe above hit `fatal: no such path`, so blame is currently *ungraded*, not *proven safe*)."
    state: met
    evidence: test:crates/wcore-tools/tests/git_content_store_deny.rs::a_contained_blame_of_a_denied_path_is_refused_and_reported
    owner: core
    note: "`a_contained_blame_of_a_denied_path_is_refused_and_reported`, with the fixture the ticket asked for: a KNOWN-POSITIVE control first runs `git blame -L 1,1 HEAD~1 -- .env` directly and asserts it succeeds AND prints the secret, so the path really does exist in that revision and the refusal below is the filter's rather than git's. Blame is refused BEFORE `git` runs, because its output IS the committed line and there is no hunk to strip. Wrong-refusal control: `src/main.rs` still blames."
  - id: c5
    text: "— The posture boundary is explicit and tested: whatever is decided for `Trusted` (Sean's #667 carve-out) is different from what is decided for `Contained`, and a test pins each."
    state: met
    evidence: test:crates/wcore-tools/tests/git_content_store_deny.rs::the_posture_decides_and_trusted_local_is_left_alone
    owner: core
    note: "`the_posture_decides_and_trusted_local_is_left_alone` asserts BOTH directions on the same repository and the same call: `trusted_local` returns the secret (Sean's #667 carve-out preserved) and `contained` withholds, with an `assert_ne!` between the two so a filter that withheld everywhere cannot pass. The boundary is `WorkspacePolicy::secret_read_deny_required` and NOT `denies_read_content`, because `is_project_secret_resolved` is unconditional on posture -- #667's carve-out is expressed by not INSTALLING `SecretDenyFs`, which `GitTool` has no equivalent of. Recorded on the call site."
---

Created 2026-08-31 to close a COVERAGE gap. It records no work as done.

`scripts/check-criteria-ledger.py` scopes every open `area:core` issue on
wayland and EVERY open issue on wayland-core. This issue was in scope from
the moment it was filed and had no ledger file, so
`scripts/check-release-readiness.py` -- which reads ledger files and nothing
else -- could not count it. CI runs the coverage gate with `--offline`, the
arm that would have reported the gap, so nothing said so for two days.

Criteria are transcribed from the issue body without edit. Where the body's
wording is loose it is LEFT loose rather than tightened here: sharpening a
criterion inside the ledger is how a criterion quietly becomes an easier
adjacent property. Whoever takes this restates it on the ISSUE first.
