---
issue: 1197
repo: FerroxLabs/wayland
kind: defect
title: "The #1134 CI lint is structurally blind to the defect shape it was filed for: a set_var inside a non-test helper"
status: open
last_verified_commit: 9de21aa1
criteria:
  - id: c1
    text: "Either the lint audits writes inside helper functions reachable from a test (the enclosing-fn machinery and closure() are already present), or the exclusion is stated where the criterion is GRADED rather than only in a ci.yml comment"
    state: not-met
    owner: core
    note: "Filed 2026-08-29 by the 0.13.12 close-sweep (worklist defect D14, found while verifying wayland#1134). Nothing has been done. The measured finding, verbatim: The shipped CI lint `scripts/check-test-env-globals.py` is structurally blind to the defect shape #1134 was filed about, and nothing anywhere says so. It fails only on kind UNSERIALIZED-TEST (a `set_var` lexically inside a fn carrying a test attribute). A write inside a non-test helper is classified 'helper' and never audited - 153 sites today, including `PinnedRetryBudget::pin` itself, the helper in the issue's opening paragraph. MEASURED: removing `#[serial_test::serial]` from its caller at crates/wcore-agent/src/engine.rs:29581 makes `cargo test -p wcore-agent --lib` fail with the original 3-vs-11 signature, while the lint returns exit 0 and emits byte-identical counts {'serial-attr': 459, 'helper': 153, 'UNSERIALIZED-TEST': 49, 'lock-guarded': 20}. The script's own `--self-test` cannot expose this: all 7 fixtures write directly inside a test fn, so there is no helper case in either direction."
  - id: c2
    text: "--self-test carries a helper fixture in BOTH directions, so the classifier's blind spot cannot be reintroduced silently"
    state: not-met
    owner: core
    note: "Filed 2026-08-29 by the 0.13.12 close-sweep (worklist defect D14). Nothing has been done; the measurement is on c1 and the file:line anchors are in the prose below."
  - id: c3
    text: "Removing #[serial_test::serial] from the caller of PinnedRetryBudget::pin at engine.rs:29581 makes the lint exit non-zero -- or #1134 c3's text is restated to what the lint actually catches"
    state: not-met
    owner: core
    note: "Filed 2026-08-29 by the 0.13.12 close-sweep (worklist defect D14). Nothing has been done; the measurement is on c1 and the file:line anchors are in the prose below."
---

The shipped CI lint `scripts/check-test-env-globals.py` is structurally blind to the defect shape #1134 was filed about, and nothing anywhere says so. It fails only on kind UNSERIALIZED-TEST (a `set_var` lexically inside a fn carrying a test attribute). A write inside a non-test helper is classified 'helper' and never audited - 153 sites today, including `PinnedRetryBudget::pin` itself, the helper in the issue's opening paragraph. MEASURED: removing `#[serial_test::serial]` from its caller at crates/wcore-agent/src/engine.rs:29581 makes `cargo test -p wcore-agent --lib` fail with the original 3-vs-11 signature, while the lint returns exit 0 and emits byte-identical counts {'serial-attr': 459, 'helper': 153, 'UNSERIALIZED-TEST': 49, 'lock-guarded': 20}. The script's own `--self-test` cannot expose this: all 7 fixtures write directly inside a test fn, so there is no helper case in either direction.

**Where.** scripts/check-test-env-globals.py (classifier in `scan()`, ~line 318; `self_test()` at line 437) and the ci.yml:1370-1399 comment block that describes it

**Why it matters.** The ci.yml comment argues the narrowing is deliberate and correct, and it may be - but the criterion it is graded against says 'a lint catches the class', and a reader of the ledger or the closing comment will believe new instances of the ticket's own shape get caught at lint time. They do not; they are caught only by the 90-minute lib leg, if at all. Either add a helper-call-closure pass (the enclosing-fn machinery and the dependency `closure()` are already there) plus a helper fixture to `--self-test`, or state the exclusion where it is graded rather than only in a workflow comment.

Criteria are taken verbatim from the issue's Acceptance section. Nothing has been done: this entry exists so the release gate counts the work rather than anyone having to remember it.
