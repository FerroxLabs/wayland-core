---
issue: 1197
repo: FerroxLabs/wayland
kind: defect
title: "The #1134 CI lint is structurally blind to the defect shape it was filed for: a set_var inside a non-test helper"
status: open
last_verified_commit: 70a47aaed
criteria:
  - id: c1
    text: "Either the lint audits writes inside helper functions reachable from a test (the enclosing-fn machinery and closure() are already present), or the exclusion is stated where the criterion is GRADED rather than only in a ci.yml comment"
    state: met
    evidence: 'file:scripts/check-test-env-globals.py:517:kind = "UNSERIALIZED-HELPER"'
    owner: core
    note: "MET AS WRITTEN by the FIRST branch, and RE-GRADED rather than built: the caller-closure pass landed on integ/f13 ahead of this lane, so the state this entry recorded ('nothing has been done', at 9de21aa1) was stale. Verified against the tree, not against the note. `scan()` now resolves each helper write to its callers by attribution key (impl block -> Type, free fn -> its uniquely-declared name), follows a helper caller one level further, and a key with even one UNSERIALIZED-TEST caller becomes UNSERIALIZED-HELPER, which FAILS exactly like a direct write. Measured on the real tree: the gate prints 'helper writes AUDITED by caller (#1134 c3): 139 reached only serialized callers, 14 reached an unserialized test' -- the 153-site 'helper' bucket the issue counted no longer exists as an excused class. The residual exclusions are STATED in the module docstring under WHAT IT DOES NOT CHECK (production-only callers, single-test binaries, benches/examples, unattributable keys), i.e. beside the code that is graded, not only in a ci.yml comment."
  - id: c2
    text: "--self-test carries a helper fixture in BOTH directions, so the classifier's blind spot cannot be reintroduced silently"
    state: met
    evidence: "file:scripts/check-test-env-globals.py:579:_HELPER_SERIAL = ("
    owner: core
    note: "MET AS WRITTEN. `_HELPER_UNSERIAL` (the write moved one call deep into an RAII guard, caller NOT serialized) must FIRE and `_HELPER_SERIAL` (the identical guard, caller serialized) must stay QUIET -- the same hazard in both, differing only in the caller's attribute, so a classifier that ignored helpers again would fail the first and a classifier that convicted every helper would fail the second. Two further residue arms bound it: `_HELPER_UNCALLED` (a guard nothing constructs) stays quiet, and the debt-file pair proves a line written for one SITE does not excuse a second helper writing the same var in the same binary. Run on this tree: `python3 scripts/check-test-env-globals.py --self-test` exits 0 with 17 arms, 'self-test: both directions proven'."
  - id: c3
    text: "Removing #[serial_test::serial] from the caller of PinnedRetryBudget::pin at engine.rs:29581 makes the lint exit non-zero -- or #1134 c3's text is restated to what the lint actually catches"
    state: met
    evidence: "file:crates/wcore-agent/src/engine.rs:31122:async fn stream_error_exhausts_retries_then_fails_the_turn()"
    owner: core
    note: "MET AS WRITTEN, by direct measurement, first branch. LINE DRIFT: the caller the issue records at engine.rs:29581 is now at engine.rs:30635 (`stream_error_exhausts_retries_then_fails_the_turn`, its `#[serial_test::serial]` on :30634); the n-window merge moved engine.rs and the criterion text was written before it. It is the same caller -- the sole `PinnedRetryBudget::pin` call in engine.rs. RED ARM, exit codes captured directly, not through a pipe: baseline `python3 scripts/check-test-env-globals.py` exits 0; deleting line 30634 and re-running exits 1, naming 'crates/wcore-agent/src/test_utils/mod.rs:387  fn pin  [helper write, reached from an unserialized test; caller kinds seen: UNSERIALIZED-TEST,serial-attr]' plus both Drop sites, under 'WAYLAND_MAX_STREAM_RETRIES written by test binary wcore-agent (2587 tests in that process)'. Restored with `git checkout --` and `touch`; the restored run exits 0 again."
---

The shipped CI lint `scripts/check-test-env-globals.py` is structurally blind to the defect shape #1134 was filed about, and nothing anywhere says so. It fails only on kind UNSERIALIZED-TEST (a `set_var` lexically inside a fn carrying a test attribute). A write inside a non-test helper is classified 'helper' and never audited - 153 sites today, including `PinnedRetryBudget::pin` itself, the helper in the issue's opening paragraph. MEASURED: removing `#[serial_test::serial]` from its caller at crates/wcore-agent/src/engine.rs:29581 makes `cargo test -p wcore-agent --lib` fail with the original 3-vs-11 signature, while the lint returns exit 0 and emits byte-identical counts {'serial-attr': 459, 'helper': 153, 'UNSERIALIZED-TEST': 49, 'lock-guarded': 20}. The script's own `--self-test` cannot expose this: all 7 fixtures write directly inside a test fn, so there is no helper case in either direction.

**Where.** scripts/check-test-env-globals.py (classifier in `scan()`, ~line 318; `self_test()` at line 437) and the ci.yml:1370-1399 comment block that describes it

**Why it matters.** The ci.yml comment argues the narrowing is deliberate and correct, and it may be - but the criterion it is graded against says 'a lint catches the class', and a reader of the ledger or the closing comment will believe new instances of the ticket's own shape get caught at lint time. They do not; they are caught only by the 90-minute lib leg, if at all. Either add a helper-call-closure pass (the enclosing-fn machinery and the dependency `closure()` are already there) plus a helper fixture to `--self-test`, or state the exclusion where it is graded rather than only in a workflow comment.

Criteria are taken verbatim from the issue's Acceptance section. Nothing has been done: this entry exists so the release gate counts the work rather than anyone having to remember it.
